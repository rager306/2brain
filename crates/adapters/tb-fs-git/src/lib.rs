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
use std::sync::{Arc, Mutex};

use fs4::fs_std::FileExt;
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use rustix::fs::{
    AtFlags, Mode, OFlags, RenameFlags, fsync, mkdirat, open, openat, readlinkat, renameat,
    renameat_with, statat, unlinkat,
};
use rustix::io::Errno;
use saphyr::{LoadableYamlNode, Scalar, Yaml};
use tb_app::{IndexedFile, MemberMeta, MutationReceipt, StoreError, VaultSnapshot, WorkspaceStore};
use tb_contracts::{
    CasVerified, EvidenceReceipt, Generation, Locator, RevisionDigest, WriteGuardHash, WriteIntent,
};
use tb_graph::{GraphHealth, GraphShape, HubRow, TypedLinkGraph};
use tb_schema::{Diagnostic, SchemaRegistry, Severity, parse_type_def};
use tb_search::SearchIndex;
use tb_syntax::{EdgeKind, FrontmatterValue, ParsedDocument, parse_document};
use thiserror::Error;

static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// Confined filesystem and Git workspace store (read-only + atomic CAS mutations).
#[derive(Clone, Debug)]
pub struct FsWorkspaceStore {
    members: BTreeMap<String, PathBuf>,
    write_lock: Arc<Mutex<()>>,
}

impl FsWorkspaceStore {
    #[must_use]
    pub fn single(repo_name: impl Into<String>, canonical_root: PathBuf) -> Self {
        let mut members = BTreeMap::new();
        members.insert(repo_name.into(), canonical_root);
        Self {
            members,
            write_lock: Arc::new(Mutex::new(())),
        }
    }

    #[must_use]
    pub fn member_root(&self, repo: &str) -> Option<&Path> {
        self.members.get(repo).map(PathBuf::as_path)
    }

    fn root_for(&self, repo: &str) -> Result<&PathBuf, StoreError> {
        self.members
            .get(repo)
            .ok_or_else(|| StoreError::NotFound(format!("unknown member '{repo}'")))
    }
}

impl WorkspaceStore for FsWorkspaceStore {
    fn read_exact(&self, locator: &Locator) -> Result<(Vec<u8>, EvidenceReceipt), StoreError> {
        let canonical_root = self.root_for(&locator.repo)?;
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
        let Some(root) = self.members.values().next() else {
            return Ok(None);
        };
        let snap = load_snapshot_with_generation(root, next_generation)
            .map_err(|e| StoreError::Io(e.to_string()))?;
        Ok(Some(snap))
    }

    fn load_snapshot_at_git_ref(
        &self,
        repo: &str,
        git_ref: &str,
    ) -> Result<VaultSnapshot, StoreError> {
        let canonical_root = self.root_for(repo)?;
        load_snapshot_at_git_ref(canonical_root, git_ref).map_err(|e| match e {
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
        let _lock = acquire_workspace_write_lock(&self.write_lock, canonical_root)?;
        let (parent_fd, file_name) = open_confined_parent_dir(canonical_root, &locator.path, true)?;
        if statat(&parent_fd, &file_name, AtFlags::SYMLINK_NOFOLLOW).is_ok() {
            return Err(StoreError::AlreadyExists(format!(
                "{}/{}",
                locator.repo, locator.path
            )));
        }
        atomic_write_in_dirfd(&parent_fd, &file_name, content.as_bytes())?;
        let commit = match git_commit_paths(
            canonical_root,
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
        let _lock = acquire_workspace_write_lock(&self.write_lock, canonical_root)?;
        let target = canonical_root.join(&locator.path);
        let prior_bytes = verify_live_cas(canonical_root, &target, locator, expected_hash)?;
        let (parent_fd, file_name) =
            open_confined_parent_dir(canonical_root, &locator.path, false)?;
        reject_symlink_leaf(&parent_fd, &file_name)?;
        atomic_write_in_dirfd(&parent_fd, &file_name, content.as_bytes())?;
        let commit = match git_commit_paths(
            canonical_root,
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
        let _lock = acquire_workspace_write_lock(&self.write_lock, canonical_root)?;
        let target = canonical_root.join(&locator.path);
        let live_bytes = verify_live_cas(canonical_root, &target, locator, expected_hash)?;
        let live_text = std::str::from_utf8(&live_bytes)
            .map_err(|_| StoreError::InvalidUtf8(format!("{}/{}", locator.repo, locator.path)))?;
        if old_str.is_empty() || !live_text.contains(old_str) {
            return Err(StoreError::InvalidEdit(
                "target text not found in current file".to_owned(),
            ));
        }
        let updated = live_text.replacen(old_str, new_str, 1);
        let (parent_fd, file_name) =
            open_confined_parent_dir(canonical_root, &locator.path, false)?;
        reject_symlink_leaf(&parent_fd, &file_name)?;
        atomic_write_in_dirfd(&parent_fd, &file_name, updated.as_bytes())?;
        let commit = match git_commit_paths(
            canonical_root,
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
        let _lock = acquire_workspace_write_lock(&self.write_lock, canonical_root)?;
        let target = canonical_root.join(&locator.path);
        let prior_bytes = verify_live_cas(canonical_root, &target, locator, expected_hash)?;
        let (parent_fd, file_name) =
            open_confined_parent_dir(canonical_root, &locator.path, false)?;
        reject_symlink_leaf(&parent_fd, &file_name)?;
        unlinkat(&parent_fd, &file_name, AtFlags::empty())
            .map_err(|e| StoreError::Io(format!("unlinkat {}: {e}", locator.path)))?;
        match git_commit_paths(canonical_root, &[locator.path.as_str()], commit_message) {
            Ok(c) => Ok(c),
            Err(git_err) => match atomic_write_in_dirfd(&parent_fd, &file_name, &prior_bytes) {
                Ok(()) => Err(git_err),
                Err(restore_err) => Err(StoreError::PostEffectFailure(format!(
                    "git commit failed ({git_err}) and rollback restore failed ({restore_err})"
                ))),
            },
        }
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
        let _lock = acquire_workspace_write_lock(&self.write_lock, canonical_root)?;
        let src = canonical_root.join(&from.path);
        let live_bytes = verify_live_cas(canonical_root, &src, from, expected_hash)?;
        let (src_parent_fd, src_name) =
            open_confined_parent_dir(canonical_root, &from.path, false)?;
        reject_symlink_leaf(&src_parent_fd, &src_name)?;
        let (dst_parent_fd, dst_name) = open_confined_parent_dir(canonical_root, &to.path, true)?;
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
            canonical_root,
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
            let wanted = rel_path == ".2brain/repo.yaml"
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
}
