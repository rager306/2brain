//! Driving adapter: MCP 2.0 (`2026-07-28` stateless core + legacy handshake),
//! SEP-2640 Skills (`skills/list`, `skills/get`, `resources/directory/read`),
//! 38/54 progressive tool surface, and fail-closed SEM-02 write gate (`ReadLedger`).

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use rmcp::model::{
    CacheScope, CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock,
    GetPromptRequestParams, GetPromptResponse, GetPromptResult, GetSkillRequestParams,
    GetSkillResult, Implementation, ListPromptsResult, ListResourcesResult, ListSkillsResult,
    ListToolsResult, PaginatedRequestParams, Prompt, PromptArgument, PromptMessage,
    ProtocolVersion, ReadResourceDirectoryRequestParams, ReadResourceDirectoryResult,
    ReadResourceRequestParams, ReadResourceResponse, ReadResourceResult, Resource,
    ResourceContents, Role, ServerCapabilities, ServerConfig, Skill, SkillFrontmatter,
    SkillResource, SkillResources, Tool,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler, ServiceExt};
use serde_json::{Map, Value, json};
use tb_app::{App, AppError, ContentQuery, WorkspaceStore};
use tb_contracts::Locator;
use tb_skills::SkillCatalog;
use thiserror::Error;

/// Protocol versions supported: MCP 2.0 stateless (`2026-07-28`) plus legacy handshakes.
const SUPPORTED_PROTOCOL_VERSIONS: &[ProtocolVersion] = &[
    ProtocolVersion::V_2024_11_05,
    ProtocolVersion::V_2025_03_26,
    ProtocolVersion::V_2025_06_18,
    ProtocolVersion::V_2025_11_25,
    ProtocolVersion::V_2026_07_28,
];

const SERVER_INSTRUCTIONS: &str = "Read-only access to a 2brain v2 workspace (MIT), plus gated mutations and SEP-2640 skills. Start with tb_overview, tb_members, tb_types, or skills/list. Partial tb_content reads never arm writes (SEM-02); tb_search is BM25+proximity and tb_grep is literal substring search (SEM-09).";

#[derive(Debug, Error)]
pub enum McpServerError {
    #[error("MCP transport error: {0}")]
    Transport(String),
}

/// Launch-time capability grants (fail-closed by default).
#[derive(Clone, Copy, Debug, Default)]
pub struct LaunchPolicy {
    pub allow_mutations: bool,
    pub allow_composition: bool,
    pub allow_workspace_shape: bool,
}

#[derive(Clone, Debug)]
struct ArmedRead {
    short_hash: String,
    sha256: String,
}

/// Process-local ledger of complete, non-truncated `tb_content` reads (SEM-02).
#[derive(Default)]
struct ReadLedger {
    armed: HashMap<(String, String), ArmedRead>,
}

impl ReadLedger {
    fn record_whole_read(&mut self, repo: &str, path: &str, short_hash: String, sha256: String) {
        self.armed.insert(
            (repo.to_owned(), path.to_owned()),
            ArmedRead { short_hash, sha256 },
        );
    }

    fn verify_write(&self, repo: &str, path: &str, expected_hash: &str) -> Result<(), String> {
        let Some(armed) = self.armed.get(&(repo.to_owned(), path.to_owned())) else {
            return Err(
                "policy/prior_read: read this file whole with tb_content before writing it (partial reads do not arm writes)".to_owned(),
            );
        };
        if expected_hash != armed.short_hash && !expected_hash.eq_ignore_ascii_case(&armed.sha256) {
            return Err(format!(
                "policy/freshness: expected_hash '{}' does not match the last whole read '{}'",
                expected_hash, armed.short_hash
            ));
        }
        Ok(())
    }
}

/// Thin driving MCP adapter over [`App<W>`] + [`SkillCatalog`].
#[derive(Clone)]
pub struct TwoBrainMcp<W: WorkspaceStore + 'static> {
    app: Arc<App<W>>,
    policy: LaunchPolicy,
    ledger: Arc<Mutex<ReadLedger>>,
    tools: Arc<Vec<Tool>>,
}

impl<W: WorkspaceStore + 'static> TwoBrainMcp<W> {
    #[must_use]
    pub fn new(app: Arc<App<W>>, policy: LaunchPolicy) -> Self {
        let tools = Arc::new(build_tool_catalog(policy));
        Self {
            app,
            policy,
            ledger: Arc::new(Mutex::new(ReadLedger::default())),
            tools,
        }
    }

    /// Serve MCP over stdio until EOF.
    pub async fn serve_stdio(self) -> Result<(), McpServerError> {
        let running = self
            .serve(rmcp::transport::stdio())
            .await
            .map_err(|e| McpServerError::Transport(e.to_string()))?;
        running
            .waiting()
            .await
            .map_err(|e| McpServerError::Transport(e.to_string()))?;
        Ok(())
    }

    fn skill_catalog(&self) -> SkillCatalog {
        let snap = self.app.snapshot();
        SkillCatalog::from_snapshot(&snap)
    }

    fn default_repo(&self) -> Option<String> {
        self.app.snapshot().members.keys().next().cloned()
    }

    fn dispatch_tool(&self, name: &str, args: &Map<String, Value>) -> CallToolResult {
        match name {
            "tb_overview" => ok_json(self.app.overview(opt_str(args, "repo"))),
            "tb_members" => ok_json(self.app.members()),
            "tb_resolve_member" => match req_str(args, "path") {
                Ok(p) => ok_json(self.app.resolve_member(p)),
                Err(e) => err_text(e),
            },
            "tb_top_level_dirs" => ok_json(self.app.top_level_dirs(opt_str(args, "repo"))),
            "tb_files" => {
                let limit = opt_usize(args, "limit", 50).clamp(1, 500);
                let offset = opt_usize(args, "offset", 0);
                ok_json(self.app.files(
                    opt_str(args, "repo"),
                    opt_str(args, "prefix"),
                    limit,
                    offset,
                ))
            }
            "tb_content" => self.handle_content(args),
            "tb_frontmatter" => match self.parse_locator(args) {
                Ok(loc) => map_app(self.app.frontmatter(&loc)),
                Err(e) => err_text(e),
            },
            "tb_anchors" => match self.parse_locator(args) {
                Ok(loc) => map_app(self.app.anchors(&loc)),
                Err(e) => err_text(e),
            },
            "tb_block_ids" => match self.parse_locator(args) {
                Ok(loc) => map_app(self.app.block_ids(&loc)),
                Err(e) => err_text(e),
            },
            "tb_semantic_tokens" => match self.parse_locator(args) {
                Ok(loc) => map_app(self.app.semantic_tokens(&loc)),
                Err(e) => err_text(e),
            },
            "tb_types" => ok_json(self.app.types(opt_str(args, "repo"))),
            "tb_type" => {
                let repo = opt_str(args, "repo")
                    .map(str::to_owned)
                    .or_else(|| self.default_repo());
                match (repo, req_str(args, "name")) {
                    (Some(r), Ok(n)) => map_app(self.app.type_def(&r, n)),
                    (None, _) => err_text("no workspace member available"),
                    (_, Err(e)) => err_text(e),
                }
            }
            "tb_type_tree" => ok_json(self.app.type_tree(opt_str(args, "repo"))),
            "tb_subtypes" => {
                let repo = opt_str(args, "repo")
                    .map(str::to_owned)
                    .or_else(|| self.default_repo());
                match (repo, req_str(args, "name")) {
                    (Some(r), Ok(n)) => ok_json(self.app.subtypes(&r, n)),
                    (None, _) => err_text("no workspace member available"),
                    (_, Err(e)) => err_text(e),
                }
            }
            "tb_typed" | "tb_instances" => {
                let limit = opt_usize(args, "limit", 50).clamp(1, 500);
                let offset = opt_usize(args, "offset", 0);
                ok_json(self.app.instances(opt_str(args, "repo"), limit, offset))
            }
            "tb_instances_of" => {
                let limit = opt_usize(args, "limit", 50).clamp(1, 500);
                let offset = opt_usize(args, "offset", 0);
                match req_str(args, "type").or_else(|_| req_str(args, "name")) {
                    Ok(t) => {
                        ok_json(
                            self.app
                                .instances_of(opt_str(args, "repo"), t, limit, offset),
                        )
                    }
                    Err(e) => err_text(e),
                }
            }
            "tb_candidates" => {
                let limit = opt_usize(args, "limit", 50).clamp(1, 500);
                let offset = opt_usize(args, "offset", 0);
                let repo = opt_str(args, "repo")
                    .map(str::to_owned)
                    .or_else(|| self.default_repo());
                match (
                    repo,
                    req_str(args, "type").or_else(|_| req_str(args, "name")),
                ) {
                    (Some(r), Ok(t)) => map_app(self.app.candidates(&r, t, limit, offset)),
                    (None, _) => err_text("no workspace member available"),
                    (_, Err(e)) => err_text(e),
                }
            }
            "tb_validate_value" => {
                let repo = opt_str(args, "repo")
                    .map(str::to_owned)
                    .or_else(|| self.default_repo());
                match (
                    repo,
                    req_str(args, "type"),
                    req_str(args, "field"),
                    args.get("value"),
                ) {
                    (Some(r), Ok(t), Ok(f), Some(v)) => {
                        map_app(self.app.validate_value(&r, t, f, v))
                    }
                    (None, _, _, _) => err_text("no workspace member available"),
                    (_, Err(e), _, _) | (_, _, Err(e), _) => err_text(e),
                    (_, _, _, None) => err_text("'value' is required"),
                }
            }
            "tb_diagnostics" => {
                let limit = opt_usize(args, "limit", 50).clamp(1, 500);
                let offset = opt_usize(args, "offset", 0);
                ok_json(self.app.diagnostics(
                    opt_str(args, "repo"),
                    opt_str(args, "path"),
                    limit,
                    offset,
                ))
            }
            "tb_search" => match req_str(args, "query") {
                Ok(q) => {
                    let limit = opt_usize(args, "limit", 20).clamp(1, 200);
                    let offset = opt_usize(args, "offset", 0);
                    let exact = args.get("exact").and_then(Value::as_bool).unwrap_or(false);
                    let msm = args
                        .get("min_should_match")
                        .and_then(Value::as_u64)
                        .map(|n| n as usize);
                    ok_json(
                        self.app
                            .search(q, opt_str(args, "repo"), exact, msm, limit, offset),
                    )
                }
                Err(e) => err_text(e),
            },
            "tb_grep" => match req_str(args, "query") {
                Ok(q) => {
                    let limit = opt_usize(args, "limit", 20).clamp(1, 200);
                    let offset = opt_usize(args, "offset", 0);
                    let cs = args
                        .get("case_sensitive")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    ok_json(self.app.grep(q, opt_str(args, "repo"), cs, limit, offset))
                }
                Err(e) => err_text(e),
            },
            "tb_references" => match self.parse_locator(args) {
                Ok(loc) => map_app(self.app.references(&loc)),
                Err(e) => err_text(e),
            },
            "tb_follow" => {
                let repo = opt_str(args, "repo")
                    .map(str::to_owned)
                    .or_else(|| self.default_repo());
                match (repo, req_str(args, "target")) {
                    (Some(r), Ok(t)) => ok_json(self.app.follow(&r, t)),
                    (None, _) => err_text("no workspace member available"),
                    (_, Err(e)) => err_text(e),
                }
            }
            "tb_resolved" => match self.parse_locator(args) {
                Ok(loc) => map_app(self.app.resolved(&loc)),
                Err(e) => err_text(e),
            },
            "tb_children" => match self.parse_locator(args) {
                Ok(loc) => map_app(self.app.children(&loc)),
                Err(e) => err_text(e),
            },
            "tb_imports" => ok_json(self.app.imports(opt_str(args, "repo"))),
            "tb_hubs" => {
                let limit = opt_usize(args, "limit", 20).clamp(1, 200);
                ok_json(self.app.hubs(limit))
            }
            "tb_graph_health" => ok_json(self.app.graph_health(opt_str(args, "repo"))),
            "tb_neighborhood" => match self.parse_locator(args) {
                Ok(loc) => {
                    let depth = opt_usize(args, "depth", 1).clamp(1, 4) as u8;
                    let limit = opt_usize(args, "limit", 30).clamp(1, 200);
                    map_app(self.app.neighborhood(&loc, depth, limit))
                }
                Err(e) => err_text(e),
            },
            "tb_context" => match req_str(args, "query") {
                Ok(q) => {
                    let max_notes = opt_usize(args, "max_notes", 5);
                    let max_chars = opt_usize(args, "max_chars_per_note", 1200);
                    map_app(
                        self.app
                            .context(q, opt_str(args, "repo"), max_notes, max_chars),
                    )
                }
                Err(e) => err_text(e),
            },
            "tb_mutation_outcome" => match req_str(args, "request_id") {
                Ok(id) => ok_json(self.app.mutation_outcome(id)),
                Err(e) => err_text(e),
            },
            "tb_describe_tool" => match req_str(args, "name") {
                Ok(tool_name) => match self.tools.iter().find(|t| t.name.as_ref() == tool_name) {
                    Some(t) => ok_json(json!({ "describe_tool": t })),
                    None => err_text(format!("unknown tool '{tool_name}'")),
                },
                Err(e) => err_text(e),
            },
            "tb_search_tools" => match req_str(args, "query") {
                Ok(q) => {
                    let q_lower = q.to_lowercase();
                    let limit = opt_usize(args, "limit", 10).clamp(1, 54);
                    let matched: Vec<&Tool> = self
                        .tools
                        .iter()
                        .filter(|t| {
                            t.name.to_lowercase().contains(&q_lower)
                                || t.description
                                    .as_deref()
                                    .is_some_and(|d| d.to_lowercase().contains(&q_lower))
                        })
                        .take(limit)
                        .collect();
                    ok_json(json!({ "search_tools": matched }))
                }
                Err(e) => err_text(e),
            },
            "tb_preview_mutation" | "tb_reconcile" | "tb_rollback_preview" | "tb_watch" => {
                ok_json(json!({
                    name.trim_start_matches("tb_"): {
                        "status": "phase3_pending",
                        "generation": self.app.generation().0,
                    }
                }))
            }
            "tb_write_file" | "tb_edit_file" | "tb_delete_file" => {
                self.handle_guarded_write_gate(name, args)
            }
            "tb_create_file" | "tb_rename" | "tb_assign_block_id" | "tb_rename_block_id"
            | "tb_rename_type" | "tb_inline" | "tb_promote" | "tb_batch" => {
                if !self.policy.allow_mutations {
                    return err_text("policy/launch_grant: mutations are disabled");
                }
                err_text(format!(
                    "phase3_pending: '{name}' write execution is wired in Phase 3"
                ))
            }
            "tb_add_dep" | "tb_remove_dep" => {
                if !self.policy.allow_mutations || !self.policy.allow_composition {
                    return err_text(
                        "policy/launch_grant: composition mutations require --allow-mutations and --allow-composition",
                    );
                }
                err_text(format!(
                    "phase3_pending: '{name}' composition mutation is wired in Phase 3"
                ))
            }
            "tb_add_workspace_member"
            | "tb_remove_workspace_member"
            | "tb_set_workspace_member_role" => {
                if !self.policy.allow_mutations || !self.policy.allow_workspace_shape {
                    return err_text(
                        "policy/launch_grant: workspace-shape mutations require --allow-mutations and --allow-workspace-shape",
                    );
                }
                err_text(format!(
                    "phase3_pending: '{name}' workspace-shape mutation is wired in Phase 3"
                ))
            }
            _ => err_text(format!("unknown tool '{name}'")),
        }
    }

    fn handle_content(&self, args: &Map<String, Value>) -> CallToolResult {
        let loc = match self.parse_locator(args) {
            Ok(l) => l,
            Err(e) => return err_text(e),
        };
        let offset_chars = opt_usize(args, "offset_chars", 0);
        let max_chars = args
            .get("max_chars")
            .and_then(Value::as_u64)
            .map(|n| n as usize);
        let expected_sha256 = args
            .get("expected")
            .and_then(Value::as_object)
            .and_then(|o| o.get("sha256"))
            .and_then(Value::as_str)
            .map(str::to_owned);

        let req = ContentQuery {
            locator: loc.clone(),
            offset_chars,
            max_chars,
            expected_sha256,
        };
        match self.app.content(&req) {
            Ok(res) => {
                // SEM-02: ONLY a complete, non-truncated read arms the write ledger.
                if !res.truncated
                    && let Ok(mut ledger) = self.ledger.lock()
                {
                    ledger.record_whole_read(&loc.repo, &loc.path, res.hash, res.sha256);
                }
                ok_json(res.payload)
            }
            Err(err) => err_text(err.to_string()),
        }
    }

    fn handle_guarded_write_gate(&self, name: &str, args: &Map<String, Value>) -> CallToolResult {
        if !self.policy.allow_mutations {
            return err_text("policy/launch_grant: mutations are disabled");
        }
        let loc = match self.parse_locator(args) {
            Ok(l) => l,
            Err(e) => return err_text(e),
        };
        let expected_hash = match req_str(args, "expected_hash") {
            Ok(h) => h,
            Err(e) => return err_text(e),
        };
        if let Ok(ledger) = self.ledger.lock()
            && let Err(reason) = ledger.verify_write(&loc.repo, &loc.path, expected_hash)
        {
            return err_text(reason);
        }
        err_text(format!(
            "phase3_pending: '{name}' passed SEM-02/freshness gate; atomic write commit is wired in Phase 3"
        ))
    }

    fn parse_locator(&self, args: &Map<String, Value>) -> Result<Locator, String> {
        let repo = opt_str(args, "repo")
            .map(str::to_owned)
            .or_else(|| self.default_repo())
            .ok_or_else(|| "policy/arguments: 'repo' is required".to_owned())?;
        let path = req_str(args, "path")?;
        Locator::new(repo, path).map_err(|e| format!("policy/path_scope: {e}"))
    }
}

impl<W: WorkspaceStore + 'static> ServerHandler for TwoBrainMcp<W> {
    fn get_info(&self) -> ServerConfig {
        let capabilities = ServerCapabilities::builder()
            .enable_tools()
            .enable_prompts()
            .enable_resources()
            .enable_skills_with_directory_read()
            .build();
        ServerConfig::new(capabilities)
            .with_server_info(Implementation::new("2brain", env!("CARGO_PKG_VERSION")))
            .with_instructions(SERVER_INSTRUCTIONS)
    }

    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        Cow::Borrowed(SUPPORTED_PROTOCOL_VERSIONS)
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        Ok(ListToolsResult::with_all_items((*self.tools).clone()))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let empty = Map::new();
        let args = request.arguments.as_ref().unwrap_or(&empty);
        let result = self.dispatch_tool(request.name.as_ref(), args);
        Ok(CallToolResponse::Complete(result))
    }

    async fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, McpError> {
        let prompts = vec![
            Prompt::new(
                "recall",
                Some(
                    "Orient, search, widen, read whole, answer with citations and explicit uncertainty.",
                ),
                Some(vec![PromptArgument::new("question").with_required(true)]),
            ),
            Prompt::new(
                "intake",
                Some(
                    "Check existing notes, choose type honestly, write with full-read CAS, verify diagnostics.",
                ),
                Some(vec![PromptArgument::new("record").with_required(true)]),
            ),
            Prompt::new(
                "handoff",
                Some(
                    "Decisions, cited evidence paths, unverified items, and concrete next tool action.",
                ),
                Some(vec![PromptArgument::new("state").with_required(true)]),
            ),
        ];
        Ok(ListPromptsResult::with_all_items(prompts))
    }

    async fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<GetPromptResponse, McpError> {
        let text = match request.name.as_str() {
            "recall" => {
                "Orient via tb_overview/tb_types, search via tb_search/tb_grep, read whole via tb_content, answer with citations."
            }
            "intake" => {
                "Search existing notes, verify schema via tb_types/tb_type, write via full-read CAS, confirm with tb_diagnostics."
            }
            "handoff" => {
                "List decisions, evidence paths, unverified checks, and one concrete next tool action."
            }
            other => {
                return Err(McpError::invalid_params(
                    format!("unknown prompt '{other}'"),
                    None,
                ));
            }
        };
        Ok(GetPromptResponse::Complete(GetPromptResult::new(vec![
            PromptMessage::new_text(Role::User, text),
        ])))
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        let mut resources = vec![
            Resource::new("2brain://workflow/answer-shape", "Answer shape")
                .with_mime_type("text/markdown")
                .with_description("The six-part answer contract and honesty standard"),
            Resource::new("2brain://workflow/write-gates", "Write gates")
                .with_mime_type("text/markdown")
                .with_description("Fail-closed write gate stages and recovery rules"),
        ];
        let catalog = self.skill_catalog();
        for skill in catalog.list() {
            for f in &skill.files {
                resources.push(
                    Resource::new(&f.uri, &skill.name)
                        .with_mime_type(&f.mime_type)
                        .with_description(&skill.description),
                );
            }
        }
        Ok(ListResourcesResult::with_all_items(resources))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, McpError> {
        let uri = request.uri.as_str();
        if uri == "2brain://workflow/answer-shape" {
            return Ok(ReadResourceResponse::Complete(ReadResourceResult::new(
                vec![
                    ResourceContents::text(WORKFLOW_ANSWER_SHAPE, uri)
                        .with_mime_type("text/markdown"),
                ],
            )));
        }
        if uri == "2brain://workflow/write-gates" {
            return Ok(ReadResourceResponse::Complete(ReadResourceResult::new(
                vec![
                    ResourceContents::text(WORKFLOW_WRITE_GATES, uri)
                        .with_mime_type("text/markdown"),
                ],
            )));
        }
        if uri.starts_with("skill://") {
            let catalog = self.skill_catalog();
            if let Some(sf) = catalog.read_file(uri) {
                return Ok(ReadResourceResponse::Complete(ReadResourceResult::new(
                    vec![
                        ResourceContents::text(&sf.content, &sf.uri).with_mime_type(&sf.mime_type),
                    ],
                )));
            }
            return Err(McpError::resource_not_found(
                format!("unknown skill resource '{uri}'"),
                None,
            ));
        }
        Err(McpError::resource_not_found(
            format!("unknown resource '{uri}'"),
            None,
        ))
    }

    async fn list_skills(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListSkillsResult, McpError> {
        let catalog = self.skill_catalog();
        let skills: Vec<Skill> = catalog.list().map(to_rmcp_skill).collect();
        Ok(ListSkillsResult::with_all_items(skills))
    }

    async fn get_skill(
        &self,
        request: GetSkillRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<GetSkillResult, McpError> {
        let catalog = self.skill_catalog();
        let Some(entry) = catalog.get(&request.uri) else {
            return Err(McpError::resource_not_found(
                format!("unknown skill '{}'", request.uri),
                None,
            ));
        };
        Ok(GetSkillResult::new(to_rmcp_skill(entry))
            .with_ttl_ms(300_000)
            .with_cache_scope(CacheScope::Private))
    }

    async fn read_resource_directory(
        &self,
        request: ReadResourceDirectoryRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceDirectoryResult, McpError> {
        let catalog = self.skill_catalog();
        let Some(files) = catalog.read_directory(&request.uri) else {
            return Err(McpError::resource_not_found(
                format!("unknown skill directory '{}'", request.uri),
                None,
            ));
        };
        let resources: Vec<Resource> = files
            .iter()
            .map(|f| Resource::new(&f.uri, &f.relative_path).with_mime_type(&f.mime_type))
            .collect();
        Ok(ListResourcesResult::with_all_items(resources))
    }
}

fn to_rmcp_skill(entry: &tb_skills::SkillEntry) -> Skill {
    let frontmatter = SkillFrontmatter::new(&entry.name, &entry.description)
        .with_field("source", entry.source.clone());
    let resources = SkillResources::Files(
        entry
            .files
            .iter()
            .map(|f| SkillResource::new(&f.uri, &f.digest, f.size))
            .collect(),
    );
    Skill::new(&entry.uri, frontmatter, resources)
}

fn ok_json(value: Value) -> CallToolResult {
    let text = serde_json::to_string(&value).unwrap_or_else(|_| "{}".to_owned());
    let mut res = CallToolResult::success(vec![ContentBlock::text(text)]);
    res.structured_content = Some(value);
    res
}

fn err_text(msg: impl Into<String>) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(msg.into())])
}

fn map_app(res: Result<Value, AppError>) -> CallToolResult {
    match res {
        Ok(v) => ok_json(v),
        Err(e) => err_text(e.to_string()),
    }
}

fn opt_str<'a>(args: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
}

fn req_str<'a>(args: &'a Map<String, Value>, key: &str) -> Result<&'a str, String> {
    opt_str(args, key).ok_or_else(|| format!("policy/arguments: '{key}' is required"))
}

fn opt_usize(args: &Map<String, Value>, key: &str, default: usize) -> usize {
    args.get(key)
        .and_then(Value::as_u64)
        .map_or(default, |v| v as usize)
}

fn empty_object_schema() -> Arc<Map<String, Value>> {
    Arc::new(
        json!({ "type": "object", "properties": {} })
            .as_object()
            .expect("object")
            .clone(),
    )
}

fn make_tool(name: &'static str, description: &'static str) -> Tool {
    Tool::new(name, description, empty_object_schema())
}

/// Build the exact 38 read-only or 54 full tool catalogue based on launch policy.
fn build_tool_catalog(policy: LaunchPolicy) -> Vec<Tool> {
    let mut tools = vec![
        make_tool(
            "tb_anchors",
            "List heading and ^block-id anchors in a note.",
        ),
        make_tool("tb_block_ids", "List ^block-id anchors in a note."),
        make_tool(
            "tb_candidates",
            "Find untyped notes whose fields satisfy a type.",
        ),
        make_tool("tb_children", "List outgoing references of a note."),
        make_tool(
            "tb_content",
            "Read a note's exact UTF-8 content, SHA-256, and hash.",
        ),
        make_tool(
            "tb_context",
            "Assemble a bounded, revision-consistent evidence pack.",
        ),
        make_tool("tb_describe_tool", "Return the full schema of one tool."),
        make_tool(
            "tb_diagnostics",
            "List schema, UTF-8, and link diagnostics.",
        ),
        make_tool("tb_files", "List indexed markdown files with paging."),
        make_tool("tb_follow", "Resolve a wikilink target from a member."),
        make_tool(
            "tb_frontmatter",
            "Read parsed YAML frontmatter fields of a note.",
        ),
        make_tool(
            "tb_graph_health",
            "Report cycles, bridges, articulation points, and triangles.",
        ),
        make_tool(
            "tb_grep",
            "Literal substring search across notes (distinct from BM25).",
        ),
        make_tool(
            "tb_hubs",
            "Rank notes by inbound references (structural vs navigational).",
        ),
        make_tool("tb_imports", "List declared repository dependencies."),
        make_tool("tb_instances", "List typed notes across the workspace."),
        make_tool("tb_instances_of", "List notes declaring a specific type."),
        make_tool("tb_members", "List workspace members and their roots."),
        make_tool(
            "tb_mutation_outcome",
            "Query durable outcome state for a request_id.",
        ),
        make_tool(
            "tb_neighborhood",
            "Walk a bounded typed neighborhood around a note.",
        ),
        make_tool(
            "tb_overview",
            "Summarize corpus counts, diagnostics, and graph shape.",
        ),
        make_tool(
            "tb_preview_mutation",
            "Preview a planned mutation without applying it.",
        ),
        make_tool(
            "tb_reconcile",
            "Compare current file state after an outcome_unknown mutation.",
        ),
        make_tool(
            "tb_references",
            "List incoming and outgoing edges for a note.",
        ),
        make_tool(
            "tb_resolve_member",
            "Map an absolute path to its workspace member.",
        ),
        make_tool(
            "tb_resolved",
            "List wikilink resolution outcomes for a note.",
        ),
        make_tool(
            "tb_rollback_preview",
            "Preview paths affected by rolling back a commit.",
        ),
        make_tool(
            "tb_search",
            "Rank notes by BM25 + body proximity + rare-term title lift.",
        ),
        make_tool(
            "tb_search_tools",
            "Search available tools by name or description.",
        ),
        make_tool(
            "tb_semantic_tokens",
            "Return parsed wikilink spans for a note.",
        ),
        make_tool("tb_subtypes", "List direct subtypes of a type."),
        make_tool(
            "tb_top_level_dirs",
            "List top-level directories and file counts.",
        ),
        make_tool(
            "tb_type",
            "Inspect one type definition and its effective fields.",
        ),
        make_tool("tb_type_tree", "Inspect the type inheritance hierarchy."),
        make_tool("tb_typed", "List typed notes across the workspace."),
        make_tool("tb_types", "List all loaded type definitions."),
        make_tool(
            "tb_validate_value",
            "Validate a single field value against a type definition.",
        ),
        make_tool("tb_watch", "Wait for workspace snapshot changes."),
    ];

    if policy.allow_mutations {
        tools.extend([
            make_tool(
                "tb_assign_block_id",
                "Assign a ^block-id to a block in a note.",
            ),
            make_tool("tb_batch", "Apply a batch of mutations atomically."),
            make_tool(
                "tb_create_file",
                "Create a new note (refuses existing paths).",
            ),
            make_tool(
                "tb_delete_file",
                "Delete a note after full tb_content read and CAS check.",
            ),
            make_tool(
                "tb_edit_file",
                "Edit a note after full tb_content read and CAS check.",
            ),
            make_tool("tb_inline", "Inline a target note into a caller note."),
            make_tool("tb_promote", "Promote a block or record to its own note."),
            make_tool("tb_rename", "Rename a note and update inbound wikilinks."),
            make_tool(
                "tb_rename_block_id",
                "Rename a ^block-id and update references.",
            ),
            make_tool(
                "tb_rename_type",
                "Rename a type definition and update instances.",
            ),
            make_tool(
                "tb_write_file",
                "Replace a note after full tb_content read and CAS check.",
            ),
        ]);
        if policy.allow_composition {
            tools.extend([
                make_tool("tb_add_dep", "Add a repository dependency."),
                make_tool("tb_remove_dep", "Remove a repository dependency."),
            ]);
        }
        if policy.allow_workspace_shape {
            tools.extend([
                make_tool("tb_add_workspace_member", "Add a member to the workspace."),
                make_tool(
                    "tb_remove_workspace_member",
                    "Remove a member from the workspace.",
                ),
                make_tool(
                    "tb_set_workspace_member_role",
                    "Change a workspace member's role.",
                ),
            ]);
        }
    }

    tools
}

const WORKFLOW_ANSWER_SHAPE: &str = "# Answer shape\n\n1. Direct answer first.\n2. Backing cited by (repo, path) from full tb_content reads.\n3. Implications in reader vocabulary.\n4. Tensions stated explicitly.\n5. Concrete next explorations.\n6. Sources consulted with counts.\n";

const WORKFLOW_WRITE_GATES: &str = "# Write gates\n\nFail-closed order: launch_grant -> arguments -> path_scope -> member_grant -> prior_read (whole file only, SEM-02) -> freshness (expected_hash CAS).\n";

#[cfg(test)]
mod tests {
    use super::*;
    use tb_app::{StoreError, VaultSnapshot};
    use tb_contracts::{CasVerified, EvidenceReceipt, WriteIntent};

    struct DummyStore;
    impl WorkspaceStore for DummyStore {
        fn read_exact(&self, locator: &Locator) -> Result<(Vec<u8>, EvidenceReceipt), StoreError> {
            Err(StoreError::NotFound(locator.path.clone()))
        }
        fn commit_verified(&self, _intent: WriteIntent<CasVerified>) -> Result<String, StoreError> {
            Err(StoreError::Unsupported("dummy".into()))
        }
    }

    #[test]
    fn tool_surface_counts_match_38_readonly_and_54_full() {
        let ro = build_tool_catalog(LaunchPolicy::default());
        assert_eq!(ro.len(), 38);

        let full = build_tool_catalog(LaunchPolicy {
            allow_mutations: true,
            allow_composition: true,
            allow_workspace_shape: true,
        });
        assert_eq!(full.len(), 54);
    }

    #[test]
    fn rmcp_skills_pass_sep2640_protocol_validation() {
        let app = Arc::new(App::new(DummyStore, VaultSnapshot::empty()));
        let server = TwoBrainMcp::new(app, LaunchPolicy::default());
        let catalog = server.skill_catalog();
        let skills: Vec<Skill> = catalog.list().map(to_rmcp_skill).collect();
        rmcp::model::validate_skills(&skills)
            .expect("all built-in skills must pass SEP-2640 validate()");
        assert_eq!(skills.len(), 5);
    }
}
