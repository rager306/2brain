//! The engine connection as a port: one request frame in, one answer out.
//!
//! `EngineTransport` is the seam between the adapter and the engine.
//! `UnixTransport` speaks the real wire over a unix socket, and
//! `MemoryTransport` answers from a script so a test can assert what the
//! adapter sent without opening one. Nothing here knows a tool name, a policy,
//! or the shape of a verb's payload; that is `wire.rs`. It does own the one-time
//! report for a launch whose socket never answered, because this is where that
//! failure is first seen and the only place that holds the path that was tried.

use std::{
    path::{Path, PathBuf},
    sync::{
        OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};

use futures::future::BoxFuture;
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

/// The engine wire schema this build speaks.
pub(crate) const SCHEMA_VERSION: u64 = 29;

/// Largest frame either direction may carry.
pub(crate) const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, PartialEq)]
pub(crate) struct ReadResult {
    pub(crate) version: u64,
    pub(crate) payload: Value,
}

/// A framed engine exchange that failed, split by what it proves about a
/// mutation: an engine rejection wrote nothing, while a transport failure may
/// have landed.
pub(crate) enum WireError {
    /// The engine answered with a rejection: nothing was written. `detail` is the
    /// error frame's optional machine-usable context, null when the frame carried
    /// none — a tool that must tell one rejection from another keys on it, never
    /// on the message text.
    Rejected { message: String, detail: Value },
    /// No usable answer: the operation may or may not have happened.
    Broken(String),
}

pub(crate) enum MutationFailure {
    /// The engine rejected the mutation: nothing was written. `detail` as on
    /// [`WireError::Rejected`].
    Rejected { message: String, detail: Value },
    /// No usable answer: the outcome is unknown and a retry could double-apply.
    Unknown(String),
}

impl WireError {
    pub(crate) fn message(self) -> String {
        match self {
            Self::Rejected { message, .. } | Self::Broken(message) => message,
        }
    }
}

/// What the launch resolved, kept so the FIRST failed connection can name the
/// socket it tried and where that path came from — the question `2brain doctor`
/// answers with its own lines. The dispatcher installs it before serving; a
/// server started some other way has none, and then reports only the error.
pub(crate) struct Launch {
    /// Where the socket path came from, for the `state` line.
    pub(crate) origin: String,
    /// What to run or pass instead, for the `next` line.
    pub(crate) next: String,
}

/// The one launch this process serves.
static LAUNCH: OnceLock<Launch> = OnceLock::new();

/// Whether the one-time connection diagnosis has been printed.
static DIAGNOSED: AtomicBool = AtomicBool::new(false);

/// Record what the launch resolved, before the first request.
pub(crate) fn announce_launch(launch: Launch) {
    let _ = LAUNCH.set(launch);
}

/// Name the socket a failed connection tried, where that path came from, and the
/// next step — once per process. A per-call diagnosis is the noise this replaces:
/// every call fails the same way, and only the first one has news.
fn diagnose(socket: &Path, error: &std::io::Error) {
    let Some(launch) = LAUNCH.get() else {
        return;
    };
    if DIAGNOSED.swap(true, Ordering::Relaxed) {
        return;
    }
    eprintln!(
        "2brain: no engine answers this launch's socket, so every tool call fails until one is running"
    );
    eprintln!("2brain:   socket  {} — {error}", socket.display());
    eprintln!("2brain:   state   {}", launch.origin);
    eprintln!("2brain:   next    {}", launch.next);
}

/// One owned, sendable answer per call: the object-safe form of an async
/// transport method, so a `Server` can hold `dyn EngineTransport`.
pub(crate) type TransportFuture = BoxFuture<'static, Result<Value, WireError>>;

/// One frame in, one frame out.
///
/// A read or a mutation is a single exchange. A subscription answers an `ack`
/// and then events, so `watch` consumes the `ack` — exactly as the watch loop
/// always did — and returns the first frame that answers the caller: a
/// `change_event` or an `error`.
pub(crate) trait EngineTransport: Send + Sync {
    /// Send one request frame and return the engine's answering frame.
    fn exchange(&self, request: Value) -> TransportFuture;

    /// Send one subscription request and return the frame that answers it.
    fn watch(&self, request: Value) -> TransportFuture;
}

/// The engine reached over its unix socket, with the framing the engine has
/// always spoken.
pub(crate) struct UnixTransport {
    socket: PathBuf,
}

impl UnixTransport {
    pub(crate) fn new(socket: PathBuf) -> Self {
        Self { socket }
    }
}

impl EngineTransport for UnixTransport {
    fn exchange(&self, request: Value) -> TransportFuture {
        let socket = self.socket.clone();
        Box::pin(async move {
            let data = encode_frame(&request)?;
            let mut stream = connect(&socket).await?;
            write_encoded(&mut stream, &data).await?;
            let response = read_frame(&mut stream).await?;
            validate(&request, response)
        })
    }

    fn watch(&self, request: Value) -> TransportFuture {
        let socket = self.socket.clone();
        Box::pin(async move {
            let data = encode_frame(&request)?;
            let mut stream = connect(&socket).await?;
            write_encoded(&mut stream, &data).await?;
            loop {
                let frame = read_frame(&mut stream).await?;
                // The `ack` and any frame that answers nothing are skipped;
                // the event or the engine's refusal is the answer.
                if matches!(
                    frame.get("type").and_then(Value::as_str),
                    Some("change_event" | "error")
                ) {
                    return Ok(frame);
                }
            }
        })
    }
}

async fn connect(socket: &Path) -> Result<UnixStream, WireError> {
    match UnixStream::connect(socket).await {
        Ok(stream) => Ok(stream),
        Err(error) => {
            diagnose(socket, &error);
            Err(WireError::Broken(format!("connecting to engine: {error}")))
        }
    }
}

/// Refuse an answering frame that does not answer this request. The schema
/// version, the correlation id, and the response envelope are the transport's
/// contract, not a verb's.
fn validate(request: &Value, response: Value) -> Result<Value, WireError> {
    if response.get("schema_version").and_then(Value::as_u64) != Some(SCHEMA_VERSION) {
        return Err(WireError::Broken(format!(
            "unsupported or missing engine schema_version (expected {SCHEMA_VERSION})"
        )));
    }
    if response.get("id") != request.get("id") {
        return Err(WireError::Broken("response id mismatch".into()));
    }
    match response.get("type").and_then(Value::as_str) {
        Some("error") => {
            // The engine may omit `for`; the id check above already tied this
            // frame to our request, so any error frame is a rejection: the
            // engine refused before writing and nothing landed.
            let message = response
                .get("error")
                .and_then(Value::as_str)
                .ok_or_else(|| WireError::Broken("invalid engine error frame".into()))?;
            let detail = response.get("detail").cloned().unwrap_or(Value::Null);
            return Err(WireError::Rejected {
                message: format!("engine error: {message}"),
                detail,
            });
        }
        Some("response") => {}
        _ => return Err(WireError::Broken("unexpected engine frame type".into())),
    }
    match response.get("ready").and_then(Value::as_bool) {
        Some(false) => {
            return Err(WireError::Rejected {
                message: "engine not ready".into(),
                detail: Value::Null,
            });
        }
        Some(true) => {}
        None => return Err(WireError::Broken("invalid engine readiness".into())),
    }
    if response.get("version").and_then(Value::as_u64).is_none() {
        return Err(WireError::Broken(
            "missing or invalid engine version".into(),
        ));
    }
    if response.get("result").and_then(Value::as_object).is_none() {
        return Err(WireError::Broken("invalid result envelope".into()));
    }
    Ok(response)
}

/// Read one engine frame: a big-endian length, then that many JSON bytes.
async fn read_frame(stream: &mut UnixStream) -> Result<Value, WireError> {
    let len = stream
        .read_u32()
        .await
        .map_err(|e| WireError::Broken(format!("reading response length: {e}")))?
        as usize;
    if len > MAX_FRAME_BYTES {
        return Err(WireError::Broken("response frame too large".into()));
    }
    let mut data = vec![0; len];
    stream
        .read_exact(&mut data)
        .await
        .map_err(|e| WireError::Broken(format!("reading response: {e}")))?;
    serde_json::from_slice(&data)
        .map_err(|e| WireError::Broken(format!("invalid response JSON: {e}")))
}

/// Encode one request frame, refusing an oversized one BEFORE any connection is
/// opened: a request that cannot be sent must not cost the peer a socket.
fn encode_frame(request: &Value) -> Result<Vec<u8>, WireError> {
    let data = serde_json::to_vec(request)
        .map_err(|e| WireError::Broken(format!("encoding request: {e}")))?;
    if data.len() > MAX_FRAME_BYTES {
        return Err(WireError::Broken("request frame too large".into()));
    }
    Ok(data)
}

/// Write an encoded frame: a big-endian length, then the JSON bytes.
async fn write_encoded(stream: &mut UnixStream, data: &[u8]) -> Result<(), WireError> {
    let frame_len = u32::try_from(data.len())
        .map_err(|_| WireError::Broken("request frame too large".into()))?;
    stream
        .write_u32(frame_len)
        .await
        .map_err(|e| WireError::Broken(format!("writing request length: {e}")))?;
    stream
        .write_all(data)
        .await
        .map_err(|e| WireError::Broken(format!("writing request: {e}")))?;
    Ok(())
}

/// A scripted answer: the request the adapter sent in, the frame that answers
/// it out.
#[cfg(test)]
type Script = Box<dyn Fn(&Value) -> Result<Value, WireError> + Send + Sync>;

/// A transport that answers from a script, for tests: no socket, no daemon, and
/// the caller sees the exact request the adapter built.
#[cfg(test)]
pub(crate) struct MemoryTransport {
    answer: Script,
}

#[cfg(test)]
impl MemoryTransport {
    pub(crate) fn new(
        answer: impl Fn(&Value) -> Result<Value, WireError> + Send + Sync + 'static,
    ) -> Self {
        Self {
            answer: Box::new(answer),
        }
    }
}

#[cfg(test)]
impl EngineTransport for MemoryTransport {
    fn exchange(&self, request: Value) -> TransportFuture {
        // The frame checks are the socket implementation's: a request too large
        // to send, or an answer that does not answer this request, fails the
        // same way, so a test exercises the adapter and not a lenient stub.
        let answer = encode_frame(&request).and_then(|_| (self.answer)(&request));
        Box::pin(async move { answer.and_then(|frame| validate(&request, frame)) })
    }

    fn watch(&self, request: Value) -> TransportFuture {
        let answer = (self.answer)(&request);
        Box::pin(async move { answer })
    }
}
