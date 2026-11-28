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
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};

/// Maximum wire frame size (16 MiB).
pub const MAX_WIRE_FRAME: usize = 16 * 1024 * 1024;

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
    let _daemon_lock = DaemonLock::try_acquire(socket_path)?;
    if socket_path.exists() {
        let _ = fs::remove_file(socket_path);
    }
    let listener = UnixListener::bind(socket_path)?;
    let _ = fs::set_permissions(socket_path, fs::Permissions::from_mode(0o600));
    loop {
        let (stream, _) = listener.accept().await?;
        let disp = Arc::clone(&dispatcher);
        tokio::spawn(async move {
            let _ = handle_connection(stream, disp).await;
        });
    }
}

async fn handle_connection(
    mut stream: UnixStream,
    dispatcher: Arc<dyn AppDispatcher>,
) -> Result<(), IpcError> {
    loop {
        let mut len_buf = [0u8; 4];
        match stream.read_exact(&mut len_buf).await {
            Ok(_) => {}
            Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(()),
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
}
