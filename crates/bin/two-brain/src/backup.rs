//! Verified workspace backup and restore (`U12`/`R25`/`AE25`, clean-room MIT).
//!
//! Security & integrity invariants:
//! - Every directory and file read inside `<workspace>` and `<backup>/data` uses
//!   directory-descriptor `openat(..., O_NOFOLLOW)` + `fstat` (zero TOCTOU symlink swaps).
//! - Any symlink, FIFO, or device file inside the source workspace or backup payload fails
//!   closed immediately.
//! - Transient engine lockfiles (`*.lock` under `.2brain/engine/` or `.git/index.lock`) and
//!   Unix sockets under `.2brain/engine/run/` are skipped safely.
//! - Backup writes `<out>/data/<rel>` plus `<out>/manifest.sha256.json` (`2brain.backup/v1`)
//!   with per-file SHA-256 + canonical `manifest_digest`.
//! - Restore enforces strict bijective verification (every manifest entry exists with exact
//!   byte length + SHA-256, and zero extra untracked files exist in `<backup>/data/`),
//!   refuses non-empty `--to` targets, and validates the restored workspace via
//!   `tb_fs_git::load_snapshot`.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rustix::fs::{FileType, Mode, OFlags, fstat, openat};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

const BACKUP_SCHEMA_VERSION: &str = "2brain.backup/v1";
const MAX_SINGLE_FILE_BYTES: u64 = 256 * 1024 * 1024; // 256 MiB safety cap per file
const MAX_TOTAL_FILES: usize = 100_000;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BackupFileEntry {
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BackupManifest {
    pub schema_version: String,
    pub two_brain_version: String,
    pub created_unix: u64,
    pub file_count: usize,
    pub total_bytes: u64,
    pub manifest_digest: String,
    pub files: Vec<BackupFileEntry>,
}

#[derive(Clone, Debug, Serialize)]
pub struct BackupSummary {
    pub workspace: String,
    pub output_dir: String,
    pub schema_version: String,
    pub file_count: usize,
    pub total_bytes: u64,
    pub manifest_digest: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct RestoreSummary {
    pub backup_dir: String,
    pub target_dir: String,
    pub manifest_digest: String,
    pub restored_files: usize,
    pub restored_bytes: u64,
    pub verified_workspace_files: usize,
    pub verified_workspace_types: usize,
    pub verified_workspace_diagnostics: usize,
}

pub fn run_backup(workspace: &Path, out_dir: &Path, as_json: bool) {
    match backup_workspace(workspace, out_dir) {
        Ok(summary) => {
            if as_json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&summary).expect("json serialization")
                );
            } else {
                println!(
                    "backup ok: {} -> {} (files={}, bytes={}, digest={})",
                    summary.workspace,
                    summary.output_dir,
                    summary.file_count,
                    summary.total_bytes,
                    &summary.manifest_digest[..16]
                );
            }
        }
        Err(err) => {
            if as_json {
                println!("{}", json!({ "ok": false, "error": err }));
            } else {
                eprintln!("backup error: {err}");
            }
            std::process::exit(1);
        }
    }
}

pub fn run_restore(backup_dir: &Path, target_dir: &Path, as_json: bool) {
    match restore_workspace(backup_dir, target_dir) {
        Ok(summary) => {
            if as_json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&summary).expect("json serialization")
                );
            } else {
                println!(
                    "restore ok: {} -> {} (files={}, bytes={}, digest={}, loaded_notes={}, types={}, diagnostics={})",
                    summary.backup_dir,
                    summary.target_dir,
                    summary.restored_files,
                    summary.restored_bytes,
                    &summary.manifest_digest[..16],
                    summary.verified_workspace_files,
                    summary.verified_workspace_types,
                    summary.verified_workspace_diagnostics
                );
            }
        }
        Err(err) => {
            if as_json {
                println!("{}", json!({ "ok": false, "error": err }));
            } else {
                eprintln!("restore error: {err}");
            }
            std::process::exit(1);
        }
    }
}

pub fn backup_workspace(workspace: &Path, out_dir: &Path) -> Result<BackupSummary, String> {
    let canonical_ws = fs::canonicalize(workspace)
        .map_err(|e| format!("cannot resolve workspace '{}': {e}", workspace.display()))?;
    if !canonical_ws.is_dir() {
        return Err(format!(
            "workspace '{}' is not a directory",
            canonical_ws.display()
        ));
    }

    let resolved_out = resolve_target_path(out_dir)?;
    if resolved_out == canonical_ws || resolved_out.starts_with(&canonical_ws) {
        return Err(format!(
            "refusing backup output '{}' inside source workspace '{}'",
            resolved_out.display(),
            canonical_ws.display()
        ));
    }
    ensure_empty_or_new_dir(&resolved_out)?;

    let data_dir = resolved_out.join("data");
    fs::create_dir_all(&data_dir)
        .map_err(|e| format!("create backup data dir '{}': {e}", data_dir.display()))?;

    let root_fd = open_dir_nofollow_abs(&canonical_ws)?;
    let mut entries = Vec::new();
    let mut total_bytes: u64 = 0;

    copy_dir_recursive(
        root_fd.as_fd(),
        &canonical_ws,
        "",
        &data_dir,
        &mut entries,
        &mut total_bytes,
    )?;

    entries.sort_by(|a, b| a.path.cmp(&b.path));
    let manifest_digest = compute_manifest_digest(&entries);
    let created_unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let manifest = BackupManifest {
        schema_version: BACKUP_SCHEMA_VERSION.to_owned(),
        two_brain_version: env!("CARGO_PKG_VERSION").to_owned(),
        created_unix,
        file_count: entries.len(),
        total_bytes,
        manifest_digest: manifest_digest.clone(),
        files: entries,
    };

    let manifest_bytes =
        serde_json::to_vec_pretty(&manifest).map_err(|e| format!("serialize manifest: {e}"))?;
    let tmp_path = resolved_out.join(".manifest.sha256.json.tmp");
    let final_path = resolved_out.join("manifest.sha256.json");
    {
        let mut f = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&tmp_path)
            .map_err(|e| format!("create temp manifest '{}': {e}", tmp_path.display()))?;
        f.write_all(&manifest_bytes)
            .map_err(|e| format!("write temp manifest: {e}"))?;
        f.write_all(b"\n")
            .map_err(|e| format!("write temp manifest newline: {e}"))?;
        f.sync_all()
            .map_err(|e| format!("sync temp manifest: {e}"))?;
    }
    fs::rename(&tmp_path, &final_path)
        .map_err(|e| format!("publish manifest '{}': {e}", final_path.display()))?;

    Ok(BackupSummary {
        workspace: canonical_ws.display().to_string(),
        output_dir: resolved_out.display().to_string(),
        schema_version: BACKUP_SCHEMA_VERSION.to_owned(),
        file_count: manifest.file_count,
        total_bytes,
        manifest_digest,
    })
}

pub fn restore_workspace(backup_dir: &Path, target_dir: &Path) -> Result<RestoreSummary, String> {
    let canonical_backup = fs::canonicalize(backup_dir)
        .map_err(|e| format!("cannot resolve backup '{}': {e}", backup_dir.display()))?;
    if !canonical_backup.is_dir() {
        return Err(format!(
            "backup '{}' is not a directory",
            canonical_backup.display()
        ));
    }

    let resolved_target = resolve_target_path(target_dir)?;
    if resolved_target == canonical_backup || resolved_target.starts_with(&canonical_backup) {
        return Err(format!(
            "refusing restore target '{}' inside backup directory '{}'",
            resolved_target.display(),
            canonical_backup.display()
        ));
    }

    let backup_fd = open_dir_nofollow_abs(&canonical_backup)?;
    let manifest_bytes =
        read_regular_file_nofollow(backup_fd.as_fd(), "manifest.sha256.json", 16 * 1024 * 1024)?;
    let manifest: BackupManifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|e| format!("invalid backup manifest JSON: {e}"))?;

    if manifest.schema_version != BACKUP_SCHEMA_VERSION {
        return Err(format!(
            "unsupported backup schema_version '{}' (expected '{}')",
            manifest.schema_version, BACKUP_SCHEMA_VERSION
        ));
    }
    if manifest.file_count != manifest.files.len() {
        return Err(format!(
            "manifest file_count mismatch: header={} entries={}",
            manifest.file_count,
            manifest.files.len()
        ));
    }

    let mut expected_by_path: BTreeMap<String, &BackupFileEntry> = BTreeMap::new();
    let mut expected_total_bytes: u64 = 0;
    for entry in &manifest.files {
        validate_rel_posix_path(&entry.path)?;
        if entry.sha256.len() != 64
            || !entry
                .sha256
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        {
            return Err(format!(
                "invalid sha256 digest in manifest for '{}'",
                entry.path
            ));
        }
        if expected_by_path.insert(entry.path.clone(), entry).is_some() {
            return Err(format!(
                "duplicate path '{}' in backup manifest",
                entry.path
            ));
        }
        expected_total_bytes = expected_total_bytes
            .checked_add(entry.bytes)
            .ok_or_else(|| "manifest total_bytes overflow".to_owned())?;
    }
    if expected_total_bytes != manifest.total_bytes {
        return Err(format!(
            "manifest total_bytes mismatch: header={} sum={}",
            manifest.total_bytes, expected_total_bytes
        ));
    }

    let mut sorted_entries = manifest.files.clone();
    sorted_entries.sort_by(|a, b| a.path.cmp(&b.path));
    let recomputed_digest = compute_manifest_digest(&sorted_entries);
    if recomputed_digest != manifest.manifest_digest {
        return Err(format!(
            "manifest_digest mismatch: header={} computed={}",
            manifest.manifest_digest, recomputed_digest
        ));
    }

    // Walk `<backup>/data` strictly with O_NOFOLLOW to prove exact bijective set membership
    // and verify every file's bytes + SHA-256 BEFORE mutating the target directory.
    let data_fd = openat(
        backup_fd.as_fd(),
        "data",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .map_err(|e| format!("open backup data directory: {e}"))?;

    let mut observed_paths: BTreeSet<String> = BTreeSet::new();
    verify_backup_data_tree(
        data_fd.as_fd(),
        &canonical_backup.join("data"),
        "",
        &expected_by_path,
        &mut observed_paths,
    )?;

    for path in expected_by_path.keys() {
        if !observed_paths.contains(path) {
            return Err(format!("missing file '{}' listed in backup manifest", path));
        }
    }

    // Fail-closed: target must be new or empty before writing anything.
    ensure_empty_or_new_dir(&resolved_target)?;

    // Copy verified payload into target and re-hash on write.
    let mut restored_bytes: u64 = 0;
    for entry in &sorted_entries {
        let dest_path = resolved_target.join(&entry.path);
        if let Some(parent) = dest_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("create target dir '{}': {e}", parent.display()))?;
        }
        let (written_bytes, written_hex) =
            copy_verified_rel_file(data_fd.as_fd(), &entry.path, &dest_path)?;
        if written_bytes != entry.bytes || written_hex != entry.sha256 {
            return Err(format!(
                "post-copy verification failed for '{}' (expected {}:{}, got {}:{})",
                entry.path, entry.bytes, entry.sha256, written_bytes, written_hex
            ));
        }
        restored_bytes = restored_bytes.saturating_add(written_bytes);
    }

    // Prove the restored directory actually loads cleanly as a 2brain workspace.
    let (_, snap) = tb_fs_git::load_snapshot(&resolved_target).map_err(|e| {
        format!(
            "restored files verified, but workspace load failed at '{}': {e}",
            resolved_target.display()
        )
    })?;

    Ok(RestoreSummary {
        backup_dir: canonical_backup.display().to_string(),
        target_dir: resolved_target.display().to_string(),
        manifest_digest: manifest.manifest_digest,
        restored_files: sorted_entries.len(),
        restored_bytes,
        verified_workspace_files: snap.files.len(),
        verified_workspace_types: snap.registry.iter().count(),
        verified_workspace_diagnostics: snap.diagnostics.len(),
    })
}

fn compute_manifest_digest(sorted_entries: &[BackupFileEntry]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(BACKUP_SCHEMA_VERSION.as_bytes());
    hasher.update(b"\n");
    for e in sorted_entries {
        hasher.update(e.path.as_bytes());
        hasher.update([0u8]);
        hasher.update(e.bytes.to_string().as_bytes());
        hasher.update([0u8]);
        hasher.update(e.sha256.as_bytes());
        hasher.update(b"\n");
    }
    hex_encode(&hasher.finalize())
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

fn validate_rel_posix_path(rel: &str) -> Result<(), String> {
    if rel.is_empty()
        || rel.starts_with('/')
        || rel.ends_with('/')
        || rel.contains('\\')
        || rel.contains('\0')
    {
        return Err(format!("invalid relative path in manifest: '{rel}'"));
    }
    for seg in rel.split('/') {
        if seg.is_empty() || seg == "." || seg == ".." {
            return Err(format!("unsafe path segment in manifest: '{rel}'"));
        }
    }
    Ok(())
}

fn resolve_target_path(path: &Path) -> Result<PathBuf, String> {
    if path.exists() {
        let meta = fs::symlink_metadata(path)
            .map_err(|e| format!("inspect target '{}': {e}", path.display()))?;
        if meta.file_type().is_symlink() {
            return Err(format!("refusing symlink target path '{}'", path.display()));
        }
        return fs::canonicalize(path)
            .map_err(|e| format!("canonicalize target '{}': {e}", path.display()));
    }
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| format!("read cwd: {e}"))?
            .join(path)
    };
    let mut missing = Vec::new();
    let mut cur = abs.as_path();
    while !cur.exists() {
        let name = cur
            .file_name()
            .ok_or_else(|| format!("cannot resolve path '{}'", path.display()))?;
        missing.push(name.to_os_string());
        cur = cur
            .parent()
            .ok_or_else(|| format!("cannot resolve parent of '{}'", path.display()))?;
    }
    let mut base = fs::canonicalize(cur)
        .map_err(|e| format!("canonicalize ancestor '{}': {e}", cur.display()))?;
    for part in missing.into_iter().rev() {
        base.push(part);
    }
    Ok(base)
}

fn ensure_empty_or_new_dir(dir: &Path) -> Result<(), String> {
    match fs::symlink_metadata(dir) {
        Ok(meta) => {
            if meta.file_type().is_symlink() {
                return Err(format!("refusing symlink directory '{}'", dir.display()));
            }
            if !meta.is_dir() {
                return Err(format!(
                    "target '{}' exists and is not a directory",
                    dir.display()
                ));
            }
            let mut rd = fs::read_dir(dir)
                .map_err(|e| format!("read directory '{}': {e}", dir.display()))?;
            if rd.next().transpose().map_err(|e| e.to_string())?.is_some() {
                return Err(format!(
                    "refusing non-empty destination directory '{}'",
                    dir.display()
                ));
            }
            Ok(())
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => fs::create_dir_all(dir)
            .map_err(|e| format!("create destination directory '{}': {e}", dir.display())),
        Err(err) => Err(format!("inspect destination '{}': {err}", dir.display())),
    }
}

fn open_dir_nofollow_abs(path: &Path) -> Result<OwnedFd, String> {
    openat(
        rustix::fs::CWD,
        path,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .map_err(|e| format!("open directory '{}' (O_NOFOLLOW): {e}", path.display()))
}

fn should_skip_transient(rel_path: &str, file_type: FileType) -> bool {
    if file_type == FileType::Socket && rel_path.starts_with(".2brain/engine/run/") {
        return true;
    }
    if file_type == FileType::RegularFile
        && (rel_path == ".git/index.lock"
            || (rel_path.starts_with(".2brain/engine/") && rel_path.ends_with(".lock")))
    {
        return true;
    }
    false
}

fn copy_dir_recursive(
    dir_fd: BorrowedFd<'_>,
    disk_dir: &Path,
    rel_prefix: &str,
    dest_root: &Path,
    entries: &mut Vec<BackupFileEntry>,
    total_bytes: &mut u64,
) -> Result<(), String> {
    let mut names = Vec::new();
    for item in fs::read_dir(disk_dir)
        .map_err(|e| format!("read directory '{}': {e}", disk_dir.display()))?
    {
        let item = item.map_err(|e| format!("read dir entry in '{}': {e}", disk_dir.display()))?;
        let name_os = item.file_name();
        let name = name_os
            .to_str()
            .ok_or_else(|| format!("non-UTF-8 filename in '{}'", disk_dir.display()))?
            .to_owned();
        names.push(name);
    }
    names.sort();

    for name in names {
        let rel = if rel_prefix.is_empty() {
            name.clone()
        } else {
            format!("{rel_prefix}/{name}")
        };
        let child_disk = disk_dir.join(&name);

        // Open child without following symlinks first to classify safely.
        let stat_meta =
            fs::symlink_metadata(&child_disk).map_err(|e| format!("lstat '{rel}': {e}"))?;
        if stat_meta.file_type().is_symlink() {
            return Err(format!(
                "refusing symlink '{rel}' inside workspace (security policy)"
            ));
        }

        if stat_meta.is_dir() {
            let sub_fd = openat(
                dir_fd,
                name.as_str(),
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW,
                Mode::empty(),
            )
            .map_err(|e| format!("open subdirectory '{rel}' (O_NOFOLLOW): {e}"))?;
            copy_dir_recursive(
                sub_fd.as_fd(),
                &child_disk,
                &rel,
                dest_root,
                entries,
                total_bytes,
            )?;
            continue;
        }

        // Handle sockets/transient lockfiles cleanly.
        let fd = match openat(
            dir_fd,
            name.as_str(),
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK,
            Mode::empty(),
        ) {
            Ok(fd) => fd,
            Err(_) if rel.starts_with(".2brain/engine/run/") => continue,
            Err(e) => return Err(format!("open file '{rel}' (O_NOFOLLOW): {e}")),
        };
        let st = fstat(&fd).map_err(|e| format!("fstat '{rel}': {e}"))?;
        let ftype = FileType::from_raw_mode(st.st_mode);
        if should_skip_transient(&rel, ftype) {
            continue;
        }
        if ftype != FileType::RegularFile {
            return Err(format!(
                "refusing non-regular file '{rel}' (type={ftype:?})"
            ));
        }

        if entries.len() >= MAX_TOTAL_FILES {
            return Err(format!(
                "workspace exceeds backup limit of {MAX_TOTAL_FILES} files"
            ));
        }

        let dest_path = dest_root.join(&rel);
        if let Some(parent) = dest_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("create backup dir '{}': {e}", parent.display()))?;
        }
        let mut src = File::from(fd);
        let mut dst = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&dest_path)
            .map_err(|e| format!("create backup file '{rel}': {e}"))?;

        let (bytes, sha256) = stream_copy_and_hash(&mut src, &mut dst, &rel)?;
        dst.sync_all()
            .map_err(|e| format!("sync backup file '{rel}': {e}"))?;

        *total_bytes = total_bytes
            .checked_add(bytes)
            .ok_or_else(|| "backup total_bytes overflow".to_owned())?;
        entries.push(BackupFileEntry {
            path: rel,
            bytes,
            sha256,
        });
    }
    Ok(())
}

fn verify_backup_data_tree(
    dir_fd: BorrowedFd<'_>,
    disk_dir: &Path,
    rel_prefix: &str,
    expected: &BTreeMap<String, &BackupFileEntry>,
    observed: &mut BTreeSet<String>,
) -> Result<(), String> {
    let mut names = Vec::new();
    for item in fs::read_dir(disk_dir)
        .map_err(|e| format!("read backup dir '{}': {e}", disk_dir.display()))?
    {
        let item = item.map_err(|e| format!("read backup entry: {e}"))?;
        let name = item
            .file_name()
            .to_str()
            .ok_or_else(|| format!("non-UTF-8 name in backup '{}'", disk_dir.display()))?
            .to_owned();
        names.push(name);
    }
    names.sort();

    for name in names {
        let rel = if rel_prefix.is_empty() {
            name.clone()
        } else {
            format!("{rel_prefix}/{name}")
        };
        let child_disk = disk_dir.join(&name);
        let meta =
            fs::symlink_metadata(&child_disk).map_err(|e| format!("lstat backup '{rel}': {e}"))?;
        if meta.file_type().is_symlink() {
            return Err(format!("refusing symlink '{rel}' inside backup payload"));
        }
        if meta.is_dir() {
            let sub_fd = openat(
                dir_fd,
                name.as_str(),
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW,
                Mode::empty(),
            )
            .map_err(|e| format!("open backup subdir '{rel}': {e}"))?;
            verify_backup_data_tree(sub_fd.as_fd(), &child_disk, &rel, expected, observed)?;
            continue;
        }

        let Some(entry) = expected.get(&rel) else {
            return Err(format!(
                "unexpected untracked file '{rel}' in backup payload"
            ));
        };
        let fd = openat(
            dir_fd,
            name.as_str(),
            OFlags::RDONLY | OFlags::NOFOLLOW,
            Mode::empty(),
        )
        .map_err(|e| format!("open backup file '{rel}': {e}"))?;
        let st = fstat(&fd).map_err(|e| format!("fstat backup file '{rel}': {e}"))?;
        if FileType::from_raw_mode(st.st_mode) != FileType::RegularFile {
            return Err(format!("non-regular file '{rel}' inside backup payload"));
        }
        let mut src = File::from(fd);
        let (actual_bytes, actual_hex) = stream_hash_only(&mut src, &rel)?;
        if actual_bytes != entry.bytes {
            return Err(format!(
                "backup byte-length mismatch for '{rel}': expected={} actual={}",
                entry.bytes, actual_bytes
            ));
        }
        if actual_hex != entry.sha256 {
            return Err(format!(
                "backup sha256 mismatch for '{rel}': expected={} actual={}",
                entry.sha256, actual_hex
            ));
        }
        observed.insert(rel);
    }
    Ok(())
}

fn read_regular_file_nofollow(
    dir_fd: BorrowedFd<'_>,
    name: &str,
    max_bytes: u64,
) -> Result<Vec<u8>, String> {
    let fd = openat(
        dir_fd,
        name,
        OFlags::RDONLY | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .map_err(|e| format!("open '{name}' (O_NOFOLLOW): {e}"))?;
    let st = fstat(&fd).map_err(|e| format!("fstat '{name}': {e}"))?;
    if FileType::from_raw_mode(st.st_mode) != FileType::RegularFile {
        return Err(format!("'{name}' is not a regular file"));
    }
    let file = File::from(fd);
    let mut buf = Vec::new();
    file.take(max_bytes + 1)
        .read_to_end(&mut buf)
        .map_err(|e| format!("read '{name}': {e}"))?;
    if (buf.len() as u64) > max_bytes {
        return Err(format!("'{name}' exceeds {max_bytes}-byte cap"));
    }
    Ok(buf)
}

fn copy_verified_rel_file(
    data_root_fd: BorrowedFd<'_>,
    rel_path: &str,
    dest_path: &Path,
) -> Result<(u64, String), String> {
    let parts: Vec<&str> = rel_path.split('/').collect();
    let (file_name, dirs) = parts
        .split_last()
        .ok_or_else(|| format!("empty relative path '{rel_path}'"))?;
    let mut owned: Option<OwnedFd> = None;
    for dir in dirs {
        let cur = owned.as_ref().map_or(data_root_fd, AsFd::as_fd);
        let next = openat(
            cur,
            *dir,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW,
            Mode::empty(),
        )
        .map_err(|e| format!("open backup dir '{dir}' for '{rel_path}': {e}"))?;
        owned = Some(next);
    }
    let parent_fd = owned.as_ref().map_or(data_root_fd, AsFd::as_fd);
    let fd = openat(
        parent_fd,
        *file_name,
        OFlags::RDONLY | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .map_err(|e| format!("open backup file '{rel_path}': {e}"))?;
    let st = fstat(&fd).map_err(|e| format!("fstat backup file '{rel_path}': {e}"))?;
    if FileType::from_raw_mode(st.st_mode) != FileType::RegularFile {
        return Err(format!("backup entry '{rel_path}' is not a regular file"));
    }
    let mut src = File::from(fd);
    let mut dst = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dest_path)
        .map_err(|e| format!("create restored file '{}': {e}", dest_path.display()))?;
    let res = stream_copy_and_hash(&mut src, &mut dst, rel_path)?;
    dst.sync_all()
        .map_err(|e| format!("sync restored file '{}': {e}", dest_path.display()))?;
    Ok(res)
}

fn stream_copy_and_hash(
    src: &mut File,
    dst: &mut File,
    label: &str,
) -> Result<(u64, String), String> {
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 32 * 1024];
    let mut total: u64 = 0;
    loop {
        let n = src
            .read(&mut buf)
            .map_err(|e| format!("read '{label}': {e}"))?;
        if n == 0 {
            break;
        }
        total = total.saturating_add(n as u64);
        if total > MAX_SINGLE_FILE_BYTES {
            return Err(format!(
                "file '{label}' exceeds {MAX_SINGLE_FILE_BYTES}-byte safety limit"
            ));
        }
        hasher.update(&buf[..n]);
        dst.write_all(&buf[..n])
            .map_err(|e| format!("write '{label}': {e}"))?;
    }
    Ok((total, hex_encode(&hasher.finalize())))
}

fn stream_hash_only(src: &mut File, label: &str) -> Result<(u64, String), String> {
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 32 * 1024];
    let mut total: u64 = 0;
    loop {
        let n = src
            .read(&mut buf)
            .map_err(|e| format!("read '{label}': {e}"))?;
        if n == 0 {
            break;
        }
        total = total.saturating_add(n as u64);
        if total > MAX_SINGLE_FILE_BYTES {
            return Err(format!(
                "file '{label}' exceeds {MAX_SINGLE_FILE_BYTES}-byte safety limit"
            ));
        }
        hasher.update(&buf[..n]);
    }
    Ok((total, hex_encode(&hasher.finalize())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn make_workspace(root: &Path) {
        fs::create_dir_all(root.join("notes")).unwrap();
        fs::write(
            root.join("notes/alpha.md"),
            "---\ndescription: Alpha note\n---\n# Alpha\n\nLinks to [[beta]].\n",
        )
        .unwrap();
        fs::write(
            root.join("notes/beta.md"),
            "---\ndescription: Beta note\n---\n# Beta\n\nContent.\n",
        )
        .unwrap();
    }

    #[test]
    fn backup_and_restore_roundtrip_verifies_sha256_and_loads_workspace() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().join("ws");
        let bak = tmp.path().join("bak");
        let restored = tmp.path().join("restored");
        make_workspace(&ws);

        let b = backup_workspace(&ws, &bak).expect("backup should succeed");
        assert_eq!(b.file_count, 2);
        assert!(bak.join("manifest.sha256.json").exists());

        let r = restore_workspace(&bak, &restored).expect("restore should succeed");
        assert_eq!(r.restored_files, 2);
        assert_eq!(r.manifest_digest, b.manifest_digest);
        assert_eq!(r.verified_workspace_files, 2);
        assert_eq!(
            fs::read_to_string(restored.join("notes/alpha.md")).unwrap(),
            fs::read_to_string(ws.join("notes/alpha.md")).unwrap()
        );
    }

    #[test]
    fn restore_fails_closed_on_tampered_file_and_extra_untracked_file() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().join("ws");
        let bak = tmp.path().join("bak");
        make_workspace(&ws);
        backup_workspace(&ws, &bak).unwrap();

        // 1. Tamper with an existing backed-up file.
        let orig = fs::read(bak.join("data/notes/alpha.md")).unwrap();
        fs::write(bak.join("data/notes/alpha.md"), b"tampered bytes").unwrap();
        let target1 = tmp.path().join("restored1");
        let err = restore_workspace(&bak, &target1).unwrap_err();
        assert!(err.contains("mismatch"), "unexpected error: {err}");
        assert!(
            !target1.exists(),
            "target must not be created on failed precheck"
        );

        // Restore original bytes, then inject an extra untracked file into data/.
        fs::write(bak.join("data/notes/alpha.md"), orig).unwrap();
        fs::write(bak.join("data/notes/evil.md"), b"injected").unwrap();
        let target2 = tmp.path().join("restored2");
        let err2 = restore_workspace(&bak, &target2).unwrap_err();
        assert!(err2.contains("untracked"), "unexpected error: {err2}");
    }

    #[test]
    fn backup_refuses_symlinks_and_non_empty_output_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().join("ws");
        make_workspace(&ws);

        // Non-empty output directory refused.
        let non_empty = tmp.path().join("non-empty");
        fs::create_dir_all(&non_empty).unwrap();
        fs::write(non_empty.join("existing.txt"), b"keep").unwrap();
        let err = backup_workspace(&ws, &non_empty).unwrap_err();
        assert!(err.contains("non-empty"), "unexpected error: {err}");

        // Escaping symlink inside workspace refused.
        let outside = tmp.path().join("secret.txt");
        fs::write(&outside, b"secret").unwrap();
        symlink(&outside, ws.join("notes/escape.md")).unwrap();
        let bak = tmp.path().join("bak-sym");
        let err_sym = backup_workspace(&ws, &bak).unwrap_err();
        assert!(err_sym.contains("symlink"), "unexpected error: {err_sym}");
    }
}
