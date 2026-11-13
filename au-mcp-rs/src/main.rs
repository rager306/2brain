mod bounded_stdio;
mod policy;

use std::{
    borrow::Cow,
    collections::HashMap,
    env,
    path::{Path, PathBuf},
    sync::Mutex,
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
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
        InitializeRequestParams, InitializeResult, ListToolsResult, MetaObject,
        PaginatedRequestParams, ProtocolVersion, ServerCapabilities, ServerConfig,
    },
    service::RequestContext,
    tool, tool_handler, tool_router,
};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixStream,
    sync::Semaphore,
};

const SCHEMA_VERSION: u64 = 29;

/// The protocol revisions this server answers. `2026-07-28` is the inline
/// lifecycle: a request carries its version in `_meta` and needs no handshake.
const SUPPORTED_PROTOCOL_VERSIONS: &[ProtocolVersion] = &[
    ProtocolVersion::V_2024_11_05,
    ProtocolVersion::V_2025_03_26,
    ProtocolVersion::V_2025_06_18,
    ProtocolVersion::V_2025_11_25,
    ProtocolVersion::V_2026_07_28,
];
const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;
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

#[derive(Debug, PartialEq)]
struct ReadResult {
    version: u64,
    payload: Value,
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

/// How much of each tool definition the server advertises up front.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolProfile {
    /// Full descriptions, schemas, and output schemas for every tool.
    Full,
    /// One line per tool, no output schema; `au_describe_tool` serves the rest.
    Concise,
}

#[derive(Debug, Clone)]
struct Server {
    socket: PathBuf,
    policy: policy::Policy,
    principal: String,
    audit: policy::Audit,
    initialize_called: Arc<AtomicBool>,
    read_slots: Arc<Semaphore>,
    mutation_slot: Arc<Semaphore>,
    served: Arc<Mutex<HashMap<(String, String), String>>>,
    profile: ToolProfile,
    payload: PayloadMode,
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl Server {
    fn new(
        socket: PathBuf,
        policy: policy::Policy,
        principal: String,
        profile: ToolProfile,
        payload: PayloadMode,
    ) -> Self {
        let audit = policy::Audit::new(&policy.audit);
        Self {
            socket,
            policy,
            principal,
            audit,
            initialize_called: Arc::new(AtomicBool::new(false)),
            read_slots: Arc::new(Semaphore::new(MAX_IN_FLIGHT_READS)),
            mutation_slot: Arc::new(Semaphore::new(1)),
            served: Arc::new(Mutex::new(HashMap::new())),
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
        output_schema = envelope_schema("overview"),
    )]
    async fn au_overview(
        &self,
        _arguments: Parameters<EmptyArguments>,
        ctx: RequestContext<RoleServer>,
    ) -> CallToolResult {
        self.read(json!({"read": "overview", "id": 1}), "overview", ctx)
            .await
    }

    #[tool(
        description = "List top-level directories in own scope.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true
        ),
        output_schema = envelope_schema("top_level_dirs"),
    )]
    async fn au_top_level_dirs(
        &self,
        _arguments: Parameters<EmptyArguments>,
        ctx: RequestContext<RoleServer>,
    ) -> CallToolResult {
        self.read(
            json!({"read": "top_level_dirs", "id": 1}),
            "top_level_dirs",
            ctx,
        )
        .await
    }

    #[tool(
        description = "Get the complete, unpaged type tree for the whole workspace, including dependencies.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true
        ),
        output_schema = envelope_schema("type_tree"),
    )]
    async fn au_type_tree(
        &self,
        _arguments: Parameters<EmptyArguments>,
        ctx: RequestContext<RoleServer>,
    ) -> CallToolResult {
        self.read(json!({"read": "type_tree", "id": 1}), "type_tree", ctx)
            .await
    }

    #[tool(
        description = "Get one type definition by name, across the workspace or in a named member. An unknown type or member returns null.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true
        ),
        output_schema = envelope_schema("type"),
    )]
    async fn au_type(
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
        self.read(json!({"read": "type", "id": 1, "name": name}), "type", ctx)
            .await
    }

    #[tool(
        description = "List a name-sorted page of types across the workspace or in a named member. This experimental interface differs from the existing MCP tool: it uses summary instead of detail, returns the engine envelope without counts, and rejects limits outside 1..=500 instead of clamping. Defaults: summary=true, limit=50, offset=0. A short page indicates the end; no total is returned. An unknown member returns null, distinct from an empty array. Offset pagination is not a snapshot: graph changes between requests can cause duplicates or omissions. The _meta engineVersion is a diagnostic-only decimal string; without daemon identity, equal versions do not establish the same snapshot, and metadata may be dropped.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true
        ),
        output_schema = envelope_schema("types"),
    )]
    async fn au_types(
        &self,
        Parameters(args): Parameters<TypesArguments>,
        ctx: RequestContext<RoleServer>,
    ) -> CallToolResult {
        if !(1..=500).contains(&args.limit) {
            return CallToolResult::error(vec![ContentBlock::text(
                "'limit' must be between 1 and 500",
            )]);
        }
        let mut request = json!({"read": "types", "id": 1, "summary": args.summary, "limit": args.limit, "offset": args.offset});
        if let Some(repo) = args.repo {
            request["repo"] = json!(repo);
        }
        self.read(request, "types", ctx).await
    }

    #[tool(
        description = "List a ranked page of hubs. Scope and repo filter targets only; inbound counts include all incoming edges, including sources outside the selected scope. Scope defaults to own, or all when repo is supplied. An unknown repo returns an empty list. Paths may be absolute. Defaults: limit=20, offset=0; limits outside 1..=500 are rejected. Offset pagination is not a snapshot: graph changes can cause duplicates or omissions. The _meta engineVersion is diagnostic only and does not establish a snapshot.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true
        ),
        output_schema = envelope_schema("hubs"),
    )]
    async fn au_hubs(
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
        let mut request =
            json!({"read": "hubs", "id": 1, "limit": args.limit, "offset": args.offset});
        if let Some(repo) = args.repo {
            request["repo"] = json!(repo);
        }
        if let Some(scope) = args.scope {
            request["scope"] = json!(match scope {
                HubScope::Own => "own",
                HubScope::All => "all",
            });
        }
        self.read(request, "hubs", ctx).await
    }

    #[tool(
        description = "List a page of diagnostics. Filters compose with AND. Scope defaults to own, or all when repo is supplied; an empty page says nothing about other scopes. Paths, messages and suggested actions may reveal workspace data; suggested actions are not authorization to mutate. Defaults: limit=50, offset=0, limit range 1..=500. A short page signals the end; pages are not a snapshot and changes may cause omissions or duplicates.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true
        ),
        output_schema = envelope_schema("diagnostics"),
    )]
    async fn au_diagnostics(
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
        let mut request =
            json!({"read": "diagnostics", "id": 1, "limit": args.limit, "offset": args.offset});
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
        self.read(request, "diagnostics", ctx).await
    }

    #[tool(
        description = "List all direct and transitive subtypes of a base type. A bare base matches by name across members; name::repo selects that base identity. Scope filters the member holding each subtype, and defaults to all. Unknown bases return an empty array. Identical definitions in different members remain separate. Results are unpaged and may contain paths and workspace content; frames over 16 MiB fail.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true
        ),
        output_schema = envelope_schema("subtypes"),
    )]
    async fn au_subtypes(
        &self,
        Parameters(args): Parameters<SubtypesArguments>,
        ctx: RequestContext<RoleServer>,
    ) -> CallToolResult {
        if args.base.is_empty() {
            return CallToolResult::error(vec![ContentBlock::text("'base' must not be empty")]);
        }
        let mut request = json!({"read": "subtypes", "id": 1, "base": args.base});
        if let Some(scope) = args.scope {
            request["scope"] = json!(match scope {
                HubScope::Own => "own",
                HubScope::All => "all",
            });
        }
        self.read(request, "subtypes", ctx).await
    }

    #[tool(
        description = "List workspace members, optionally selecting an exact repo. Unknown repos return an empty array. The result includes dependencies and disabled members, their roles and local/editable flags, absolute roots and Git topology. It is unpaged and may reveal workspace topology; it is not an authorization boundary.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true
        ),
        output_schema = envelope_schema("members"),
    )]
    async fn au_members(
        &self,
        Parameters(args): Parameters<MembersArguments>,
        ctx: RequestContext<RoleServer>,
    ) -> CallToolResult {
        if args.repo.as_deref() == Some("") {
            return CallToolResult::error(vec![ContentBlock::text("'repo' must not be empty")]);
        }
        let mut request = json!({"read": "members", "id": 1});
        if let Some(repo) = args.repo {
            request["repo"] = json!(repo);
        }
        self.read(request, "members", ctx).await
    }

    #[tool(
        description = "List resolved cross-repo type imports. Scope filters the importing members: all by default, or own to exclude other importers and the automatic engine builtin fold. Referenced field shapes, unresolved imports, and dependencies without type use are excluded. The unpaged result may reveal repo and type names; it is not an authorization boundary.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true
        ),
        output_schema = envelope_schema("imports"),
    )]
    async fn au_imports(
        &self,
        Parameters(args): Parameters<ImportsArguments>,
        ctx: RequestContext<RoleServer>,
    ) -> CallToolResult {
        let mut request = json!({"read": "imports", "id": 1});
        if let Some(scope) = args.scope {
            request["scope"] = json!(match scope {
                HubScope::Own => "own",
                HubScope::All => "all",
            });
        }
        self.read(request, "imports", ctx).await
    }

    #[tool(
        description = "List a page of catalogued files, including assets and dependencies by default. Filters repo and scope compose; scope defaults to all. Paths may be absolute and stems are not unique. Order is by path components, not plain string order. Defaults: limit=50, offset=0, limit range 1..=500. Pages are not a snapshot; concurrent changes may cause omissions or duplicates. This tool is not an authorization boundary.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true
        ),
        output_schema = envelope_schema("files"),
    )]
    async fn au_files(
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
        let mut request = json!({"read":"files","id":1,"limit":args.limit,"offset":args.offset});
        if let Some(repo) = args.repo {
            request["repo"] = json!(repo);
        }
        if let Some(scope) = args.scope {
            request["scope"] = json!(match scope {
                HubScope::Own => "own",
                HubScope::All => "all",
            });
        }
        self.read(request, "files", ctx).await
    }

    #[tool(
        description = "Read one member's file text, resolved from the member's declared root, with the content hash that guards a later write and the HEAD commit of the owning repo. The path must be relative to that member and must not contain '..'; absolute paths are rejected. max_chars returns a window with truncated, total_chars, and next_offset; the hash still covers the whole file, so a write based on it stays valid. The result is null when the file is unreadable. Text is workspace content and may be sensitive; it is not an authorization boundary.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true
        ),
        output_schema = envelope_schema("content"),
    )]
    async fn au_content(
        &self,
        Parameters(args): Parameters<ContentArguments>,
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
        if ctx.ct.is_cancelled() {
            return CallToolResult::error(vec![ContentBlock::text("engine read cancelled")]);
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
                    self.served.lock().expect("served lock poisoned").insert(key, hash);
                }
                match max_chars {
                    Some(max) => window_content(result, max, self.payload),
                    None => result,
                }
            }
        }
    }

    #[tool(
        description = "Write one member's file text after a successful au_content read of the same path. expected_hash is required and must equal the hash returned by that read; a stale or missing read is rejected without writing. The member must be editable. A transport failure after the request is sent is reported as outcome_unknown and must not be retried blindly; an engine rejection means nothing was written. Writing invalidates the recorded read, so read again before another write.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false
        ),
        output_schema = mutation_schema(),
    )]
    async fn au_write_file(
        &self,
        Parameters(args): Parameters<WriteArguments>,
        _ctx: RequestContext<RoleServer>,
    ) -> CallToolResult {
        let expected_hash = args.expected_hash.clone();
        self.guarded_mutation(args.repo, args.path, args.expected_hash, move |target| {
            json!({
                "mutate": "write_file",
                "id": 1,
                "path": target,
                "content": args.content,
                "expected_hash": expected_hash,
            })
        })
        .await
    }

    #[tool(
        description = "Replace one exact string in a member's file after a successful au_content read of the same path. expected_hash must equal the hash that read returned. old_string must match exactly and uniquely unless replace_all is true; the engine rejects a miss or a non-unique match without writing. Unknown outcomes are reported as outcome_unknown and must not be retried blindly.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false
        ),
        output_schema = mutation_schema(),
    )]
    async fn au_edit_file(
        &self,
        Parameters(args): Parameters<EditArguments>,
        _ctx: RequestContext<RoleServer>,
    ) -> CallToolResult {
        self.guarded_mutation(args.repo, args.path, args.expected_hash, |target| {
            json!({
                "mutate": "edit_file",
                "id": 1,
                "path": target,
                "old_string": args.old_string,
                "new_string": args.new_string,
                "replace_all": args.replace_all,
            })
        })
        .await
    }

    #[tool(
        description = "Delete one member's file after a successful au_content read of the same path. expected_hash must equal the hash that read returned. The engine rejects an absent file and refuses to delete a path with uncommitted changes. Unknown outcomes are reported as outcome_unknown and must not be retried blindly.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false
        ),
        output_schema = mutation_schema(),
    )]
    async fn au_delete_file(
        &self,
        Parameters(args): Parameters<DeleteArguments>,
        _ctx: RequestContext<RoleServer>,
    ) -> CallToolResult {
        let expected_hash = args.expected_hash.clone();
        self.guarded_mutation(args.repo, args.path, args.expected_hash, move |target| {
            json!({
                "mutate": "delete_file",
                "id": 1,
                "path": target,
                "expected_hash": expected_hash,
            })
        })
        .await
    }

    #[tool(
        description = "Preview a write, edit, or delete without changing anything. Returns the engine's would-be target, hash, diagnostics, and blast radius so a caller can inspect consequences before mutating. The member must be editable and the path relative to it. No expected_hash is required because nothing is written. This is a read: it is not an authorization decision.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true
        ),
        output_schema = envelope_schema("preview_mutation"),
    )]
    async fn au_preview_mutation(
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
        let target = match resolve_editable(&self.socket, &args.repo, &args.path).await {
            Ok(target) => target,
            Err(error) => return CallToolResult::error(vec![ContentBlock::text(error)]),
        };
        let mut request = json!({
            "read": "preview_mutation",
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
            result = self.read_with_timeout(request, "preview_mutation", ENGINE_READ_TIMEOUT) => result,
        }
    }

    #[tool(
        description = "Find tools by words from their name, description, or parameter names. Returns the best matches; detail selects names only, names with a one-line summary (default), or full definitions with schemas. Use it when you know the task but not the tool name.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true
        ),
        output_schema = envelope_schema("tools"),
    )]
    async fn au_search_tools(
        &self,
        Parameters(args): Parameters<SearchToolsArguments>,
        _ctx: RequestContext<RoleServer>,
    ) -> CallToolResult {
        let query = args.query.trim();
        if query.is_empty() {
            return CallToolResult::error(vec![ContentBlock::text("'query' must not be empty")]);
        }
        let matches = search_tools(&self.visible_tools(), query, args.limit);
        let tools: Vec<Value> = matches
            .into_iter()
            .map(|tool| match args.detail {
                SearchDetail::Name => json!({"name": tool.name}),
                SearchDetail::Summary => {
                    json!({"name": tool.name, "description": first_sentence(tool.description.as_deref())})
                }
                SearchDetail::Schema => serde_json::to_value(&tool).unwrap_or(Value::Null),
            })
            .collect();
        success_json(json!({"tools": tools}), self.payload)
    }

    #[tool(
        description = "Get one tool's full definition: description, input and output schemas, and annotations. Use it after au_search_tools, or when a tool's one-line description is not enough.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true
        ),
        output_schema = envelope_schema("tool"),
    )]
    async fn au_describe_tool(
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
                "unknown tool '{}'; call au_search_tools or tools/list to see what this launch exposes",
                args.name
            ))]);
        };
        success_json(
            serde_json::to_value(&tool).unwrap_or(Value::Null),
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
        output_schema = envelope_schema("instances_of"),
    )]
    async fn au_instances_of(
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
            "read": "instances_of",
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
        let result = self.read(request, "instances_of", ctx).await;
        match (args.limit, args.offset) {
            (None, None) => result,
            (limit, offset) => page_instances(result, limit, offset.unwrap_or(0), self.payload),
        }
    }

    #[tool(
        description = "Validate a value against a type without touching disk. Returns one verdict per identity the name denotes, each with diagnostics and advisory undeclared fields; an unknown type yields a null identity carrying unknown-type-claim rather than an empty result. Values and diagnostics may reveal workspace vocabulary.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true
        ),
        output_schema = envelope_schema("validate_value"),
    )]
    async fn au_validate_value(
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
            "read": "validate_value",
            "id": 1,
            "type_name": args.type_name,
            "value": args.value,
        });
        if let Some(repo) = args.repo {
            request["repo"] = json!(repo);
        }
        self.read(request, "validate_value", ctx).await
    }

    #[tool(
        description = "Check what actually happened after an outcome_unknown mutation. Give the hash the interrupted write should have produced, or expect_absent for a delete. Returns outcome applied or not_applied with the file's current hash and commit; a not_applied answer re-arms the read-before-write guard with the current hash, so the caller can decide whether to retry. This is a read: it never writes.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true
        ),
        output_schema = envelope_schema("reconcile"),
    )]
    async fn au_reconcile(
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
            (Some(hash), false) if !hash.is_empty() => Some(hash.to_owned()),
            (None, true) => None,
            _ => {
                return CallToolResult::error(vec![ContentBlock::text(
                    "give exactly one of a nonempty 'expected_hash' or 'expect_absent': true",
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
                read_content(&self.socket, &args.repo, &args.path),
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
        let mut served = self.served.lock().expect("served lock poisoned");
        if applied {
            served.remove(&key);
        } else if let Some(hash) = current_hash.clone() {
            // A differing file is a fresh, coherent read: arm the guard so a
            // deliberate retry does not need a second call.
            served.insert(key, hash);
        } else {
            served.remove(&key);
        }
        drop(served);
        let payload = json!({
            "outcome": if applied { "applied" } else { "not_applied" },
            "current_hash": current_hash,
            "current_commit": current_commit,
        });
        let mut response = success_payload(payload, self.payload);
        response.meta = Some(MetaObject(serde_json::Map::from_iter([(
            "io.arsumbris/engineVersion".to_owned(),
            Value::String(result.version.to_string()),
        )])));
        response
    }

    /// Shared mutation policy: a recorded read of this exact path must exist,
    /// the hash must match, the member must be editable, one mutation runs at a
    /// time, and a lost answer is `outcome_unknown` rather than a retry.
    async fn guarded_mutation(
        &self,
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
                    "mutation",
                    &repo,
                    &format!("denied:{}", denial.stage.name()),
                );
                return CallToolResult::error(vec![ContentBlock::text(format!(
                    "policy/{}: {}",
                    denial.stage.name(),
                    denial.message
                ))]);
            }
        };
        let request = build(&prepared.target);
        match read_mutation_with_timeout(&self.socket, request, ENGINE_READ_TIMEOUT).await {
            Ok(result) => {
                self.audit
                    .record(&self.principal, "mutation", &repo, "applied");
                self.served
                    .lock()
                    .expect("served lock poisoned")
                    .remove(&prepared.key);
                success_result(result, self.payload)
            }
            Err(MutationFailure::Rejected(error)) => {
                self.audit
                    .record(&self.principal, "mutation", &repo, "rejected");
                CallToolResult::error(vec![ContentBlock::text(error)])
            }
            Err(MutationFailure::Unknown(error)) => {
                self.audit
                    .record(&self.principal, "mutation", &repo, "outcome_unknown");
                self.served
                    .lock()
                    .expect("served lock poisoned")
                    .remove(&prepared.key);
                CallToolResult::error(vec![ContentBlock::text(format!(
                    "outcome_unknown: {error}. The mutation may have landed; read the file again before retrying."
                ))])
            }
        }
    }

    /// Run the mutation gate in `policy::MUTATION_STAGES` order, stopping at the
    /// first denial. The returned value holds the slot permit and the resolved
    /// target, so a caller that gets one may proceed to the engine.
    async fn prepare_mutation(
        &self,
        repo: &str,
        path: &str,
        expected_hash: &str,
    ) -> Result<PreparedMutation, Denial> {
        use policy::Stage;

        let key = (repo.to_owned(), path.to_owned());
        let mut served: Option<String> = None;
        let mut permit: Option<tokio::sync::OwnedSemaphorePermit> = None;
        let mut target: Option<String> = None;

        // The order comes from `policy::MUTATION_STAGES`, so the documented gate
        // and the executed gate cannot drift apart.
        for stage in policy::MUTATION_STAGES {
            match stage {
                Stage::Arguments => {
                    if repo.is_empty() {
                        return Err(Denial::new(stage, "'repo' must not be empty"));
                    }
                    if path.is_empty() {
                        return Err(Denial::new(stage, "'path' must not be empty"));
                    }
                    if expected_hash.is_empty() {
                        return Err(Denial::new(stage, "'expected_hash' must not be empty"));
                    }
                }
                Stage::PathScope => {
                    if let Err(error) = relative_path(path) {
                        return Err(Denial::new(stage, error));
                    }
                }
                Stage::MemberGrant => {
                    if !self.policy.permits_member(repo) {
                        return Err(Denial::new(
                            stage,
                            format!("mutations are not allowed in workspace member {repo}"),
                        ));
                    }
                }
                Stage::PriorRead => {
                    let recorded = self
                        .served
                        .lock()
                        .expect("served lock poisoned")
                        .get(&key)
                        .cloned();
                    let Some(recorded) = recorded else {
                        return Err(Denial::new(
                            stage,
                            "read this file with au_content before writing it",
                        ));
                    };
                    served = Some(recorded);
                }
                Stage::Freshness => {
                    if served.as_deref() != Some(expected_hash) {
                        return Err(Denial::new(
                            stage,
                            "'expected_hash' does not match the hash returned by the last au_content read",
                        ));
                    }
                }
                Stage::Capacity => {
                    let Ok(held) = self.mutation_slot.clone().try_acquire_owned() else {
                        return Err(Denial::new(
                            stage,
                            "another mutation is in progress; retry later",
                        ));
                    };
                    permit = Some(held);
                }
                Stage::Editability => {
                    // A mutation is not cancelled: an aborted write has an
                    // unknown outcome, so the bounded exchange finishes instead.
                    match resolve_editable(&self.socket, repo, path).await {
                        Ok(resolved) => target = Some(resolved),
                        Err(error) => return Err(Denial::new(stage, error)),
                    }
                }
            }
        }

        match (permit, target) {
            (Some(permit), Some(target)) => Ok(PreparedMutation {
                key,
                target,
                _permit: permit,
            }),
            // Unreachable while MUTATION_STAGES keeps both later stages, but a
            // total return beats a panic in a server.
            _ => Err(Denial::new(
                Stage::Arguments,
                "internal: the mutation gate did not complete",
            )),
        }
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
            "{text}.{hint} Call au_describe_tool {{\"name\":\"{name}\"}} for the full schema."
        ))];
        CallToolResponse::Complete(result)
    }

    /// The tools this launch may advertise, before any profile compacting.
    fn visible_tools(&self) -> Vec<rmcp::model::Tool> {
        self.tool_router
            .list_all()
            .into_iter()
            .filter(|tool| {
                self.policy.allow_mutations || !policy::MUTATION_TOOLS.contains(&tool.name.as_ref())
            })
            .collect()
    }

    async fn read(
        &self,
        request: Value,
        verb: &str,
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
            result = self.read_with_timeout(request, verb, ENGINE_READ_TIMEOUT) => result,
        }
    }

    async fn read_with_timeout(
        &self,
        request: Value,
        verb: &str,
        timeout: Duration,
    ) -> CallToolResult {
        match read_engine_with_timeout(&self.socket, request, verb, timeout).await {
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
        match tokio::time::timeout(timeout, read_content(&self.socket, &repo, &relative)).await {
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
            // back from au_describe_tool, so discovery stays cheap.
            for tool in &mut tools {
                tool.description = Some(first_sentence(tool.description.as_deref()).into());
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

    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(
                env!("CARGO_PKG_NAME"),
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(INSTRUCTIONS)
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
    /// Hash returned by the au_content read that this write is based on.
    #[schemars(length(min = 1))]
    expected_hash: String,
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
    /// Hash returned by the au_content read that this edit is based on.
    #[schemars(length(min = 1))]
    expected_hash: String,
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
    /// Hash returned by the au_content read that this delete is based on.
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

/// A mutation that passed every gate: it holds the single mutation slot and the
/// resolved absolute target.
struct PreparedMutation {
    key: (String, String),
    target: String,
    _permit: tokio::sync::OwnedSemaphorePermit,
}

/// The first gate that refused a mutation.
struct Denial {
    stage: policy::Stage,
    message: String,
}

impl Denial {
    fn new(stage: policy::Stage, message: impl Into<String>) -> Self {
        Self {
            stage,
            message: message.into(),
        }
    }
}

/// What a client should know before choosing a tool.
const INSTRUCTIONS: &str = "Read-only access to an arsumbris workspace, plus gated mutations. Every result is the engine's own result object under the read's key: au_overview answers {\"overview\": ...}, au_type answers {\"type\": ...}. Start with au_overview, then au_members for member roots, au_types and au_type for vocabulary, and au_files or au_content to locate text. Paged tools take limit and offset; au_type_tree, au_content, au_instances_of, and au_validate_value are unpaged and can return large answers. Mutations are disabled unless the operator granted them at launch; when enabled they need a prior au_content read of the same path and the hash it returned. A lost reply is outcome_unknown and must not be retried blindly; au_reconcile reports whether it landed.";

#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct SearchToolsArguments {
    /// Words describing what you want to do.
    #[schemars(length(min = 1))]
    query: String,
    /// How much to return per match; default summary.
    #[serde(default)]
    detail: SearchDetail,
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

#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct DescribeToolArguments {
    /// Tool name, exactly as tools/list reports it.
    #[schemars(length(min = 1))]
    name: String,
}

/// Rank tools for a query: a term in the name weighs most, then a parameter
/// name, then the description. Deterministic, and cheap enough to run per call.
fn search_tools(tools: &[rmcp::model::Tool], query: &str, limit: usize) -> Vec<rmcp::model::Tool> {
    let terms: Vec<String> = tokenize(query);
    if terms.is_empty() {
        return Vec::new();
    }
    let mut scored: Vec<(u32, &rmcp::model::Tool)> = tools
        .iter()
        .filter_map(|tool| {
            let name = tokenize(tool.name.as_ref());
            let text = tokenize(tool.description.as_deref().unwrap_or(""));
            let params: Vec<String> = tool
                .input_schema
                .get("properties")
                .and_then(Value::as_object)
                .map(|properties| properties.keys().flat_map(|key| tokenize(key)).collect())
                .unwrap_or_default();
            let mut score = 0;
            for term in &terms {
                if name.contains(term) {
                    score += 3;
                }
                if params.contains(term) {
                    score += 2;
                }
                if text.iter().any(|word| word == term) {
                    score += 1;
                }
                if text.iter().any(|word| word.starts_with(term.as_str())) {
                    score += 1;
                }
            }
            (score > 0).then_some((score, tool))
        })
        .collect();
    scored.sort_by(|(left_score, left), (right_score, right)| {
        right_score
            .cmp(left_score)
            .then_with(|| left.name.cmp(&right.name))
    });
    scored
        .into_iter()
        .take(limit)
        .map(|(_, tool)| tool.clone())
        .collect()
}

/// Lowercase word tokens; underscores and dots split so `au_content` matches
/// "content" and `io.arsumbris` matches "arsumbris".
fn tokenize(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The output schema of a read: the engine's result object with its own key.
fn envelope_schema(verb: &'static str) -> Arc<serde_json::Map<String, Value>> {
    let mut properties = serde_json::Map::new();
    properties.insert(
        verb.to_owned(),
        json!({"description": format!("the {verb} read's result")}),
    );
    // Paging metadata the adapter may add to any read's answer. A client
    // validates `structuredContent` against this schema, so a field the adapter
    // adds without declaring it here is a protocol error at the client, not a
    // cosmetic mismatch.
    for key in ["total", "offset", "next_offset"] {
        properties.insert(
            key.to_owned(),
            json!({"type": "integer", "description": format!("paging metadata added by the adapter ({key})")}),
        );
    }
    Arc::new(
        json!({
            "type": "object",
            "properties": properties,
            "required": [verb],
            "additionalProperties": false,
        })
        .as_object()
        .expect("object schema")
        .clone(),
    )
}

/// The output schema shared by every mutation: the engine's mutation result.
fn mutation_schema() -> Arc<serde_json::Map<String, Value>> {
    Arc::new(
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "absolute path that was written"},
                "hash": {"type": "string", "description": "content hash after the mutation"},
                "reflected": {"type": "boolean"},
                "commit": {},
                "commits": {"type": "array"},
                "diagnostics": {"type": "array"},
                "untracked_files": {"type": "array"}
            },
            "required": ["path"],
            "additionalProperties": true,
        })
        .as_object()
        .expect("object schema")
        .clone(),
    )
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

/// The content hash reported by a successful `au_content` result.
fn served_hash(result: &CallToolResult) -> Option<String> {
    let text = result.content.first()?.as_text()?.text.clone();
    let payload: Value = serde_json::from_str(&text).ok()?;
    payload
        .get("content")?
        .get("hash")?
        .as_str()
        .map(str::to_owned)
}

/// Resolve a member-relative path to an absolute one, refusing members the
/// engine does not author.
async fn resolve_editable(socket: &Path, repo: &str, relative: &str) -> Result<String, String> {
    let members = read_engine(
        socket,
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

enum MutationFailure {
    /// The engine rejected the mutation: nothing was written.
    Rejected(String),
    /// No usable answer: the outcome is unknown and a retry could double-apply.
    Unknown(String),
}

async fn read_mutation_with_timeout(
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

fn success_result(result: ReadResult, mode: PayloadMode) -> CallToolResult {
    let version = result.version;
    let mut response = success_payload(result.payload, mode);
    response.meta = Some(MetaObject(serde_json::Map::from_iter([(
        "io.arsumbris/engineVersion".to_owned(),
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
    let Some(records) = payload
        .get("instances_of")
        .and_then(Value::as_array)
        .cloned()
    else {
        return result;
    };
    let total = records.len();
    let page: Vec<Value> = records
        .into_iter()
        .skip(offset)
        .take(limit.unwrap_or(usize::MAX))
        .collect();
    let returned = page.len();
    payload["instances_of"] = Value::Array(page);
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

/// The first sentence of a description, so a concise profile costs one line.
fn first_sentence(description: Option<&str>) -> String {
    let text = description.unwrap_or("").trim();
    let end = text.find(". ").filter(|cut| *cut < 140).map_or_else(
        || text.chars().take(140).collect::<String>(),
        |cut| text[..=cut].to_owned(),
    );
    end.trim().to_owned()
}

/// Reject a member-relative path that could escape the member root.
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

/// Read one member's file text by joining its declared root with a
/// client-supplied relative path. The path is validated by the caller.
async fn read_content(socket: &Path, repo: &str, relative: &str) -> Result<ReadResult, String> {
    let members = read_engine(
        socket,
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
    let path = Path::new(root).join(relative);
    let path = path
        .to_str()
        .ok_or("resolved path is not valid UTF-8")?
        .to_owned();
    read_engine(
        socket,
        json!({"read": "content", "id": 1, "path": path}),
        "content",
    )
    .await
}

async fn read_engine_with_timeout(
    socket: &Path,
    request: Value,
    verb: &str,
    timeout: Duration,
) -> Result<ReadResult, String> {
    tokio::time::timeout(timeout, read_engine(socket, request, verb))
        .await
        .map_err(|_| format!("engine read '{verb}' timed out after {timeout:?} (connect, write, or response); no retry was attempted"))?
}

/// A framed engine exchange that failed, split by what it proves about a
/// mutation: an engine rejection wrote nothing, while a transport failure may
/// have landed.
enum WireError {
    /// The engine answered with a rejection: nothing was written.
    Rejected(String),
    /// No usable answer: the operation may or may not have happened.
    Broken(String),
}

impl WireError {
    fn message(self) -> String {
        match self {
            Self::Rejected(message) | Self::Broken(message) => message,
        }
    }
}

async fn exchange(socket: &Path, request: &Value) -> Result<Value, WireError> {
    let data = serde_json::to_vec(request)
        .map_err(|e| WireError::Broken(format!("encoding request: {e}")))?;
    if data.len() > MAX_FRAME_BYTES {
        return Err(WireError::Broken("request frame too large".into()));
    }
    let frame_len = u32::try_from(data.len())
        .map_err(|_| WireError::Broken("request frame too large".into()))?;
    let mut stream = UnixStream::connect(socket)
        .await
        .map_err(|e| WireError::Broken(format!("connecting to engine: {e}")))?;
    stream
        .write_u32(frame_len)
        .await
        .map_err(|e| WireError::Broken(format!("writing request length: {e}")))?;
    stream
        .write_all(&data)
        .await
        .map_err(|e| WireError::Broken(format!("writing request: {e}")))?;
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
    let response: Value = serde_json::from_slice(&data)
        .map_err(|e| WireError::Broken(format!("invalid response JSON: {e}")))?;
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

async fn read_engine(socket: &Path, request: Value, verb: &str) -> Result<ReadResult, String> {
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

/// Send one mutation. A rejection is definitive; anything else means the
/// mutation may have landed, so a caller must never retry it blindly.
async fn mutate_engine(socket: &Path, request: Value) -> Result<ReadResult, WireError> {
    let response = exchange(socket, &request).await?;
    let version = response["version"]
        .as_u64()
        .ok_or_else(|| WireError::Broken("missing or invalid engine version".into()))?;
    let result = response["result"]
        .as_object()
        .ok_or_else(|| WireError::Broken("invalid result envelope".into()))?;
    if !result.get("path").is_some_and(Value::is_string) {
        return Err(WireError::Broken("invalid mutation result payload".into()));
    }
    Ok(ReadResult {
        version,
        payload: Value::Object(result.clone()),
    })
}

const USAGE: &str = "usage: au-mcp-rs <engine-unix-socket> [--policy <file>] [--allow-mutations] [--principal <name>] [--tools full|concise] [--payload both|text|structured]";

/// Parse the launch arguments. Mutations need an explicit grant: either a
/// policy file that allows them or `--allow-mutations`.
fn parse_args(
    arguments: impl Iterator<Item = std::ffi::OsString>,
) -> Result<(PathBuf, policy::Policy, String, ToolProfile, PayloadMode), String> {
    let mut socket: Option<PathBuf> = None;
    let mut policy_path: Option<PathBuf> = None;
    let mut allow_mutations = false;
    let mut principal = "unknown".to_owned();
    let mut profile = ToolProfile::Full;
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
    Ok((socket, policy, principal, profile, payload))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (socket, policy, principal, profile, payload) = parse_args(env::args_os().skip(1))?;
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
        let result = read_engine(&path, request, verb).await;
        task.await.unwrap();
        result
    }

    #[test]
    fn empty_arguments_reject_unknown_fields() {
        assert!(serde_json::from_value::<EmptyArguments>(json!({})).is_ok());
        assert!(serde_json::from_value::<EmptyArguments>(json!({"scope": "other"})).is_err());
        let router = Server::tool_router();
        for name in ["au_overview", "au_top_level_dirs", "au_type_tree"] {
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
        let schema = &router.get("au_type").unwrap().input_schema;
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
                .au_type(
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
                .au_type(Parameters(args), request_context())
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
                .get("au_types")
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
                .au_types(
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
                    result["_meta"]["io.arsumbris/engineVersion"],
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
            .au_overview(Parameters(EmptyArguments {}), ctx)
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
                .au_overview(Parameters(EmptyArguments {}), ctx)
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
            .read_with_timeout(
                json!({"read":"overview","id":1}),
                "overview",
                Duration::from_millis(50),
            )
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
            read_engine(&path, request, "type").await.unwrap_err(),
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
            read_engine(&path, request, "type").await.unwrap_err(),
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
}
