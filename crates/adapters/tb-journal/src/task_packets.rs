//! Crash-safe persistent [`TaskPacket`] store (`D08`, `U10`/`R17`/`AE17`, MIT).
//!
//! Stored under `<workspace>/.2brain/engine/tasks/<task_id>.json`.
//! Security & concurrency invariants:
//! - Exclusive inter-process `fs4` lock on `tasks.lock` with post-lock `(st_dev, st_ino)`
//!   identity verification.
//! - Directory & file opens strictly use `O_NOFOLLOW` (`rustix::fs::openat`).
//! - Writes go through a temp file (`sync_all`) + atomic `rename` + parent dir sync.
//! - Optimistic revision CAS rejects stale writers with `expected_hash_mismatch` /
//!   revision conflict, preserves critical obligations (e.g. `publish=false`), and
//!   enforces monotonic spent budget counters.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use fs4::fs_std::FileExt;
use rustix::fs::{AtFlags, Mode, OFlags, fstat, fsync, openat, renameat, statat, unlinkat};
use tb_app::{AppError, TaskScopeGuard, TaskStorePort};
use tb_contracts::{TASK_PACKET_SCHEMA_VERSION, TaskPacket};

const MAX_TASK_FILE_BYTES: u64 = 2 * 1024 * 1024; // 2 MiB safety cap per TaskPacket
static TMP_NONCE: AtomicU64 = AtomicU64::new(1);

/// File-backed crash-safe `TaskPacket` store rooted at `<workspace>/.2brain/engine/tasks`.
#[derive(Clone, Debug)]
pub struct FileTaskStore {
    tasks_dir: PathBuf,
}

impl FileTaskStore {
    #[must_use]
    pub fn for_workspace(workspace_root: &Path) -> Self {
        Self {
            tasks_dir: workspace_root.join(".2brain").join("engine").join("tasks"),
        }
    }

    #[must_use]
    pub fn at_dir(tasks_dir: PathBuf) -> Self {
        Self { tasks_dir }
    }

    #[must_use]
    pub fn tasks_dir(&self) -> &Path {
        &self.tasks_dir
    }

    fn acquire_lock(&self) -> Result<(OwnedFd, File), AppError> {
        fs::create_dir_all(&self.tasks_dir)
            .map_err(|e| AppError::Io(format!("create tasks dir: {e}")))?;
        let dir_fd = openat(
            rustix::fs::CWD,
            &self.tasks_dir,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW,
            Mode::empty(),
        )
        .map_err(|e| AppError::Io(format!("open tasks dir (O_NOFOLLOW): {e}")))?;

        let lock_fd = openat(
            dir_fd.as_fd(),
            "tasks.lock",
            OFlags::RDWR | OFlags::CREATE | OFlags::NOFOLLOW,
            Mode::from_raw_mode(0o600),
        )
        .map_err(|e| AppError::Io(format!("open tasks.lock: {e}")))?;
        let lock_file = File::from(lock_fd);
        lock_file
            .lock_exclusive()
            .map_err(|e| AppError::Io(format!("lock tasks.lock: {e}")))?;

        // Verify post-lock path identity matches locked fd (Rule #18).
        let fd_st =
            fstat(&lock_file).map_err(|e| AppError::Io(format!("fstat tasks.lock: {e}")))?;
        let path_st = statat(dir_fd.as_fd(), "tasks.lock", AtFlags::SYMLINK_NOFOLLOW)
            .map_err(|e| AppError::Io(format!("statat tasks.lock: {e}")))?;
        if fd_st.st_dev != path_st.st_dev || fd_st.st_ino != path_st.st_ino {
            return Err(AppError::Io(
                "tasks.lock inode changed concurrently while acquiring lock".into(),
            ));
        }
        Ok((dir_fd, lock_file))
    }

    fn read_packet_fd(dir_fd: &OwnedFd, file_name: &str) -> Result<TaskPacket, AppError> {
        let fd = openat(
            dir_fd.as_fd(),
            file_name,
            OFlags::RDONLY | OFlags::NOFOLLOW,
            Mode::empty(),
        )
        .map_err(|e| {
            if e == rustix::io::Errno::NOENT {
                AppError::NotFound(format!("task packet '{file_name}'"))
            } else {
                AppError::Io(format!("open task '{file_name}': {e}"))
            }
        })?;
        let file = File::from(fd);
        let mut buf = Vec::new();
        file.take(MAX_TASK_FILE_BYTES + 1)
            .read_to_end(&mut buf)
            .map_err(|e| AppError::Io(format!("read task '{file_name}': {e}")))?;
        if (buf.len() as u64) > MAX_TASK_FILE_BYTES {
            return Err(AppError::Io(format!(
                "task '{file_name}' exceeds {MAX_TASK_FILE_BYTES}-byte limit"
            )));
        }
        let packet: TaskPacket = serde_json::from_slice(&buf)
            .map_err(|e| AppError::Io(format!("corrupt task JSON '{file_name}': {e}")))?;
        packet
            .validate()
            .map_err(|e| AppError::InvalidArguments(e.to_string()))?;
        Ok(packet)
    }

    fn write_packet_atomic_fd(dir_fd: BorrowedFd<'_>, packet: &TaskPacket) -> Result<(), AppError> {
        let final_name = format!("{}.json", packet.task_id);
        let nonce = TMP_NONCE.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let tmp_name = format!(
            ".{}.json.tmp.{}.{}.{}",
            packet.task_id,
            std::process::id(),
            nanos,
            nonce
        );
        let bytes = serde_json::to_vec_pretty(packet)
            .map_err(|e| AppError::Io(format!("serialize task packet: {e}")))?;

        // Strictly descriptor-relative O_CREAT|O_EXCL|O_NOFOLLOW — immune to symlink pre-creation
        // and parent-directory swaps (Rule #19).
        let fd = openat(
            dir_fd,
            tmp_name.as_str(),
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW,
            Mode::from_raw_mode(0o600),
        )
        .map_err(|e| AppError::Io(format!("create temp task file '{tmp_name}': {e}")))?;
        let mut f = File::from(fd);
        let write_res = (|| -> Result<(), AppError> {
            f.write_all(&bytes)
                .map_err(|e| AppError::Io(format!("write temp task file: {e}")))?;
            f.write_all(b"\n")
                .map_err(|e| AppError::Io(format!("write temp task newline: {e}")))?;
            f.sync_all()
                .map_err(|e| AppError::Io(format!("sync temp task file: {e}")))?;
            Ok(())
        })();
        if let Err(err) = write_res {
            drop(f);
            let _ = unlinkat(dir_fd, tmp_name.as_str(), AtFlags::empty());
            return Err(err);
        }
        drop(f);

        if let Err(e) = renameat(dir_fd, tmp_name.as_str(), dir_fd, final_name.as_str()) {
            let _ = unlinkat(dir_fd, tmp_name.as_str(), AtFlags::empty());
            return Err(AppError::Io(format!("publish task file: {e}")));
        }
        let _ = fsync(dir_fd);
        Ok(())
    }
}

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl TaskStorePort for FileTaskStore {
    fn create_task(&self, mut packet: TaskPacket) -> Result<TaskPacket, AppError> {
        if packet.schema_version.is_empty() {
            packet.schema_version = TASK_PACKET_SCHEMA_VERSION.to_owned();
        }
        packet.revision = 1;
        packet.updated_unix = now_unix_secs();
        packet
            .validate()
            .map_err(|e| AppError::InvalidArguments(e.to_string()))?;

        let (dir_fd, _lock) = self.acquire_lock()?;
        let file_name = format!("{}.json", packet.task_id);
        match statat(
            dir_fd.as_fd(),
            file_name.as_str(),
            AtFlags::SYMLINK_NOFOLLOW,
        ) {
            Ok(_) => {
                return Err(AppError::AlreadyExists(format!(
                    "task '{}'",
                    packet.task_id
                )));
            }
            Err(e) if e == rustix::io::Errno::NOENT => {}
            Err(e) => return Err(AppError::Io(format!("stat task '{file_name}': {e}"))),
        }
        Self::write_packet_atomic_fd(dir_fd.as_fd(), &packet)?;
        Ok(packet)
    }

    fn load_task(&self, task_id: &str) -> Result<TaskPacket, AppError> {
        TaskPacket::validate_id(task_id).map_err(|e| AppError::InvalidArguments(e.to_string()))?;
        let dir_fd = openat(
            rustix::fs::CWD,
            &self.tasks_dir,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW,
            Mode::empty(),
        )
        .map_err(|e| {
            if e == rustix::io::Errno::NOENT {
                AppError::NotFound(format!("task '{task_id}'"))
            } else {
                AppError::Io(format!("open tasks dir: {e}"))
            }
        })?;
        let file_name = format!("{task_id}.json");
        Self::read_packet_fd(&dir_fd, &file_name)
    }

    fn list_tasks(&self) -> Result<Vec<TaskPacket>, AppError> {
        let dir_fd = match openat(
            rustix::fs::CWD,
            &self.tasks_dir,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW,
            Mode::empty(),
        ) {
            Ok(fd) => fd,
            Err(e) if e == rustix::io::Errno::NOENT => return Ok(Vec::new()),
            Err(e) => return Err(AppError::Io(format!("open tasks dir: {e}"))),
        };
        let mut names = Vec::new();
        for entry in fs::read_dir(&self.tasks_dir)
            .map_err(|e| AppError::Io(format!("read tasks dir: {e}")))?
        {
            let entry = entry.map_err(|e| AppError::Io(format!("read tasks entry: {e}")))?;
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if name.ends_with(".json") && !name.starts_with('.') {
                names.push(name);
            }
        }
        names.sort();
        let mut out = Vec::with_capacity(names.len());
        for name in names {
            out.push(Self::read_packet_fd(&dir_fd, &name)?);
        }
        Ok(out)
    }

    fn checkpoint_task(
        &self,
        task_id: &str,
        expected_revision: u64,
        mut next: TaskPacket,
        allow_drop_critical: bool,
        scope_guard: Option<&TaskScopeGuard>,
    ) -> Result<TaskPacket, AppError> {
        TaskPacket::validate_id(task_id).map_err(|e| AppError::InvalidArguments(e.to_string()))?;
        if next.schema_version.is_empty() {
            next.schema_version = TASK_PACKET_SCHEMA_VERSION.to_owned();
        }
        next.task_id = task_id.to_owned();
        next.validate()
            .map_err(|e| AppError::InvalidArguments(e.to_string()))?;

        let (dir_fd, _lock) = self.acquire_lock()?;
        let file_name = format!("{task_id}.json");
        let current = Self::read_packet_fd(&dir_fd, &file_name)?;

        // Authorize BOTH persisted and proposed scopes atomically under the same exclusive lock.
        if let Some(guard) = scope_guard {
            guard.check_scope(&current.scope_repos, "persisted")?;
            guard.check_scope(&next.scope_repos, "proposed")?;
        }

        if current.revision != expected_revision {
            return Err(AppError::CasMismatch(format!(
                "task '{task_id}' revision conflict: expected={expected_revision}, actual={}",
                current.revision
            )));
        }
        next.validate_transition_from(&current, allow_drop_critical)
            .map_err(AppError::InvalidArguments)?;

        next.revision = current
            .revision
            .checked_add(1)
            .ok_or_else(|| AppError::Io("task revision overflow".into()))?;
        next.updated_unix = now_unix_secs();

        Self::write_packet_atomic_fd(dir_fd.as_fd(), &next)?;
        Ok(next)
    }
}

/// Helper for direct path-based inspection of a workspace's task packet directory.
pub fn tasks_dir_for_workspace(workspace_root: &Path) -> PathBuf {
    FileTaskStore::for_workspace(workspace_root).tasks_dir
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use tb_contracts::{TaskBudget, TaskObligation, TaskStatus};

    use super::*;

    fn seed_packet() -> TaskPacket {
        TaskPacket {
            schema_version: TASK_PACKET_SCHEMA_VERSION.to_owned(),
            task_id: "handoff-demo".to_owned(),
            revision: 0,
            title: "Two-client handoff task".to_owned(),
            status: TaskStatus::Open,
            criterion_revision: "2brain.criterion/v1".to_owned(),
            scope_repos: vec!["vault".to_owned()],
            obligations: vec![TaskObligation {
                key: "publish".to_owned(),
                value: "false".to_owned(),
                critical: true,
            }],
            budget: TaskBudget {
                max_actions: 8,
                spent_actions: 1,
                max_tokens: 4000,
                spent_tokens: 450,
            },
            completed_steps: vec!["step-1".to_owned()],
            unchecked_steps: vec!["step-2".to_owned()],
            rejected_routes: vec!["fp-stale".to_owned()],
            pending_operations: vec![],
            evidence_digests: BTreeMap::new(),
            updated_by: "client-1".to_owned(),
            updated_unix: 0,
        }
    }

    #[test]
    fn task_store_handoff_preserves_obligations_and_rejects_stale_writer() {
        let tmp = tempfile::tempdir().unwrap();
        let store = FileTaskStore::for_workspace(tmp.path());

        let created = store.create_task(seed_packet()).unwrap();
        assert_eq!(created.revision, 1);

        // Client 2 resumes and advances revision 1 -> 2.
        let mut client2 = store.load_task("handoff-demo").unwrap();
        client2.updated_by = "client-2".to_owned();
        client2.budget.spent_actions = 2;
        client2.budget.spent_tokens = 900;
        client2.completed_steps.push("step-2".to_owned());
        client2.unchecked_steps.clear();
        let rev2 = store
            .checkpoint_task("handoff-demo", 1, client2.clone(), false, None)
            .unwrap();
        assert_eq!(rev2.revision, 2);
        assert_eq!(rev2.obligations[0].key, "publish");
        assert_eq!(rev2.obligations[0].value, "false");

        // Stale Client 1 attempts to write using old revision=1 -> rejected (AE17!).
        let mut stale_client1 = created;
        stale_client1.title = "Stale overwrite".to_owned();
        let err = store
            .checkpoint_task("handoff-demo", 1, stale_client1, false, None)
            .unwrap_err();
        assert!(err.to_string().contains("revision conflict"));

        // Client 2 trying to silently drop critical `publish=false` -> rejected!
        let mut drop_ob = rev2.clone();
        drop_ob.obligations.clear();
        let err_drop = store
            .checkpoint_task("handoff-demo", 2, drop_ob, false, None)
            .unwrap_err();
        assert!(err_drop.to_string().contains("publish=false"));

        // Restricted client trying to hijack a revoked-repo task by rewriting scope_repos -> rejected!
        let guard = TaskScopeGuard {
            allowed_repos: vec!["public".to_owned()],
            revoked_repos: vec!["vault".to_owned()],
        };
        let mut hijack = rev2.clone();
        hijack.scope_repos = vec!["public".to_owned()];
        let err_hijack = store
            .checkpoint_task("handoff-demo", 2, hijack, false, Some(&guard))
            .unwrap_err();
        assert!(err_hijack.to_string().contains("policy/revoked"));

        // List returns persisted packet at rev 2.
        let listed = store.list_tasks().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].revision, 2);
    }
}
