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

/// The shape a read's payload must have, one entry per engine verb.
///
/// This table is the only place a verb's shape is stated. It used to be a `match` inside
/// [`read_engine`], and when the engine learned `search` the arm was missing: every call failed with
/// "invalid read result payload" while the suite stayed green, because no test ever fed a payload the
/// table did not know. `verb_shapes_cover_every_captured_read` in the test module now derives the verb
/// from each captured fixture and fails when the table has no entry, so a new verb cannot ship
/// unvalidated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Shape {
    Object,
    Array,
    ObjectOrNull,
    ArrayOrNull,
    /// `{roots: [...], nodes: [...]}`.
    TypeTree,
}

/// The shape table. Every verb the adapter reads must appear here.
const VERB_SHAPES: &[(&str, Shape)] = &[
    ("overview", Shape::Object),
    ("top_level_dirs", Shape::Array),
    ("hubs", Shape::Array),
    ("diagnostics", Shape::Array),
    ("members", Shape::Array),
    ("imports", Shape::Array),
    ("files", Shape::Array),
    ("subtypes", Shape::Array),
    ("type_tree", Shape::TypeTree),
    ("type", Shape::ObjectOrNull),
    ("content", Shape::ObjectOrNull),
    ("instances_of", Shape::Array),
    ("validate_value", Shape::Array),
    ("frontmatter", Shape::ObjectOrNull),
    ("anchors", Shape::ArrayOrNull),
    ("block_ids", Shape::ArrayOrNull),
    ("references_out", Shape::Array),
    ("references_in", Shape::Array),
    ("neighborhood", Shape::Object),
    ("instances", Shape::Array),
    ("candidates", Shape::Array),
    ("semantic_tokens", Shape::ArrayOrNull),
    ("resolve_member", Shape::ObjectOrNull),
    ("instance", Shape::ObjectOrNull),
    ("resolve_target", Shape::ObjectOrNull),
    ("resolve_block_id", Shape::ObjectOrNull),
    ("resolve_anchor", Shape::ObjectOrNull),
    ("dir_entries", Shape::Array),
    ("grep", Shape::Object),
    ("graph_health", Shape::Object),
    ("rollback_preview", Shape::Object),
    ("preview_mutation", Shape::Object),
    ("types", Shape::ArrayOrNull),
    ("search", Shape::Object),
];

/// The declared shape of `verb`, or `None` when the table has no entry.
pub(crate) fn shape_of(verb: &str) -> Option<Shape> {
    VERB_SHAPES
        .iter()
        .find(|(name, _)| *name == verb)
        .map(|(_, shape)| *shape)
}

/// Whether `payload` matches the declared shape.
pub(crate) fn payload_matches(shape: Shape, payload: &Value) -> bool {
    match shape {
        Shape::Object => payload.is_object(),
        Shape::Array => payload.is_array(),
        Shape::ObjectOrNull => payload.is_object() || payload.is_null(),
        Shape::ArrayOrNull => payload.is_array() || payload.is_null(),
        Shape::TypeTree => {
            payload.is_object()
                && payload.get("roots").is_some_and(Value::is_array)
                && payload.get("nodes").is_some_and(Value::is_array)
        }
    }
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
    // Owned from here on: the result is moved out of the frame instead of being deep-cloned, which for
    // a paged read (`au_files` at 500 rows) used to copy the whole array on every call.
    let mut frame = response;
    let version = frame["version"]
        .as_u64()
        .ok_or("missing or invalid engine version")?;
    let result = match frame
        .as_object_mut()
        .and_then(|frame| frame.remove("result"))
    {
        Some(Value::Object(result)) => result,
        _ => return Err("invalid result envelope".into()),
    };
    let shape = shape_of(verb).ok_or_else(|| format!("unvalidated read verb: {verb}"))?;
    let payload = result.get(verb).ok_or("missing read result payload")?;
    if !payload_matches(shape, payload) {
        return Err("invalid read result payload".into());
    }
    if verb == "subtypes" && result.get("base") != base.as_ref() {
        return Err("invalid read result payload".into());
    }
    Ok(ReadResult {
        version,
        payload: Value::Object(result),
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Every captured read must have a declared shape, and the captured payload must satisfy it.
    ///
    /// The verb is taken from the payload itself — a read result object is keyed by the verb — so this
    /// test needs no hand-written list to drift from. A verb the engine learns without a `VERB_SHAPES`
    /// entry fails here instead of failing every call at runtime, which is how `search` shipped broken.
    #[test]
    fn verb_shapes_cover_every_captured_read() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/synthetic-reads");
        let mut checked = std::collections::BTreeSet::new();
        for entry in std::fs::read_dir(&dir).expect("fixture directory") {
            let path = entry.expect("fixture entry").path();
            if path.extension().and_then(std::ffi::OsStr::to_str) != Some("json") {
                continue;
            }
            let text = std::fs::read_to_string(&path).expect("fixture text");
            let fixture: Value = serde_json::from_str(&text).expect("fixture json");
            // A composed tool builds its answer in the adapter and never crosses the engine boundary,
            // so its payload has no engine verb to declare. Adding one here is a deliberate act.
            const COMPOSED: &[&str] = &["au_follow"];
            if fixture
                .get("tool")
                .and_then(Value::as_str)
                .is_some_and(|tool| COMPOSED.contains(&tool))
            {
                continue;
            }
            let payload = fixture
                .get("payload")
                .and_then(Value::as_object)
                .unwrap_or_else(|| panic!("{}: no payload object", path.display()));
            // A verb-keyed read answers `{<verb>: …}`; a few tools (`au_typed`, `au_resolved`) flatten
            // their answer into several fields instead, and those have no verb to check.
            let keys: Vec<&String> = payload.keys().filter(|key| *key != "base").collect();
            if keys.len() != 1 {
                continue;
            }
            for (verb, value) in payload {
                // `subtypes` carries the base it answered for beside the payload.
                if verb == "base" {
                    continue;
                }
                let shape = shape_of(verb).unwrap_or_else(|| {
                    panic!(
                        "{}: verb `{verb}` has no shape in VERB_SHAPES",
                        path.display()
                    )
                });
                assert!(
                    payload_matches(shape, value),
                    "{}: payload for `{verb}` does not match {shape:?}",
                    path.display()
                );
                checked.insert(verb.clone());
            }
        }
        assert!(checked.len() >= 25, "checked only {checked:?}");
    }

    #[test]
    fn an_unknown_verb_is_refused_rather_than_guessed() {
        assert_eq!(shape_of("not_a_verb"), None);
        let shape = shape_of("search").expect("search is declared");
        assert!(payload_matches(shape, &json!({"matches": [], "total": 0})));
        assert!(!payload_matches(shape, &json!([])));
    }
}
