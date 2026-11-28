//! Hash-chained, zero-TOCTOU JSONL audit log for MCP mutations and policy refusals (Issue #7 R24 / spec §5.2).
//!
//! Every entry is cryptographically chained to the previous entry via SHA-256 over a
//! length-prefixed canonical encoding, serialized under an exclusive `fs4` interprocess lock,
//! opened strictly with `O_NOFOLLOW`, and synced to disk on append.

use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use fs4::fs_std::FileExt;
use rustix::fs::{AtFlags, Mode, OFlags, fstat, openat, statat};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const GENESIS_HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// One hash-chained audit record persisted in `audit.jsonl`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AuditEvent {
    pub seq: u64,
    pub ts_unix: u64,
    pub principal: String,
    pub tool: String,
    pub repo: String,
    pub path: String,
    pub outcome: String,
    pub reason_code: String,
    pub prev: String,
    pub hash: String,
}

/// Input payload to record an audit event.
#[derive(Clone, Debug)]
pub struct AuditEntryInput<'a> {
    pub principal: &'a str,
    pub tool: &'a str,
    pub repo: &'a str,
    pub path: &'a str,
    pub outcome: &'a str,
    pub reason_code: &'a str,
}

/// File-backed append-only hash-chained audit log.
#[derive(Clone, Debug)]
pub struct FileAuditLog {
    path: PathBuf,
}

impl FileAuditLog {
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Validate that the audit log is writable, non-symlinked, lockable, and its existing hash
    /// chain is intact BEFORE executing a mutation.
    pub fn check_ready(&self) -> io::Result<(u64, String)> {
        let (_lock, mut log_file) = self.open_locked()?;
        let mut existing = String::new();
        log_file.read_to_string(&mut existing)?;
        verify_chain_str(&existing)
    }

    /// Return the verified chain head `(seq, hash)` for external checkpointing/anchoring.
    pub fn head(&self) -> io::Result<(u64, String)> {
        self.check_ready()
    }

    fn open_locked(&self) -> io::Result<(File, File)> {
        let parent = self
            .path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)?;
        let file_name = self
            .path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid audit log path"))?;

        let parent_fd = openat(
            rustix::fs::CWD,
            parent,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW,
            Mode::empty(),
        )
        .map_err(|e| io::Error::other(format!("open audit parent dir: {e}")))?;

        let lock_name = format!("{file_name}.lock");
        let lock_fd = openat(
            &parent_fd,
            lock_name.as_str(),
            OFlags::RDWR | OFlags::CREATE | OFlags::NOFOLLOW,
            Mode::from_raw_mode(0o600),
        )
        .map_err(|e| io::Error::other(format!("open audit lock file: {e}")))?;
        let lock_file = File::from(lock_fd);
        let lock_meta = lock_file.metadata()?;
        if !lock_meta.file_type().is_file() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "audit lock path is not a regular file",
            ));
        }
        lock_file.lock_exclusive()?;
        // Verify lock inode identity under the lock so unlinked/replaced lock files fail closed.
        let lock_stat = fstat(&lock_file)
            .map_err(|e| io::Error::other(format!("fstat audit lock after locking: {e}")))?;
        let path_stat = statat(&parent_fd, lock_name.as_str(), AtFlags::SYMLINK_NOFOLLOW)
            .map_err(|e| io::Error::other(format!("stat audit lock after locking: {e}")))?;
        if path_stat.st_dev != lock_stat.st_dev || path_stat.st_ino != lock_stat.st_ino {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "audit lock file was replaced while acquiring lock",
            ));
        }

        let log_fd = openat(
            &parent_fd,
            file_name,
            OFlags::RDWR | OFlags::CREATE | OFlags::NOFOLLOW,
            Mode::from_raw_mode(0o600),
        )
        .map_err(|e| io::Error::other(format!("open audit log file: {e}")))?;
        let log_file = File::from(log_fd);
        if !log_file.metadata()?.file_type().is_file() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "audit log path is not a regular file",
            ));
        }
        Ok((lock_file, log_file))
    }

    /// Append a new hash-chained event under an exclusive interprocess lock (`<path>.lock`).
    /// Fails closed if either the lock file or the audit log itself is a symlink or if existing
    /// log lines are corrupted.
    pub fn append(&self, input: &AuditEntryInput<'_>) -> io::Result<AuditEvent> {
        let (_lock, mut log_file) = self.open_locked()?;

        let mut existing = String::new();
        log_file.read_to_string(&mut existing)?;
        let (prev_seq, prev_hash) = verify_chain_str(&existing)?;

        let seq = prev_seq.saturating_add(1);
        let ts_unix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let hash = compute_event_hash(
            seq,
            ts_unix,
            input.principal,
            input.tool,
            input.repo,
            input.path,
            input.outcome,
            input.reason_code,
            &prev_hash,
        );
        let event = AuditEvent {
            seq,
            ts_unix,
            principal: input.principal.to_owned(),
            tool: input.tool.to_owned(),
            repo: input.repo.to_owned(),
            path: input.path.to_owned(),
            outcome: input.outcome.to_owned(),
            reason_code: input.reason_code.to_owned(),
            prev: prev_hash,
            hash,
        };
        let mut line = serde_json::to_string(&event)
            .map_err(|e| io::Error::other(format!("serialize audit event: {e}")))?;
        line.push('\n');

        log_file.seek(SeekFrom::End(0))?;
        log_file.write_all(line.as_bytes())?;
        log_file.sync_all()?;
        Ok(event)
    }
}

/// Verify the entire cryptographic hash chain of an `audit.jsonl` file.
/// Returns the number of verified events (`0` for an empty file).
pub fn verify_audit_chain(path: &Path) -> io::Result<u64> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid audit log path"))?;
    let parent_fd = openat(
        rustix::fs::CWD,
        parent,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .map_err(|e| io::Error::other(format!("open audit parent dir: {e}")))?;
    let log_fd = openat(
        &parent_fd,
        file_name,
        OFlags::RDONLY | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .map_err(|e| io::Error::other(format!("open audit log: {e}")))?;
    let mut file = File::from(log_fd);
    if !file.metadata()?.file_type().is_file() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "audit log path is not a regular file",
        ));
    }
    let mut raw = String::new();
    file.read_to_string(&mut raw)?;
    let (count, _) = verify_chain_str(&raw)?;
    Ok(count)
}

fn verify_chain_str(raw: &str) -> io::Result<(u64, String)> {
    if !raw.is_empty() && !raw.ends_with('\n') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "corrupt audit log: missing terminating newline",
        ));
    }
    let mut expected_seq = 0u64;
    let mut expected_prev = GENESIS_HASH.to_owned();
    for (idx, line) in raw.lines().enumerate() {
        if line.trim().is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("corrupt audit log: blank line at line {}", idx + 1),
            ));
        }
        let ev: AuditEvent = serde_json::from_str(line).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("corrupt audit log JSON at line {}: {e}", idx + 1),
            )
        })?;
        expected_seq = expected_seq.saturating_add(1);
        if ev.seq != expected_seq {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "audit chain seq mismatch at line {}: expected {expected_seq}, got {}",
                    idx + 1,
                    ev.seq
                ),
            ));
        }
        if ev.prev != expected_prev {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("audit chain prev hash mismatch at line {}", idx + 1),
            ));
        }
        let recomputed = compute_event_hash(
            ev.seq,
            ev.ts_unix,
            &ev.principal,
            &ev.tool,
            &ev.repo,
            &ev.path,
            &ev.outcome,
            &ev.reason_code,
            &ev.prev,
        );
        if ev.hash != recomputed {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("audit chain hash mismatch (tampered) at line {}", idx + 1),
            ));
        }
        expected_prev = ev.hash;
    }
    Ok((expected_seq, expected_prev))
}

#[allow(clippy::too_many_arguments)]
fn compute_event_hash(
    seq: u64,
    ts_unix: u64,
    principal: &str,
    tool: &str,
    repo: &str,
    path: &str,
    outcome: &str,
    reason_code: &str,
    prev: &str,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"2brain.audit/v1\0");
    hasher.update(seq.to_le_bytes());
    hasher.update(ts_unix.to_le_bytes());
    for field in [principal, tool, repo, path, outcome, reason_code, prev] {
        hasher.update((field.len() as u64).to_le_bytes());
        hasher.update(field.as_bytes());
    }
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(64);
    for b in digest {
        let _ = write!(hex, "{b:02x}");
    }
    hex
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audit_log_chains_events_and_detects_tampering() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.jsonl");
        let log = FileAuditLog::new(path.clone());

        let e1 = log
            .append(&AuditEntryInput {
                principal: "hermes",
                tool: "tb_edit_file",
                repo: "vault",
                path: "notes/a.md",
                outcome: "denied",
                reason_code: "prior_read",
            })
            .unwrap();
        let e2 = log
            .append(&AuditEntryInput {
                principal: "hermes",
                tool: "tb_edit_file",
                repo: "vault",
                path: "notes/a.md",
                outcome: "allowed",
                reason_code: "ok",
            })
            .unwrap();
        assert_eq!(e1.seq, 1);
        assert_eq!(e2.seq, 2);
        assert_eq!(e2.prev, e1.hash);
        assert_eq!(verify_audit_chain(&path).unwrap(), 2);

        // Tamper with line 1 outcome -> verification must fail closed.
        let tampered = fs::read_to_string(&path)
            .unwrap()
            .replace("\"denied\"", "\"allowed\"");
        fs::write(&path, tampered).unwrap();
        assert!(verify_audit_chain(&path).is_err());
    }
}
