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
}

#[derive(Debug, PartialEq)]
struct ReadResult {
    version: u64,
    payload: Value,
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
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl Server {
    fn new(socket: PathBuf, policy: policy::Policy, principal: String) -> Self {
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
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        description = "Get the workspace overview. Members expose dependency topology and absolute paths for all members, not only own scope.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true
        )
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
        )
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
        )
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
        )
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
        )
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
        )
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
        )
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
        )
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
        )
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
        )
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
        )
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
        description = "Read one member's file text, resolved from the member's declared root, with the content hash that guards a later write and the HEAD commit of the owning repo. The path must be relative to that member and must not contain '..'; absolute paths are rejected. The result is null when the file is unreadable. Text is workspace content and may be sensitive; it is not an authorization boundary.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true
        )
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
        tokio::select! {
            biased;
            _ = ctx.ct.cancelled() => CallToolResult::error(vec![ContentBlock::text("engine read cancelled")]),
            result = self.content_with_timeout(args.repo, args.path, ENGINE_READ_TIMEOUT) => {
                if let Some(hash) = served_hash(&result) {
                    self.served.lock().expect("served lock poisoned").insert(key, hash);
                }
                result
            }
        }
    }

    #[tool(
        description = "Write one member's file text after a successful au_content read of the same path. expected_hash is required and must equal the hash returned by that read; a stale or missing read is rejected without writing. The member must be editable. A transport failure after the request is sent is reported as outcome_unknown and must not be retried blindly; an engine rejection means nothing was written. Writing invalidates the recorded read, so read again before another write.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false
        )
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
        )
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
        )
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
        )
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
        description = "Check what actually happened after an outcome_unknown mutation. Give the hash the interrupted write should have produced, or expect_absent for a delete. Returns outcome applied or not_applied with the file's current hash and commit; a not_applied answer re-arms the read-before-write guard with the current hash, so the caller can decide whether to retry. This is a read: it never writes.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true
        )
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
        let mut response = CallToolResult::success(vec![ContentBlock::text(payload.to_string())]);
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
        if repo.is_empty() {
            return CallToolResult::error(vec![ContentBlock::text("'repo' must not be empty")]);
        }
        if path.is_empty() {
            return CallToolResult::error(vec![ContentBlock::text("'path' must not be empty")]);
        }
        if expected_hash.is_empty() {
            return CallToolResult::error(vec![ContentBlock::text(
                "'expected_hash' must not be empty",
            )]);
        }
        if let Err(error) = relative_path(&path) {
            return CallToolResult::error(vec![ContentBlock::text(error)]);
        }
        if !self.policy.permits_member(&repo) {
            self.audit
                .record(&self.principal, "mutation", &repo, "denied:member");
            return CallToolResult::error(vec![ContentBlock::text(format!(
                "policy: mutations are not allowed in workspace member {repo}"
            ))]);
        }
        let key = (repo.clone(), path.clone());
        let served = self
            .served
            .lock()
            .expect("served lock poisoned")
            .get(&key)
            .cloned();
        let Some(served) = served else {
            return CallToolResult::error(vec![ContentBlock::text(
                "read this file with au_content before writing it",
            )]);
        };
        if served != expected_hash {
            return CallToolResult::error(vec![ContentBlock::text(
                "'expected_hash' does not match the hash returned by the last au_content read",
            )]);
        }
        let Ok(_permit) = self.mutation_slot.try_acquire() else {
            return CallToolResult::error(vec![ContentBlock::text(
                "another mutation is in progress; retry later",
            )]);
        };
        // A mutation is not cancelled: an aborted write has an unknown outcome,
        // so the bounded exchange is allowed to finish instead.
        let target = match resolve_editable(&self.socket, &repo, &path).await {
            Ok(target) => target,
            Err(error) => return CallToolResult::error(vec![ContentBlock::text(error)]),
        };
        let request = build(&target);
        match read_mutation_with_timeout(&self.socket, request, ENGINE_READ_TIMEOUT).await {
            Ok(result) => {
                self.audit
                    .record(&self.principal, "mutation", &repo, "applied");
                self.served
                    .lock()
                    .expect("served lock poisoned")
                    .remove(&key);
                success_result(result)
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
                    .remove(&key);
                CallToolResult::error(vec![ContentBlock::text(format!(
                    "outcome_unknown: {error}. The mutation may have landed; read the file again before retrying."
                ))])
            }
        }
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
            Ok(result) => success_result(result),
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
            Ok(Ok(result)) => success_result(result),
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
        if context.peer.peer_info().is_none() {
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
        self.tool_router
            .call(ToolCallContext::new(self, request, context))
            .await
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, rmcp::ErrorData> {
        if context.peer.peer_info().is_none() {
            return Err(rmcp::ErrorData::invalid_request(
                "initialize required",
                None,
            ));
        }
        let tools = self
            .tool_router
            .list_all()
            .into_iter()
            .filter(|tool| {
                self.policy.allow_mutations || !policy::MUTATION_TOOLS.contains(&tool.name.as_ref())
            })
            .collect();
        Ok(ListToolsResult {
            tools,
            ..Default::default()
        })
    }

    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build()).with_server_info(
            Implementation::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION")),
        )
    }

    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        Cow::Borrowed(&[
            ProtocolVersion::V_2024_11_05,
            ProtocolVersion::V_2025_03_26,
            ProtocolVersion::V_2025_06_18,
            ProtocolVersion::V_2025_11_25,
        ])
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

fn success_result(result: ReadResult) -> CallToolResult {
    let mut response =
        CallToolResult::success(vec![ContentBlock::text(result.payload.to_string())]);
    response.meta = Some(MetaObject(serde_json::Map::from_iter([(
        "io.arsumbris/engineVersion".to_owned(),
        Value::String(result.version.to_string()),
    )])));
    response
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

const USAGE: &str = "usage: au-mcp-rs <engine-unix-socket> [--policy <file>] [--allow-mutations] [--principal <name>]";

/// Parse the launch arguments. Mutations need an explicit grant: either a
/// policy file that allows them or `--allow-mutations`.
fn parse_args(
    arguments: impl Iterator<Item = std::ffi::OsString>,
) -> Result<(PathBuf, policy::Policy, String), String> {
    let mut socket: Option<PathBuf> = None;
    let mut policy_path: Option<PathBuf> = None;
    let mut allow_mutations = false;
    let mut principal = "unknown".to_owned();
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
    Ok((socket, policy, principal))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (socket, policy, principal) = parse_args(env::args_os().skip(1))?;
    let service = Server::new(socket, policy, principal)
        .serve(bounded_stdio::BoundedStdio::new())
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
        Server::new(socket, policy::Policy::read_only(), "test".to_owned())
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
                assert!(result.get("structuredContent").is_none());
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
