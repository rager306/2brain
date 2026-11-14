//! The engine wire: framed JSON over a unix socket.
//!
//! One request frame in, one response frame out, except a subscription, which
//! answers an `ack` and then events until the caller stops listening. Nothing
//! here knows a tool name or a policy; the server layer decides what to ask and
//! what a refusal means.

use std::path::Path;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

use crate::SCHEMA_VERSION;

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
    /// The engine answered with a rejection: nothing was written.
    Rejected(String),
    /// No usable answer: the operation may or may not have happened.
    Broken(String),
}

pub(crate) enum MutationFailure {
    /// The engine rejected the mutation: nothing was written.
    Rejected(String),
    /// No usable answer: the outcome is unknown and a retry could double-apply.
    Unknown(String),
}

impl WireError {
    pub(crate) fn message(self) -> String {
        match self {
            Self::Rejected(message) | Self::Broken(message) => message,
        }
    }
}

/// Read one engine frame: a big-endian length, then that many JSON bytes.
pub(crate) async fn read_frame(stream: &mut UnixStream) -> Result<Value, WireError> {
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
pub(crate) fn encode_frame(request: &Value) -> Result<Vec<u8>, WireError> {
    let data = serde_json::to_vec(request)
        .map_err(|e| WireError::Broken(format!("encoding request: {e}")))?;
    if data.len() > MAX_FRAME_BYTES {
        return Err(WireError::Broken("request frame too large".into()));
    }
    Ok(data)
}

/// Write an encoded frame: a big-endian length, then the JSON bytes.
pub(crate) async fn write_encoded(stream: &mut UnixStream, data: &[u8]) -> Result<(), WireError> {
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

/// Encode and write one request frame.
pub(crate) async fn write_frame(stream: &mut UnixStream, request: &Value) -> Result<(), WireError> {
    let data = encode_frame(request)?;
    write_encoded(stream, &data).await
}

/// Wait for the next file change the engine reports, or for the deadline.
///
/// A watch is a SUBSCRIPTION, not a read: this opens its own connection, skips
/// the `ack`, and ends at the first `change_event`. The engine's `changes`
/// channel delivers net file deltas per rebuild, so a caller learns what moved
/// without polling `au_files` and diffing by hand. Never waits past `timeout`.
pub(crate) async fn watch_engine(socket: &Path, timeout: Duration) -> Result<Value, String> {
    let mut stream = UnixStream::connect(socket)
        .await
        .map_err(|e| format!("connecting to engine: {e}"))?;
    write_frame(&mut stream, &json!({"subscribe": "changes", "id": 1}))
        .await
        .map_err(|e| e.message())?;
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Ok(
                json!({"added": [], "removed": [], "modified": [], "version": serde_json::Value::Null, "timed_out": true}),
            );
        }
        let frame = match tokio::time::timeout(remaining, read_frame(&mut stream)).await {
            Err(_) => {
                return Ok(json!({"added": [], "removed": [], "modified": [],
                    "version": serde_json::Value::Null, "timed_out": true}));
            }
            Ok(Err(error)) => return Err(error.message()),
            Ok(Ok(frame)) => frame,
        };
        match frame.get("type").and_then(Value::as_str) {
            Some("ack") => continue,
            Some("change_event") => {
                let hint = &frame["scope_hint"];
                return Ok(json!({
                    "added": hint.get("added").cloned().unwrap_or_else(|| json!([])),
                    "removed": hint.get("removed").cloned().unwrap_or_else(|| json!([])),
                    "modified": hint.get("modified").cloned().unwrap_or_else(|| json!([])),
                    "version": frame.get("at_version").cloned().unwrap_or(Value::Null),
                    "timed_out": false,
                }));
            }
            Some("error") => {
                return Err(frame
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("engine refused the subscription")
                    .to_owned());
            }
            _ => continue,
        }
    }
}

pub(crate) async fn exchange(socket: &Path, request: &Value) -> Result<Value, WireError> {
    let data = encode_frame(request)?;
    let mut stream = UnixStream::connect(socket)
        .await
        .map_err(|e| WireError::Broken(format!("connecting to engine: {e}")))?;
    write_encoded(&mut stream, &data).await?;
    let response = read_frame(&mut stream).await?;
    if response.get("schema_version").and_then(Value::as_u64) != Some(SCHEMA_VERSION) {
        return Err(WireError::Broken(
            "unsupported or missing engine schema_version (expected 29)".into(),
        ));
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
            return Err(WireError::Rejected(format!("engine error: {message}")));
        }
        Some("response") => {}
        _ => return Err(WireError::Broken("unexpected engine frame type".into())),
    }
    match response.get("ready").and_then(Value::as_bool) {
        Some(false) => return Err(WireError::Rejected("engine not ready".into())),
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

pub(crate) async fn read_engine(
    socket: &Path,
    request: Value,
    verb: &str,
) -> Result<ReadResult, String> {
    let response = exchange(socket, &request)
        .await
        .map_err(WireError::message)?;
    let version = response["version"]
        .as_u64()
        .ok_or("missing or invalid engine version")?;
    let result = response["result"]
        .as_object()
        .ok_or("invalid result envelope")?;
    let payload = result.get(verb).ok_or("missing read result payload")?;
    let valid = match verb {
        "overview" => payload.is_object(),
        "top_level_dirs" | "hubs" | "diagnostics" | "members" | "imports" | "files" => {
            payload.is_array()
        }
        "subtypes" => payload.is_array() && result.get("base") == request.get("base"),
        "type_tree" => {
            payload.is_object()
                && payload.get("roots").is_some_and(Value::is_array)
                && payload.get("nodes").is_some_and(Value::is_array)
        }
        "type" => payload.is_object() || payload.is_null(),
        "content" => payload.is_object() || payload.is_null(),
        "instances_of" | "validate_value" => payload.is_array(),
        "frontmatter" => payload.is_object() || payload.is_null(),
        "anchors" | "block_ids" => payload.is_array() || payload.is_null(),
        "references_out" | "references_in" => payload.is_array(),
        "neighborhood" => payload.is_object(),
        "instances" | "candidates" => payload.is_array(),
        "semantic_tokens" => payload.is_array() || payload.is_null(),
        "resolve_member" | "instance" | "resolve_target" | "resolve_block_id"
        | "resolve_anchor" => payload.is_object() || payload.is_null(),
        "dir_entries" => payload.is_array(),
        "grep" => payload.is_object(),
        "graph_health" => payload.is_object(),
        "rollback_preview" => payload.is_object(),
        "preview_mutation" => payload.is_object(),
        "types" => payload.is_array() || payload.is_null(),
        _ => false,
    };
    if !valid {
        return Err("invalid read result payload".into());
    }
    Ok(ReadResult {
        version,
        payload: Value::Object(result.clone()),
    })
}

pub(crate) async fn read_engine_with_timeout(
    socket: &Path,
    request: Value,
    verb: &str,
    timeout: Duration,
) -> Result<ReadResult, String> {
    tokio::time::timeout(timeout, read_engine(socket, request, verb))
        .await
        .map_err(|_| format!("engine read '{verb}' timed out after {timeout:?} (connect, write, or response); no retry was attempted"))?
}

/// Send one mutation. A rejection is definitive; anything else means the
/// mutation may have landed, so a caller must never retry it blindly.
pub(crate) async fn mutate_engine(socket: &Path, request: Value) -> Result<ReadResult, WireError> {
    let response = exchange(socket, &request).await?;
    let version = response["version"]
        .as_u64()
        .ok_or_else(|| WireError::Broken("missing or invalid engine version".into()))?;
    let result = response["result"]
        .as_object()
        .ok_or_else(|| WireError::Broken("invalid result envelope".into()))?;
    // A single-file mutation names its path; a batch names its ops instead.
    let named = result.get("path").is_some_and(Value::is_string)
        || result.get("ops").is_some_and(Value::is_array);
    if !named {
        return Err(WireError::Broken("invalid mutation result payload".into()));
    }
    Ok(ReadResult {
        version,
        payload: Value::Object(result.clone()),
    })
}

pub(crate) async fn read_mutation_with_timeout(
    socket: &Path,
    request: Value,
    timeout: Duration,
) -> Result<ReadResult, MutationFailure> {
    match tokio::time::timeout(timeout, mutate_engine(socket, request)).await {
        Err(_) => Err(MutationFailure::Unknown(format!(
            "engine mutation timed out after {timeout:?}"
        ))),
        Ok(Err(WireError::Rejected(message))) => Err(MutationFailure::Rejected(message)),
        Ok(Err(WireError::Broken(message))) => Err(MutationFailure::Unknown(message)),
        Ok(Ok(result)) => Ok(result),
    }
}
