//! Driven adapter: read-only workspace scanner with member confinement (SEM-18),
//! strict UTF-8 decode reporting (SEM-04), and `.2brain/ignore` filtering.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use fs4::fs_std::FileExt;
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use rustix::fs::{
    AtFlags, Mode, OFlags, RenameFlags, fsync, mkdirat, open, openat, readlinkat, renameat,
    renameat_with, statat, unlinkat,
};
use rustix::io::Errno;
use saphyr::{LoadableYamlNode, Scalar, Yaml};
use serde_json::{Value, json};
use tb_app::search_doc::{extract_enrichment_fields, extract_freshness_meta};
use tb_app::{
    IndexedFile, MemberMeta, MutationReceipt, ReviewDecision, StoreError, VaultSnapshot,
    WorkspaceMemberAction, WorkspaceStore,
};
use tb_contracts::{
    CasVerified, EvidenceReceipt, Generation, Locator, RevisionDigest, WriteGuardHash, WriteIntent,
};
use tb_graph::{GraphHealth, GraphShape, HubRow, TypedLinkGraph};
use tb_schema::{Diagnostic, SchemaRegistry, Severity, parse_type_def};
use tb_search::{FreshnessMeta, FreshnessPolicy, SearchDocInput, SearchIndex, SearchWeights};
use tb_syntax::{EdgeKind, FrontmatterValue, ParsedDocument, parse_document};
use thiserror::Error;

static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

pub use tb_app::ingest::IngestWriteOp;

/// Confined filesystem and Git workspace store (single-repo or multi-member `workspace.yaml`).
#[derive(Clone, Debug)]
pub struct FsWorkspaceStore {
    workspace_root: PathBuf,
    members: Arc<RwLock<BTreeMap<String, PathBuf>>>,
    write_lock: Arc<Mutex<()>>,
}

impl FsWorkspaceStore {
    /// Atomically commit a batch of ingest writes for a single repository under one `write.lock`
    /// and a single Git commit, with compensating rollback if any write or commit step fails.
    pub fn commit_ingest_batch(
        &self,
        repo: &str,
        items: &[IngestWriteOp],
        commit_message: &str,
    ) -> Result<Vec<MutationReceipt>, StoreError> {
        if items.is_empty() {
            return Ok(Vec::new());
        }
        for item in items {
            if item.locator.repo != repo {
                return Err(StoreError::InvalidEdit(format!(
                    "ingest batch mixes repositories ('{}' vs '{repo}')",
                    item.locator.repo
                )));
            }
        }
        let canonical_root = self.root_for(repo)?;
        let _lock = acquire_workspace_write_lock(&self.write_lock, &canonical_root)?;

        // Phase 1: Full pre-flight verification under write.lock (zero disk writes yet).
        // Captures prior bytes for refresh targets so any failure can roll back cleanly.
        let mut preflight: Vec<Option<Vec<u8>>> = Vec::with_capacity(items.len());
        for item in items {
            let target = canonical_root.join(&item.locator.path);
            if let Some(ref expected) = item.expected_hash {
                let prior = verify_live_cas(&canonical_root, &target, &item.locator, expected)?;
                preflight.push(Some(prior));
            } else {
                if let Ok((parent_fd, file_name)) =
                    open_confined_parent_dir(&canonical_root, &item.locator.path, false)
                    && statat(&parent_fd, &file_name, AtFlags::SYMLINK_NOFOLLOW).is_ok()
                {
                    return Err(StoreError::AlreadyExists(format!(
                        "{}/{}",
                        item.locator.repo, item.locator.path
                    )));
                }
                preflight.push(None);
            }
        }

        // Phase 2: Write all files via confined dirfd + O_NOFOLLOW; rollback on first error.
        let mut applied: Vec<(OwnedFd, std::ffi::OsString, Option<Vec<u8>>)> =
            Vec::with_capacity(items.len());
        for (item, prior) in items.iter().zip(preflight) {
            let write_res: Result<(OwnedFd, std::ffi::OsString), StoreError> = (|| {
                let create_parents = item.expected_hash.is_none();
                let (parent_fd, file_name) =
                    open_confined_parent_dir(&canonical_root, &item.locator.path, create_parents)?;
                if item.expected_hash.is_none() {
                    if statat(&parent_fd, &file_name, AtFlags::SYMLINK_NOFOLLOW).is_ok() {
                        return Err(StoreError::AlreadyExists(format!(
                            "{}/{}",
                            item.locator.repo, item.locator.path
                        )));
                    }
                } else {
                    reject_symlink_leaf(&parent_fd, &file_name)?;
                }
                atomic_write_in_dirfd(&parent_fd, &file_name, item.content.as_bytes())?;
                Ok((parent_fd, file_name))
            })();
            match write_res {
                Ok((parent_fd, file_name)) => applied.push((parent_fd, file_name, prior)),
                Err(write_err) => {
                    return Err(rollback_applied_batch(applied, write_err));
                }
            }
        }

        // Phase 3: Single atomic Git commit across all written paths.
        let rel_paths: Vec<&str> = items.iter().map(|i| i.locator.path.as_str()).collect();
        let commit = match git_commit_paths(&canonical_root, &rel_paths, commit_message) {
            Ok(c) => c,
            Err(git_err) => {
                return Err(rollback_applied_batch(applied, git_err));
            }
        };

        Ok(items
            .iter()
            .map(|item| {
                let bytes = item.content.as_bytes();
                MutationReceipt {
                    repo: item.locator.repo.clone(),
                    path: item.locator.path.clone(),
                    sha256: RevisionDigest::of_bytes(bytes).to_hex(),
                    hash: WriteGuardHash::of_bytes(bytes).to_hex(),
                    commit: commit.clone(),
                }
            })
            .collect())
    }

    #[must_use]
    pub fn single(repo_name: impl Into<String>, canonical_root: PathBuf) -> Self {
        let mut members = BTreeMap::new();
        members.insert(repo_name.into(), canonical_root.clone());
        Self {
            workspace_root: canonical_root,
            members: Arc::new(RwLock::new(members)),
            write_lock: Arc::new(Mutex::new(())),
        }
    }

    #[must_use]
    pub fn multi(workspace_root: PathBuf, members: BTreeMap<String, PathBuf>) -> Self {
        Self {
            workspace_root,
            members: Arc::new(RwLock::new(members)),
            write_lock: Arc::new(Mutex::new(())),
        }
    }
}

fn rollback_applied_batch(
    applied: Vec<(OwnedFd, std::ffi::OsString, Option<Vec<u8>>)>,
    cause: StoreError,
) -> StoreError {
    let mut rollback_failures = Vec::new();
    for (parent_fd, file_name, prior) in applied.into_iter().rev() {
        let res = match prior {
            Some(bytes) => atomic_write_in_dirfd(&parent_fd, &file_name, &bytes),
            None => unlinkat(&parent_fd, &file_name, AtFlags::empty()).map_err(|e| {
                StoreError::Io(format!(
                    "rollback unlink '{}': {e}",
                    file_name.to_string_lossy()
                ))
            }),
        };
        if let Err(err) = res {
            rollback_failures.push(format!("{}: {err}", file_name.to_string_lossy()));
        }
    }
    if rollback_failures.is_empty() {
        cause
    } else {
        StoreError::PostEffectFailure(format!(
            "batch failed ({cause}) and rollback had {} failure(s): {}",
            rollback_failures.len(),
            rollback_failures.join("; ")
        ))
    }
}

impl FsWorkspaceStore {
    #[must_use]
    pub fn member_root(&self, repo: &str) -> Option<PathBuf> {
        self.members
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(repo)
            .cloned()
    }

    fn root_for(&self, repo: &str) -> Result<PathBuf, StoreError> {
        let stored = self
            .member_root(repo)
            .ok_or_else(|| StoreError::NotFound(format!("unknown member '{repo}'")))?;
        // Revalidate every component from `workspace_root` to `stored` with `O_NOFOLLOW`
        // so a post-load member-dir symlink swap can never redirect writes outside the workspace.
        let rel = stored.strip_prefix(&self.workspace_root).map_err(|_| {
            StoreError::SymlinkEscape(format!("member '{repo}' root escapes workspace boundary"))
        })?;
        let mut dir_fd = open(
            &self.workspace_root,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|e| StoreError::Io(format!("open workspace root: {e}")))?;
        for comp in rel.components() {
            match comp {
                Component::CurDir => {}
                Component::Normal(seg) => {
                    dir_fd = openat(
                        &dir_fd,
                        seg,
                        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                        Mode::empty(),
                    )
                    .map_err(|errno| {
                        if errno == Errno::LOOP || errno == Errno::NOTDIR {
                            StoreError::SymlinkEscape(format!(
                                "member '{repo}' path component '{}' is a symlink or non-directory",
                                seg.to_string_lossy()
                            ))
                        } else {
                            StoreError::Io(format!(
                                "openat member '{repo}' segment '{}': {errno}",
                                seg.to_string_lossy()
                            ))
                        }
                    })?;
                }
                _ => {
                    return Err(StoreError::SymlinkEscape(format!(
                        "member '{repo}' contains disallowed path component"
                    )));
                }
            }
        }
        Ok(stored)
    }
}

impl WorkspaceStore for FsWorkspaceStore {
    fn read_exact(&self, locator: &Locator) -> Result<(Vec<u8>, EvidenceReceipt), StoreError> {
        let canonical_root = self.root_for(&locator.repo)?;
        let candidate = canonical_root.join(&locator.path);
        let bytes = match confined_read_bytes(&canonical_root, &candidate) {
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
            Err(FsError::InvalidGitRef(msg)) => {
                return Err(StoreError::InvalidEdit(msg));
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

    fn commit_verified(&self, intent: WriteIntent<CasVerified>) -> Result<String, StoreError> {
        let receipt = self.write_file_cas(
            intent.locator(),
            &intent.expected_guard().to_hex(),
            intent.new_content(),
            &format!("2brain: write {}", intent.locator().path),
        )?;
        Ok(receipt.commit.unwrap_or(receipt.hash))
    }

    fn reload_snapshot(
        &self,
        next_generation: Generation,
    ) -> Result<Option<VaultSnapshot>, StoreError> {
        let (fresh_store, snap) = load_snapshot_inner(&self.workspace_root, next_generation)
            .map_err(|e| StoreError::Io(e.to_string()))?;
        let fresh_members = fresh_store
            .members
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        *self
            .members
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = fresh_members;
        Ok(Some(snap))
    }

    fn load_snapshot_at_git_ref(
        &self,
        repo: &str,
        git_ref: &str,
    ) -> Result<VaultSnapshot, StoreError> {
        let canonical_root = self.root_for(repo)?;
        load_snapshot_at_git_ref(&canonical_root, git_ref).map_err(|e| match e {
            FsError::InvalidGitRef(msg) => StoreError::InvalidEdit(msg),
            FsError::SymlinkEscape(p) => StoreError::SymlinkEscape(p.display().to_string()),
            FsError::Io { path, source } => StoreError::Io(format!("{}: {source}", path.display())),
        })
    }

    fn create_file(
        &self,
        locator: &Locator,
        content: &str,
        commit_message: &str,
    ) -> Result<MutationReceipt, StoreError> {
        let canonical_root = self.root_for(&locator.repo)?;
        let _lock = acquire_workspace_write_lock(&self.write_lock, &canonical_root)?;
        let (parent_fd, file_name) =
            open_confined_parent_dir(&canonical_root, &locator.path, true)?;
        if statat(&parent_fd, &file_name, AtFlags::SYMLINK_NOFOLLOW).is_ok() {
            return Err(StoreError::AlreadyExists(format!(
                "{}/{}",
                locator.repo, locator.path
            )));
        }
        atomic_write_in_dirfd(&parent_fd, &file_name, content.as_bytes())?;
        let commit = match git_commit_paths(
            &canonical_root,
            &[locator.path.as_str()],
            commit_message,
        ) {
            Ok(c) => c,
            Err(git_err) => {
                return match unlinkat(&parent_fd, &file_name, AtFlags::empty()) {
                    Ok(()) => Err(git_err),
                    Err(unlink_err) => Err(StoreError::PostEffectFailure(format!(
                        "git commit failed ({git_err}) and rollback unlink failed ({unlink_err})"
                    ))),
                };
            }
        };
        let bytes = content.as_bytes();
        Ok(MutationReceipt {
            repo: locator.repo.clone(),
            path: locator.path.clone(),
            sha256: RevisionDigest::of_bytes(bytes).to_hex(),
            hash: WriteGuardHash::of_bytes(bytes).to_hex(),
            commit,
        })
    }

    fn write_file_cas(
        &self,
        locator: &Locator,
        expected_hash: &str,
        content: &str,
        commit_message: &str,
    ) -> Result<MutationReceipt, StoreError> {
        let canonical_root = self.root_for(&locator.repo)?;
        let _lock = acquire_workspace_write_lock(&self.write_lock, &canonical_root)?;
        let target = canonical_root.join(&locator.path);
        let prior_bytes = verify_live_cas(&canonical_root, &target, locator, expected_hash)?;
        let (parent_fd, file_name) =
            open_confined_parent_dir(&canonical_root, &locator.path, false)?;
        reject_symlink_leaf(&parent_fd, &file_name)?;
        atomic_write_in_dirfd(&parent_fd, &file_name, content.as_bytes())?;
        let commit = match git_commit_paths(
            &canonical_root,
            &[locator.path.as_str()],
            commit_message,
        ) {
            Ok(c) => c,
            Err(git_err) => {
                return match atomic_write_in_dirfd(&parent_fd, &file_name, &prior_bytes) {
                    Ok(()) => Err(git_err),
                    Err(restore_err) => Err(StoreError::PostEffectFailure(format!(
                        "git commit failed ({git_err}) and rollback restore failed ({restore_err})"
                    ))),
                };
            }
        };
        let bytes = content.as_bytes();
        Ok(MutationReceipt {
            repo: locator.repo.clone(),
            path: locator.path.clone(),
            sha256: RevisionDigest::of_bytes(bytes).to_hex(),
            hash: WriteGuardHash::of_bytes(bytes).to_hex(),
            commit,
        })
    }

    fn edit_file_cas(
        &self,
        locator: &Locator,
        expected_hash: &str,
        old_str: &str,
        new_str: &str,
        commit_message: &str,
    ) -> Result<MutationReceipt, StoreError> {
        let canonical_root = self.root_for(&locator.repo)?;
        let _lock = acquire_workspace_write_lock(&self.write_lock, &canonical_root)?;
        let target = canonical_root.join(&locator.path);
        let live_bytes = verify_live_cas(&canonical_root, &target, locator, expected_hash)?;
        let live_text = std::str::from_utf8(&live_bytes)
            .map_err(|_| StoreError::InvalidUtf8(format!("{}/{}", locator.repo, locator.path)))?;
        if old_str.is_empty() || !live_text.contains(old_str) {
            return Err(StoreError::InvalidEdit(
                "target text not found in current file".to_owned(),
            ));
        }
        let updated = live_text.replacen(old_str, new_str, 1);
        let (parent_fd, file_name) =
            open_confined_parent_dir(&canonical_root, &locator.path, false)?;
        reject_symlink_leaf(&parent_fd, &file_name)?;
        atomic_write_in_dirfd(&parent_fd, &file_name, updated.as_bytes())?;
        let commit = match git_commit_paths(
            &canonical_root,
            &[locator.path.as_str()],
            commit_message,
        ) {
            Ok(c) => c,
            Err(git_err) => {
                return match atomic_write_in_dirfd(&parent_fd, &file_name, &live_bytes) {
                    Ok(()) => Err(git_err),
                    Err(restore_err) => Err(StoreError::PostEffectFailure(format!(
                        "git commit failed ({git_err}) and rollback restore failed ({restore_err})"
                    ))),
                };
            }
        };
        let bytes = updated.as_bytes();
        Ok(MutationReceipt {
            repo: locator.repo.clone(),
            path: locator.path.clone(),
            sha256: RevisionDigest::of_bytes(bytes).to_hex(),
            hash: WriteGuardHash::of_bytes(bytes).to_hex(),
            commit,
        })
    }

    fn delete_file_cas(
        &self,
        locator: &Locator,
        expected_hash: &str,
        commit_message: &str,
    ) -> Result<Option<String>, StoreError> {
        let canonical_root = self.root_for(&locator.repo)?;
        let _lock = acquire_workspace_write_lock(&self.write_lock, &canonical_root)?;
        let target = canonical_root.join(&locator.path);
        let prior_bytes = verify_live_cas(&canonical_root, &target, locator, expected_hash)?;
        let (parent_fd, file_name) =
            open_confined_parent_dir(&canonical_root, &locator.path, false)?;
        reject_symlink_leaf(&parent_fd, &file_name)?;
        unlinkat(&parent_fd, &file_name, AtFlags::empty())
            .map_err(|e| StoreError::Io(format!("unlinkat {}: {e}", locator.path)))?;
        match git_commit_paths(&canonical_root, &[locator.path.as_str()], commit_message) {
            Ok(c) => Ok(c),
            Err(git_err) => match atomic_write_in_dirfd(&parent_fd, &file_name, &prior_bytes) {
                Ok(()) => Err(git_err),
                Err(restore_err) => Err(StoreError::PostEffectFailure(format!(
                    "git commit failed ({git_err}) and rollback restore failed ({restore_err})"
                ))),
            },
        }
    }

    fn commit_ingest_batch(
        &self,
        repo: &str,
        items: &[IngestWriteOp],
        commit_message: &str,
    ) -> Result<Vec<MutationReceipt>, StoreError> {
        FsWorkspaceStore::commit_ingest_batch(self, repo, items, commit_message)
    }

    fn rename_file_cas(
        &self,
        from: &Locator,
        to: &Locator,
        expected_hash: &str,
        commit_message: &str,
    ) -> Result<MutationReceipt, StoreError> {
        if from.repo != to.repo {
            return Err(StoreError::InvalidEdit(
                "rename must stay within the same repository".to_owned(),
            ));
        }
        let canonical_root = self.root_for(&from.repo)?;
        let _lock = acquire_workspace_write_lock(&self.write_lock, &canonical_root)?;
        let src = canonical_root.join(&from.path);
        let live_bytes = verify_live_cas(&canonical_root, &src, from, expected_hash)?;
        let (src_parent_fd, src_name) =
            open_confined_parent_dir(&canonical_root, &from.path, false)?;
        reject_symlink_leaf(&src_parent_fd, &src_name)?;
        let (dst_parent_fd, dst_name) = open_confined_parent_dir(&canonical_root, &to.path, true)?;
        if statat(&dst_parent_fd, &dst_name, AtFlags::SYMLINK_NOFOLLOW).is_ok() {
            return Err(StoreError::AlreadyExists(format!(
                "{}/{}",
                to.repo, to.path
            )));
        }
        renameat_with(
            &src_parent_fd,
            &src_name,
            &dst_parent_fd,
            &dst_name,
            RenameFlags::NOREPLACE,
        )
        .map_err(|errno| {
            if errno == Errno::EXIST {
                StoreError::AlreadyExists(format!("{}/{}", to.repo, to.path))
            } else {
                StoreError::Io(format!("renameat {} -> {}: {errno}", from.path, to.path))
            }
        })?;
        let commit = match git_commit_paths(
            &canonical_root,
            &[from.path.as_str(), to.path.as_str()],
            commit_message,
        ) {
            Ok(c) => c,
            Err(git_err) => {
                return match renameat(&dst_parent_fd, &dst_name, &src_parent_fd, &src_name) {
                    Ok(()) => Err(git_err),
                    Err(rb_err) => Err(StoreError::PostEffectFailure(format!(
                        "git commit failed ({git_err}) and rollback rename failed ({rb_err})"
                    ))),
                };
            }
        };
        Ok(MutationReceipt {
            repo: to.repo.clone(),
            path: to.path.clone(),
            sha256: RevisionDigest::of_bytes(&live_bytes).to_hex(),
            hash: WriteGuardHash::of_bytes(&live_bytes).to_hex(),
            commit,
        })
    }

    fn git_commit_preview(
        &self,
        repo: &str,
        commit_ref: &str,
        limit: usize,
    ) -> Result<Value, StoreError> {
        validate_git_ref(commit_ref).map_err(|e| StoreError::InvalidEdit(e.to_string()))?;
        let canonical_root = self.root_for(repo)?;
        let rev_spec = format!("{commit_ref}^{{commit}}");
        let show = Command::new("git")
            .arg("-C")
            .arg(&canonical_root)
            .args([
                "show",
                "--name-status",
                "--format=%H%x00%s%x00%ci",
                "--end-of-options",
                &rev_spec,
            ])
            .output()
            .map_err(|e| StoreError::Io(format!("git show failed: {e}")))?;
        if !show.status.success() {
            let stderr = String::from_utf8_lossy(&show.stderr).trim().to_owned();
            return Err(StoreError::InvalidEdit(format!(
                "cannot inspect git commit '{commit_ref}': {stderr}"
            )));
        }
        let stdout = String::from_utf8_lossy(&show.stdout);
        let mut lines = stdout.lines();
        let meta_line = lines.next().unwrap_or("");
        let mut meta_parts = meta_line.split('\0');
        let commit_sha = meta_parts.next().unwrap_or("").to_owned();
        let subject = meta_parts.next().unwrap_or("").to_owned();
        let date = meta_parts.next().unwrap_or("").to_owned();

        let mut affected = Vec::new();
        for line in lines {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            if let Some((status, path)) = trimmed.split_once('\t') {
                affected.push(json!({ "status": status, "path": path }));
                if affected.len() >= limit {
                    break;
                }
            }
        }

        let status_out = Command::new("git")
            .arg("-C")
            .arg(&canonical_root)
            .args(["status", "--porcelain"])
            .output()
            .ok();
        let dirty_files: Vec<String> = status_out
            .filter(|o| o.status.success())
            .map(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .filter_map(|l| l.get(3..).map(str::to_owned))
                    .take(limit)
                    .collect()
            })
            .unwrap_or_default();

        Ok(json!({
            "repo": repo,
            "commit": commit_sha,
            "subject": subject,
            "date": date,
            "affected_files": affected,
            "dirty_working_tree_files": dirty_files,
        }))
    }

    fn update_repo_deps(&self, repo: &str, dep_name: &str, add: bool) -> Result<Value, StoreError> {
        let canonical_root = self.root_for(repo)?;
        let _lock = acquire_workspace_write_lock(&self.write_lock, &canonical_root)?;
        let rel = ".2brain/repo.yaml";
        let manifest_path = canonical_root.join(rel);
        let existing_bytes = match confined_read_bytes(&canonical_root, &manifest_path) {
            Ok(Some(b)) => Some(b),
            Ok(None) => None,
            Err(FsError::SymlinkEscape(p)) => {
                return Err(StoreError::SymlinkEscape(p.display().to_string()));
            }
            Err(FsError::Io { path, source }) => {
                return Err(StoreError::Io(format!("{}: {source}", path.display())));
            }
            Err(FsError::InvalidGitRef(msg)) => return Err(StoreError::InvalidEdit(msg)),
        };

        let existing_text: Option<String> = match existing_bytes {
            Some(bytes) => {
                let text = String::from_utf8(bytes)
                    .map_err(|_| StoreError::InvalidUtf8(".2brain/repo.yaml".into()))?;
                Some(text)
            }
            None => None,
        };

        let mut deps: Vec<String> = Vec::new();
        if let Some(ref text) = existing_text {
            let docs = Yaml::load_from_str(text).map_err(|err| {
                StoreError::InvalidEdit(format!("malformed .2brain/repo.yaml: {err}"))
            })?;
            if let Some(first) = docs.first() {
                let Yaml::Mapping(map) = first else {
                    return Err(StoreError::InvalidEdit(
                        ".2brain/repo.yaml root must be a YAML mapping".into(),
                    ));
                };
                if text.trim_start().starts_with('{') {
                    return Err(StoreError::InvalidEdit(
                        "flow-style root YAML mapping in .2brain/repo.yaml is not supported for in-place edits"
                            .into(),
                    ));
                }
                for (k, v) in map {
                    let Yaml::Value(Scalar::String(ks)) = k else {
                        continue;
                    };
                    if ks.as_ref() == "deps"
                        && let Yaml::Sequence(seq) = v
                    {
                        for item in seq {
                            match item {
                                Yaml::Value(Scalar::String(s)) => deps.push(s.to_string()),
                                Yaml::Mapping(m) => {
                                    for (mk, mv) in m {
                                        if let (
                                            Yaml::Value(Scalar::String(mks)),
                                            Yaml::Value(Scalar::String(mvs)),
                                        ) = (mk, mv)
                                            && mks.as_ref() == "name"
                                        {
                                            deps.push(mvs.to_string());
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
        }

        let changed = if add {
            if deps.iter().any(|d| d == dep_name) {
                false
            } else {
                deps.push(dep_name.to_owned());
                deps.sort();
                deps.dedup();
                true
            }
        } else {
            let before = deps.len();
            deps.retain(|d| d != dep_name);
            deps.len() != before
        };

        let rendered = splice_repo_yaml_deps(existing_text.as_deref(), repo, &deps);
        validate_rendered_yaml_mapping(".2brain/repo.yaml", &rendered)?;

        let (parent_fd, file_name) = open_confined_parent_dir(&canonical_root, rel, true)?;
        reject_symlink_leaf(&parent_fd, &file_name)?;
        atomic_write_in_dirfd(&parent_fd, &file_name, rendered.as_bytes())?;
        let action_word = if add { "add" } else { "remove" };
        let commit = git_commit_paths(
            &canonical_root,
            &[rel],
            &format!("2brain: {action_word} dep {dep_name}"),
        )?;

        Ok(json!({
            "repo": repo,
            "dep": dep_name,
            "changed": changed,
            "deps": deps,
            "commit": commit,
        }))
    }

    fn update_workspace_member(&self, action: WorkspaceMemberAction) -> Result<Value, StoreError> {
        let _lock = acquire_workspace_write_lock(&self.write_lock, &self.workspace_root)?;
        let ws_path = self.workspace_root.join("workspace.yaml");
        let existing_text: Option<String> = match confined_read_bytes(
            &self.workspace_root,
            &ws_path,
        ) {
            Ok(Some(bytes)) => {
                let text = String::from_utf8(bytes)
                    .map_err(|_| StoreError::InvalidUtf8("workspace.yaml".into()))?;
                let docs = Yaml::load_from_str(&text).map_err(|err| {
                    StoreError::InvalidEdit(format!("malformed workspace.yaml: {err}"))
                })?;
                if let Some(first) = docs.first()
                    && !matches!(first, Yaml::Mapping(_))
                {
                    return Err(StoreError::InvalidEdit(
                        "workspace.yaml root must be a YAML mapping".into(),
                    ));
                }
                if text.trim_start().starts_with('{') {
                    return Err(StoreError::InvalidEdit(
                            "flow-style root YAML mapping in workspace.yaml is not supported for in-place edits"
                                .into(),
                        ));
                }
                Some(text)
            }
            Ok(None) => None,
            Err(FsError::SymlinkEscape(p)) => {
                return Err(StoreError::SymlinkEscape(p.display().to_string()));
            }
            Err(FsError::Io { path, source }) => {
                return Err(StoreError::Io(format!("{}: {source}", path.display())));
            }
            Err(FsError::InvalidGitRef(msg)) => return Err(StoreError::InvalidEdit(msg)),
        };

        let mut entries: Vec<WorkspaceManifestEntry> = match existing_text.as_deref() {
            Some(text) => parse_workspace_yaml_entries(text),
            None => {
                // Bootstrap workspace.yaml from current single primary member.
                let cur = self
                    .members
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                cur.iter()
                    .map(|(name, path)| {
                        let rel = path
                            .strip_prefix(&self.workspace_root)
                            .ok()
                            .map(|p| {
                                let s = p.to_string_lossy().replace('\\', "/");
                                if s.is_empty() { ".".to_owned() } else { s }
                            })
                            .unwrap_or_else(|| ".".to_owned());
                        WorkspaceManifestEntry {
                            name: name.clone(),
                            path: rel,
                            role: "own".to_owned(),
                        }
                    })
                    .collect()
            }
        };

        match &action {
            WorkspaceMemberAction::Add { name, path, role } => {
                if entries.iter().any(|e| e.name == *name) {
                    return Err(StoreError::AlreadyExists(format!(
                        "workspace member '{name}' already exists"
                    )));
                }
                let candidate = resolve_confined_member_path(&self.workspace_root, path)?;
                if !candidate.is_dir() {
                    return Err(StoreError::NotFound(format!(
                        "member path '{}' is not an existing directory",
                        path
                    )));
                }
                entries.push(WorkspaceManifestEntry {
                    name: name.clone(),
                    path: path.clone(),
                    role: role.clone(),
                });
            }
            WorkspaceMemberAction::Remove { name } => {
                let before = entries.len();
                entries.retain(|e| e.name != *name);
                if entries.len() == before {
                    return Err(StoreError::NotFound(format!(
                        "workspace member '{name}' not found"
                    )));
                }
                if entries.is_empty() {
                    return Err(StoreError::InvalidEdit(
                        "cannot remove the last workspace member".into(),
                    ));
                }
            }
            WorkspaceMemberAction::SetRole { name, role } => {
                let Some(entry) = entries.iter_mut().find(|e| e.name == *name) else {
                    return Err(StoreError::NotFound(format!(
                        "workspace member '{name}' not found"
                    )));
                };
                entry.role = role.clone();
            }
        }

        let mut members_block = String::from("members:\n");
        for e in &entries {
            members_block.push_str(&format!(
                "  - name: {}\n    path: {}\n    role: {}\n",
                yaml_safe_scalar(&e.name),
                yaml_safe_scalar(&e.path),
                yaml_safe_scalar(&e.role)
            ));
        }
        let rendered = match existing_text.as_deref() {
            Some(text) if !text.trim().is_empty() => {
                splice_yaml_top_level_section(text, "members", &members_block)
            }
            _ => members_block,
        };
        validate_rendered_yaml_mapping("workspace.yaml", &rendered)?;
        let (parent_fd, file_name) =
            open_confined_parent_dir(&self.workspace_root, "workspace.yaml", false)?;
        reject_symlink_leaf(&parent_fd, &file_name)?;
        atomic_write_in_dirfd(&parent_fd, &file_name, rendered.as_bytes())?;

        Ok(json!({
            "members": entries
                .iter()
                .map(|e| json!({ "name": e.name, "path": e.path, "role": e.role }))
                .collect::<Vec<_>>()
        }))
    }

    fn load_review_decisions(&self) -> Result<Vec<ReviewDecision>, StoreError> {
        let rel = ".2brain/engine/review-decisions.jsonl";
        let path = self.workspace_root.join(rel);
        let bytes = match confined_read_bytes(&self.workspace_root, &path) {
            Ok(Some(b)) => b,
            Ok(None) => return Ok(Vec::new()),
            Err(FsError::SymlinkEscape(p)) => {
                return Err(StoreError::SymlinkEscape(p.display().to_string()));
            }
            Err(FsError::Io { path, source }) => {
                return Err(StoreError::Io(format!("{}: {source}", path.display())));
            }
            Err(FsError::InvalidGitRef(msg)) => return Err(StoreError::InvalidEdit(msg)),
        };
        parse_review_decisions_strict(rel, &bytes)
    }

    fn record_review_decision(
        &self,
        decision: ReviewDecision,
    ) -> Result<ReviewDecision, StoreError> {
        let _lock = acquire_workspace_write_lock(&self.write_lock, &self.workspace_root)?;
        let rel = ".2brain/engine/review-decisions.jsonl";
        let (parent_fd, file_name) = open_confined_parent_dir(&self.workspace_root, rel, true)?;
        reject_symlink_leaf(&parent_fd, &file_name)?;
        let mut existing =
            match confined_read_bytes(&self.workspace_root, &self.workspace_root.join(rel)) {
                Ok(Some(b)) => b,
                Ok(None) => Vec::new(),
                Err(FsError::SymlinkEscape(p)) => {
                    return Err(StoreError::SymlinkEscape(p.display().to_string()));
                }
                Err(FsError::Io { path, source }) => {
                    return Err(StoreError::Io(format!("{}: {source}", path.display())));
                }
                Err(FsError::InvalidGitRef(msg)) => return Err(StoreError::InvalidEdit(msg)),
            };
        if !existing.is_empty() {
            let _ = parse_review_decisions_strict(rel, &existing)?;
        }
        let line = serde_json::to_string(&decision)
            .map_err(|e| StoreError::Io(format!("serialize review decision: {e}")))?;
        if !existing.is_empty() && !existing.ends_with(b"\n") {
            existing.push(b'\n');
        }
        existing.extend_from_slice(line.as_bytes());
        existing.push(b'\n');
        atomic_write_in_dirfd(&parent_fd, &file_name, &existing)?;
        Ok(decision)
    }
}

fn parse_review_decisions_strict(
    rel: &str,
    bytes: &[u8],
) -> Result<Vec<ReviewDecision>, StoreError> {
    let text = std::str::from_utf8(bytes).map_err(|_| StoreError::InvalidUtf8(rel.to_owned()))?;
    let mut out = Vec::new();
    for (idx, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let rec = serde_json::from_str::<ReviewDecision>(trimmed).map_err(|e| {
            StoreError::Io(format!(
                "malformed review decision in {rel} at line {}: {e}",
                idx + 1
            ))
        })?;
        out.push(rec);
    }
    Ok(out)
}

struct WorkspaceWriteLock<'a> {
    _in_proc: std::sync::MutexGuard<'a, ()>,
    _file_lock: fs::File,
}

/// Acquire BOTH the in-process mutex and the interprocess `fs4` lock on `<root>/.2brain/engine/write.lock`.
fn acquire_workspace_write_lock<'a>(
    in_proc_mutex: &'a Mutex<()>,
    canonical_root: &Path,
) -> Result<WorkspaceWriteLock<'a>, StoreError> {
    let in_proc = in_proc_mutex
        .lock()
        .map_err(|_| StoreError::Io("workspace write lock poisoned".into()))?;
    let engine_dir = canonical_root.join(".2brain").join("engine");
    fs::create_dir_all(&engine_dir)
        .map_err(|e| StoreError::Io(format!("{}: {e}", engine_dir.display())))?;
    let lock_path = engine_dir.join("write.lock");
    if let Ok(meta) = fs::symlink_metadata(&lock_path)
        && meta.file_type().is_symlink()
    {
        return Err(StoreError::SymlinkEscape(lock_path.display().to_string()));
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(|e| StoreError::Io(format!("{}: {e}", lock_path.display())))?;
    file.lock_exclusive()
        .map_err(|e| StoreError::Io(format!("lock_exclusive {}: {e}", lock_path.display())))?;
    Ok(WorkspaceWriteLock {
        _in_proc: in_proc,
        _file_lock: file,
    })
}

fn verify_live_cas(
    canonical_root: &Path,
    target: &Path,
    locator: &Locator,
    expected_hash: &str,
) -> Result<Vec<u8>, StoreError> {
    let bytes = match confined_read_bytes(canonical_root, target) {
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
        Err(FsError::InvalidGitRef(msg)) => {
            return Err(StoreError::InvalidEdit(msg));
        }
    };
    let live_guard = WriteGuardHash::of_bytes(&bytes).to_hex();
    let live_sha = RevisionDigest::of_bytes(&bytes).to_hex();
    let expected_norm = expected_hash
        .strip_prefix("sha256:")
        .unwrap_or(expected_hash);
    if expected_norm != live_guard && !expected_norm.eq_ignore_ascii_case(&live_sha) {
        return Err(StoreError::CasMismatch(format!(
            "expected_hash '{expected_hash}' does not match current file guard '{live_guard}'"
        )));
    }
    Ok(bytes)
}

/// Open the parent directory of `rel_path` strictly via `openat(..., O_NOFOLLOW | O_DIRECTORY)`
/// starting from `canonical_root`, optionally creating missing subdirectories via `mkdirat`.
fn open_confined_parent_dir(
    canonical_root: &Path,
    rel_path: &str,
    create_parents: bool,
) -> Result<(OwnedFd, OsString), StoreError> {
    let mut comps: Vec<OsString> = Vec::new();
    for comp in Path::new(rel_path).components() {
        match comp {
            Component::Normal(seg) => comps.push(seg.to_os_string()),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(StoreError::SymlinkEscape(rel_path.to_owned()));
            }
        }
    }
    let Some(file_name) = comps.pop() else {
        return Err(StoreError::InvalidEdit("empty target path".to_owned()));
    };
    let mut cur_fd: OwnedFd = open(
        canonical_root,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|e| StoreError::Io(format!("open root {}: {e}", canonical_root.display())))?;

    for dir_seg in comps {
        match openat(
            &cur_fd,
            dir_seg.as_os_str(),
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        ) {
            Ok(next_fd) => cur_fd = next_fd,
            Err(Errno::NOENT) if create_parents => {
                match mkdirat(
                    &cur_fd,
                    dir_seg.as_os_str(),
                    Mode::from_bits_truncate(0o755),
                ) {
                    Ok(()) | Err(Errno::EXIST) => {}
                    Err(e) => {
                        return Err(StoreError::Io(format!(
                            "mkdirat {}: {e}",
                            dir_seg.to_string_lossy()
                        )));
                    }
                }
                cur_fd = openat(
                    &cur_fd,
                    dir_seg.as_os_str(),
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                )
                .map_err(|errno| {
                    if matches!(errno, Errno::LOOP | Errno::NOTDIR) {
                        StoreError::SymlinkEscape(rel_path.to_owned())
                    } else {
                        StoreError::Io(format!(
                            "openat created dir {}: {errno}",
                            dir_seg.to_string_lossy()
                        ))
                    }
                })?;
            }
            Err(Errno::LOOP | Errno::NOTDIR) => {
                return Err(StoreError::SymlinkEscape(rel_path.to_owned()));
            }
            Err(Errno::NOENT) => return Err(StoreError::NotFound(rel_path.to_owned())),
            Err(errno) => {
                return Err(StoreError::Io(format!(
                    "openat dir {}: {errno}",
                    dir_seg.to_string_lossy()
                )));
            }
        }
    }
    Ok((cur_fd, file_name))
}

fn reject_symlink_leaf(parent_fd: &OwnedFd, file_name: &OsString) -> Result<(), StoreError> {
    if let Ok(st) = statat(parent_fd, file_name.as_os_str(), AtFlags::SYMLINK_NOFOLLOW)
        && (st.st_mode & 0o170000) == 0o120000
    {
        return Err(StoreError::SymlinkEscape(
            file_name.to_string_lossy().into_owned(),
        ));
    }
    Ok(())
}

/// Zero-TOCTOU atomic write strictly inside an already-opened confined parent directory FD.
fn atomic_write_in_dirfd(
    parent_fd: &OwnedFd,
    file_name: &OsString,
    bytes: &[u8],
) -> Result<(), StoreError> {
    let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp_name = OsString::from(format!(".2brain-write-{}-{seq}.tmp", std::process::id()));
    let fd = openat(
        parent_fd,
        tmp_name.as_os_str(),
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::from_bits_truncate(0o644),
    )
    .map_err(|e| StoreError::Io(format!("openat temp file: {e}")))?;
    let mut file = fs::File::from(fd);
    if let Err(err) = file.write_all(bytes).and_then(|()| file.sync_all()) {
        let _ = unlinkat(parent_fd, tmp_name.as_os_str(), AtFlags::empty());
        return Err(StoreError::Io(format!("write/sync temp file: {err}")));
    }
    drop(file);
    if let Err(err) = renameat(
        parent_fd,
        tmp_name.as_os_str(),
        parent_fd,
        file_name.as_os_str(),
    ) {
        let _ = unlinkat(parent_fd, tmp_name.as_os_str(), AtFlags::empty());
        return Err(StoreError::Io(format!("renameat temp file: {err}")));
    }
    let _ = fsync(parent_fd);
    Ok(())
}

fn git_commit_paths(
    canonical_root: &Path,
    rel_paths: &[&str],
    message: &str,
) -> Result<Option<String>, StoreError> {
    if !canonical_root.join(".git").exists() {
        return Ok(None);
    }
    let mut add = Command::new("git");
    add.arg("-C")
        .arg(canonical_root)
        .arg("add")
        .arg("-A")
        .arg("--");
    for p in rel_paths {
        add.arg(p);
    }
    let add_out = add
        .output()
        .map_err(|e| StoreError::Io(format!("git add failed to spawn: {e}")))?;
    if !add_out.status.success() {
        return Err(StoreError::Io(format!(
            "git add failed: {}",
            String::from_utf8_lossy(&add_out.stderr)
        )));
    }
    let mut commit = Command::new("git");
    commit.arg("-C").arg(canonical_root).args([
        "-c",
        "user.name=2brain",
        "-c",
        "user.email=2brain@local",
        "commit",
        "-q",
        "-m",
        message,
        "--",
    ]);
    for p in rel_paths {
        commit.arg(p);
    }
    let commit_out = commit
        .output()
        .map_err(|e| StoreError::Io(format!("git commit failed to spawn: {e}")))?;
    if !commit_out.status.success() {
        let stderr = String::from_utf8_lossy(&commit_out.stderr);
        if !stderr.contains("nothing to commit") {
            return Err(StoreError::Io(format!("git commit failed: {stderr}")));
        }
    }
    let rev = Command::new("git")
        .arg("-C")
        .arg(canonical_root)
        .args(["rev-parse", "HEAD"])
        .output()
        .map_err(|e| StoreError::Io(format!("git rev-parse failed: {e}")))?;
    if rev.status.success() {
        Ok(Some(String::from_utf8_lossy(&rev.stdout).trim().to_owned()))
    } else {
        Ok(None)
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

/// Incrementally refresh a [`VaultSnapshot`] when a bounded set of `.md` files changed on disk,
/// avoiding re-reading untouched notes. Automatically falls back to a full reload if any
/// schema (`*.type.yaml`), manifest (`workspace.yaml`, `repo.yaml`, `ignore`), directory, or
/// large batch (> 16 paths) changed.
pub fn refresh_snapshot_incremental(
    root: &Path,
    prev: &VaultSnapshot,
    changed_paths: &[PathBuf],
    generation: Generation,
) -> Result<VaultSnapshot, FsError> {
    if changed_paths.is_empty() || changed_paths.len() > 16 || prev.members.is_empty() {
        return load_snapshot_with_generation(root, generation);
    }

    // Deduplicate and classify every changed path; any non-markdown or structural path
    // triggers a full reload for exactness.
    let mut member_roots: Vec<(String, PathBuf)> = prev
        .members
        .iter()
        .map(|(repo, meta)| (repo.clone(), PathBuf::from(&meta.root)))
        .collect();
    // Longest-prefix match first in case a member root is nested under workspace_root.
    member_roots.sort_by_key(|(_, p)| std::cmp::Reverse(p.as_os_str().len()));

    let mut targets: Vec<(String, PathBuf, String)> = Vec::new(); // (repo, member_root, rel_posix)
    let mut seen: std::collections::BTreeSet<(String, String)> = std::collections::BTreeSet::new();

    for raw_path in changed_paths {
        let fname = raw_path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if fname == "workspace.yaml"
            || fname == "repo.yaml"
            || fname == "ignore"
            || fname.ends_with(".type.yaml")
            || raw_path.is_dir()
        {
            return load_snapshot_with_generation(root, generation);
        }
        if !fname.ends_with(".md") {
            continue;
        }
        let matching_members: Vec<&(String, PathBuf)> = member_roots
            .iter()
            .filter(|(_, mroot)| raw_path.starts_with(mroot))
            .collect();
        if matching_members.is_empty() {
            return load_snapshot_with_generation(root, generation);
        }
        // Refresh EVERY containing member when member roots overlap (nested members).
        for (repo, member_root) in matching_members {
            let Ok(rel) = raw_path.strip_prefix(member_root) else {
                return load_snapshot_with_generation(root, generation);
            };
            let rel_posix = rel.to_string_lossy().replace('\\', "/");
            if rel_posix.starts_with(".git/") || rel_posix.starts_with(".2brain/engine/") {
                continue;
            }
            if seen.insert((repo.clone(), rel_posix.clone())) {
                targets.push((repo.clone(), member_root.clone(), rel_posix));
            }
        }
    }

    // If any symlink aliases exist in the workspace, refresh them too whenever a target note
    // changes so symlinked aliases never retain stale content or edges.
    if !targets.is_empty() && !prev.indexed_symlinks.is_empty() {
        for sym_loc in &prev.indexed_symlinks {
            if let Some(meta) = prev.members.get(&sym_loc.repo) {
                let mroot = PathBuf::from(&meta.root);
                if seen.insert((sym_loc.repo.clone(), sym_loc.path.clone())) {
                    targets.push((sym_loc.repo.clone(), mroot, sym_loc.path.clone()));
                }
            }
        }
    }

    if targets.is_empty() {
        let mut unchanged = prev.clone();
        unchanged.generation = generation;
        return Ok(unchanged);
    }

    let mut files = prev.files.clone();
    let mut path_index = prev.path_index.clone();
    let mut stem_index = prev.stem_index.clone();
    let mut all_diags = prev.diagnostics.clone();
    let mut indexed_symlinks = prev.indexed_symlinks.clone();
    let mut search = prev.search.clone();

    // Load ignore matchers per touched member only.
    let mut ignore_by_repo: HashMap<String, Gitignore> = HashMap::new();
    let mut utf8_errors = 0usize;

    for (repo, member_root, rel_posix) in &targets {
        let Ok(loc) = Locator::new(repo, rel_posix) else {
            continue;
        };

        // Remove stale entries for this locator before re-reading.
        files.remove(&loc);
        indexed_symlinks.remove(&loc);
        search.remove_document(&loc);
        path_index.remove(&(repo.clone(), rel_posix.clone()));
        if let Some(no_ext) = rel_posix.strip_suffix(".md") {
            path_index.remove(&(repo.clone(), no_ext.to_owned()));
        }
        let file_name = rel_posix.rsplit('/').next().unwrap_or(rel_posix);
        if let Some(stem) = file_name.strip_suffix(".md")
            && let Some(vec) = stem_index.get_mut(&(repo.clone(), stem.to_owned()))
        {
            vec.retain(|l| l != &loc);
            if vec.is_empty() {
                stem_index.remove(&(repo.clone(), stem.to_owned()));
            }
        }
        all_diags.retain(|d| !(d.repo == *repo && d.path == *rel_posix));

        let gitignore = if let Some(ig) = ignore_by_repo.get(repo) {
            ig
        } else {
            let mut ig_diags = Vec::new();
            let ig = load_ignore_rules(member_root, repo, &mut ig_diags, &mut utf8_errors)?;
            ignore_by_repo.insert(repo.clone(), ig);
            &ignore_by_repo[repo]
        };

        let full_path = member_root.join(rel_posix);
        if gitignore
            .matched_path_or_any_parents(&full_path, false)
            .is_ignore()
        {
            continue;
        }
        let bytes = match confined_read_bytes(member_root, &full_path) {
            Ok(Some(b)) => b,
            Ok(None) | Err(FsError::SymlinkEscape(_)) => continue,
            Err(err) => return Err(err),
        };
        if bytes.iter().take(8192).any(|&b| b == 0) {
            continue;
        }
        if fs::symlink_metadata(&full_path).is_ok_and(|m| m.file_type().is_symlink()) {
            indexed_symlinks.insert(loc.clone());
        }

        let sha256 = RevisionDigest::of_bytes(&bytes);
        let short_hash = WriteGuardHash::of_bytes(&bytes).to_hex();
        match parse_document(&bytes) {
            Ok(doc) => {
                let diags = prev.registry.validate_document(repo, rel_posix, &doc);
                all_diags.extend(diags);
                let raw_text = String::from_utf8(bytes).unwrap_or_default();
                let description =
                    doc.frontmatter
                        .as_ref()
                        .and_then(|fm| match fm.get("description") {
                            Some(FrontmatterValue::String(s)) => Some(s.clone()),
                            _ => None,
                        });
                let declared_type = doc
                    .frontmatter
                    .as_ref()
                    .and_then(|fm| match fm.get("type") {
                        Some(FrontmatterValue::String(s)) => Some(s.clone()),
                        _ => None,
                    });
                if let Some(stem) = file_name.strip_suffix(".md") {
                    let entry = stem_index
                        .entry((repo.clone(), stem.to_owned()))
                        .or_default();
                    entry.push(loc.clone());
                    entry.sort();
                    entry.dedup();
                }
                path_index.insert((repo.clone(), rel_posix.clone()), loc.clone());
                if let Some(no_ext) = rel_posix.strip_suffix(".md") {
                    path_index.insert((repo.clone(), no_ext.to_owned()), loc.clone());
                }
                let indexed = IndexedFile {
                    locator: loc.clone(),
                    sha256,
                    short_hash,
                    size_bytes: raw_text.len(),
                    raw_text,
                    doc,
                    description,
                    declared_type,
                };
                let (kw, sq) = extract_enrichment_fields(&indexed);
                let fresh = extract_freshness_meta(&indexed);
                search.upsert_rich_document(SearchDocInput {
                    locator: loc.clone(),
                    body: indexed.body(),
                    description: indexed.description.as_deref().unwrap_or(""),
                    keywords: &kw,
                    search_questions: &sq,
                    freshness: fresh,
                });
                files.insert(loc, indexed);
            }
            Err(_) => {
                all_diags.push(Diagnostic {
                    repo: repo.clone(),
                    path: rel_posix.clone(),
                    byte_start: 0,
                    code: "invalid-utf8".to_owned(),
                    severity: Severity::Error,
                    message: "file contains invalid UTF-8 bytes".to_owned(),
                });
            }
        }
    }

    let mut locators = Vec::with_capacity(files.len());
    let mut edges: Vec<(Locator, Locator, EdgeKind)> = Vec::new();
    for (loc, file) in &files {
        locators.push(loc.clone());
        for link in &file.doc.wikilinks {
            let target_repo = link.repo.as_deref().unwrap_or(&loc.repo);
            if !prev.members.contains_key(target_repo) {
                continue;
            }
            if let Some(target_loc) = path_index.get(&(target_repo.to_owned(), link.target.clone()))
            {
                edges.push((loc.clone(), target_loc.clone(), link.kind));
            } else if let Some(matches) =
                stem_index.get(&(target_repo.to_owned(), link.target.clone()))
                && matches.len() == 1
            {
                edges.push((loc.clone(), matches[0].clone(), link.kind));
            }
        }
    }

    let today_days = current_utc_epoch_days();
    if search.needs_compaction() {
        search = build_rich_search_index(
            &files,
            today_days,
            prev.search.weights(),
            prev.search.freshness_policy(),
        );
    } else {
        search.set_default_reference_days(today_days);
    }

    let graph = TypedLinkGraph::build(locators, &edges);

    Ok(VaultSnapshot {
        generation,
        members: prev.members.clone(),
        files,
        registry: prev.registry.clone(),
        graph,
        search,
        diagnostics: all_diags,
        path_index,
        stem_index,
        indexed_symlinks,
    })
}

#[derive(Clone, Debug)]
struct WorkspaceManifestEntry {
    name: String,
    path: String,
    role: String,
}

fn parse_workspace_yaml_entries(raw: &str) -> Vec<WorkspaceManifestEntry> {
    let Ok(docs) = Yaml::load_from_str(raw) else {
        return Vec::new();
    };
    let Some(Yaml::Mapping(root)) = docs.first() else {
        return Vec::new();
    };
    let mut entries = Vec::new();
    for (k, v) in root {
        let Yaml::Value(Scalar::String(ks)) = k else {
            continue;
        };
        if ks.as_ref() != "members" {
            continue;
        }
        let Yaml::Sequence(seq) = v else {
            continue;
        };
        for item in seq {
            let Yaml::Mapping(m) = item else {
                continue;
            };
            let mut name = None;
            let mut path = None;
            let mut role = "own".to_owned();
            for (mk, mv) in m {
                let (Yaml::Value(Scalar::String(mks)), Yaml::Value(Scalar::String(mvs))) = (mk, mv)
                else {
                    continue;
                };
                match mks.as_ref() {
                    "name" => name = Some(mvs.to_string()),
                    "path" => path = Some(mvs.to_string()),
                    "role" => role = mvs.to_string(),
                    _ => {}
                }
            }
            if let (Some(n), Some(p)) = (name, path)
                && !n.is_empty()
                && !p.is_empty()
            {
                entries.push(WorkspaceManifestEntry {
                    name: n,
                    path: p,
                    role,
                });
            }
        }
    }
    entries
}

fn resolve_confined_member_path(
    workspace_root: &Path,
    rel_or_dot: &str,
) -> Result<PathBuf, StoreError> {
    let p = Path::new(rel_or_dot);
    if p.is_absolute() {
        return Err(StoreError::InvalidEdit(format!(
            "member path '{rel_or_dot}' must be relative to workspace root"
        )));
    }
    for c in p.components() {
        if matches!(
            c,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        ) {
            return Err(StoreError::SymlinkEscape(format!(
                "member path '{rel_or_dot}' escapes workspace root"
            )));
        }
    }
    let joined = workspace_root.join(p);
    let canonical = fs::canonicalize(&joined)
        .map_err(|e| StoreError::NotFound(format!("member path '{rel_or_dot}': {e}")))?;
    if !canonical.starts_with(workspace_root) {
        return Err(StoreError::SymlinkEscape(format!(
            "member path '{rel_or_dot}' escapes workspace root via symlink"
        )));
    }
    Ok(canonical)
}

fn read_repo_deps(canonical_root: &Path) -> Vec<String> {
    let manifest = canonical_root.join(".2brain/repo.yaml");
    let Ok(Some(bytes)) = confined_read_bytes(canonical_root, &manifest) else {
        return Vec::new();
    };
    let Ok(text) = std::str::from_utf8(&bytes) else {
        return Vec::new();
    };
    let Ok(docs) = Yaml::load_from_str(text) else {
        return Vec::new();
    };
    let Some(Yaml::Mapping(map)) = docs.first() else {
        return Vec::new();
    };
    let mut deps = Vec::new();
    for (k, v) in map {
        if let (Yaml::Value(Scalar::String(ks)), Yaml::Sequence(seq)) = (k, v)
            && ks.as_ref() == "deps"
        {
            for item in seq {
                match item {
                    Yaml::Value(Scalar::String(s)) => deps.push(s.to_string()),
                    Yaml::Mapping(m) => {
                        for (mk, mv) in m {
                            if let (
                                Yaml::Value(Scalar::String(mks)),
                                Yaml::Value(Scalar::String(mvs)),
                            ) = (mk, mv)
                                && mks.as_ref() == "name"
                            {
                                deps.push(mvs.to_string());
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    deps.sort();
    deps.dedup();
    deps
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

    // Discover members: multi-member `workspace.yaml` if present, otherwise single primary repo.
    let mut member_specs: Vec<(String, PathBuf, String)> = Vec::new();
    let ws_yaml = canonical_root.join("workspace.yaml");
    let mut ws_yaml_valid_text: Option<String> = None;
    let mut ws_manifest_invalid = false;
    if let Some(bytes) = confined_read_bytes(&canonical_root, &ws_yaml)? {
        match String::from_utf8(bytes) {
            Ok(text) => match Yaml::load_from_str(&text) {
                Ok(docs) => match docs.first() {
                    Some(Yaml::Mapping(_)) => {
                        for entry in parse_workspace_yaml_entries(&text) {
                            match resolve_confined_member_path(&canonical_root, &entry.path) {
                                Ok(member_dir) if member_dir.is_dir() => {
                                    member_specs.push((entry.name, member_dir, entry.role));
                                }
                                Ok(_) | Err(_) => {
                                    all_diags.push(Diagnostic {
                                        repo: entry.name,
                                        path: "workspace.yaml".to_owned(),
                                        byte_start: 0,
                                        code: "invalid-workspace-member".to_owned(),
                                        severity: Severity::Error,
                                        message: format!(
                                            "workspace member path '{}' is missing or escapes workspace root",
                                            entry.path
                                        ),
                                    });
                                }
                            }
                        }
                        ws_yaml_valid_text = Some(text);
                    }
                    None => {}
                    Some(_) => {
                        ws_manifest_invalid = true;
                        all_diags.push(Diagnostic {
                            repo: "workspace".to_owned(),
                            path: "workspace.yaml".to_owned(),
                            byte_start: 0,
                            code: "invalid-workspace-manifest".to_owned(),
                            severity: Severity::Error,
                            message: "workspace.yaml root must be a YAML mapping".to_owned(),
                        });
                    }
                },
                Err(err) => {
                    ws_manifest_invalid = true;
                    all_diags.push(Diagnostic {
                        repo: "workspace".to_owned(),
                        path: "workspace.yaml".to_owned(),
                        byte_start: 0,
                        code: "invalid-workspace-manifest".to_owned(),
                        severity: Severity::Error,
                        message: format!("failed to parse workspace.yaml: {err}"),
                    });
                }
            },
            Err(_) => {
                ws_manifest_invalid = true;
                utf8_errors += 1;
                all_diags.push(Diagnostic {
                    repo: "workspace".to_owned(),
                    path: "workspace.yaml".to_owned(),
                    byte_start: 0,
                    code: "invalid-utf8".to_owned(),
                    severity: Severity::Error,
                    message: "workspace.yaml contains invalid UTF-8 bytes".to_owned(),
                });
            }
        }
    }

    if member_specs.is_empty() && !ws_manifest_invalid {
        let repo_name = read_repo_name(&canonical_root)?.unwrap_or_else(|| {
            canonical_root
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("workspace")
                .to_owned()
        });
        member_specs.push((repo_name, canonical_root.clone(), "primary".to_owned()));
    }

    let mut registry = SchemaRegistry::new();
    let mut files: BTreeMap<Locator, IndexedFile> = BTreeMap::new();
    let mut path_index: BTreeMap<(String, String), Locator> = BTreeMap::new();
    let mut stem_index: BTreeMap<(String, String), Vec<Locator>> = BTreeMap::new();
    let mut indexed_symlinks: std::collections::BTreeSet<Locator> =
        std::collections::BTreeSet::new();
    let mut members: BTreeMap<String, MemberMeta> = BTreeMap::new();
    let mut store_members: BTreeMap<String, PathBuf> = BTreeMap::new();

    for (repo_name, member_root, role) in member_specs {
        store_members.insert(repo_name.clone(), member_root.clone());
        let gitignore =
            load_ignore_rules(&member_root, &repo_name, &mut all_diags, &mut utf8_errors)?;

        let type_dir = member_root.join("type");
        if let Some(real_type_dir) = confined_dir(&member_root, &type_dir)? {
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
                let rel_posix = rel_posix_path(&member_root, &path);
                match confined_read_bytes(&member_root, &path) {
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
                    Err(FsError::InvalidGitRef(msg)) => all_diags.push(Diagnostic {
                        repo: repo_name.clone(),
                        path: rel_posix,
                        byte_start: 0,
                        code: "type-read-error".to_owned(),
                        severity: Severity::Error,
                        message: msg,
                    }),
                }
            }
        }

        let mut stack = vec![member_root.clone()];
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
                if dir == member_root.join(".2brain") && name_str == "engine" {
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
                    if !is_confined(&member_root, &path)? {
                        continue;
                    }
                    if path.is_dir() {
                        continue;
                    }
                }
                if is_dir {
                    stack.push(path);
                } else if path.extension().and_then(|e| e.to_str()) == Some("md") {
                    let Ok(rel) = path.strip_prefix(&member_root) else {
                        continue;
                    };
                    let rel_posix = rel.to_string_lossy().replace('\\', "/");
                    let Ok(loc) = Locator::new(&repo_name, &rel_posix) else {
                        continue;
                    };
                    let bytes = match confined_read_bytes(&member_root, &path) {
                        Ok(Some(b)) => b,
                        Ok(None) | Err(FsError::SymlinkEscape(_)) => continue,
                        Err(err) => return Err(err),
                    };
                    if bytes.iter().take(8192).any(|&b| b == 0) {
                        continue;
                    }
                    if ft.is_symlink() {
                        indexed_symlinks.insert(loc.clone());
                    }
                    let sha256 = RevisionDigest::of_bytes(&bytes);
                    let short_hash = WriteGuardHash::of_bytes(&bytes).to_hex();
                    match parse_document(&bytes) {
                        Ok(doc) => {
                            let diags = registry.validate_document(&repo_name, &rel_posix, &doc);
                            all_diags.extend(diags);
                            let raw_text = String::from_utf8(bytes).unwrap_or_default();
                            let description = doc.frontmatter.as_ref().and_then(|fm| {
                                match fm.get("description") {
                                    Some(FrontmatterValue::String(s)) => Some(s.clone()),
                                    _ => None,
                                }
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
                                path_index
                                    .insert((repo_name.clone(), no_ext.to_owned()), loc.clone());
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

        let mut deps = read_repo_deps(&member_root);
        for d in registry.deps_of(&repo_name) {
            if !deps.contains(&d) {
                deps.push(d);
            }
        }
        deps.sort();
        members.insert(
            repo_name.clone(),
            MemberMeta {
                repo: repo_name,
                root: member_root.display().to_string(),
                role,
                deps,
            },
        );
    }

    for matches in stem_index.values_mut() {
        matches.sort();
    }

    let mut locators = Vec::with_capacity(files.len());
    let mut edges: Vec<(Locator, Locator, EdgeKind)> = Vec::new();
    for (loc, file) in &files {
        locators.push(loc.clone());
        for link in &file.doc.wikilinks {
            let target_repo = link.repo.as_deref().unwrap_or(&loc.repo);
            if !members.contains_key(target_repo) {
                continue;
            }
            if let Some(target_loc) = path_index.get(&(target_repo.to_owned(), link.target.clone()))
            {
                edges.push((loc.clone(), target_loc.clone(), link.kind));
            } else if let Some(matches) =
                stem_index.get(&(target_repo.to_owned(), link.target.clone()))
                && matches.len() == 1
            {
                edges.push((loc.clone(), matches[0].clone(), link.kind));
            }
        }
    }

    let graph = TypedLinkGraph::build(locators, &edges);
    let (search_weights, freshness_policy) = resolve_workspace_search_config(
        &canonical_root,
        ws_yaml_valid_text.as_deref(),
        &members,
        &mut all_diags,
    )?;
    let search = build_rich_search_index(
        &files,
        current_utc_epoch_days(),
        search_weights,
        freshness_policy,
    );

    let store = FsWorkspaceStore::multi(canonical_root, store_members);
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
        indexed_symlinks,
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
    #[error("invalid git ref: {0}")]
    InvalidGitRef(String),
}

fn validate_git_ref(git_ref: &str) -> Result<(), FsError> {
    if git_ref.is_empty() || git_ref.len() > 256 {
        return Err(FsError::InvalidGitRef(
            "git ref must be 1..=256 characters".into(),
        ));
    }
    if git_ref.starts_with('-') {
        return Err(FsError::InvalidGitRef(
            "git ref cannot start with '-'".into(),
        ));
    }
    if git_ref.contains("..") || git_ref.contains("@{") {
        return Err(FsError::InvalidGitRef(
            "git ref cannot contain range or reflog operators".into(),
        ));
    }
    if git_ref.bytes().any(|b| {
        b <= 0x20
            || b == 0x7f
            || matches!(
                b,
                b'\\' | b':' | b'?' | b'*' | b'[' | b';' | b'|' | b'&' | b'\0'
            )
    }) {
        return Err(FsError::InvalidGitRef(
            "git ref contains forbidden characters".into(),
        ));
    }
    Ok(())
}

/// Load a read-only [`VaultSnapshot`] purely from Git object storage at `git_ref`
/// (`HEAD~1`, commit SHA, tag, or branch) without mutating the working tree.
pub fn load_snapshot_at_git_ref(root: &Path, git_ref: &str) -> Result<VaultSnapshot, FsError> {
    validate_git_ref(git_ref)?;
    let canonical_root = fs::canonicalize(root).map_err(|source| FsError::Io {
        path: root.to_path_buf(),
        source,
    })?;

    let rev_spec = format!("{git_ref}^{{commit}}");
    let rev_out = Command::new("git")
        .arg("-C")
        .arg(&canonical_root)
        .args(["rev-parse", "--verify", "--end-of-options", &rev_spec])
        .output()
        .map_err(|source| FsError::Io {
            path: canonical_root.clone(),
            source,
        })?;
    if !rev_out.status.success() {
        let stderr = String::from_utf8_lossy(&rev_out.stderr).trim().to_owned();
        return Err(FsError::InvalidGitRef(format!(
            "cannot resolve git ref '{git_ref}': {stderr}"
        )));
    }
    let commit_sha = String::from_utf8_lossy(&rev_out.stdout).trim().to_owned();
    if commit_sha.len() < 40 || !commit_sha.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(FsError::InvalidGitRef(format!(
            "unexpected commit sha '{commit_sha}'"
        )));
    }

    let ls_out = Command::new("git")
        .arg("-C")
        .arg(&canonical_root)
        .args(["ls-tree", "-r", "-z", "--full-tree", &commit_sha])
        .output()
        .map_err(|source| FsError::Io {
            path: canonical_root.clone(),
            source,
        })?;
    if !ls_out.status.success() {
        let stderr = String::from_utf8_lossy(&ls_out.stderr).trim().to_owned();
        return Err(FsError::InvalidGitRef(format!(
            "git ls-tree failed: {stderr}"
        )));
    }

    // Parse NUL-terminated `ls-tree` records: `<mode> <type> <oid>\t<path>`.
    // Fail closed on malformed or unterminated records so a corrupted stream can never
    // silently produce a partial historical snapshot.
    if !ls_out.stdout.is_empty() && !ls_out.stdout.ends_with(&[0]) {
        return Err(FsError::Io {
            path: canonical_root.clone(),
            source: std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "git ls-tree output was not NUL-terminated",
            ),
        });
    }
    let mut selected: Vec<(String, String)> = Vec::new(); // (rel_posix_path, oid)
    let raw_entries = ls_out.stdout.strip_suffix(&[0]).unwrap_or(&ls_out.stdout);
    if !raw_entries.is_empty() {
        for entry in raw_entries.split(|&b| b == 0) {
            let Some(tab_pos) = entry.iter().position(|&b| b == b'\t') else {
                return Err(FsError::Io {
                    path: canonical_root.clone(),
                    source: std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "malformed git ls-tree record: missing tab delimiter",
                    ),
                });
            };
            let header = std::str::from_utf8(&entry[..tab_pos]).map_err(|e| FsError::Io {
                path: canonical_root.clone(),
                source: std::io::Error::new(std::io::ErrorKind::InvalidData, e),
            })?;
            let Ok(rel_path) = std::str::from_utf8(&entry[tab_pos + 1..]) else {
                continue;
            };
            let mut parts = header.split_ascii_whitespace();
            let (Some(mode), Some(kind), Some(oid), None) =
                (parts.next(), parts.next(), parts.next(), parts.next())
            else {
                return Err(FsError::Io {
                    path: canonical_root.clone(),
                    source: std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("malformed git ls-tree header: '{header}'"),
                    ),
                });
            };
            if oid.len() < 40 || !oid.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(FsError::Io {
                    path: canonical_root.clone(),
                    source: std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("malformed git ls-tree oid: '{oid}'"),
                    ),
                });
            }
            // Skip symlinks (120000), gitlinks/submodules (160000), and non-blobs.
            if kind != "blob" || mode == "120000" || mode == "160000" {
                continue;
            }
            if rel_path.starts_with(".git/") || rel_path.starts_with(".2brain/engine/") {
                continue;
            }
            let wanted = rel_path == "workspace.yaml"
                || rel_path == ".2brain/repo.yaml"
                || rel_path == ".2brain/ignore"
                || (rel_path.starts_with("type/")
                    && rel_path.ends_with(".type.yaml")
                    && !rel_path["type/".len()..].contains('/'))
                || rel_path.ends_with(".md");
            if wanted {
                selected.push((rel_path.to_owned(), oid.to_owned()));
            }
        }
    }
    selected.sort_by(|a, b| a.0.cmp(&b.0));

    // Fetch all selected blobs in a single `git cat-file --batch` pass (fail closed on any truncation).
    let blobs = read_git_blobs_batch(&canonical_root, &selected)?;

    let mut repo_name = canonical_root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("workspace")
        .to_owned();
    if let Some(repo_yaml_bytes) = blobs.get(".2brain/repo.yaml")
        && let Ok(text) = std::str::from_utf8(repo_yaml_bytes)
        && let Ok(docs) = Yaml::load_from_str(text)
        && let Some(Yaml::Mapping(map)) = docs.first()
    {
        for (k, v) in map {
            if let (Yaml::Value(Scalar::String(ks)), Yaml::Value(Scalar::String(vs))) = (k, v)
                && ks.as_ref() == "name"
                && !vs.is_empty()
            {
                repo_name = vs.to_string();
            }
        }
    }

    // Apply `.2brain/ignore` from the selected commit with identical path+parent rules to live scans.
    let mut all_diags: Vec<Diagnostic> = Vec::new();
    let mut ignore_builder = GitignoreBuilder::new(&canonical_root);
    if let Some(ignore_bytes) = blobs.get(".2brain/ignore") {
        match std::str::from_utf8(ignore_bytes) {
            Ok(text) => {
                for line in text.lines() {
                    if let Err(err) = ignore_builder.add_line(None, line) {
                        all_diags.push(Diagnostic {
                            repo: repo_name.clone(),
                            path: ".2brain/ignore".to_owned(),
                            byte_start: 0,
                            code: "invalid-ignore".to_owned(),
                            severity: Severity::Error,
                            message: err.to_string(),
                        });
                    }
                }
            }
            Err(_) => {
                all_diags.push(Diagnostic {
                    repo: repo_name.clone(),
                    path: ".2brain/ignore".to_owned(),
                    byte_start: 0,
                    code: "invalid-utf8".to_owned(),
                    severity: Severity::Error,
                    message: "ignore file contains invalid UTF-8 bytes".to_owned(),
                });
            }
        }
    }
    let gitignore = ignore_builder
        .build()
        .unwrap_or_else(|_| Gitignore::empty());
    let is_ignored = |rel_path: &str| {
        gitignore
            .matched_path_or_any_parents(canonical_root.join(rel_path), false)
            .is_ignore()
    };

    let mut registry = SchemaRegistry::new();
    for (rel_path, bytes) in &blobs {
        if is_ignored(rel_path) {
            continue;
        }
        if let Some(stem) = rel_path
            .strip_prefix("type/")
            .and_then(|rest| rest.strip_suffix(".type.yaml"))
            && !stem.contains('/')
            && let Ok(raw) = std::str::from_utf8(bytes)
            && let Ok(def) = parse_type_def(&repo_name, stem, raw)
        {
            registry.insert(def);
        }
    }

    let mut files: BTreeMap<Locator, IndexedFile> = BTreeMap::new();
    let mut path_index: BTreeMap<(String, String), Locator> = BTreeMap::new();
    let mut stem_index: BTreeMap<(String, String), Vec<Locator>> = BTreeMap::new();

    for (rel_posix, bytes) in &blobs {
        if !rel_posix.ends_with(".md")
            || is_ignored(rel_posix)
            || bytes.iter().take(8192).any(|&b| b == 0)
        {
            continue;
        }
        let Ok(loc) = Locator::new(&repo_name, rel_posix) else {
            continue;
        };
        let sha256 = RevisionDigest::of_bytes(bytes);
        let short_hash = WriteGuardHash::of_bytes(bytes).to_hex();
        match parse_document(bytes) {
            Ok(doc) => {
                all_diags.extend(registry.validate_document(&repo_name, rel_posix, &doc));
                let raw_text = String::from_utf8(bytes.clone()).unwrap_or_default();
                let description =
                    doc.frontmatter
                        .as_ref()
                        .and_then(|fm| match fm.get("description") {
                            Some(FrontmatterValue::String(s)) => Some(s.clone()),
                            _ => None,
                        });
                let declared_type = doc
                    .frontmatter
                    .as_ref()
                    .and_then(|fm| match fm.get("type") {
                        Some(FrontmatterValue::String(s)) => Some(s.clone()),
                        _ => None,
                    });
                let file_name = rel_posix.rsplit('/').next().unwrap_or(rel_posix);
                if let Some(stem) = file_name.strip_suffix(".md") {
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
                    path: rel_posix.clone(),
                    byte_start: 0,
                    code: "invalid-utf8".to_owned(),
                    severity: Severity::Error,
                    message: "file contains invalid UTF-8 bytes".to_owned(),
                });
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
    let ws_search = match blobs.get("workspace.yaml") {
        Some(bytes) => match std::str::from_utf8(bytes) {
            Ok(raw) => {
                parse_search_config_from_yaml(&repo_name, "workspace.yaml", raw, &mut all_diags)
            }
            Err(_) => {
                all_diags.push(Diagnostic {
                    repo: repo_name.clone(),
                    path: "workspace.yaml".to_owned(),
                    byte_start: 0,
                    code: "invalid-utf8".to_owned(),
                    severity: Severity::Error,
                    message: "workspace.yaml contains invalid UTF-8 bytes".to_owned(),
                });
                None
            }
        },
        None => None,
    };
    let repo_search = match blobs.get(".2brain/repo.yaml") {
        Some(bytes) => match std::str::from_utf8(bytes) {
            Ok(raw) => {
                parse_search_config_from_yaml(&repo_name, ".2brain/repo.yaml", raw, &mut all_diags)
            }
            Err(_) => {
                all_diags.push(Diagnostic {
                    repo: repo_name.clone(),
                    path: ".2brain/repo.yaml".to_owned(),
                    byte_start: 0,
                    code: "invalid-utf8".to_owned(),
                    severity: Severity::Error,
                    message: "repo manifest contains invalid UTF-8 bytes".to_owned(),
                });
                None
            }
        },
        None => None,
    };
    let (search_weights, freshness_policy) = ws_search.or(repo_search).unwrap_or_default();
    let search = build_rich_search_index(
        &files,
        current_utc_epoch_days(),
        search_weights,
        freshness_policy,
    );

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

    Ok(VaultSnapshot {
        generation: Generation(0),
        members,
        files,
        registry,
        graph,
        search,
        diagnostics: all_diags,
        path_index,
        stem_index,
        indexed_symlinks: std::collections::BTreeSet::new(),
    })
}

fn read_git_blobs_batch(
    canonical_root: &Path,
    selected: &[(String, String)],
) -> Result<BTreeMap<String, Vec<u8>>, FsError> {
    let mut out = BTreeMap::new();
    if selected.is_empty() {
        return Ok(out);
    }

    let mut child = Command::new("git")
        .arg("-C")
        .arg(canonical_root)
        .args(["cat-file", "--batch"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|source| FsError::Io {
            path: canonical_root.to_path_buf(),
            source,
        })?;

    let mut stdin = child.stdin.take().ok_or_else(|| FsError::Io {
        path: canonical_root.to_path_buf(),
        source: std::io::Error::other("git cat-file stdin unavailable"),
    })?;
    let stdout = child.stdout.take().ok_or_else(|| FsError::Io {
        path: canonical_root.to_path_buf(),
        source: std::io::Error::other("git cat-file stdout unavailable"),
    })?;

    let read_res: Result<BTreeMap<String, Vec<u8>>, std::io::Error> = std::thread::scope(|s| {
        let writer = s.spawn(move || -> std::io::Result<()> {
            for (_, oid) in selected {
                stdin.write_all(oid.as_bytes())?;
                stdin.write_all(b"\n")?;
            }
            stdin.flush()?;
            drop(stdin);
            Ok(())
        });

        let mut reader = BufReader::new(stdout);
        let mut header = String::new();
        for (rel_path, expected_oid) in selected {
            header.clear();
            let n = reader.read_line(&mut header)?;
            if n == 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    format!("git cat-file --batch ended prematurely before reading '{rel_path}'"),
                ));
            }
            if !header.ends_with('\n') {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    format!("git cat-file --batch header missing newline for '{rel_path}'"),
                ));
            }
            let trimmed = header.trim_end_matches(['\r', '\n']);
            let mut parts = trimmed.split_ascii_whitespace();
            let (Some(got_oid), Some(kind), Some(size_str), None) =
                (parts.next(), parts.next(), parts.next(), parts.next())
            else {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("malformed git cat-file --batch header for '{rel_path}': '{trimmed}'"),
                ));
            };
            if got_oid != expected_oid || kind != "blob" {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "unexpected git object for '{rel_path}': expected {expected_oid} blob, got {got_oid} {kind}"
                    ),
                ));
            }
            let size: usize = size_str
                .parse()
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
            let mut buf = vec![0u8; size];
            reader.read_exact(&mut buf)?;
            // Consume exact trailing LF emitted by `git cat-file --batch`.
            let mut nl = [0u8; 1];
            reader.read_exact(&mut nl)?;
            if nl[0] != b'\n' {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("missing trailing LF delimiter after git blob '{rel_path}'"),
                ));
            }
            out.insert(rel_path.clone(), buf);
        }

        let writer_res = writer
            .join()
            .map_err(|_| std::io::Error::other("git cat-file writer thread panicked"))?;
        writer_res?;
        if out.len() != selected.len() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                format!(
                    "git cat-file returned {} of {} requested blobs",
                    out.len(),
                    selected.len()
                ),
            ));
        }
        Ok(out)
    });

    let status = child.wait().map_err(|source| FsError::Io {
        path: canonical_root.to_path_buf(),
        source,
    })?;
    let blobs = read_res.map_err(|source| FsError::Io {
        path: canonical_root.to_path_buf(),
        source,
    })?;
    if !status.success() {
        return Err(FsError::Io {
            path: canonical_root.to_path_buf(),
            source: std::io::Error::other(format!("git cat-file --batch exited with {status}")),
        });
    }
    Ok(blobs)
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
                Err(FsError::InvalidGitRef(msg)) => {
                    all_diags.push(Diagnostic {
                        repo: repo_name.clone(),
                        path: rel_posix,
                        byte_start: 0,
                        code: "type-read-error".to_owned(),
                        severity: Severity::Error,
                        message: msg,
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

fn current_utc_epoch_days() -> Option<i64> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| (d.as_secs() / 86_400) as i64)
}

fn yaml_safe_scalar(val: &str) -> String {
    let is_plain_safe = !val.is_empty()
        && !matches!(
            val.to_ascii_lowercase().as_str(),
            "true" | "false" | "null" | "~" | "yes" | "no" | "on" | "off"
        )
        && !val.starts_with('-')
        && val
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b'/'));
    if is_plain_safe {
        val.to_owned()
    } else {
        serde_json::to_string(val).unwrap_or_else(|_| "\"\"".to_owned())
    }
}

fn validate_rendered_yaml_mapping(manifest_rel: &str, rendered: &str) -> Result<(), StoreError> {
    let docs = Yaml::load_from_str(rendered).map_err(|err| {
        StoreError::InvalidEdit(format!(
            "refusing to write invalid YAML for {manifest_rel}: {err}"
        ))
    })?;
    if !matches!(docs.first(), Some(Yaml::Mapping(_))) {
        return Err(StoreError::InvalidEdit(format!(
            "refusing to write non-mapping YAML for {manifest_rel}"
        )));
    }
    Ok(())
}

fn is_top_level_yaml_key(line: &str, key: &str) -> bool {
    if line.starts_with(' ') || line.starts_with('\t') || line.starts_with('#') {
        return false;
    }
    let trimmed_rest = if let Some(r) = line.strip_prefix(key) {
        r
    } else if let Some(r) = line
        .strip_prefix('"')
        .and_then(|s| s.strip_prefix(key))
        .and_then(|s| s.strip_prefix('"'))
    {
        r
    } else if let Some(r) = line
        .strip_prefix('\'')
        .and_then(|s| s.strip_prefix(key))
        .and_then(|s| s.strip_prefix('\''))
    {
        r
    } else {
        return false;
    };
    let after_space = trimmed_rest.trim_start_matches([' ', '\t']);
    let Some(rest) = after_space.strip_prefix(':') else {
        return false;
    };
    rest.is_empty()
        || rest.starts_with(' ')
        || rest.starts_with('\t')
        || rest.starts_with('#')
        || rest.starts_with('[')
        || rest.starts_with('{')
}

fn is_yaml_doc_end_marker(line: &str) -> bool {
    let Some(rest) = line.strip_prefix("...") else {
        return false;
    };
    rest.is_empty() || rest.starts_with(' ') || rest.starts_with('\t') || rest.starts_with('#')
}

fn flow_bracket_delta(line: &str) -> isize {
    let mut delta: isize = 0;
    let mut in_single = false;
    let mut in_double = false;
    let mut escaped = false;
    for ch in line.chars() {
        if in_double {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_double = false;
            }
            continue;
        }
        if in_single {
            if ch == '\'' {
                in_single = false;
            }
            continue;
        }
        match ch {
            '#' => break,
            '"' => in_double = true,
            '\'' => in_single = true,
            '[' | '{' => delta += 1,
            ']' | '}' => delta -= 1,
            _ => {}
        }
    }
    delta
}

/// Non-destructively splice a top-level YAML key/block (`deps:` or `members:`) inside an
/// existing block-YAML manifest, preserving all unrelated keys (`search:`, comments, custom
/// metadata), zero-indented block sequences, multiline flow values, and `...` end markers.
fn splice_yaml_top_level_section(
    existing_text: &str,
    key: &str,
    replacement_block: &str,
) -> String {
    let mut out_lines: Vec<String> = Vec::new();
    let mut pending_trivia: Vec<String> = Vec::new();
    let mut skipping = false;
    let mut flow_depth: usize = 0;
    let mut replaced_in_place = false;

    for raw_line in existing_text.lines() {
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        if skipping {
            if flow_depth > 0 {
                let next = (flow_depth as isize + flow_bracket_delta(line)).max(0) as usize;
                flow_depth = next;
                pending_trivia.clear();
                continue;
            }
            if line.is_empty() || line.starts_with('#') {
                pending_trivia.push(line.to_owned());
                continue;
            }
            let is_block_continuation = line.starts_with(' ')
                || line.starts_with('\t')
                || line == "-"
                || line.starts_with("- ")
                || line.starts_with("-\t");
            if is_block_continuation {
                pending_trivia.clear();
                continue;
            }
            // Reached the next top-level key or document marker; preserve trivia preceding it.
            out_lines.append(&mut pending_trivia);
            skipping = false;
        }

        if is_yaml_doc_end_marker(line) {
            if !replaced_in_place && !replacement_block.is_empty() {
                for dl in replacement_block.lines() {
                    out_lines.push(dl.to_owned());
                }
                replaced_in_place = true;
            }
            out_lines.push(line.to_owned());
            continue;
        }

        if is_top_level_yaml_key(line, key) {
            skipping = true;
            flow_depth = flow_bracket_delta(line).max(0) as usize;
            if !replaced_in_place && !replacement_block.is_empty() {
                for dl in replacement_block.lines() {
                    out_lines.push(dl.to_owned());
                }
                replaced_in_place = true;
            }
            continue;
        }
        out_lines.push(line.to_owned());
    }

    if skipping {
        out_lines.append(&mut pending_trivia);
    }

    if !replaced_in_place && !replacement_block.is_empty() {
        for dl in replacement_block.lines() {
            out_lines.push(dl.to_owned());
        }
    }

    let mut rendered = out_lines.join("\n");
    if !rendered.is_empty() && !rendered.ends_with('\n') {
        rendered.push('\n');
    }
    rendered
}

/// Non-destructively splice the top-level `deps:` section in `.2brain/repo.yaml`,
/// preserving unrelated keys (such as `search:`, comments, and custom metadata).
fn splice_repo_yaml_deps(
    existing_text: Option<&str>,
    fallback_name: &str,
    deps: &[String],
) -> String {
    let mut deps_block = String::new();
    if !deps.is_empty() {
        deps_block.push_str("deps:\n");
        for d in deps {
            deps_block.push_str(&format!("  - name: {}\n", yaml_safe_scalar(d)));
        }
    }

    match existing_text {
        Some(text) if !text.trim().is_empty() => {
            splice_yaml_top_level_section(text, "deps", &deps_block)
        }
        _ => {
            let mut out = format!("name: {}\n", yaml_safe_scalar(fallback_name));
            out.push_str(&deps_block);
            out
        }
    }
}

fn manifest_config_warning(
    repo: &str,
    manifest_rel: &str,
    code: &str,
    message: impl Into<String>,
) -> Diagnostic {
    Diagnostic {
        repo: repo.to_owned(),
        path: manifest_rel.to_owned(),
        byte_start: 0,
        code: code.to_owned(),
        severity: Severity::Warning,
        message: message.into(),
    }
}

fn yaml_finite_f64(val: &Yaml<'_>) -> Option<f64> {
    let parsed = match val {
        Yaml::Value(Scalar::FloatingPoint(f)) => Some(f.into_inner()),
        Yaml::Value(Scalar::Integer(i)) => Some(*i as f64),
        Yaml::Value(Scalar::String(s)) => s.trim().parse::<f64>().ok(),
        _ => None,
    }?;
    parsed.is_finite().then_some(parsed)
}

fn parse_search_config_from_yaml(
    repo: &str,
    manifest_rel: &str,
    raw: &str,
    diags: &mut Vec<Diagnostic>,
) -> Option<(SearchWeights, FreshnessPolicy)> {
    let mut weights = SearchWeights::default();
    let mut policy = FreshnessPolicy::default();
    let docs = match Yaml::load_from_str(raw) {
        Ok(docs) => docs,
        Err(err) => {
            diags.push(manifest_config_warning(
                repo,
                manifest_rel,
                "invalid-repo-config",
                format!("failed to parse {manifest_rel}: {err}"),
            ));
            return None;
        }
    };
    let first_doc = docs.into_iter().next()?;
    let Yaml::Mapping(root) = first_doc else {
        diags.push(manifest_config_warning(
            repo,
            manifest_rel,
            "invalid-repo-config",
            format!("{manifest_rel} root must be a YAML mapping"),
        ));
        return None;
    };
    let search_val = root.iter().find_map(|(k, v)| {
        matches!(k, Yaml::Value(Scalar::String(s)) if s == "search").then_some(v)
    })?;
    let Yaml::Mapping(search_map) = search_val else {
        diags.push(manifest_config_warning(
            repo,
            manifest_rel,
            "invalid-repo-config",
            format!("search section in {manifest_rel} must be a YAML mapping"),
        ));
        return Some((weights, policy));
    };

    for (k, v) in search_map {
        let Yaml::Value(Scalar::String(section)) = k else {
            diags.push(manifest_config_warning(
                repo,
                manifest_rel,
                "invalid-repo-config",
                "search section contains a non-string key",
            ));
            continue;
        };
        match section.as_ref() {
            "weights" => {
                let Yaml::Mapping(wmap) = v else {
                    diags.push(manifest_config_warning(
                        repo,
                        manifest_rel,
                        "invalid-repo-config",
                        "search.weights must be a YAML mapping",
                    ));
                    continue;
                };
                for (wk, wv) in wmap {
                    let Yaml::Value(Scalar::String(field)) = wk else {
                        diags.push(manifest_config_warning(
                            repo,
                            manifest_rel,
                            "invalid-repo-config",
                            "search.weights contains a non-string key",
                        ));
                        continue;
                    };
                    let Some(num) = yaml_finite_f64(wv).filter(|n| *n >= 0.0) else {
                        diags.push(manifest_config_warning(
                            repo,
                            manifest_rel,
                            "invalid-repo-config",
                            format!(
                                "search.weights.{field} must be a finite number >= 0.0; keeping default"
                            ),
                        ));
                        continue;
                    };
                    match field.as_ref() {
                        "body" => weights.body = num,
                        "description" => weights.description = num,
                        "keywords" => weights.keywords = num,
                        "search_questions" => weights.search_questions = num,
                        other => diags.push(manifest_config_warning(
                            repo,
                            manifest_rel,
                            "invalid-repo-config",
                            format!("unknown search.weights field '{other}'"),
                        )),
                    }
                }
            }
            "freshness" => {
                let Yaml::Mapping(fmap) = v else {
                    diags.push(manifest_config_warning(
                        repo,
                        manifest_rel,
                        "invalid-repo-config",
                        "search.freshness must be a YAML mapping",
                    ));
                    continue;
                };
                for (fk, fv) in fmap {
                    let Yaml::Value(Scalar::String(field)) = fk else {
                        diags.push(manifest_config_warning(
                            repo,
                            manifest_rel,
                            "invalid-repo-config",
                            "search.freshness contains a non-string key",
                        ));
                        continue;
                    };
                    match field.as_ref() {
                        "transient_half_life_days" => {
                            if let Some(num) = yaml_finite_f64(fv).filter(|n| *n > 0.0) {
                                policy.transient_half_life_days = num;
                            } else {
                                diags.push(manifest_config_warning(
                                    repo,
                                    manifest_rel,
                                    "invalid-repo-config",
                                    "search.freshness.transient_half_life_days must be a finite number > 0.0; keeping default",
                                ));
                            }
                        }
                        "min_transient_time_factor" => {
                            if let Some(num) =
                                yaml_finite_f64(fv).filter(|n| (0.01..=1.0).contains(n))
                            {
                                policy.min_transient_time_factor = num;
                            } else {
                                diags.push(manifest_config_warning(
                                    repo,
                                    manifest_rel,
                                    "invalid-repo-config",
                                    "search.freshness.min_transient_time_factor must be a finite number in [0.01, 1.0]; keeping default",
                                ));
                            }
                        }
                        other => diags.push(manifest_config_warning(
                            repo,
                            manifest_rel,
                            "invalid-repo-config",
                            format!("unknown search.freshness field '{other}'"),
                        )),
                    }
                }
            }
            other => diags.push(manifest_config_warning(
                repo,
                manifest_rel,
                "invalid-repo-config",
                format!("unknown search config section '{other}'"),
            )),
        }
    }
    Some((weights.sanitized(), policy.sanitized()))
}

fn read_manifest_search_config(
    canonical_root: &Path,
    manifest_rel: &str,
    repo: &str,
    diags: &mut Vec<Diagnostic>,
) -> Result<Option<(SearchWeights, FreshnessPolicy)>, FsError> {
    let manifest_path = canonical_root.join(manifest_rel);
    let Some(bytes) = confined_read_bytes(canonical_root, &manifest_path)? else {
        return Ok(None);
    };
    let Ok(raw) = std::str::from_utf8(&bytes) else {
        diags.push(Diagnostic {
            repo: repo.to_owned(),
            path: manifest_rel.to_owned(),
            byte_start: 0,
            code: "invalid-utf8".to_owned(),
            severity: Severity::Error,
            message: format!("{manifest_rel} contains invalid UTF-8 bytes"),
        });
        return Ok(None);
    };
    Ok(parse_search_config_from_yaml(
        repo,
        manifest_rel,
        raw,
        diags,
    ))
}

fn resolve_workspace_search_config(
    canonical_root: &Path,
    ws_yaml_valid_text: Option<&str>,
    members: &BTreeMap<String, MemberMeta>,
    diags: &mut Vec<Diagnostic>,
) -> Result<(SearchWeights, FreshnessPolicy), FsError> {
    let root_repo = members
        .values()
        .find(|m| Path::new(&m.root) == canonical_root)
        .map(|m| m.repo.as_str())
        .unwrap_or("workspace");

    // Priority 1: workspace-level `workspace.yaml` `search:` section (already read & validated once).
    let ws_cfg = ws_yaml_valid_text
        .and_then(|raw| parse_search_config_from_yaml(root_repo, "workspace.yaml", raw, diags));

    // Always validate root and member `.2brain/repo.yaml` confinement, UTF-8, and YAML syntax
    // even when `workspace.yaml` already supplied the winning `search:` config.
    let root_cfg =
        read_manifest_search_config(canonical_root, ".2brain/repo.yaml", root_repo, diags)?;

    let mut member_chosen: Option<(String, SearchWeights, FreshnessPolicy)> = None;
    for member in members.values() {
        let member_root = Path::new(&member.root);
        if member_root == canonical_root {
            continue;
        }
        let member_cfg =
            read_manifest_search_config(member_root, ".2brain/repo.yaml", &member.repo, diags)?;
        if member.role == "dependency" {
            continue;
        }
        if let Some((weights, policy)) = member_cfg
            && ws_cfg.is_none()
            && root_cfg.is_none()
        {
            if let Some((ref first_repo, _, _)) = member_chosen {
                diags.push(manifest_config_warning(
                    &member.repo,
                    ".2brain/repo.yaml",
                    "duplicate-search-config",
                    format!(
                        "ignoring search config in member '{}'; already configured by '{}'",
                        member.repo, first_repo
                    ),
                ));
            } else {
                member_chosen = Some((member.repo.clone(), weights, policy));
            }
        }
    }

    Ok(ws_cfg
        .or(root_cfg)
        .or_else(|| member_chosen.map(|(_, w, p)| (w, p)))
        .unwrap_or_else(|| (SearchWeights::default(), FreshnessPolicy::default())))
}

fn build_rich_search_index(
    files: &BTreeMap<Locator, IndexedFile>,
    reference_days: Option<i64>,
    weights: SearchWeights,
    freshness_policy: FreshnessPolicy,
) -> SearchIndex {
    let owned: Vec<(&IndexedFile, String, String, FreshnessMeta)> = files
        .values()
        .map(|f| {
            let (kw, sq) = extract_enrichment_fields(f);
            (f, kw, sq, extract_freshness_meta(f))
        })
        .collect();
    let mut idx = SearchIndex::build_rich_with_weights(
        owned.iter().map(|(f, kw, sq, fresh)| SearchDocInput {
            locator: f.locator.clone(),
            body: f.body(),
            description: f.description.as_deref().unwrap_or(""),
            keywords: kw.as_str(),
            search_questions: sq.as_str(),
            freshness: *fresh,
        }),
        reference_days,
        weights,
    );
    idx.set_freshness_policy(freshness_policy);
    idx
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

    #[test]
    fn independent_store_instances_serialize_cas_via_interprocess_write_lock() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("note.md"), "rev1\n").unwrap();
        let canonical = fs::canonicalize(dir.path()).unwrap();
        let store_a = FsWorkspaceStore::single("ws", canonical.clone());
        let store_b = FsWorkspaceStore::single("ws", canonical);
        let loc = Locator::new("ws", "note.md").unwrap();
        let (_, r1) = store_a.read_exact(&loc).unwrap();
        let guard = r1.write_guard.to_hex();

        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let b1 = std::sync::Arc::clone(&barrier);
        let b2 = std::sync::Arc::clone(&barrier);
        let loc1 = loc.clone();
        let loc2 = loc.clone();
        let g1 = guard.clone();
        let g2 = guard;

        let t1 = std::thread::spawn(move || {
            b1.wait();
            store_a.write_file_cas(&loc1, &g1, "rev2-a\n", "a")
        });
        let t2 = std::thread::spawn(move || {
            b2.wait();
            store_b.write_file_cas(&loc2, &g2, "rev2-b\n", "b")
        });

        let r_a = t1.join().unwrap();
        let r_b = t2.join().unwrap();
        assert!(
            (r_a.is_ok() && matches!(r_b, Err(StoreError::CasMismatch(_))))
                || (r_b.is_ok() && matches!(r_a, Err(StoreError::CasMismatch(_)))),
            "expected exactly one winner and one CasMismatch, got a={r_a:?}, b={r_b:?}"
        );
    }

    #[test]
    fn git_ref_loader_rejects_unsafe_refs_and_diffs_historical_commit_without_checkout() {
        for bad in [
            "-n",
            "--upload-pack=x",
            "HEAD..main",
            "HEAD@{0}",
            "a b",
            "x:y",
            "",
        ] {
            assert!(
                validate_git_ref(bad).is_err(),
                "expected git ref '{bad}' to be rejected"
            );
        }

        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let run_git = |args: &[&str]| {
            let st = Command::new("git")
                .arg("-C")
                .arg(root)
                .args(args)
                .status()
                .unwrap();
            assert!(st.success(), "git {args:?} failed");
        };
        run_git(&["init", "-q"]);
        run_git(&["config", "user.name", "Test"]);
        run_git(&["config", "user.email", "test@example.com"]);
        fs::create_dir_all(root.join(".2brain")).unwrap();
        fs::write(root.join(".2brain/ignore"), "secret.md\n").unwrap();
        fs::write(root.join("secret.md"), "ignored secret note\n").unwrap();
        fs::write(root.join("source.md"), "source v1\n").unwrap();
        fs::write(root.join("derived.md"), "depends on [[source]]\n").unwrap();
        run_git(&["add", "."]);
        run_git(&["commit", "-q", "-m", "v1"]);

        // Commit v2 modifies source.md and adds extra.md; derived.md stays untouched.
        fs::write(root.join("source.md"), "source v2 updated\n").unwrap();
        fs::write(root.join("extra.md"), "links [[source]]\n").unwrap();
        run_git(&["add", "."]);
        run_git(&["commit", "-q", "-m", "v2"]);

        let (store, snap) = load_snapshot(root).unwrap();
        let app = tb_app::App::new(store, snap);
        let diff_json = app.diff_graphs("HEAD~1", None, None, 10).unwrap();
        let d = &diff_json["diff_graphs"];
        assert_eq!(d["added_nodes_count"], 1);
        assert_eq!(d["removed_nodes_count"], 0);
        assert_eq!(d["modified_nodes_count"], 1);
        assert_eq!(d["decay_candidates"][0]["locator"]["path"], "derived.md");

        // Unchanged HEAD -> WORKING_TREE must NOT report ignored `secret.md` as removed.
        let clean_diff = app.diff_graphs("HEAD", None, None, 10).unwrap();
        assert_eq!(clean_diff["diff_graphs"]["removed_nodes_count"], 0);
        assert_eq!(clean_diff["diff_graphs"]["added_nodes_count"], 0);
    }

    #[test]
    fn ambiguous_conflicts_with_stem_surfaces_explicit_ambiguous_reference_warning() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("a")).unwrap();
        fs::create_dir_all(root.join("b")).unwrap();
        fs::write(
            root.join("a/decision.md"),
            "---\ntype: claim\npolarity: positive\n---\nPole A\n",
        )
        .unwrap();
        fs::write(
            root.join("b/decision.md"),
            "---\ntype: claim\npolarity: positive\n---\nPole B\n",
        )
        .unwrap();
        fs::write(
            root.join("counter.md"),
            "---\ntype: claim\npolarity: negative\nconflicts_with: decision\n---\nCounter\n",
        )
        .unwrap();

        let (store, snap) = load_snapshot(root).unwrap();
        let app = tb_app::App::new(store, snap);
        let audit = app.audit_contradictions(Some("2026-11-28"), 20).unwrap();
        let conflicts = audit["audit_contradictions"]["conflicts"]
            .as_array()
            .unwrap();
        assert!(
            conflicts.iter().any(|c| c["kind"] == "ambiguous_reference"),
            "expected explicit ambiguous_reference warning, got {conflicts:?}"
        );
    }

    #[test]
    fn wave_a_structural_mutations_preview_reconcile_watch_and_deps_work_end_to_end() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("type")).unwrap();
        fs::write(
            root.join("type/draft.type.yaml"),
            "fields:\n  status:\n    type: string\n",
        )
        .unwrap();
        fs::write(
            root.join("target.md"),
            "---\ntype: draft\nstatus: current\n---\n\nImportant core paragraph.\n",
        )
        .unwrap();
        fs::write(root.join("caller.md"), "Refer to [[target]].\n").unwrap();

        let (store, snap) = load_snapshot(root).unwrap();
        let repo = snap.members.keys().next().unwrap().clone();
        let app = Arc::new(tb_app::App::new(store, snap));
        let target_loc = Locator::new(&repo, "target.md").unwrap();
        let caller_loc = Locator::new(&repo, "caller.md").unwrap();

        // 1. preview_mutation catches stale CAS and validates clean edit.
        let h0 = app.snapshot().files[&target_loc].short_hash.clone();
        let bad_preview = app
            .preview_mutation(
                json!({
                    "op": "edit_file",
                    "repo": repo,
                    "path": "target.md",
                    "expected_hash": "deadbeefdeadbeef",
                    "old_str": "Important",
                    "new_str": "Updated"
                })
                .as_object()
                .unwrap(),
            )
            .unwrap();
        assert_eq!(bad_preview["preview_mutation"]["would_succeed"], false);

        // 2. assign_block_id -> rename_block_id (rewriting inbound wikilink).
        app.assign_block_id(
            &target_loc,
            &h0,
            "fact-1",
            tb_app::BlockTarget::MatchText("Important"),
            None,
        )
        .unwrap();
        let h_caller = app.snapshot().files[&caller_loc].short_hash.clone();
        app.write_file(
            &caller_loc,
            &h_caller,
            "Refer to [[target#^fact-1|fact]].\n",
            None,
        )
        .unwrap();
        let h1 = app.snapshot().files[&target_loc].short_hash.clone();
        let rb = app
            .rename_block_id(&target_loc, &h1, "fact-1", "core-1", None)
            .unwrap();
        assert_eq!(
            rb.payload["rename_block_id"]["updated_references"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert!(
            app.snapshot().files[&caller_loc]
                .raw_text
                .contains("^core-1")
        );

        // 3. promote block into new note, then inline it back into caller.
        let h2 = app.snapshot().files[&target_loc].short_hash.clone();
        app.promote_block(
            &target_loc,
            &h2,
            tb_app::PromoteSource::BlockId("core-1"),
            "promoted.md",
            Some("draft"),
            None,
        )
        .unwrap();
        assert!(
            app.snapshot()
                .files
                .contains_key(&Locator::new(&repo, "promoted.md").unwrap())
        );
        let h3 = app.snapshot().files[&target_loc].short_hash.clone();
        app.inline_note(&target_loc, &h3, "promoted", None).unwrap();
        assert!(
            app.snapshot().files[&target_loc]
                .raw_text
                .contains("Important core paragraph.")
        );

        // 4. rename_type updates schema file + instance frontmatter.
        let rt = app.rename_type(&repo, "draft", "note", None).unwrap();
        assert!(
            !rt.payload["rename_type"]["updated_instances"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert!(
            app.snapshot()
                .registry
                .iter()
                .any(|t| t.repo == repo && t.name == "note")
        );
        assert!(
            !app.snapshot()
                .registry
                .iter()
                .any(|t| t.repo == repo && t.name == "draft")
        );

        // 5. batch + add_dep + remove_dep + reconcile + watch wakeup.
        let batch = app
            .batch_mutations(
                &[json!({
                    "op": "create_file",
                    "repo": repo,
                    "path": "batched.md",
                    "content": "hello batch\n"
                })],
                None,
            )
            .unwrap();
        assert_eq!(batch.payload["batch"]["completed"], 1);

        let added = app.mutate_dep(&repo, "shared-lib", true, None).unwrap();
        assert_eq!(added.payload["add_dep"]["changed"], true);
        let removed = app.mutate_dep(&repo, "shared-lib", false, None).unwrap();
        assert_eq!(removed.payload["remove_dep"]["changed"], true);

        let rec = app
            .reconcile(&Locator::new(&repo, "batched.md").unwrap(), None, None)
            .unwrap();
        assert_eq!(rec["reconcile"]["status"], "matched");

        let app_bg = Arc::clone(&app);
        let gen_before = app.generation().0;
        let waiter = std::thread::spawn(move || app_bg.watch(Some(gen_before), 3_000).unwrap());
        std::thread::sleep(std::time::Duration::from_millis(40));
        app.create_file(&Locator::new(&repo, "wake.md").unwrap(), "wake\n", None)
            .unwrap();
        let watch_res = waiter.join().unwrap();
        assert_eq!(watch_res["watch"]["changed"], true);
        assert!(watch_res["watch"]["generation"].as_u64().unwrap() > gen_before);
    }

    #[test]
    fn multi_member_workspace_yaml_resolves_cross_repo_links_and_member_mutations() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("alpha")).unwrap();
        fs::create_dir_all(root.join("beta")).unwrap();
        fs::create_dir_all(root.join("gamma")).unwrap();
        fs::write(
            root.join("workspace.yaml"),
            "members:\n  - name: alpha\n    path: alpha\n    role: own\n  - name: beta\n    path: beta\n    role: dependency\n",
        )
        .unwrap();
        fs::write(
            root.join("alpha/a.md"),
            "Links across repo to [[b::beta]].\n",
        )
        .unwrap();
        fs::write(root.join("beta/b.md"), "Target in beta.\n").unwrap();
        fs::write(root.join("gamma/g.md"), "Note in gamma.\n").unwrap();

        let (store, snap) = load_snapshot(root).unwrap();
        assert_eq!(snap.members.len(), 2);
        assert_eq!(snap.graph.edge_count(), 1);
        let app = tb_app::App::new(store, snap);

        app.mutate_workspace_member(
            WorkspaceMemberAction::Add {
                name: "gamma".into(),
                path: "gamma".into(),
                role: "dependency".into(),
            },
            None,
        )
        .unwrap();
        assert_eq!(app.snapshot().members.len(), 3);
        assert!(
            app.snapshot()
                .files
                .contains_key(&Locator::new("gamma", "g.md").unwrap())
        );

        app.mutate_workspace_member(
            WorkspaceMemberAction::SetRole {
                name: "gamma".into(),
                role: "own".into(),
            },
            None,
        )
        .unwrap();
        assert_eq!(app.snapshot().members["gamma"].role, "own");

        app.mutate_workspace_member(
            WorkspaceMemberAction::Remove {
                name: "gamma".into(),
            },
            None,
        )
        .unwrap();
        assert_eq!(app.snapshot().members.len(), 2);

        // Post-load symlink swap of a member directory must fail closed with SymlinkEscape.
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("pwn.md"), "outside\n").unwrap();
        fs::remove_dir_all(root.join("beta")).unwrap();
        std::os::unix::fs::symlink(outside.path(), root.join("beta")).unwrap();
        let err = app
            .store()
            .read_exact(&Locator::new("beta", "pwn.md").unwrap())
            .unwrap_err();
        assert!(
            matches!(err, StoreError::SymlinkEscape(_)),
            "expected SymlinkEscape after member dir symlink swap, got {err:?}"
        );
    }

    #[test]
    fn persistent_negative_memory_suppresses_dismissed_gap_across_restarts() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        // Two disconnected notes with rich shared rare technical vocabulary -> produces a structural gap.
        fs::write(
            root.join("alpha.md"),
            "Deterministic quasar telemetry calibration hypergraph lattice synchronization protocol.\n",
        )
        .unwrap();
        fs::write(
            root.join("beta.md"),
            "Deterministic quasar telemetry calibration hypergraph lattice synchronization benchmark.\n",
        )
        .unwrap();
        fs::write(
            root.join("other1.md"),
            "Unrelated cooking recipe banana bread.\n",
        )
        .unwrap();
        fs::write(
            root.join("other2.md"),
            "Unrelated gardening tomato soil compost.\n",
        )
        .unwrap();
        fs::write(
            root.join("other3.md"),
            "Unrelated sailing wind compass harbor.\n",
        )
        .unwrap();

        let (store1, snap1) = load_snapshot(root).unwrap();
        let app1 = tb_app::App::new(store1, snap1);
        let before = app1
            .detect_gaps(&std::collections::HashSet::new(), 10)
            .unwrap();
        let gaps_before = before["detect_gaps"]["gaps"].as_array().unwrap();
        assert!(
            !gaps_before.is_empty(),
            "expected at least one structural gap"
        );
        let qid = gaps_before[0]["question_id"].as_str().unwrap().to_owned();

        // Persist dismissal in Negative Memory.
        app1.record_decision(
            &qid,
            "question",
            "dismiss",
            "intentional separation",
            Some("tester"),
        )
        .unwrap();

        // Reload a completely fresh App instance (simulating daemon restart).
        let (store2, snap2) = load_snapshot(root).unwrap();
        let app2 = tb_app::App::new(store2, snap2);
        let after = app2
            .detect_gaps(&std::collections::HashSet::new(), 10)
            .unwrap();
        let gaps_after = after["detect_gaps"]["gaps"].as_array().unwrap();
        assert!(
            !gaps_after.iter().any(|g| g["question_id"] == qid),
            "dismissed question '{qid}' must stay suppressed across restart"
        );
        let listed = app2.list_decisions(Some("question")).unwrap();
        assert_eq!(listed["list_decisions"]["count"], 1);

        // Corrupted JSONL line must fail closed instead of silently restoring dismissed questions.
        let log_path = root.join(".2brain/engine/review-decisions.jsonl");
        let mut raw = fs::read(&log_path).unwrap();
        raw.extend_from_slice(b"{not valid json\n");
        fs::write(&log_path, raw).unwrap();
        assert!(app2.list_decisions(None).is_err());
        assert!(
            app2.record_decision("q_next", "question", "dismiss", "nope", None)
                .is_err()
        );
    }

    #[test]
    fn incremental_refresh_matches_full_reload_across_create_edit_and_delete() {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        fs::write(root.join("a.md"), "Alpha links [[b]].\n").unwrap();
        fs::write(root.join("b.md"), "Beta original.\n").unwrap();

        let (_, s1) = load_snapshot(&root).unwrap();
        assert_eq!(s1.files.len(), 2);
        assert_eq!(s1.graph.edge_count(), 1);

        // Create c.md and edit b.md -> incremental refresh must match fresh full scan.
        fs::write(root.join("c.md"), "Gamma links [[a]] and [[b]].\n").unwrap();
        fs::write(root.join("b.md"), "Beta edited unique_token_v2.\n").unwrap();
        let inc_add = refresh_snapshot_incremental(
            &root,
            &s1,
            &[root.join("c.md"), root.join("b.md")],
            Generation(2),
        )
        .unwrap();
        let full_add = load_snapshot_with_generation(&root, Generation(2)).unwrap();
        assert_eq!(inc_add.files.len(), full_add.files.len());
        assert_eq!(inc_add.graph.edge_count(), full_add.graph.edge_count());
        assert_eq!(
            inc_add
                .search
                .query("unique_token_v2", false, None, None, 5)
                .len(),
            full_add
                .search
                .query("unique_token_v2", false, None, None, 5)
                .len()
        );

        // Delete c.md -> incremental refresh removes node and edges cleanly.
        fs::remove_file(root.join("c.md")).unwrap();
        let inc_del =
            refresh_snapshot_incremental(&root, &inc_add, &[root.join("c.md")], Generation(3))
                .unwrap();
        let full_del = load_snapshot_with_generation(&root, Generation(3)).unwrap();
        assert_eq!(inc_del.files.len(), full_del.files.len());
        assert_eq!(inc_del.graph.edge_count(), full_del.graph.edge_count());

        // Confined symlink alias must also refresh when its target is edited.
        std::os::unix::fs::symlink(root.join("b.md"), root.join("alias.md")).unwrap();
        let s_sym = load_snapshot_with_generation(&root, Generation(4)).unwrap();
        assert!(!s_sym.indexed_symlinks.is_empty());
        fs::write(root.join("b.md"), "Beta v3_symlink_sync.\n").unwrap();
        let inc_sym =
            refresh_snapshot_incremental(&root, &s_sym, &[root.join("b.md")], Generation(5))
                .unwrap();
        let repo = inc_sym.members.keys().next().unwrap().clone();
        assert!(
            inc_sym.files[&Locator::new(&repo, "alias.md").unwrap()]
                .raw_text
                .contains("v3_symlink_sync"),
            "symlink alias must reflect target edit after incremental refresh"
        );
    }

    #[test]
    fn repo_yaml_search_config_overrides_bm25_weights_and_freshness_policy() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join(".2brain")).unwrap();
        fs::write(
            root.join(".2brain/repo.yaml"),
            "name: custom\nsearch:\n  weights:\n    body: 1.0\n    description: 0.5\n    keywords: 5.0\n    search_questions: 2.0\n  freshness:\n    transient_half_life_days: 30\n    min_transient_time_factor: 0.2\n",
        )
        .unwrap();
        fs::write(
            root.join("desc-hit.md"),
            "---\ndescription: quorum consensus note\n---\n\nneutral body\n",
        )
        .unwrap();
        fs::write(
            root.join("kw-hit.md"),
            "---\ndescription: neutral note\nkeywords: [\"quorum consensus\"]\n---\n\nneutral body\n",
        )
        .unwrap();

        let (_, snap) = load_snapshot(root).expect("load workspace with search config");
        assert_eq!(snap.search.weights().keywords, 5.0);
        assert_eq!(snap.search.weights().description, 0.5);
        assert_eq!(
            snap.search.freshness_policy().transient_half_life_days,
            30.0
        );
        assert_eq!(
            snap.search.freshness_policy().min_transient_time_factor,
            0.2
        );

        let hits = snap.search.query("quorum consensus", false, None, None, 2);
        assert_eq!(hits.len(), 2);
        assert_eq!(
            hits[0].locator.path, "kw-hit.md",
            "custom keywords weight (5.0) must outrank description weight (0.5)"
        );
    }

    #[test]
    fn update_repo_deps_preserves_search_config_and_comments() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join(".2brain")).unwrap();
        let initial = "# custom manifest\nname: main\ndescription: \"Main vault\"\nsearch:\n  weights:\n    keywords: 4.5\n  freshness:\n    transient_half_life_days: 45\n";
        fs::write(root.join(".2brain/repo.yaml"), initial).unwrap();
        fs::write(root.join("note.md"), "# Note\n").unwrap();

        let (store, _) = load_snapshot(root).unwrap();
        store.update_repo_deps("main", "lib-a", true).unwrap();
        let after_add = fs::read_to_string(root.join(".2brain/repo.yaml")).unwrap();
        assert!(
            after_add.contains("keywords: 4.5"),
            "add_dep must preserve search.weights"
        );
        assert!(
            after_add.contains("transient_half_life_days: 45"),
            "add_dep must preserve search.freshness"
        );
        assert!(after_add.contains("- name: lib-a"));

        store.update_repo_deps("main", "lib-a", false).unwrap();
        let after_rm = fs::read_to_string(root.join(".2brain/repo.yaml")).unwrap();
        assert!(
            after_rm.contains("keywords: 4.5"),
            "remove_dep must preserve search.weights"
        );
        assert!(!after_rm.contains("lib-a"));

        let (_, snap) = load_snapshot(root).unwrap();
        assert_eq!(snap.search.weights().keywords, 4.5);
        assert_eq!(
            snap.search.freshness_policy().transient_half_life_days,
            45.0
        );
    }

    #[test]
    fn invalid_repo_search_config_emits_diagnostics_and_preserves_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join(".2brain")).unwrap();
        fs::write(
            root.join(".2brain/repo.yaml"),
            "name: bad\nsearch:\n  weights:\n    body: -1.0\n    description: .nan\n    keywords: \"inf\"\n    search_questions: 2.5\n  freshness:\n    transient_half_life_days: 0\n    min_transient_time_factor: 2.0\n",
        )
        .unwrap();
        fs::write(root.join("note.md"), "# Note\n").unwrap();

        let (_, snap) = load_snapshot(root).unwrap();
        let defaults = SearchWeights::default();
        assert_eq!(
            snap.search.weights().body,
            defaults.body,
            "negative weight must preserve default instead of coercing to 0.0"
        );
        assert_eq!(snap.search.weights().description, defaults.description);
        assert_eq!(snap.search.weights().keywords, defaults.keywords);
        assert_eq!(snap.search.weights().search_questions, 2.5);
        assert_eq!(
            snap.search.freshness_policy(),
            FreshnessPolicy::default(),
            "invalid freshness fields must preserve defaults"
        );
        let cfg_diags: Vec<_> = snap
            .diagnostics
            .iter()
            .filter(|d| d.code == "invalid-repo-config")
            .collect();
        assert!(
            cfg_diags.len() >= 5,
            "expected diagnostics for each invalid field, got: {cfg_diags:?}"
        );
    }

    #[test]
    fn multi_member_workspace_reads_own_member_search_config() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let own_dir = root.join("own-vault");
        let dep_dir = root.join("dep-lib");
        fs::create_dir_all(own_dir.join(".2brain")).unwrap();
        fs::create_dir_all(dep_dir.join(".2brain")).unwrap();
        fs::write(
            root.join("workspace.yaml"),
            "members:\n  - name: own-vault\n    path: own-vault\n    role: own\n  - name: dep-lib\n    path: dep-lib\n    role: dependency\n",
        )
        .unwrap();
        fs::write(
            own_dir.join(".2brain/repo.yaml"),
            "name: own-vault\nsearch:\n  weights:\n    keywords: 6.0\n  freshness:\n    transient_half_life_days: 15\n",
        )
        .unwrap();
        fs::write(
            dep_dir.join(".2brain/repo.yaml"),
            "name: dep-lib\nsearch:\n  weights:\n    keywords: 99.0\n",
        )
        .unwrap();
        fs::write(own_dir.join("a.md"), "# Own\n").unwrap();
        fs::write(dep_dir.join("b.md"), "# Dep\n").unwrap();

        let (_, snap) = load_snapshot(root).unwrap();
        assert_eq!(
            snap.search.weights().keywords,
            6.0,
            "own member search config must be honored and dependency member ignored"
        );
        assert_eq!(
            snap.search.freshness_policy().transient_half_life_days,
            15.0
        );
    }

    #[test]
    fn workspace_yaml_search_config_overrides_member_repo_yaml_and_survives_member_edits() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let own_dir = root.join("own-vault");
        let extra_dir = root.join("extra-lib");
        fs::create_dir_all(own_dir.join(".2brain")).unwrap();
        fs::create_dir_all(extra_dir.join(".2brain")).unwrap();
        fs::write(
            root.join("workspace.yaml"),
            "# workspace config\nmembers:\n  - name: own-vault\n    path: own-vault\n    role: own\nsearch:\n  weights:\n    keywords: 7.25\n  freshness:\n    transient_half_life_days: 20\n",
        )
        .unwrap();
        fs::write(
            own_dir.join(".2brain/repo.yaml"),
            "name: own-vault\nsearch:\n  weights:\n    keywords: 3.0\n",
        )
        .unwrap();
        fs::write(own_dir.join("a.md"), "# Own\n").unwrap();
        fs::write(extra_dir.join("b.md"), "# Extra\n").unwrap();

        let (store, snap) = load_snapshot(root).unwrap();
        assert_eq!(
            snap.search.weights().keywords,
            7.25,
            "workspace.yaml search config must take precedence over member repo.yaml"
        );
        assert_eq!(
            snap.search.freshness_policy().transient_half_life_days,
            20.0
        );

        store
            .update_workspace_member(WorkspaceMemberAction::Add {
                name: "extra-lib".to_owned(),
                path: "extra-lib".to_owned(),
                role: "dependency".to_owned(),
            })
            .unwrap();
        let ws_after = fs::read_to_string(root.join("workspace.yaml")).unwrap();
        assert!(ws_after.contains("# workspace config"));
        assert!(ws_after.contains("keywords: 7.25"));
        assert!(ws_after.contains("transient_half_life_days: 20"));

        let (_, snap2) = load_snapshot(root).unwrap();
        assert_eq!(snap2.search.weights().keywords, 7.25);
        assert_eq!(snap2.members.len(), 2);
    }

    #[test]
    fn splice_yaml_handles_zero_indent_sequences_flow_and_doc_end_marker() {
        let zero_indent = "members:\n- name: a\n  path: a\n# keep search comment\nsearch:\n  weights:\n    keywords: 4.0\n...\n";
        let spliced = splice_yaml_top_level_section(
            zero_indent,
            "members",
            "members:\n  - name: a\n    path: a\n  - name: b\n    path: b\n",
        );
        validate_rendered_yaml_mapping("workspace.yaml", &spliced).unwrap();
        assert!(spliced.contains("# keep search comment"));
        assert!(spliced.contains("keywords: 4.0"));
        assert!(spliced.contains("- name: b"));
        assert!(!spliced.contains("\n- name: a"));

        let flow_seq = "members: [\n  {name: a, path: a}\n]\nsearch:\n  weights:\n    body: 2.0\n";
        let spliced_flow = splice_yaml_top_level_section(
            flow_seq,
            "members",
            "members:\n  - name: a\n    path: a\n",
        );
        validate_rendered_yaml_mapping("workspace.yaml", &spliced_flow).unwrap();
        assert!(spliced_flow.contains("body: 2.0"));
        assert!(!spliced_flow.contains("\n]\n"));

        let doc_end = "name: v\nsearch:\n  weights:\n    body: 1.5\n...\n";
        let with_deps = splice_repo_yaml_deps(Some(doc_end), "v", &["dep-x".to_owned()]);
        validate_rendered_yaml_mapping(".2brain/repo.yaml", &with_deps).unwrap();
        let deps_pos = with_deps.find("deps:").unwrap();
        let end_pos = with_deps.find("...").unwrap();
        assert!(
            deps_pos < end_pos,
            "inserted deps must precede `...` marker"
        );
    }

    #[test]
    fn malformed_workspace_yaml_emits_error_and_validates_member_repo_yaml_under_ws_search() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join("workspace.yaml"), "members: [\nunclosed").unwrap();
        fs::write(root.join("secret.md"), "# Secret\n").unwrap();

        let (_, snap) = load_snapshot(root).unwrap();
        assert!(
            snap.files.is_empty(),
            "malformed workspace.yaml must not silently fall back to scanning root as primary"
        );
        assert!(
            snap.diagnostics.iter().any(|d| d.path == "workspace.yaml"
                && d.code == "invalid-workspace-manifest"
                && d.severity == Severity::Error),
            "expected error diagnostic for malformed workspace.yaml, got: {:?}",
            snap.diagnostics
        );

        // Even when workspace.yaml provides `search:`, an escaping symlink in a member's
        // `.2brain/repo.yaml` must still be caught fail-closed.
        let dir2 = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let outside_file = outside.path().join("leak.yaml");
        fs::write(&outside_file, "name: leak\n").unwrap();
        let root2 = dir2.path();
        let own = root2.join("own");
        fs::create_dir_all(own.join(".2brain")).unwrap();
        fs::write(
            root2.join("workspace.yaml"),
            "members:\n  - name: own\n    path: own\n    role: own\nsearch:\n  weights:\n    keywords: 5.0\n",
        )
        .unwrap();
        std::os::unix::fs::symlink(&outside_file, own.join(".2brain/repo.yaml")).unwrap();
        let err = load_snapshot(root2).expect_err("escaping member repo.yaml must fail closed");
        assert!(matches!(err, FsError::SymlinkEscape(_)), "got: {err:?}");
    }
}
