//! Driven adapter: crash-safe durable outcome registry (`ADR-0009`, G4).
//!
//! Implements `outcome_journal`, `outcome_digest`, and `outcome_registry` with
//! framed IEEE CRC-32 journals, write-ahead `usage.dirty`, atomic CRC-32
//! `usage.json`, `fs4` inter-process `registry.lock`, ground-truth
//! `prune_tombstones_to`, and exact-frame replay.

mod outcome_digest;
mod outcome_journal;
mod outcome_registry;

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;
use tb_app::{AppError, OutcomeBegin, OutcomeObservation, OutcomePort, OutcomeSession};

pub use outcome_journal::{OutcomeJournal, Record, RecordKind, RequestId};
pub use outcome_registry::{
    JournalState, OutcomeRecovery, Quota, Registry, RegistryError, RegistryLock, Reservation,
};

/// Default engine state root inside a workspace: `<workspace>/.2brain/engine`.
#[must_use]
pub fn outcome_registry_root(workspace_root: &Path) -> PathBuf {
    workspace_root.join(".2brain").join("engine")
}

/// Enable the opt-in outcome registry fence at `<state_root>/outcomes/registry.version`.
pub fn enable(state_root: &Path) -> io::Result<()> {
    outcome_registry::enable(state_root)
}

/// Write an explicit quota configuration at `<state_root>/outcomes/quota.json`.
pub fn configure_quota(state_root: &Path, max_records: usize, max_bytes: u64) -> io::Result<()> {
    outcome_registry::configure_quota(state_root, max_records, max_bytes)
}

/// Path to `<state_root>/outcomes/registry.version`.
#[must_use]
pub fn fence_path(state_root: &Path) -> PathBuf {
    outcome_registry::fence_path(state_root)
}

/// Open a workspace-scoped outcome registry handle at `<state_root>` (containing `outcomes/`).
#[must_use]
pub fn open_registry(state_root: &Path) -> Registry {
    Registry::at(state_root)
}

/// Startup recovery wrapper matching the G4 workspace contract.
pub fn recover_outcome_registry(workspace_root: &Path) -> Result<OutcomeRecovery, String> {
    let root = outcome_registry_root(workspace_root);
    let reg = Registry::at(&root);
    if !reg.is_enabled() {
        return Ok(OutcomeRecovery::default());
    }
    reg.recover_pre_effect().map_err(|e| e.to_string())
}

/// Background auto-compaction (7-day retention window) matching the G4 contract.
pub fn auto_compact_outcome_registry(workspace_root: &Path) -> Result<usize, String> {
    let root = outcome_registry_root(workspace_root);
    let reg = Registry::at(&root);
    if !reg.is_enabled() {
        return Ok(0);
    }
    reg.compact(Duration::from_secs(7 * 24 * 60 * 60))
        .map_err(|e| e.to_string())
}

/// Concrete [`OutcomePort`] backed by `<workspace>/.2brain/engine/outcomes`.
#[derive(Clone, Debug)]
pub struct FileOutcomeStore {
    state_root: PathBuf,
}

impl FileOutcomeStore {
    #[must_use]
    pub fn for_workspace(workspace_root: &Path) -> Self {
        Self {
            state_root: outcome_registry_root(workspace_root),
        }
    }

    #[must_use]
    pub fn at_state_root(state_root: PathBuf) -> Self {
        Self { state_root }
    }

    #[must_use]
    pub fn is_enabled(&self) -> bool {
        Registry::at(&self.state_root).is_enabled()
    }
}

struct FileOutcomeSession {
    registry: Registry,
    _lock: RegistryLock,
    usage_base: Option<(usize, u64)>,
    journal: OutcomeJournal,
}

impl OutcomeSession for FileOutcomeSession {
    fn record_executing(&mut self) -> Result<(), AppError> {
        self.journal
            .append(RecordKind::Prepared, b"")
            .map_err(|e| AppError::Io(e.to_string()))?;
        self.journal
            .append(RecordKind::Executing, b"")
            .map_err(|e| AppError::Io(e.to_string()))
    }

    fn finish_committed(mut self: Box<Self>, response_payload: &Value) -> Result<(), AppError> {
        let bytes = serde_json::to_vec(response_payload)
            .map_err(|e| AppError::OutcomeUnknown(format!("serialize outcome response: {e}")))?;
        self.journal
            .append(RecordKind::Committed, b"")
            .map_err(|e| AppError::OutcomeUnknown(format!("append committed record: {e}")))?;
        let resp_res = self
            .journal
            .append_response(&bytes)
            .map_err(|e| AppError::OutcomeUnknown(format!("persist replay response frame: {e}")));
        self.registry
            .commit_journal_append_locked(&self._lock, self.usage_base, &self.journal);
        resp_res
    }

    fn finish_rejected(mut self: Box<Self>, reason: &str) -> Result<(), AppError> {
        let _ = self.journal.append(RecordKind::Rejected, reason.as_bytes());
        self.registry
            .commit_journal_append_locked(&self._lock, self.usage_base, &self.journal);
        Ok(())
    }
}

fn map_registry_err(err: RegistryError) -> AppError {
    match err {
        RegistryError::NotEnabled => AppError::Unsupported("outcome registry not enabled".into()),
        RegistryError::Conflict { request_id } => AppError::OutcomeConflict(format!(
            "request_id '{request_id}' already bound to a different request"
        )),
        RegistryError::Quota {
            records,
            bytes,
            max_records,
            max_bytes,
        } => AppError::OutcomeQuota(format!(
            "outcome registry quota reached (records={records}/{max_records}, bytes={bytes}/{max_bytes})"
        )),
        RegistryError::Corrupt(msg) => AppError::InvalidArguments(msg),
        RegistryError::Io(err) => AppError::Io(err.to_string()),
    }
}

impl OutcomePort for FileOutcomeStore {
    fn observe(&self, request_id: &str) -> Result<OutcomeObservation, AppError> {
        let reg = Registry::at(&self.state_root);
        if !reg.is_enabled() {
            return Ok(OutcomeObservation {
                enabled: false,
                present: false,
                request_id: request_id.to_owned(),
                state: "unknown".to_owned(),
                replay: "absent".to_owned(),
            });
        }
        let id = RequestId::parse(request_id).map_err(AppError::InvalidArguments)?;
        let Some(state) = reg.observe(&id).map_err(map_registry_err)? else {
            return Ok(OutcomeObservation {
                enabled: true,
                present: false,
                request_id: request_id.to_owned(),
                state: "unknown".to_owned(),
                replay: "absent".to_owned(),
            });
        };
        let replay = if reg.saved_response(&id).map_err(map_registry_err)?.is_some() {
            "available"
        } else {
            "absent"
        };
        Ok(OutcomeObservation {
            enabled: true,
            present: true,
            request_id: request_id.to_owned(),
            state: state.as_str().to_owned(),
            replay: replay.to_owned(),
        })
    }

    fn begin_mutation(
        &self,
        request_id: &str,
        verb: &str,
        workspace: &str,
        arguments: &Value,
        preconditions: &Value,
    ) -> Result<OutcomeBegin, AppError> {
        let reg = Registry::at(&self.state_root);
        if !reg.is_enabled() {
            return Ok(OutcomeBegin::Disabled);
        }
        let id = RequestId::parse(request_id).map_err(AppError::InvalidArguments)?;
        let lock = reg.acquire_lock().map_err(map_registry_err)?;
        match reg
            .reserve_locked(&lock, &id, verb, workspace, arguments, preconditions)
            .map_err(map_registry_err)?
        {
            Reservation::Created => {
                let usage_base = reg.snapshot_clean_usage_locked(&lock);
                let journal = OutcomeJournal::open(&self.state_root, &id)
                    .map_err(|e| AppError::Io(e.to_string()))?;
                Ok(OutcomeBegin::Reserved(Box::new(FileOutcomeSession {
                    registry: reg,
                    _lock: lock,
                    usage_base,
                    journal,
                })))
            }
            Reservation::Existing { state } => {
                if state == JournalState::Committed
                    && let Some(saved) = reg.saved_response(&id).map_err(map_registry_err)?
                {
                    return Ok(OutcomeBegin::Replay {
                        payload: saved,
                        outcome_state: state.as_str(),
                    });
                }
                Err(AppError::OutcomeConflict(format!(
                    "request_id '{request_id}' already exists in state '{}' without replayable frame",
                    state.as_str()
                )))
            }
        }
    }
}
