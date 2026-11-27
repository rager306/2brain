//! Application layer: hexagonal ports (traits), immutable `VaultSnapshot`,
//! `ArcSwap` lock-free serving, and complete read-core use cases (0 I/O).
//!
//! Depends ONLY on `crates/domain/*`. Concrete I/O adapters live in
//! `crates/adapters/*` and are wired once in `crates/bin/two-brain`.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::{Arc, Mutex};

use arc_swap::ArcSwap;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tb_cognition::{
    AmbiguousClaimRef, ClaimRecord, Modality, OrientHint, Polarity, SnapshotNodeMeta,
    TopologyReport, analyze_fragility as run_fragility, analyze_topology as run_topology,
    audit_contradictions as run_contradictions, audit_response_bias as run_bias_audit,
    build_orient_hint, detect_structural_gaps as run_gaps, diff_snapshots as run_diff,
    is_claim_relevant,
};
use tb_contracts::{
    Assessment, CasVerified, EvidenceReceipt, Generation, Locator, RevisionDigest, WriteIntent,
};
use tb_graph::{Direction, TypedLinkGraph};
use tb_schema::{Diagnostic, SchemaRegistry};
use tb_search::{EXCERPT_CHARS, SearchIndex, distinct_terms, excerpt, grep_text};
use tb_syntax::{Anchor, FrontmatterValue, ParsedDocument};
use thiserror::Error;

/// Error returned by driven `WorkspaceStore` implementations.
#[derive(Debug, Error)]
pub enum StoreError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("already exists: {0}")]
    AlreadyExists(String),
    #[error("expected_hash_mismatch: {0}")]
    CasMismatch(String),
    #[error("symlink escapes member root: {0}")]
    SymlinkEscape(String),
    #[error("invalid UTF-8 at {0}")]
    InvalidUtf8(String),
    #[error("invalid edit: {0}")]
    InvalidEdit(String),
    #[error("I/O error: {0}")]
    Io(String),
    #[error("outcome_unknown (post-effect failure): {0}")]
    PostEffectFailure(String),
    #[error("unsupported in current mode: {0}")]
    Unsupported(String),
}

/// Application-layer error with explicit machine-readable stage/reason codes.
#[derive(Debug, Error)]
pub enum AppError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("already_exists: {0}")]
    AlreadyExists(String),
    #[error("expected_hash_mismatch: {0}")]
    CasMismatch(String),
    #[error("policy/path_scope: symlink escapes member root ({0})")]
    SymlinkEscape(String),
    #[error("invalid_utf8: file contains non-UTF-8 bytes ({0})")]
    InvalidUtf8(String),
    #[error("invalid_arguments: {0}")]
    InvalidArguments(String),
    #[error("snapshot_stale: generation advanced during release admission check")]
    SnapshotStale,
    #[error("outcome_quota: {0}")]
    OutcomeQuota(String),
    #[error("outcome_conflict: {0}")]
    OutcomeConflict(String),
    #[error("outcome_unknown: {0}")]
    OutcomeUnknown(String),
    #[error("io_error: {0}")]
    Io(String),
    #[error("unsupported: {0}")]
    Unsupported(String),
}

impl AppError {
    #[must_use]
    pub fn outcome_state(&self) -> Option<&'static str> {
        match self {
            Self::OutcomeQuota(_) => Some("quota"),
            Self::OutcomeConflict(_) => Some("conflict"),
            Self::OutcomeUnknown(_) => Some("unknown"),
            Self::CasMismatch(_) => Some("rejected"),
            _ => None,
        }
    }

    #[must_use]
    pub fn is_post_effect(&self) -> bool {
        matches!(self, Self::OutcomeUnknown(_))
    }
}

impl From<StoreError> for AppError {
    fn from(err: StoreError) -> Self {
        match err {
            StoreError::NotFound(s) => Self::NotFound(s),
            StoreError::AlreadyExists(s) => Self::AlreadyExists(s),
            StoreError::CasMismatch(s) => Self::CasMismatch(s),
            StoreError::SymlinkEscape(s) => Self::SymlinkEscape(s),
            StoreError::InvalidUtf8(s) => Self::InvalidUtf8(s),
            StoreError::InvalidEdit(s) => Self::InvalidArguments(s),
            StoreError::Io(s) => Self::Io(s),
            StoreError::PostEffectFailure(s) => Self::OutcomeUnknown(s),
            StoreError::Unsupported(s) => Self::Unsupported(s),
        }
    }
}

/// Receipt returned after an atomic file mutation and optional git commit.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MutationReceipt {
    pub repo: String,
    pub path: String,
    pub sha256: String,
    pub hash: String,
    pub commit: Option<String>,
}

/// Driven port for workspace filesystem and Git access.
pub trait WorkspaceStore: Send + Sync {
    fn read_exact(&self, locator: &Locator) -> Result<(Vec<u8>, EvidenceReceipt), StoreError>;
    fn commit_verified(&self, intent: WriteIntent<CasVerified>) -> Result<String, StoreError>;

    /// Optional live snapshot reload when external edits or mutations advance disk state.
    fn reload_snapshot(
        &self,
        _next_generation: Generation,
    ) -> Result<Option<VaultSnapshot>, StoreError> {
        Ok(None)
    }

    /// Optional read-only historical snapshot loaded at a Git commit/ref (`tb_diff_graphs`).
    fn load_snapshot_at_git_ref(
        &self,
        _repo: &str,
        _git_ref: &str,
    ) -> Result<VaultSnapshot, StoreError> {
        Err(StoreError::Unsupported(
            "load_snapshot_at_git_ref unsupported".into(),
        ))
    }

    fn create_file(
        &self,
        _locator: &Locator,
        _content: &str,
        _commit_message: &str,
    ) -> Result<MutationReceipt, StoreError> {
        Err(StoreError::Unsupported("create_file unsupported".into()))
    }

    fn write_file_cas(
        &self,
        _locator: &Locator,
        _expected_hash: &str,
        _content: &str,
        _commit_message: &str,
    ) -> Result<MutationReceipt, StoreError> {
        Err(StoreError::Unsupported("write_file_cas unsupported".into()))
    }

    fn edit_file_cas(
        &self,
        _locator: &Locator,
        _expected_hash: &str,
        _old_str: &str,
        _new_str: &str,
        _commit_message: &str,
    ) -> Result<MutationReceipt, StoreError> {
        Err(StoreError::Unsupported("edit_file_cas unsupported".into()))
    }

    fn delete_file_cas(
        &self,
        _locator: &Locator,
        _expected_hash: &str,
        _commit_message: &str,
    ) -> Result<Option<String>, StoreError> {
        Err(StoreError::Unsupported(
            "delete_file_cas unsupported".into(),
        ))
    }

    fn rename_file_cas(
        &self,
        _from: &Locator,
        _to: &Locator,
        _expected_hash: &str,
        _commit_message: &str,
    ) -> Result<MutationReceipt, StoreError> {
        Err(StoreError::Unsupported(
            "rename_file_cas unsupported".into(),
        ))
    }
}

/// Durable outcome observation returned by [`OutcomePort::observe`].
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OutcomeObservation {
    pub enabled: bool,
    pub present: bool,
    pub request_id: String,
    pub state: String,
    pub replay: String,
}

/// Result of running a mutation through [`OutcomePort`].
#[derive(Clone, Debug)]
pub struct IdempotentOutcome {
    pub payload: Value,
    pub outcome_state: Option<&'static str>,
    pub replayed: bool,
}

/// Active reservation session holding the workspace outcome lock until terminal record is flushed.
pub trait OutcomeSession: Send {
    fn record_executing(&mut self) -> Result<(), AppError>;
    fn finish_committed(self: Box<Self>, response_payload: &Value) -> Result<(), AppError>;
    fn finish_rejected(self: Box<Self>, reason: &str) -> Result<(), AppError>;
}

/// Outcome of beginning a request-id-tracked mutation.
pub enum OutcomeBegin {
    /// Registry fence is disabled; run mutation normally without journaling.
    Disabled,
    /// Terminal response already recorded for this exact `(request_id, digest)`; return verbatim.
    Replay {
        payload: Value,
        outcome_state: &'static str,
    },
    /// Fresh reservation acquired; caller executes mutation and finalizes via session.
    Reserved(Box<dyn OutcomeSession>),
}

/// Driven port for the ADR-0009 durable outcome registry (implemented in `tb-journal`).
pub trait OutcomePort: Send + Sync {
    fn observe(&self, request_id: &str) -> Result<OutcomeObservation, AppError>;

    fn begin_mutation(
        &self,
        request_id: &str,
        verb: &str,
        workspace: &str,
        arguments: &Value,
        preconditions: &Value,
    ) -> Result<OutcomeBegin, AppError>;
}

/// Default no-op outcome port (fence disabled).
#[derive(Clone, Copy, Debug, Default)]
pub struct DisabledOutcomePort;

impl OutcomePort for DisabledOutcomePort {
    fn observe(&self, request_id: &str) -> Result<OutcomeObservation, AppError> {
        Ok(OutcomeObservation {
            enabled: false,
            present: false,
            request_id: request_id.to_owned(),
            state: "unknown".to_owned(),
            replay: "absent".to_owned(),
        })
    }

    fn begin_mutation(
        &self,
        _request_id: &str,
        _verb: &str,
        _workspace: &str,
        _arguments: &Value,
        _preconditions: &Value,
    ) -> Result<OutcomeBegin, AppError> {
        Ok(OutcomeBegin::Disabled)
    }
}

/// Metadata for a workspace member repository.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MemberMeta {
    pub repo: String,
    pub root: String,
    pub role: String,
    pub deps: Vec<String>,
}

/// One indexed markdown file inside an immutable [`VaultSnapshot`].
#[derive(Clone, Debug)]
pub struct IndexedFile {
    pub locator: Locator,
    pub sha256: RevisionDigest,
    pub short_hash: String,
    pub size_bytes: usize,
    pub raw_text: String,
    pub doc: ParsedDocument,
    pub description: Option<String>,
    pub declared_type: Option<String>,
}

impl IndexedFile {
    #[must_use]
    pub fn body(&self) -> &str {
        self.raw_text.get(self.doc.body_offset..).unwrap_or("")
    }
}

/// Immutable, lock-free-shareable read snapshot of the workspace.
#[derive(Clone, Debug)]
pub struct VaultSnapshot {
    pub generation: Generation,
    pub members: BTreeMap<String, MemberMeta>,
    pub files: BTreeMap<Locator, IndexedFile>,
    pub registry: SchemaRegistry,
    pub graph: TypedLinkGraph,
    pub search: SearchIndex,
    pub diagnostics: Vec<Diagnostic>,
    pub path_index: BTreeMap<(String, String), Locator>,
    pub stem_index: BTreeMap<(String, String), Vec<Locator>>,
}

impl VaultSnapshot {
    #[must_use]
    pub fn empty() -> Self {
        Self {
            generation: Generation(0),
            members: BTreeMap::new(),
            files: BTreeMap::new(),
            registry: SchemaRegistry::new(),
            graph: TypedLinkGraph::build(Vec::new(), &[]),
            search: SearchIndex::build(std::iter::empty()),
            diagnostics: Vec::new(),
            path_index: BTreeMap::new(),
            stem_index: BTreeMap::new(),
        }
    }

    /// Resolve a wikilink target from `from_repo`.
    #[must_use]
    pub fn resolve_link(
        &self,
        from_repo: &str,
        target: &str,
        explicit_repo: Option<&str>,
    ) -> LinkResolution {
        let repo = explicit_repo.unwrap_or(from_repo);
        if !self.members.contains_key(repo) {
            return LinkResolution::Unresolved;
        }
        if let Some(loc) = self.path_index.get(&(repo.to_owned(), target.to_owned())) {
            return LinkResolution::Resolved(loc.clone());
        }
        if let Some(matches) = self.stem_index.get(&(repo.to_owned(), target.to_owned())) {
            return match matches.len() {
                1 => LinkResolution::Resolved(matches[0].clone()),
                n if n > 1 => LinkResolution::Ambiguous(matches.clone()),
                _ => LinkResolution::Unresolved,
            };
        }
        LinkResolution::Unresolved
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LinkResolution {
    Resolved(Locator),
    Ambiguous(Vec<Locator>),
    Unresolved,
}

/// Parameters for `tb_content`.
#[derive(Clone, Debug)]
pub struct ContentQuery {
    pub locator: Locator,
    pub offset_chars: usize,
    pub max_chars: Option<usize>,
    pub expected_sha256: Option<String>,
}

/// Result of `tb_content`, carrying exact SHA-256 and truncation flag for SEM-02.
#[derive(Clone, Debug)]
pub struct ContentResult {
    pub sha256: String,
    pub hash: String,
    pub truncated: bool,
    pub payload: Value,
}

/// Application composition shell parameterized over static-dispatch driven ports.
pub struct App<W: WorkspaceStore> {
    store: W,
    snapshot: ArcSwap<VaultSnapshot>,
    outcomes: Arc<dyn OutcomePort>,
    orient_cache: Mutex<Option<(Generation, OrientHint)>>,
    topology_cache: Mutex<Option<(Generation, usize, usize, TopologyReport)>>,
}

impl<W: WorkspaceStore> App<W> {
    #[must_use]
    pub fn new(store: W, initial: VaultSnapshot) -> Self {
        Self {
            store,
            snapshot: ArcSwap::from_pointee(initial),
            outcomes: Arc::new(DisabledOutcomePort),
            orient_cache: Mutex::new(None),
            topology_cache: Mutex::new(None),
        }
    }

    #[must_use]
    pub fn with_outcomes(store: W, initial: VaultSnapshot, outcomes: Arc<dyn OutcomePort>) -> Self {
        Self {
            store,
            snapshot: ArcSwap::from_pointee(initial),
            outcomes,
            orient_cache: Mutex::new(None),
            topology_cache: Mutex::new(None),
        }
    }

    #[must_use]
    pub fn outcomes(&self) -> &dyn OutcomePort {
        self.outcomes.as_ref()
    }

    #[must_use]
    pub fn snapshot(&self) -> Arc<VaultSnapshot> {
        self.snapshot.load_full()
    }

    pub fn swap_snapshot(&self, next: VaultSnapshot) {
        self.snapshot.store(Arc::new(next));
    }

    #[must_use]
    pub fn generation(&self) -> Generation {
        self.snapshot.load().generation
    }

    #[must_use]
    pub fn store(&self) -> &W {
        &self.store
    }

    /// `tb_overview`
    pub fn overview(&self, repo: Option<&str>) -> Value {
        let snap = self.snapshot();
        let shape = snap.graph.shape();
        let diag_count = snap
            .diagnostics
            .iter()
            .filter(|d| repo.is_none_or(|r| d.repo == r))
            .count();
        let file_count = snap
            .files
            .keys()
            .filter(|loc| repo.is_none_or(|r| loc.repo == r))
            .count();
        let type_count = snap
            .registry
            .iter()
            .filter(|t| repo.is_none_or(|r| t.repo == r))
            .count();
        let member_names: Vec<&str> = snap.members.keys().map(String::as_str).collect();

        json!({
            "overview": {
                "generation": snap.generation.0,
                "members": member_names,
                "file_count": file_count,
                "type_count": type_count,
                "diagnostic_count": diag_count,
                "graph_shape": shape,
            }
        })
    }

    /// `tb_members`
    pub fn members(&self) -> Value {
        let snap = self.snapshot();
        let members: Vec<&MemberMeta> = snap.members.values().collect();
        json!({ "members": members })
    }

    /// `tb_resolve_member`
    pub fn resolve_member(&self, path: &str) -> Value {
        let snap = self.snapshot();
        let found = snap.members.values().find(|m| {
            let prefix = format!("{}/", m.root.trim_end_matches('/'));
            path == m.root || path.starts_with(&prefix)
        });
        match found {
            Some(m) => {
                let prefix = format!("{}/", m.root.trim_end_matches('/'));
                let relative = path.strip_prefix(&prefix).unwrap_or("");
                json!({
                    "resolve_member": {
                        "repo": m.repo,
                        "root": m.root,
                        "relative_path": relative,
                        "found": true,
                    }
                })
            }
            None => json!({ "resolve_member": { "found": false } }),
        }
    }

    /// `tb_top_level_dirs`
    pub fn top_level_dirs(&self, repo: Option<&str>) -> Value {
        let snap = self.snapshot();
        let mut counts: BTreeMap<String, usize> = BTreeMap::new();
        for loc in snap.files.keys() {
            if repo.is_some_and(|r| loc.repo != r) {
                continue;
            }
            let dir = loc
                .path
                .split_once('/')
                .map_or(".", |(head, _)| head)
                .to_owned();
            *counts.entry(dir).or_insert(0) += 1;
        }
        let dirs: Vec<Value> = counts
            .into_iter()
            .map(|(name, files)| json!({ "name": name, "files": files }))
            .collect();
        json!({ "top_level_dirs": dirs })
    }

    /// `tb_files`
    pub fn files(
        &self,
        repo: Option<&str>,
        prefix: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Value {
        let snap = self.snapshot();
        let matched: Vec<&IndexedFile> = snap
            .files
            .values()
            .filter(|f| repo.is_none_or(|r| f.locator.repo == r))
            .filter(|f| prefix.is_none_or(|p| f.locator.path.starts_with(p)))
            .collect();
        let total = matched.len();
        let page: Vec<Value> = matched
            .into_iter()
            .skip(offset)
            .take(limit)
            .map(|f| {
                let root = snap
                    .members
                    .get(&f.locator.repo)
                    .map(|m| m.root.trim_end_matches('/'))
                    .unwrap_or("");
                let abs_path = if root.is_empty() {
                    f.locator.path.clone()
                } else {
                    format!("{root}/{}", f.locator.path)
                };
                let stem = file_stem(&f.locator.path);
                json!({
                    "repo": f.locator.repo,
                    "path": abs_path,
                    "relative_path": f.locator.path,
                    "stem": stem,
                    "type": f.declared_type,
                    "description": f.description,
                    "sha256": f.sha256.to_hex(),
                    "hash": f.short_hash,
                })
            })
            .collect();
        let next_offset = if offset + page.len() < total {
            Some(offset + page.len())
        } else {
            None
        };
        json!({
            "files": page,
            "total": total,
            "offset": offset,
            "next_offset": next_offset,
        })
    }

    /// `tb_content`: exact disk read with SEM-04 (strict UTF-8), SEM-18 (symlink confinement),
    /// D02 exact SHA-256 + conditional `expected.sha256`, and `retention: { retained: false }`.
    pub fn content(&self, req: &ContentQuery) -> Result<ContentResult, AppError> {
        let (bytes, receipt) = self.store.read_exact(&req.locator)?;
        let sha256_hex = receipt.sha256.to_hex();
        let short_hash = receipt.write_guard.to_hex();

        // SEM-04: refuse invalid UTF-8 explicitly; never serve lossy text.
        let text = std::str::from_utf8(&bytes).map_err(|_| {
            AppError::InvalidUtf8(format!("{}/{}", req.locator.repo, req.locator.path))
        })?;

        let total_chars = text.chars().count();
        if let Some(expected) = &req.expected_sha256 {
            let expected_norm = expected.strip_prefix("sha256:").unwrap_or(expected);
            if !expected_norm.eq_ignore_ascii_case(&sha256_hex) {
                let payload = json!({
                    "content": {
                        "repo": req.locator.repo,
                        "path": req.locator.path,
                        "sha256": sha256_hex,
                        "hash": short_hash,
                        "size_bytes": bytes.len(),
                        "total_chars": total_chars,
                        "offset_chars": req.offset_chars,
                        "truncated": true,
                        "text": Value::Null,
                        "revision": {
                            "status": "stale",
                            "expected_sha256": expected_norm,
                            "actual_sha256": sha256_hex,
                        },
                        "retention": { "retained": false },
                    }
                });
                return Ok(ContentResult {
                    sha256: sha256_hex,
                    hash: short_hash,
                    truncated: true,
                    payload,
                });
            }
        }

        let sliced: String = match req.max_chars {
            Some(max) => text.chars().skip(req.offset_chars).take(max).collect(),
            None => text.chars().skip(req.offset_chars).collect(),
        };
        let sliced_chars = sliced.chars().count();
        let truncated = req.offset_chars > 0 || (req.offset_chars + sliced_chars < total_chars);

        let mut content_obj = serde_json::Map::new();
        content_obj.insert("repo".into(), json!(req.locator.repo));
        content_obj.insert("path".into(), json!(req.locator.path));
        content_obj.insert("sha256".into(), json!(sha256_hex));
        content_obj.insert("hash".into(), json!(short_hash));
        content_obj.insert("size_bytes".into(), json!(bytes.len()));
        content_obj.insert("total_chars".into(), json!(total_chars));
        content_obj.insert("offset_chars".into(), json!(req.offset_chars));
        content_obj.insert("truncated".into(), json!(truncated));
        content_obj.insert("text".into(), json!(sliced));
        if req.expected_sha256.is_some() {
            content_obj.insert(
                "revision".into(),
                json!({ "status": "match", "actual_sha256": sha256_hex }),
            );
        }
        content_obj.insert("retention".into(), json!({ "retained": false }));

        Ok(ContentResult {
            sha256: sha256_hex,
            hash: short_hash,
            truncated,
            payload: json!({ "content": Value::Object(content_obj) }),
        })
    }

    /// `tb_frontmatter`
    pub fn frontmatter(&self, locator: &Locator) -> Result<Value, AppError> {
        let snap = self.snapshot();
        let file = snap
            .files
            .get(locator)
            .ok_or_else(|| AppError::NotFound(format!("{}/{}", locator.repo, locator.path)))?;
        let fields = file
            .doc
            .frontmatter
            .as_ref()
            .map(fm_map_to_json)
            .unwrap_or_else(|| json!({}));
        Ok(json!({
            "frontmatter": {
                "repo": locator.repo,
                "path": locator.path,
                "type": file.declared_type,
                "fields": fields,
                "error": file.doc.frontmatter_error,
            }
        }))
    }

    /// `tb_anchors`
    pub fn anchors(&self, locator: &Locator) -> Result<Value, AppError> {
        let snap = self.snapshot();
        let file = snap
            .files
            .get(locator)
            .ok_or_else(|| AppError::NotFound(format!("{}/{}", locator.repo, locator.path)))?;
        Ok(json!({ "anchors": file.doc.anchors }))
    }

    /// `tb_block_ids`
    pub fn block_ids(&self, locator: &Locator) -> Result<Value, AppError> {
        let snap = self.snapshot();
        let file = snap
            .files
            .get(locator)
            .ok_or_else(|| AppError::NotFound(format!("{}/{}", locator.repo, locator.path)))?;
        let ids: Vec<Value> = file
            .doc
            .anchors
            .iter()
            .filter_map(|a| match a {
                Anchor::Block {
                    id,
                    line,
                    byte_start,
                    ..
                } => Some(json!({ "id": id, "line": line, "byte_start": byte_start })),
                Anchor::Heading { .. } => None,
            })
            .collect();
        Ok(json!({ "block_ids": ids }))
    }

    /// `tb_semantic_tokens`
    pub fn semantic_tokens(&self, locator: &Locator) -> Result<Value, AppError> {
        let snap = self.snapshot();
        let file = snap
            .files
            .get(locator)
            .ok_or_else(|| AppError::NotFound(format!("{}/{}", locator.repo, locator.path)))?;
        let tokens: Vec<Value> = file
            .doc
            .wikilinks
            .iter()
            .map(|w| {
                json!({
                    "kind": "wikilink",
                    "edge_kind": w.kind,
                    "target": w.target,
                    "repo": w.repo,
                    "anchor": w.anchor,
                    "byte_start": w.byte_start,
                    "byte_end": w.byte_end,
                })
            })
            .collect();
        Ok(json!({ "semantic_tokens": tokens }))
    }

    /// `tb_types`
    pub fn types(&self, repo: Option<&str>) -> Value {
        let snap = self.snapshot();
        let types: Vec<Value> = snap
            .registry
            .iter()
            .filter(|t| repo.is_none_or(|r| t.repo == r))
            .map(|t| {
                json!({
                    "repo": t.repo,
                    "name": t.name,
                    "abstract": t.is_abstract,
                    "sealed": t.sealed,
                    "meta": t.meta,
                    "extends": t.extends.as_ref().map(|(n, r)| match r {
                        Some(rr) => format!("{n}::{rr}"),
                        None => n.clone(),
                    }),
                    "field_count": t.fields.len(),
                })
            })
            .collect();
        json!({ "types": types })
    }

    /// `tb_type`
    pub fn type_def(&self, repo: &str, name: &str) -> Result<Value, AppError> {
        let snap = self.snapshot();
        let def = snap
            .registry
            .resolve(repo, name)
            .ok_or_else(|| AppError::NotFound(format!("type '{name}' in '{repo}'")))?;
        let effective = snap
            .registry
            .effective_fields(&def.repo, &def.name)
            .unwrap_or_else(|_| def.fields.clone());
        Ok(json!({
            "type": {
                "repo": def.repo,
                "name": def.name,
                "abstract": def.is_abstract,
                "sealed": def.sealed,
                "meta": def.meta,
                "location": def.location,
                "body": def.body,
                "extends": def.extends,
                "fields": effective,
            }
        }))
    }

    /// `tb_type_tree`
    pub fn type_tree(&self, repo: Option<&str>) -> Value {
        let snap = self.snapshot();
        let nodes: Vec<Value> = snap
            .registry
            .iter()
            .filter(|t| repo.is_none_or(|r| t.repo == r))
            .map(|t| {
                let children: Vec<String> = snap
                    .registry
                    .subtypes_of(&t.repo, &t.name)
                    .into_iter()
                    .map(|c| c.name.clone())
                    .collect();
                json!({
                    "repo": t.repo,
                    "name": t.name,
                    "parent": t.extends.as_ref().map(|(n, _)| n),
                    "children": children,
                })
            })
            .collect();
        json!({ "type_tree": nodes })
    }

    /// `tb_subtypes`
    pub fn subtypes(&self, repo: &str, name: &str) -> Value {
        let snap = self.snapshot();
        let subs: Vec<Value> = snap
            .registry
            .subtypes_of(repo, name)
            .into_iter()
            .map(|t| json!({ "repo": t.repo, "name": t.name }))
            .collect();
        json!({ "subtypes": subs })
    }

    /// `tb_typed` / `tb_instances`
    pub fn instances(&self, repo: Option<&str>, limit: usize, offset: usize) -> Value {
        let snap = self.snapshot();
        let matched: Vec<&IndexedFile> = snap
            .files
            .values()
            .filter(|f| f.declared_type.is_some())
            .filter(|f| repo.is_none_or(|r| f.locator.repo == r))
            .collect();
        let total = matched.len();
        let page: Vec<Value> = matched
            .into_iter()
            .skip(offset)
            .take(limit)
            .map(|f| {
                json!({
                    "repo": f.locator.repo,
                    "path": f.locator.path,
                    "type": f.declared_type,
                    "description": f.description,
                })
            })
            .collect();
        let next_offset = (offset + page.len() < total).then_some(offset + page.len());
        json!({
            "instances": page,
            "total": total,
            "offset": offset,
            "next_offset": next_offset,
        })
    }

    /// `tb_instances_of`
    pub fn instances_of(
        &self,
        repo: Option<&str>,
        type_name: &str,
        limit: usize,
        offset: usize,
    ) -> Value {
        let snap = self.snapshot();
        let matched: Vec<&IndexedFile> = snap
            .files
            .values()
            .filter(|f| f.declared_type.as_deref() == Some(type_name))
            .filter(|f| repo.is_none_or(|r| f.locator.repo == r))
            .collect();
        let total = matched.len();
        let page: Vec<Value> = matched
            .into_iter()
            .skip(offset)
            .take(limit)
            .map(|f| {
                json!({
                    "repo": f.locator.repo,
                    "path": f.locator.path,
                    "type": f.declared_type,
                    "description": f.description,
                })
            })
            .collect();
        let next_offset = (offset + page.len() < total).then_some(offset + page.len());
        json!({
            "instances_of": page,
            "total": total,
            "offset": offset,
            "next_offset": next_offset,
        })
    }

    /// `tb_candidates`: untyped notes whose frontmatter satisfies all required fields of `type_name`.
    pub fn candidates(
        &self,
        repo: &str,
        type_name: &str,
        limit: usize,
        offset: usize,
    ) -> Result<Value, AppError> {
        let snap = self.snapshot();
        let def = snap
            .registry
            .resolve(repo, type_name)
            .ok_or_else(|| AppError::NotFound(format!("type '{type_name}' in '{repo}'")))?;
        let fields = snap
            .registry
            .effective_fields(&def.repo, &def.name)
            .map_err(AppError::InvalidArguments)?;

        let matched: Vec<&IndexedFile> = snap
            .files
            .values()
            .filter(|f| f.locator.repo == repo && f.declared_type.is_none())
            .filter(|f| {
                let Some(fm) = &f.doc.frontmatter else {
                    return fields.values().all(|spec| spec.optional);
                };
                fields.iter().all(|(k, spec)| match fm.get(k) {
                    None | Some(FrontmatterValue::Null) => spec.optional,
                    Some(v) => snap
                        .registry
                        .validate_field_value(&def.repo, &spec.expr, v)
                        .is_ok(),
                })
            })
            .collect();
        let total = matched.len();
        let page: Vec<Value> = matched
            .into_iter()
            .skip(offset)
            .take(limit)
            .map(|f| json!({ "repo": f.locator.repo, "path": f.locator.path }))
            .collect();
        let next_offset = (offset + page.len() < total).then_some(offset + page.len());
        Ok(json!({
            "candidates": page,
            "total": total,
            "offset": offset,
            "next_offset": next_offset,
        }))
    }

    /// `tb_validate_value`
    pub fn validate_value(
        &self,
        repo: &str,
        type_name: &str,
        field: &str,
        value: &Value,
    ) -> Result<Value, AppError> {
        let snap = self.snapshot();
        let def = snap
            .registry
            .resolve(repo, type_name)
            .ok_or_else(|| AppError::NotFound(format!("type '{type_name}' in '{repo}'")))?;
        let fields = snap
            .registry
            .effective_fields(&def.repo, &def.name)
            .map_err(AppError::InvalidArguments)?;
        let spec = fields
            .get(field)
            .ok_or_else(|| AppError::NotFound(format!("field '{field}' on type '{type_name}'")))?;
        let fm_val = json_to_fm_value(value);
        match snap
            .registry
            .validate_field_value(&def.repo, &spec.expr, &fm_val)
        {
            Ok(()) => Ok(json!({ "validate_value": { "valid": true, "error": Value::Null } })),
            Err(err) => Ok(json!({ "validate_value": { "valid": false, "error": err } })),
        }
    }

    /// `tb_diagnostics`
    pub fn diagnostics(
        &self,
        repo: Option<&str>,
        path: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Value {
        let snap = self.snapshot();
        let matched: Vec<&Diagnostic> = snap
            .diagnostics
            .iter()
            .filter(|d| repo.is_none_or(|r| d.repo == r))
            .filter(|d| path.is_none_or(|p| d.path == p))
            .collect();
        let total = matched.len();
        let page: Vec<&Diagnostic> = matched.into_iter().skip(offset).take(limit).collect();
        let next_offset = (offset + page.len() < total).then_some(offset + page.len());
        json!({
            "diagnostics": page,
            "total": total,
            "offset": offset,
            "next_offset": next_offset,
        })
    }

    /// `tb_search` (BM25 + proximity + rare-IDF escape + excerpts)
    pub fn search(
        &self,
        query: &str,
        repo: Option<&str>,
        exact: bool,
        min_should_match: Option<usize>,
        limit: usize,
        offset: usize,
    ) -> Value {
        let snap = self.snapshot();
        let all_hits = snap
            .search
            .query(query, exact, min_should_match, repo, usize::MAX);
        let total = all_hits.len();
        let terms = distinct_terms(query);
        let matches: Vec<Value> = all_hits
            .into_iter()
            .skip(offset)
            .take(limit)
            .map(|hit| {
                let file = snap.files.get(&hit.locator);
                let snip = file
                    .map(|f| excerpt(f.body(), &terms, EXCERPT_CHARS))
                    .unwrap_or_default();
                json!({
                    "repo": hit.locator.repo,
                    "path": hit.locator.path,
                    "score": hit.score,
                    "matched_terms": hit.matched_terms,
                    "weak": hit.weak,
                    "description": file.and_then(|f| f.description.as_deref()),
                    "excerpt": snip,
                })
            })
            .collect();
        let next_offset = (offset + matches.len() < total).then_some(offset + matches.len());
        json!({
            "search": {
                "query": query,
                "exact": exact,
                "matches": matches,
            },
            "total": total,
            "offset": offset,
            "next_offset": next_offset,
        })
    }

    /// `tb_grep` (SEM-09: strictly literal substring search, never BM25 conjunction)
    pub fn grep(
        &self,
        query: &str,
        repo: Option<&str>,
        case_sensitive: bool,
        limit: usize,
        offset: usize,
    ) -> Value {
        let snap = self.snapshot();
        let mut all_matches = Vec::new();
        for file in snap.files.values() {
            if repo.is_some_and(|r| file.locator.repo != r) {
                continue;
            }
            let lines = grep_text(&file.raw_text, query, case_sensitive);
            if !lines.is_empty() {
                all_matches.push(json!({
                    "repo": file.locator.repo,
                    "path": file.locator.path,
                    "line": lines[0].line,
                    "column": lines[0].column,
                    "snippet": lines[0].snippet,
                    "match_count": lines.len(),
                }));
            }
        }
        let total = all_matches.len();
        let page: Vec<Value> = all_matches.into_iter().skip(offset).take(limit).collect();
        let next_offset = (offset + page.len() < total).then_some(offset + page.len());
        json!({
            "grep": {
                "query": query,
                "matches": page,
            },
            "total": total,
            "offset": offset,
            "next_offset": next_offset,
        })
    }

    /// `tb_references`
    pub fn references(&self, locator: &Locator) -> Result<Value, AppError> {
        let snap = self.snapshot();
        let id = snap
            .graph
            .node_id(locator)
            .ok_or_else(|| AppError::NotFound(format!("{}/{}", locator.repo, locator.path)))?;
        let (out_ids, out_kinds) = snap.graph.out_neighbors(id);
        let (in_ids, in_kinds) = snap.graph.in_neighbors(id);
        let outgoing: Vec<Value> = out_ids
            .iter()
            .zip(out_kinds.iter())
            .filter_map(|(&nid, &kind)| {
                let l = snap.graph.locator(nid)?;
                Some(json!({ "repo": l.repo, "path": l.path, "kind": kind }))
            })
            .collect();
        let incoming: Vec<Value> = in_ids
            .iter()
            .zip(in_kinds.iter())
            .filter_map(|(&nid, &kind)| {
                let l = snap.graph.locator(nid)?;
                Some(json!({ "repo": l.repo, "path": l.path, "kind": kind }))
            })
            .collect();
        Ok(json!({
            "references": {
                "repo": locator.repo,
                "path": locator.path,
                "incoming": incoming,
                "outgoing": outgoing,
            }
        }))
    }

    /// `tb_follow`
    pub fn follow(&self, from_repo: &str, target: &str) -> Value {
        let snap = self.snapshot();
        let (base, explicit_repo) = match target.split_once("::") {
            Some((t, r)) => (t, Some(r)),
            None => (target, None),
        };
        let (base_target, anchor) = match base.split_once('#') {
            Some((t, a)) => (t, Some(a)),
            None => (base, None),
        };
        match snap.resolve_link(from_repo, base_target, explicit_repo) {
            LinkResolution::Resolved(loc) => json!({
                "follow": {
                    "status": "resolved",
                    "repo": loc.repo,
                    "path": loc.path,
                    "anchor": anchor,
                }
            }),
            LinkResolution::Ambiguous(candidates) => json!({
                "follow": {
                    "status": "ambiguous",
                    "candidates": candidates,
                }
            }),
            LinkResolution::Unresolved => json!({
                "follow": {
                    "status": "unresolved",
                    "target": target,
                }
            }),
        }
    }

    /// `tb_resolved`
    pub fn resolved(&self, locator: &Locator) -> Result<Value, AppError> {
        let snap = self.snapshot();
        let file = snap
            .files
            .get(locator)
            .ok_or_else(|| AppError::NotFound(format!("{}/{}", locator.repo, locator.path)))?;
        let links: Vec<Value> = file
            .doc
            .wikilinks
            .iter()
            .map(|w| {
                let resolution = snap.resolve_link(&locator.repo, &w.target, w.repo.as_deref());
                match resolution {
                    LinkResolution::Resolved(t) => json!({
                        "target": w.target,
                        "status": "resolved",
                        "resolved_repo": t.repo,
                        "resolved_path": t.path,
                        "kind": w.kind,
                    }),
                    LinkResolution::Ambiguous(c) => json!({
                        "target": w.target,
                        "status": "ambiguous",
                        "candidates": c,
                        "kind": w.kind,
                    }),
                    LinkResolution::Unresolved => json!({
                        "target": w.target,
                        "status": "unresolved",
                        "kind": w.kind,
                    }),
                }
            })
            .collect();
        Ok(json!({ "resolved": links }))
    }

    /// `tb_children`
    pub fn children(&self, locator: &Locator) -> Result<Value, AppError> {
        let snap = self.snapshot();
        let id = snap
            .graph
            .node_id(locator)
            .ok_or_else(|| AppError::NotFound(format!("{}/{}", locator.repo, locator.path)))?;
        let (out_ids, out_kinds) = snap.graph.out_neighbors(id);
        let children: Vec<Value> = out_ids
            .iter()
            .zip(out_kinds.iter())
            .filter_map(|(&nid, &kind)| {
                let l = snap.graph.locator(nid)?;
                Some(json!({ "repo": l.repo, "path": l.path, "kind": kind }))
            })
            .collect();
        Ok(json!({ "children": children }))
    }

    /// `tb_imports`
    pub fn imports(&self, repo: Option<&str>) -> Value {
        let snap = self.snapshot();
        let deps: BTreeMap<String, Vec<String>> = snap
            .members
            .keys()
            .filter(|r| repo.is_none_or(|wanted| r.as_str() == wanted))
            .map(|r| (r.clone(), snap.registry.deps_of(r)))
            .collect();
        json!({ "imports": deps })
    }

    /// `tb_hubs`
    pub fn hubs(&self, limit: usize) -> Value {
        let snap = self.snapshot();
        let rows = snap.graph.hubs(limit);
        json!({ "hubs": rows })
    }

    /// `tb_graph_health`
    pub fn graph_health(&self, repo: Option<&str>) -> Value {
        let snap = self.snapshot();
        let shape = snap.graph.shape();
        let health = snap.graph.health();
        json!({
            "graph_health": {
                "repo": repo,
                "scope": "workspace",
                "shape": shape,
                "health": health,
            }
        })
    }

    /// `tb_neighborhood`
    pub fn neighborhood(
        &self,
        locator: &Locator,
        depth: u8,
        limit: usize,
    ) -> Result<Value, AppError> {
        let snap = self.snapshot();
        let sub = snap
            .graph
            .neighborhood(locator, depth as usize, Direction::Both, limit)
            .ok_or_else(|| AppError::NotFound(format!("{}/{}", locator.repo, locator.path)))?;
        Ok(json!({ "neighborhood": sub }))
    }

    /// `tb_context` (Issue #7 D01–D06, KTD5 Release Admission Gate, KTD9 Honest Uncertainty).
    pub fn context(
        &self,
        query: &str,
        repo: Option<&str>,
        max_notes: usize,
        max_chars_per_note: usize,
    ) -> Result<Value, AppError> {
        match self.assemble_context_once(query, repo, max_notes, max_chars_per_note) {
            Ok(v) => Ok(v),
            Err(AppError::SnapshotStale) => {
                let next_gen = self.generation().next();
                if let Some(fresh) = self.store.reload_snapshot(next_gen)? {
                    self.swap_snapshot(fresh);
                    self.assemble_context_once(query, repo, max_notes, max_chars_per_note)
                } else {
                    Err(AppError::SnapshotStale)
                }
            }
            Err(err) => Err(err),
        }
    }

    fn assemble_context_once(
        &self,
        query: &str,
        repo: Option<&str>,
        max_notes: usize,
        max_chars_per_note: usize,
    ) -> Result<Value, AppError> {
        let snap = self.snapshot();
        let gen_before = snap.generation;
        let max_notes = max_notes.clamp(1, 20);
        let max_chars = max_chars_per_note.clamp(100, 4000);

        let all_hits = snap.search.query(query, false, None, repo, max_notes + 1);
        let truncated_by_limit = all_hits.len() > max_notes;
        let terms = distinct_terms(query);

        let mut any_note_truncated = truncated_by_limit;
        let mut fragments = Vec::new();
        let mut neighbor_set: BTreeSet<Locator> = BTreeSet::new();

        for hit in all_hits.into_iter().take(max_notes) {
            if let Some(file) = snap.files.get(&hit.locator) {
                // Verify live disk revision before releasing fragment text so external
                // edits cannot silently serve stale excerpts alongside live tb_content.
                let (_, live_receipt) = self
                    .store
                    .read_exact(&file.locator)
                    .map_err(|_| AppError::SnapshotStale)?;
                if live_receipt.sha256 != file.sha256 {
                    return Err(AppError::SnapshotStale);
                }

                let body = file.body();
                let text_chars = body.chars().count();
                let window = if text_chars <= max_chars {
                    body.to_owned()
                } else {
                    any_note_truncated = true;
                    excerpt(body, &terms, max_chars)
                };
                if let Some(sub) = snap.graph.neighborhood(&hit.locator, 1, Direction::Both, 6) {
                    for loc in sub.nodes {
                        if loc != hit.locator {
                            neighbor_set.insert(loc);
                        }
                    }
                }
                fragments.push(json!({
                    "repo": file.locator.repo,
                    "path": file.locator.path,
                    "sha256": file.sha256.to_hex(),
                    "score": hit.score,
                    "weak": hit.weak,
                    "description": file.description,
                    "text": window,
                }));
            }
        }

        // KTD5 / U5 Release Admission Gate: verify generation did not advance during assembly.
        if self.snapshot.load().generation != gen_before {
            return Err(AppError::SnapshotStale);
        }

        let support_assessment: Assessment<usize> = if fragments.is_empty() {
            Assessment::AssessedEmpty
        } else {
            Assessment::Available {
                items: fragments.len(),
            }
        };
        let missingness_assessment: Assessment<bool> = if any_note_truncated {
            Assessment::Incomplete {
                partial: true,
                reason: "bounded by max_notes or max_chars_per_note".to_owned(),
            }
        } else {
            Assessment::Unassessed
        };
        let conflicts_assessment: Assessment<Vec<String>> = Assessment::Unassessed;
        let supersession_assessment: Assessment<Vec<String>> = Assessment::Unassessed;

        Ok(json!({
            "context": {
                "query": query,
                "generation": gen_before.0,
                "fragments": fragments,
                "related_neighbors": neighbor_set.into_iter().take(10).collect::<Vec<_>>(),
                "why": {
                    "support": support_assessment,
                    "missingness": missingness_assessment,
                    "conflicts": conflicts_assessment,
                    "supersession": supersession_assessment,
                },
                "retention": { "retained": false },
            }
        }))
    }

    /// `tb_mutation_outcome`
    pub fn mutation_outcome(&self, request_id: &str) -> Result<Value, AppError> {
        let obs = self.outcomes.observe(request_id)?;
        Ok(json!({ "mutation_outcome": obs }))
    }

    /// `tb_orient`: generation-fenced ~400-token Structural Contextual Hint.
    pub fn orient(&self, suppressed_question_ids: &HashSet<String>) -> Result<Value, AppError> {
        let snap = self.snapshot();
        let gen_before = snap.generation;
        if suppressed_question_ids.is_empty()
            && let Ok(guard) = self.orient_cache.lock()
            && let Some((cached_gen, hint)) = guard.as_ref()
            && *cached_gen == gen_before
        {
            if self.snapshot.load().generation != gen_before {
                return Err(AppError::SnapshotStale);
            }
            return Ok(json!({ "orient": hint, "generation": gen_before.0, "cached": true }));
        }
        let hint = build_orient_hint(
            &snap.graph,
            &snap.search,
            snap.files.iter().map(|(loc, f)| (loc, f.body())),
            suppressed_question_ids,
        );
        // Generation fence: refuse to cache or release analytics computed against a superseded snapshot.
        if self.snapshot.load().generation != gen_before {
            return Err(AppError::SnapshotStale);
        }
        if suppressed_question_ids.is_empty()
            && let Ok(mut guard) = self.orient_cache.lock()
        {
            *guard = Some((gen_before, hint.clone()));
        }
        Ok(json!({ "orient": hint, "generation": gen_before.0, "cached": false }))
    }

    /// `tb_analyze_topology`: generation-fenced Brandes, Gateways, Leiden+NMI+Null, Callon, Peel.
    pub fn analyze_topology(&self, peel_top_k: usize, limit: usize) -> Result<Value, AppError> {
        let snap = self.snapshot();
        let gen_before = snap.generation;
        let peel_k = peel_top_k.clamp(1, 20);
        let top_n = limit.clamp(1, 30);
        if let Ok(guard) = self.topology_cache.lock()
            && let Some((g, pk, lim, report)) = guard.as_ref()
            && *g == gen_before
            && *pk == peel_k
            && *lim == top_n
        {
            if self.snapshot.load().generation != gen_before {
                return Err(AppError::SnapshotStale);
            }
            return Ok(json!({
                "analyze_topology": report,
                "generation": gen_before.0,
                "cached": true,
            }));
        }
        let report = run_topology(&snap.graph, peel_k, top_n);
        if self.snapshot.load().generation != gen_before {
            return Err(AppError::SnapshotStale);
        }
        if let Ok(mut guard) = self.topology_cache.lock() {
            *guard = Some((gen_before, peel_k, top_n, report.clone()));
        }
        Ok(json!({
            "analyze_topology": report,
            "generation": gen_before.0,
            "cached": false,
        }))
    }

    /// `tb_detect_gaps`: Dual-Layer Link ⊕ Rare-IDF Discrepancy with Anti-Tautology & Negative Memory.
    pub fn detect_gaps(
        &self,
        suppressed_question_ids: &HashSet<String>,
        limit: usize,
    ) -> Result<Value, AppError> {
        let snap = self.snapshot();
        let gen_before = snap.generation;
        let report = run_gaps(
            &snap.graph,
            &snap.search,
            snap.files.iter().map(|(loc, f)| (loc, f.body())),
            suppressed_question_ids,
            limit.clamp(1, 30),
        );
        if self.snapshot.load().generation != gen_before {
            return Err(AppError::SnapshotStale);
        }
        Ok(json!({
            "detect_gaps": report,
            "generation": gen_before.0,
        }))
    }

    /// `tb_audit_bias`: deterministic source-cluster coverage vs blind spots for a draft answer.
    pub fn audit_bias(&self, locators: &[Locator], response_text: &str) -> Result<Value, AppError> {
        let snap = self.snapshot();
        let gen_before = snap.generation;
        let sources: Vec<(&Locator, &str)> = locators
            .iter()
            .filter_map(|loc| snap.files.get(loc).map(|f| (&f.locator, f.body())))
            .collect();
        let report = run_bias_audit(sources, response_text);
        if self.snapshot.load().generation != gen_before {
            return Err(AppError::SnapshotStale);
        }
        Ok(json!({
            "audit_bias": report,
            "generation": gen_before.0,
        }))
    }

    /// `tb_audit_contradictions`: pure-domain D03 Claim Layer + R14 Temporal Validity +
    /// Polysingularity Contradiction Audit (preserves both poles).
    pub fn audit_contradictions(
        &self,
        as_of: Option<&str>,
        limit: usize,
    ) -> Result<Value, AppError> {
        let snap = self.snapshot();
        let gen_before = snap.generation;
        let claims = extract_claims(&snap);
        let report = run_contradictions(&claims, as_of, limit);
        if self.snapshot.load().generation != gen_before {
            return Err(AppError::SnapshotStale);
        }
        Ok(json!({
            "audit_contradictions": report,
            "generation": gen_before.0,
        }))
    }

    /// `tb_fragility_report`: Hub Fragility & Overloaded MOC `map + satellite` Split Analysis.
    pub fn fragility_report(&self, limit: usize) -> Result<Value, AppError> {
        let snap = self.snapshot();
        let gen_before = snap.generation;
        let kinds: BTreeMap<Locator, Option<String>> = snap
            .files
            .iter()
            .map(|(loc, f)| (loc.clone(), f.declared_type.clone()))
            .collect();
        let report = run_fragility(&snap.graph, &kinds, limit);
        if self.snapshot.load().generation != gen_before {
            return Err(AppError::SnapshotStale);
        }
        Ok(json!({
            "fragility_report": report,
            "generation": gen_before.0,
        }))
    }

    /// `tb_diff_graphs`: structural & topological Difference Graph (`G(T1) -> G(T2)`)
    /// plus Change-Driven Decay candidates (never decays durable types by age alone).
    pub fn diff_graphs(
        &self,
        from_ref: &str,
        to_ref: Option<&str>,
        repo: Option<&str>,
        limit: usize,
    ) -> Result<Value, AppError> {
        let repo_name = repo
            .map(str::to_owned)
            .or_else(|| self.snapshot().members.keys().next().cloned())
            .ok_or_else(|| AppError::NotFound("no workspace member".into()))?;
        let before_snap = self.store.load_snapshot_at_git_ref(&repo_name, from_ref)?;
        let before_meta = snapshot_node_meta(&before_snap);

        let (after_snap, gen_fence): (Arc<VaultSnapshot>, Option<Generation>) = match to_ref {
            Some(r) => (
                Arc::new(self.store.load_snapshot_at_git_ref(&repo_name, r)?),
                None,
            ),
            None => {
                let s = self.snapshot();
                let g = s.generation;
                (s, Some(g))
            }
        };
        let after_meta = snapshot_node_meta(&after_snap);
        let report = run_diff(
            &before_snap.graph,
            &before_meta,
            &after_snap.graph,
            &after_meta,
            limit,
        );
        if let Some(g) = gen_fence
            && self.snapshot.load().generation != g
        {
            return Err(AppError::SnapshotStale);
        }
        Ok(json!({
            "diff_graphs": report,
            "from_ref": from_ref,
            "to_ref": to_ref.unwrap_or("WORKING_TREE"),
            "generation": gen_fence.map(|g| g.0),
        }))
    }

    /// Refresh the snapshot synchronously after a committed mutation.
    fn refresh_after_mutation(&self) {
        let next_gen = self.generation().next();
        if let Ok(Some(fresh)) = self.store.reload_snapshot(next_gen) {
            self.swap_snapshot(fresh);
        }
    }

    /// `tb_create_file`
    pub fn create_file(
        &self,
        locator: &Locator,
        content: &str,
        request_id: Option<&str>,
    ) -> Result<IdempotentOutcome, AppError> {
        let commit_msg = format!("2brain: create {}", locator.path);
        let run = || {
            let receipt = self.store.create_file(locator, content, &commit_msg)?;
            self.refresh_after_mutation();
            Ok(json!({ "create_file": receipt }))
        };
        if let Some(req_id) = request_id {
            let args = json!({ "repo": locator.repo, "path": locator.path, "content": content });
            let pre = json!({});
            let store_loc = locator.clone();
            let content_owned = content.to_owned();
            // Capture store/self via direct execution under outcome port:
            let _ = (store_loc, content_owned);
            return self.execute_with_outcomes(
                req_id,
                "create_file",
                &locator.repo,
                &args,
                &pre,
                run,
            );
        }
        Ok(IdempotentOutcome {
            payload: run()?,
            outcome_state: None,
            replayed: false,
        })
    }

    /// `tb_write_file`
    pub fn write_file(
        &self,
        locator: &Locator,
        expected_hash: &str,
        content: &str,
        request_id: Option<&str>,
    ) -> Result<IdempotentOutcome, AppError> {
        let commit_msg = format!("2brain: write {}", locator.path);
        let run = || {
            let receipt =
                self.store
                    .write_file_cas(locator, expected_hash, content, &commit_msg)?;
            self.refresh_after_mutation();
            Ok(json!({ "write_file": receipt }))
        };
        if let Some(req_id) = request_id {
            let args = json!({ "repo": locator.repo, "path": locator.path, "content": content });
            let pre = json!({ "expected_hash": expected_hash });
            return self.execute_with_outcomes(
                req_id,
                "write_file",
                &locator.repo,
                &args,
                &pre,
                run,
            );
        }
        Ok(IdempotentOutcome {
            payload: run()?,
            outcome_state: None,
            replayed: false,
        })
    }

    /// `tb_edit_file`
    pub fn edit_file(
        &self,
        locator: &Locator,
        expected_hash: &str,
        old_str: &str,
        new_str: &str,
        request_id: Option<&str>,
    ) -> Result<IdempotentOutcome, AppError> {
        let commit_msg = format!("2brain: edit {}", locator.path);
        let run = || {
            let receipt =
                self.store
                    .edit_file_cas(locator, expected_hash, old_str, new_str, &commit_msg)?;
            self.refresh_after_mutation();
            Ok(json!({ "edit_file": receipt }))
        };
        if let Some(req_id) = request_id {
            let args = json!({
                "repo": locator.repo,
                "path": locator.path,
                "old_str": old_str,
                "new_str": new_str,
            });
            let pre = json!({ "expected_hash": expected_hash });
            return self.execute_with_outcomes(
                req_id,
                "edit_file",
                &locator.repo,
                &args,
                &pre,
                run,
            );
        }
        Ok(IdempotentOutcome {
            payload: run()?,
            outcome_state: None,
            replayed: false,
        })
    }

    /// `tb_delete_file`
    pub fn delete_file(
        &self,
        locator: &Locator,
        expected_hash: &str,
        request_id: Option<&str>,
    ) -> Result<IdempotentOutcome, AppError> {
        let commit_msg = format!("2brain: delete {}", locator.path);
        let run = || {
            let commit = self
                .store
                .delete_file_cas(locator, expected_hash, &commit_msg)?;
            self.refresh_after_mutation();
            Ok(json!({
                "delete_file": {
                    "repo": locator.repo,
                    "path": locator.path,
                    "deleted": true,
                    "commit": commit,
                }
            }))
        };
        if let Some(req_id) = request_id {
            let args = json!({ "repo": locator.repo, "path": locator.path });
            let pre = json!({ "expected_hash": expected_hash });
            return self.execute_with_outcomes(
                req_id,
                "delete_file",
                &locator.repo,
                &args,
                &pre,
                run,
            );
        }
        Ok(IdempotentOutcome {
            payload: run()?,
            outcome_state: None,
            replayed: false,
        })
    }

    /// `tb_rename`
    pub fn rename_file(
        &self,
        from: &Locator,
        to: &Locator,
        expected_hash: &str,
        request_id: Option<&str>,
    ) -> Result<IdempotentOutcome, AppError> {
        let commit_msg = format!("2brain: rename {} -> {}", from.path, to.path);
        let run = || {
            let receipt = self
                .store
                .rename_file_cas(from, to, expected_hash, &commit_msg)?;
            self.refresh_after_mutation();
            Ok(json!({ "rename": receipt }))
        };
        if let Some(req_id) = request_id {
            let args = json!({
                "repo": from.repo,
                "path": from.path,
                "new_path": to.path,
            });
            let pre = json!({ "expected_hash": expected_hash });
            return self.execute_with_outcomes(req_id, "rename", &from.repo, &args, &pre, run);
        }
        Ok(IdempotentOutcome {
            payload: run()?,
            outcome_state: None,
            replayed: false,
        })
    }

    fn execute_with_outcomes<F>(
        &self,
        request_id: &str,
        verb: &str,
        workspace: &str,
        arguments: &Value,
        preconditions: &Value,
        action: F,
    ) -> Result<IdempotentOutcome, AppError>
    where
        F: FnOnce() -> Result<Value, AppError>,
    {
        match self
            .outcomes
            .begin_mutation(request_id, verb, workspace, arguments, preconditions)?
        {
            OutcomeBegin::Disabled => {
                let payload = action()?;
                Ok(IdempotentOutcome {
                    payload,
                    outcome_state: None,
                    replayed: false,
                })
            }
            OutcomeBegin::Replay {
                payload,
                outcome_state,
            } => Ok(IdempotentOutcome {
                payload,
                outcome_state: Some(outcome_state),
                replayed: true,
            }),
            OutcomeBegin::Reserved(mut session) => {
                session.record_executing()?;
                match action() {
                    Ok(payload) => {
                        session.finish_committed(&payload)?;
                        Ok(IdempotentOutcome {
                            payload,
                            outcome_state: Some("committed"),
                            replayed: false,
                        })
                    }
                    Err(err) => {
                        // Only record Rejected if failure happened BEFORE disk state changed;
                        // post-effect failures leave the journal in Executing (outcome_unknown)
                        // so callers must reconcile rather than trusting a false Rejected state.
                        if !err.is_post_effect() {
                            let _ = session.finish_rejected(&err.to_string());
                        }
                        Err(err)
                    }
                }
            }
        }
    }
}

/// Uniform command dispatcher shared by in-process [`App<W>`] and remote Unix-socket `tb-ipc` clients.
pub trait AppDispatcher: Send + Sync {
    fn call_read(
        &self,
        verb: &str,
        args: &serde_json::Map<String, Value>,
    ) -> Result<Value, AppError>;

    fn call_mutation(
        &self,
        verb: &str,
        args: &serde_json::Map<String, Value>,
    ) -> Result<IdempotentOutcome, AppError>;

    fn local_snapshot(&self) -> Option<Arc<VaultSnapshot>> {
        None
    }
}

impl<W: WorkspaceStore + 'static> App<W> {
    fn default_repo_name(&self) -> Option<String> {
        self.snapshot().members.keys().next().cloned()
    }

    fn parse_locator_arg(
        &self,
        args: &serde_json::Map<String, Value>,
    ) -> Result<Locator, AppError> {
        let repo = args
            .get("repo")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .or_else(|| self.default_repo_name())
            .ok_or_else(|| AppError::InvalidArguments("'repo' is required".into()))?;
        let path = args
            .get("path")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| AppError::InvalidArguments("'path' is required".into()))?;
        Locator::new(repo, path).map_err(|e| AppError::InvalidArguments(e.to_string()))
    }
}

impl<W: WorkspaceStore + 'static> AppDispatcher for App<W> {
    fn local_snapshot(&self) -> Option<Arc<VaultSnapshot>> {
        Some(self.snapshot())
    }

    fn call_read(
        &self,
        verb: &str,
        args: &serde_json::Map<String, Value>,
    ) -> Result<Value, AppError> {
        let opt_str = |k: &str| {
            args.get(k)
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
        };
        let req_str = |k: &str| {
            opt_str(k).ok_or_else(|| AppError::InvalidArguments(format!("'{k}' is required")))
        };
        let opt_usize = |k: &str, default: usize| {
            args.get(k)
                .and_then(Value::as_u64)
                .map_or(default, |v| v as usize)
        };

        match verb {
            "overview" => Ok(self.overview(opt_str("repo"))),
            "members" => Ok(self.members()),
            "resolve_member" => Ok(self.resolve_member(req_str("path")?)),
            "top_level_dirs" => Ok(self.top_level_dirs(opt_str("repo"))),
            "files" => Ok(self.files(
                opt_str("repo"),
                opt_str("prefix"),
                opt_usize("limit", 50).clamp(1, 500),
                opt_usize("offset", 0),
            )),
            "content" => {
                let loc = self.parse_locator_arg(args)?;
                let req = ContentQuery {
                    locator: loc,
                    offset_chars: opt_usize("offset_chars", 0),
                    max_chars: args
                        .get("max_chars")
                        .and_then(Value::as_u64)
                        .map(|n| n as usize),
                    expected_sha256: args
                        .get("expected")
                        .and_then(Value::as_object)
                        .and_then(|o| o.get("sha256"))
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                };
                Ok(self.content(&req)?.payload)
            }
            "frontmatter" => self.frontmatter(&self.parse_locator_arg(args)?),
            "anchors" => self.anchors(&self.parse_locator_arg(args)?),
            "block_ids" => self.block_ids(&self.parse_locator_arg(args)?),
            "semantic_tokens" => self.semantic_tokens(&self.parse_locator_arg(args)?),
            "types" => Ok(self.types(opt_str("repo"))),
            "type" => {
                let repo = opt_str("repo")
                    .map(str::to_owned)
                    .or_else(|| self.default_repo_name())
                    .ok_or_else(|| AppError::NotFound("no workspace member".into()))?;
                self.type_def(&repo, req_str("name")?)
            }
            "type_tree" => Ok(self.type_tree(opt_str("repo"))),
            "subtypes" => {
                let repo = opt_str("repo")
                    .map(str::to_owned)
                    .or_else(|| self.default_repo_name())
                    .ok_or_else(|| AppError::NotFound("no workspace member".into()))?;
                Ok(self.subtypes(&repo, req_str("name")?))
            }
            "typed" | "instances" => Ok(self.instances(
                opt_str("repo"),
                opt_usize("limit", 50).clamp(1, 500),
                opt_usize("offset", 0),
            )),
            "instances_of" => Ok(self.instances_of(
                opt_str("repo"),
                req_str("type").or_else(|_| req_str("name"))?,
                opt_usize("limit", 50).clamp(1, 500),
                opt_usize("offset", 0),
            )),
            "candidates" => {
                let repo = opt_str("repo")
                    .map(str::to_owned)
                    .or_else(|| self.default_repo_name())
                    .ok_or_else(|| AppError::NotFound("no workspace member".into()))?;
                self.candidates(
                    &repo,
                    req_str("type").or_else(|_| req_str("name"))?,
                    opt_usize("limit", 50).clamp(1, 500),
                    opt_usize("offset", 0),
                )
            }
            "validate_value" => {
                let repo = opt_str("repo")
                    .map(str::to_owned)
                    .or_else(|| self.default_repo_name())
                    .ok_or_else(|| AppError::NotFound("no workspace member".into()))?;
                let val = args
                    .get("value")
                    .ok_or_else(|| AppError::InvalidArguments("'value' is required".into()))?;
                self.validate_value(&repo, req_str("type")?, req_str("field")?, val)
            }
            "diagnostics" => Ok(self.diagnostics(
                opt_str("repo"),
                opt_str("path"),
                opt_usize("limit", 50).clamp(1, 500),
                opt_usize("offset", 0),
            )),
            "search" => Ok(self.search(
                req_str("query")?,
                opt_str("repo"),
                args.get("exact").and_then(Value::as_bool).unwrap_or(false),
                args.get("min_should_match")
                    .and_then(Value::as_u64)
                    .map(|n| n as usize),
                opt_usize("limit", 20).clamp(1, 200),
                opt_usize("offset", 0),
            )),
            "grep" => Ok(self.grep(
                req_str("query")?,
                opt_str("repo"),
                args.get("case_sensitive")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                opt_usize("limit", 20).clamp(1, 200),
                opt_usize("offset", 0),
            )),
            "references" => self.references(&self.parse_locator_arg(args)?),
            "follow" => {
                let repo = opt_str("repo")
                    .map(str::to_owned)
                    .or_else(|| self.default_repo_name())
                    .ok_or_else(|| AppError::NotFound("no workspace member".into()))?;
                Ok(self.follow(&repo, req_str("target")?))
            }
            "resolved" => self.resolved(&self.parse_locator_arg(args)?),
            "children" => self.children(&self.parse_locator_arg(args)?),
            "imports" => Ok(self.imports(opt_str("repo"))),
            "hubs" => Ok(self.hubs(opt_usize("limit", 20).clamp(1, 200))),
            "graph_health" => Ok(self.graph_health(opt_str("repo"))),
            "neighborhood" => self.neighborhood(
                &self.parse_locator_arg(args)?,
                opt_usize("depth", 1).clamp(1, 4) as u8,
                opt_usize("limit", 30).clamp(1, 200),
            ),
            "context" => self.context(
                req_str("query")?,
                opt_str("repo"),
                opt_usize("max_notes", 5),
                opt_usize("max_chars_per_note", 1200),
            ),
            "mutation_outcome" => self.mutation_outcome(req_str("request_id")?),
            "orient" => {
                let suppressed = string_set_arg(args, "suppressed_question_ids");
                self.orient(&suppressed)
            }
            "analyze_topology" => {
                self.analyze_topology(opt_usize("peel_top_k", 5), opt_usize("limit", 10))
            }
            "detect_gaps" => {
                let suppressed = string_set_arg(args, "suppressed_question_ids");
                self.detect_gaps(&suppressed, opt_usize("limit", 15))
            }
            "audit_bias" => {
                let response_text = req_str("response_text")?;
                let default_repo = self.default_repo_name().unwrap_or_default();
                let mut locs = Vec::new();
                if let Some(arr) = args.get("sources").and_then(Value::as_array) {
                    for item in arr {
                        if let Some(obj) = item.as_object()
                            && let Some(path) = obj.get("path").and_then(Value::as_str)
                        {
                            let repo = obj
                                .get("repo")
                                .and_then(Value::as_str)
                                .unwrap_or(&default_repo);
                            if let Ok(loc) = Locator::new(repo, path) {
                                locs.push(loc);
                            }
                        }
                    }
                }
                self.audit_bias(&locs, response_text)
            }
            "audit_contradictions" => {
                self.audit_contradictions(opt_str("as_of"), opt_usize("limit", 50))
            }
            "fragility_report" => self.fragility_report(opt_usize("limit", 20)),
            "diff_graphs" => self.diff_graphs(
                req_str("from_ref")?,
                opt_str("to_ref"),
                opt_str("repo"),
                opt_usize("limit", 20),
            ),
            "preview_mutation" | "reconcile" | "rollback_preview" | "watch" => Ok(json!({
                verb: {
                    "status": "ok",
                    "generation": self.generation().0,
                }
            })),
            other => Err(AppError::InvalidArguments(format!(
                "unknown read verb '{other}'"
            ))),
        }
    }

    fn call_mutation(
        &self,
        verb: &str,
        args: &serde_json::Map<String, Value>,
    ) -> Result<IdempotentOutcome, AppError> {
        let opt_str = |k: &str| {
            args.get(k)
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
        };
        let req_str = |k: &str| {
            opt_str(k).ok_or_else(|| AppError::InvalidArguments(format!("'{k}' is required")))
        };
        let request_id = opt_str("request_id");

        match verb {
            "create_file" => {
                let loc = self.parse_locator_arg(args)?;
                let content = args
                    .get("content")
                    .and_then(Value::as_str)
                    .ok_or_else(|| AppError::InvalidArguments("'content' is required".into()))?;
                self.create_file(&loc, content, request_id)
            }
            "write_file" => {
                let loc = self.parse_locator_arg(args)?;
                let expected_hash = req_str("expected_hash")?;
                let content = args
                    .get("content")
                    .and_then(Value::as_str)
                    .ok_or_else(|| AppError::InvalidArguments("'content' is required".into()))?;
                self.write_file(&loc, expected_hash, content, request_id)
            }
            "edit_file" => {
                let loc = self.parse_locator_arg(args)?;
                let expected_hash = req_str("expected_hash")?;
                let old_str = args
                    .get("old_str")
                    .or_else(|| args.get("old_string"))
                    .and_then(Value::as_str)
                    .ok_or_else(|| AppError::InvalidArguments("'old_str' is required".into()))?;
                let new_str = args
                    .get("new_str")
                    .or_else(|| args.get("new_string"))
                    .and_then(Value::as_str)
                    .ok_or_else(|| AppError::InvalidArguments("'new_str' is required".into()))?;
                self.edit_file(&loc, expected_hash, old_str, new_str, request_id)
            }
            "delete_file" => {
                let loc = self.parse_locator_arg(args)?;
                let expected_hash = req_str("expected_hash")?;
                self.delete_file(&loc, expected_hash, request_id)
            }
            "rename" => {
                let from = self.parse_locator_arg(args)?;
                let expected_hash = req_str("expected_hash")?;
                let new_path = req_str("new_path").or_else(|_| req_str("to"))?;
                let to = Locator::new(&from.repo, new_path)
                    .map_err(|e| AppError::InvalidArguments(e.to_string()))?;
                self.rename_file(&from, &to, expected_hash, request_id)
            }
            other => Err(AppError::Unsupported(format!(
                "mutation verb '{other}' not yet implemented"
            ))),
        }
    }
}

fn string_set_arg(args: &serde_json::Map<String, Value>, key: &str) -> HashSet<String> {
    args.get(key)
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// Extract per-node metadata + SHA-256 revision for Difference Graph & Change-Driven Decay.
#[must_use]
pub fn snapshot_node_meta(snap: &VaultSnapshot) -> Vec<SnapshotNodeMeta> {
    snap.files
        .values()
        .map(|f| SnapshotNodeMeta {
            locator: f.locator.clone(),
            sha256: f.sha256.to_hex(),
            kind: f.declared_type.clone(),
            status: f
                .doc
                .frontmatter
                .as_ref()
                .and_then(|fm| fm.get("status"))
                .and_then(fm_as_str)
                .map(str::to_owned),
        })
        .collect()
}

/// Extract normalized `D03`/`R14` [`ClaimRecord`]s from a [`VaultSnapshot`].
#[must_use]
pub fn extract_claims(snap: &VaultSnapshot) -> Vec<ClaimRecord> {
    let mut claims = Vec::new();
    for f in snap.files.values() {
        let fm = f.doc.frontmatter.as_ref();
        let kind = f.declared_type.clone().or_else(|| {
            fm.and_then(|m| m.get("kind"))
                .and_then(fm_as_str)
                .map(str::to_owned)
        });
        let status = fm
            .and_then(|m| m.get("status"))
            .and_then(fm_as_str)
            .map(str::to_owned);
        let raw_polarity = fm.and_then(|m| m.get("polarity")).and_then(fm_as_str);
        let raw_modality = fm
            .and_then(|m| m.get("modality"))
            .and_then(fm_as_str)
            .or(kind.as_deref());
        let claim_key = fm
            .and_then(|m| m.get("claim_key").or_else(|| m.get("subject")))
            .and_then(fm_as_str)
            .map(str::to_owned);
        let valid_from = fm
            .and_then(|m| {
                m.get("valid_from").and_then(fm_as_str).or_else(|| {
                    if let Some(FrontmatterValue::Map(vt)) = m.get("valid_time") {
                        vt.get("from").and_then(fm_as_str)
                    } else {
                        None
                    }
                })
            })
            .map(str::to_owned);
        let valid_to = fm
            .and_then(|m| {
                m.get("valid_to").and_then(fm_as_str).or_else(|| {
                    if let Some(FrontmatterValue::Map(vt)) = m.get("valid_time") {
                        vt.get("to").and_then(fm_as_str)
                    } else {
                        None
                    }
                })
            })
            .map(str::to_owned);
        let observed_at = fm
            .and_then(|m| m.get("observed_at").or_else(|| m.get("observed_time")))
            .and_then(fm_as_str)
            .map(str::to_owned);

        let mut ambiguous_references = Vec::new();
        let supersedes = fm
            .and_then(|m| m.get("supersedes"))
            .map(|v| {
                resolve_fm_targets(
                    snap,
                    &f.locator.repo,
                    "supersedes",
                    v,
                    &mut ambiguous_references,
                )
            })
            .unwrap_or_default();
        let conflicts_with = fm
            .and_then(|m| m.get("conflicts_with"))
            .map(|v| {
                resolve_fm_targets(
                    snap,
                    &f.locator.repo,
                    "conflicts_with",
                    v,
                    &mut ambiguous_references,
                )
            })
            .unwrap_or_default();

        let relevant = is_claim_relevant(
            kind.as_deref(),
            raw_polarity.is_some(),
            valid_from.is_some() || valid_to.is_some(),
            !supersedes.is_empty()
                || !conflicts_with.is_empty()
                || !ambiguous_references.is_empty(),
        );
        if !relevant {
            continue;
        }

        let polarity = Polarity::parse(raw_polarity);
        let modality = Modality::parse(raw_modality);

        claims.push(ClaimRecord {
            locator: f.locator.clone(),
            sha256: f.sha256.to_hex(),
            kind,
            status,
            claim_key,
            polarity,
            modality,
            valid_from,
            valid_to,
            observed_at,
            supersedes,
            conflicts_with,
            ambiguous_references,
            summary: f.description.clone(),
        });
    }
    claims
}

fn fm_as_str(v: &FrontmatterValue) -> Option<&str> {
    match v {
        FrontmatterValue::String(s) => Some(s.as_str()),
        _ => None,
    }
}

fn resolve_fm_targets(
    snap: &VaultSnapshot,
    default_repo: &str,
    field: &str,
    val: &FrontmatterValue,
    ambiguous_out: &mut Vec<AmbiguousClaimRef>,
) -> Vec<Locator> {
    let mut raw_strings = Vec::new();
    match val {
        FrontmatterValue::String(s) => raw_strings.push(s.as_str()),
        FrontmatterValue::Seq(items) => {
            for item in items {
                if let FrontmatterValue::String(s) = item {
                    raw_strings.push(s.as_str());
                }
            }
        }
        _ => {}
    }
    let mut out = Vec::new();
    for raw in raw_strings {
        let cleaned = raw
            .trim()
            .trim_start_matches("[[")
            .trim_end_matches("]]")
            .split('|')
            .next()
            .unwrap_or("")
            .split('#')
            .next()
            .unwrap_or("")
            .trim();
        if cleaned.is_empty() {
            continue;
        }
        let (target_repo, target_path) = if let Some((t, r)) = cleaned.split_once("::") {
            (r.trim(), t.trim())
        } else {
            (default_repo, cleaned)
        };
        if let Some(loc) = snap
            .path_index
            .get(&(target_repo.to_owned(), target_path.to_owned()))
        {
            out.push(loc.clone());
        } else if let Some(matches) = snap
            .stem_index
            .get(&(target_repo.to_owned(), target_path.to_owned()))
        {
            if matches.len() == 1 {
                out.push(matches[0].clone());
            } else if matches.len() > 1 {
                ambiguous_out.push(AmbiguousClaimRef {
                    field: field.to_owned(),
                    raw_target: cleaned.to_owned(),
                    candidates: matches.clone(),
                });
            }
        } else if let Ok(loc) = Locator::new(target_repo, target_path) {
            out.push(loc);
        }
    }
    out.sort();
    out.dedup();
    out
}

fn file_stem(rel_path: &str) -> &str {
    let file = rel_path.rsplit('/').next().unwrap_or(rel_path);
    file.strip_suffix(".md").unwrap_or(file)
}

fn fm_map_to_json(map: &BTreeMap<String, FrontmatterValue>) -> Value {
    let mut obj = serde_json::Map::new();
    for (k, v) in map {
        obj.insert(k.clone(), fm_val_to_json(v));
    }
    Value::Object(obj)
}

fn fm_val_to_json(v: &FrontmatterValue) -> Value {
    match v {
        FrontmatterValue::Null => Value::Null,
        FrontmatterValue::Bool(b) => json!(b),
        FrontmatterValue::Int(i) => json!(i),
        FrontmatterValue::Float(f) => json!(f),
        FrontmatterValue::String(s) => json!(s),
        FrontmatterValue::Seq(items) => Value::Array(items.iter().map(fm_val_to_json).collect()),
        FrontmatterValue::Map(m) => fm_map_to_json(m),
    }
}

fn json_to_fm_value(v: &Value) -> FrontmatterValue {
    match v {
        Value::Null => FrontmatterValue::Null,
        Value::Bool(b) => FrontmatterValue::Bool(*b),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                FrontmatterValue::Int(i)
            } else {
                FrontmatterValue::Float(n.as_f64().unwrap_or(0.0))
            }
        }
        Value::String(s) => FrontmatterValue::String(s.clone()),
        Value::Array(arr) => FrontmatterValue::Seq(arr.iter().map(json_to_fm_value).collect()),
        Value::Object(obj) => {
            let mut m = BTreeMap::new();
            for (k, val) in obj {
                m.insert(k.clone(), json_to_fm_value(val));
            }
            FrontmatterValue::Map(m)
        }
    }
}
