//! The engine wire: what one answering frame means for a read or a mutation.
//!
//! The frames themselves belong to `transport.rs`; nothing here opens a socket.
//! A read checks that the verb's payload has the shape its tool promises and, for
//! the hot reads, that it deserializes into the fields those tools name; a
//! mutation checks that the engine named what it wrote, a member-relative path
//! is resolved against the member table, and a failure is split by what it
//! proves about a mutation.

use std::path::Path;
use std::time::Duration;

use serde::de::DeserializeOwned;
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

/// The engine verb each read tool asks for: the table the router consults instead of naming a
/// verb itself.
///
/// `VERB_SHAPES` says what a verb's payload looks like; this table says which tool asks for
/// which verb, and the tests join the two rather than a reviewer's memory. `search` shipped
/// broken because nothing joined them: the tool was advertised, no payload for it was ever
/// validated, and every call failed at "invalid read result payload".
///
/// A tool whose answer is one engine read names exactly one verb here, and the router fills
/// that verb into the request (`read_for`). A tool the adapter composes from several reads
/// lists every verb it builds its answer from, so the table stays the whole statement of what
/// the read surface asks the engine and no declared shape is left unreachable: `au_follow`
/// picks a resolve verb by the link's grammar and peeks the addressed line with `content`,
/// `au_typed` classifies with `instance` then `resolve_target`, and `au_references` reads both
/// directions.
pub(crate) const READ_VERBS: &[(&str, &[&str])] = &[
    ("au_anchors", &["anchors"]),
    ("au_block_ids", &["block_ids"]),
    ("au_candidates", &["candidates"]),
    ("au_children", &["dir_entries"]),
    ("au_content", &["content"]),
    ("au_diagnostics", &["diagnostics"]),
    ("au_files", &["files"]),
    (
        "au_follow",
        &[
            "resolve_block_id",
            "resolve_anchor",
            "resolve_target",
            "content",
        ],
    ),
    ("au_frontmatter", &["frontmatter"]),
    ("au_graph_health", &["graph_health"]),
    ("au_grep", &["grep"]),
    ("au_hubs", &["hubs"]),
    ("au_imports", &["imports"]),
    ("au_instances", &["instances"]),
    ("au_instances_of", &["instances_of"]),
    ("au_members", &["members"]),
    ("au_neighborhood", &["neighborhood"]),
    ("au_overview", &["overview"]),
    ("au_preview_mutation", &["preview_mutation"]),
    ("au_reconcile", &["content"]),
    ("au_references", &["references_out", "references_in"]),
    ("au_resolve_member", &["resolve_member"]),
    ("au_resolved", &["instance"]),
    ("au_rollback_preview", &["rollback_preview"]),
    ("au_search", &["search"]),
    ("au_semantic_tokens", &["semantic_tokens"]),
    ("au_subtypes", &["subtypes"]),
    ("au_top_level_dirs", &["top_level_dirs"]),
    ("au_type", &["type"]),
    ("au_type_tree", &["type_tree"]),
    ("au_typed", &["instance", "resolve_target"]),
    ("au_types", &["types"]),
    ("au_validate_value", &["validate_value"]),
];

/// The verbs the table names for `tool`, or `None` when it has no row.
fn verbs_for(tool: &str) -> Option<&'static [&'static str]> {
    READ_VERBS
        .iter()
        .find(|(name, _)| *name == tool)
        .map(|(_, verbs)| *verbs)
}

/// The single engine verb a plain read tool asks for; `None` when the tool is unknown or
/// composes its answer from several reads, and so builds its own requests.
pub(crate) fn read_verb(tool: &str) -> Option<&'static str> {
    match verbs_for(tool)? {
        [verb] => Some(*verb),
        _ => None,
    }
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

/// The typed form of each hot read's payload: the fields a client reads by name.
///
/// These are the *check*, not the transport: the value forwarded to the client stays the raw
/// [`Value`], because `structuredContent` is protocol JSON whose contract is the tool's declared
/// `outputSchema` — re-serializing a struct here would restate that contract, and drop any field
/// these structs do not name, instead of checking the payload against it.
///
/// Every field is optional. The check therefore says "this field, when the engine sends it, has
/// this type" and nothing more: a payload that omits a field is accepted, so a client-visible field
/// the engine stops sending is caught by the tool's `outputSchema` at the client, not here. What
/// this check does catch is a field whose TYPE changed — an id that became a number, a list that
/// became an object — which is the drift a client would otherwise discover at runtime. No struct
/// denies unknown fields: a field the engine adds later must reach the client unchanged.
#[allow(dead_code)] // The deserializer is the reader: a field is declared to be checked, not fetched.
mod payloads {
    use std::collections::BTreeMap;

    use serde::Deserialize;

    /// `overview`: the orientation map, each field mirroring one read it folds.
    #[derive(Debug, Deserialize)]
    pub(super) struct Overview {
        repo: Option<String>,
        scope: Option<String>,
        #[serde(default)]
        members: Vec<Member>,
        #[serde(default)]
        top_level_dirs: Vec<TopLevelDir>,
        type_counts: Option<TypeCounts>,
        diagnostic_counts: Option<DiagnosticCounts>,
        #[serde(default)]
        hubs: Vec<Hub>,
        graph_shape: Option<GraphShape>,
    }

    /// One workspace member; `git.root` is null off-git and `tier` is absent for an
    /// unmounted member.
    #[derive(Debug, Deserialize)]
    pub(super) struct Member {
        repo: Option<String>,
        root: Option<String>,
        scattered: Option<bool>,
        editable: Option<bool>,
        local: Option<bool>,
        tier: Option<String>,
        role: Option<String>,
        disabled: Option<bool>,
        git: Option<MemberGit>,
    }

    #[derive(Debug, Deserialize)]
    pub(super) struct MemberGit {
        tracked: Option<bool>,
        root: Option<String>,
    }

    #[derive(Debug, Deserialize)]
    pub(super) struct TopLevelDir {
        repo: Option<String>,
        name: Option<String>,
        path: Option<String>,
        /// The member rooted at this directory; absent for ordinary content.
        member: Option<String>,
    }

    #[derive(Debug, Deserialize)]
    pub(super) struct TypeCounts {
        total: Option<u64>,
        by_repo: Option<BTreeMap<String, u64>>,
    }

    #[derive(Debug, Deserialize)]
    pub(super) struct DiagnosticCounts {
        total: Option<u64>,
        by_severity: Option<BTreeMap<String, u64>>,
        by_code: Option<BTreeMap<String, u64>>,
    }

    /// `files`, `hubs`, and `overview.hubs`: one catalogued file.
    #[derive(Debug, Deserialize)]
    pub(super) struct FileEntry {
        path: Option<String>,
        stem: Option<String>,
        repo: Option<String>,
        kind: Option<String>,
    }

    /// One hub: a referenced file and its inbound edges, split by kind.
    #[derive(Debug, Deserialize)]
    pub(super) struct Hub {
        path: Option<String>,
        repo: Option<String>,
        kind: Option<String>,
        refs_structural: Option<u64>,
        refs_navigational: Option<u64>,
        refs_total: Option<u64>,
    }

    /// The scalar whole-graph summary `overview.graph_shape` mirrors.
    #[derive(Debug, Deserialize)]
    pub(super) struct GraphShape {
        repo: Option<String>,
        scope: Option<String>,
        node_count: Option<u64>,
        edge_count: Option<u64>,
        edges_structural: Option<u64>,
        edges_navigational: Option<u64>,
        components: Option<u64>,
        largest_component: Option<u64>,
        orphans: Option<Orphans>,
        degree: Option<Degree>,
        density: Option<f64>,
    }

    #[derive(Debug, Deserialize)]
    pub(super) struct Orphans {
        no_inbound: Option<u64>,
        isolated: Option<u64>,
        /// Present only for a read that asked for the unbounded path list.
        paths: Option<Vec<OrphanPath>>,
    }

    #[derive(Debug, Deserialize)]
    pub(super) struct OrphanPath {
        path: Option<String>,
        kind: Option<String>,
    }

    #[derive(Debug, Deserialize)]
    pub(super) struct Degree {
        #[serde(rename = "in", default)]
        inbound: Vec<DegreeBucket>,
        #[serde(default)]
        out: Vec<DegreeBucket>,
    }

    #[derive(Debug, Deserialize)]
    pub(super) struct DegreeBucket {
        degree: Option<u64>,
        count: Option<u64>,
    }

    /// `content`: `null` when the file is unreadable, so the check deserializes
    /// `Option<Content>`.
    #[derive(Debug, Deserialize)]
    pub(super) struct Content {
        text: Option<String>,
        hash: Option<String>,
        /// HEAD of the repo owning the file; null off-git.
        commit: Option<String>,
    }

    /// `search`: one page of ranked documents.
    #[derive(Debug, Deserialize)]
    pub(super) struct Search {
        #[serde(default)]
        matches: Vec<SearchMatch>,
        total: Option<u64>,
        offset: Option<u64>,
        /// `null` on the page that reached the end, never a sentinel.
        next_offset: Option<u64>,
        exact: Option<bool>,
    }

    #[derive(Debug, Deserialize)]
    pub(super) struct SearchMatch {
        path: Option<String>,
        score: Option<f64>,
        matched_terms: Option<u64>,
    }

    /// One diagnostic, the shape `diagnostics` and `validate_value` share.
    #[derive(Debug, Deserialize)]
    pub(super) struct Diagnostic {
        code: Option<String>,
        severity: Option<String>,
        message: Option<String>,
        span: Option<Span>,
        /// Omitted when empty.
        related: Option<Vec<Span>>,
        /// Omitted when the code has no suggested fix.
        fix: Option<Fix>,
    }

    #[derive(Debug, Deserialize)]
    pub(super) struct Span {
        file: Option<String>,
        range: Option<ByteRange>,
        /// Absent for a span into a file the build never read.
        line_col: Option<LineColRange>,
    }

    #[derive(Debug, Deserialize)]
    pub(super) struct ByteRange {
        start: Option<u64>,
        end: Option<u64>,
    }

    #[derive(Debug, Deserialize)]
    pub(super) struct LineColRange {
        start: Option<LineCol>,
        end: Option<LineCol>,
    }

    #[derive(Debug, Deserialize)]
    pub(super) struct LineCol {
        line: Option<u64>,
        col: Option<u64>,
    }

    #[derive(Debug, Deserialize)]
    pub(super) struct Fix {
        description: Option<String>,
        /// Omitted when the fix is advisory only.
        #[serde(default)]
        actions: Vec<FixAction>,
    }

    #[derive(Debug, Deserialize)]
    pub(super) struct FixAction {
        verb: Option<String>,
        title: Option<String>,
        /// Keyed by the mutation verb's own argument names; omitted when empty.
        args: Option<BTreeMap<String, String>>,
    }

    /// `graph_health`: the report and its bounded lists.
    #[derive(Debug, Deserialize)]
    pub(super) struct GraphHealth {
        repo: Option<String>,
        scope: Option<String>,
        counts: Option<GraphHealthCounts>,
        #[serde(default)]
        cycles: Vec<Cycle>,
        #[serde(default)]
        isolated: Vec<HealthNode>,
        #[serde(default)]
        dead_ends: Vec<HealthNode>,
        #[serde(default)]
        sources: Vec<HealthNode>,
        #[serde(default)]
        broken_links: Vec<BrokenLink>,
        #[serde(default)]
        central: Vec<HealthNode>,
        truncated: Option<bool>,
    }

    #[derive(Debug, Deserialize)]
    pub(super) struct GraphHealthCounts {
        nodes: Option<u64>,
        edges: Option<u64>,
        components: Option<u64>,
        isolated: Option<u64>,
        dead_ends: Option<u64>,
        sources: Option<u64>,
        cycles: Option<u64>,
        nodes_in_cycles: Option<u64>,
        broken_links: Option<u64>,
    }

    #[derive(Debug, Deserialize)]
    pub(super) struct Cycle {
        length: Option<u64>,
        #[serde(default)]
        nodes: Vec<String>,
    }

    #[derive(Debug, Deserialize)]
    pub(super) struct HealthNode {
        path: Option<String>,
        #[serde(rename = "in")]
        inbound: Option<u64>,
        out: Option<u64>,
        total: Option<u64>,
    }

    #[derive(Debug, Deserialize)]
    pub(super) struct BrokenLink {
        path: Option<String>,
        code: Option<String>,
        message: Option<String>,
        /// The link's line; null when the diagnostic has no position.
        line: Option<u64>,
    }
}

/// Whether `payload` deserializes into the typed form declared for `verb`.
///
/// A verb with no typed form returns true: its shape is its whole contract, as before. A payload
/// that fails here is refused with the same message a shape mismatch gets, so the typed check
/// tightens the existing contract without adding a second failure mode.
fn payload_deserializes(verb: &str, payload: &Value) -> bool {
    match verb {
        "overview" => deserializes::<payloads::Overview>(payload),
        "content" => deserializes::<Option<payloads::Content>>(payload),
        "files" => deserializes::<Vec<payloads::FileEntry>>(payload),
        "search" => deserializes::<payloads::Search>(payload),
        "diagnostics" => deserializes::<Vec<payloads::Diagnostic>>(payload),
        "hubs" => deserializes::<Vec<payloads::Hub>>(payload),
        "graph_health" => deserializes::<payloads::GraphHealth>(payload),
        _ => true,
    }
}

/// Deserialize a payload in place of a `Value`, without cloning it.
fn deserializes<T: DeserializeOwned>(payload: &Value) -> bool {
    T::deserialize(payload).is_ok()
}

/// Send one read and validate the answer against the verb the request asks for.
///
/// The verb is taken from the request, so the request and the shape the answer is checked
/// against cannot disagree; which verb a tool asks for comes from `READ_VERBS`.
pub(crate) async fn read_engine(
    transport: &dyn EngineTransport,
    request: Value,
) -> Result<ReadResult, String> {
    let base = request.get("base").cloned();
    let verb = requested_verb(&request)
        .ok_or("missing read verb")?
        .to_owned();
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
    let shape = shape_of(&verb).ok_or_else(|| format!("unvalidated read verb: {verb}"))?;
    let payload = result
        .get(verb.as_str())
        .ok_or("missing read result payload")?;
    if !payload_matches(shape, payload) || !payload_deserializes(&verb, payload) {
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

/// The verb a read request asks for.
fn requested_verb(request: &Value) -> Option<&str> {
    request.get("read").and_then(Value::as_str)
}

/// `read_engine`, bounded by `timeout`.
pub(crate) async fn read_engine_with_timeout(
    transport: &dyn EngineTransport,
    request: Value,
    timeout: Duration,
) -> Result<ReadResult, String> {
    let verb = requested_verb(&request).unwrap_or_default().to_owned();
    tokio::time::timeout(timeout, read_engine(transport, request))
        .await
        .map_err(|_| format!("engine read '{verb}' timed out after {timeout:?} (connect, write, or response); no retry was attempted"))?
}

/// One engine read for `tool`: the verb comes from `READ_VERBS` and the request carries only
/// the parameters, so a call site cannot name a verb the shape table does not know.
pub(crate) async fn read_for(
    transport: &dyn EngineTransport,
    tool: &str,
    request: Value,
) -> Result<ReadResult, String> {
    read_engine(transport, request_for(tool, request)?).await
}

/// `read_for`, bounded by `timeout`.
pub(crate) async fn read_for_with_timeout(
    transport: &dyn EngineTransport,
    tool: &str,
    request: Value,
    timeout: Duration,
) -> Result<ReadResult, String> {
    read_engine_with_timeout(transport, request_for(tool, request)?, timeout).await
}

/// `request` carrying the verb the table names for `tool`.
fn request_for(tool: &str, mut request: Value) -> Result<Value, String> {
    let verb = read_verb(tool).ok_or_else(|| format!("{tool} has no single engine read verb"))?;
    request["read"] = json!(verb);
    Ok(request)
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
    let members = read_engine(transport, json!({"read": "members", "id": 1, "repo": repo})).await?;
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
    let members = read_engine(transport, json!({"read": "members", "id": 1, "repo": repo})).await?;
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
    let members = read_engine(transport, json!({"read": "members", "id": 1, "repo": repo})).await?;
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
    read_engine(transport, json!({"read": "content", "id": 1, "path": path})).await
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::transport::{MemoryTransport, SCHEMA_VERSION};

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

    /// Every declared shape is asked for by some read tool: the reverse of the direction the
    /// tool-side test checks. A shape nothing asks for outlived its tool, and a tool cannot stop
    /// reading a verb without the table noticing.
    #[test]
    fn every_declared_shape_is_asked_for_by_a_read_tool() {
        let named: std::collections::BTreeSet<&str> = READ_VERBS
            .iter()
            .flat_map(|(_, verbs)| verbs.iter().copied())
            .collect();
        for (verb, _) in VERB_SHAPES {
            assert!(
                named.contains(verb),
                "VERB_SHAPES declares `{verb}` and no read tool asks for it"
            );
        }
    }

    #[test]
    fn an_unknown_verb_is_refused_rather_than_guessed() {
        assert_eq!(shape_of("not_a_verb"), None);
        let shape = shape_of("search").expect("search is declared");
        assert!(payload_matches(shape, &json!({"matches": [], "total": 0})));
        assert!(!payload_matches(shape, &json!([])));
    }

    /// The captured payload of one synthetic-read fixture, as the engine sends it.
    fn captured(tool: &str) -> Value {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/synthetic-reads")
            .join(format!("{tool}.json"));
        let text =
            std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let fixture: Value = serde_json::from_str(&text).expect("fixture json");
        fixture
            .get("payload")
            .cloned()
            .unwrap_or_else(|| panic!("{}: no payload", path.display()))
    }

    /// One engine response frame carrying `payload` under `verb`.
    fn answering_frame(verb: &str, payload: &Value) -> Value {
        let mut result = serde_json::Map::new();
        result.insert(verb.to_owned(), payload.clone());
        json!({
            "type": "response",
            "schema_version": SCHEMA_VERSION,
            "id": 1,
            "ready": true,
            "version": 7,
            "result": Value::Object(result),
        })
    }

    /// Run one read against a scripted engine that answers `payload` for `verb`.
    async fn read_with(verb: &str, payload: Value) -> Result<ReadResult, String> {
        let answering_verb = verb.to_owned();
        let transport = MemoryTransport::new(move |_request: &Value| {
            Ok(answering_frame(&answering_verb, &payload))
        });
        read_engine(&transport, json!({"read": verb, "id": 1})).await
    }

    /// A payload the typed form refuses must fail with the message a shape mismatch uses.
    async fn assert_refused(verb: &str, payload: Value) {
        assert_eq!(
            read_with(verb, payload).await.unwrap_err(),
            "invalid read result payload",
            "{verb}: a payload that does not fit the typed form must not invent a second message"
        );
    }

    #[tokio::test]
    async fn the_overview_payload_is_typed() {
        let payload = captured("au_overview")["overview"].clone();
        let read = read_with("overview", payload.clone())
            .await
            .expect("the captured overview payload deserializes");
        assert_eq!(read.payload, json!({ "overview": payload }));

        let mut wrong = payload.clone();
        wrong["type_counts"] = json!(5);
        assert_refused("overview", wrong).await;
    }

    #[tokio::test]
    async fn the_content_payload_is_typed() {
        let payload = captured("au_content")["content"].clone();
        let read = read_with("content", payload.clone())
            .await
            .expect("the captured content payload deserializes");
        assert_eq!(read.payload, json!({ "content": payload }));

        // An unreadable file answers null, which is the read's own answer, not a failure.
        read_with("content", Value::Null)
            .await
            .expect("null is the content read's not-found payload");

        let mut wrong = payload.clone();
        wrong["text"] = json!(["line"]);
        assert_refused("content", wrong).await;
    }

    #[tokio::test]
    async fn the_files_payload_is_typed() {
        let payload = captured("au_files")["files"].clone();
        let read = read_with("files", payload.clone())
            .await
            .expect("the captured files payload deserializes");
        assert_eq!(read.payload, json!({ "files": payload }));

        let mut wrong = payload.clone();
        wrong[0]["stem"] = json!(7);
        assert_refused("files", wrong).await;
    }

    #[tokio::test]
    async fn the_search_payload_is_typed() {
        let payload = captured("au_search")["search"].clone();
        let read = read_with("search", payload.clone())
            .await
            .expect("the captured search payload deserializes");
        assert_eq!(read.payload, json!({ "search": payload }));

        let mut wrong = payload.clone();
        wrong["matches"][0]["score"] = json!("high");
        assert_refused("search", wrong).await;
    }

    #[tokio::test]
    async fn the_diagnostics_payload_is_typed() {
        let payload = captured("au_diagnostics")["diagnostics"].clone();
        let read = read_with("diagnostics", payload.clone())
            .await
            .expect("the captured diagnostics payload deserializes");
        assert_eq!(read.payload, json!({ "diagnostics": payload }));

        let mut wrong = payload.clone();
        wrong[1]["span"] = json!("notes/invalid.md:3");
        assert_refused("diagnostics", wrong).await;
    }

    #[tokio::test]
    async fn the_hubs_payload_is_typed() {
        let payload = captured("au_hubs")["hubs"].clone();
        let read = read_with("hubs", payload.clone())
            .await
            .expect("the captured hubs payload deserializes");
        assert_eq!(read.payload, json!({ "hubs": payload }));

        let mut wrong = payload.clone();
        wrong[0]["refs_navigational"] = json!("0");
        assert_refused("hubs", wrong).await;
    }

    #[tokio::test]
    async fn the_graph_health_payload_is_typed() {
        let payload = captured("au_graph_health")["graph_health"].clone();
        let read = read_with("graph_health", payload.clone())
            .await
            .expect("the captured graph_health payload deserializes");
        assert_eq!(read.payload, json!({ "graph_health": payload }));

        let mut wrong = payload.clone();
        wrong["central"] = json!("nodes");
        assert_refused("graph_health", wrong).await;
    }

    /// Both directions of tolerance, on every typed read: a field the engine adds later must reach
    /// the client, and a field it stops sending must not fail the read — the structs name fields to
    /// check their types, they do not demand a complete payload.
    #[tokio::test]
    async fn an_added_or_a_dropped_field_does_not_break_a_typed_read() {
        for (tool, verb, dropped) in [
            ("au_overview", "overview", "members"),
            ("au_content", "content", "hash"),
            ("au_files", "files", "path"),
            ("au_search", "search", "matches"),
            ("au_diagnostics", "diagnostics", "severity"),
            ("au_hubs", "hubs", "refs_total"),
            ("au_graph_health", "graph_health", "counts"),
        ] {
            let mut added = captured(tool)[verb].clone();
            edited_element(&mut added, |element| {
                element.insert("added_later".to_owned(), json!(true));
            });
            read_with(verb, added)
                .await
                .unwrap_or_else(|error| panic!("{verb}: an added field broke the read: {error}"));

            let mut dropped_field = captured(tool)[verb].clone();
            edited_element(&mut dropped_field, |element| {
                element.remove(dropped);
            });
            read_with(verb, dropped_field)
                .await
                .unwrap_or_else(|error| {
                    panic!("{verb}: a dropped `{dropped}` broke the read: {error}")
                });
        }
    }

    /// Run `edit` on the payload's only element: the object itself, or the first item of the array.
    fn edited_element(payload: &mut Value, edit: impl FnOnce(&mut serde_json::Map<String, Value>)) {
        match payload {
            Value::Object(map) => edit(map),
            Value::Array(items) => edit(items[0].as_object_mut().expect("object element")),
            Value::Null => panic!("the captured payload is null"),
            Value::Bool(_) | Value::Number(_) | Value::String(_) => {
                panic!("the captured payload is a scalar")
            }
        }
    }
}
