//! Driven adapter: read-only workspace scanner with member confinement (SEM-18),
//! strict UTF-8 decode reporting (SEM-04), and `.2brain/ignore` filtering.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::ffi::OsString;
use std::fs;
use std::io::Read;
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};

use ignore::gitignore::{Gitignore, GitignoreBuilder};
use rustix::fs::{Mode, OFlags, open, openat, readlinkat};
use rustix::io::Errno;
use saphyr::{LoadableYamlNode, Scalar, Yaml};
use tb_app::{IndexedFile, MemberMeta, StoreError, VaultSnapshot, WorkspaceStore};
use tb_contracts::{
    CasVerified, EvidenceReceipt, Generation, Locator, RevisionDigest, WriteGuardHash, WriteIntent,
};
use tb_graph::{GraphHealth, GraphShape, HubRow, TypedLinkGraph};
use tb_schema::{Diagnostic, SchemaRegistry, Severity, parse_type_def};
use tb_search::SearchIndex;
use tb_syntax::{EdgeKind, FrontmatterValue, ParsedDocument, parse_document};
use thiserror::Error;

/// Read-only (Phase 2) / read-write (Phase 3) confined filesystem workspace store.
#[derive(Clone, Debug)]
pub struct FsWorkspaceStore {
    members: BTreeMap<String, PathBuf>,
}

impl FsWorkspaceStore {
    #[must_use]
    pub fn single(repo_name: impl Into<String>, canonical_root: PathBuf) -> Self {
        let mut members = BTreeMap::new();
        members.insert(repo_name.into(), canonical_root);
        Self { members }
    }

    #[must_use]
    pub fn member_root(&self, repo: &str) -> Option<&Path> {
        self.members.get(repo).map(PathBuf::as_path)
    }
}

impl WorkspaceStore for FsWorkspaceStore {
    fn read_exact(&self, locator: &Locator) -> Result<(Vec<u8>, EvidenceReceipt), StoreError> {
        let Some(canonical_root) = self.members.get(&locator.repo) else {
            return Err(StoreError::NotFound(format!(
                "unknown member '{}'",
                locator.repo
            )));
        };
        let candidate = canonical_root.join(&locator.path);
        let bytes = match confined_read_bytes(canonical_root, &candidate) {
            Ok(Some(b)) => b,
            Ok(None) => {
                return Err(StoreError::NotFound(format!(
                    "{}/{}",
                    locator.repo, locator.path
                )));
            }
            Err(FsError::SymlinkEscape(p)) => {
                return Err(StoreError::SymlinkEscape(p.display().to_string()));
            }
            Err(FsError::Io { path, source }) => {
                return Err(StoreError::Io(format!("{}: {source}", path.display())));
            }
        };
        let sha256 = RevisionDigest::of_bytes(&bytes);
        let write_guard = WriteGuardHash::of_bytes(&bytes);
        let total_bytes = bytes.len();
        let receipt = EvidenceReceipt {
            locator: locator.clone(),
            sha256,
            write_guard,
            total_bytes,
            byte_start: 0,
            byte_end: total_bytes,
            truncated: false,
            generation: Generation(0),
        };
        Ok((bytes, receipt))
    }

    fn commit_verified(&self, _intent: WriteIntent<CasVerified>) -> Result<String, StoreError> {
        Err(StoreError::Unsupported(
            "mutations are wired in Phase 3".to_owned(),
        ))
    }

    fn reload_snapshot(
        &self,
        next_generation: Generation,
    ) -> Result<Option<VaultSnapshot>, StoreError> {
        let Some(root) = self.members.values().next() else {
            return Ok(None);
        };
        let snap = load_snapshot_with_generation(root, next_generation)
            .map_err(|e| StoreError::Io(e.to_string()))?;
        Ok(Some(snap))
    }
}

/// Build an immutable [`VaultSnapshot`] and its confined [`FsWorkspaceStore`] from `root`.
pub fn load_snapshot(root: &Path) -> Result<(FsWorkspaceStore, VaultSnapshot), FsError> {
    load_snapshot_inner(root, Generation(1))
}

/// Reload an immutable [`VaultSnapshot`] at an explicit monotonically advancing [`Generation`].
pub fn load_snapshot_with_generation(
    root: &Path,
    generation: Generation,
) -> Result<VaultSnapshot, FsError> {
    load_snapshot_inner(root, generation).map(|(_, snap)| snap)
}

fn load_snapshot_inner(
    root: &Path,
    generation: Generation,
) -> Result<(FsWorkspaceStore, VaultSnapshot), FsError> {
    let canonical_root = fs::canonicalize(root).map_err(|source| FsError::Io {
        path: root.to_path_buf(),
        source,
    })?;

    let mut all_diags: Vec<Diagnostic> = Vec::new();
    let mut utf8_errors = 0usize;

    let repo_name = read_repo_name(&canonical_root)?.unwrap_or_else(|| {
        canonical_root
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("workspace")
            .to_owned()
    });
    let gitignore = load_ignore_rules(
        &canonical_root,
        &repo_name,
        &mut all_diags,
        &mut utf8_errors,
    )?;

    let mut registry = SchemaRegistry::new();
    let type_dir = canonical_root.join("type");
    if let Some(real_type_dir) = confined_dir(&canonical_root, &type_dir)? {
        let mut type_entries: Vec<PathBuf> = fs::read_dir(&real_type_dir)
            .map_err(|source| FsError::Io {
                path: real_type_dir.clone(),
                source,
            })?
            .filter_map(|e| e.ok().map(|entry| entry.path()))
            .collect();
        type_entries.sort();

        for path in type_entries {
            let Some(fname) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let Some(stem) = fname.strip_suffix(".type.yaml") else {
                continue;
            };
            if gitignore
                .matched_path_or_any_parents(&path, false)
                .is_ignore()
            {
                continue;
            }
            let rel_posix = rel_posix_path(&canonical_root, &path);
            match confined_read_bytes(&canonical_root, &path) {
                Ok(Some(bytes)) => match std::str::from_utf8(&bytes) {
                    Ok(raw) => match parse_type_def(&repo_name, stem, raw) {
                        Ok(def) => registry.insert(def),
                        Err(err) => all_diags.push(Diagnostic {
                            repo: repo_name.clone(),
                            path: rel_posix,
                            byte_start: 0,
                            code: "invalid-type-def".to_owned(),
                            severity: Severity::Error,
                            message: err.to_string(),
                        }),
                    },
                    Err(_) => all_diags.push(Diagnostic {
                        repo: repo_name.clone(),
                        path: rel_posix,
                        byte_start: 0,
                        code: "invalid-utf8".to_owned(),
                        severity: Severity::Error,
                        message: "type definition contains invalid UTF-8 bytes".to_owned(),
                    }),
                },
                Ok(None) => {}
                Err(FsError::SymlinkEscape(_)) => all_diags.push(Diagnostic {
                    repo: repo_name.clone(),
                    path: rel_posix,
                    byte_start: 0,
                    code: "symlink-escape".to_owned(),
                    severity: Severity::Error,
                    message: "type definition symlink escapes member root".to_owned(),
                }),
                Err(FsError::Io { source, .. }) => all_diags.push(Diagnostic {
                    repo: repo_name.clone(),
                    path: rel_posix,
                    byte_start: 0,
                    code: "type-read-error".to_owned(),
                    severity: Severity::Error,
                    message: format!("failed to read type definition: {source}"),
                }),
            }
        }
    }

    let mut stack = vec![canonical_root.clone()];
    let mut files: BTreeMap<Locator, IndexedFile> = BTreeMap::new();
    let mut path_index: BTreeMap<(String, String), Locator> = BTreeMap::new();
    let mut stem_index: BTreeMap<(String, String), Vec<Locator>> = BTreeMap::new();

    while let Some(dir) = stack.pop() {
        let mut dir_entries: Vec<fs::DirEntry> = Vec::new();
        for entry in fs::read_dir(&dir).map_err(|source| FsError::Io {
            path: dir.clone(),
            source,
        })? {
            dir_entries.push(entry.map_err(|source| FsError::Io {
                path: dir.clone(),
                source,
            })?);
        }
        dir_entries.sort_by_key(|e| e.file_name());

        for entry in dir_entries {
            let path = entry.path();
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str == ".git" {
                continue;
            }
            if dir == canonical_root.join(".2brain") && name_str == "engine" {
                continue;
            }
            let Ok(ft) = entry.file_type() else {
                continue;
            };
            let is_dir = ft.is_dir();
            if gitignore
                .matched_path_or_any_parents(&path, is_dir)
                .is_ignore()
            {
                continue;
            }
            if ft.is_symlink() {
                if !is_confined(&canonical_root, &path)? {
                    continue;
                }
                if path.is_dir() {
                    continue;
                }
            }
            if is_dir {
                stack.push(path);
            } else if path.extension().and_then(|e| e.to_str()) == Some("md") {
                let Ok(rel) = path.strip_prefix(&canonical_root) else {
                    continue;
                };
                let rel_posix = rel.to_string_lossy().replace('\\', "/");
                let Ok(loc) = Locator::new(&repo_name, &rel_posix) else {
                    continue;
                };
                let bytes = match confined_read_bytes(&canonical_root, &path) {
                    Ok(Some(b)) => b,
                    Ok(None) | Err(FsError::SymlinkEscape(_)) => continue,
                    Err(err) => return Err(err),
                };
                if bytes.iter().take(8192).any(|&b| b == 0) {
                    continue;
                }
                let sha256 = RevisionDigest::of_bytes(&bytes);
                let short_hash = WriteGuardHash::of_bytes(&bytes).to_hex();
                match parse_document(&bytes) {
                    Ok(doc) => {
                        let diags = registry.validate_document(&repo_name, &rel_posix, &doc);
                        all_diags.extend(diags);
                        let raw_text = String::from_utf8(bytes).unwrap_or_default();
                        let description =
                            doc.frontmatter
                                .as_ref()
                                .and_then(|fm| match fm.get("description") {
                                    Some(FrontmatterValue::String(s)) => Some(s.clone()),
                                    _ => None,
                                });
                        let declared_type =
                            doc.frontmatter
                                .as_ref()
                                .and_then(|fm| match fm.get("type") {
                                    Some(FrontmatterValue::String(s)) => Some(s.clone()),
                                    _ => None,
                                });
                        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                            stem_index
                                .entry((repo_name.clone(), stem.to_owned()))
                                .or_default()
                                .push(loc.clone());
                        }
                        path_index.insert((repo_name.clone(), rel_posix.clone()), loc.clone());
                        if let Some(no_ext) = rel_posix.strip_suffix(".md") {
                            path_index.insert((repo_name.clone(), no_ext.to_owned()), loc.clone());
                        }
                        files.insert(
                            loc.clone(),
                            IndexedFile {
                                locator: loc,
                                sha256,
                                short_hash,
                                size_bytes: raw_text.len(),
                                raw_text,
                                doc,
                                description,
                                declared_type,
                            },
                        );
                    }
                    Err(_) => {
                        all_diags.push(Diagnostic {
                            repo: repo_name.clone(),
                            path: rel_posix,
                            byte_start: 0,
                            code: "invalid-utf8".to_owned(),
                            severity: Severity::Error,
                            message: "file contains invalid UTF-8 bytes".to_owned(),
                        });
                    }
                }
            }
        }
    }

    for matches in stem_index.values_mut() {
        matches.sort();
    }

    let mut locators = Vec::with_capacity(files.len());
    let mut edges: Vec<(Locator, Locator, EdgeKind)> = Vec::new();
    for (loc, file) in &files {
        locators.push(loc.clone());
        for link in &file.doc.wikilinks {
            if let Some(target_repo) = &link.repo
                && target_repo != &repo_name
            {
                continue;
            }
            if let Some(target_loc) = path_index.get(&(repo_name.clone(), link.target.clone())) {
                edges.push((loc.clone(), target_loc.clone(), link.kind));
            } else if let Some(matches) = stem_index.get(&(repo_name.clone(), link.target.clone()))
                && matches.len() == 1
            {
                edges.push((loc.clone(), matches[0].clone(), link.kind));
            }
        }
    }

    let graph = TypedLinkGraph::build(locators, &edges);
    let search = SearchIndex::build(files.values().map(|f| {
        (
            f.locator.clone(),
            f.body(),
            f.description.as_deref().unwrap_or(""),
        )
    }));

    let mut members = BTreeMap::new();
    members.insert(
        repo_name.clone(),
        MemberMeta {
            repo: repo_name.clone(),
            root: canonical_root.display().to_string(),
            role: "primary".to_owned(),
            deps: registry.deps_of(&repo_name),
        },
    );

    let store = FsWorkspaceStore::single(repo_name, canonical_root);
    let snapshot = VaultSnapshot {
        generation,
        members,
        files,
        registry,
        graph,
        search,
        diagnostics: all_diags,
        path_index,
        stem_index,
    };

    Ok((store, snapshot))
}

#[derive(Debug, Error)]
pub enum FsError {
    #[error("I/O error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("symlink escapes member root: {0}")]
    SymlinkEscape(PathBuf),
}

#[derive(Clone, Debug)]
pub struct CheckSummary {
    pub repo_name: String,
    pub types_loaded: usize,
    pub markdown_files: usize,
    pub schema_diagnostics: usize,
    pub broken_links: usize,
    pub ambiguous_links: usize,
    pub unresolved_qualified_links: usize,
    pub utf8_errors: usize,
    pub symlink_refusals: usize,
    pub diagnostics: Vec<Diagnostic>,
    pub shape: GraphShape,
    pub health: GraphHealth,
    pub top_hubs: Vec<HubRow>,
}

/// Read-only scan, parse, validate, and CSR graph build for a single folder-repo.
pub fn check_workspace(root: &Path) -> Result<CheckSummary, FsError> {
    let canonical_root = fs::canonicalize(root).map_err(|source| FsError::Io {
        path: root.to_path_buf(),
        source,
    })?;

    let mut all_diags: Vec<Diagnostic> = Vec::new();
    let mut utf8_errors = 0usize;
    let mut symlink_refusals = 0usize;

    // 0. Read .2brain/repo.yaml and .2brain/ignore strictly inside canonical_root (SEM-18).
    let repo_name = read_repo_name(&canonical_root)?.unwrap_or_else(|| {
        canonical_root
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("workspace")
            .to_owned()
    });
    let gitignore = load_ignore_rules(
        &canonical_root,
        &repo_name,
        &mut all_diags,
        &mut utf8_errors,
    )?;

    // 1. Load type/*.type.yaml (confined, strict UTF-8, explicit schema diagnostics on failure).
    let mut registry = SchemaRegistry::new();
    let type_dir = canonical_root.join("type");
    if let Some(real_type_dir) = confined_dir(&canonical_root, &type_dir)? {
        let mut type_entries: Vec<PathBuf> = fs::read_dir(&real_type_dir)
            .map_err(|source| FsError::Io {
                path: real_type_dir.clone(),
                source,
            })?
            .filter_map(|e| e.ok().map(|entry| entry.path()))
            .collect();
        type_entries.sort();

        for path in type_entries {
            let Some(fname) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let Some(stem) = fname.strip_suffix(".type.yaml") else {
                continue;
            };
            if gitignore
                .matched_path_or_any_parents(&path, false)
                .is_ignore()
            {
                continue;
            }
            let rel_posix = rel_posix_path(&canonical_root, &path);
            match confined_read_bytes(&canonical_root, &path) {
                Ok(Some(bytes)) => match std::str::from_utf8(&bytes) {
                    Ok(raw) => match parse_type_def(&repo_name, stem, raw) {
                        Ok(def) => registry.insert(def),
                        Err(err) => {
                            all_diags.push(Diagnostic {
                                repo: repo_name.clone(),
                                path: rel_posix,
                                byte_start: 0,
                                code: "invalid-type-def".to_owned(),
                                severity: Severity::Error,
                                message: err.to_string(),
                            });
                        }
                    },
                    Err(_) => {
                        utf8_errors += 1;
                        all_diags.push(Diagnostic {
                            repo: repo_name.clone(),
                            path: rel_posix,
                            byte_start: 0,
                            code: "invalid-utf8".to_owned(),
                            severity: Severity::Error,
                            message: "type definition contains invalid UTF-8 bytes".to_owned(),
                        });
                    }
                },
                Ok(None) => {}
                Err(FsError::SymlinkEscape(_)) => {
                    symlink_refusals += 1;
                    all_diags.push(Diagnostic {
                        repo: repo_name.clone(),
                        path: rel_posix,
                        byte_start: 0,
                        code: "symlink-escape".to_owned(),
                        severity: Severity::Error,
                        message: "type definition symlink escapes member root".to_owned(),
                    });
                }
                Err(FsError::Io { source, .. }) => {
                    all_diags.push(Diagnostic {
                        repo: repo_name.clone(),
                        path: rel_posix,
                        byte_start: 0,
                        code: "type-read-error".to_owned(),
                        severity: Severity::Error,
                        message: format!("failed to read type definition: {source}"),
                    });
                }
            }
        }
    } else if fs::symlink_metadata(&type_dir).is_ok() {
        // `type` exists as an escaping symlink or broken symlink.
        let meta = fs::symlink_metadata(&type_dir).map_err(|source| FsError::Io {
            path: type_dir.clone(),
            source,
        })?;
        if meta.file_type().is_symlink() {
            symlink_refusals += 1;
            all_diags.push(Diagnostic {
                repo: repo_name.clone(),
                path: "type".to_owned(),
                byte_start: 0,
                code: "symlink-escape".to_owned(),
                severity: Severity::Error,
                message: "type directory symlink escapes member root".to_owned(),
            });
        }
    }

    // 2. Walk markdown files (excluding .git, .2brain/engine, and .2brain/ignore patterns).
    let mut stack = vec![canonical_root.clone()];
    let mut parsed_files: Vec<(Locator, ParsedDocument)> = Vec::new();
    let mut stem_index: HashMap<String, Vec<Locator>> = HashMap::new();
    let mut path_index: HashMap<String, Locator> = HashMap::new();

    while let Some(dir) = stack.pop() {
        let mut dir_entries: Vec<fs::DirEntry> = Vec::new();
        for entry in fs::read_dir(&dir).map_err(|source| FsError::Io {
            path: dir.clone(),
            source,
        })? {
            dir_entries.push(entry.map_err(|source| FsError::Io {
                path: dir.clone(),
                source,
            })?);
        }
        dir_entries.sort_by_key(|e| e.file_name());

        for entry in dir_entries {
            let path = entry.path();
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str == ".git" {
                continue;
            }
            if dir == canonical_root.join(".2brain") && name_str == "engine" {
                continue;
            }
            let Ok(ft) = entry.file_type() else {
                continue;
            };
            let is_dir = ft.is_dir();
            if gitignore
                .matched_path_or_any_parents(&path, is_dir)
                .is_ignore()
            {
                continue;
            }
            if ft.is_symlink() {
                if !is_confined(&canonical_root, &path)? {
                    symlink_refusals += 1;
                    continue;
                }
                if path.is_dir() {
                    continue;
                }
            }
            if is_dir {
                stack.push(path);
            } else if path.extension().and_then(|e| e.to_str()) == Some("md") {
                let Ok(rel) = path.strip_prefix(&canonical_root) else {
                    continue;
                };
                let rel_posix = rel.to_string_lossy().replace('\\', "/");
                let Ok(loc) = Locator::new(&repo_name, &rel_posix) else {
                    continue;
                };
                let bytes = match confined_read_bytes(&canonical_root, &path) {
                    Ok(Some(b)) => b,
                    Ok(None) => continue,
                    Err(FsError::SymlinkEscape(_)) => {
                        symlink_refusals += 1;
                        continue;
                    }
                    Err(err) => return Err(err),
                };
                if bytes.iter().take(8192).any(|&b| b == 0) {
                    continue;
                }
                match parse_document(&bytes) {
                    Ok(doc) => {
                        let diags = registry.validate_document(&repo_name, &rel_posix, &doc);
                        all_diags.extend(diags);
                        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                            stem_index
                                .entry(stem.to_owned())
                                .or_default()
                                .push(loc.clone());
                        }
                        path_index.insert(rel_posix.clone(), loc.clone());
                        if let Some(no_ext) = rel_posix.strip_suffix(".md") {
                            path_index.insert(no_ext.to_owned(), loc.clone());
                        }
                        parsed_files.push((loc, doc));
                    }
                    Err(_) => {
                        utf8_errors += 1;
                        all_diags.push(Diagnostic {
                            repo: repo_name.clone(),
                            path: rel_posix,
                            byte_start: 0,
                            code: "invalid-utf8".to_owned(),
                            severity: Severity::Error,
                            message: "file contains invalid UTF-8 bytes".to_owned(),
                        });
                    }
                }
            }
        }
    }

    // Deterministic order across all files and stem buckets.
    parsed_files.sort_by(|a, b| a.0.cmp(&b.0));
    for matches in stem_index.values_mut() {
        matches.sort();
    }

    let schema_diagnostics = all_diags.len();

    // 3. Resolve wikilinks and build CSR TypedLinkGraph.
    let mut locators = Vec::with_capacity(parsed_files.len());
    let mut edges: Vec<(Locator, Locator, EdgeKind)> = Vec::new();
    let mut broken_links = 0usize;
    let mut ambiguous_links = 0usize;
    let mut unresolved_qualified_links = 0usize;

    for (loc, doc) in &parsed_files {
        locators.push(loc.clone());
        for link in &doc.wikilinks {
            if let Some(target_repo) = &link.repo
                && target_repo != &repo_name
            {
                unresolved_qualified_links += 1;
                all_diags.push(Diagnostic {
                    repo: repo_name.clone(),
                    path: loc.path.clone(),
                    byte_start: link.byte_start,
                    code: "unresolved-qualified-link".to_owned(),
                    severity: Severity::Warning,
                    message: format!(
                        "qualified wikilink '[[{}::{}]]' references repository '{}' outside single-member scope",
                        link.target, target_repo, target_repo
                    ),
                });
                continue;
            }
            if let Some(target_loc) = path_index.get(&link.target) {
                edges.push((loc.clone(), target_loc.clone(), link.kind));
            } else if let Some(matches) = stem_index.get(&link.target) {
                if matches.len() == 1 {
                    edges.push((loc.clone(), matches[0].clone(), link.kind));
                } else {
                    ambiguous_links += 1;
                    all_diags.push(Diagnostic {
                        repo: repo_name.clone(),
                        path: loc.path.clone(),
                        byte_start: link.byte_start,
                        code: "ambiguous-link".to_owned(),
                        severity: Severity::Warning,
                        message: format!(
                            "wikilink '[[{}]]' matches {} files; qualify with relative path",
                            link.target,
                            matches.len()
                        ),
                    });
                }
            } else {
                broken_links += 1;
            }
        }
    }

    let graph = TypedLinkGraph::build(locators, &edges);
    let shape = graph.shape();
    let health = graph.health();
    let top_hubs = graph.hubs(5);

    Ok(CheckSummary {
        repo_name,
        types_loaded: registry.len(),
        markdown_files: parsed_files.len(),
        schema_diagnostics,
        broken_links,
        ambiguous_links,
        unresolved_qualified_links,
        utf8_errors,
        symlink_refusals,
        diagnostics: all_diags,
        shape,
        health,
        top_hubs,
    })
}

fn rel_posix_path(canonical_root: &Path, path: &Path) -> String {
    path.strip_prefix(canonical_root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn is_confined(canonical_root: &Path, path: &Path) -> Result<bool, FsError> {
    match fs::canonicalize(path) {
        Ok(real) => Ok(real.starts_with(canonical_root)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(FsError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn confined_dir(canonical_root: &Path, path: &Path) -> Result<Option<PathBuf>, FsError> {
    match fs::symlink_metadata(path) {
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(FsError::Io {
                path: path.to_path_buf(),
                source,
            });
        }
        Ok(_) => {}
    }
    match fs::canonicalize(path) {
        Ok(real) if real.starts_with(canonical_root) && real.is_dir() => Ok(Some(real)),
        Ok(_) => Ok(None),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(FsError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

/// Read a file via dirfd-relative `openat(..., O_NOFOLLOW)` starting from `canonical_root`,
/// eliminating TOCTOU path-replacement windows between confinement validation and reading.
fn confined_read_bytes(canonical_root: &Path, path: &Path) -> Result<Option<Vec<u8>>, FsError> {
    const MAX_SYMLINK_HOPS: usize = 16;

    let Ok(rel) = path.strip_prefix(canonical_root) else {
        return Err(FsError::SymlinkEscape(path.to_path_buf()));
    };
    let root_fd: OwnedFd = open(
        canonical_root,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|errno| FsError::Io {
        path: canonical_root.to_path_buf(),
        source: std::io::Error::from_raw_os_error(errno.raw_os_error()),
    })?;

    let mut queue: VecDeque<OsString> = VecDeque::new();
    for comp in rel.components() {
        match comp {
            Component::Normal(seg) => queue.push_back(seg.to_os_string()),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(FsError::SymlinkEscape(path.to_path_buf()));
            }
        }
    }
    if queue.is_empty() {
        return Ok(None);
    }

    let mut dir_stack: Vec<OwnedFd> = Vec::new();
    let mut hops = 0usize;

    while let Some(seg) = queue.pop_front() {
        if seg == "." {
            continue;
        }
        if seg == ".." {
            if dir_stack.pop().is_none() {
                return Err(FsError::SymlinkEscape(path.to_path_buf()));
            }
            continue;
        }
        let cur_fd = dir_stack.last().map_or(root_fd.as_fd(), OwnedFd::as_fd);
        let is_last = queue.is_empty();

        if !is_last {
            match openat(
                cur_fd,
                seg.as_os_str(),
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            ) {
                Ok(child_fd) => dir_stack.push(child_fd),
                Err(Errno::NOENT) => return Ok(None),
                Err(Errno::LOOP | Errno::NOTDIR) => {
                    match readlinkat(cur_fd, seg.as_os_str(), Vec::new()) {
                        Ok(target_cstr) => {
                            hops += 1;
                            if hops > MAX_SYMLINK_HOPS {
                                return Err(FsError::SymlinkEscape(path.to_path_buf()));
                            }
                            let target_os = std::ffi::OsStr::from_bytes(target_cstr.as_bytes());
                            expand_confined_symlink(
                                canonical_root,
                                Path::new(target_os),
                                path,
                                &mut dir_stack,
                                &mut queue,
                            )?;
                        }
                        Err(Errno::INVAL) => return Ok(None),
                        Err(Errno::NOENT) => return Ok(None),
                        Err(errno) => {
                            return Err(FsError::Io {
                                path: path.to_path_buf(),
                                source: std::io::Error::from_raw_os_error(errno.raw_os_error()),
                            });
                        }
                    }
                }
                Err(errno) => {
                    return Err(FsError::Io {
                        path: path.to_path_buf(),
                        source: std::io::Error::from_raw_os_error(errno.raw_os_error()),
                    });
                }
            }
        } else {
            match openat(
                cur_fd,
                seg.as_os_str(),
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
                Mode::empty(),
            ) {
                Ok(fd) => {
                    let mut file = fs::File::from(fd);
                    let meta = file.metadata().map_err(|source| FsError::Io {
                        path: path.to_path_buf(),
                        source,
                    })?;
                    if !meta.is_file() {
                        return Ok(None);
                    }
                    let mut buf = Vec::with_capacity(meta.len() as usize);
                    file.read_to_end(&mut buf).map_err(|source| FsError::Io {
                        path: path.to_path_buf(),
                        source,
                    })?;
                    return Ok(Some(buf));
                }
                Err(Errno::NOENT) => return Ok(None),
                Err(Errno::LOOP) => {
                    let target_cstr =
                        readlinkat(cur_fd, seg.as_os_str(), Vec::new()).map_err(|errno| {
                            FsError::Io {
                                path: path.to_path_buf(),
                                source: std::io::Error::from_raw_os_error(errno.raw_os_error()),
                            }
                        })?;
                    hops += 1;
                    if hops > MAX_SYMLINK_HOPS {
                        return Err(FsError::SymlinkEscape(path.to_path_buf()));
                    }
                    let target_os = std::ffi::OsStr::from_bytes(target_cstr.as_bytes());
                    expand_confined_symlink(
                        canonical_root,
                        Path::new(target_os),
                        path,
                        &mut dir_stack,
                        &mut queue,
                    )?;
                }
                Err(errno) => {
                    return Err(FsError::Io {
                        path: path.to_path_buf(),
                        source: std::io::Error::from_raw_os_error(errno.raw_os_error()),
                    });
                }
            }
        }
    }
    Ok(None)
}

fn expand_confined_symlink(
    canonical_root: &Path,
    target: &Path,
    orig_path: &Path,
    dir_stack: &mut Vec<OwnedFd>,
    queue: &mut VecDeque<OsString>,
) -> Result<(), FsError> {
    let mut prefix: Vec<OsString> = Vec::new();
    if target.is_absolute() {
        let Ok(stripped) = target.strip_prefix(canonical_root) else {
            return Err(FsError::SymlinkEscape(orig_path.to_path_buf()));
        };
        dir_stack.clear();
        for comp in stripped.components() {
            match comp {
                Component::Normal(s) => prefix.push(s.to_os_string()),
                Component::ParentDir => prefix.push(OsString::from("..")),
                Component::CurDir => {}
                Component::RootDir | Component::Prefix(_) => {
                    return Err(FsError::SymlinkEscape(orig_path.to_path_buf()));
                }
            }
        }
    } else {
        for comp in target.components() {
            match comp {
                Component::Normal(s) => prefix.push(s.to_os_string()),
                Component::ParentDir => prefix.push(OsString::from("..")),
                Component::CurDir => {}
                Component::RootDir | Component::Prefix(_) => {
                    return Err(FsError::SymlinkEscape(orig_path.to_path_buf()));
                }
            }
        }
    }
    for seg in prefix.into_iter().rev() {
        queue.push_front(seg);
    }
    Ok(())
}

fn read_repo_name(canonical_root: &Path) -> Result<Option<String>, FsError> {
    let manifest_path = canonical_root.join(".2brain/repo.yaml");
    let Some(bytes) = confined_read_bytes(canonical_root, &manifest_path)? else {
        return Ok(None);
    };
    let Ok(raw) = std::str::from_utf8(&bytes) else {
        return Ok(None);
    };
    let Ok(docs) = Yaml::load_from_str(raw) else {
        return Ok(None);
    };
    let Some(Yaml::Mapping(map)) = docs.into_iter().next() else {
        return Ok(None);
    };
    for (k, v) in map {
        if matches!(k, Yaml::Value(Scalar::String(ref s)) if s == "name")
            && let Yaml::Value(Scalar::String(val)) = v
        {
            return Ok(Some(val.into_owned()));
        }
    }
    Ok(None)
}

fn load_ignore_rules(
    canonical_root: &Path,
    repo_name: &str,
    diags: &mut Vec<Diagnostic>,
    utf8_errors: &mut usize,
) -> Result<Gitignore, FsError> {
    let mut builder = GitignoreBuilder::new(canonical_root);
    let ignore_path = canonical_root.join(".2brain/ignore");
    if let Some(bytes) = confined_read_bytes(canonical_root, &ignore_path)? {
        match std::str::from_utf8(&bytes) {
            Ok(raw) => {
                for (idx, line) in raw.lines().enumerate() {
                    if let Err(err) = builder.add_line(None, line) {
                        diags.push(Diagnostic {
                            repo: repo_name.to_owned(),
                            path: ".2brain/ignore".to_owned(),
                            byte_start: idx,
                            code: "invalid-ignore-pattern".to_owned(),
                            severity: Severity::Warning,
                            message: format!("invalid ignore pattern on line {}: {err}", idx + 1),
                        });
                    }
                }
            }
            Err(_) => {
                *utf8_errors += 1;
                diags.push(Diagnostic {
                    repo: repo_name.to_owned(),
                    path: ".2brain/ignore".to_owned(),
                    byte_start: 0,
                    code: "invalid-utf8".to_owned(),
                    severity: Severity::Error,
                    message: ".2brain/ignore contains invalid UTF-8 bytes".to_owned(),
                });
            }
        }
    }
    Ok(builder.build().unwrap_or_else(|_| Gitignore::empty()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn escaping_manifest_symlink_is_refused_before_reading() {
        let outer = tempfile::tempdir().unwrap();
        let outside_file = outer.path().join("secret.yaml");
        fs::write(&outside_file, "name: leaked-outside\n").unwrap();

        let ws = outer.path().join("ws");
        fs::create_dir_all(ws.join(".2brain")).unwrap();
        symlink(&outside_file, ws.join(".2brain/repo.yaml")).unwrap();

        let err = check_workspace(&ws).expect_err("escaping repo.yaml symlink must fail closed");
        assert!(matches!(err, FsError::SymlinkEscape(_)));
    }

    #[test]
    fn gitignore_rules_exclude_files_and_dirs_from_graph() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join(".2brain")).unwrap();
        fs::create_dir_all(dir.path().join("scratch")).unwrap();
        fs::write(dir.path().join(".2brain/ignore"), "scratch/\nsecret.md\n").unwrap();
        fs::write(
            dir.path().join("index.md"),
            "See [[hidden]] and [[secret]].\n",
        )
        .unwrap();
        fs::write(dir.path().join("scratch/hidden.md"), "Ignored note.\n").unwrap();
        fs::write(dir.path().join("secret.md"), "Ignored secret.\n").unwrap();

        let summary = check_workspace(dir.path()).unwrap();
        assert_eq!(summary.markdown_files, 1);
        assert_eq!(summary.shape.node_count, 1);
        assert_eq!(summary.shape.edge_count, 0);
        assert_eq!(summary.broken_links, 2);
    }

    #[test]
    fn duplicate_stems_report_ambiguity_instead_of_arbitrary_edge() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("a")).unwrap();
        fs::create_dir_all(dir.path().join("b")).unwrap();
        fs::write(dir.path().join("a/note.md"), "First.\n").unwrap();
        fs::write(dir.path().join("b/note.md"), "Second.\n").unwrap();
        fs::write(
            dir.path().join("index.md"),
            "Ambiguous [[note]] and exact [[a/note]].\n",
        )
        .unwrap();

        let summary = check_workspace(dir.path()).unwrap();
        assert_eq!(summary.shape.edge_count, 1);
        assert_eq!(summary.ambiguous_links, 1);
        assert!(
            summary
                .diagnostics
                .iter()
                .any(|d| d.code == "ambiguous-link")
        );
    }

    #[test]
    fn malformed_type_yaml_emits_explicit_schema_diagnostic() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("type")).unwrap();
        fs::write(dir.path().join("type/broken.type.yaml"), "fields: [\n").unwrap();
        fs::write(dir.path().join("note.md"), "Plain note.\n").unwrap();

        let summary = check_workspace(dir.path()).unwrap();
        assert_eq!(summary.types_loaded, 0);
        assert_eq!(summary.schema_diagnostics, 1);
        assert!(
            summary
                .diagnostics
                .iter()
                .any(|d| d.code == "invalid-type-def")
        );
    }

    #[test]
    fn qualified_links_resolve_self_repo_and_report_external_repo() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join(".2brain")).unwrap();
        fs::write(dir.path().join(".2brain/repo.yaml"), "name: vault\n").unwrap();
        fs::write(dir.path().join("target.md"), "Target.\n").unwrap();
        fs::write(
            dir.path().join("source.md"),
            "Self [[target::vault]] and external [[other::upstream]].\n",
        )
        .unwrap();

        let summary = check_workspace(dir.path()).unwrap();
        assert_eq!(summary.shape.edge_count, 1);
        assert_eq!(summary.unresolved_qualified_links, 1);
        assert!(
            summary
                .diagnostics
                .iter()
                .any(|d| d.code == "unresolved-qualified-link")
        );
    }

    #[test]
    fn dirfd_confinement_allows_internal_symlink_and_refuses_escape() {
        let outer = tempfile::tempdir().unwrap();
        let outside = outer.path().join("outside.md");
        fs::write(&outside, "secret outside bytes\n").unwrap();

        let ws = outer.path().join("ws");
        fs::create_dir_all(ws.join("notes")).unwrap();
        fs::write(ws.join("notes/real.md"), "inside note\n").unwrap();
        symlink("real.md", ws.join("notes/internal-link.md")).unwrap();
        symlink("../../outside.md", ws.join("notes/rel-escape.md")).unwrap();

        let (store, _snap) = load_snapshot(&ws).unwrap();
        let ok_loc = Locator::new("ws", "notes/internal-link.md").unwrap();
        let (bytes, _) = store.read_exact(&ok_loc).unwrap();
        assert_eq!(bytes, b"inside note\n");

        let esc_loc = Locator::new("ws", "notes/rel-escape.md").unwrap();
        let err = store.read_exact(&esc_loc).unwrap_err();
        assert!(matches!(err, StoreError::SymlinkEscape(_)));
    }

    #[test]
    fn context_refreshes_snapshot_after_external_edit() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("note.md"), "alpha original uniquev1\n").unwrap();

        let (store, snap) = load_snapshot(dir.path()).unwrap();
        let repo = snap.members.keys().next().unwrap().clone();
        let app = tb_app::App::new(store, snap);
        assert_eq!(app.generation(), Generation(1));

        // Simulate external edit on disk without telling App directly.
        fs::write(dir.path().join("note.md"), "alpha updated uniquev2\n").unwrap();

        // Context detects SHA-256 drift on the selected fragment, reloads Generation(2),
        // and serves fresh bytes (never stale original text).
        let ctx = app.context("alpha", Some(&repo), 5, 500).unwrap();
        assert_eq!(ctx["context"]["generation"], 2);
        let text = ctx["context"]["fragments"][0]["text"].as_str().unwrap();
        assert!(text.contains("uniquev2"), "stale text returned: {text}");
        assert!(!text.contains("uniquev1"));
    }
}
