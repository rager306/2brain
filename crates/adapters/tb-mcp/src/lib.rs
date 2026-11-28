//! Driving adapter: MCP 2.0 (`2026-07-28` stateless core + legacy handshake),
//! SEP-2640 Skills (`skills/list`, `skills/get`, `resources/directory/read`),
//! 38/54 progressive tool surface, stateful `ReadLedger` AND stateless HMAC-SHA256
//! `read_receipt` fail-closed SEM-02 write gate.

use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

pub mod http;
pub mod schemas;

pub use http::{BoundHttpServer, HttpServerConfig};

use rmcp::model::{
    CacheScope, CallToolRequestParams, CallToolResponse, CallToolResult, CancelTaskParams,
    ContentBlock, CreateTaskResult, ElicitRequest, ElicitRequestParams, ElicitationSchema,
    GetPromptRequestParams, GetPromptResponse, GetPromptResult, GetSkillRequestParams,
    GetSkillResult, GetTaskParams, GetTaskResult, Implementation, InputRequest, ListPromptsResult,
    ListResourcesResult, ListSkillsResult, ListToolsResult, PaginatedRequestParams, Prompt,
    PromptArgument, PromptMessage, ProtocolVersion, ReadResourceDirectoryRequestParams,
    ReadResourceDirectoryResult, ReadResourceRequestParams, ReadResourceResponse,
    ReadResourceResult, Resource, ResourceContents, Role, ServerCapabilities, ServerConfig, Skill,
    SkillFrontmatter, SkillResource, SkillResources, Tool, UpdateTaskParams,
};
use rmcp::service::RequestContext;
use rmcp::task_manager::{TaskExit, TaskManager, TaskOptions};
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler, ServiceExt};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use tb_app::{AppDispatcher, AppError, VaultSnapshot};
use tb_skills::SkillCatalog;
use thiserror::Error;
use tokio::sync::Semaphore;

/// Protocol versions supported: MCP 2.0 stateless (`2026-07-28`) plus legacy handshakes.
const SUPPORTED_PROTOCOL_VERSIONS: &[ProtocolVersion] = &[
    ProtocolVersion::V_2024_11_05,
    ProtocolVersion::V_2025_03_26,
    ProtocolVersion::V_2025_06_18,
    ProtocolVersion::V_2025_11_25,
    ProtocolVersion::V_2026_07_28,
];

const SERVER_INSTRUCTIONS: &str = "Read-only access to a 2brain v2 workspace (MIT), plus gated mutations and SEP-2640 skills. Start with tb_overview, tb_members, tb_types, or skills/list. Partial tb_content reads never arm writes (SEM-02); complete reads arm both stateful ReadLedger and stateless HMAC read_receipt. tb_search is BM25+proximity and tb_grep is literal substring search (SEM-09).";

const READ_RECEIPT_TTL_SECS: u64 = 900;
const MAX_CONCURRENT_TASKS: usize = 8;

fn is_cognitive_tool(name: &str) -> bool {
    matches!(
        name,
        "tb_orient"
            | "tb_analyze_topology"
            | "tb_detect_gaps"
            | "tb_audit_bias"
            | "tb_audit_contradictions"
            | "tb_fragility_report"
            | "tb_diff_graphs"
            | "tb_record_decision"
            | "tb_list_decisions"
    )
}

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
    pub outcome_registry: bool,
    pub allow_cognitive: bool,
    pub full_schemas: bool,
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

    fn invalidate(&mut self, repo: &str, path: &str) {
        self.armed.remove(&(repo.to_owned(), path.to_owned()));
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

/// Stateless HMAC-SHA256 signer for MCP 2.0 `read_receipt` tokens.
#[derive(Clone)]
struct ReceiptSigner {
    key: [u8; 32],
}

impl ReceiptSigner {
    fn try_ephemeral() -> Result<Self, McpServerError> {
        let mut key = [0u8; 32];
        getrandom::fill(&mut key).map_err(|e| {
            McpServerError::Transport(format!("OS CSPRNG entropy unavailable: {e}"))
        })?;
        Ok(Self { key })
    }

    fn sign(&self, repo: &str, path: &str, short_hash: &str, sha256: &str) -> String {
        let exp = now_unix_secs().saturating_add(READ_RECEIPT_TTL_SECS);
        let payload = format!("v1|{exp}|{repo}|{path}|{short_hash}|{sha256}|whole");
        let tag = hmac_sha256_hex(&self.key, payload.as_bytes());
        format!("{payload}|{tag}")
    }

    fn verify(
        &self,
        token: &str,
        repo: &str,
        path: &str,
        expected_hash: &str,
    ) -> Result<(), String> {
        let Some((payload, tag)) = token.rsplit_once('|') else {
            return Err("policy/prior_read: malformed read_receipt".to_owned());
        };
        let expected_tag = hmac_sha256_hex(&self.key, payload.as_bytes());
        if !ct_eq(tag.as_bytes(), expected_tag.as_bytes()) {
            return Err("policy/prior_read: invalid read_receipt signature".to_owned());
        }
        let parts: Vec<&str> = payload.split('|').collect();
        if parts.len() != 7 || parts[0] != "v1" || parts[6] != "whole" {
            return Err("policy/prior_read: invalid read_receipt payload".to_owned());
        }
        let exp: u64 = parts[1]
            .parse()
            .map_err(|_| "policy/prior_read: invalid read_receipt expiry".to_owned())?;
        if now_unix_secs() > exp {
            return Err(
                "policy/prior_read: read_receipt expired; re-read with tb_content".to_owned(),
            );
        }
        if parts[2] != repo || parts[3] != path {
            return Err(
                "policy/prior_read: read_receipt does not match target (repo, path)".to_owned(),
            );
        }
        if expected_hash != parts[4] && !expected_hash.eq_ignore_ascii_case(parts[5]) {
            return Err("policy/freshness: expected_hash does not match read_receipt".to_owned());
        }
        Ok(())
    }
}

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// RFC 2104 HMAC-SHA256 over pure `sha2` (zero extra dependencies).
fn hmac_sha256_hex(key: &[u8; 32], data: &[u8]) -> String {
    let mut ipad = [0x36u8; 64];
    let mut opad = [0x5cu8; 64];
    for i in 0..32 {
        ipad[i] ^= key[i];
        opad[i] ^= key[i];
    }
    let mut inner = Sha256::new();
    inner.update(ipad);
    inner.update(data);
    let inner_hash = inner.finalize();

    let mut outer = Sha256::new();
    outer.update(opad);
    outer.update(inner_hash);
    let digest = outer.finalize();
    let mut hex = String::with_capacity(64);
    for b in digest {
        let _ = write!(hex, "{b:02x}");
    }
    hex
}

fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (&x, &y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Thin driving MCP adapter over [`AppDispatcher`] (in-process OR Unix-socket daemon) + [`SkillCatalog`].
#[derive(Clone)]
pub struct TwoBrainMcp {
    dispatcher: Arc<dyn AppDispatcher>,
    policy: LaunchPolicy,
    ledger: Arc<Mutex<ReadLedger>>,
    signer: ReceiptSigner,
    tools: Arc<Vec<Tool>>,
    tasks: TaskManager,
    task_slots: Arc<Semaphore>,
}

impl TwoBrainMcp {
    pub fn try_new(
        dispatcher: Arc<dyn AppDispatcher>,
        policy: LaunchPolicy,
    ) -> Result<Self, McpServerError> {
        let tools = Arc::new(build_tool_catalog(policy));
        let signer = ReceiptSigner::try_ephemeral()?;
        Ok(Self {
            dispatcher,
            policy,
            ledger: Arc::new(Mutex::new(ReadLedger::default())),
            signer,
            tools,
            tasks: TaskManager::new(),
            task_slots: Arc::new(Semaphore::new(MAX_CONCURRENT_TASKS)),
        })
    }

    #[must_use]
    pub fn new(dispatcher: Arc<dyn AppDispatcher>, policy: LaunchPolicy) -> Self {
        Self::try_new(dispatcher, policy).expect("OS CSPRNG entropy must be available")
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
        if let Some(snap) = self.dispatcher.local_snapshot() {
            SkillCatalog::from_snapshot(&snap)
        } else {
            SkillCatalog::from_snapshot(&VaultSnapshot::empty())
        }
    }

    fn dispatch_tool(&self, name: &str, args: &Map<String, Value>) -> CallToolResult {
        match name {
            "tb_describe_tool" => match req_str(args, "name") {
                Ok(tool_name) => match self.tools.iter().find(|t| t.name.as_ref() == tool_name) {
                    Some(t) => {
                        let mut described = t.clone();
                        described.input_schema = schemas::full_schema_for(tool_name);
                        ok_json(json!({ "describe_tool": described }))
                    }
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
            "tb_content" => self.handle_content(args),
            "tb_create_file" => self.handle_create_file(args),
            "tb_write_file" => self.handle_guarded_mutation("write_file", args),
            "tb_edit_file" => self.handle_guarded_mutation("edit_file", args),
            "tb_delete_file" => self.handle_guarded_mutation("delete_file", args),
            "tb_rename" => self.handle_guarded_mutation("rename", args),
            "tb_assign_block_id" => self.handle_guarded_mutation("assign_block_id", args),
            "tb_rename_block_id" => self.handle_guarded_mutation("rename_block_id", args),
            "tb_inline" => self.handle_guarded_mutation("inline", args),
            "tb_promote" => self.handle_guarded_mutation("promote", args),
            "tb_rename_type" => {
                if !self.policy.allow_mutations {
                    return err_text("policy/launch_grant: mutations are disabled");
                }
                map_mutation(self.dispatcher.call_mutation("rename_type", args))
            }
            "tb_batch" => self.handle_batch(args),
            "tb_add_dep" | "tb_remove_dep" => {
                if !self.policy.allow_mutations || !self.policy.allow_composition {
                    return err_text(
                        "policy/launch_grant: composition mutations require --allow-mutations and --allow-composition",
                    );
                }
                let verb = name.strip_prefix("tb_").unwrap_or(name);
                map_mutation(self.dispatcher.call_mutation(verb, args))
            }
            "tb_add_workspace_member"
            | "tb_remove_workspace_member"
            | "tb_set_workspace_member_role" => {
                if !self.policy.allow_mutations || !self.policy.allow_workspace_shape {
                    return err_text(
                        "policy/launch_grant: workspace-shape mutations require --allow-mutations and --allow-workspace-shape",
                    );
                }
                let verb = name.strip_prefix("tb_").unwrap_or(name);
                map_mutation(self.dispatcher.call_mutation(verb, args))
            }
            "tb_record_decision" => {
                if !self.policy.allow_cognitive {
                    return err_text(
                        "policy/launch_grant: cognitive analytics tools require --allow-cognitive",
                    );
                }
                // Translate `id` -> `target_id` before crossing IPC so the daemon's numeric
                // frame `id` field never collides with the decision target identifier.
                let mut normalized = args.clone();
                if !normalized.contains_key("target_id")
                    && let Some(id_val) = normalized.get("id").cloned()
                {
                    normalized.insert("target_id".into(), id_val);
                }
                map_app(self.dispatcher.call_read("record_decision", &normalized))
            }
            "tb_orient"
            | "tb_analyze_topology"
            | "tb_detect_gaps"
            | "tb_audit_bias"
            | "tb_audit_contradictions"
            | "tb_fragility_report"
            | "tb_diff_graphs"
            | "tb_list_decisions" => {
                if !self.policy.allow_cognitive {
                    return err_text(
                        "policy/launch_grant: cognitive analytics tools require --allow-cognitive",
                    );
                }
                let verb = name.strip_prefix("tb_").unwrap_or(name);
                map_app(self.dispatcher.call_read(verb, args))
            }
            other => {
                if !self.tools.iter().any(|t| t.name.as_ref() == other) {
                    return err_text(format!("unknown tool '{other}'"));
                }
                let Some(verb) = other.strip_prefix("tb_") else {
                    return err_text(format!("unknown tool '{other}'"));
                };
                map_app(self.dispatcher.call_read(verb, args))
            }
        }
    }

    fn handle_content(&self, args: &Map<String, Value>) -> CallToolResult {
        match self.dispatcher.call_read("content", args) {
            Ok(mut payload) => {
                let Some(content_obj) = payload.get_mut("content").and_then(Value::as_object_mut)
                else {
                    return ok_json(payload);
                };
                let truncated = content_obj
                    .get("truncated")
                    .and_then(Value::as_bool)
                    .unwrap_or(true);
                // SEM-02: ONLY a complete, non-truncated read arms the stateful ledger
                // AND emits a stateless MCP 2.0 HMAC `read_receipt`.
                if !truncated
                    && let (Some(repo), Some(path), Some(short_hash), Some(sha256)) = (
                        content_obj
                            .get("repo")
                            .and_then(Value::as_str)
                            .map(str::to_owned),
                        content_obj
                            .get("path")
                            .and_then(Value::as_str)
                            .map(str::to_owned),
                        content_obj
                            .get("hash")
                            .and_then(Value::as_str)
                            .map(str::to_owned),
                        content_obj
                            .get("sha256")
                            .and_then(Value::as_str)
                            .map(str::to_owned),
                    )
                {
                    let receipt = self.signer.sign(&repo, &path, &short_hash, &sha256);
                    content_obj.insert("read_receipt".into(), json!(receipt));
                    if let Ok(mut ledger) = self.ledger.lock() {
                        ledger.record_whole_read(&repo, &path, short_hash, sha256);
                    }
                }
                ok_json(payload)
            }
            Err(err) => err_text(err.to_string()),
        }
    }

    fn handle_create_file(&self, args: &Map<String, Value>) -> CallToolResult {
        if !self.policy.allow_mutations {
            return err_text("policy/launch_grant: mutations are disabled");
        }
        map_mutation(self.dispatcher.call_mutation("create_file", args))
    }

    /// Enforce SEM-02 prior-read gate for every existing-file operation in `tb_batch`
    /// BEFORE executing step 0, and invalidate all modified `(repo, path)` ledger entries.
    fn handle_batch(&self, args: &Map<String, Value>) -> CallToolResult {
        if !self.policy.allow_mutations {
            return err_text("policy/launch_grant: mutations are disabled");
        }
        let Some(ops) = args.get("operations").and_then(Value::as_array) else {
            return err_text("policy/arguments: 'operations' array is required");
        };
        let mut to_invalidate: Vec<(String, String)> = Vec::new();
        for (idx, item) in ops.iter().enumerate() {
            let Some(obj) = item.as_object() else {
                return err_text(format!(
                    "policy/arguments: operation[{idx}] must be an object"
                ));
            };
            let op_name = obj
                .get("op")
                .or_else(|| obj.get("tool"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let verb = op_name.strip_prefix("tb_").unwrap_or(op_name);
            if matches!(
                verb,
                "write_file" | "edit_file" | "delete_file" | "rename" | "assign_block_id"
            ) {
                let repo = match req_str(obj, "repo") {
                    Ok(r) => r,
                    Err(e) => return err_text(format!("operation[{idx}]: {e}")),
                };
                let path = match req_str(obj, "path") {
                    Ok(p) => p,
                    Err(e) => return err_text(format!("operation[{idx}]: {e}")),
                };
                let expected_hash = match req_str(obj, "expected_hash") {
                    Ok(h) => h,
                    Err(e) => return err_text(format!("operation[{idx}]: {e}")),
                };
                if let Some(receipt) = opt_str(obj, "read_receipt") {
                    if let Err(reason) = self.signer.verify(receipt, repo, path, expected_hash) {
                        return err_text(format!("operation[{idx}]: {reason}"));
                    }
                } else if let Ok(ledger) = self.ledger.lock() {
                    if let Err(reason) = ledger.verify_write(repo, path, expected_hash) {
                        return err_text(format!("operation[{idx}]: {reason}"));
                    }
                } else {
                    return err_text("internal: read ledger lock poisoned");
                }
                to_invalidate.push((repo.to_owned(), path.to_owned()));
            }
        }
        let res = self.dispatcher.call_mutation("batch", args);
        if res.is_ok()
            && let Ok(mut ledger) = self.ledger.lock()
        {
            for (repo, path) in &to_invalidate {
                ledger.invalidate(repo, path);
            }
        }
        map_mutation(res)
    }

    fn handle_guarded_mutation(&self, verb: &str, args: &Map<String, Value>) -> CallToolResult {
        if !self.policy.allow_mutations {
            return err_text("policy/launch_grant: mutations are disabled");
        }
        let repo = match req_str(args, "repo") {
            Ok(r) => r,
            Err(e) => return err_text(e),
        };
        let path = match req_str(args, "path") {
            Ok(p) => p,
            Err(e) => return err_text(e),
        };
        let expected_hash = match req_str(args, "expected_hash") {
            Ok(h) => h,
            Err(e) => return err_text(e),
        };

        // Fail-closed SEM-02 gate: accept EITHER a valid stateless HMAC `read_receipt`
        // (MCP 2.0 stateless mode) OR an armed whole-file `ReadLedger` entry (legacy mode).
        if let Some(receipt) = opt_str(args, "read_receipt") {
            if let Err(reason) = self.signer.verify(receipt, repo, path, expected_hash) {
                return err_text(reason);
            }
        } else if let Ok(ledger) = self.ledger.lock()
            && let Err(reason) = ledger.verify_write(repo, path, expected_hash)
        {
            return err_text(reason);
        }

        let res = self.dispatcher.call_mutation(verb, args);
        if res.is_ok()
            && let Ok(mut ledger) = self.ledger.lock()
        {
            ledger.invalidate(repo, path);
        }
        map_mutation(res)
    }
}

impl ServerHandler for TwoBrainMcp {
    fn get_info(&self) -> ServerConfig {
        let capabilities = ServerCapabilities::builder()
            .enable_tools()
            .enable_prompts()
            .enable_resources()
            .enable_skills_with_directory_read()
            .enable_tasks()
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
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let empty = Map::new();
        let args = request.arguments.as_ref().unwrap_or(&empty).clone();
        let name = request.name.into_owned();

        let wants_async_task = args
            .get("async_task")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let client_supports_tasks = context
            .client_capabilities()
            .is_some_and(|caps| caps.supports_tasks());

        if wants_async_task && client_supports_tasks && is_cognitive_tool(&name) {
            if !self.policy.allow_cognitive {
                return Ok(CallToolResponse::Complete(err_text(
                    "policy/launch_grant: cognitive analytics tools require --allow-cognitive",
                )));
            }
            let Ok(permit) = Arc::clone(&self.task_slots).try_acquire_owned() else {
                return Ok(CallToolResponse::Complete(err_text(format!(
                    "policy/task_quota: at most {MAX_CONCURRENT_TASKS} concurrent async tasks permitted"
                ))));
            };
            let this = self.clone();
            let task_name = name.clone();
            let needs_decision_elicitation =
                name == "tb_record_decision" && opt_str(&args, "decision").is_none();
            let task = self.tasks.spawn(
                TaskOptions::new().with_status_message(format!("running {task_name}")),
                move |ctx| {
                    Box::pin(async move {
                        if ctx.is_cancel_requested() {
                            drop(permit);
                            return Err(TaskExit::Cancelled);
                        }
                        let mut effective_args = args;
                        if needs_decision_elicitation {
                            let id_label = effective_args
                                .get("target_id")
                                .or_else(|| effective_args.get("id"))
                                .and_then(Value::as_str)
                                .unwrap_or("target");
                            let elicit_req = build_review_decision_elicitation(id_label)
                                .map_err(|e| TaskExit::Error(McpError::internal_error(e, None)))?;
                            ctx.set_status_message(format!(
                                "awaiting review decision for {id_label}"
                            ));
                            let response = ctx.request_input("review_decision", elicit_req).await?;
                            let payload = response.get("content").unwrap_or(&response);
                            if response.get("action").and_then(Value::as_str) == Some("cancel")
                                || response.get("action").and_then(Value::as_str) == Some("decline")
                            {
                                drop(permit);
                                return Err(TaskExit::Cancelled);
                            }
                            let Some(dec) = payload.get("decision").and_then(Value::as_str) else {
                                drop(permit);
                                return Ok(err_text(
                                    "policy/arguments: elicitation response missing 'decision'",
                                ));
                            };
                            effective_args.insert("decision".into(), json!(dec));
                            if let Some(rat) = payload.get("rationale").and_then(Value::as_str) {
                                effective_args.insert("rationale".into(), json!(rat));
                            }
                        }
                        if ctx.is_cancel_requested() {
                            drop(permit);
                            return Err(TaskExit::Cancelled);
                        }
                        let is_mutating_task = name == "tb_record_decision";
                        let res = tokio::task::spawn_blocking(move || {
                            let _slot = permit;
                            this.dispatch_tool(&name, &effective_args)
                        })
                        .await
                        .map_err(|e| {
                            TaskExit::Error(McpError::internal_error(
                                format!("tool worker panicked: {e}"),
                                None,
                            ))
                        })?;
                        // Commit boundary: once a mutating task (tb_record_decision) has
                        // durably persisted its decision, never misreport it as Cancelled.
                        if !is_mutating_task && ctx.is_cancel_requested() {
                            return Err(TaskExit::Cancelled);
                        }
                        Ok(res)
                    })
                },
            );
            return Ok(CallToolResponse::Task(CreateTaskResult::new(task)));
        }

        let this = self.clone();
        let result = tokio::task::spawn_blocking(move || this.dispatch_tool(&name, &args))
            .await
            .unwrap_or_else(|e| err_text(format!("tool worker panicked: {e}")));
        Ok(CallToolResponse::Complete(result))
    }

    async fn get_task(
        &self,
        request: GetTaskParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<GetTaskResult, McpError> {
        let detailed = self.tasks.get_task(&request.task_id)?;
        Ok(GetTaskResult::new(detailed))
    }

    async fn update_task(
        &self,
        request: UpdateTaskParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<(), McpError> {
        self.tasks
            .update_task(&request.task_id, request.input_responses)
    }

    async fn cancel_task(
        &self,
        request: CancelTaskParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<(), McpError> {
        self.tasks.cancel_task(&request.task_id)
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
    let text = msg.into();
    let (reason_code, recovery_hint) = schemas::classify_error_with_hint(&text);
    let mut res = CallToolResult::error(vec![ContentBlock::text(text.clone())]);
    res.structured_content = Some(json!({
        "error": text,
        "reason_code": reason_code,
        "recovery_hint": recovery_hint,
    }));
    res
}

fn build_review_decision_elicitation(id_label: &str) -> Result<InputRequest, String> {
    let schema: ElicitationSchema = serde_json::from_value(json!({
        "type": "object",
        "properties": {
            "decision": {
                "type": "string",
                "enum": ["accept", "dismiss", "defer"],
                "description": "Resolution for this ResearchQuestion or Contradiction"
            },
            "rationale": {
                "type": "string",
                "description": "Short rationale persisted in Negative Memory (D11)"
            }
        },
        "required": ["decision"]
    }))
    .map_err(|e| format!("invalid elicitation schema: {e}"))?;
    Ok(InputRequest::Elicitation(ElicitRequest::new(
        ElicitRequestParams::FormElicitationParams {
            meta: None,
            message: format!("Confirm D11 review decision for '{id_label}'"),
            requested_schema: schema,
        },
    )))
}

fn map_app(res: Result<Value, AppError>) -> CallToolResult {
    match res {
        Ok(v) => ok_json(v),
        Err(e) => err_text(e.to_string()),
    }
}

fn map_mutation(res: Result<tb_app::IdempotentOutcome, AppError>) -> CallToolResult {
    match res {
        Ok(out) => ok_json(out.payload),
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

    if policy.allow_cognitive {
        tools.extend([
            make_tool(
                "tb_orient",
                "Return a compact ~400-token structural contextual hint (communities, gateways, peel bridges, gaps).",
            ),
            make_tool(
                "tb_analyze_topology",
                "Run CSR Brandes betweenness, conceptual gateways, Leiden+NMI+Null Callon communities, and Peel-the-Onion.",
            ),
            make_tool(
                "tb_detect_gaps",
                "Detect Dual-Layer (Link ⊕ Rare-IDF) structural gaps with template & anti-tautology filters.",
            ),
            make_tool(
                "tb_audit_bias",
                "Audit a draft answer against source notes to surface omitted blind-spot clusters.",
            ),
            make_tool(
                "tb_audit_contradictions",
                "Audit D03 claims, R14 temporal validity, and Polysingularity contradictions (preserves both poles).",
            ),
            make_tool(
                "tb_fragility_report",
                "Analyze articulation-point hub fragility and recommend overloaded MOC map+satellite splits.",
            ),
            make_tool(
                "tb_diff_graphs",
                "Compare two Git snapshots (G(T1) -> G(T2)) for topological drift and Change-Driven Decay.",
            ),
            make_tool(
                "tb_record_decision",
                "Persist a D07/D11 review decision (accept/dismiss/defer) into workspace Negative Memory (supports SEP-2663 elicitation).",
            ),
            make_tool(
                "tb_list_decisions",
                "List persisted D07/D11 review decisions from workspace Negative Memory.",
            ),
        ]);
    }

    if policy.full_schemas {
        for tool in &mut tools {
            tool.input_schema = schemas::full_schema_for(tool.name.as_ref());
        }
    }

    tools
}

const WORKFLOW_ANSWER_SHAPE: &str = "# Answer shape\n\n1. Direct answer first.\n2. Backing cited by (repo, path) from full tb_content reads.\n3. Implications in reader vocabulary.\n4. Tensions stated explicitly.\n5. Concrete next explorations.\n6. Sources consulted with counts.\n";

const WORKFLOW_WRITE_GATES: &str = "# Write gates\n\nFail-closed order: launch_grant -> arguments -> path_scope -> member_grant -> prior_read (whole file only, SEM-02; stateful ReadLedger or stateless HMAC read_receipt) -> freshness (expected_hash CAS).\n";

#[cfg(test)]
mod tests {
    use super::*;
    use tb_app::{App, StoreError, WorkspaceStore};
    use tb_contracts::{CasVerified, EvidenceReceipt, Locator, WriteIntent};

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
            outcome_registry: false,
            allow_cognitive: false,
            full_schemas: false,
        });
        assert_eq!(full.len(), 54);

        let cognitive = build_tool_catalog(LaunchPolicy {
            allow_cognitive: true,
            ..LaunchPolicy::default()
        });
        assert_eq!(cognitive.len(), 47);

        let full_schema_cat = build_tool_catalog(LaunchPolicy {
            full_schemas: true,
            ..LaunchPolicy::default()
        });
        let content_tool = full_schema_cat
            .iter()
            .find(|t| t.name.as_ref() == "tb_content")
            .unwrap();
        assert!(
            content_tool.input_schema["properties"]
                .get("path")
                .is_some()
        );
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

    #[test]
    fn stateless_hmac_read_receipt_validates_and_rejects_tampering() {
        let signer = ReceiptSigner::try_ephemeral().expect("CSPRNG must succeed");
        let token = signer.sign(
            "vault",
            "ops/task.md",
            "1122334455667788",
            "a".repeat(64).as_str(),
        );
        signer
            .verify(&token, "vault", "ops/task.md", "1122334455667788")
            .expect("valid receipt must verify");
        assert!(
            signer
                .verify(&token, "vault", "ops/other.md", "1122334455667788")
                .is_err()
        );
        assert!(
            signer
                .verify(&token, "vault", "ops/task.md", "deadbeefdeadbeef")
                .is_err()
        );
        let tampered = format!("{token}0");
        assert!(
            signer
                .verify(&tampered, "vault", "ops/task.md", "1122334455667788")
                .is_err()
        );
    }

    #[test]
    fn repeated_prefix_cannot_bypass_cognitive_or_mutation_grants() {
        let app = Arc::new(App::new(DummyStore, VaultSnapshot::empty()));
        let server = TwoBrainMcp::new(app, LaunchPolicy::default());
        let empty = Map::new();
        for sneaky in [
            "tb_orient",
            "tb_tb_orient",
            "tb_tb_analyze_topology",
            "tb_tb_audit_contradictions",
            "tb_tb_fragility_report",
            "tb_tb_diff_graphs",
            "tb_tb_write_file",
        ] {
            let res = server.dispatch_tool(sneaky, &empty);
            assert_eq!(
                res.is_error,
                Some(true),
                "expected '{sneaky}' to be refused under default read-only launch policy"
            );
        }
    }

    #[tokio::test]
    async fn sep2663_task_manager_runs_cognitive_tool_and_reports_completed_state() {
        let app = Arc::new(App::new(DummyStore, VaultSnapshot::empty()));
        let server = TwoBrainMcp::new(
            app,
            LaunchPolicy {
                allow_cognitive: true,
                ..LaunchPolicy::default()
            },
        );
        assert!(server.get_info().capabilities.supports_tasks());

        let this = server.clone();
        let task = server.tasks.spawn(
            TaskOptions::new().with_status_message("running tb_fragility_report"),
            move |_ctx| {
                Box::pin(async move {
                    let empty = Map::new();
                    Ok(this.dispatch_tool("tb_fragility_report", &empty))
                })
            },
        );

        let mut completed = false;
        for _ in 0..40 {
            let detailed = server
                .tasks
                .get_task(&task.task_id)
                .expect("spawned task must be observable immediately");
            if detailed.task.status == rmcp::model::TaskStatus::Completed {
                completed = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(completed, "SEP-2663 task should reach Completed state");
    }

    #[test]
    fn task_slot_semaphore_enforces_exact_concurrency_cap_atomically() {
        let app = Arc::new(App::new(DummyStore, VaultSnapshot::empty()));
        let server = TwoBrainMcp::new(app, LaunchPolicy::default());
        let mut held = Vec::new();
        for _ in 0..MAX_CONCURRENT_TASKS {
            held.push(
                Arc::clone(&server.task_slots)
                    .try_acquire_owned()
                    .expect("slot within quota must succeed"),
            );
        }
        assert!(
            Arc::clone(&server.task_slots).try_acquire_owned().is_err(),
            "admission beyond MAX_CONCURRENT_TASKS must fail closed"
        );
        held.pop();
        assert!(
            Arc::clone(&server.task_slots).try_acquire_owned().is_ok(),
            "releasing a finished worker permit must re-open admission"
        );
    }

    #[test]
    fn batch_write_without_prior_read_is_refused_by_sem02_gate() {
        let app = Arc::new(App::new(DummyStore, VaultSnapshot::empty()));
        let server = TwoBrainMcp::new(
            app,
            LaunchPolicy {
                allow_mutations: true,
                ..LaunchPolicy::default()
            },
        );
        let args = json!({
            "operations": [
                {
                    "op": "write_file",
                    "repo": "vault",
                    "path": "a.md",
                    "expected_hash": "1122334455667788",
                    "content": "hacked\n"
                }
            ]
        });
        let res = server.dispatch_tool("tb_batch", args.as_object().unwrap());
        assert_eq!(res.is_error, Some(true));
        let txt = format!("{res:?}");
        assert!(
            txt.contains("policy/prior_read"),
            "expected SEM-02 policy/prior_read refusal on batch write, got {txt}"
        );
        let sc = res.structured_content.expect("structured error expected");
        assert_eq!(sc["reason_code"], "prior_read");
        assert!(sc["recovery_hint"].as_str().unwrap().contains("tb_content"));
    }

    #[test]
    fn describe_tool_returns_full_schema_even_in_concise_mode() {
        let app = Arc::new(App::new(DummyStore, VaultSnapshot::empty()));
        let server = TwoBrainMcp::new(app, LaunchPolicy::default());
        let args = json!({ "name": "tb_search" });
        let res = server.dispatch_tool("tb_describe_tool", args.as_object().unwrap());
        assert_ne!(res.is_error, Some(true));
        let sc = res.structured_content.unwrap();
        assert!(
            sc["describe_tool"]["inputSchema"]["properties"]
                .get("query")
                .is_some()
        );
    }

    #[tokio::test]
    async fn sep2663_elicitation_pauses_in_input_required_and_resumes_on_update() {
        let app = Arc::new(App::new(DummyStore, VaultSnapshot::empty()));
        let server = TwoBrainMcp::new(
            app,
            LaunchPolicy {
                allow_cognitive: true,
                ..LaunchPolicy::default()
            },
        );
        let req = build_review_decision_elicitation("q_12345678").expect("valid elicitation");
        let task = server.tasks.spawn(
            TaskOptions::new().with_status_message("awaiting decision"),
            move |ctx| {
                Box::pin(async move {
                    let ans = ctx.request_input("review_decision", req).await?;
                    Ok(ok_json(json!({ "accepted": ans })))
                })
            },
        );

        // Wait briefly for task to enter InputRequired state.
        let mut saw_input_required = false;
        for _ in 0..40 {
            let d = server.tasks.get_task(&task.task_id).unwrap();
            if d.task.status == rmcp::model::TaskStatus::InputRequired {
                saw_input_required = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(saw_input_required, "task must pause in InputRequired state");

        // Fulfill elicitation via tasks/update.
        let mut responses = rmcp::model::InputResponses::new();
        responses.insert(
            "review_decision".to_owned(),
            json!({ "action": "accept", "content": { "decision": "dismiss", "rationale": "intentional" } }),
        );
        server
            .tasks
            .update_task(&task.task_id, responses)
            .expect("update_task must succeed");

        let mut completed = false;
        for _ in 0..40 {
            let d = server.tasks.get_task(&task.task_id).unwrap();
            if d.task.status == rmcp::model::TaskStatus::Completed {
                completed = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(completed, "task must complete after elicitation response");
    }
}
