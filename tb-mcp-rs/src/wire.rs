//! The engine wire: what one answering frame means for a read or a mutation.
//!
//! The frames themselves belong to `transport.rs`; nothing here opens a socket.
//! A read checks that the verb's payload has the shape its tool promises, a
//! mutation checks that the engine named what it wrote, a member-relative path
//! is resolved against the member table, and a failure is split by what it
//! proves about a mutation.

use std::path::Path;
use std::time::Duration;

use serde_json::{Value, json};

use crate::transport::{EngineTransport, MutationFailure, ReadResult, WireError};

/// Wait for the next file change the engine reports, or for the deadline.
///
/// A watch is a SUBSCRIPTION, not a read: the transport opens its own
/// connection, skips the `ack`, and ends at the first `change_event`. The
/// engine's `changes` channel delivers net file deltas per rebuild, so a caller
/// learns what moved without polling `au_files` and diffing by hand. Never waits
/// past `timeout`.
pub(crate) async fn watch_engine(
    transport: &dyn EngineTransport,
    timeout: Duration,
) -> Result<Value, String> {
    let frame = match tokio::time::timeout(
        timeout,
        transport.watch(json!({"subscribe": "changes", "id": 1})),
    )
    .await
    {
        Err(_) => return Ok(watch_timeout()),
        Ok(Err(error)) => return Err(error.message()),
        Ok(Ok(frame)) => frame,
    };
    match frame.get("type").and_then(Value::as_str) {
        Some("change_event") => {
            let hint = &frame["scope_hint"];
            Ok(json!({
                "added": hint.get("added").cloned().unwrap_or_else(|| json!([])),
                "removed": hint.get("removed").cloned().unwrap_or_else(|| json!([])),
                "modified": hint.get("modified").cloned().unwrap_or_else(|| json!([])),
                "version": frame.get("at_version").cloned().unwrap_or(Value::Null),
                "timed_out": false,
            }))
        }
        Some("error") => Err(frame
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("engine refused the subscription")
            .to_owned()),
        _ => Err("unexpected engine subscription frame".to_owned()),
    }
}

/// A timeout is an ordinary `au_watch` answer, not an error.
fn watch_timeout() -> Value {
    json!({
        "added": [],
        "removed": [],
        "modified": [],
        "version": Value::Null,
        "timed_out": true,
    })
}

pub(crate) async fn read_engine(
    transport: &dyn EngineTransport,
    request: Value,
    verb: &str,
) -> Result<ReadResult, String> {
    let base = request.get("base").cloned();
    let response = transport
        .exchange(request)
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
        "subtypes" => payload.is_array() && result.get("base") == base.as_ref(),
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
    transport: &dyn EngineTransport,
    request: Value,
    verb: &str,
    timeout: Duration,
) -> Result<ReadResult, String> {
    tokio::time::timeout(timeout, read_engine(transport, request, verb))
        .await
        .map_err(|_| format!("engine read '{verb}' timed out after {timeout:?} (connect, write, or response); no retry was attempted"))?
}

/// Send one mutation. A rejection is definitive; anything else means the
/// mutation may have landed, so a caller must never retry it blindly.
pub(crate) async fn mutate_engine(
    transport: &dyn EngineTransport,
    request: Value,
) -> Result<ReadResult, WireError> {
    let response = transport.exchange(request).await?;
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
    transport: &dyn EngineTransport,
    request: Value,
    timeout: Duration,
) -> Result<ReadResult, MutationFailure> {
    match tokio::time::timeout(timeout, mutate_engine(transport, request)).await {
        Err(_) => Err(MutationFailure::Unknown(format!(
            "engine mutation timed out after {timeout:?}"
        ))),
        Ok(Err(WireError::Rejected(message))) => Err(MutationFailure::Rejected(message)),
        Ok(Err(WireError::Broken(message))) => Err(MutationFailure::Unknown(message)),
        Ok(Ok(result)) => Ok(result),
    }
}

/// Resolve a member-relative path to an absolute one, refusing members the
/// engine does not author.
pub(crate) async fn resolve_editable(
    transport: &dyn EngineTransport,
    repo: &str,
    relative: &str,
) -> Result<String, String> {
    let members = read_engine(
        transport,
        json!({"read": "members", "id": 1, "repo": repo}),
        "members",
    )
    .await?;
    let member = members
        .payload
        .get("members")
        .and_then(Value::as_array)
        .and_then(|members| members.iter().find(|m| m.get("repo") == Some(&json!(repo))))
        .ok_or_else(|| format!("unknown workspace member: {repo}"))?;
    if member.get("editable").and_then(Value::as_bool) != Some(true) {
        return Err(format!(
            "workspace member {repo} is not editable; a consumed member cannot be written"
        ));
    }
    let root = member
        .get("root")
        .and_then(Value::as_str)
        .filter(|root| !root.is_empty())
        .ok_or_else(|| format!("workspace member {repo} has no local root"))?;
    Path::new(root)
        .join(relative)
        .to_str()
        .map(str::to_owned)
        .ok_or_else(|| "resolved path is not valid UTF-8".to_owned())
}

/// Join a member's declared root with a client-supplied relative path.
///
/// The path is validated by the caller; this only resolves the member and joins.
pub(crate) async fn absolute_member_path(
    transport: &dyn EngineTransport,
    repo: &str,
    relative: &str,
) -> Result<String, String> {
    let members = read_engine(
        transport,
        json!({"read": "members", "id": 1, "repo": repo}),
        "members",
    )
    .await?;
    let member = members
        .payload
        .get("members")
        .and_then(Value::as_array)
        .and_then(|members| members.iter().find(|m| m.get("repo") == Some(&json!(repo))))
        .ok_or_else(|| format!("unknown workspace member: {repo}"))?;
    let root = member
        .get("root")
        .and_then(Value::as_str)
        .filter(|root| !root.is_empty())
        .ok_or_else(|| format!("workspace member {repo} has no local root"))?;
    Path::new(root)
        .join(relative)
        .to_str()
        .map(str::to_owned)
        .ok_or_else(|| "resolved path is not valid UTF-8".to_owned())
}

/// A member's declared root, without joining a relative path.
pub(crate) async fn member_root(
    transport: &dyn EngineTransport,
    repo: &str,
) -> Result<String, String> {
    let members = read_engine(
        transport,
        json!({"read": "members", "id": 1, "repo": repo}),
        "members",
    )
    .await?;
    members
        .payload
        .get("members")
        .and_then(Value::as_array)
        .and_then(|members| members.iter().find(|m| m.get("repo") == Some(&json!(repo))))
        .and_then(|member| member.get("root"))
        .and_then(Value::as_str)
        .filter(|root| !root.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| format!("workspace member {repo} has no local root"))
}

/// Read one member's file text by joining its declared root with a
/// client-supplied relative path. The path is validated by the caller.
pub(crate) async fn read_content(
    transport: &dyn EngineTransport,
    repo: &str,
    relative: &str,
) -> Result<ReadResult, String> {
    let path = absolute_member_path(transport, repo, relative).await?;
    read_engine(
        transport,
        json!({"read": "content", "id": 1, "path": path}),
        "content",
    )
    .await
}
