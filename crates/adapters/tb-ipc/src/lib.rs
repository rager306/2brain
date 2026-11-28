//! Driving/Driven adapter: Unix-socket IPC daemon server and client with big-endian `u32`
//! length-prefixed JSON framing (max 16 MiB) compatible with the Black-Box TCK and G4 wire tests.

use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream as StdUnixStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use fs4::fs_std::FileExt;
use serde_json::{Map, Value, json};
use tb_app::{AppDispatcher, AppError, IdempotentOutcome};
use tb_contracts::Locator;
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, broadcast};
use tokio_util::sync::CancellationToken;

/// Maximum wire frame size (16 MiB).
pub const MAX_WIRE_FRAME: usize = 16 * 1024 * 1024;
const IPC_EVENT_BUFFER: usize = 512;
const MAX_EVENT_SUBSCRIBERS: usize = 64;
const EVENT_WRITE_TIMEOUT: Duration = Duration::from_secs(5);
const BRIDGE_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
const BRIDGE_INITIAL_BACKOFF: Duration = Duration::from_millis(200);
const BRIDGE_MAX_BACKOFF: Duration = Duration::from_secs(5);
const BRIDGE_UNSUPPORTED_BACKOFF: Duration = Duration::from_secs(30);

/// Broadcast hub for workspace note change events streamed from `2brain daemon`
/// to connected Unix-socket clients (`{"subscribe": "resources"}`).
#[derive(Clone, Debug)]
pub struct IpcEventHub {
    tx: broadcast::Sender<Locator>,
}

impl Default for IpcEventHub {
    fn default() -> Self {
        Self::new()
    }
}

impl IpcEventHub {
    #[must_use]
    pub fn new() -> Self {
        let (tx, _rx) = broadcast::channel(IPC_EVENT_BUFFER);
        Self { tx }
    }

    /// Publish a single changed workspace [`Locator`].
    pub fn publish_locator(&self, locator: Locator) {
        let _ = self.tx.send(locator);
    }

    /// Publish a batch of changed workspace [`Locator`]s.
    pub fn publish_locators(&self, locators: &[Locator]) {
        for loc in locators {
            let _ = self.tx.send(loc.clone());
        }
    }

    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<Locator> {
        self.tx.subscribe()
    }
}

#[derive(Debug, Error)]
pub enum IpcError {
    #[error("IPC I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("IPC protocol error: {0}")]
    Protocol(String),
    #[error("daemon already running for socket: {0}")]
    AlreadyRunning(PathBuf),
}

/// Resolve the runtime socket directory (`$2BRAIN_HOME/engine/run` or `$HOME/.2brain/engine/run`).
#[must_use]
pub fn run_dir() -> PathBuf {
    if let Ok(home) = std::env::var("2BRAIN_HOME")
        && !home.is_empty()
    {
        return PathBuf::from(home).join("engine").join("run");
    }
    if let Ok(home) = std::env::var("HOME")
        && !home.is_empty()
    {
        return PathBuf::from(home)
            .join(".2brain")
            .join("engine")
            .join("run");
    }
    PathBuf::from("/tmp/.2brain/engine/run")
}

/// Deterministic Unix socket path for `canonical_workspace_root` (64-bit FNV-1a hex,
/// matching existing client/daemon socket conventions).
#[must_use]
pub fn socket_path_for(canonical_workspace_root: &Path) -> PathBuf {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in canonical_workspace_root.to_string_lossy().as_bytes() {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    run_dir().join(format!("{hash:016x}.sock"))
}

/// Exclusive daemon ownership lock preventing a second daemon from hijacking a live socket.
pub struct DaemonLock {
    _file: fs::File,
}

impl DaemonLock {
    pub fn try_acquire(socket_path: &Path) -> Result<Self, IpcError> {
        if let Some(parent) = socket_path.parent() {
            fs::create_dir_all(parent)?;
            let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
        }
        let lock_path = socket_path.with_extension("lock");
        if let Ok(meta) = fs::symlink_metadata(&lock_path)
            && meta.file_type().is_symlink()
        {
            return Err(IpcError::Protocol(format!(
                "refusing symlink daemon lock: {}",
                lock_path.display()
            )));
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)?;
        match file.try_lock_exclusive() {
            Ok(()) => Ok(Self { _file: file }),
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                Err(IpcError::AlreadyRunning(socket_path.to_path_buf()))
            }
            Err(err) => Err(IpcError::Io(err)),
        }
    }
}

/// Serve big-endian `u32` framed requests over a Unix domain socket until cancelled/shutdown.
pub async fn serve_unix_socket(
    socket_path: &Path,
    dispatcher: Arc<dyn AppDispatcher>,
) -> Result<(), IpcError> {
    serve_unix_socket_with_events(socket_path, dispatcher, None).await
}

/// Serve big-endian `u32` framed requests and optional push resource update streams
/// (`{"subscribe": "resources"}`) over a Unix domain socket until cancelled/shutdown.
pub async fn serve_unix_socket_with_events(
    socket_path: &Path,
    dispatcher: Arc<dyn AppDispatcher>,
    events: Option<IpcEventHub>,
) -> Result<(), IpcError> {
    serve_unix_socket_inner(socket_path, dispatcher, events, MAX_EVENT_SUBSCRIBERS).await
}

async fn serve_unix_socket_inner(
    socket_path: &Path,
    dispatcher: Arc<dyn AppDispatcher>,
    events: Option<IpcEventHub>,
    max_event_subscribers: usize,
) -> Result<(), IpcError> {
    let _daemon_lock = DaemonLock::try_acquire(socket_path)?;
    if fs::symlink_metadata(socket_path).is_ok() {
        let _ = fs::remove_file(socket_path);
    }
    let listener = UnixListener::bind(socket_path)?;
    let _ = fs::set_permissions(socket_path, fs::Permissions::from_mode(0o600));
    let sub_slots = Arc::new(Semaphore::new(max_event_subscribers));
    let shutdown = CancellationToken::new();
    let _shutdown_guard = shutdown.clone().drop_guard();
    loop {
        let (stream, _) = listener.accept().await?;
        let disp = Arc::clone(&dispatcher);
        let ev = events.clone();
        let slots = Arc::clone(&sub_slots);
        let cancel = shutdown.clone();
        tokio::spawn(async move {
            let _ = handle_connection(stream, disp, ev, slots, cancel).await;
        });
    }
}

async fn handle_connection(
    mut stream: UnixStream,
    dispatcher: Arc<dyn AppDispatcher>,
    events: Option<IpcEventHub>,
    sub_slots: Arc<Semaphore>,
    shutdown: CancellationToken,
) -> Result<(), IpcError> {
    loop {
        let body = tokio::select! {
            () = shutdown.cancelled() => return Ok(()),
            res = read_async_frame(&mut stream) => match res? {
                Some(b) => b,
                None => return Ok(()),
            }
        };

        let req: Value = match serde_json::from_slice(&body) {
            Ok(v) => v,
            Err(err) => {
                let reply = json!({
                    "type": "error",
                    "id": 0,
                    "error": format!("malformed JSON request: {err}"),
                });
                write_async_frame(&mut stream, &reply).await?;
                continue;
            }
        };

        if let Some(obj) = req.as_object()
            && obj.get("read").is_none()
            && obj.get("mutation").is_none()
            && let Some(stream_kind) = obj.get("subscribe").and_then(Value::as_str)
        {
            let id = obj.get("id").cloned().unwrap_or_else(|| json!(1));
            if stream_kind != "resources" {
                let reply = json!({
                    "type": "error",
                    "id": id,
                    "error": format!("unsupported subscribe stream '{stream_kind}'"),
                });
                let _ = tokio::time::timeout(
                    EVENT_WRITE_TIMEOUT,
                    write_async_frame(&mut stream, &reply),
                )
                .await;
                return Ok(());
            }
            let Some(ref hub) = events else {
                let reply = json!({
                    "type": "error",
                    "id": id,
                    "error": "resource event stream unavailable",
                });
                let _ = tokio::time::timeout(
                    EVENT_WRITE_TIMEOUT,
                    write_async_frame(&mut stream, &reply),
                )
                .await;
                return Ok(());
            };
            let Ok(permit) = sub_slots.clone().try_acquire_owned() else {
                let reply = json!({
                    "type": "error",
                    "id": id,
                    "error": "policy/subscription_quota: too many active IPC resource subscribers",
                });
                let _ = tokio::time::timeout(
                    EVENT_WRITE_TIMEOUT,
                    write_async_frame(&mut stream, &reply),
                )
                .await;
                return Ok(());
            };
            // Subscribe BEFORE writing the ack frame so no event emitted after ack can be missed.
            let rx = hub.subscribe();
            let ack = json!({
                "type": "subscribed",
                "id": id,
                "stream": "resources",
            });
            tokio::time::timeout(EVENT_WRITE_TIMEOUT, write_async_frame(&mut stream, &ack))
                .await
                .map_err(|_| IpcError::Protocol("timed out writing subscribe ack".into()))??;
            return stream_resource_events_to_client(stream, rx, permit, shutdown).await;
        }

        let disp = Arc::clone(&dispatcher);
        let reply = tokio::task::spawn_blocking(move || dispatch_wire_frame(disp.as_ref(), &req))
            .await
            .unwrap_or_else(|e| {
                json!({
                    "type": "error",
                    "id": 0,
                    "error": format!("worker task panicked: {e}"),
                })
            });

        write_async_frame(&mut stream, &reply).await?;
    }
}

async fn read_async_frame(stream: &mut UnixStream) -> Result<Option<Vec<u8>>, IpcError> {
    let mut len_buf = [0u8; 4];
    match stream.read_exact(&mut len_buf).await {
        Ok(_) => {}
        Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(err) => return Err(IpcError::Io(err)),
    }
    let len = u32::from_be_bytes(len_buf) as usize;
    if len == 0 || len > MAX_WIRE_FRAME {
        return Err(IpcError::Protocol(format!(
            "invalid wire frame length {len}"
        )));
    }
    let mut body = vec![0u8; len];
    stream.read_exact(&mut body).await?;
    Ok(Some(body))
}

async fn stream_resource_events_to_client(
    mut stream: UnixStream,
    mut rx: broadcast::Receiver<Locator>,
    _permit: OwnedSemaphorePermit,
    shutdown: CancellationToken,
) -> Result<(), IpcError> {
    let mut probe_buf = [0u8; 16];
    loop {
        tokio::select! {
            () = shutdown.cancelled() => return Ok(()),
            recv = rx.recv() => match recv {
                Ok(loc) => {
                    let frame = json!({
                        "type": "event",
                        "stream": "resources",
                        "repo": loc.repo,
                        "path": loc.path,
                    });
                    match tokio::time::timeout(
                        EVENT_WRITE_TIMEOUT,
                        write_async_frame(&mut stream, &frame),
                    ).await {
                        Ok(Ok(())) => {}
                        _ => return Ok(()),
                    }
                }
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    let frame = json!({
                        "type": "event",
                        "stream": "resources",
                        "lagged": skipped,
                    });
                    match tokio::time::timeout(
                        EVENT_WRITE_TIMEOUT,
                        write_async_frame(&mut stream, &frame),
                    ).await {
                        Ok(Ok(())) => {}
                        _ => return Ok(()),
                    }
                }
                Err(broadcast::error::RecvError::Closed) => return Ok(()),
            },
            // Detect peer disconnect (or unexpected inbound bytes on a push-only socket) immediately.
            read_res = stream.read(&mut probe_buf) => match read_res {
                Ok(0) | Err(_) => return Ok(()),
                Ok(_) => return Ok(()),
            }
        }
    }
}

/// Connect and perform the `{"subscribe": "resources"}` handshake against a daemon Unix socket.
/// Returns the subscribed [`UnixStream`] on success, or an [`IpcError`] if the daemon is
/// unreachable, older/unsupported, or at subscriber quota.
pub async fn connect_resource_event_stream(socket_path: &Path) -> Result<UnixStream, IpcError> {
    let mut stream =
        tokio::time::timeout(BRIDGE_HANDSHAKE_TIMEOUT, UnixStream::connect(socket_path))
            .await
            .map_err(|_| {
                IpcError::Protocol("timed out connecting to daemon event stream".into())
            })??;
    let sub_req = json!({ "subscribe": "resources", "id": 1 });
    let raw_ack = tokio::time::timeout(BRIDGE_HANDSHAKE_TIMEOUT, async {
        write_async_frame(&mut stream, &sub_req).await?;
        read_async_frame(&mut stream).await
    })
    .await
    .map_err(|_| IpcError::Protocol("timed out waiting for subscribe ack".into()))??
    .ok_or_else(|| IpcError::Protocol("daemon closed connection before subscribe ack".into()))?;

    let val: Value = serde_json::from_slice(&raw_ack)
        .map_err(|e| IpcError::Protocol(format!("malformed subscribe ack JSON: {e}")))?;
    let Some(obj) = val.as_object() else {
        return Err(IpcError::Protocol(
            "subscribe ack must be a JSON object".into(),
        ));
    };
    if obj.get("type").and_then(Value::as_str) == Some("subscribed")
        && obj.get("stream").and_then(Value::as_str) == Some("resources")
    {
        return Ok(stream);
    }
    let err_msg = obj
        .get("error")
        .and_then(Value::as_str)
        .unwrap_or("daemon rejected resource subscription");
    Err(IpcError::Protocol(err_msg.to_owned()))
}

/// Connect to a `2brain daemon` Unix socket, subscribe to its `resources` push stream,
/// validate every inbound [`Locator`] fail-closed, and invoke `on_locator`.
/// Reconnects with bounded exponential backoff across daemon restarts until `cancel` triggers.
pub fn spawn_resource_event_bridge<F>(
    socket_path: PathBuf,
    cancel: CancellationToken,
    on_locator: F,
) -> tokio::task::JoinHandle<()>
where
    F: Fn(Locator) + Send + Sync + 'static,
{
    spawn_resource_event_bridge_with_initial(socket_path, None, cancel, on_locator)
}

/// Same as [`spawn_resource_event_bridge`], starting from an already-handshaked [`UnixStream`]
/// returned by [`connect_resource_event_stream`] so capability checks do not consume a second slot.
pub fn spawn_resource_event_bridge_with_initial<F>(
    socket_path: PathBuf,
    mut initial_stream: Option<UnixStream>,
    cancel: CancellationToken,
    on_locator: F,
) -> tokio::task::JoinHandle<()>
where
    F: Fn(Locator) + Send + Sync + 'static,
{
    tokio::spawn(async move {
        let mut backoff = BRIDGE_INITIAL_BACKOFF;
        loop {
            if cancel.is_cancelled() {
                break;
            }
            let mut stream = if let Some(s) = initial_stream.take() {
                backoff = BRIDGE_INITIAL_BACKOFF;
                s
            } else {
                let connect_res = tokio::select! {
                    () = cancel.cancelled() => break,
                    res = connect_resource_event_stream(&socket_path) => res,
                };
                match connect_res {
                    Ok(s) => {
                        backoff = BRIDGE_INITIAL_BACKOFF;
                        s
                    }
                    Err(IpcError::Protocol(_)) => {
                        // Older daemon or quota refusal — avoid tight reconnect loops.
                        if sleep_or_cancel(&cancel, BRIDGE_UNSUPPORTED_BACKOFF).await {
                            break;
                        }
                        continue;
                    }
                    Err(_) => {
                        if sleep_or_cancel(&cancel, backoff).await {
                            break;
                        }
                        backoff = (backoff * 2).min(BRIDGE_MAX_BACKOFF);
                        continue;
                    }
                }
            };

            loop {
                let next_frame = tokio::select! {
                    () = cancel.cancelled() => return,
                    res = read_async_frame(&mut stream) => res,
                };
                match next_frame {
                    Ok(Some(bytes)) => {
                        let Ok(val) = serde_json::from_slice::<Value>(&bytes) else {
                            // Fail closed on corrupt frames: drop stream and reconnect.
                            break;
                        };
                        let Some(obj) = val.as_object() else {
                            break;
                        };
                        if obj.get("type").and_then(Value::as_str) != Some("event")
                            || obj.get("stream").and_then(Value::as_str) != Some("resources")
                        {
                            break;
                        }
                        if obj.get("lagged").and_then(Value::as_u64).is_some() {
                            // Valid server backpressure indicator.
                            continue;
                        }
                        let (Some(repo), Some(path)) = (
                            obj.get("repo").and_then(Value::as_str),
                            obj.get("path").and_then(Value::as_str),
                        ) else {
                            break;
                        };
                        match Locator::new(repo, path) {
                            Ok(loc) => on_locator(loc),
                            Err(_) => break,
                        }
                    }
                    Ok(None) | Err(_) => break,
                }
            }

            if sleep_or_cancel(&cancel, backoff).await {
                break;
            }
            backoff = (backoff * 2).min(BRIDGE_MAX_BACKOFF);
        }
    })
}

async fn sleep_or_cancel(cancel: &CancellationToken, duration: Duration) -> bool {
    tokio::select! {
        () = cancel.cancelled() => true,
        () = tokio::time::sleep(duration) => false,
    }
}

async fn write_async_frame(stream: &mut UnixStream, value: &Value) -> Result<(), IpcError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|e| IpcError::Protocol(format!("failed to serialize response: {e}")))?;
    if bytes.len() > MAX_WIRE_FRAME {
        return Err(IpcError::Protocol(format!(
            "wire response exceeds max frame size: {}",
            bytes.len()
        )));
    }
    let header = (bytes.len() as u32).to_be_bytes();
    stream.write_all(&header).await?;
    stream.write_all(&bytes).await?;
    stream.flush().await?;
    Ok(())
}

fn dispatch_wire_frame(dispatcher: &dyn AppDispatcher, req: &Value) -> Value {
    let Some(obj) = req.as_object() else {
        return json!({ "type": "error", "id": 0, "error": "request must be a JSON object" });
    };
    let id = obj.get("id").cloned().unwrap_or_else(|| json!(1));

    if let Some(read_verb) = obj.get("read").and_then(Value::as_str) {
        return match dispatcher.call_read(read_verb, obj) {
            Ok(result) => json!({
                "type": "response",
                "id": id,
                "ready": true,
                "result": result,
            }),
            Err(err) => error_frame(&id, &err),
        };
    }

    if let Some(mut_verb) = obj.get("mutation").and_then(Value::as_str) {
        return match dispatcher.call_mutation(mut_verb, obj) {
            Ok(outcome) => {
                let mut frame = Map::new();
                frame.insert("type".into(), json!("response"));
                frame.insert("id".into(), id);
                frame.insert("ready".into(), json!(true));
                frame.insert("result".into(), outcome.payload);
                if let Some(state) = outcome.outcome_state {
                    frame.insert("outcome_state".into(), json!(state));
                }
                if outcome.replayed {
                    frame.insert("replayed".into(), json!(true));
                }
                Value::Object(frame)
            }
            Err(err) => error_frame(&id, &err),
        };
    }

    json!({
        "type": "error",
        "id": id,
        "error": "wire frame must specify either 'read' or 'mutation'",
    })
}

fn error_frame(id: &Value, err: &AppError) -> Value {
    let mut frame = Map::new();
    frame.insert("type".into(), json!("error"));
    frame.insert("id".into(), id.clone());
    frame.insert("error".into(), json!(err.to_string()));
    if let Some(state) = err.outcome_state() {
        frame.insert("outcome_state".into(), json!(state));
    }
    Value::Object(frame)
}

/// Blocking Unix-socket IPC client implementing [`AppDispatcher`] for thin `2brain mcp <socket>`.
#[derive(Clone, Debug)]
pub struct IpcClient {
    socket_path: PathBuf,
    timeout: Duration,
}

impl IpcClient {
    #[must_use]
    pub fn new(socket_path: PathBuf) -> Self {
        Self {
            socket_path,
            timeout: Duration::from_secs(60),
        }
    }

    fn exchange(&self, frame: &Value) -> Result<Value, AppError> {
        let body = serde_json::to_vec(frame)
            .map_err(|e| AppError::Io(format!("serialize IPC request: {e}")))?;
        if body.len() > MAX_WIRE_FRAME {
            return Err(AppError::InvalidArguments(
                "IPC request exceeds 16 MiB".into(),
            ));
        }
        let mut stream = StdUnixStream::connect(&self.socket_path).map_err(|e| {
            AppError::Io(format!(
                "failed to connect to daemon socket '{}': {e}",
                self.socket_path.display()
            ))
        })?;
        let _ = stream.set_read_timeout(Some(self.timeout));
        let _ = stream.set_write_timeout(Some(self.timeout));

        let header = (body.len() as u32).to_be_bytes();
        stream
            .write_all(&header)
            .and_then(|()| stream.write_all(&body))
            .and_then(|()| stream.flush())
            .map_err(|e| AppError::Io(format!("IPC write failed: {e}")))?;

        let mut resp_header = [0u8; 4];
        stream
            .read_exact(&mut resp_header)
            .map_err(|e| AppError::Io(format!("IPC read header failed: {e}")))?;
        let resp_len = u32::from_be_bytes(resp_header) as usize;
        if resp_len == 0 || resp_len > MAX_WIRE_FRAME {
            return Err(AppError::Io(format!(
                "invalid IPC response length {resp_len}"
            )));
        }
        let mut resp_body = vec![0u8; resp_len];
        stream
            .read_exact(&mut resp_body)
            .map_err(|e| AppError::Io(format!("IPC read body failed: {e}")))?;
        serde_json::from_slice(&resp_body)
            .map_err(|e| AppError::Io(format!("malformed IPC response JSON: {e}")))
    }
}

impl AppDispatcher for IpcClient {
    fn call_read(&self, verb: &str, args: &Map<String, Value>) -> Result<Value, AppError> {
        let mut req = args.clone();
        req.insert("read".into(), json!(verb));
        req.insert("id".into(), json!(1));
        let reply = self.exchange(&Value::Object(req))?;
        if reply.get("type").and_then(Value::as_str) == Some("response") {
            return Ok(reply.get("result").cloned().unwrap_or(Value::Null));
        }
        Err(decode_ipc_error(&reply))
    }

    fn call_mutation(
        &self,
        verb: &str,
        args: &Map<String, Value>,
    ) -> Result<IdempotentOutcome, AppError> {
        let mut req = args.clone();
        req.insert("mutation".into(), json!(verb));
        req.insert("id".into(), json!(1));
        let reply = self.exchange(&Value::Object(req))?;
        if reply.get("type").and_then(Value::as_str) == Some("response") {
            let outcome_state = match reply.get("outcome_state").and_then(Value::as_str) {
                Some("committed") => Some("committed"),
                Some("rejected") => Some("rejected"),
                Some("compensated") => Some("compensated"),
                _ => None,
            };
            let replayed = reply
                .get("replayed")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            return Ok(IdempotentOutcome {
                payload: reply.get("result").cloned().unwrap_or(Value::Null),
                outcome_state,
                replayed,
            });
        }
        Err(decode_ipc_error(&reply))
    }
}

fn decode_ipc_error(reply: &Value) -> AppError {
    let msg = reply
        .get("error")
        .and_then(Value::as_str)
        .unwrap_or("daemon error")
        .to_owned();
    match reply.get("outcome_state").and_then(Value::as_str) {
        Some("quota") => AppError::OutcomeQuota(msg),
        Some("conflict") => AppError::OutcomeConflict(msg),
        _ => {
            if msg.contains("expected_hash_mismatch") {
                AppError::CasMismatch(msg)
            } else if msg.contains("symlink") {
                AppError::SymlinkEscape(msg)
            } else if msg.contains("invalid_utf8") {
                AppError::InvalidUtf8(msg)
            } else {
                AppError::Io(msg)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_daemon_lock_fails_closed_without_hijacking_socket() {
        let dir = std::env::temp_dir().join(format!("tb-ipc-lock-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let sock = dir.join("ws.sock");
        let first = DaemonLock::try_acquire(&sock).expect("first daemon acquires lock");
        let second = DaemonLock::try_acquire(&sock);
        assert!(matches!(second, Err(IpcError::AlreadyRunning(_))));
        drop(first);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn socket_path_uses_deterministic_16_hex_fnv1a64_name() {
        let p = socket_path_for(Path::new("/tmp/sample-vault"));
        let fname = p.file_name().and_then(|n| n.to_str()).unwrap();
        assert!(fname.ends_with(".sock"));
        let stem = fname.strip_suffix(".sock").unwrap();
        assert_eq!(stem.len(), 16);
        assert!(stem.bytes().all(|b| b.is_ascii_hexdigit()));
    }

    struct DummyDispatcher;

    impl AppDispatcher for DummyDispatcher {
        fn call_read(&self, _verb: &str, _args: &Map<String, Value>) -> Result<Value, AppError> {
            Ok(json!({ "ok": true }))
        }

        fn call_mutation(
            &self,
            _verb: &str,
            _args: &Map<String, Value>,
        ) -> Result<IdempotentOutcome, AppError> {
            Ok(IdempotentOutcome {
                payload: json!({ "ok": true }),
                outcome_state: Some("committed"),
                replayed: false,
            })
        }
    }

    #[tokio::test]
    async fn ipc_resource_event_stream_delivers_locators_and_reconnects_after_restart() {
        let dir = std::env::temp_dir().join(format!("tb-ipc-events-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let sock = dir.join("events.sock");

        let hub1 = IpcEventHub::new();
        let disp: Arc<dyn AppDispatcher> = Arc::new(DummyDispatcher);
        let sock1 = sock.clone();
        let hub1_srv = hub1.clone();
        let disp1 = Arc::clone(&disp);
        let srv1 = tokio::spawn(async move {
            let _ = serve_unix_socket_with_events(&sock1, disp1, Some(hub1_srv)).await;
        });

        for _ in 0..40 {
            if sock.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let cancel = CancellationToken::new();
        let bridge = spawn_resource_event_bridge(sock.clone(), cancel.clone(), move |loc| {
            let _ = tx.send(loc);
        });

        // Give bridge a moment to complete handshake, then publish first locator.
        tokio::time::sleep(Duration::from_millis(120)).await;
        let loc1 = Locator::new("vault", "notes/first.md").unwrap();
        hub1.publish_locator(loc1.clone());

        let got1 = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("first locator timeout")
            .expect("first locator received");
        assert_eq!(got1, loc1);

        // Simulate daemon restart on the exact same socket path.
        srv1.abort();
        let _ = srv1.await;
        tokio::time::sleep(Duration::from_millis(50)).await;

        let hub2 = IpcEventHub::new();
        let sock2 = sock.clone();
        let hub2_srv = hub2.clone();
        let srv2 = tokio::spawn(async move {
            let _ = serve_unix_socket_with_events(&sock2, disp, Some(hub2_srv)).await;
        });

        // Wait for bridge backoff reconnect and publish second locator.
        let loc2 = Locator::new("vault", "notes/second.md").unwrap();
        let mut got2 = None;
        for _ in 0..20 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            hub2.publish_locator(loc2.clone());
            if let Ok(Some(loc)) = tokio::time::timeout(Duration::from_millis(150), rx.recv()).await
            {
                got2 = Some(loc);
                break;
            }
        }
        assert_eq!(got2, Some(loc2));

        cancel.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(2), bridge).await;
        srv2.abort();
        let _ = srv2.await;
        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn ipc_subscribe_fails_closed_when_disabled_or_quota_exceeded() {
        let dir = std::env::temp_dir().join(format!("tb-ipc-quota-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        // 1. Disabled events -> explicit error frame
        let sock_off = dir.join("off.sock");
        let disp: Arc<dyn AppDispatcher> = Arc::new(DummyDispatcher);
        let sock_off_c = sock_off.clone();
        let disp_c = Arc::clone(&disp);
        let srv_off = tokio::spawn(async move {
            let _ = serve_unix_socket(&sock_off_c, disp_c).await;
        });
        for _ in 0..40 {
            if sock_off.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let mut s = UnixStream::connect(&sock_off).await.unwrap();
        write_async_frame(&mut s, &json!({ "subscribe": "resources", "id": 7 }))
            .await
            .unwrap();
        let raw = read_async_frame(&mut s).await.unwrap().unwrap();
        let val: Value = serde_json::from_slice(&raw).unwrap();
        assert_eq!(val["type"], "error");
        assert!(val["error"].as_str().unwrap().contains("unavailable"));
        srv_off.abort();

        // 2. Quota = 1 -> second subscriber rejected fail-closed
        let sock_q = dir.join("quota.sock");
        let hub = IpcEventHub::new();
        let sock_q_c = sock_q.clone();
        let srv_q = tokio::spawn(async move {
            let _ = serve_unix_socket_inner(&sock_q_c, disp, Some(hub), 1).await;
        });
        for _ in 0..40 {
            if sock_q.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let mut s1 = UnixStream::connect(&sock_q).await.unwrap();
        write_async_frame(&mut s1, &json!({ "subscribe": "resources", "id": 1 }))
            .await
            .unwrap();
        let ack1: Value =
            serde_json::from_slice(&read_async_frame(&mut s1).await.unwrap().unwrap()).unwrap();
        assert_eq!(ack1["type"], "subscribed");

        let mut s2 = UnixStream::connect(&sock_q).await.unwrap();
        write_async_frame(&mut s2, &json!({ "subscribe": "resources", "id": 2 }))
            .await
            .unwrap();
        let rej2: Value =
            serde_json::from_slice(&read_async_frame(&mut s2).await.unwrap().unwrap()).unwrap();
        assert_eq!(rej2["type"], "error");
        assert!(
            rej2["error"]
                .as_str()
                .unwrap()
                .contains("subscription_quota")
        );

        drop(s1);
        drop(s2);
        srv_q.abort();
        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn read_frame_with_subscribe_argument_is_not_hijacked_and_repo_slash_rejected() {
        assert!(Locator::new("secret/sub", "a.md").is_err());
        assert!(Locator::new("..", "a.md").is_err());

        let dir = std::env::temp_dir().join(format!("tb-ipc-hijack-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let sock = dir.join("hijack.sock");
        let hub = IpcEventHub::new();
        let disp: Arc<dyn AppDispatcher> = Arc::new(DummyDispatcher);
        let sock_c = sock.clone();
        let srv = tokio::spawn(async move {
            let _ = serve_unix_socket_with_events(&sock_c, disp, Some(hub)).await;
        });
        for _ in 0..40 {
            if sock.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        let mut s = UnixStream::connect(&sock).await.unwrap();
        write_async_frame(
            &mut s,
            &json!({ "read": "overview", "subscribe": "resources", "id": 42 }),
        )
        .await
        .unwrap();
        let reply: Value =
            serde_json::from_slice(&read_async_frame(&mut s).await.unwrap().unwrap()).unwrap();
        assert_eq!(reply["type"], "response");
        assert_eq!(reply["id"], 42);
        assert_eq!(reply["result"]["ok"], true);

        srv.abort();
        let _ = fs::remove_dir_all(&dir);
    }
}
