//! Application layer: hexagonal ports (traits), immutable `VaultSnapshot`,
//! `ArcSwap` lock-free serving, and complete read-core use cases (0 I/O).
//!
//! Depends ONLY on `crates/domain/*`. Concrete I/O adapters live in
//! `crates/adapters/*` and are wired once in `crates/bin/two-brain`.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use arc_swap::ArcSwap;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
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
    #[error("symlink escapes member root: {0}")]
    SymlinkEscape(String),
    #[error("invalid UTF-8 at {0}")]
    InvalidUtf8(String),
    #[error("I/O error: {0}")]
    Io(String),
    #[error("unsupported in current mode: {0}")]
    Unsupported(String),
}

/// Application-layer error with explicit machine-readable stage/reason codes.
#[derive(Debug, Error)]
pub enum AppError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("policy/path_scope: symlink escapes member root ({0})")]
    SymlinkEscape(String),
    #[error("invalid_utf8: file contains non-UTF-8 bytes ({0})")]
    InvalidUtf8(String),
    #[error("invalid_arguments: {0}")]
    InvalidArguments(String),
    #[error("snapshot_stale: generation advanced during release admission check")]
    SnapshotStale,
    #[error("io_error: {0}")]
    Io(String),
    #[error("unsupported: {0}")]
    Unsupported(String),
}

impl From<StoreError> for AppError {
    fn from(err: StoreError) -> Self {
        match err {
            StoreError::NotFound(s) => Self::NotFound(s),
            StoreError::SymlinkEscape(s) => Self::SymlinkEscape(s),
            StoreError::InvalidUtf8(s) => Self::InvalidUtf8(s),
            StoreError::Io(s) => Self::Io(s),
            StoreError::Unsupported(s) => Self::Unsupported(s),
        }
    }
}

/// Driven port for workspace filesystem and Git access.
pub trait WorkspaceStore: Send + Sync {
    fn read_exact(&self, locator: &Locator) -> Result<(Vec<u8>, EvidenceReceipt), StoreError>;
    fn commit_verified(&self, intent: WriteIntent<CasVerified>) -> Result<String, StoreError>;
    /// Optional live snapshot reload when external edits advance disk state past the cached snapshot.
    fn reload_snapshot(
        &self,
        _next_generation: Generation,
    ) -> Result<Option<VaultSnapshot>, StoreError> {
        Ok(None)
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
}

impl<W: WorkspaceStore> App<W> {
    #[must_use]
    pub fn new(store: W, initial: VaultSnapshot) -> Self {
        Self {
            store,
            snapshot: ArcSwap::from_pointee(initial),
        }
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

    /// `tb_mutation_outcome` (Phase 2: read-only fence OFF until Phase 3 wires `tb-journal`).
    pub fn mutation_outcome(&self, request_id: &str) -> Value {
        json!({
            "mutation_outcome": {
                "enabled": false,
                "present": false,
                "request_id": request_id,
                "state": "unknown",
                "replay": "absent",
            }
        })
    }
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
