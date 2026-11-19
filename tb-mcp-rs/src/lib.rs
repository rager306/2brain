mod bounded_stdio;
mod capability;
pub mod cli;
mod contract;
mod describe;
mod gate;
mod help;
mod ledger;
mod policy;
mod resources;
mod schemas;
mod transport;
mod wire;

pub(crate) use resources::*;
pub(crate) use transport::*;
pub(crate) use wire::*;

use std::{
    borrow::Cow,
    env,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use rmcp::{
    RoleServer, ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, tool::ToolCallContext, wrapper::Parameters},
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock,
        GetPromptRequestParams, GetPromptResponse, Implementation, InitializeRequestParams,
        InitializeResult, ListPromptsResult, ListResourceTemplatesResult, ListResourcesResult,
        ListToolsResult, MetaObject, PaginatedRequestParams, ProtocolVersion,
        ReadResourceRequestParams, ReadResourceResponse, ReadResourceResult, ResourceContents,
        ResourceTemplate, ServerCapabilities, ServerConfig,
    },
    service::RequestContext,
    tool, tool_handler, tool_router,
};
use serde_json::{Value, json};
use tokio::sync::Semaphore;

// The in-crate tests speak the engine wire directly, so they need the IO traits
// and the socket type; the server itself goes through `wire`.
#[cfg(test)]
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// The protocol revisions this server answers. `2026-07-28` is the inline
/// lifecycle: a request carries its version in `_meta` and needs no handshake.
const SUPPORTED_PROTOCOL_VERSIONS: &[ProtocolVersion] = &[
    ProtocolVersion::V_2024_11_05,
    ProtocolVersion::V_2025_03_26,
    ProtocolVersion::V_2025_06_18,
    ProtocolVersion::V_2025_11_25,
    ProtocolVersion::V_2026_07_28,
];
const ENGINE_READ_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_IN_FLIGHT_READS: usize = 8;

#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct EmptyArguments {}

#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct TypeArguments {
    #[schemars(length(min = 1))]
    name: String,
    #[schemars(length(min = 1))]
    repo: Option<String>,
}

fn default_summary() -> bool {
    true
}

fn default_limit() -> usize {
    50
}

fn default_grep_limit() -> usize {
    20
}

fn default_graph_health_limit() -> usize {
    20
}

fn default_watch_timeout() -> u64 {
    10_000
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct TypesArguments {
    /// Limit the read to a named workspace member. Unknown members return null.
    repo: Option<String>,
    /// Return lightweight type summaries rather than full definitions (default true).
    #[serde(default = "default_summary")]
    summary: bool,
    /// Maximum number of entries, from 1 to 500 (default 50).
    #[serde(default = "default_limit")]
    #[schemars(range(min = 1, max = 500))]
    limit: usize,
    /// Number of entries to skip (default 0).
    #[serde(default)]
    offset: usize,
}

fn default_hubs_limit() -> usize {
    20
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
enum HubScope {
    Own,
    All,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct HubsArguments {
    /// Select targets in a named workspace member.
    #[schemars(length(min = 1))]
    repo: Option<String>,
    /// Select own or all targets. When omitted, defaults to own, or all if repo is provided.
    scope: Option<HubScope>,
    /// Maximum number of entries, from 1 to 500 (default 20).
    #[serde(default = "default_hubs_limit")]
    #[schemars(range(min = 1, max = 500))]
    limit: usize,
    /// Number of entries to skip (default 0).
    #[serde(default)]
    offset: usize,
}

#[derive(serde::Deserialize, serde::Serialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
enum DiagnosticSeverity {
    Error,
    Drift,
    Warning,
    Hint,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct DiagnosticsArguments {
    /// Select diagnostics owned by this workspace member.
    #[schemars(length(min = 1))]
    repo: Option<String>,
    /// Select exactly this severity.
    severity: Option<DiagnosticSeverity>,
    /// Select exactly this diagnostic code.
    #[schemars(length(min = 1))]
    code: Option<String>,
    /// Defaults to own, or all if repo is supplied.
    scope: Option<HubScope>,
    /// Maximum number of entries, from 1 to 500 (default 50).
    #[serde(default = "default_limit")]
    #[schemars(range(min = 1, max = 500))]
    limit: usize,
    /// Number of entries to skip (default 0).
    #[serde(default)]
    offset: usize,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct SubtypesArguments {
    /// Base type name, optionally qualified as name::repo.
    #[schemars(length(min = 1))]
    base: String,
    /// Select subtype definitions from own or all members (default all).
    scope: Option<HubScope>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct MembersArguments {
    /// Select a named workspace member; unknown members return an empty array.
    #[schemars(length(min = 1))]
    repo: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct ImportsArguments {
    /// Filter by importing member: own or all (default all).
    scope: Option<HubScope>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct FilesArguments {
    /// Select a named workspace member.
    #[schemars(length(min = 1))]
    repo: Option<String>,
    /// Select files in own or all members (default all).
    scope: Option<HubScope>,
    /// Maximum number of entries, from 1 to 500 (default 50).
    #[serde(default = "default_limit")]
    #[schemars(range(min = 1, max = 500))]
    limit: usize,
    /// Number of entries to skip (default 0).
    #[serde(default)]
    offset: usize,
}

/// One operation inside `tb_batch`. Each carries its own freshness hash: the
/// batch is one engine saga, but every file in it still passed the same gate it
/// would pass alone.
// The variant names ARE the wire tags (`write_file`, `edit_file`, `delete_file`),
// so the shared `File` postfix is the protocol, not a naming accident.
#[allow(clippy::enum_variant_names)]
#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum BatchOperation {
    WriteFile {
        /// Path relative to the member root.
        #[schemars(length(min = 1))]
        path: String,
        /// Full file text.
        content: String,
        /// Hash of `path` from its tb_content read.
        #[schemars(length(min = 1))]
        expected_hash: String,
    },
    EditFile {
        /// Path relative to the member root.
        #[schemars(length(min = 1))]
        path: String,
        old_string: String,
        new_string: String,
        /// Replace every occurrence instead of requiring a unique match.
        #[serde(default)]
        replace_all: bool,
        /// Hash of `path` from its tb_content read.
        #[schemars(length(min = 1))]
        expected_hash: String,
    },
    DeleteFile {
        /// Path relative to the member root.
        #[schemars(length(min = 1))]
        path: String,
        /// Hash of `path` from its tb_content read.
        #[schemars(length(min = 1))]
        expected_hash: String,
    },
}

impl BatchOperation {
    fn path(&self) -> &str {
        match self {
            Self::WriteFile { path, .. }
            | Self::EditFile { path, .. }
            | Self::DeleteFile { path, .. } => path,
        }
    }

    fn expected_hash(&self) -> &str {
        match self {
            Self::WriteFile { expected_hash, .. }
            | Self::EditFile { expected_hash, .. }
            | Self::DeleteFile { expected_hash, .. } => expected_hash,
        }
    }
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct BatchArguments {
    /// Member that owns every path in the batch.
    #[schemars(length(min = 1))]
    repo: String,
    /// Operations, applied in order and committed once.
    #[schemars(length(min = 1, max = 50))]
    ops: Vec<BatchOperation>,
    /// Refuse before writing when the state the whole set would produce carries an
    /// ERROR-severity diagnostic, naming the operation that would introduce it. The
    /// whole batch is compensated, so a refused set leaves every file as it was.
    #[serde(default)]
    require_clean: bool,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct WatchArguments {
    /// How long to wait for the next change, in milliseconds (default 10000,
    /// range 1..=30000).
    #[serde(default = "default_watch_timeout")]
    #[schemars(range(min = 1, max = 30000))]
    timeout_ms: u64,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct RollbackPreviewArguments {
    /// Member whose working tree holds the commit.
    #[schemars(length(min = 1))]
    repo: String,
    /// Commit to preview reverting.
    #[schemars(length(min = 1))]
    commit: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct GraphHealthArguments {
    /// Restrict the node set to one member.
    #[schemars(length(min = 1))]
    repo: Option<String>,
    /// Select own or all members (default all).
    scope: Option<HubScope>,
    /// Maximum entries per list, from 1 to 200 (default 20).
    #[serde(default = "default_graph_health_limit")]
    #[schemars(range(min = 1, max = 200))]
    limit: usize,
    /// Include the engine's per-verb request counts and latency histogram.
    ///
    /// Diagnostic: it describes the engine process, not the graph, so it rides beside the payload as a
    /// `verbs` sibling key rather than inside `graph_health`. Off by default, because a client that does not
    /// ask should not pay for it in bytes.
    #[serde(default)]
    verbs: bool,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct GrepArguments {
    /// Literal text to find; not a regular expression.
    #[schemars(length(min = 1))]
    query: String,
    /// Search only this member's files.
    #[schemars(length(min = 1))]
    repo: Option<String>,
    /// Select own or all members (default own; `all` widens to every mounted member).
    scope: Option<HubScope>,
    /// Match case exactly (default false).
    #[serde(default)]
    case_sensitive: bool,
    /// Maximum number of matches, from 1 to 200 (default 20).
    #[serde(default = "default_grep_limit")]
    #[schemars(range(min = 1, max = 200))]
    limit: usize,
    /// Number of matches to skip (default 0).
    #[serde(default)]
    offset: usize,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct SearchArguments {
    /// Words to rank notes by; not a regular expression.
    #[schemars(length(min = 1))]
    query: String,
    /// Search only this member's files.
    #[schemars(length(min = 1))]
    repo: Option<String>,
    /// Select own or all members (default own; `all` widens to every mounted member).
    scope: Option<HubScope>,
    /// Require every term instead of a ranked subset. Use it for punctuation or a
    /// two-word phrase; ranking alone can put a heavier document first.
    #[serde(default)]
    exact: bool,
    /// Minimum number of query terms a note must hold (default: all terms for 1-2
    /// terms, 2 above that).
    min_should_match: Option<usize>,
    /// Maximum number of matches, from 1 to 100 (default 20).
    #[serde(default = "default_rank_limit")]
    #[schemars(range(min = 1, max = 100))]
    limit: usize,
    /// Number of matches to skip (default 0).
    #[serde(default)]
    offset: usize,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct FileArguments {
    /// Workspace member that owns the file.
    #[schemars(length(min = 1))]
    repo: String,
    /// Path relative to that member's root; absolute paths and `..` are rejected.
    #[schemars(length(min = 1))]
    path: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct PageArguments {
    /// Return at most this many records.
    #[schemars(range(min = 1))]
    limit: Option<usize>,
    /// Skip this many records before the page.
    offset: Option<usize>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct CandidatesArguments {
    /// Project each file to its candidate type names.
    #[serde(default)]
    summary: bool,
    /// Maximum files, from 1 to 500 (default: engine default).
    #[schemars(range(min = 1, max = 500))]
    limit: Option<usize>,
    /// Files to skip.
    #[schemars(range(min = 0))]
    offset: Option<usize>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReferencesArguments {
    /// Workspace member that owns the file.
    #[schemars(length(min = 1))]
    repo: String,
    /// Path relative to that member's root; absolute paths and `..` are rejected.
    #[schemars(length(min = 1))]
    path: String,
    /// Which edges to return; default both.
    #[serde(default)]
    direction: ReferenceDirection,
}

#[derive(serde::Deserialize, schemars::JsonSchema, Default, Clone, Copy)]
#[serde(rename_all = "lowercase")]
enum ReferenceDirection {
    /// Edges the file authors.
    Out,
    /// Edges pointing at the file.
    In,
    /// Both, as two payload keys.
    #[default]
    Both,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct FollowArguments {
    /// Workspace member that owns the file the link appears in.
    #[schemars(length(min = 1))]
    repo: String,
    /// Path of the file the link appears in (or would), relative to the member root.
    #[schemars(length(min = 1))]
    origin_path: String,
    /// The wikilink target to resolve, e.g. `note`, `note::repo`, `note#Heading`, `note^blk`, `note^^blk`.
    #[schemars(length(min = 1))]
    target: String,
    /// Include the addressed line as a snippet; default true.
    #[serde(default = "default_true")]
    peek: bool,
}

fn default_true() -> bool {
    true
}

/// A wikilink split into its targets, without judging the grammar.
///
/// The engine owns the grammar and the verdict; this only decides which resolve
/// verb to ask. `#anchor` precedes `^block` in the canonical order, and `^^id` is
/// the block-referent form.
struct FollowParts {
    base: String,
    anchor: Option<String>,
    block: Option<String>,
    referent: bool,
}

fn split_follow(target: &str) -> FollowParts {
    let mut base_end = target.len();
    let mut anchor = None;
    let mut block = None;
    let mut referent = false;
    // The canonical order is `#anchor` then `^block`; whichever comes first ends
    // the file part. The engine validates the grammar, this only routes the call.
    if let Some((start, _)) = target
        .char_indices()
        .find(|(_, character)| *character == '#' || *character == '^')
    {
        base_end = start;
        let rest = &target[start..];
        if let Some(rest) = rest.strip_prefix('#') {
            let (anchor_text, tail) = match rest.find('^') {
                Some(cut) => (&rest[..cut], Some(&rest[cut..])),
                None => (rest, None),
            };
            anchor = Some(anchor_text.to_owned());
            if let Some(tail) = tail {
                let (id, double) = match tail.strip_prefix("^^") {
                    Some(id) => (id, true),
                    None => (tail.trim_start_matches('^'), false),
                };
                referent = double;
                block = Some(id.to_owned());
            }
        } else {
            let (id, double) = match rest.strip_prefix("^^") {
                Some(id) => (id, true),
                None => (rest.trim_start_matches('^'), false),
            };
            referent = double;
            block = Some(id.to_owned());
        }
    }
    FollowParts {
        base: target[..base_end].to_owned(),
        anchor,
        block,
        referent,
    }
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct NeighborhoodArguments {
    /// Workspace member that owns the seed file.
    #[schemars(length(min = 1))]
    repo: String,
    /// Seed path relative to that member's root.
    #[schemars(length(min = 1))]
    path: String,
    /// Walk direction; default out.
    direction: Option<WalkDirection>,
    /// Maximum hop count; default 1. Beyond 1 the engine requires `kinds`.
    #[schemars(range(min = 1, max = 8))]
    depth: Option<u8>,
    /// Edge kinds to follow, required past depth 1.
    kinds: Option<Vec<WalkKind>>,
    /// Prune at the repo boundary (`own`) or cross it (`all`); default all.
    scope: Option<WalkScope>,
    /// Node cap, seed included.
    #[schemars(range(min = 1))]
    max_nodes: Option<u32>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
enum WalkDirection {
    Out,
    In,
    Both,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
enum WalkKind {
    Navigational,
    Contributing,
    #[serde(rename = "field")]
    FieldReference,
    CommitReferent,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
enum WalkScope {
    All,
    Own,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct ContentArguments {
    /// Workspace member that owns the file.
    #[schemars(length(min = 1))]
    repo: String,
    /// Path relative to that member's root; absolute paths and `..` are rejected.
    #[schemars(length(min = 1))]
    path: String,
    /// Return at most this many characters, with a continuation offset for the
    /// rest. The hash still covers the whole file, so a later write is safe.
    #[schemars(range(min = 1))]
    max_chars: Option<usize>,
}

/// Whether a result carries a text block, structured content, or both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PayloadMode {
    /// Both, which the specification recommends for backward compatibility.
    Both,
    /// Only the text block.
    Text,
    /// Only structured content, for clients that read it and would otherwise
    /// pay for the same payload twice.
    Structured,
}

/// The refusals every file-addressed read answers before it touches the engine, in one place:
/// an empty member, an empty path, a path that is not member-relative, and an already-cancelled
/// request. Each is a local answer, so no transport is involved and the order is the contract.
fn file_read_guard(repo: &str, path: &str, cancelled: bool) -> Result<(), CallToolResult> {
    let refusal = |message: &str| {
        Err(CallToolResult::error(vec![ContentBlock::text(
            message.to_owned(),
        )]))
    };
    if repo.is_empty() {
        return refusal("'repo' must not be empty");
    }
    if path.is_empty() {
        return refusal("'path' must not be empty");
    }
    if let Err(error) = relative_path(path) {
        return refusal(error);
    }
    if cancelled {
        return refusal("engine read cancelled");
    }
    Ok(())
}

/// How much of each tool definition the server advertises up front.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolProfile {
    /// Full descriptions, schemas, and output schemas for every tool.
    Full,
    /// One line per tool, no output schema; `tb_describe_tool` serves the rest.
    Concise,
}

#[derive(Clone)]
struct Server {
    transport: Arc<dyn EngineTransport>,
    policy: policy::Policy,
    principal: String,
    audit: policy::Audit,
    initialize_called: Arc<AtomicBool>,
    read_slots: Arc<Semaphore>,
    mutation_slot: Arc<Semaphore>,
    ledger: ledger::ReadLedger,
    profile: ToolProfile,
    payload: PayloadMode,
    tool_router: ToolRouter<Self>,
}

/// A deserialization failure names the missing field but not the tool or the
#[tool_router]
impl Server {
    fn new(
        socket: PathBuf,
        policy: policy::Policy,
        principal: String,
        profile: ToolProfile,
        payload: PayloadMode,
    ) -> Self {
        Self::with_transport(
            Arc::new(UnixTransport::new(socket)),
            policy,
            principal,
            profile,
            payload,
        )
    }

    /// Build a server over any transport. `new` is the production caller; a
    /// test supplies a `MemoryTransport` here instead of a socket.
    fn with_transport(
        transport: Arc<dyn EngineTransport>,
        policy: policy::Policy,
        principal: String,
        profile: ToolProfile,
        payload: PayloadMode,
    ) -> Self {
        let audit = policy::Audit::new(&policy.audit);
        Self {
            transport,
            policy,
            principal,
            audit,
            initialize_called: Arc::new(AtomicBool::new(false)),
            read_slots: Arc::new(Semaphore::new(MAX_IN_FLIGHT_READS)),
            mutation_slot: Arc::new(Semaphore::new(1)),
            ledger: ledger::ReadLedger::new(),
            profile,
            payload,
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        description = "Get the workspace overview. Members expose dependency topology and absolute paths for all members, not only own scope.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true
        ),
        output_schema = crate::schemas::envelope_schema("overview"),
    )]
    async fn tb_overview(
        &self,
        _arguments: Parameters<EmptyArguments>,
        ctx: RequestContext<RoleServer>,
    ) -> CallToolResult {
        self.read(json!({"id": 1}), "tb_overview", ctx).await
    }

    #[tool(
        description = "List top-level directories in own scope.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true
        ),
        output_schema = crate::schemas::envelope_schema("top_level_dirs"),
    )]
    async fn tb_top_level_dirs(
        &self,
        _arguments: Parameters<EmptyArguments>,
        ctx: RequestContext<RoleServer>,
    ) -> CallToolResult {
        self.read(json!({"id": 1}), "tb_top_level_dirs", ctx).await
    }

    #[tool(
        description = "Get the complete, unpaged type tree for the whole workspace, including dependencies.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true
        ),
        output_schema = crate::schemas::envelope_schema("type_tree"),
    )]
    async fn tb_type_tree(
        &self,
        _arguments: Parameters<EmptyArguments>,
        ctx: RequestContext<RoleServer>,
    ) -> CallToolResult {
        self.read(json!({"id": 1}), "tb_type_tree", ctx).await
    }

    #[tool(
        description = "Get one type definition by name, across the workspace or in a named member. An unknown type or member returns null.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true
        ),
        output_schema = crate::schemas::envelope_schema("type"),
    )]
    async fn tb_type(
        &self,
        Parameters(args): Parameters<TypeArguments>,
        ctx: RequestContext<RoleServer>,
    ) -> CallToolResult {
        if args.name.is_empty() {
            return CallToolResult::error(vec![ContentBlock::text("'name' is required")]);
        }
        if args.repo.as_deref() == Some("") {
            return CallToolResult::error(vec![ContentBlock::text("'repo' must not be empty")]);
        }
        let name = match args.repo {
            Some(repo) if !args.name.contains("::") => format!("{}::{repo}", args.name),
            _ => args.name,
        };
        self.read(json!({"id": 1, "name": name}), "tb_type", ctx)
            .await
    }

    #[tool(
        description = "List a name-sorted page of types across the workspace or in a named member. This experimental interface differs from the existing MCP tool: it uses summary instead of detail, returns the engine envelope without counts, and rejects limits outside 1..=500 instead of clamping. Defaults: summary=true, limit=50, offset=0. A short page indicates the end; no total is returned. An unknown member returns null, distinct from an empty array. Offset pagination is not a snapshot: graph changes between requests can cause duplicates or omissions. The _meta engineVersion is a diagnostic-only decimal string; without daemon identity, equal versions do not establish the same snapshot, and metadata may be dropped.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true
        ),
        output_schema = crate::schemas::envelope_schema("types"),
    )]
    async fn tb_types(
        &self,
        Parameters(args): Parameters<TypesArguments>,
        ctx: RequestContext<RoleServer>,
    ) -> CallToolResult {
        if !(1..=500).contains(&args.limit) {
            return CallToolResult::error(vec![ContentBlock::text(
                "'limit' must be between 1 and 500",
            )]);
        }
        let mut request =
            json!({"id": 1, "summary": args.summary, "limit": args.limit, "offset": args.offset});
        if let Some(repo) = args.repo {
            request["repo"] = json!(repo);
        }
        self.read(request, "tb_types", ctx).await
    }

    #[tool(
        description = "List a ranked page of hubs. Scope and repo filter targets only; inbound counts include all incoming edges, including sources outside the selected scope. Scope defaults to own, or all when repo is supplied. An unknown repo returns an empty list. Paths may be absolute. Defaults: limit=20, offset=0; limits outside 1..=500 are rejected. Offset pagination is not a snapshot: graph changes can cause duplicates or omissions. The _meta engineVersion is diagnostic only and does not establish a snapshot.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true
        ),
        output_schema = crate::schemas::envelope_schema("hubs"),
    )]
    async fn tb_hubs(
        &self,
        Parameters(args): Parameters<HubsArguments>,
        ctx: RequestContext<RoleServer>,
    ) -> CallToolResult {
        if args.repo.as_deref() == Some("") {
            return CallToolResult::error(vec![ContentBlock::text("'repo' must not be empty")]);
        }
        if !(1..=500).contains(&args.limit) {
            return CallToolResult::error(vec![ContentBlock::text(
                "'limit' must be between 1 and 500",
            )]);
        }
        let mut request = json!({"id": 1, "limit": args.limit, "offset": args.offset});
        if let Some(repo) = args.repo {
            request["repo"] = json!(repo);
        }
        if let Some(scope) = args.scope {
            request["scope"] = json!(match scope {
                HubScope::Own => "own",
                HubScope::All => "all",
            });
        }
        self.read(request, "tb_hubs", ctx).await
    }

    #[tool(
        description = "List a page of diagnostics. Filters compose with AND. Scope defaults to own, or all when repo is supplied; an empty page says nothing about other scopes. Paths, messages and suggested actions may reveal workspace data; suggested actions are not authorization to mutate. Defaults: limit=50, offset=0, limit range 1..=500. A short page signals the end; pages are not a snapshot and changes may cause omissions or duplicates.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true
        ),
        output_schema = crate::schemas::envelope_schema("diagnostics"),
    )]
    async fn tb_diagnostics(
        &self,
        Parameters(args): Parameters<DiagnosticsArguments>,
        ctx: RequestContext<RoleServer>,
    ) -> CallToolResult {
        if args.repo.as_deref() == Some("") {
            return CallToolResult::error(vec![ContentBlock::text("'repo' must not be empty")]);
        }
        if args.code.as_deref() == Some("") {
            return CallToolResult::error(vec![ContentBlock::text("'code' must not be empty")]);
        }
        if !(1..=500).contains(&args.limit) {
            return CallToolResult::error(vec![ContentBlock::text(
                "'limit' must be between 1 and 500",
            )]);
        }
        let mut request = json!({"id": 1, "limit": args.limit, "offset": args.offset});
        if let Some(repo) = args.repo {
            request["repo"] = json!(repo);
        }
        if let Some(severity) = args.severity {
            request["severity"] = json!(severity);
        }
        if let Some(code) = args.code {
            request["code"] = json!(code);
        }
        if let Some(scope) = args.scope {
            request["scope"] = json!(match scope {
                HubScope::Own => "own",
                HubScope::All => "all",
            });
        }
        self.read(request, "tb_diagnostics", ctx).await
    }

    #[tool(
        description = "List all direct and transitive subtypes of a base type. A bare base matches by name across members; name::repo selects that base identity. Scope filters the member holding each subtype, and defaults to all. Unknown bases return an empty array. Identical definitions in different members remain separate. Results are unpaged and may contain paths and workspace content; frames over 16 MiB fail.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true
        ),
        output_schema = crate::schemas::subtypes_schema(),
    )]
    async fn tb_subtypes(
        &self,
        Parameters(args): Parameters<SubtypesArguments>,
        ctx: RequestContext<RoleServer>,
    ) -> CallToolResult {
        if args.base.is_empty() {
            return CallToolResult::error(vec![ContentBlock::text("'base' must not be empty")]);
        }
        let mut request = json!({"id": 1, "base": args.base});
        if let Some(scope) = args.scope {
            request["scope"] = json!(match scope {
                HubScope::Own => "own",
                HubScope::All => "all",
            });
        }
        self.read(request, "tb_subtypes", ctx).await
    }

    #[tool(
        description = "List workspace members, optionally selecting an exact repo. Unknown repos return an empty array. The result includes dependencies and disabled members, their roles and local/editable flags, absolute roots and Git topology. It is unpaged and may reveal workspace topology; it is not an authorization boundary.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true
        ),
        output_schema = crate::schemas::envelope_schema("members"),
    )]
    async fn tb_members(
        &self,
        Parameters(args): Parameters<MembersArguments>,
        ctx: RequestContext<RoleServer>,
    ) -> CallToolResult {
        if args.repo.as_deref() == Some("") {
            return CallToolResult::error(vec![ContentBlock::text("'repo' must not be empty")]);
        }
        let mut request = json!({"id": 1});
        if let Some(repo) = args.repo {
            request["repo"] = json!(repo);
        }
        self.read(request, "tb_members", ctx).await
    }

    #[tool(
        description = "List resolved cross-repo type imports. Scope filters the importing members: all by default, or own to exclude other importers and the automatic engine builtin fold. Referenced field shapes, unresolved imports, and dependencies without type use are excluded. The unpaged result may reveal repo and type names; it is not an authorization boundary.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true
        ),
        output_schema = crate::schemas::envelope_schema("imports"),
    )]
    async fn tb_imports(
        &self,
        Parameters(args): Parameters<ImportsArguments>,
        ctx: RequestContext<RoleServer>,
    ) -> CallToolResult {
        let mut request = json!({"id": 1});
        if let Some(scope) = args.scope {
            request["scope"] = json!(match scope {
                HubScope::Own => "own",
                HubScope::All => "all",
            });
        }
        self.read(request, "tb_imports", ctx).await
    }

    #[tool(
        description = "List a page of catalogued files, including assets and dependencies by default. Filters repo and scope compose; scope defaults to all. Paths may be absolute and stems are not unique. Order is by path components, not plain string order. Defaults: limit=50, offset=0, limit range 1..=500. Pages are not a snapshot; concurrent changes may cause omissions or duplicates. This tool is not an authorization boundary.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true
        ),
        output_schema = crate::schemas::envelope_schema("files"),
    )]
    async fn tb_files(
        &self,
        Parameters(args): Parameters<FilesArguments>,
        ctx: RequestContext<RoleServer>,
    ) -> CallToolResult {
        if args.repo.as_deref() == Some("") {
            return CallToolResult::error(vec![ContentBlock::text("'repo' must not be empty")]);
        }
        if !(1..=500).contains(&args.limit) {
            return CallToolResult::error(vec![ContentBlock::text(
                "'limit' must be between 1 and 500",
            )]);
        }
        let mut request = json!({"id":1,"limit":args.limit,"offset":args.offset});
        if let Some(repo) = args.repo {
            request["repo"] = json!(repo);
        }
        if let Some(scope) = args.scope {
            request["scope"] = json!(match scope {
                HubScope::Own => "own",
                HubScope::All => "all",
            });
        }
        self.read(request, "tb_files", ctx).await
    }

    #[tool(
        description = "Wait for the next change to the workspace, up to timeout_ms (default 10000, max 30000). Returns the net file delta the engine reports for the first rebuild after the call: added, removed, and modified paths, plus the version. A timeout is an ordinary answer (timed_out true), not an error — call it again to keep watching. It holds the tool call for that long, so keep the timeout short; a client that cannot wait at all should poll tb_files and diff on its side.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = false
        ),
        output_schema = crate::schemas::watch_schema(),
    )]
    async fn tb_watch(
        &self,
        Parameters(args): Parameters<WatchArguments>,
        ctx: RequestContext<RoleServer>,
    ) -> CallToolResult {
        if !(1..=30_000).contains(&args.timeout_ms) {
            return return_error("'timeout_ms' must be between 1 and 30000");
        }
        let timeout = std::time::Duration::from_millis(args.timeout_ms);
        tokio::select! {
            biased;
            _ = ctx.ct.cancelled() => return_error("watch cancelled"),
            outcome = watch_engine(self.transport.as_ref(), timeout) => match outcome {
                Ok(watch) => success_payload(json!({ "watch": watch }), self.payload),
                Err(error) => return_error(error),
            },
        }
    }

    #[tool(
        description = "Preview reverting one commit without touching anything: the paths it changed, whether a revert would restore the parent's bytes or delete the path, and whether each path is still safe (clean, and no later commit rewrote it). `safe` is false when any path is dirty or superseded, because a revert is all-or-nothing. Read-only: it is a plan, not an action.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true
        ),
        output_schema = crate::schemas::rollback_preview_schema(),
    )]
    async fn tb_rollback_preview(
        &self,
        Parameters(args): Parameters<RollbackPreviewArguments>,
        ctx: RequestContext<RoleServer>,
    ) -> CallToolResult {
        if args.repo.is_empty() {
            return CallToolResult::error(vec![ContentBlock::text("'repo' must not be empty")]);
        }
        if args.commit.is_empty() {
            return CallToolResult::error(vec![ContentBlock::text("'commit' must not be empty")]);
        }
        let request = json!({
            "id": 1,
            "repo": args.repo,
            "commit": args.commit,
        });
        self.read(request, "tb_rollback_preview", ctx).await
    }

    #[tool(
        description = "Report what is wrong with the graph in one answer: directed cycles (strongly connected components, shortest first), dead ends (nothing links onward), sources (nothing points at them), isolated files, broken links (reference diagnostics: target missing, ambiguous, or outside the repo), and the most central nodes by total degree. It also names the graph's fragile joints on the undirected projection of the same edges: bridges (an edge whose removal disconnects its ends) and articulation points (a node whose removal does); a pair joined by more than one edge is never a bridge. It also reports how tightly that projection is woven: its triangle count and global transitivity. Counts cover the whole in-scope graph; each list is capped by limit (default 20, max 200). A dangling link is only diagnosed on a file that claims a type, so an untyped note's broken link is not listed.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true
        ),
        output_schema = crate::schemas::graph_health_schema(),
    )]
    async fn tb_graph_health(
        &self,
        Parameters(args): Parameters<GraphHealthArguments>,
        ctx: RequestContext<RoleServer>,
    ) -> CallToolResult {
        if args.repo.as_deref() == Some("") {
            return CallToolResult::error(vec![ContentBlock::text("'repo' must not be empty")]);
        }
        if !(1..=200).contains(&args.limit) {
            return CallToolResult::error(vec![ContentBlock::text(
                "'limit' must be between 1 and 200",
            )]);
        }
        let mut request = json!({"id": 1, "limit": args.limit});
        if let Some(repo) = args.repo {
            request["repo"] = json!(repo);
        }
        if let Some(scope) = args.scope {
            request["scope"] = json!(match scope {
                HubScope::Own => "own",
                HubScope::All => "all",
            });
        }
        if args.verbs {
            request["verbs"] = json!(true);
        }
        self.read(request, "tb_graph_health", ctx).await
    }

    #[tool(
        description = "Search the catalogued files for literal text, one match per line. Not a regular expression: it answers 'which notes mention this phrase'. Case-insensitive unless case_sensitive is set; repo and scope compose, and scope defaults to the workspace's own members rather than the dependencies mounted beside them (scope='all' widens); defaults limit=20, offset=0, limit range 1..=200. The answer carries matches, the total over the whole scanned set, truncated, and skipped_files (files above 1 MiB or binary, counted rather than silently missed). Text is workspace content and may be sensitive; this is not an authorization boundary.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true
        ),
        output_schema = crate::schemas::grep_schema(),
    )]
    async fn tb_grep(
        &self,
        Parameters(args): Parameters<GrepArguments>,
        ctx: RequestContext<RoleServer>,
    ) -> CallToolResult {
        if args.query.is_empty() {
            return CallToolResult::error(vec![ContentBlock::text("'query' must not be empty")]);
        }
        if args.repo.as_deref() == Some("") {
            return CallToolResult::error(vec![ContentBlock::text("'repo' must not be empty")]);
        }
        if !(1..=200).contains(&args.limit) {
            return CallToolResult::error(vec![ContentBlock::text(
                "'limit' must be between 1 and 200",
            )]);
        }
        let mut request = json!({
            "id": 1,
            "query": args.query,
            "limit": args.limit,
            "offset": args.offset,
        });
        if args.case_sensitive {
            request["case_sensitive"] = json!(true);
        }
        if let Some(repo) = args.repo {
            request["repo"] = json!(repo);
        }
        if let Some(scope) = args.scope {
            request["scope"] = json!(match scope {
                HubScope::Own => "own",
                HubScope::All => "all",
            });
        }
        self.read(request, "tb_grep", ctx).await
    }

    #[tool(
        description = "Rank notes by relevance to a query instead of scanning them. BM25 over each note's body and its frontmatter description, with the description weighted higher; the answer carries matches (path, score, matched_terms, and a short body excerpt cut to excerpt_chars so a hit can be judged without tb_content), the total, and paging. exact requires every term rather than a majority, which is what a two-word name needs; it does not require them adjacent, so a literal phrase is tb_grep's job. min_should_match defaults to all terms for one or two terms and to two above that, and it is a floor rather than a conjunction: a document is kept when it matches that many distinct terms or when one term it matched is rare in the corpus (present in about 13% of the notes or fewer), so a three-word query is not an OR over a third of the vault and still does not drop the note whose only matching word the base uses rarely. scope defaults to the workspace's own members, not the dependency repositories mounted beside it — scope='all' widens to every mounted member. Each match carries `weak`: true when every term it matched is common in the corpus, i.e. the hit says the words occur rather than pointing at something the base knows. A weak page is a reason to ask a sharper question, not proof the base is empty — a rare-but-unrelated term is not flagged, and a broad question like 'agent memory' is legitimately weak. Use tb_grep when you need the literal line, and this when you need the right notes.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true
        ),
        output_schema = crate::schemas::envelope_schema("search"),
    )]
    async fn tb_search(
        &self,
        Parameters(args): Parameters<SearchArguments>,
        ctx: RequestContext<RoleServer>,
    ) -> CallToolResult {
        if args.query.trim().is_empty() {
            return CallToolResult::error(vec![ContentBlock::text("'query' must not be empty")]);
        }
        if args.repo.as_deref() == Some("") {
            return CallToolResult::error(vec![ContentBlock::text("'repo' must not be empty")]);
        }
        if !(1..=100).contains(&args.limit) {
            return CallToolResult::error(vec![ContentBlock::text(
                "'limit' must be between 1 and 100",
            )]);
        }
        let mut request = json!({
            "id": 1,
            "query": args.query,
            "limit": args.limit,
            "offset": args.offset,
        });
        if args.exact {
            request["exact"] = json!(true);
        }
        if let Some(min) = args.min_should_match {
            request["min_should_match"] = json!(min);
        }
        if let Some(repo) = args.repo {
            request["repo"] = json!(repo);
        }
        if let Some(scope) = args.scope {
            request["scope"] = json!(match scope {
                HubScope::Own => "own",
                HubScope::All => "all",
            });
        }
        self.read(request, "tb_search", ctx).await
    }

    #[tool(
        description = "Read one member's file text, resolved from the member's declared root, with the content hash that guards a later write and the HEAD commit of the owning repo. The path must be relative to that member and must not contain '..'; absolute paths are rejected. max_chars returns a window with truncated, total_chars, and next_offset; the hash still covers the whole file, so a write based on it stays valid. The result is null when the file is unreadable. Text is workspace content and may be sensitive; it is not an authorization boundary.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true
        ),
        output_schema = crate::schemas::envelope_schema("content"),
    )]
    async fn tb_content(
        &self,
        Parameters(args): Parameters<ContentArguments>,
        ctx: RequestContext<RoleServer>,
    ) -> CallToolResult {
        if let Err(refusal) = file_read_guard(&args.repo, &args.path, ctx.ct.is_cancelled()) {
            return refusal;
        }
        let Ok(_permit) = self.read_slots.try_acquire() else {
            return CallToolResult::error(vec![ContentBlock::text(
                "engine read capacity reached; retry later",
            )]);
        };
        let key = (args.repo.clone(), args.path.clone());
        let max_chars = args.max_chars;
        tokio::select! {
            biased;
            _ = ctx.ct.cancelled() => CallToolResult::error(vec![ContentBlock::text("engine read cancelled")]),
            result = self.content_with_timeout(args.repo, args.path, ENGINE_READ_TIMEOUT) => {
                if let Some(hash) = served_hash(&result) {
                    self.ledger.record(key, hash);
                }
                match max_chars {
                    Some(max) => window_content(result, max, self.payload),
                    None => result,
                }
            }
        }
    }

    #[tool(
        description = "Write one member's file text after a successful tb_content read of the same path. expected_hash is required and must equal the hash returned by that read; a stale or missing read is rejected without writing. The member must be editable. require_clean refuses before writing when the state the write would produce carries an ERROR-severity diagnostic, naming the codes — use it to promote a draft into accepted knowledge, not for ordinary drafting. A transport failure after the request is sent is reported as outcome_unknown and must not be retried blindly; an engine rejection means nothing was written. Writing invalidates the recorded read, so read again before another write.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false
        ),
        output_schema = crate::schemas::mutation_schema(),
    )]
    async fn tb_write_file(
        &self,
        Parameters(args): Parameters<WriteArguments>,
        _ctx: RequestContext<RoleServer>,
    ) -> CallToolResult {
        let expected_hash = args.expected_hash.clone();
        self.guarded_mutation(
            "tb_write_file",
            args.repo,
            args.path,
            args.expected_hash,
            move |target| {
                let mut request = json!({
                    "mutate": "write_file",
                    "id": 1,
                    "path": target,
                    "content": args.content,
                    "expected_hash": expected_hash,
                });
                crate::wire::with_clean_rider(&mut request, args.require_clean);
                request
            },
        )
        .await
    }

    #[tool(
        description = "Replace one exact string in a member's file after a successful tb_content read of the same path. expected_hash must equal the hash that read returned, checked against this server's read ledger; it does not freeze the whole file. The engine's own precondition is old_string itself: old_string must match exactly and uniquely unless replace_all is true, and a miss or a non-unique match rejects without writing. A concurrent change ELSEWHERE in the file therefore does not cancel this edit, by design — a fragment edit that only applied to a byte-identical file would refuse exactly the case of two agents editing different sections of one note. Use tb_write_file when the whole file must be unchanged since the read; that one passes expected_hash to the engine. Unknown outcomes are reported as outcome_unknown and must not be retried blindly.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false
        ),
        output_schema = crate::schemas::mutation_schema(),
    )]
    async fn tb_edit_file(
        &self,
        Parameters(args): Parameters<EditArguments>,
        _ctx: RequestContext<RoleServer>,
    ) -> CallToolResult {
        self.guarded_mutation(
            "tb_edit_file",
            args.repo,
            args.path,
            args.expected_hash,
            |target| {
                let mut request = json!({
                    "mutate": "edit_file",
                    "id": 1,
                    "path": target,
                    "old_string": args.old_string,
                    "new_string": args.new_string,
                    "replace_all": args.replace_all,
                });
                crate::wire::with_clean_rider(&mut request, args.require_clean);
                request
            },
        )
        .await
    }

    #[tool(
        description = "Delete one member's file after a successful tb_content read of the same path. expected_hash must equal the hash that read returned. The engine rejects an absent file and refuses to delete a path with uncommitted changes. Unknown outcomes are reported as outcome_unknown and must not be retried blindly.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false
        ),
        output_schema = crate::schemas::mutation_schema(),
    )]
    async fn tb_delete_file(
        &self,
        Parameters(args): Parameters<DeleteArguments>,
        _ctx: RequestContext<RoleServer>,
    ) -> CallToolResult {
        let expected_hash = args.expected_hash.clone();
        self.guarded_mutation(
            "tb_delete_file",
            args.repo,
            args.path,
            args.expected_hash,
            move |target| {
                json!({
                    "mutate": "delete_file",
                    "id": 1,
                    "path": target,
                    "expected_hash": expected_hash,
                })
            },
        )
        .await
    }

    #[tool(
        description = "Apply 1 to 50 file operations (write_file, edit_file, delete_file) as one all-or-nothing saga: every op passed the same gate it would pass alone (prior tb_content read of that path, matching expected_hash, editable member), the engine commits the whole set once per touched member, and any rejection leaves every file as it was. Use it when a change spans files and half of it landing would be worse than none. require_clean refuses the whole set before anything is written when the state it would produce carries an ERROR-severity diagnostic, naming the operation that would introduce it — use it to promote a set of drafts into accepted knowledge, not for ordinary drafting. A lost reply is outcome_unknown and must not be retried blindly.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false
        ),
        output_schema = crate::schemas::batch_schema(),
    )]
    async fn tb_batch(
        &self,
        Parameters(args): Parameters<BatchArguments>,
        _ctx: RequestContext<RoleServer>,
    ) -> CallToolResult {
        if args.repo.is_empty() {
            return return_error("'repo' must not be empty");
        }
        if args.ops.is_empty() {
            return return_error("'ops' must not be empty");
        }
        if args.ops.len() > 50 {
            return return_error("'ops' may hold at most 50 operations");
        }
        let repo = args.repo;
        // Arm every file first: any refusal means nothing is written, which is
        // the same promise the engine makes for the saga.
        let mut armed = Vec::with_capacity(args.ops.len());
        for op in &args.ops {
            match self.arm_file(&repo, op.path(), op.expected_hash()).await {
                Ok(file) => armed.push(file),
                Err(denial) => {
                    self.audit.record(
                        &self.principal,
                        "tb_batch",
                        &repo,
                        &format!("denied:{}", denial.stage.name()),
                    );
                    return return_error(denial.text());
                }
            }
        }
        let _permit = match self.take_slot() {
            Ok(permit) => permit,
            Err(denial) => {
                return return_error(denial.text());
            }
        };
        let keys: Vec<(String, String)> = armed.iter().map(|file| file.key.clone()).collect();
        let ops: Vec<Value> = args
            .ops
            .iter()
            .zip(&armed)
            .map(|(op, file)| match op {
                BatchOperation::WriteFile {
                    content,
                    expected_hash,
                    ..
                } => json!({
                    "op": "write_file",
                    "path": file.target,
                    "content": content,
                    "expected_hash": expected_hash,
                }),
                BatchOperation::EditFile {
                    old_string,
                    new_string,
                    replace_all,
                    ..
                } => json!({
                    "op": "edit_file",
                    "path": file.target,
                    "old_string": old_string,
                    "new_string": new_string,
                    "replace_all": replace_all,
                }),
                BatchOperation::DeleteFile { expected_hash, .. } => json!({
                    "op": "delete_file",
                    "path": file.target,
                    "expected_hash": expected_hash,
                }),
            })
            .collect();
        let mut request = json!({"mutate": "batch", "id": 1, "ops": ops});
        crate::wire::with_clean_rider(&mut request, args.require_clean);
        self.finish_mutation("tb_batch", &repo, request, keys).await
    }

    #[tool(
        description = "Create a new file in a member. The path must not exist: the server reads it first and refuses with 'already exists' rather than overwriting, and the engine re-checks absence where the write happens, so a competing create of the same path is refused instead of overwriting this one. Changing an existing file goes through tb_write_file. Writes commit like any other mutation; a lost reply is outcome_unknown and must not be retried blindly.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false
        ),
        output_schema = crate::schemas::mutation_schema(),
    )]
    async fn tb_create_file(
        &self,
        Parameters(args): Parameters<CreateArguments>,
        _ctx: RequestContext<RoleServer>,
    ) -> CallToolResult {
        let repo = args.repo;
        let (prepared, _permit) = match self.prepare_create(&repo, &args.path).await {
            Ok(prepared) => prepared,
            Err(denial) => {
                self.audit.record(
                    &self.principal,
                    "tb_create_file",
                    &repo,
                    &format!("denied:{}", denial.stage.name()),
                );
                return return_error(denial.text());
            }
        };
        let ArmedFile { key, target } = prepared;
        // The read above is this process's view; the engine's create is the
        // race-free half, because it takes the path inside the serialized write.
        let mut request = json!({
            "mutate": "create_file",
            "id": 1,
            "path": target,
            "content": args.content,
        });
        crate::wire::with_clean_rider(&mut request, args.require_clean);
        self.finish_mutation("tb_create_file", &repo, request, [key])
            .await
    }

    #[tool(
        description = "Declare a member in one workspace's .2brain/workspace.yaml, in the edit or discover list. Changing which members a workspace serves needs the workspace-shape grant on top of mutations; a member already declared in either list is refused. Nothing is fetched or mounted by this.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false
        ),
        output_schema = crate::schemas::mutation_schema(),
    )]
    async fn tb_add_workspace_member(
        &self,
        Parameters(args): Parameters<AddWorkspaceMemberArguments>,
        _ctx: RequestContext<RoleServer>,
    ) -> CallToolResult {
        if args.name.is_empty() {
            return return_error("'name' must not be empty");
        }
        let mut request = json!({
            "mutate": "add_workspace_member",
            "id": 1,
            "name": args.name,
            "role": workspace_role(args.role),
        });
        if let Some(hash) = args.expected_hash {
            request["expected_hash"] = json!(hash);
        }
        self.workspace_mutation("tb_add_workspace_member", args.repo, request)
            .await
    }

    #[tool(
        description = "Remove a member from one workspace's edit and discover lists. The containing repo cannot be removed. Needs the workspace-shape grant; changes take effect on the engine's rebuild.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false
        ),
        output_schema = crate::schemas::mutation_schema(),
    )]
    async fn tb_remove_workspace_member(
        &self,
        Parameters(args): Parameters<RemoveWorkspaceMemberArguments>,
        _ctx: RequestContext<RoleServer>,
    ) -> CallToolResult {
        if args.name.is_empty() {
            return return_error("'name' must not be empty");
        }
        let mut request = json!({
            "mutate": "remove_workspace_member",
            "id": 1,
            "name": args.name,
        });
        if let Some(hash) = args.expected_hash {
            request["expected_hash"] = json!(hash);
        }
        self.workspace_mutation("tb_remove_workspace_member", args.repo, request)
            .await
    }

    #[tool(
        description = "Move a declared member between the edit and discover lists in one commit. Refused when the member is absent, already in that role, or when moving the containing repo into discover. Needs the workspace-shape grant.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false
        ),
        output_schema = crate::schemas::mutation_schema(),
    )]
    async fn tb_set_workspace_member_role(
        &self,
        Parameters(args): Parameters<SetWorkspaceMemberRoleArguments>,
        _ctx: RequestContext<RoleServer>,
    ) -> CallToolResult {
        if args.name.is_empty() {
            return return_error("'name' must not be empty");
        }
        let mut request = json!({
            "mutate": "set_workspace_member_role",
            "id": 1,
            "name": args.name,
            "role": workspace_role(args.role),
        });
        if let Some(hash) = args.expected_hash {
            request["expected_hash"] = json!(hash);
        }
        self.workspace_mutation("tb_set_workspace_member_role", args.repo, request)
            .await
    }

    #[tool(
        description = "Fold one file into a referrer as an inline record and delete it, as one saga: the chosen [[path]] becomes the record, other whole-file links become local or host refs, and nested ids travel. Needs a prior tb_content read of both files and their hashes; a multi-reference host needs `at`.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false
        ),
        output_schema = crate::schemas::mutation_schema(),
    )]
    async fn tb_inline(
        &self,
        Parameters(args): Parameters<InlineArguments>,
        _ctx: RequestContext<RoleServer>,
    ) -> CallToolResult {
        if args.into.is_empty() {
            return return_error("'into' must not be empty");
        }
        for path in [&args.path, &args.into] {
            if let Err(error) = relative_path(path) {
                return return_error(error);
            }
        }
        let repo = args.repo;
        let (prepared_path, prepared_into, _permit) = match self
            .prepare_pair(
                (&repo, &args.path, &args.expected_hash),
                (&repo, &args.into, &args.into_expected_hash),
            )
            .await
        {
            Ok(armed) => armed,
            Err(denial) => {
                self.audit.record(
                    &self.principal,
                    "tb_inline",
                    &repo,
                    &format!("denied:{}", denial.stage.name()),
                );
                return return_error(denial.text());
            }
        };
        let mut request = json!({
            "mutate": "inline",
            "id": 1,
            "path": prepared_path.target,
            "into": prepared_into.target,
        });
        if let Some(at) = args.at {
            request["at"] = json!(at);
        }
        crate::wire::with_member_grant(&mut request, self.policy.member_allowlist());
        self.finish_mutation(
            "tb_inline",
            &repo,
            request,
            [prepared_path.key, prepared_into.key],
        )
        .await
    }

    #[tool(
        description = "Lift an inline record into its own file and leave a reference where it sat, as one saga: every [[host^id]] referrer follows to the new file, nested ids travel, and the format follows the extension (.md fences the YAML, .yaml does not). Locate the record with `at` or `block_id`, not both. Needs a prior tb_content read of the host.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false
        ),
        output_schema = crate::schemas::mutation_schema(),
    )]
    async fn tb_promote(
        &self,
        Parameters(args): Parameters<PromoteArguments>,
        _ctx: RequestContext<RoleServer>,
    ) -> CallToolResult {
        if args.to.is_empty() {
            return return_error("'to' must not be empty");
        }
        if let Err(error) = relative_path(&args.to) {
            return return_error(error);
        }
        match (&args.at, &args.block_id) {
            (Some(_), Some(_)) => {
                return return_error("give exactly one of 'at' or 'block_id'");
            }
            (None, None) => {
                return return_error("give exactly one of 'at' or 'block_id'");
            }
            _ => {}
        }
        let repo = args.repo;
        let to = match absolute_member_path(self.transport.as_ref(), &repo, &args.to).await {
            Ok(path) => path,
            Err(error) => return return_error(error),
        };
        let prepared = match self.arm_file(&repo, &args.path, &args.expected_hash).await {
            Ok(armed) => armed,
            Err(denial) => {
                self.audit.record(
                    &self.principal,
                    "tb_promote",
                    &repo,
                    &format!("denied:{}", denial.stage.name()),
                );
                return return_error(denial.text());
            }
        };
        let _permit = match self.take_slot() {
            Ok(permit) => permit,
            Err(denial) => {
                return return_error(denial.text());
            }
        };
        let mut request = json!({
            "mutate": "promote",
            "id": 1,
            "path": prepared.target,
            "to": to,
        });
        if let Some(at) = args.at {
            request["at"] = json!(at);
        }
        if let Some(block_id) = args.block_id {
            request["block_id"] = json!(block_id);
        }
        crate::wire::with_member_grant(&mut request, self.policy.member_allowlist());
        self.finish_mutation("tb_promote", &repo, request, [prepared.key])
            .await
    }

    #[tool(
        description = "Rename a type-def and carry every reference with it, as one saga: the def file moves and claims, slot shapes, meta, and wikilinks follow. expected_hash is the type's hash from tb_type, so a stale read cannot rename. Policy covers the WHOLE cascade: the engine previews every file the rename writes, and the call is refused with policy/member_grant (audited as denied:member_grant) when any of them lies in a member the launch does not allow — the defining member being allowed is not enough.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false
        ),
        output_schema = crate::schemas::mutation_schema(),
    )]
    async fn tb_rename_type(
        &self,
        Parameters(args): Parameters<RenameTypeArguments>,
        _ctx: RequestContext<RoleServer>,
    ) -> CallToolResult {
        if args.old_name.is_empty() {
            return return_error("'old_name' must not be empty");
        }
        if args.new_name.is_empty() {
            return return_error("'new_name' must not be empty");
        }
        if args.expected_hash.is_empty() {
            return return_error("'expected_hash' must not be empty");
        }
        let Ok(_permit) = self.mutation_slot.clone().try_acquire_owned() else {
            return return_error("another mutation is in progress; retry later");
        };
        // The gate: the type still has the hash the caller read, and its owner is
        // an allowed member. A type has no path, so its hash is the freshness check.
        let read = read_engine(
            self.transport.as_ref(),
            json!({"read": "type", "id": 1, "name": args.old_name}),
        )
        .await;
        let read = match read {
            Ok(read) => read,
            Err(error) => return return_error(error),
        };
        let definition = &read.payload["type"];
        if definition.is_null() {
            return return_error(format!("unknown type: {}", args.old_name));
        }
        let current = definition
            .get("hash")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if current != args.expected_hash {
            return return_error(
                "'expected_hash' does not match the hash returned by the last tb_type read",
            );
        }
        let owner = definition
            .get("repo")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        if !self.policy.permits_member(&owner) {
            self.audit.record(
                &self.principal,
                "tb_rename_type",
                &owner,
                "denied:member_grant",
            );
            return return_error(format!(
                "policy/member_grant: mutations are not allowed in workspace member {owner}"
            ));
        }
        // The cascade, not only the definition, must be inside this run's grant.
        // The engine's preview names every file the rename rewrites and the member
        // owning it, from the same planning code the mutation runs; the engine's own
        // consumed-member guard is the workspace's topology, a different question.
        let cascade = wire::rename_type_grant(
            self.transport.as_ref(),
            |member| self.policy.permits_member(member),
            &args.old_name,
            &args.new_name,
        )
        .await;
        let outside = match cascade {
            Ok(wire::CascadeGrant::Allowed) => None,
            // The mutation would refuse with the same words.
            Ok(wire::CascadeGrant::Rejected(message)) => {
                self.audit
                    .record(&self.principal, "tb_rename_type", &owner, "rejected");
                return return_error(message);
            }
            Ok(wire::CascadeGrant::Outside(outside)) => Some(outside),
            Err(error) => return return_error(error),
        };
        if let Some(outside) = outside {
            self.audit.record(
                &self.principal,
                "tb_rename_type",
                &outside[0].0,
                "denied:member_grant",
            );
            return return_error(
                policy::rename_type_cascade_denial(&args.old_name, &owner, &outside).text(),
            );
        }
        let mut request = json!({
            "mutate": "rename_type",
            "id": 1,
            "old_name": args.old_name,
            "new_name": args.new_name,
        });
        // ADR-0003: the allowlist travels WITH the mutation, so the engine checks the
        // cascade it is about to write instead of the one a preview saw earlier. Sent
        // only when the policy has an allowlist: an absent policy permits every member,
        // and an engine that predates the argument refuses an unknown field rather than
        // running the write unguarded.
        crate::wire::with_member_grant(&mut request, self.policy.member_allowlist());
        match read_mutation_with_timeout(self.transport.as_ref(), request, ENGINE_READ_TIMEOUT)
            .await
        {
            Ok(result) => {
                self.audit
                    .record(&self.principal, "tb_rename_type", &owner, "applied");
                success_result(result, self.payload)
            }
            Err(MutationFailure::Rejected { message, .. }) => {
                self.audit
                    .record(&self.principal, "tb_rename_type", &owner, "rejected");
                return_error(message)
            }
            Err(MutationFailure::Unknown(error)) => {
                self.audit
                    .record(&self.principal, "tb_rename_type", &owner, "outcome_unknown");
                return_error(format!(
                    "outcome_unknown: {error}. The rename may have landed; read the type again before retrying."
                ))
            }
        }
    }

    #[tool(
        description = "Declare a dependency in one member's .2brain/repo.yaml, as a byte splice that keeps comments and order. Nothing is fetched: the peer stays unmounted until a resolve. Requires both the mutation grant and the composition grant; expected_hash is a compare-and-set on that file.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false
        ),
        output_schema = crate::schemas::mutation_schema(),
    )]
    async fn tb_add_dep(
        &self,
        Parameters(args): Parameters<AddDepArguments>,
        _ctx: RequestContext<RoleServer>,
    ) -> CallToolResult {
        if args.dep.is_empty() {
            return return_error("'dep' must not be empty");
        }
        let mut request = json!({
            "mutate": "add_dep",
            "id": 1,
            "name": args.dep,
        });
        if let Some(remote) = args.remote {
            request["remote"] = json!(remote);
        }
        if let Some(reference) = args.reference {
            request["ref"] = json!(reference);
        }
        if let Some(hash) = args.expected_hash {
            request["expected_hash"] = json!(hash);
        }
        self.composition_mutation("tb_add_dep", args.repo, request)
            .await
    }

    #[tool(
        description = "Remove a dependency from one member's .2brain/repo.yaml, as a byte splice that keeps siblings and comments; a sole entry collapses deps to []. Requires both the mutation grant and the composition grant.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false
        ),
        output_schema = crate::schemas::mutation_schema(),
    )]
    async fn tb_remove_dep(
        &self,
        Parameters(args): Parameters<RemoveDepArguments>,
        _ctx: RequestContext<RoleServer>,
    ) -> CallToolResult {
        if args.dep.is_empty() {
            return return_error("'dep' must not be empty");
        }
        let mut request = json!({
            "mutate": "remove_dep",
            "id": 1,
            "name": args.dep,
        });
        if let Some(hash) = args.expected_hash {
            request["expected_hash"] = json!(hash);
        }
        self.composition_mutation("tb_remove_dep", args.repo, request)
            .await
    }

    #[tool(
        description = "Mint a block id for the entity at a byte offset and return the id and its [[target^id]] ref. Inline records gain a ^: key, markdown blocks a trailing ^marker. Assigning to an entity that already has one returns the existing id and writes nothing. Requires a prior tb_content read and its hash; one mutation at a time.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = true
        ),
        output_schema = crate::schemas::mutation_schema(),
    )]
    async fn tb_assign_block_id(
        &self,
        Parameters(args): Parameters<AssignBlockIdArguments>,
        _ctx: RequestContext<RoleServer>,
    ) -> CallToolResult {
        let at = args.at;
        self.guarded_mutation(
            "tb_assign_block_id",
            args.repo,
            args.path,
            args.expected_hash,
            move |target| json!({"mutate": "assign_block_id", "id": 1, "path": target, "at": at}),
        )
        .await
    }

    #[tool(
        description = "Rename an inline record's block id and carry every referrer with it: the ^: declaration and each [[host^id]] / [[^id]] rewrite as one mutation. Only inline records are renamed; a body marker is refused. Requires a prior tb_content read of the host and its hash.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false
        ),
        output_schema = crate::schemas::mutation_schema(),
    )]
    async fn tb_rename_block_id(
        &self,
        Parameters(args): Parameters<RenameBlockIdArguments>,
        _ctx: RequestContext<RoleServer>,
    ) -> CallToolResult {
        let (block_id, to_block_id) = (args.block_id, args.to_block_id);
        let grant = self.policy.member_allowlist().map(<[String]>::to_vec);
        self.guarded_mutation(
            "tb_rename_block_id",
            args.repo,
            args.path,
            args.expected_hash,
            move |target| {
                crate::wire::rename_block_id_request(
                    target,
                    &block_id,
                    &to_block_id,
                    grant.as_deref(),
                )
            },
        )
        .await
    }

    #[tool(
        description = "Move a file inside its member and rewrite every inbound reference to the new name, preserving each referrer's spelling, fragments, and ::repo qualifier, all as one mutation. A commit-pinned referrer stays untouched. Refuses a type-def file (use tb_rename_type), a name that already exists, and a cross-member target. Requires a prior tb_content read and its hash.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false
        ),
        output_schema = crate::schemas::mutation_schema(),
    )]
    async fn tb_rename(
        &self,
        Parameters(args): Parameters<RenameArguments>,
        _ctx: RequestContext<RoleServer>,
    ) -> CallToolResult {
        if args.to.is_empty() {
            return return_error("'to' must not be empty");
        }
        if let Err(error) = relative_path(&args.to) {
            return return_error(error);
        }
        let repo = args.repo;
        let to = args.to;
        let to_absolute = match absolute_member_path(self.transport.as_ref(), &repo, &to).await {
            Ok(path) => path,
            Err(error) => return return_error(error),
        };
        let grant = self.policy.member_allowlist().map(<[String]>::to_vec);
        self.guarded_mutation(
            "tb_rename",
            repo,
            args.path,
            args.expected_hash,
            move |target| crate::wire::rename_request(target, &to_absolute, grant.as_deref()),
        )
        .await
    }

    #[tool(
        description = "Preview a write, edit, or delete without changing anything. Returns the engine's would-be target, hash, diagnostics, and blast radius so a caller can inspect consequences before mutating. The member must be editable and the path relative to it. No expected_hash is required because nothing is written. This is a read: it is not an authorization decision.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true
        ),
        output_schema = crate::schemas::envelope_schema("preview_mutation"),
    )]
    async fn tb_preview_mutation(
        &self,
        Parameters(args): Parameters<PreviewArguments>,
        ctx: RequestContext<RoleServer>,
    ) -> CallToolResult {
        if args.repo.is_empty() {
            return CallToolResult::error(vec![ContentBlock::text("'repo' must not be empty")]);
        }
        if args.path.is_empty() {
            return CallToolResult::error(vec![ContentBlock::text("'path' must not be empty")]);
        }
        if let Err(error) = relative_path(&args.path) {
            return CallToolResult::error(vec![ContentBlock::text(error)]);
        }
        let (op, extra) = match args.op {
            MutationOp::Write => {
                if args.old_string.is_some() || args.new_string.is_some() {
                    return CallToolResult::error(vec![ContentBlock::text(
                        "'old_string' and 'new_string' are not valid for write_file",
                    )]);
                }
                let Some(content) = args.content else {
                    return CallToolResult::error(vec![ContentBlock::text(
                        "'content' is required for write_file",
                    )]);
                };
                ("write_file", json!({ "content": content }))
            }
            MutationOp::Edit => {
                if args.content.is_some() {
                    return CallToolResult::error(vec![ContentBlock::text(
                        "'content' is not valid for edit_file",
                    )]);
                }
                let (Some(old_string), Some(new_string)) = (args.old_string, args.new_string)
                else {
                    return CallToolResult::error(vec![ContentBlock::text(
                        "'old_string' and 'new_string' are required for edit_file",
                    )]);
                };
                (
                    "edit_file",
                    json!({
                        "old_string": old_string,
                        "new_string": new_string,
                        "replace_all": args.replace_all,
                    }),
                )
            }
            MutationOp::Delete => {
                if args.content.is_some() || args.old_string.is_some() || args.new_string.is_some()
                {
                    return CallToolResult::error(vec![ContentBlock::text(
                        "'content', 'old_string', and 'new_string' are not valid for delete_file",
                    )]);
                }
                ("delete_file", json!({}))
            }
        };
        if ctx.ct.is_cancelled() {
            return CallToolResult::error(vec![ContentBlock::text("engine read cancelled")]);
        }
        let Ok(_permit) = self.read_slots.try_acquire() else {
            return CallToolResult::error(vec![ContentBlock::text(
                "engine read capacity reached; retry later",
            )]);
        };
        let target = match resolve_editable(self.transport.as_ref(), &args.repo, &args.path).await {
            Ok(target) => target,
            Err(error) => return CallToolResult::error(vec![ContentBlock::text(error)]),
        };
        let mut request = json!({
            "id": 1,
            "op": op,
            "path": target,
        });
        if let (Some(request), Some(extra)) = (request.as_object_mut(), extra.as_object()) {
            request.extend(extra.clone());
        }
        tokio::select! {
            biased;
            _ = ctx.ct.cancelled() => CallToolResult::error(vec![ContentBlock::text("engine read cancelled")]),
            result = self.read_with_timeout(request, "tb_preview_mutation", ENGINE_READ_TIMEOUT) => result,
        }
    }

    #[tool(
        description = "Find tools by words from their name, description, or parameter names. Returns the best matches; detail selects names only, names with a one-line summary (default), or full definitions with schemas. Use it when you know the task but not the tool name.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true
        ),
        output_schema = crate::schemas::envelope_schema("tools"),
    )]
    async fn tb_search_tools(
        &self,
        Parameters(args): Parameters<SearchToolsArguments>,
        _ctx: RequestContext<RoleServer>,
    ) -> CallToolResult {
        let query = args.query.trim();
        if query.is_empty() {
            return CallToolResult::error(vec![ContentBlock::text("'query' must not be empty")]);
        }
        let matches = crate::describe::search_tools(
            &self.visible_tools(),
            query,
            args.limit,
            args.capability,
        );
        let tools: Vec<Value> = matches
            .into_iter()
            .map(|tool| match args.detail {
                SearchDetail::Name => json!({"name": tool.name}),
                SearchDetail::Summary => {
                    json!({"name": tool.name, "description": crate::describe::first_sentence(tool.description.as_deref())})
                }
                SearchDetail::Schema => serde_json::to_value(&tool).unwrap_or(Value::Null),
            })
            .collect();
        success_json(json!({"tools": tools}), self.payload)
    }

    #[tool(
        description = "Get one tool's full definition: description, input and output schemas, and annotations. Use it after tb_search_tools, or when a tool's one-line description is not enough.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true
        ),
        output_schema = crate::schemas::envelope_schema("tool"),
    )]
    async fn tb_describe_tool(
        &self,
        Parameters(args): Parameters<DescribeToolArguments>,
        _ctx: RequestContext<RoleServer>,
    ) -> CallToolResult {
        if args.name.is_empty() {
            return CallToolResult::error(vec![ContentBlock::text("'name' must not be empty")]);
        }
        let Some(tool) = self
            .visible_tools()
            .into_iter()
            .find(|tool| tool.name.as_ref() == args.name)
        else {
            return CallToolResult::error(vec![ContentBlock::text(format!(
                "unknown tool '{}'; call tb_search_tools or tools/list to see what this launch exposes",
                args.name
            ))]);
        };
        // The definition goes under `tool`, matching the envelope every other
        // answer uses and the one `envelope_schema("tool")` declares. Returning it
        // bare made a strict client reject the answer: the declared schema is
        // closed, and `name`, `description`, `inputSchema` and `annotations` are
        // not its properties.
        success_json(
            json!({"tool": serde_json::to_value(&tool).unwrap_or(Value::Null)}),
            self.payload,
        )
    }

    #[tool(
        description = "List every instance matching a type. A bare name matches each identity of that name; name::repo selects one. Optional origins restrict to file, nested, or meta sites; instance and body splice extra per-match facts. limit and offset page the records and add total, offset, and next_offset; without them the answer is the engine's. Paths and workspace content may be exposed; this is not an authorization boundary.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true
        ),
        output_schema = crate::schemas::envelope_schema("instances_of"),
    )]
    async fn tb_instances_of(
        &self,
        Parameters(args): Parameters<InstancesOfArguments>,
        ctx: RequestContext<RoleServer>,
    ) -> CallToolResult {
        if args.type_name.is_empty() {
            return CallToolResult::error(vec![ContentBlock::text(
                "'type_name' must not be empty",
            )]);
        }
        let mut request = json!({
            "id": 1,
            "type": args.type_name,
            "instance": args.instance,
            "body": args.body,
        });
        if let Some(origins) = args.origins {
            let origins: Vec<&str> = origins
                .iter()
                .map(|origin| match origin {
                    InstanceOrigin::File => "file",
                    InstanceOrigin::Nested => "nested",
                    InstanceOrigin::Meta => "meta",
                })
                .collect();
            request["origins"] = json!(origins);
        }
        let result = self.read(request, "tb_instances_of", ctx).await;
        match (args.limit, args.offset) {
            (None, None) => result,
            (limit, offset) => page_instances(result, limit, offset.unwrap_or(0), self.payload),
        }
    }

    #[tool(
        description = "Resolve one wikilink string, including one you are about to write: does this address resolve, and what is there? target carries the link (`note`, `note::repo`, `note#Heading`, `note^blk`, `note^^blk`); origin_path is the file it appears in, which scopes a bare target to its repo. With peek (default true) the addressed line comes back as a snippet.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true
        ),
        output_schema = crate::schemas::follow_schema(),
    )]
    async fn tb_follow(
        &self,
        Parameters(args): Parameters<FollowArguments>,
        ctx: RequestContext<RoleServer>,
    ) -> CallToolResult {
        if args.repo.is_empty() {
            return return_error("'repo' must not be empty");
        }
        if args.origin_path.is_empty() {
            return return_error("'origin_path' must not be empty");
        }
        if args.target.trim().is_empty() {
            return return_error("'target' must not be empty");
        }
        if let Err(error) = relative_path(&args.origin_path) {
            return return_error(error);
        }
        if ctx.ct.is_cancelled() {
            return return_error("engine read cancelled");
        }
        let Ok(_permit) = self.read_slots.try_acquire() else {
            return return_error("engine read capacity reached; retry later");
        };
        let peek = args.peek;
        let outcome = async {
            let origin =
                absolute_member_path(self.transport.as_ref(), &args.repo, &args.origin_path)
                    .await?;
            let parts = split_follow(args.target.trim());
            let (verb, key) = match (&parts.block, &parts.anchor) {
                (Some(_), _) => ("resolve_block_id", "resolve_block_id"),
                (None, Some(_)) => ("resolve_anchor", "resolve_anchor"),
                (None, None) => ("resolve_target", "resolve_target"),
            };
            let mut request =
                json!({"read": verb, "id": 1, "target": parts.base, "origin": origin});
            if let Some(block) = &parts.block {
                request["block_id"] = json!(block);
            }
            if let Some(anchor) = &parts.anchor {
                request["anchor"] = json!(anchor);
            }
            let resolved = read_engine(self.transport.as_ref(), request).await?;
            let answer = &resolved.payload[key];
            let mut out = json!({
                "target": args.target.trim(),
                "origin": origin,
                "resolved": !answer.is_null(),
                "referent": parts.referent,
            });
            if let Some(object) = answer.as_object() {
                for (field, value) in object {
                    out[field.clone()] = value.clone();
                }
            }
            // Peek the addressed line, best effort: a snippet is a convenience,
            // never the verdict.
            let addressed = answer
                .get("file_path")
                .or_else(|| answer.get("path"))
                .cloned()
                .unwrap_or(Value::Null);
            if peek
                && let Some(line) = answer
                    .get("span")
                    .and_then(|span| span.get("line_col"))
                    .and_then(|line_col| line_col.get("start"))
                    .and_then(|start| start.get("line"))
                    .and_then(Value::as_u64)
                && !addressed.is_null()
                && let Ok(content) = read_engine(
                    self.transport.as_ref(),
                    json!({"read": "content", "id": 1, "path": addressed}),
                )
                .await
                && let Some(text) = content.payload["content"]["text"].as_str()
            {
                let snippet: Vec<&str> = text
                    .lines()
                    .skip(line.saturating_sub(1) as usize)
                    .take(3)
                    .collect();
                if !snippet.is_empty() {
                    out["snippet"] = json!(snippet.join("\\n"));
                }
            }
            Ok::<Value, String>(json!({"follow": out}))
        };
        match tokio::select! {
            biased;
            _ = ctx.ct.cancelled() => Err("engine read cancelled".to_owned()),
            result = outcome => result,
        } {
            Ok(payload) => success_payload(payload, self.payload),
            Err(error) => return_error(error),
        }
    }

    #[tool(
        description = "Classify a path: a typed instance (with its claim, whether it resolves, and its closure), a type-def, or a plain note. Use it before tb_resolved or tb_references, which return null or [] on a non-instance.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true
        ),
        output_schema = crate::schemas::typed_schema(),
    )]
    async fn tb_typed(
        &self,
        Parameters(args): Parameters<FileArguments>,
        ctx: RequestContext<RoleServer>,
    ) -> CallToolResult {
        if args.repo.is_empty() {
            return return_error("'repo' must not be empty");
        }
        if args.path.is_empty() {
            return return_error("'path' must not be empty");
        }
        if let Err(error) = relative_path(&args.path) {
            return return_error(error);
        }
        if ctx.ct.is_cancelled() {
            return return_error("engine read cancelled");
        }
        let Ok(_permit) = self.read_slots.try_acquire() else {
            return return_error("engine read capacity reached; retry later");
        };
        let outcome = async {
            let absolute =
                absolute_member_path(self.transport.as_ref(), &args.repo, &args.path).await?;
            let instance = read_engine(
                self.transport.as_ref(),
                json!({"read": "instance", "id": 1, "path": absolute}),
            )
            .await?;
            let view = &instance.payload["instance"];
            if !view.is_null() {
                return Ok::<Value, String>(json!({
                    "typed": true,
                    "kind": "instance",
                    "path": absolute,
                    "claim": view.get("claim").cloned().unwrap_or(Value::Array(Vec::new())),
                    "resolves": view.get("resolved").cloned().unwrap_or(Value::Bool(false)),
                    "closure": view.get("closure").cloned().unwrap_or(Value::Array(Vec::new())),
                }));
            }
            // Not an instance: the local resolve form tells a type-def from a plain note.
            let kind = read_engine(
                self.transport.as_ref(),
                json!({"read": "resolve_target", "id": 1, "target": "", "origin": absolute}),
            )
            .await?;
            let kind = kind.payload["resolve_target"]
                .get("kind")
                .cloned()
                .unwrap_or(Value::Null);
            Ok::<Value, String>(json!({
                "typed": false,
                "kind": kind,
                "path": absolute,
            }))
        };
        match tokio::select! {
            biased;
            _ = ctx.ct.cancelled() => Err("engine read cancelled".to_owned()),
            result = outcome => result,
        } {
            Ok(payload) => success_payload(payload, self.payload),
            Err(error) => return_error(error),
        }
    }

    #[tool(
        description = "Resolve one instance: field values with their contribution provenance, the type claim and closure, and the candidates it could have claimed. Null when the path is not a parsed instance.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true
        ),
        output_schema = crate::schemas::envelope_schema("instance"),
    )]
    async fn tb_resolved(
        &self,
        Parameters(args): Parameters<FileArguments>,
        ctx: RequestContext<RoleServer>,
    ) -> CallToolResult {
        self.file_read(args, "tb_resolved", ctx).await
    }

    #[tool(
        description = "List a directory's direct entries: files and subdirectories, hidden excluded, catalog-derived so only directories holding a catalogued file appear. The path is member-relative and names the directory.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true
        ),
        output_schema = crate::schemas::envelope_schema("dir_entries"),
    )]
    async fn tb_children(
        &self,
        Parameters(args): Parameters<FileArguments>,
        ctx: RequestContext<RoleServer>,
    ) -> CallToolResult {
        if args.repo.is_empty() {
            return return_error("'repo' must not be empty");
        }
        if args.path.is_empty() {
            return return_error("'path' must not be empty");
        }
        if let Err(error) = relative_path(&args.path) {
            return return_error(error);
        }
        if ctx.ct.is_cancelled() {
            return return_error("engine read cancelled");
        }
        let Ok(_permit) = self.read_slots.try_acquire() else {
            return return_error("engine read capacity reached; retry later");
        };
        let outcome = async {
            let dir = absolute_member_path(self.transport.as_ref(), &args.repo, &args.path).await?;
            read_for(
                self.transport.as_ref(),
                "tb_children",
                json!({"id": 1, "dir": dir}),
            )
            .await
        };
        let result = tokio::select! {
            biased;
            _ = ctx.ct.cancelled() => return return_error("engine read cancelled"),
            result = outcome => result,
        };
        match result {
            Ok(result) => success_result(result, self.payload),
            Err(error) => return_error(error),
        }
    }

    #[tool(
        description = "List every resolved instance in the workspace, with its file, claim, closure, effective shape and values, and addressable inline records. The engine sends the whole set (942 on the live vault), so limit and offset page the answer and add total, offset, and next_offset.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true
        ),
        output_schema = crate::schemas::instances_schema(),
    )]
    async fn tb_instances(
        &self,
        Parameters(args): Parameters<PageArguments>,
        ctx: RequestContext<RoleServer>,
    ) -> CallToolResult {
        let result = self.read(json!({"id": 1}), "tb_instances", ctx).await;
        match (args.limit, args.offset) {
            (None, None) => result,
            (limit, offset) => page_array(
                result,
                "instances",
                limit,
                offset.unwrap_or(0),
                self.payload,
            ),
        }
    }

    #[tool(
        description = "List the files that satisfy a type's required fields without claiming it, grouped by file, with the candidate type names and the fields that satisfied them. summary projects each file to its type names; limit and offset page the scanned files.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true
        ),
        output_schema = crate::schemas::candidates_schema(),
    )]
    async fn tb_candidates(
        &self,
        Parameters(args): Parameters<CandidatesArguments>,
        ctx: RequestContext<RoleServer>,
    ) -> CallToolResult {
        let mut request = json!({"id": 1});
        if args.summary {
            request["summary"] = json!(true);
        }
        if let Some(limit) = args.limit {
            request["limit"] = json!(limit);
        }
        if let Some(offset) = args.offset {
            request["offset"] = json!(offset);
        }
        self.read(request, "tb_candidates", ctx).await
    }

    #[tool(
        description = "Read one file's semantic token stream: resolved and broken wikilinks, pinned commits, typed field values, type claims with the identity they resolve to, typed blocks, block ids, and headings. Null when the path is not a held file. Spans are byte ranges with line and column.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true
        ),
        output_schema = crate::schemas::envelope_schema("semantic_tokens"),
    )]
    async fn tb_semantic_tokens(
        &self,
        Parameters(args): Parameters<FileArguments>,
        ctx: RequestContext<RoleServer>,
    ) -> CallToolResult {
        self.file_read(args, "tb_semantic_tokens", ctx).await
    }

    #[tool(
        description = "Resolve which declared member owns a member-relative path, with its root, role, and whether it is editable and local. Null when the path lies under no declared member. Use it to route a path to a member without re-deriving roots.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true
        ),
        output_schema = crate::schemas::envelope_schema("resolve_member"),
    )]
    async fn tb_resolve_member(
        &self,
        Parameters(args): Parameters<FileArguments>,
        ctx: RequestContext<RoleServer>,
    ) -> CallToolResult {
        self.file_read(args, "tb_resolve_member", ctx).await
    }

    #[tool(
        description = "Read one file's parsed frontmatter as JSON. Null when the file is neither a typed instance nor a note; a typed instance carries its claim under `type`, a note carries no `type` key. Paths are member-relative.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true
        ),
        output_schema = crate::schemas::envelope_schema("frontmatter"),
    )]
    async fn tb_frontmatter(
        &self,
        Parameters(args): Parameters<FileArguments>,
        ctx: RequestContext<RoleServer>,
    ) -> CallToolResult {
        self.file_read(args, "tb_frontmatter", ctx).await
    }

    #[tool(
        description = "List every heading in one file, in document order, as the anchors a `#fragment` can address: text, level, and span. Null when the target does not resolve; an empty array when the file exists but has no headings.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true
        ),
        output_schema = crate::schemas::envelope_schema("anchors"),
    )]
    async fn tb_anchors(
        &self,
        Parameters(args): Parameters<FileArguments>,
        ctx: RequestContext<RoleServer>,
    ) -> CallToolResult {
        self.target_read(args, "tb_anchors", ctx).await
    }

    #[tool(
        description = "List every addressable block id in one file, in document order, with kind (record, typed_block, marker), type claim, and span. Every occurrence is listed, duplicates included. Null when the target does not resolve; an empty array when the file has no ids.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true
        ),
        output_schema = crate::schemas::envelope_schema("block_ids"),
    )]
    async fn tb_block_ids(
        &self,
        Parameters(args): Parameters<FileArguments>,
        ctx: RequestContext<RoleServer>,
    ) -> CallToolResult {
        self.target_read(args, "tb_block_ids", ctx).await
    }

    #[tool(
        description = "List one file's wikilink edges, classified by kind and surface: outgoing (what it links to) and incoming (what links to it), each with span and resolution. Direction defaults to both.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true
        ),
        output_schema = crate::schemas::references_schema(),
    )]
    async fn tb_references(
        &self,
        Parameters(args): Parameters<ReferencesArguments>,
        ctx: RequestContext<RoleServer>,
    ) -> CallToolResult {
        if let Err(refusal) = file_read_guard(&args.repo, &args.path, ctx.ct.is_cancelled()) {
            return refusal;
        }
        let Ok(_permit) = self.read_slots.try_acquire() else {
            return CallToolResult::error(vec![ContentBlock::text(
                "engine read capacity reached; retry later",
            )]);
        };
        let direction = args.direction;
        let outcome = async {
            let path =
                absolute_member_path(self.transport.as_ref(), &args.repo, &args.path).await?;
            let mut payload = serde_json::Map::new();
            if matches!(
                direction,
                ReferenceDirection::Out | ReferenceDirection::Both
            ) {
                let out = read_engine(
                    self.transport.as_ref(),
                    json!({"read": "references_out", "id": 1, "path": path}),
                )
                .await?;
                payload.insert(
                    "references_out".to_owned(),
                    out.payload["references_out"].clone(),
                );
            }
            if matches!(direction, ReferenceDirection::In | ReferenceDirection::Both) {
                let inb = read_engine(
                    self.transport.as_ref(),
                    json!({"read": "references_in", "id": 1, "path": path}),
                )
                .await?;
                payload.insert(
                    "references_in".to_owned(),
                    inb.payload["references_in"].clone(),
                );
            }
            Ok::<Value, String>(Value::Object(payload))
        };
        match tokio::select! {
            biased;
            _ = ctx.ct.cancelled() => Err("engine read cancelled".to_owned()),
            result = outcome => result,
        } {
            Ok(payload) => success_payload(payload, self.payload),
            Err(error) => CallToolResult::error(vec![ContentBlock::text(error)]),
        }
    }

    #[tool(
        description = "Walk the reference graph from one seed file and return the reachable subgraph (nodes plus edges). Direction defaults to out, depth to 1; past depth 1 the engine requires an edge-kind filter, because navigational fan-out explodes. Scope `own` prunes at the repo boundary. `truncated` says a bound was hit.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true
        ),
        output_schema = crate::schemas::envelope_schema("neighborhood"),
    )]
    async fn tb_neighborhood(
        &self,
        Parameters(args): Parameters<NeighborhoodArguments>,
        ctx: RequestContext<RoleServer>,
    ) -> CallToolResult {
        if let Err(refusal) = file_read_guard(&args.repo, &args.path, ctx.ct.is_cancelled()) {
            return refusal;
        }
        let Ok(_permit) = self.read_slots.try_acquire() else {
            return CallToolResult::error(vec![ContentBlock::text(
                "engine read capacity reached; retry later",
            )]);
        };
        let path = match absolute_member_path(self.transport.as_ref(), &args.repo, &args.path).await
        {
            Ok(path) => path,
            Err(error) => return CallToolResult::error(vec![ContentBlock::text(error)]),
        };
        let mut request = json!({"id": 1, "path": path});
        if let Some(direction) = args.direction {
            request["direction"] = json!(match direction {
                WalkDirection::Out => "out",
                WalkDirection::In => "in",
                WalkDirection::Both => "both",
            });
        }
        if let Some(depth) = args.depth {
            request["depth"] = json!(depth);
        }
        if let Some(kinds) = args.kinds {
            let kinds: Vec<&str> = kinds
                .iter()
                .map(|kind| match kind {
                    WalkKind::Navigational => "navigational",
                    WalkKind::Contributing => "contributing",
                    WalkKind::FieldReference => "field",
                    WalkKind::CommitReferent => "commit-referent",
                })
                .collect();
            request["kinds"] = json!(kinds);
        }
        if let Some(scope) = args.scope {
            request["scope"] = json!(match scope {
                WalkScope::All => "all",
                WalkScope::Own => "own",
            });
        }
        if let Some(max_nodes) = args.max_nodes {
            request["max_nodes"] = json!(max_nodes);
        }
        // The permit above is this read's; taking another would charge one call twice.
        self.read_under_permit(request, "tb_neighborhood", ctx.ct.clone())
            .await
    }

    #[tool(
        description = "Validate a value against a type without touching disk. Returns one verdict per identity the name denotes, each with diagnostics and advisory undeclared fields; an unknown type yields a null identity carrying unknown-type-claim rather than an empty result. Values and diagnostics may reveal workspace vocabulary.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true
        ),
        output_schema = crate::schemas::envelope_schema("validate_value"),
    )]
    async fn tb_validate_value(
        &self,
        Parameters(args): Parameters<ValidateValueArguments>,
        ctx: RequestContext<RoleServer>,
    ) -> CallToolResult {
        if args.type_name.is_empty() {
            return CallToolResult::error(vec![ContentBlock::text(
                "'type_name' must not be empty",
            )]);
        }
        if args.repo.as_deref() == Some("") {
            return CallToolResult::error(vec![ContentBlock::text("'repo' must not be empty")]);
        }
        let mut request = json!({
            "id": 1,
            "type_name": args.type_name,
            "value": args.value,
        });
        if let Some(repo) = args.repo {
            request["repo"] = json!(repo);
        }
        self.read(request, "tb_validate_value", ctx).await
    }

    #[tool(
        description = "Check what actually happened after an outcome_unknown mutation. Give the 16-hex-character hash the interrupted write should have produced, or expect_absent for a delete; a value that is not a digest is refused rather than compared. Returns outcome applied or not_applied with the file's current hash and commit; a not_applied answer re-arms the read-before-write guard with the current hash, so the caller can decide whether to retry. If you do not know the expected digest, read the file with tb_content and compare the text instead. This is a read: it never writes.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true
        ),
        output_schema = crate::schemas::reconcile_schema(),
    )]
    async fn tb_reconcile(
        &self,
        Parameters(args): Parameters<ReconcileArguments>,
        ctx: RequestContext<RoleServer>,
    ) -> CallToolResult {
        if args.repo.is_empty() {
            return CallToolResult::error(vec![ContentBlock::text("'repo' must not be empty")]);
        }
        if args.path.is_empty() {
            return CallToolResult::error(vec![ContentBlock::text("'path' must not be empty")]);
        }
        if let Err(error) = relative_path(&args.path) {
            return CallToolResult::error(vec![ContentBlock::text(error)]);
        }
        let expected = match (args.expected_hash.as_deref(), args.expect_absent) {
            (Some(hash), false) if is_engine_hash(hash) => Some(hash.to_owned()),
            // A value that is not a digest cannot describe the interrupted mutation,
            // and answering `not_applied` to it would be a confident lie: a model that
            // sends the string "null" would be told the write did not land when it did.
            (Some(hash), false) => {
                return CallToolResult::error(vec![ContentBlock::text(format!(
                    "'expected_hash' must be the 16-hex-character digest tb_content returns, got {hash:?};                      for a delete pass 'expect_absent': true, and if the digest is unknown read the                      file with tb_content and compare the text instead"
                ))]);
            }
            (None, true) => None,
            _ => {
                return CallToolResult::error(vec![ContentBlock::text(
                    "give exactly one of a nonempty 'expected_hash' or 'expect_absent': true; \
                     if you do not know the digest, read the file with tb_content and compare \
                     the text instead, because reconcile can only compare against an expectation",
                )]);
            }
        };
        if ctx.ct.is_cancelled() {
            return CallToolResult::error(vec![ContentBlock::text("engine read cancelled")]);
        }
        let Ok(_permit) = self.read_slots.try_acquire() else {
            return CallToolResult::error(vec![ContentBlock::text(
                "engine read capacity reached; retry later",
            )]);
        };
        let key = (args.repo.clone(), args.path.clone());
        let result = tokio::select! {
            biased;
            _ = ctx.ct.cancelled() => {
                return CallToolResult::error(vec![ContentBlock::text("engine read cancelled")]);
            }
            result = tokio::time::timeout(
                ENGINE_READ_TIMEOUT,
                read_content(self.transport.as_ref(), &args.repo, &args.path),
            ) => result,
        };
        let result = match result {
            Err(_) => {
                return CallToolResult::error(vec![ContentBlock::text(format!(
                    "engine read 'content' timed out after {ENGINE_READ_TIMEOUT:?}; the outcome is still unknown"
                ))]);
            }
            Ok(Err(error)) => return CallToolResult::error(vec![ContentBlock::text(error)]),
            Ok(Ok(result)) => result,
        };
        let current = result
            .payload
            .get("content")
            .cloned()
            .unwrap_or(Value::Null);
        let current_hash = current
            .get("hash")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let current_commit = current.get("commit").cloned().unwrap_or(Value::Null);
        let applied = match &expected {
            None => current.is_null(),
            Some(expected) => current_hash.as_deref() == Some(expected.as_str()),
        };
        if applied {
            self.ledger.forget(&key);
        } else if let Some(hash) = current_hash.clone() {
            // A differing file is a fresh, coherent read: arm the guard so a
            // deliberate retry does not need a second call.
            self.ledger.record(key, hash);
        } else {
            self.ledger.forget(&key);
        }
        let payload = json!({
            "outcome": if applied { "applied" } else { "not_applied" },
            "current_hash": current_hash,
            "current_commit": current_commit,
        });
        let mut response = success_payload(payload, self.payload);
        response.meta = Some(MetaObject(serde_json::Map::from_iter([(
            "io.2brain/engineVersion".to_owned(),
            Value::String(result.version.to_string()),
        )])));
        response
    }

    /// Shared mutation policy: a recorded read of this exact path must exist,
    /// the hash must match, the member must be editable, one mutation runs at a
    /// time, and a lost answer is `outcome_unknown` rather than a retry.
    async fn guarded_mutation(
        &self,
        tool: &str,
        repo: String,
        path: String,
        expected_hash: String,
        build: impl FnOnce(&str) -> Value,
    ) -> CallToolResult {
        let prepared = match self.prepare_mutation(&repo, &path, &expected_hash).await {
            Ok(prepared) => prepared,
            Err(denial) => {
                self.audit.record(
                    &self.principal,
                    tool,
                    &repo,
                    &format!("denied:{}", denial.stage.name()),
                );
                return CallToolResult::error(vec![ContentBlock::text(denial.text())]);
            }
        };
        let request = build(&prepared.target);
        match read_mutation_with_timeout(self.transport.as_ref(), request, ENGINE_READ_TIMEOUT)
            .await
        {
            Ok(result) => {
                self.audit.record(&self.principal, tool, &repo, "applied");
                self.ledger.forget(&prepared.key);
                success_result(result, self.payload)
            }
            Err(MutationFailure::Rejected { message, .. }) => {
                self.audit.record(&self.principal, tool, &repo, "rejected");
                CallToolResult::error(vec![ContentBlock::text(message)])
            }
            Err(MutationFailure::Unknown(error)) => {
                self.audit
                    .record(&self.principal, tool, &repo, "outcome_unknown");
                self.ledger.forget(&prepared.key);
                CallToolResult::error(vec![ContentBlock::text(format!(
                    "outcome_unknown: {error}. The mutation may have landed; read the file again before retrying."
                ))])
            }
        }
    }

    /// Run the mutation gate in `policy::MUTATION_STAGES` order, stopping at the
    /// first denial. The returned value holds the slot permit and the resolved
    /// target, so a caller that gets one may proceed to the engine.
    /// Armed state for one file: every gate except the mutation slot.
    async fn arm_file(
        &self,
        repo: &str,
        path: &str,
        expected_hash: &str,
    ) -> Result<ArmedFile, Denial> {
        let key = (repo.to_owned(), path.to_owned());
        // The pure half of the gate: order and answers, no IO. See `gate::reduce`.
        let input = gate::Input {
            repo,
            path,
            expected_hash,
            recorded: self.ledger.recorded(&key),
            permits_member: self.policy.permits_member(repo),
        };
        gate::reduce(&policy::MUTATION_STAGES, &input)?;
        // The engine answers whether it will author this member; capacity is a resource,
        // not a judgement.
        let target = resolve_editable(self.transport.as_ref(), repo, path)
            .await
            .map_err(|error| Denial::new(policy::Stage::Editability, error))?;
        // The member named is not always the member the engine writes; the check lives in the
        // wire layer with the exchange it needs, and runs only when a policy lists members.
        if let Some(allowed) = self.policy.member_allowlist() {
            crate::wire::enforce_owner_grant(self.transport.as_ref(), &target, repo, allowed)
                .await
                .map_err(|message| Denial::new(policy::Stage::MemberGrant, message))?;
        }
        Ok(ArmedFile { key, target })
    }

    /// Send an armed mutation and settle its outcome, consuming every recorded
    /// read it invalidates.
    async fn finish_mutation(
        &self,
        tool: &'static str,
        repo: &str,
        request: Value,
        keys: impl IntoIterator<Item = (String, String)>,
    ) -> CallToolResult {
        let keys: Vec<(String, String)> = keys.into_iter().collect();
        match read_mutation_with_timeout(self.transport.as_ref(), request, ENGINE_READ_TIMEOUT)
            .await
        {
            Ok(result) => {
                self.audit.record(&self.principal, tool, repo, "applied");
                for key in keys {
                    self.ledger.forget(&key);
                }
                success_result(result, self.payload)
            }
            // The engine refused because the path was taken, which is the ABSENCE gate
            // answering a few milliseconds after this process's own pre-check looked: a
            // competing client created the file in between. Same gate, same words.
            Err(MutationFailure::Rejected { detail, .. }) if wire::absence_conflict(&detail) => {
                let absence = policy::Stage::Absence;
                self.audit.record(
                    &self.principal,
                    tool,
                    repo,
                    &format!("denied:{}", absence.name()),
                );
                return_error(Denial::new(absence, policy::CREATE_ALREADY_EXISTS).text())
            }
            Err(MutationFailure::Rejected { message, .. }) => {
                self.audit.record(&self.principal, tool, repo, "rejected");
                return_error(message)
            }
            Err(MutationFailure::Unknown(error)) => {
                self.audit
                    .record(&self.principal, tool, repo, "outcome_unknown");
                for key in keys {
                    self.ledger.forget(&key);
                }
                return_error(format!(
                    "outcome_unknown: {error}. The mutation may have landed; read the file again before retrying."
                ))
            }
        }
    }

    /// Arm a create: the same gates as a mutation, with absence in place of the
    /// read-then-hash pair.
    async fn prepare_create(
        &self,
        repo: &str,
        path: &str,
    ) -> Result<(ArmedFile, tokio::sync::OwnedSemaphorePermit), Denial> {
        let key = (repo.to_owned(), path.to_owned());
        let input = gate::Input {
            repo,
            path,
            expected_hash: "",
            recorded: None,
            permits_member: self.policy.permits_member(repo),
        };
        gate::reduce(&policy::CREATE_STAGES, &input)?;
        // Absence is the engine's answer, not a filesystem check. This read is the cheap
        // refusal (one round trip, no write attempt); it is NOT the guarantee — a competing
        // create can land between it and the engine's write, which is why the mutation is
        // `create_file` and refuses there too, in the same words.
        let read = read_content(self.transport.as_ref(), repo, path)
            .await
            .map_err(|error| Denial::new(policy::Stage::Absence, error))?;
        if read.payload.get("content").is_some_and(|c| !c.is_null()) {
            return Err(Denial::new(
                policy::Stage::Absence,
                policy::CREATE_ALREADY_EXISTS,
            ));
        }
        let target = resolve_editable(self.transport.as_ref(), repo, path)
            .await
            .map_err(|error| Denial::new(policy::Stage::Editability, error))?;
        let permit = self.take_slot()?;
        Ok((ArmedFile { key, target }, permit))
    }

    /// Take the single mutation slot.
    fn take_slot(&self) -> Result<tokio::sync::OwnedSemaphorePermit, Denial> {
        self.mutation_slot.clone().try_acquire_owned().map_err(|_| {
            Denial::new(
                policy::Stage::Capacity,
                "another mutation is in progress; retry later",
            )
        })
    }

    async fn prepare_mutation(
        &self,
        repo: &str,
        path: &str,
        expected_hash: &str,
    ) -> Result<PreparedMutation, Denial> {
        let armed = self.arm_file(repo, path, expected_hash).await?;
        let permit = self.take_slot()?;
        Ok(PreparedMutation {
            key: armed.key,
            target: armed.target,
            _permit: permit,
        })
    }

    /// Two armed files for one saga (`inline`, `promote`): both need their own
    /// prior read and hash, and the slot is taken once for the pair.
    async fn prepare_pair(
        &self,
        first: (&str, &str, &str),
        second: (&str, &str, &str),
    ) -> Result<(ArmedFile, ArmedFile, tokio::sync::OwnedSemaphorePermit), Denial> {
        let first = self.arm_file(first.0, first.1, first.2).await?;
        let second = self.arm_file(second.0, second.1, second.2).await?;
        let permit = self.take_slot()?;
        Ok((first, second, permit))
    }

    /// A deserialization failure names the missing field but not the tool or the
    /// accepted shape, so restate both and point at the full definition.
    fn explain_argument_error(&self, name: &str, response: CallToolResponse) -> CallToolResponse {
        let CallToolResponse::Complete(mut result) = response else {
            return response;
        };
        if result.is_error != Some(true) {
            return CallToolResponse::Complete(result);
        }
        let Some(text) = result
            .content
            .first()
            .and_then(|block| block.as_text())
            .map(|block| block.text.clone())
        else {
            return CallToolResponse::Complete(result);
        };
        if !text.starts_with("failed to deserialize parameters:") {
            return CallToolResponse::Complete(result);
        }
        let required = self
            .tool_router
            .get(name)
            .and_then(|tool| tool.input_schema.get("required"))
            .and_then(Value::as_array)
            .map(|fields| {
                fields
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();
        let hint = match required.is_empty() {
            true => format!(" Tool '{name}' takes no required arguments."),
            false => format!(" Tool '{name}' requires: {required}."),
        };
        result.content = vec![ContentBlock::text(format!(
            "{text}.{hint} Call tb_describe_tool {{\"name\":\"{name}\"}} for the full schema."
        ))];
        CallToolResponse::Complete(result)
    }

    /// A workspace-shape mutation: it authors a workspace's `.2brain/
    /// workspace.yaml`, so it needs the third grant on top of mutations.
    async fn workspace_mutation(
        &self,
        tool: &'static str,
        repo: String,
        mut request: Value,
    ) -> CallToolResult {
        if repo.is_empty() {
            return return_error("'repo' must not be empty");
        }
        if !self.policy.allow_workspace_shape {
            self.audit
                .record(&self.principal, tool, &repo, "denied:workspace_shape");
            return return_error(
                "policy/workspace_shape: this launch does not allow changing which members the workspace serves",
            );
        }
        if !self.policy.permits_member(&repo) {
            self.audit
                .record(&self.principal, tool, &repo, "denied:member_grant");
            return return_error(format!(
                "policy/member_grant: mutations are not allowed in workspace member {repo}"
            ));
        }
        let root = match member_root(self.transport.as_ref(), &repo).await {
            Ok(root) => root,
            Err(error) => return return_error(error),
        };
        let Ok(_permit) = self.mutation_slot.clone().try_acquire_owned() else {
            return return_error("another mutation is in progress; retry later");
        };
        request["root"] = json!(root);
        match read_mutation_with_timeout(self.transport.as_ref(), request, ENGINE_READ_TIMEOUT)
            .await
        {
            Ok(result) => {
                self.audit.record(&self.principal, tool, &repo, "applied");
                success_result(result, self.payload)
            }
            Err(MutationFailure::Rejected { message, .. }) => {
                self.audit.record(&self.principal, tool, &repo, "rejected");
                return_error(message)
            }
            Err(MutationFailure::Unknown(error)) => {
                self.audit
                    .record(&self.principal, tool, &repo, "outcome_unknown");
                return_error(format!(
                    "outcome_unknown: {error}. The workspace change may have landed; read the workspace.yaml again before retrying."
                ))
            }
        }
    }

    /// A composition mutation: it authors a member's `.2brain/repo.yaml`, so
    /// it needs the composition grant on top of the mutation grant.
    async fn composition_mutation(
        &self,
        tool: &'static str,
        repo: String,
        mut request: Value,
    ) -> CallToolResult {
        if repo.is_empty() {
            return return_error("'repo' must not be empty");
        }
        if !self.policy.allow_composition {
            self.audit
                .record(&self.principal, tool, &repo, "denied:composition");
            return return_error(
                "policy/composition: this launch does not allow changing the workspace composition",
            );
        }
        if !self.policy.permits_member(&repo) {
            self.audit
                .record(&self.principal, tool, &repo, "denied:member_grant");
            return return_error(format!(
                "policy/member_grant: mutations are not allowed in workspace member {repo}"
            ));
        }
        let root = match member_root(self.transport.as_ref(), &repo).await {
            Ok(root) => root,
            Err(error) => return return_error(error),
        };
        let Ok(_permit) = self.mutation_slot.clone().try_acquire_owned() else {
            return return_error("another mutation is in progress; retry later");
        };
        request["root"] = json!(root);
        match read_mutation_with_timeout(self.transport.as_ref(), request, ENGINE_READ_TIMEOUT)
            .await
        {
            Ok(result) => {
                self.audit.record(&self.principal, tool, &repo, "applied");
                success_result(result, self.payload)
            }
            Err(MutationFailure::Rejected { message, .. }) => {
                self.audit.record(&self.principal, tool, &repo, "rejected");
                return_error(message)
            }
            Err(MutationFailure::Unknown(error)) => {
                self.audit
                    .record(&self.principal, tool, &repo, "outcome_unknown");
                return_error(format!(
                    "outcome_unknown: {error}. The composition change may have landed; read the member's repo.yaml again before retrying."
                ))
            }
        }
    }

    /// One file-addressed read: validate, resolve the member root, read.
    async fn file_read(
        &self,
        args: FileArguments,
        tool: &str,
        ctx: RequestContext<RoleServer>,
    ) -> CallToolResult {
        if let Err(refusal) = file_read_guard(&args.repo, &args.path, ctx.ct.is_cancelled()) {
            return refusal;
        }
        let Ok(_permit) = self.read_slots.try_acquire() else {
            return CallToolResult::error(vec![ContentBlock::text(
                "engine read capacity reached; retry later",
            )]);
        };
        let request = async {
            let path =
                absolute_member_path(self.transport.as_ref(), &args.repo, &args.path).await?;
            Ok::<Value, String>(json!({"id": 1, "path": path}))
        };
        let request = match request.await {
            Ok(request) => request,
            Err(error) => return CallToolResult::error(vec![ContentBlock::text(error)]),
        };
        self.read_under_permit(request, tool, ctx.ct.clone()).await
    }

    /// One target-addressed read: `anchors` and `block_ids` take a resolvable
    /// target rather than a path, and a member-relative path resolves as one.
    async fn target_read(
        &self,
        args: FileArguments,
        tool: &str,
        ctx: RequestContext<RoleServer>,
    ) -> CallToolResult {
        if let Err(refusal) = file_read_guard(&args.repo, &args.path, ctx.ct.is_cancelled()) {
            return refusal;
        }
        let Ok(_permit) = self.read_slots.try_acquire() else {
            return CallToolResult::error(vec![ContentBlock::text(
                "engine read capacity reached; retry later",
            )]);
        };
        let request = async {
            // `target` carries the wikilink grammar, not a path. The empty
            // target with an `origin` is the engine's local form: it resolves to
            // the origin file itself, which is the file the caller named.
            let origin =
                absolute_member_path(self.transport.as_ref(), &args.repo, &args.path).await?;
            Ok::<Value, String>(json!({"id": 1, "target": "", "origin": origin}))
        };
        let request = match request.await {
            Ok(request) => request,
            Err(error) => return CallToolResult::error(vec![ContentBlock::text(error)]),
        };
        self.read_under_permit(request, tool, ctx.ct.clone()).await
    }

    /// The tools this launch may advertise, before any profile compacting.
    fn visible_tools(&self) -> Vec<rmcp::model::Tool> {
        self.tool_router
            .list_all()
            .into_iter()
            .filter(|tool| self.may_advertise(tool.name.as_ref()))
            .collect()
    }

    /// Whether this launch may advertise a tool at all. Each grant is independent:
    /// mutations, then composition, then workspace shape.
    fn may_advertise(&self, name: &str) -> bool {
        let mutation = !policy::MUTATION_TOOLS.contains(&name) || self.policy.allow_mutations;
        let composition =
            !policy::COMPOSITION_TOOLS.contains(&name) || self.policy.allow_composition;
        let workspace =
            !policy::WORKSPACE_TOOLS.contains(&name) || self.policy.allow_workspace_shape;
        mutation && composition && workspace
    }

    async fn read(
        &self,
        request: Value,
        tool: &str,
        ctx: RequestContext<RoleServer>,
    ) -> CallToolResult {
        if ctx.ct.is_cancelled() {
            return CallToolResult::error(vec![ContentBlock::text("engine read cancelled")]);
        }
        let Ok(_permit) = self.read_slots.try_acquire() else {
            return CallToolResult::error(vec![ContentBlock::text(
                "engine read capacity reached; retry later",
            )]);
        };
        tokio::select! {
            biased;
            _ = ctx.ct.cancelled() => CallToolResult::error(vec![ContentBlock::text("engine read cancelled")]),
            result = self.read_with_timeout(request, tool, ENGINE_READ_TIMEOUT) => result,
        }
    }

    /// One engine exchange for a caller that ALREADY holds a read permit: charging a second
    /// would make one call cost two slots, so eight concurrent reads would exhaust a limit of
    /// eight with half of them in flight.
    async fn read_under_permit(
        &self,
        request: Value,
        tool: &str,
        cancellation: tokio_util::sync::CancellationToken,
    ) -> CallToolResult {
        tokio::select! {
            biased;
            _ = cancellation.cancelled() => CallToolResult::error(vec![ContentBlock::text("engine read cancelled")]),
            result = self.read_with_timeout(request, tool, ENGINE_READ_TIMEOUT) => result,
        }
    }

    async fn read_with_timeout(
        &self,
        request: Value,
        tool: &str,
        timeout: Duration,
    ) -> CallToolResult {
        match read_for_with_timeout(self.transport.as_ref(), tool, request, timeout).await {
            Ok(result) => success_result(result, self.payload),
            Err(error) => CallToolResult::error(vec![ContentBlock::text(error)]),
        }
    }

    async fn content_with_timeout(
        &self,
        repo: String,
        relative: String,
        timeout: Duration,
    ) -> CallToolResult {
        match tokio::time::timeout(
            timeout,
            read_content(self.transport.as_ref(), &repo, &relative),
        )
        .await
        {
            Err(_) => CallToolResult::error(vec![ContentBlock::text(format!(
                "engine read 'content' timed out after {timeout:?}; no retry was attempted"
            ))]),
            Ok(Err(error)) => CallToolResult::error(vec![ContentBlock::text(error)]),
            Ok(Ok(result)) => success_result(result, self.payload),
        }
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for Server {
    async fn initialize(
        &self,
        request: InitializeRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, rmcp::ErrorData> {
        if self.initialize_called.swap(true, Ordering::AcqRel) {
            return Err(rmcp::ErrorData::invalid_request(
                "already initialized",
                None,
            ));
        }
        self.negotiate_initialize(&request)
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, rmcp::ErrorData> {
        if !serves_request(&context) {
            return Err(rmcp::ErrorData::invalid_request(
                "initialize required",
                None,
            ));
        }
        if !self.policy.allow_mutations && policy::MUTATION_TOOLS.contains(&request.name.as_ref()) {
            self.audit
                .record(&self.principal, &request.name, "", "denied");
            return Err(rmcp::ErrorData::invalid_request(
                "mutations are disabled by this server's launch policy",
                None,
            ));
        }
        if !self.policy.allow_composition
            && policy::COMPOSITION_TOOLS.contains(&request.name.as_ref())
        {
            self.audit
                .record(&self.principal, &request.name, "", "denied:composition");
            return Err(rmcp::ErrorData::invalid_request(
                "changing the workspace composition is disabled by this server's launch policy",
                None,
            ));
        }
        if !self.policy.allow_workspace_shape
            && policy::WORKSPACE_TOOLS.contains(&request.name.as_ref())
        {
            self.audit
                .record(&self.principal, &request.name, "", "denied:workspace_shape");
            return Err(rmcp::ErrorData::invalid_request(
                "changing which members the workspace serves is disabled by this server's launch policy",
                None,
            ));
        }
        let name = request.name.to_string();
        let response = self
            .tool_router
            .call(ToolCallContext::new(self, request, context))
            .await?;
        Ok(self.explain_argument_error(&name, response))
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, rmcp::ErrorData> {
        if !serves_request(&context) {
            return Err(rmcp::ErrorData::invalid_request(
                "initialize required",
                None,
            ));
        }
        let mut tools = self.visible_tools();
        if self.profile == ToolProfile::Concise {
            // One line per tool and no output schema: everything deeper comes
            // back from tb_describe_tool, so discovery stays cheap.
            for tool in &mut tools {
                tool.description =
                    Some(crate::describe::first_sentence(tool.description.as_deref()).into());
                tool.output_schema = None;
                // Parameter descriptions are the largest remaining cost; the
                // parameter names, types, enums, and required set stay.
                let mut schema = Value::Object((*tool.input_schema).clone());
                strip_descriptions(&mut schema);
                if let Value::Object(schema) = schema {
                    tool.input_schema = std::sync::Arc::new(schema);
                }
            }
        }
        Ok(ListToolsResult {
            tools,
            ..Default::default()
        })
    }

    /// The catalogue as MCP resources, one URI per catalogued file.
    ///
    /// A resource is addressable where a tool call is not: a client can hold
    /// `2brain://vault/inbox/note.md`, re-read it, and show it to a human.
    /// The URI carries the MEMBER and the member-relative path, because absolute
    /// paths break when a workspace moves and carry no member name.
    async fn list_resources(
        &self,
        request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, rmcp::ErrorData> {
        if !serves_request(&context) {
            return Err(rmcp::ErrorData::invalid_request(
                "initialize required",
                None,
            ));
        }
        let page: usize = RESOURCE_PAGE;
        let offset = request
            .and_then(|params| params.cursor)
            .and_then(|cursor| cursor.parse::<usize>().ok())
            .unwrap_or(0);
        let Ok(_permit) = self.read_slots.try_acquire() else {
            return Err(rmcp::ErrorData::internal_error(
                "engine read capacity reached; retry later",
                None,
            ));
        };
        let read = read_engine_with_timeout(
            self.transport.as_ref(),
            json!({"read": "files", "id": 1, "limit": page, "offset": offset}),
            ENGINE_READ_TIMEOUT,
        )
        .await
        .map_err(|error| rmcp::ErrorData::internal_error(error, None))?;
        let members_read = read_engine_with_timeout(
            self.transport.as_ref(),
            json!({"read": "members", "id": 1}),
            ENGINE_READ_TIMEOUT,
        )
        .await
        .map_err(|error| {
            rmcp::ErrorData::internal_error(format!("reading members: {error}"), None)
        })?;
        let members = member_roots(&members_read.payload);
        let catalogued = resource_page(&read.payload, &members);
        // The catalogue pages by the engine's `files` read; the workflow resources
        // sit ahead of it on the first page only, so a cursor always lands on a
        // catalogued page and the two static entries are never duplicated.
        let more_catalogued = catalogued.len() == page;
        let mut resources = contract::workflow_resources();
        resources.extend(catalogued);
        let next_cursor = more_catalogued.then(|| (offset + page).to_string());
        Ok(ListResourcesResult {
            resources,
            next_cursor,
            ..Default::default()
        })
    }

    /// The URI shape a client can construct without listing first.
    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, rmcp::ErrorData> {
        if !serves_request(&context) {
            return Err(rmcp::ErrorData::invalid_request(
                "initialize required",
                None,
            ));
        }
        Ok(ListResourceTemplatesResult {
            resource_templates: vec![
                ResourceTemplate::new("2brain://{repo}/{path}", "workspace file")
                    .with_mime_type("text/markdown")
                    .with_description(
                        "A catalogued file: the member's name, then its path inside that member",
                    ),
            ],
            ..Default::default()
        })
    }

    /// Read one resource: the engine's own content read, unchanged.
    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, rmcp::ErrorData> {
        if !serves_request(&context) {
            return Err(rmcp::ErrorData::invalid_request(
                "initialize required",
                None,
            ));
        }
        // The workflow resources are static text: they answer without the engine,
        // and an unknown workflow name never falls through to a member read.
        match contract::workflow_resource(&request.uri) {
            None => {}
            Some(Ok(text)) => {
                return Ok(ReadResourceResponse::Complete(ReadResourceResult::new(
                    vec![
                        ResourceContents::text(text.to_owned(), request.uri.clone())
                            .with_mime_type("text/markdown"),
                    ],
                )));
            }
            Some(Err(error)) => return Err(error),
        }
        let (repo, path) = parse_resource_uri(&request.uri).ok_or_else(|| {
            rmcp::ErrorData::invalid_params(
                format!("not an 2brain resource URI: {}", request.uri),
                None,
            )
        })?;
        let Ok(_permit) = self.read_slots.try_acquire() else {
            return Err(rmcp::ErrorData::internal_error(
                "engine read capacity reached; retry later",
                None,
            ));
        };
        let read = read_content(self.transport.as_ref(), &repo, &path)
            .await
            .map_err(|error| rmcp::ErrorData::internal_error(error, None))?;
        let content = read.payload.get("content").cloned().unwrap_or(Value::Null);
        let Some(text) = content.get("text").and_then(Value::as_str) else {
            return Err(rmcp::ErrorData::resource_not_found(
                format!("no readable content at {}", request.uri),
                None,
            ));
        };
        Ok(ReadResourceResponse::Complete(ReadResourceResult::new(
            vec![
                ResourceContents::text(text.to_owned(), request.uri)
                    .with_mime_type("text/markdown"),
            ],
        )))
    }

    /// The answer contract as MCP prompts; the workflow texts are `contract`'s.
    async fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, rmcp::ErrorData> {
        if !serves_request(&context) {
            return Err(rmcp::ErrorData::invalid_request(
                "initialize required",
                None,
            ));
        }
        Ok(ListPromptsResult {
            prompts: contract::prompts(),
            ..Default::default()
        })
    }

    /// One workflow with the caller's argument substituted into its template.
    async fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<GetPromptResponse, rmcp::ErrorData> {
        if !serves_request(&context) {
            return Err(rmcp::ErrorData::invalid_request(
                "initialize required",
                None,
            ));
        }
        contract::prompt(&request.name, request.arguments.as_ref()).map(Into::into)
    }

    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .enable_prompts()
                .build(),
        )
        .with_server_info(Implementation::new(
            "2brain", // the product name clients see; the crate keeps `tb-mcp-rs`
            version_string(),
        ))
        .with_instructions(contract::INSTRUCTIONS)
    }

    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        Cow::Borrowed(SUPPORTED_PROTOCOL_VERSIONS)
    }
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct WriteArguments {
    /// Workspace member that owns the file.
    #[schemars(length(min = 1))]
    repo: String,
    /// Path relative to that member's root; absolute paths and `..` are rejected.
    #[schemars(length(min = 1))]
    path: String,
    /// Full replacement text.
    content: String,
    /// Hash returned by the tb_content read that this write is based on.
    #[schemars(length(min = 1))]
    expected_hash: String,
    /// Refuse instead of writing when the state this write would produce carries an
    /// ERROR-severity diagnostic. Off by default: a draft may be inconsistent.
    #[serde(default)]
    require_clean: bool,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct EditArguments {
    /// Workspace member that owns the file.
    #[schemars(length(min = 1))]
    repo: String,
    /// Path relative to that member's root; absolute paths and `..` are rejected.
    #[schemars(length(min = 1))]
    path: String,
    /// Exact text to replace; must match once unless `replace_all` is true.
    old_string: String,
    /// Replacement text.
    new_string: String,
    /// Replace every exact occurrence.
    #[serde(default)]
    replace_all: bool,
    /// Hash returned by the tb_content read that this edit is based on.
    #[schemars(length(min = 1))]
    expected_hash: String,
    /// Refuse instead of writing when the state this edit would produce carries an
    /// ERROR-severity diagnostic. Off by default: a draft may be inconsistent.
    #[serde(default)]
    require_clean: bool,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct AssignBlockIdArguments {
    /// Workspace member that owns the file.
    #[schemars(length(min = 1))]
    repo: String,
    /// File that holds the entity, relative to the member root.
    #[schemars(length(min = 1))]
    path: String,
    /// Byte offset inside the file, from a span the server reported.
    at: u64,
    /// Hash returned by the tb_content read that this change is based on.
    #[schemars(length(min = 1))]
    expected_hash: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct RenameBlockIdArguments {
    /// Workspace member that owns the host file.
    #[schemars(length(min = 1))]
    repo: String,
    /// Host file carrying an inline record, relative to the member root.
    #[schemars(length(min = 1))]
    path: String,
    /// The record's current block id.
    #[schemars(length(min = 1))]
    block_id: String,
    /// The new id; must match the block-id grammar and be free in that file.
    #[schemars(length(min = 1))]
    to_block_id: String,
    /// Hash returned by the tb_content read that this change is based on.
    #[schemars(length(min = 1))]
    expected_hash: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct RenameArguments {
    /// Workspace member that owns the file.
    #[schemars(length(min = 1))]
    repo: String,
    /// Current path, relative to the member root.
    #[schemars(length(min = 1))]
    path: String,
    /// New path in the same member; a type-def file must use tb_rename_type.
    #[schemars(length(min = 1))]
    to: String,
    /// Hash returned by the tb_content read that this change is based on.
    #[schemars(length(min = 1))]
    expected_hash: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct CreateArguments {
    /// Member that will own the new file.
    #[schemars(length(min = 1))]
    repo: String,
    /// New path, relative to the member root.
    #[schemars(length(min = 1))]
    path: String,
    /// Full file text.
    content: String,
    /// Refuse instead of creating when the state this create would produce carries an
    /// ERROR-severity diagnostic.
    #[serde(default)]
    require_clean: bool,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct AddWorkspaceMemberArguments {
    /// Editable member whose `.2brain/workspace.yaml` gains the entry.
    #[schemars(length(min = 1))]
    repo: String,
    /// Name of the member to declare.
    #[schemars(length(min = 1))]
    name: String,
    /// Which list to declare it in.
    role: WorkspaceRole,
    /// Compare-and-set hash of the workspace.yaml this change is based on.
    expected_hash: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct RemoveWorkspaceMemberArguments {
    /// Editable member whose `.2brain/workspace.yaml` loses the entry.
    #[schemars(length(min = 1))]
    repo: String,
    /// Name of the member to remove.
    #[schemars(length(min = 1))]
    name: String,
    /// Compare-and-set hash of the workspace.yaml this change is based on.
    expected_hash: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetWorkspaceMemberRoleArguments {
    /// Editable member whose `.2brain/workspace.yaml` is authored.
    #[schemars(length(min = 1))]
    repo: String,
    /// Name of the declared member whose role changes.
    #[schemars(length(min = 1))]
    name: String,
    /// The target list.
    role: WorkspaceRole,
    /// Compare-and-set hash of the workspace.yaml this change is based on.
    expected_hash: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
enum WorkspaceRole {
    Edit,
    Discover,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct InlineArguments {
    /// Member that owns both files.
    #[schemars(length(min = 1))]
    repo: String,
    /// The file to fold in and delete, relative to the member root.
    #[schemars(length(min = 1))]
    path: String,
    /// The referrer that will host the record, relative to the member root.
    #[schemars(length(min = 1))]
    into: String,
    /// Byte offset in `into` landing on the chosen [[path]] reference, required
    /// when `into` references it more than once.
    at: Option<u64>,
    /// Hash of `path` from its tb_content read.
    #[schemars(length(min = 1))]
    expected_hash: String,
    /// Hash of `into` from its tb_content read.
    #[schemars(length(min = 1))]
    into_expected_hash: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct PromoteArguments {
    /// Member that owns the host file.
    #[schemars(length(min = 1))]
    repo: String,
    /// Host file holding the inline record, relative to the member root.
    #[schemars(length(min = 1))]
    path: String,
    /// New file to create, relative to the member root; extension selects the
    /// format (`.md`, `.markdown`, `.yaml`, `.yml`).
    #[schemars(length(min = 1))]
    to: String,
    /// Byte offset inside the record; use this or `block_id`, not both.
    at: Option<u64>,
    /// The record's block id; use this or `at`, not both.
    block_id: Option<String>,
    /// Hash of `path` from its tb_content read.
    #[schemars(length(min = 1))]
    expected_hash: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct RenameTypeArguments {
    /// The type's current name, bare or `name::repo` when two owners exist.
    #[schemars(length(min = 1))]
    old_name: String,
    /// The owner's new bare name; must not already name a type there.
    #[schemars(length(min = 1))]
    new_name: String,
    /// The type's `hash` from tb_type, so a stale read cannot rename.
    #[schemars(length(min = 1))]
    expected_hash: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct AddDepArguments {
    /// Editable member whose `.2brain/repo.yaml` gains the entry.
    #[schemars(length(min = 1))]
    repo: String,
    /// Name of the dependency to declare.
    #[schemars(length(min = 1))]
    dep: String,
    /// Where the peer lives, when it is not local.
    remote: Option<String>,
    /// Peer revision to track.
    #[serde(rename = "ref")]
    reference: Option<String>,
    /// Compare-and-set hash of the repo.yaml this change is based on.
    expected_hash: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct RemoveDepArguments {
    /// Editable member whose `.2brain/repo.yaml` loses the entry.
    #[schemars(length(min = 1))]
    repo: String,
    /// Name of the dependency to remove.
    #[schemars(length(min = 1))]
    dep: String,
    /// Compare-and-set hash of the repo.yaml this change is based on.
    expected_hash: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct DeleteArguments {
    /// Workspace member that owns the file.
    #[schemars(length(min = 1))]
    repo: String,
    /// Path relative to that member's root; absolute paths and `..` are rejected.
    #[schemars(length(min = 1))]
    path: String,
    /// Hash returned by the tb_content read that this delete is based on.
    #[schemars(length(min = 1))]
    expected_hash: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
enum MutationOp {
    #[serde(rename = "write_file")]
    Write,
    #[serde(rename = "edit_file")]
    Edit,
    #[serde(rename = "delete_file")]
    Delete,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct PreviewArguments {
    /// Workspace member that would own the file.
    #[schemars(length(min = 1))]
    repo: String,
    /// Path relative to that member's root; absolute paths and `..` are rejected.
    #[schemars(length(min = 1))]
    path: String,
    /// Mutation to preview.
    op: MutationOp,
    /// Replacement text, required for write_file.
    content: Option<String>,
    /// Exact text to replace, required for edit_file.
    old_string: Option<String>,
    /// Replacement text, required for edit_file.
    new_string: Option<String>,
    /// Replace every exact occurrence, used by edit_file.
    #[serde(default)]
    replace_all: bool,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReconcileArguments {
    /// Workspace member that owns the file.
    #[schemars(length(min = 1))]
    repo: String,
    /// Path relative to that member's root; absolute paths and `..` are rejected.
    #[schemars(length(min = 1))]
    path: String,
    /// Hash the interrupted write should have produced.
    expected_hash: Option<String>,
    /// Set when the interrupted mutation was a delete.
    #[serde(default)]
    expect_absent: bool,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct InstancesOfArguments {
    /// Type name, bare or qualified as name::repo.
    #[schemars(length(min = 1))]
    type_name: String,
    /// Restrict to these site kinds; absent means every kind.
    origins: Option<Vec<InstanceOrigin>>,
    /// Splice each match's resolved view onto its record.
    #[serde(default)]
    instance: bool,
    /// Splice each match's markdown body onto its record.
    #[serde(default)]
    body: bool,
    /// Return at most this many records, with a continuation offset.
    #[schemars(range(min = 1))]
    limit: Option<usize>,
    /// Skip this many records before the page.
    offset: Option<usize>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
enum InstanceOrigin {
    File,
    Nested,
    Meta,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct ValidateValueArguments {
    /// Type name, bare or qualified as name::repo.
    #[schemars(length(min = 1))]
    type_name: String,
    /// The instance body to validate; it never touches disk.
    value: Value,
    /// Scope to one member's identity; a qualified name wins over this.
    #[schemars(length(min = 1))]
    repo: Option<String>,
}

/// One file that passed every gate but the slot.
struct ArmedFile {
    key: (String, String),
    target: String,
}

/// A mutation that passed every gate: it holds the single mutation slot and the
/// resolved absolute target.
struct PreparedMutation {
    key: (String, String),
    target: String,
    _permit: tokio::sync::OwnedSemaphorePermit,
}

use policy::Denial;

#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct SearchToolsArguments {
    /// Words describing what you want to do.
    #[schemars(length(min = 1))]
    query: String,
    /// How much to return per match; default summary.
    #[serde(default)]
    detail: SearchDetail,
    /// Keep only tools of this capability: read, mutate, discover, or watch. A
    /// mutating tool never appears here without the grant that advertises it.
    #[serde(default)]
    capability: Option<crate::capability::Capability>,
    /// Maximum matches, from 1 to 20 (default 5).
    #[serde(default = "default_search_limit")]
    #[schemars(range(min = 1, max = 20))]
    limit: usize,
}

#[derive(serde::Deserialize, schemars::JsonSchema, Default, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum SearchDetail {
    /// Names only.
    Name,
    /// Name and a one-line summary.
    #[default]
    Summary,
    /// Full definitions with schemas.
    Schema,
}

fn default_search_limit() -> usize {
    5
}

fn default_rank_limit() -> usize {
    20
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct DescribeToolArguments {
    /// Tool name, exactly as tools/list reports it.
    #[schemars(length(min = 1))]
    name: String,
}

/// Page an array that sits under `key`, keeping the rest of the payload.
fn page_array(
    result: CallToolResult,
    key: &str,
    limit: Option<usize>,
    offset: usize,
    mode: PayloadMode,
) -> CallToolResult {
    if result.is_error == Some(true) {
        return result;
    }
    let raw = match (&result.structured_content, result.content.first()) {
        (Some(value), _) => value.to_string(),
        (None, Some(block)) => block
            .as_text()
            .map(|text| text.text.clone())
            .unwrap_or_default(),
        _ => return result,
    };
    let Ok(mut payload) = serde_json::from_str::<Value>(&raw) else {
        return result;
    };
    let Some(records) = payload.get(key).and_then(Value::as_array).cloned() else {
        return result;
    };
    let total = records.len();
    let page: Vec<Value> = records
        .into_iter()
        .skip(offset)
        .take(limit.unwrap_or(usize::MAX))
        .collect();
    let returned = page.len();
    payload[key] = Value::Array(page);
    payload["total"] = json!(total);
    payload["offset"] = json!(offset);
    payload["next_offset"] = match offset + returned < total {
        true => json!(offset + returned),
        false => Value::Null,
    };
    let mut response = success_payload(payload, mode);
    response.meta = result.meta;
    response
}

/// Whether this request may use tools.
///
/// A legacy session proves itself with `initialize`. A `2026-07-28` (or later)
/// request carries its version in `_meta` and needs no handshake. A `_meta`
/// declaring an older version is not an inline lifecycle and is refused, so
/// metadata cannot smuggle a call past the handshake.
fn serves_request(context: &RequestContext<RoleServer>) -> bool {
    context.peer.peer_info().is_some()
        || context
            .meta
            .protocol_version()
            .is_some_and(|version| version.as_str() >= ProtocolVersion::V_2026_07_28.as_str())
}

/// The content hash reported by a successful `tb_content` result.
///
/// The answer may carry the payload in `structuredContent` only (`--payload structured`), in a
/// text block only (`text`), or in both. Reading the text block alone would leave the read gate
/// unarmed in structured mode, so the caller's next write would be refused for a read it had
/// just made. Structured wins because it is the payload itself; the text block is a serialized
/// copy of the same value.
fn served_hash(result: &CallToolResult) -> Option<String> {
    if let Some(structured) = result.structured_content.as_ref()
        && let Some(hash) = structured
            .get("content")
            .and_then(|content| content.get("hash"))
            .and_then(Value::as_str)
    {
        return Some(hash.to_owned());
    }
    let text = result.content.first()?.as_text()?.text.clone();
    let payload: Value = serde_json::from_str(&text).ok()?;
    payload
        .get("content")?
        .get("hash")?
        .as_str()
        .map(str::to_owned)
}

/// The wire spelling of a workspace role.
fn workspace_role(role: WorkspaceRole) -> &'static str {
    match role {
        WorkspaceRole::Edit => "edit",
        WorkspaceRole::Discover => "discover",
    }
}

/// A tool error from a string, so call sites stay one line.
fn return_error(message: impl Into<String>) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(message.into())])
}

/// A locally produced answer (discovery results), shaped like a tool payload.
fn success_json(payload: Value, mode: PayloadMode) -> CallToolResult {
    success_payload(payload, mode)
}

/// Shape one answer according to `mode`: text, structured content, or both.
fn success_payload(payload: Value, mode: PayloadMode) -> CallToolResult {
    let mut response = CallToolResult::default();
    match mode {
        PayloadMode::Both => {
            response.content = vec![ContentBlock::text(payload.to_string())];
            response.structured_content = Some(payload);
        }
        PayloadMode::Text => {
            response.content = vec![ContentBlock::text(payload.to_string())];
        }
        PayloadMode::Structured => {
            response.structured_content = Some(payload);
        }
    }
    response
}

/// `--info`: what this build is, and — when a socket is reachable — what it
/// would serve. Static parts never need an engine, so the command works before a
/// daemon exists.
async fn print_info(arguments: Vec<std::ffi::OsString>) -> Result<(), Box<dyn std::error::Error>> {
    // The unified `2brain` binary calls this too, so name the caller.
    let program = env::args_os()
        .next()
        .map(PathBuf::from)
        .and_then(|path| {
            path.file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| NAME.to_owned());
    println!("{program} {}", version_string());
    for line in build_lines() {
        println!("  {line}");
    }
    println!("  role         MCP stdio server for the 2brain engine");
    println!(
        "  protocol     legacy 2024-11-05, 2025-03-26, 2025-06-18, 2025-11-25 (initialize first)"
    );
    println!("               modern 2026-07-28 (version in _meta, no initialize)");
    // Counted from the router, so the numbers cannot drift from the surface.
    let registered = Server::tool_router().list_all().len();
    let mutations = policy::MUTATION_TOOLS.len();
    println!(
        "  tools        {registered} registered, {mutations} mutating; advertised: {} without a grant, {registered} with one",
        registered - mutations
    );
    println!(
        "  result       {{ \"<read>\": ... }} plus structuredContent and _meta engine version"
    );
    println!("  profiles     --tools full|concise, --payload both|text|structured");
    println!("  wire         schema 29, 16 MiB frames, 30 s read deadline, 8 in-flight reads");

    let parsed = match parse_args(arguments.into_iter()) {
        Ok(parsed) => Some(parsed),
        Err(message) => {
            println!("  launch       no socket given, so no policy or engine details");
            println!("               ({message})");
            None
        }
    };
    let Some((socket, policy, principal, profile, payload)) = parsed else {
        return Ok(());
    };
    println!(
        "  policy       {}",
        if policy.allow_mutations {
            format!(
                "mutations granted, members {:?}, audit {}",
                policy.members,
                policy.audit.describe()
            )
        } else {
            "read-only (no grant)".to_owned()
        }
    );
    println!(
        "  launch       principal {principal:?}, tools {:?}, payload {:?}",
        profile, payload
    );
    println!("  engine       {}", socket.display());

    let transport = UnixTransport::new(socket);
    match tokio::time::timeout(
        Duration::from_secs(5),
        transport.exchange(json!({"read": "overview", "id": 1})),
    )
    .await
    {
        Err(_) => println!("    status     no answer within 5 s"),
        Ok(Err(WireError::Broken(message))) => println!("    status     unusable: {message}"),
        Ok(Err(WireError::Rejected { message, .. })) => {
            println!("    status     rejected: {message}");
        }
        Ok(Ok(response)) => {
            let overview = &response["result"]["overview"];
            println!("    ready      true");
            println!(
                "    schema     {} (this build supports {SCHEMA_VERSION})",
                response["schema_version"]
            );
            println!("    version    {}", response["version"]);
            println!(
                "    workspace  {} members, {} type entries, {} diagnostics",
                overview["members"].as_array().map_or(0, Vec::len),
                overview["type_counts"].as_object().map_or_else(
                    || overview["type_counts"].as_array().map_or(0, Vec::len),
                    serde_json::Map::len,
                ),
                overview["diagnostic_counts"]["total"]
                    .as_u64()
                    .unwrap_or_default()
            );
        }
    }
    Ok(())
}

fn success_result(result: ReadResult, mode: PayloadMode) -> CallToolResult {
    let version = result.version;
    let mut response = success_payload(result.payload, mode);
    response.meta = Some(MetaObject(serde_json::Map::from_iter([(
        "io.2brain/engineVersion".to_owned(),
        Value::String(version.to_string()),
    )])));
    response
}

/// Drop every `description` from a JSON schema, keeping structure and enums.
fn strip_descriptions(value: &mut Value) {
    match value {
        Value::Object(map) => {
            map.remove("description");
            // Presentation-only keys, repeated in every tool schema.
            map.remove("$schema");
            map.remove("title");
            for item in map.values_mut() {
                strip_descriptions(item);
            }
        }
        Value::Array(items) => {
            for item in items {
                strip_descriptions(item);
            }
        }
        _ => {}
    }
}

/// Page an `instances_of` answer, saying how many records exist in total.
fn page_instances(
    result: CallToolResult,
    limit: Option<usize>,
    offset: usize,
    mode: PayloadMode,
) -> CallToolResult {
    page_array(result, "instances_of", limit, offset, mode)
}

/// Cut a `content` answer to `max` characters, keeping the whole-file hash.
///
/// Truncation is explicit: the answer says how much was withheld and where to
/// continue, so a caller never mistakes a window for the whole file.
fn window_content(result: CallToolResult, max: usize, mode: PayloadMode) -> CallToolResult {
    if result.is_error == Some(true) {
        return result;
    }
    let raw = match (&result.structured_content, result.content.first()) {
        (Some(value), _) => value.to_string(),
        (None, Some(block)) => block
            .as_text()
            .map(|text| text.text.clone())
            .unwrap_or_default(),
        _ => return result,
    };
    let Ok(mut payload) = serde_json::from_str::<Value>(&raw) else {
        return result;
    };
    let Some(text) = payload
        .get("content")
        .and_then(|content| content.get("text"))
        .and_then(Value::as_str)
    else {
        return result;
    };
    let total = text.chars().count();
    if total <= max {
        return result;
    }
    let window: String = text.chars().take(max).collect();
    payload["content"]["text"] = Value::String(window);
    payload["content"]["truncated"] = Value::Bool(true);
    payload["content"]["total_chars"] = json!(total);
    payload["content"]["next_offset"] = json!(max);
    let mut response = success_payload(payload, mode);
    response.meta = result.meta;
    response
}

/// Reject a member-relative path that could escape the member root.
/// An engine content hash: 16 lowercase hex characters. `tb_content` returns it, and
/// a mutation carries it back as `expected_hash`; anything else is not a digest and
/// must not be compared as one.
fn is_engine_hash(value: &str) -> bool {
    value.len() == 16
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn relative_path(path: &str) -> Result<(), &'static str> {
    if path.starts_with('/') || path.starts_with('\\') {
        return Err("'path' must be relative to the member root");
    }
    if path.contains('\0') {
        return Err("'path' must not contain NUL");
    }
    if Path::new(path)
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err("'path' must not contain '..'");
    }
    Ok(())
}

const NAME: &str = env!("CARGO_PKG_NAME");
const VERSION: &str = env!("CARGO_PKG_VERSION");
/// The git commit this binary was built from, or `unknown` outside a checkout.
const BUILD_COMMIT: &str = env!("TB_BUILD_COMMIT");
/// Whether this crate's own tree was modified at build time.
const BUILD_DIRTY: bool = matches!(env!("TB_BUILD_DIRTY").as_bytes(), b"true");
const BUILD_TARGET: &str = env!("TB_BUILD_TARGET");
const BUILD_PROFILE: &str = env!("TB_BUILD_PROFILE");

/// `0.1.0+<commit>`, with `.dirty` when the tree was modified.
///
/// The commit rides semver build metadata, so a client that parses `version`
/// still sees a valid version, and a bug report names an exact source.
pub fn version_string() -> String {
    match BUILD_DIRTY {
        true => format!("{VERSION}+{BUILD_COMMIT}.dirty"),
        false => format!("{VERSION}+{BUILD_COMMIT}"),
    }
}

/// The version lines `--info` prints: version plus the build identity.
pub fn build_lines() -> Vec<String> {
    vec![
        format!(
            "source       {BUILD_COMMIT}{}",
            if BUILD_DIRTY { " (dirty tree)" } else { "" }
        ),
        format!("build        {BUILD_PROFILE} for {BUILD_TARGET}"),
    ]
}

const USAGE: &str = "usage: tb-mcp-rs <engine-unix-socket> [--policy <file>] [--allow-mutations] [--allow-composition] [--allow-workspace-shape] [--principal <name>] [--tools full|concise] [--payload both|text|structured]";
/// Parse the launch arguments. Mutations need an explicit grant: either a
/// policy file that allows them or `--allow-mutations`.
fn parse_args(
    arguments: impl Iterator<Item = std::ffi::OsString>,
) -> Result<(PathBuf, policy::Policy, String, ToolProfile, PayloadMode), String> {
    let mut socket: Option<PathBuf> = None;
    let mut policy_path: Option<PathBuf> = None;
    let mut allow_mutations = false;
    let mut allow_composition = false;
    let mut allow_workspace_shape = false;
    let mut principal = "unknown".to_owned();
    let mut profile = ToolProfile::Concise;
    let mut payload = PayloadMode::Both;
    let mut arguments = arguments.peekable();
    while let Some(argument) = arguments.next() {
        let text = argument.to_string_lossy().into_owned();
        match text.as_str() {
            "--policy" => {
                let path = arguments.next().ok_or(USAGE)?;
                policy_path = Some(PathBuf::from(path));
            }
            "--principal" => {
                let name = arguments.next().ok_or(USAGE)?;
                principal = name.to_string_lossy().into_owned();
            }
            "--tools" => {
                let value = arguments.next().ok_or(USAGE)?;
                profile = match value.to_string_lossy().as_ref() {
                    "full" => ToolProfile::Full,
                    "concise" => ToolProfile::Concise,
                    other => return Err(format!("unknown tool profile: {other}. {USAGE}")),
                };
            }
            "--payload" => {
                let value = arguments.next().ok_or(USAGE)?;
                payload = match value.to_string_lossy().as_ref() {
                    "both" => PayloadMode::Both,
                    "text" => PayloadMode::Text,
                    "structured" => PayloadMode::Structured,
                    other => return Err(format!("unknown payload mode: {other}. {USAGE}")),
                };
            }
            "--allow-mutations" => allow_mutations = true,
            "--allow-composition" => allow_composition = true,
            "--allow-workspace-shape" => allow_workspace_shape = true,
            other if other.starts_with("--") => {
                return Err(format!("unknown option: {other}. {USAGE}"));
            }
            other => {
                if socket.is_some() {
                    return Err(USAGE.into());
                }
                socket = Some(PathBuf::from(other));
            }
        }
    }
    let socket = socket.ok_or(USAGE)?;
    let mut policy = match policy_path {
        Some(path) => policy::Policy::load(&path)?,
        None => policy::Policy::read_only(),
    };
    if allow_mutations {
        policy.allow_mutations = true;
    }
    if allow_composition {
        policy.allow_mutations = true;
        policy.allow_composition = true;
    }
    if allow_workspace_shape {
        policy.allow_mutations = true;
        policy.allow_workspace_shape = true;
    }
    Ok((socket, policy, principal, profile, payload))
}

/// Restore the default SIGPIPE behaviour so a closed pipe (`| head`) ends the
/// process quietly instead of panicking inside `println!`.
pub fn reset_sigpipe() {
    #[cfg(unix)]
    // SAFETY: setting a signal disposition to the platform default is
    // async-signal-safe and needs no state.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
}

/// Entry point for the `tb-mcp-rs` binary: build a runtime and serve.
pub fn main_entry() -> std::process::ExitCode {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("tb-mcp-rs: cannot start a runtime: {error}");
            return std::process::ExitCode::FAILURE;
        }
    };
    match runtime.block_on(run(env::args_os().skip(1).collect())) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("tb-mcp-rs: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

/// Serve, or handle `--help`, `--version`, and `--info`.
///
/// `2brain mcp …` calls this with the same arguments after its own parser
/// removed the subcommand.
pub async fn run(arguments: Vec<std::ffi::OsString>) -> Result<(), Box<dyn std::error::Error>> {
    let flags: Vec<&str> = arguments
        .iter()
        .filter_map(|argument| argument.to_str())
        .filter(|argument| matches!(*argument, "--help" | "-h" | "--version" | "-V" | "--info"))
        .collect();
    if !flags.is_empty() {
        // A short-lived command writing to a closed pipe (`| head`) should end
        // quietly. A serving process keeps the library's EPIPE handling, so its
        // bounded transport decides when to stop instead of dying on a signal.
        reset_sigpipe();
    }
    if flags.iter().any(|flag| matches!(*flag, "--help" | "-h")) {
        print!("{}", help::text());
        return Ok(());
    }
    if flags.iter().any(|flag| matches!(*flag, "--version" | "-V")) {
        println!("{NAME} {}", version_string());
        return Ok(());
    }
    if flags.contains(&"--info") {
        let rest: Vec<std::ffi::OsString> = arguments
            .into_iter()
            .filter(|argument| argument.to_str() != Some("--info"))
            .collect();
        return print_info(rest).await;
    }
    let (socket, policy, principal, profile, payload) = parse_args(arguments.into_iter())
        .unwrap_or_else(|message| {
            eprintln!("tb-mcp-rs: {message}");
            eprintln!("try 'tb-mcp-rs --help'");
            std::process::exit(2);
        });
    let service = Server::new(socket, policy, principal, profile, payload)
        .serve(bounded_stdio::BoundedStdio::new(
            SUPPORTED_PROTOCOL_VERSIONS.to_vec(),
        ))
        .await?;
    service.waiting().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::{model::NumberOrString, service::serve_directly};
    use tokio::net::UnixListener;

    fn test_server(socket: PathBuf) -> Server {
        Server::new(
            socket,
            policy::Policy::read_only(),
            "test".to_owned(),
            ToolProfile::Full,
            PayloadMode::Both,
        )
    }

    /// A server whose engine is a script, so a test can both see the exact
    /// request the adapter built and choose the frame that answers it.
    fn scripted_server(
        answer: impl Fn(&Value) -> Result<Value, WireError> + Send + Sync + 'static,
    ) -> Server {
        Server::with_transport(
            Arc::new(MemoryTransport::new(answer)),
            policy::Policy::read_only(),
            "test".to_owned(),
            ToolProfile::Full,
            PayloadMode::Both,
        )
    }

    /// The capability table is the single place a tool is classified. It must cover the
    /// router exactly — one row per advertised tool, no row for a tool that does not
    /// exist — and it must agree with the grant lists that decide advertising.
    #[test]
    fn every_tool_has_exactly_one_capability() {
        let server = test_server(PathBuf::from("/tmp/capability.sock"));
        let mut routed: Vec<String> = server
            .tool_router
            .list_all()
            .iter()
            .map(|tool| tool.name.to_string())
            .collect();
        routed.sort();
        let mut tabulated: Vec<String> = capability::CAPABILITIES
            .iter()
            .map(|(name, _)| (*name).to_owned())
            .collect();
        tabulated.sort();
        assert_eq!(routed, tabulated, "the capability table is not the router");

        for (name, capability) in capability::CAPABILITIES {
            let mutating = policy::MUTATION_TOOLS.contains(&name);
            assert_eq!(
                mutating,
                capability == capability::Capability::Mutate,
                "{name}: mutations grant and capability disagree"
            );
        }
        for name in policy::MUTATION_TOOLS {
            assert_eq!(
                capability::of(name),
                Some(capability::Capability::Mutate),
                "{name}"
            );
        }
        assert_eq!(
            capability::of("tb_search_tools"),
            Some(capability::Capability::Discover)
        );
        assert_eq!(
            capability::of("tb_describe_tool"),
            Some(capability::Capability::Discover)
        );
        assert_eq!(
            capability::of("tb_watch"),
            Some(capability::Capability::Watch)
        );
        assert_eq!(capability::of("no_such_tool"), None);
    }

    /// A read tool's engine verb is stated once, in `wire::READ_VERBS`. Both directions of that
    /// table are asserted here: every advertised read tool is a row — a composed tool lists the
    /// verbs it builds its answer from, so it is a row too — and every verb a row names is a verb
    /// the wire can validate. Nothing joined a tool to a shape when `search` shipped, and every
    /// call to it failed as a result.
    #[test]
    fn every_read_tool_declares_its_engine_verb() {
        let mut read_tools: Vec<&str> = capability::CAPABILITIES
            .iter()
            .filter(|(_, capability)| *capability == capability::Capability::Read)
            .map(|(name, _)| *name)
            .collect();
        read_tools.sort_unstable();
        let mut tabulated: Vec<&str> = wire::READ_VERBS.iter().map(|(tool, _)| *tool).collect();
        tabulated.sort_unstable();
        assert_eq!(
            read_tools, tabulated,
            "the verb table and the advertised read tools disagree"
        );

        for (tool, verbs) in wire::READ_VERBS {
            assert!(!verbs.is_empty(), "{tool} declares no engine read verb");
            for &verb in *verbs {
                assert!(
                    wire::shape_of(verb).is_some(),
                    "{tool} asks for the read {verb:?}, which VERB_SHAPES does not declare"
                );
            }
        }
    }

    fn request_context() -> RequestContext<RoleServer> {
        let (transport, _client) = tokio::io::duplex(64);
        let service = serve_directly(test_server(PathBuf::new()), transport, None);
        RequestContext::new(NumberOrString::Number(1), service.peer().clone())
    }

    async fn fake_response(body: Vec<u8>, declared_len: Option<u32>) -> Result<ReadResult, String> {
        fake_response_for(body, declared_len, "overview").await
    }

    async fn fake_response_for(
        body: Vec<u8>,
        declared_len: Option<u32>,
        verb: &str,
    ) -> Result<ReadResult, String> {
        let request = json!({"read":verb,"id":1});
        let expected_request = request.clone();
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("fake.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let len = socket.read_u32().await.unwrap();
            let mut request = vec![0; len as usize];
            socket.read_exact(&mut request).await.unwrap();
            assert_eq!(
                serde_json::from_slice::<Value>(&request).unwrap(),
                expected_request
            );
            socket
                .write_u32(declared_len.unwrap_or(body.len() as u32))
                .await
                .unwrap();
            if !body.is_empty() {
                let _ = socket.write_all(&body).await;
            }
        });
        let result = read_engine(&UnixTransport::new(path), request).await;
        task.await.unwrap();
        result
    }

    #[test]
    fn empty_arguments_reject_unknown_fields() {
        assert!(serde_json::from_value::<EmptyArguments>(json!({})).is_ok());
        assert!(serde_json::from_value::<EmptyArguments>(json!({"scope": "other"})).is_err());
        let router = Server::tool_router();
        for name in ["tb_overview", "tb_top_level_dirs", "tb_type_tree"] {
            let tool = router.get(name).unwrap();
            assert_eq!(
                tool.input_schema.get("additionalProperties"),
                Some(&json!(false))
            );
        }
    }

    #[test]
    fn type_schema_requires_nonempty_name_and_repo_when_present() {
        let router = Server::tool_router();
        let schema = &router.get("tb_type").unwrap().input_schema;
        assert_eq!(schema["properties"]["name"]["minLength"], 1);
        let repo = &schema["properties"]["repo"];
        assert_eq!(repo["minLength"], 1);
    }

    #[tokio::test]
    async fn type_rejects_empty_name_or_repo_before_connecting() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("missing.sock");
        let server = test_server(path);
        for (name, repo, expected) in [
            ("", None, "'name' is required"),
            ("Type", Some(""), "'repo' must not be empty"),
            ("Type::member", Some(""), "'repo' must not be empty"),
        ] {
            let result = server
                .tb_type(
                    Parameters(TypeArguments {
                        name: name.into(),
                        repo: repo.map(str::to_owned),
                    }),
                    request_context(),
                )
                .await;
            let result = serde_json::to_value(result).unwrap();
            assert_eq!(result["isError"], true);
            assert_eq!(result["content"][0]["text"], expected);
        }
    }

    #[tokio::test]
    async fn type_qualifies_only_unqualified_names_with_nonempty_repo() {
        for (name, repo, expected) in [
            ("Type", None, "Type"),
            ("Type", Some("member"), "Type::member"),
            ("Type", Some("unknown"), "Type::unknown"),
            ("Type::member", Some("other"), "Type::member"),
        ] {
            let temp = tempfile::tempdir().unwrap();
            let path = temp.path().join("fake.sock");
            let listener = UnixListener::bind(&path).unwrap();
            let task = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let len = socket.read_u32().await.unwrap();
                let mut request = vec![0; len as usize];
                socket.read_exact(&mut request).await.unwrap();
                assert_eq!(
                    serde_json::from_slice::<Value>(&request).unwrap(),
                    json!({"read":"type","id":1,"name":expected})
                );
                let frame = json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":1,"result":{"type":null}});
                let body = serde_json::to_vec(&frame).unwrap();
                socket.write_u32(body.len() as u32).await.unwrap();
                socket.write_all(&body).await.unwrap();
            });
            let args: TypeArguments =
                serde_json::from_value(json!({"name":name,"repo":repo})).unwrap();
            let result = test_server(path)
                .tb_type(Parameters(args), request_context())
                .await;
            let result = serde_json::to_value(result).unwrap();
            assert_ne!(result["isError"], true);
            assert_eq!(
                result["content"][0]["text"],
                json!({"type":null}).to_string()
            );
            task.await.unwrap();
        }
        let args: TypeArguments = serde_json::from_value(json!({"name":"Type"})).unwrap();
        assert!(args.repo.is_none());
    }

    #[test]
    fn types_arguments_apply_defaults_and_reject_unknown_or_invalid_types() {
        let args: TypesArguments = serde_json::from_value(json!({})).unwrap();
        assert!(args.repo.is_none());
        assert!(args.summary);
        assert_eq!(args.limit, 50);
        assert_eq!(args.offset, 0);
        for value in [
            json!({"scope":"own"}),
            json!({"summary":null}),
            json!({"offset":-1}),
            json!({"limit":1.5}),
        ] {
            assert!(serde_json::from_value::<TypesArguments>(value).is_err());
        }
        let router = Server::tool_router();
        assert_eq!(
            router
                .get("tb_types")
                .unwrap()
                .input_schema
                .get("additionalProperties"),
            Some(&json!(false))
        );
    }

    #[test]
    fn hubs_arguments_apply_defaults_and_reject_invalid_types() {
        let args: HubsArguments = serde_json::from_value(json!({})).unwrap();
        assert!(args.repo.is_none());
        assert!(args.scope.is_none());
        assert_eq!(args.limit, 20);
        assert_eq!(args.offset, 0);
        for value in [
            json!({"scope":"other"}),
            json!({"offset":-1}),
            json!({"limit":1.5}),
            json!({"path":"/tmp"}),
        ] {
            assert!(serde_json::from_value::<HubsArguments>(value).is_err());
        }
        for scope in ["own", "all"] {
            assert!(serde_json::from_value::<HubsArguments>(json!({"scope":scope})).is_ok());
        }
    }

    #[tokio::test]
    async fn hubs_requires_array_and_preserves_envelope() {
        for (payload, valid) in [
            (json!([]), true),
            (json!([{"path":"/note.md"}]), true),
            (Value::Null, false),
            (json!({}), false),
            (json!("bad"), false),
        ] {
            let result = json!({"hubs":payload,"extra_result":123});
            let frame = json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":1,"result":result});
            let actual = fake_response_for(serde_json::to_vec(&frame).unwrap(), None, "hubs").await;
            if valid {
                assert_eq!(
                    actual.unwrap(),
                    ReadResult {
                        version: 1,
                        payload: result
                    }
                );
            } else {
                assert_eq!(actual.unwrap_err(), "invalid read result payload");
            }
        }
    }

    #[tokio::test]
    async fn search_requires_an_object_and_preserves_envelope() {
        // `search` was advertised before the engine learned the verb, so no test ever saw a
        // payload: the validator's match fell through to `_ => false` and every call failed with
        // "invalid read result payload" the moment the engine started answering. This pins the arm.
        for (payload, valid) in [
            (
                json!({"exact":false,"matches":[],"total":0,"truncated":false}),
                true,
            ),
            (json!({}), true),
            (json!([]), false),
            (Value::Null, false),
            (json!("bad"), false),
        ] {
            let result = json!({"search":payload,"extra_result":123});
            let frame = json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":1,"result":result});
            let actual =
                fake_response_for(serde_json::to_vec(&frame).unwrap(), None, "search").await;
            if valid {
                assert_eq!(
                    actual.unwrap(),
                    ReadResult {
                        version: 1,
                        payload: result
                    }
                );
            } else {
                assert_eq!(actual.unwrap_err(), "invalid read result payload");
            }
        }
    }

    #[tokio::test]
    async fn types_requires_array_or_null_and_preserves_envelope() {
        for (payload, valid) in [
            (json!([]), true),
            (json!([{"name":"A"}]), true),
            (Value::Null, true),
            (json!({}), false),
            (json!("bad"), false),
        ] {
            let result = json!({"types":payload,"extra_result":123});
            let frame = json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":1,"result":result});
            let actual =
                fake_response_for(serde_json::to_vec(&frame).unwrap(), None, "types").await;
            if valid {
                assert_eq!(
                    actual.unwrap(),
                    ReadResult {
                        version: 1,
                        payload: result
                    }
                );
            } else {
                assert_eq!(actual.unwrap_err(), "invalid read result payload");
            }
        }
    }

    #[tokio::test]
    async fn type_tree_requires_roots_and_nodes_arrays_and_preserves_envelope() {
        let valid = json!({"roots":["A"],"nodes":[{"name":"A"}],"extra":true});
        let payloads = [
            (valid.clone(), true),
            (json!({"roots":[],"nodes":[]}), true),
            (json!({}), false),
            (json!({"roots":[]}), false),
            (json!({"nodes":[]}), false),
            (json!({"roots":null,"nodes":[]}), false),
            (json!({"roots":[],"nodes":{}}), false),
            (json!([]), false),
            (Value::Null, false),
        ];
        for (payload, valid) in payloads {
            let result = json!({"type_tree":payload,"extra_result":123});
            let frame = json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":1,"result":result});
            let actual =
                fake_response_for(serde_json::to_vec(&frame).unwrap(), None, "type_tree").await;
            if valid {
                assert_eq!(
                    actual.unwrap(),
                    ReadResult {
                        version: 1,
                        payload: result
                    }
                );
            } else {
                assert_eq!(actual.unwrap_err(), "invalid read result payload");
            }
        }
    }

    #[tokio::test]
    async fn tool_response_metadata_comes_only_from_validated_engine_response() {
        for (version, valid) in [(json!(u64::MAX), true), (json!(-1), false)] {
            let temp = tempfile::tempdir().unwrap();
            let path = temp.path().join("fake.sock");
            let listener = UnixListener::bind(&path).unwrap();
            let payload = json!({"types":[{"name":"A"}],"extra_result":123});
            let frame = json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":version,"result":payload});
            let task = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let len = socket.read_u32().await.unwrap();
                let mut request = vec![0; len as usize];
                socket.read_exact(&mut request).await.unwrap();
                let body = serde_json::to_vec(&frame).unwrap();
                socket.write_u32(body.len() as u32).await.unwrap();
                socket.write_all(&body).await.unwrap();
            });
            let result = test_server(path)
                .tb_types(
                    Parameters(TypesArguments {
                        repo: None,
                        summary: true,
                        limit: 50,
                        offset: 0,
                    }),
                    request_context(),
                )
                .await;
            let result = serde_json::to_value(result).unwrap();
            task.await.unwrap();
            if valid {
                assert_eq!(
                    result["_meta"]["io.2brain/engineVersion"],
                    u64::MAX.to_string()
                );
                assert_eq!(result["content"][0]["text"], payload.to_string());
                // The same answer is offered machine-readably, so a client
                // need not parse the text block.
                assert_eq!(result["structuredContent"], payload);
            } else {
                assert_eq!(result["isError"], true);
                assert!(result.get("_meta").is_none());
            }
        }
    }

    #[tokio::test]
    async fn cancelled_before_read_does_not_connect() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("fake.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let ctx = request_context();
        ctx.ct.cancel();
        let result = test_server(path)
            .tb_overview(Parameters(EmptyArguments {}), ctx)
            .await;
        assert_eq!(serde_json::to_value(result).unwrap()["isError"], true);
        assert!(
            tokio::time::timeout(Duration::from_millis(50), listener.accept())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn cancelled_read_closes_socket_without_retry() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("fake.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let ctx = request_context();
        let cancel = ctx.ct.clone();
        let task = tokio::spawn(async move {
            test_server(path)
                .tb_overview(Parameters(EmptyArguments {}), ctx)
                .await
        });
        let (mut socket, _) = tokio::time::timeout(Duration::from_secs(1), listener.accept())
            .await
            .unwrap()
            .unwrap();
        let len = socket.read_u32().await.unwrap();
        let mut request = vec![0; len as usize];
        socket.read_exact(&mut request).await.unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&request).unwrap(),
            json!({"read":"overview","id":1})
        );
        cancel.cancel();
        let mut byte = [0];
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), socket.read(&mut byte))
                .await
                .unwrap()
                .unwrap(),
            0
        );
        let result = tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(serde_json::to_value(result).unwrap()["isError"], true);
        assert!(
            tokio::time::timeout(Duration::from_millis(50), listener.accept())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn stalled_engine_returns_tool_error_without_retry() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("fake.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let len = socket.read_u32().await.unwrap();
            let mut request = vec![0; len as usize];
            socket.read_exact(&mut request).await.unwrap();
            let mut byte = [0];
            assert_eq!(socket.read(&mut byte).await.unwrap(), 0);
            assert!(
                tokio::time::timeout(Duration::from_millis(30), listener.accept())
                    .await
                    .is_err()
            );
        });
        let server = test_server(path);
        let result = server
            .read_with_timeout(json!({"id": 1}), "tb_overview", Duration::from_millis(50))
            .await;
        let result = serde_json::to_value(result).unwrap();
        assert_eq!(result["isError"], true);
        assert!(result.get("_meta").is_none());
        let error = result["content"][0]["text"].as_str().unwrap();
        assert!(
            error.contains("overview") && error.contains("timed out") && error.contains("no retry"),
            "{error}"
        );
        tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn oversized_request_is_rejected_before_connecting() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("missing.sock");
        let request = json!({"read":"type", "id":1, "name":"a".repeat(MAX_FRAME_BYTES)});
        assert_eq!(
            read_engine(&UnixTransport::new(path), request)
                .await
                .unwrap_err(),
            "request frame too large"
        );
    }

    #[tokio::test]
    async fn escaped_oversized_request_does_not_connect() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("fake.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let name = "\"".repeat(MAX_FRAME_BYTES / 2);
        assert!(name.len() < MAX_FRAME_BYTES);
        let request = json!({"read":"type", "id":1, "name":name});
        assert_eq!(
            read_engine(&UnixTransport::new(path), request)
                .await
                .unwrap_err(),
            "request frame too large"
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(30), listener.accept())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn rejects_invalid_wire_frames() {
        let good = json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":1,"result":{"overview":{}}});
        let cases = [
            (
                json!({"type":"response","schema_version":28,"id":1}),
                "schema_version",
            ),
            (json!({"type":"response","id":1}), "schema_version"),
            (
                json!({"type":"response","schema_version":29,"id":2}),
                "id mismatch",
            ),
            (
                json!({"type":"error","schema_version":29,"id":1,"for":"read","error":"bad request"}),
                "engine error: bad request",
            ),
            (
                // Any correlated error frame is a rejection, whatever `for` says.
                json!({"type":"error","schema_version":29,"id":1,"for":"mutate","error":"bad"}),
                "engine error: bad",
            ),
            (
                // An error frame without a message is not a usable answer.
                json!({"type":"error","schema_version":29,"id":1}),
                "invalid engine error frame",
            ),
            (
                json!({"type":"response","schema_version":29,"id":1,"ready":false}),
                "not ready",
            ),
            (
                json!({"type":"response","schema_version":29,"id":1,"ready":true,"result":{"overview":{}}}),
                "version",
            ),
            (
                json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":1,"result":null}),
                "envelope",
            ),
            (
                json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":1,"result":{"overview":null}}),
                "payload",
            ),
            (
                json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":1,"result":{}}),
                "payload",
            ),
            (
                json!({"type":"ack","schema_version":29,"id":1}),
                "frame type",
            ),
        ];
        for (frame, expected) in cases {
            let result = fake_response(serde_json::to_vec(&frame).unwrap(), None)
                .await
                .unwrap_err();
            assert!(result.contains(expected), "{frame}: {result}");
        }
        assert!(
            fake_response(b"{".to_vec(), None)
                .await
                .unwrap_err()
                .contains("JSON")
        );
        assert!(
            fake_response(Vec::new(), Some((MAX_FRAME_BYTES + 1) as u32))
                .await
                .unwrap_err()
                .contains("too large")
        );
        assert!(
            fake_response(b"{}".to_vec(), Some(12))
                .await
                .unwrap_err()
                .contains("reading response")
        );
        assert_eq!(
            fake_response(serde_json::to_vec(&good).unwrap(), None)
                .await
                .unwrap(),
            ReadResult {
                version: 1,
                payload: json!({"overview":{}})
            }
        );
    }

    #[tokio::test]
    async fn a_memory_transport_answers_one_read_and_sees_the_request_the_adapter_sent() {
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorder = Arc::clone(&seen);
        let server = scripted_server(move |request: &Value| {
            recorder.lock().unwrap().push(request.clone());
            Ok(json!({
                "type": "response",
                "schema_version": SCHEMA_VERSION,
                "id": request.get("id").cloned().unwrap_or(Value::Null),
                "ready": true,
                "version": 7,
                "result": {
                    "overview": {"members": [], "type_counts": {}, "diagnostic_counts": {"total": 0}}
                },
            }))
        });
        let result = server
            .tb_overview(Parameters(EmptyArguments {}), request_context())
            .await;
        let result = serde_json::to_value(result).unwrap();
        assert_ne!(result["isError"], true, "{result}");
        assert_eq!(
            result["structuredContent"]["overview"]["members"],
            json!([])
        );
        assert_eq!(
            result["_meta"]["io.2brain/engineVersion"], "7",
            "the engine version rides the answer metadata"
        );
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1, "one read is one exchange: {seen:?}");
        assert_eq!(seen[0]["read"], "overview", "unexpected verb");
        assert_eq!(seen[0]["id"], 1, "unexpected request id");
    }

    #[tokio::test]
    async fn a_broken_frame_is_a_tool_error_and_is_not_retried() {
        for (frame, expected) in [
            (
                json!({"type": "response", "schema_version": SCHEMA_VERSION - 1, "id": 1}),
                "schema_version",
            ),
            (
                json!({"type": "error", "schema_version": SCHEMA_VERSION, "id": 1, "error": "bad request"}),
                "engine error: bad request",
            ),
        ] {
            let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let counter = Arc::clone(&calls);
            let answer = frame.clone();
            let server = scripted_server(move |_request: &Value| {
                counter.fetch_add(1, Ordering::AcqRel);
                Ok(answer.clone())
            });
            let result = server
                .tb_overview(Parameters(EmptyArguments {}), request_context())
                .await;
            let result = serde_json::to_value(result).unwrap();
            assert_eq!(result["isError"], true, "{frame}: {result}");
            let text = result["content"][0]["text"].as_str().unwrap();
            assert!(text.contains(expected), "{frame}: {text}");
            assert_eq!(
                calls.load(Ordering::Acquire),
                1,
                "{frame}: a failed exchange must not be retried"
            );
        }
    }
}
