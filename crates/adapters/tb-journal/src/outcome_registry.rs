//! The durable outcome registry's first slice: the fence, a reservation, and
//! the state a journal holds.
//!
//! The durable outcome registry design fixes the shape: a caller-supplied
//! `request_id` is bound once to the digest of the request it names, in a
//! framed append-only journal owned by that id. A fence file beside the
//! journal root refuses a workspace whose format this build cannot vouch for.
//!
//! The serving path reserves an id under the write lock, records terminal states,
//! and reads the saved terminal frame back for a retry. The operator enables the
//! fence with `tb-engine outcomes enable <entry>`.
//! The frame, its CRC-32 and the request digest live in `outcome_journal` and
//! `outcome_digest`; nothing here re-encodes a frame or hashes anything itself.
//!

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use serde_json::Value;

use crate::outcome_digest::{RequestDigest, request_digest};
use crate::outcome_journal::{OutcomeJournal, Record, RecordKind, RequestId};

/// The fence file, beside the journal directory: `<root>/outcomes/registry.version`.
const FENCE_FILE: &str = "registry.version";

/// The only fence content this build writes and accepts, newline included.
const FENCE_LINE: &str = "2brain.outcome-registry/v1\n";

/// The schema of the one `Reserved` payload, fixed inside the payload.
const RESERVED_SCHEMA: &str = "2brain.outcome-reserved/v1";
/// The tombstone sidecar that can outlive a terminal journal (D5).
const TOMBSTONE_SCHEMA: &str = "2brain.outcome-tombstone/v1";
/// Optional operator quota beside the fence and journals.
const QUOTA_FILE: &str = "quota.json";
const QUOTA_SCHEMA: &str = "2brain.outcome-quota/v1";
/// Crash-safe derived quota usage index, write-ahead dirty marker, and
/// inter-process registry lock (ADR-0009 §3 & §5, G4). Journals and tombstones
/// remain the sole source of truth; any missing, dirty, or corrupt index
/// triggers an authoritative rebuild from disk.
const USAGE_INDEX_FILE: &str = "usage.json";
const USAGE_INDEX_TMP_PREFIX: &str = "usage.";
const USAGE_INDEX_TMP_SUFFIX: &str = ".tmp";
const USAGE_DIRTY_FILE: &str = "usage.dirty";
const REGISTRY_LOCK_FILE: &str = "registry.lock";
const USAGE_INDEX_SCHEMA: &str = "2brain.outcome-usage/v1";

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Exclusive workspace-scoped file lock serializing registry mutations,
/// background compaction, recovery, pruning, and derived usage index updates
/// across threads and processes.
pub struct RegistryLock {
    _file: File,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Tombstone {
    digest: String,
    terminal: JournalState,
}

/// D5 admission defaults: live journal records and their total bytes.
const DEFAULT_MAX_RECORDS: usize = 100_000;
const DEFAULT_MAX_BYTES: u64 = 256 * 1024 * 1024;

/// The admission quota for new request-id journals (D5). Existing ids are
/// always allowed to observe/replay: quota pressure never evicts recovery
/// evidence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Quota {
    pub max_records: usize,
    pub max_bytes: u64,
}

impl Default for Quota {
    fn default() -> Self {
        Self {
            max_records: DEFAULT_MAX_RECORDS,
            max_bytes: DEFAULT_MAX_BYTES,
        }
    }
}

/// The state a request id's journal describes (D4's table).
///
/// [`JournalState::Unknown`] is the state of a journal that exists and holds no
/// record: a file created but never extended. An absent journal is `None` from
/// [`Registry::observe`], not `Unknown`, because "no reservation found" and "a
/// reservation with nothing recorded" are different answers.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum JournalState {
    Unknown,
    Reserved,
    Prepared,
    Executing,
    Compensating,
    Committed,
    Rejected,
    Compensated,
}

impl JournalState {
    /// The stable snake_case name, for logs and the outcome read of a later
    /// slice.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Reserved => "reserved",
            Self::Prepared => "prepared",
            Self::Executing => "executing",
            Self::Compensating => "compensating",
            Self::Committed => "committed",
            Self::Rejected => "rejected",
            Self::Compensated => "compensated",
        }
    }

    /// The state one record reports, or `None` for a record that carries no
    /// state of its own ([`RecordKind::Response`]).
    fn from_kind(kind: RecordKind) -> Option<Self> {
        Some(match kind {
            RecordKind::Reserved => Self::Reserved,
            RecordKind::Prepared => Self::Prepared,
            RecordKind::Executing => Self::Executing,
            RecordKind::Compensating => Self::Compensating,
            RecordKind::Committed => Self::Committed,
            RecordKind::Rejected => Self::Rejected,
            RecordKind::Compensated => Self::Compensated,
            RecordKind::Response => return None,
        })
    }

    /// Whether no further state record may follow this state. A saved response
    /// record may still follow a terminal state.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Committed | Self::Rejected | Self::Compensated)
    }
}

/// What a reservation attempt found.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Reservation {
    /// The id was bound by this call; its journal holds one `Reserved` record.
    Created,
    /// The id was already bound to this same request, at `state`; nothing was
    /// appended.
    Existing { state: JournalState },
}

/// Why a reservation or an observation was refused.
#[derive(Debug)]
pub enum RegistryError {
    /// The state root carries no fence, or one naming a format this build
    /// cannot speak for. Nothing was written.
    NotEnabled,
    /// The id is already bound to a different request: another digest, verb or
    /// workspace. Nothing was written.
    Conflict { request_id: String },
    /// The journal cannot be replayed as a binding this build trusts, or the
    /// request cannot be digested. Fail closed: nothing was written.
    Corrupt(String),
    /// Admission is at its record or byte quota; no new journal was created.
    Quota {
        records: usize,
        bytes: u64,
        max_records: usize,
        max_bytes: u64,
    },
    /// An IO failure reading or writing the fence or a journal.
    Io(io::Error),
}

impl std::fmt::Display for RegistryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotEnabled => write!(
                f,
                "the outcome registry is not enabled: {FENCE_FILE} is absent or names another format"
            ),
            Self::Conflict { request_id } => write!(
                f,
                "request id '{request_id}' is already bound to a different request"
            ),
            Self::Corrupt(detail) => write!(f, "the outcome registry failed closed: {detail}"),
            Self::Quota {
                records,
                bytes,
                max_records,
                max_bytes,
            } => write!(
                f,
                "outcome registry quota reached: {records}/{max_records} records, {bytes}/{max_bytes} bytes; no new journal was created"
            ),
            Self::Io(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for RegistryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::NotEnabled | Self::Conflict { .. } | Self::Corrupt(_) | Self::Quota { .. } => {
                None
            }
        }
    }
}

impl From<io::Error> for RegistryError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// Enable the registry rooted at `root` by writing the format fence (D6).
///
/// The fence is `<root>/outcomes/registry.version` holding exactly
/// `2brain.outcome-registry/v1` and a newline, written once. An identical file
/// is already enabled and is left alone; any other content names a format this
/// build cannot speak for and is [`io::ErrorKind::InvalidData`]. The file is
/// synced, and its directory with it, before this returns, so an enabled
/// workspace's fence is durable before the first journal can be created.
///
/// A failed write or sync leaves its partial file in place: the next `enable`
/// refuses it as another format rather than guessing what it meant.
pub fn enable(root: &Path) -> io::Result<()> {
    let dir = outcomes_dir(root);
    fs::create_dir_all(&dir)?;
    let path = dir.join(FENCE_FILE);
    match OpenOptions::new().write(true).create_new(true).open(&path) {
        Ok(mut file) => {
            file.write_all(FENCE_LINE.as_bytes())?;
            file.sync_all()?;
            sync_dir(&dir)?;
            Ok(())
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            let found = fs::read(&path)?;
            if found == FENCE_LINE.as_bytes() {
                Ok(())
            } else {
                Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "the registry fence in {} names another format: {:?}",
                        path.display(),
                        String::from_utf8_lossy(&found)
                    ),
                ))
            }
        }
        Err(error) => Err(error),
    }
}

pub fn fence_path(root: &Path) -> PathBuf {
    outcomes_dir(root).join(FENCE_FILE)
}

/// Write the operator quota file beside the fence.
pub fn configure_quota(root: &Path, max_records: usize, max_bytes: u64) -> io::Result<()> {
    write_quota(
        root,
        Quota {
            max_records,
            max_bytes,
        },
    )
}

/// What a startup recovery pass found and changed in the outcome registry.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OutcomeRecovery {
    /// Journals holding only `reserved`; no effect can have started, so they
    /// were closed as `rejected`.
    pub rejected: usize,
    /// Pre-terminal journals that may have started effects. They are left
    /// untouched and answer `unknown`.
    pub unresolved: usize,
    /// Journals that fail strict replay. They are left untouched.
    pub corrupt: usize,
}

/// The outcome registry of one engine state root.
#[derive(Clone, Debug)]
pub struct Registry {
    root: PathBuf,
    quota: Option<Quota>,
    #[cfg(test)]
    scan_count: std::cell::Cell<usize>,
}

impl Registry {
    /// The registry whose fence and journals live under `root`, in
    /// `<root>/outcomes/`.
    pub fn at(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            quota: None,
            #[cfg(test)]
            scan_count: std::cell::Cell::new(0),
        }
    }

    /// The registry with an explicit admission quota, used by tests until an
    /// operator configuration surface exists.
    #[cfg(test)]
    pub fn with_quota(root: &Path, quota: Quota) -> Self {
        Self {
            root: root.to_path_buf(),
            quota: Some(quota),
            scan_count: std::cell::Cell::new(0),
        }
    }

    #[cfg(test)]
    pub fn scan_count(&self) -> usize {
        self.scan_count.get()
    }

    /// The quota in force: an explicit test value, the operator file, or D5's
    /// defaults. A malformed file fails closed instead of silently using a
    /// different limit.
    fn effective_quota(&self) -> Result<Quota, String> {
        if let Some(quota) = self.quota {
            return Ok(quota);
        }
        let path = outcomes_dir(&self.root).join(QUOTA_FILE);
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Quota::default()),
            Err(error) => return Err(error.to_string()),
        };
        parse_quota(&bytes, &path)
    }

    /// Bind `id` to this request, or report what it is already bound to.
    ///
    /// The digest is recomputed here (D2) and never taken from the caller. A
    /// journal for `id` is created only when none exists; a second `Reserved`
    /// record is never appended. A request that cannot be digested is refused
    /// as [`RegistryError::Corrupt`]: the registry fails closed rather than
    /// reserve an identity it cannot bind.
    ///
    /// Reservation is serialised by the engine's write lock (D7). A journal
    /// that appears between this read and the create is a caller that did not
    /// hold the lock, and its `AlreadyExists` surfaces as
    /// [`RegistryError::Io`].
    /// Acquire the workspace-scoped exclusive registry lock.
    ///
    /// Callers must check the fence (or directory existence) before calling
    /// this so unfenced workspaces never get an `outcomes/` directory created.
    pub fn acquire_lock(&self) -> Result<RegistryLock, RegistryError> {
        acquire_registry_lock(&self.root).map_err(RegistryError::Io)
    }

    #[cfg(test)]
    pub fn reserve(
        &self,
        id: &RequestId,
        verb: &str,
        workspace: &str,
        arguments: &Value,
        preconditions: &Value,
    ) -> Result<Reservation, RegistryError> {
        self.require_fence()?;
        let lock = self.acquire_lock()?;
        self.reserve_locked(&lock, id, verb, workspace, arguments, preconditions)
    }

    /// Bind `id` to this request while the caller already holds `RegistryLock`
    /// (used by the serving path to hold one lock from reservation through
    /// journal append and index publication).
    pub fn reserve_locked(
        &self,
        lock: &RegistryLock,
        id: &RequestId,
        verb: &str,
        workspace: &str,
        arguments: &Value,
        preconditions: &Value,
    ) -> Result<Reservation, RegistryError> {
        self.require_fence()?;
        let digest =
            request_digest(verb, workspace, arguments, preconditions).map_err(|error| {
                RegistryError::Corrupt(format!("request cannot be digested: {error}"))
            })?;
        let Some(records) = self.read_records(id)? else {
            if let Some(tombstone) = self.read_tombstone(id)? {
                if tombstone.digest == digest.hex() {
                    return Ok(Reservation::Existing {
                        state: tombstone.terminal,
                    });
                }
                return Err(RegistryError::Conflict {
                    request_id: id.as_str().to_owned(),
                });
            }
            let (records, bytes) = self.require_admission_locked(lock)?;
            let mut journal = OutcomeJournal::create(&self.root, id)?;
            journal.append(
                RecordKind::Reserved,
                &reserved_payload(verb, workspace, &digest),
            )?;
            // `OutcomeJournal::create` already synced `usage.dirty` before touching disk.
            // Updating `usage.json` in O(1) clears `usage.dirty` only after atomic rename+sync;
            // if updating fails, `usage.dirty` remains on disk so the next admission rebuilds.
            let _ = write_usage_index_atomic(
                &self.root,
                records.saturating_add(1),
                bytes.saturating_add(journal.bytes_written()),
            );
            return Ok(Reservation::Created);
        };
        let state = journal_state(&records).map_err(RegistryError::Corrupt)?;
        let reserved = records.first().ok_or_else(|| {
            RegistryError::Corrupt("the journal holds no reserved record".to_owned())
        })?;
        // `journal_state` already refused a journal that does not start with
        // `Reserved`, so this payload is the binding.
        let binding = parse_reserved(&reserved.payload).map_err(RegistryError::Corrupt)?;
        if binding.digest == digest.hex() && binding.verb == verb && binding.workspace == workspace
        {
            Ok(Reservation::Existing { state })
        } else {
            Err(RegistryError::Conflict {
                request_id: id.as_str().to_owned(),
            })
        }
    }

    /// What the journal for `id` says, or `None` when there is no journal.
    ///
    /// Reads only, and does not require the fence: a journal that exists is a
    /// fact about this workspace. A journal that cannot be replayed strictly is
    /// an error, never a shorter valid history.
    pub fn observe(&self, id: &RequestId) -> Result<Option<JournalState>, RegistryError> {
        let Some(records) = self.read_records(id)? else {
            return Ok(self.read_tombstone(id)?.map(|tombstone| tombstone.terminal));
        };
        journal_state(&records)
            .map(Some)
            .map_err(RegistryError::Corrupt)
    }

    /// The saved terminal response frame for `id`, or `None` when the journal
    /// holds none.
    ///
    /// Reads only, under the caller's write lock when it is the replay path. A
    /// journal that does not replay strictly, or whose response payload is not
    /// a JSON object, is [`RegistryError::Corrupt`]: a retry must never be
    /// answered with a guessed frame.
    pub fn saved_response(&self, id: &RequestId) -> Result<Option<Value>, RegistryError> {
        let Some(records) = self.read_records(id)? else {
            return Ok(None);
        };
        journal_state(&records).map_err(RegistryError::Corrupt)?;
        let Some(record) = records
            .iter()
            .rev()
            .find(|record| record.kind == RecordKind::Response)
        else {
            return Ok(None);
        };
        let value: Value = serde_json::from_slice(&record.payload).map_err(|error| {
            RegistryError::Corrupt(format!("saved response is not JSON: {error}"))
        })?;
        if !value.is_object() {
            return Err(RegistryError::Corrupt(
                "saved response is not a JSON object".to_owned(),
            ));
        }
        Ok(Some(value))
    }

    /// The tombstone for `id`, or `None` when no sidecar exists.
    ///
    /// A tombstone is authoritative only when the journal is absent: a crash
    /// may leave both files, and the journal still carries the saved response.
    fn read_tombstone(&self, id: &RequestId) -> Result<Option<Tombstone>, RegistryError> {
        let path = tombstone_path(&self.root, id);
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(RegistryError::Io(error)),
        };
        parse_tombstone(&bytes, id)
            .map(Some)
            .map_err(RegistryError::Corrupt)
    }

    /// Write the tombstone for a terminal journal, then remove the journal.
    ///
    /// The tombstone is created before the journal is removed. A crash between
    /// the two leaves the journal authoritative and the tombstone idempotent.
    fn write_tombstone(
        &self,
        id: &RequestId,
        digest: &str,
        terminal: JournalState,
    ) -> Result<(), RegistryError> {
        let path = tombstone_path(&self.root, id);
        let parent = path
            .parent()
            .ok_or_else(|| RegistryError::Corrupt("tombstone path has no parent".to_owned()))?;
        fs::create_dir_all(parent)?;
        mark_usage_dirty(&self.root)?;
        let body = format!(
            "{{\"digest\":\"{digest}\",\"request_id\":\"{}\",\"schema\":\"{TOMBSTONE_SCHEMA}\",\"terminal\":\"{}\"}}",
            id.as_str(),
            terminal.as_str()
        );
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                file.write_all(body.as_bytes())?;
                file.sync_all()?;
                sync_dir(parent)?;
                Ok(())
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                let existing = self.read_tombstone(id)?.ok_or_else(|| {
                    RegistryError::Corrupt("tombstone vanished between create and read".to_owned())
                })?;
                if existing.digest == digest && existing.terminal == terminal {
                    Ok(())
                } else {
                    Err(RegistryError::Corrupt(format!(
                        "tombstone for {} names another binding",
                        id.as_str()
                    )))
                }
            }
            Err(error) => Err(RegistryError::Io(error)),
        }
    }

    /// Replace terminal journals older than `older_than` with tombstones (D5).
    pub fn compact(&self, older_than: Duration) -> Result<usize, RegistryError> {
        let cutoff = SystemTime::now()
            .checked_sub(older_than)
            .unwrap_or(SystemTime::UNIX_EPOCH);
        self.compact_before(cutoff)
    }

    fn compact_before(&self, cutoff: SystemTime) -> Result<usize, RegistryError> {
        let root = outcomes_dir(&self.root);
        if !root.exists() {
            return Ok(0);
        }
        let lock = self.acquire_lock()?;
        let mut directories = vec![root];
        let mut compacted = 0usize;
        while let Some(directory) = directories.pop() {
            for entry in fs::read_dir(&directory)? {
                let entry = entry?;
                let file_type = entry.file_type()?;
                if file_type.is_dir() {
                    directories.push(entry.path());
                    continue;
                }
                let name = entry.file_name().to_string_lossy().into_owned();
                let Some(stem) = name.strip_suffix(".journal") else {
                    continue;
                };
                let id = RequestId::parse(stem).map_err(RegistryError::Corrupt)?;
                if entry
                    .metadata()?
                    .modified()
                    .unwrap_or(SystemTime::UNIX_EPOCH)
                    > cutoff
                {
                    continue;
                }
                let Some(records) = self.read_records(&id)? else {
                    continue;
                };
                let state = journal_state(&records).map_err(RegistryError::Corrupt)?;
                if !state.is_terminal() {
                    continue;
                }
                let binding = parse_reserved(
                    &records
                        .first()
                        .ok_or_else(|| {
                            RegistryError::Corrupt(
                                "terminal journal holds no reserved record".to_owned(),
                            )
                        })?
                        .payload,
                )
                .map_err(RegistryError::Corrupt)?;
                self.write_tombstone(&id, &binding.digest, state)?;
                fs::remove_file(entry.path())?;
                if let Some(parent) = entry.path().parent() {
                    sync_dir(parent)?;
                }
                compacted += 1;
            }
        }
        if compacted > 0 || usage_dirty_exists(&self.root) {
            self.rebuild_usage_index_locked(&lock)?;
        }
        Ok(compacted)
    }

    /// Close journals that cannot have started an effect, and count the rest.
    ///
    /// A journal holding only its `reserved` record is proof that no effect
    /// began: `prepared` is appended before the mutation is dispatched. That
    /// is the only terminal recovery this pass may write. `prepared`,
    /// `executing` and `compensating` may have effects behind them, so they are
    /// left alone and counted as unresolved; a corrupt journal is counted, never
    /// guessed.
    pub fn recover_pre_effect(&self) -> Result<OutcomeRecovery, RegistryError> {
        let root = outcomes_dir(&self.root);
        if !root.exists() {
            return Ok(OutcomeRecovery::default());
        }
        let lock = self.acquire_lock()?;
        let mut report = OutcomeRecovery::default();
        let mut directories = vec![root];
        while let Some(directory) = directories.pop() {
            for entry in fs::read_dir(&directory)? {
                let entry = entry?;
                let file_type = entry.file_type()?;
                if file_type.is_dir() {
                    directories.push(entry.path());
                    continue;
                }
                let name = entry.file_name().to_string_lossy().into_owned();
                let Some(stem) = name.strip_suffix(".journal") else {
                    continue;
                };
                let id = RequestId::parse(stem).map_err(RegistryError::Corrupt)?;
                let records = match self.read_records(&id) {
                    Ok(Some(records)) => records,
                    Ok(None) => continue,
                    Err(RegistryError::Corrupt(_)) => {
                        report.corrupt += 1;
                        continue;
                    }
                    Err(error) => return Err(error),
                };
                let state = match journal_state(&records) {
                    Ok(state) => state,
                    Err(_) => {
                        report.corrupt += 1;
                        continue;
                    }
                };
                match state {
                    JournalState::Reserved => {
                        let mut journal = OutcomeJournal::open(&self.root, &id)?;
                        journal.append(RecordKind::Rejected, b"")?;
                        report.rejected += 1;
                    }
                    JournalState::Unknown
                    | JournalState::Prepared
                    | JournalState::Executing
                    | JournalState::Compensating => report.unresolved += 1,
                    JournalState::Committed
                    | JournalState::Rejected
                    | JournalState::Compensated => {}
                }
            }
        }
        // Always rebuild the derived quota index on startup recovery so any
        // pre-crash dirty marker, temp file, or corrupt index is healed before
        // the daemon serves its first mutation.
        self.rebuild_usage_index_locked(&lock)?;
        Ok(report)
    }

    /// Delete oldest tombstones until the outcomes tree fits `max_bytes`.
    ///
    /// Journals are never removed: only tombstones are eligible, and a deleted
    /// tombstone makes that id `unknown` again. The operator chooses the target;
    /// no admission path calls this automatically.
    ///
    /// Always scans authoritative files on disk under `RegistryLock` rather
    /// than trusting a possibly stale `usage.json`, and deletes tombstones
    /// before writing a new `usage.json` so pruning still frees space on a
    /// nearly-full disk.
    pub fn prune_tombstones_to(&self, max_bytes: u64) -> Result<(usize, u64), RegistryError> {
        let root = outcomes_dir(&self.root);
        if !root.exists() {
            return Ok((0, 0));
        }
        let lock = self.acquire_lock()?;
        // Scan ground truth and collect candidate tombstones in one pass:
        // never deletes a tombstone based on a stale usage.json and never
        // requires free disk space to write usage.json.tmp before freeing space.
        let mut bytes = 0u64;
        let mut tombstones = Vec::new();
        let mut directories = vec![root];
        while let Some(directory) = directories.pop() {
            for entry in fs::read_dir(&directory)? {
                let entry = entry?;
                let file_type = entry.file_type()?;
                if file_type.is_dir() {
                    directories.push(entry.path());
                } else if file_type.is_file() {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    if name.ends_with(".journal") {
                        bytes = bytes.saturating_add(entry.metadata()?.len());
                    } else if name.ends_with(".tombstone") {
                        let metadata = entry.metadata()?;
                        let size = metadata.len();
                        bytes = bytes.saturating_add(size);
                        tombstones.push((
                            metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                            entry.path(),
                            size,
                        ));
                    }
                }
            }
        }
        if bytes <= max_bytes {
            return Ok((0, bytes));
        }
        tombstones.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
        let mut removed = 0usize;
        let usage_path = outcomes_dir(&self.root).join(USAGE_INDEX_FILE);
        for (_, path, size) in tombstones {
            if bytes <= max_bytes {
                break;
            }
            if removed == 0 {
                // Invalidate any cached usage.json by unlinking it before
                // deleting tombstones: allocates zero disk blocks (safe under
                // ENOSPC) while ensuring a crash mid-prune forces a rebuild.
                reject_non_regular_service_file(&usage_path)?;
                match fs::remove_file(&usage_path) {
                    Ok(()) => sync_dir(&outcomes_dir(&self.root))?,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.into()),
                }
            }
            fs::remove_file(&path)?;
            if let Some(parent) = path.parent() {
                sync_dir(parent)?;
            }
            bytes = bytes.saturating_sub(size);
            removed += 1;
        }
        if removed > 0 {
            let (_, rebuilt_bytes) = self.rebuild_usage_index_locked(&lock)?;
            bytes = rebuilt_bytes;
        }
        Ok((removed, bytes))
    }

    /// Snapshot the clean derived usage index while holding `RegistryLock`
    /// before appending frames to an already-reserved journal.
    pub fn snapshot_clean_usage_locked(&self, _lock: &RegistryLock) -> Option<(usize, u64)> {
        read_clean_usage_index(&self.root)
    }

    #[cfg(test)]
    pub fn snapshot_clean_usage(&self) -> Option<(usize, u64)> {
        read_clean_usage_index(&self.root)
    }

    /// Advance the derived usage index in O(1) while holding `RegistryLock`
    /// after appending frames to an already-reserved journal, or leave
    /// `usage.dirty` on disk when the writer was poisoned or no clean baseline
    /// existed.
    ///
    /// Failure to update the derived index never fails or downgrades a durable
    /// mutation: `usage.dirty` stays on disk so the next admission or startup
    /// rebuilds exact counts from the authoritative journals.
    pub fn commit_journal_append_locked(
        &self,
        _lock: &RegistryLock,
        base: Option<(usize, u64)>,
        journal: &OutcomeJournal,
    ) {
        if journal.is_poisoned() {
            return;
        }
        let delta = journal.bytes_written();
        if delta == 0 {
            return;
        }
        let Some((records, bytes)) = base else {
            return;
        };
        let _ = write_usage_index_atomic(&self.root, records, bytes.saturating_add(delta));
    }

    #[cfg(test)]
    pub fn commit_journal_append(&self, base: Option<(usize, u64)>, journal: &OutcomeJournal) {
        if let Ok(lock) = self.acquire_lock() {
            self.commit_journal_append_locked(&lock, base, journal);
        }
    }

    /// Refuse a new journal when live usage has reached its quota (D5),
    /// returning the authoritative `(records, bytes)` read from the clean
    /// derived index or rebuilt from disk on dirty/corrupt/missing state.
    fn require_admission_locked(&self, lock: &RegistryLock) -> Result<(usize, u64), RegistryError> {
        let quota = self.effective_quota().map_err(RegistryError::Corrupt)?;
        let (records, bytes) = self.usage_locked(lock)?;
        if records >= quota.max_records || bytes >= quota.max_bytes {
            return Err(RegistryError::Quota {
                records,
                bytes,
                max_records: quota.max_records,
                max_bytes: quota.max_bytes,
            });
        }
        Ok((records, bytes))
    }

    /// Live journal count and total `.journal` + `.tombstone` bytes under
    /// `<root>/outcomes/`.
    ///
    /// Uses the O(1) CRC-checked `usage.json` snapshot when `usage.dirty` is
    /// absent and the snapshot validates; otherwise performs an authoritative
    /// rebuild from disk and atomically publishes a fresh `usage.json`.
    #[cfg(test)]
    pub fn usage(&self) -> Result<(usize, u64), RegistryError> {
        let root = outcomes_dir(&self.root);
        if !root.exists() {
            return Ok((0, 0));
        }
        let lock = self.acquire_lock()?;
        self.usage_locked(&lock)
    }

    fn usage_locked(&self, lock: &RegistryLock) -> Result<(usize, u64), RegistryError> {
        let root = outcomes_dir(&self.root);
        if !root.exists() {
            return Ok((0, 0));
        }
        if let Some(clean) = read_clean_usage_index(&self.root) {
            return Ok(clean);
        }
        self.rebuild_usage_index_locked(lock)
    }

    /// Authoritatively scan `<root>/outcomes/` and atomically publish a fresh
    /// `usage.json`, cleaning up any stray temp files and removing
    /// `usage.dirty` only after the new snapshot is durable.
    fn rebuild_usage_index_locked(
        &self,
        _lock: &RegistryLock,
    ) -> Result<(usize, u64), RegistryError> {
        let (records, bytes) = self.scan_usage()?;
        let root = outcomes_dir(&self.root);
        if root.exists() {
            cleanup_stale_usage_temps(&root);
            write_usage_index_atomic(&self.root, records, bytes)?;
        }
        Ok((records, bytes))
    }

    /// Full directory scan over `<root>/outcomes/`: the authoritative ground
    /// truth for `.journal` count and `.journal` + `.tombstone` byte totals.
    fn scan_usage(&self) -> Result<(usize, u64), RegistryError> {
        #[cfg(test)]
        self.scan_count.set(self.scan_count.get() + 1);
        let root = outcomes_dir(&self.root);
        if !root.exists() {
            return Ok((0, 0));
        }
        let mut directories = vec![root];
        let mut records = 0usize;
        let mut bytes = 0u64;
        while let Some(directory) = directories.pop() {
            for entry in fs::read_dir(&directory)? {
                let entry = entry?;
                let file_type = entry.file_type()?;
                if file_type.is_dir() {
                    directories.push(entry.path());
                } else if file_type.is_file() {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    if name.ends_with(".journal") {
                        records += 1;
                        bytes = bytes.saturating_add(entry.metadata()?.len());
                    } else if name.ends_with(".tombstone") {
                        bytes = bytes.saturating_add(entry.metadata()?.len());
                    }
                }
            }
        }
        Ok((records, bytes))
    }

    /// Whether the format fence is present and names this build's format.
    ///
    /// A read-only probe for status answers; it never enables anything.
    pub fn is_enabled(&self) -> bool {
        matches!(
            fs::read(outcomes_dir(&self.root).join(FENCE_FILE)),
            Ok(found) if found == FENCE_LINE.as_bytes()
        )
    }

    /// Every record of `id`'s journal, or `None` when it does not exist.
    fn read_records(&self, id: &RequestId) -> Result<Option<Vec<Record>>, RegistryError> {
        match OutcomeJournal::records(&self.root, id) {
            Ok(records) => Ok(records),
            Err(error) if error.kind() == io::ErrorKind::InvalidData => {
                Err(RegistryError::Corrupt(error.to_string()))
            }
            Err(error) => Err(RegistryError::Io(error)),
        }
    }

    /// Refuse a state root whose fence is absent or names another format (D6).
    fn require_fence(&self) -> Result<(), RegistryError> {
        match fs::read(outcomes_dir(&self.root).join(FENCE_FILE)) {
            Ok(found) if found == FENCE_LINE.as_bytes() => Ok(()),
            Ok(_) => Err(RegistryError::NotEnabled),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Err(RegistryError::NotEnabled),
            Err(error) => Err(RegistryError::Io(error)),
        }
    }
}

/// The state a journal's records describe.
///
/// Empty is [`JournalState::Unknown`]. Otherwise the first record must be
/// `Reserved` and every step must be one the registry writes, so a replayed
/// journal is either the one sequence its writer produced or an error: no
/// duplicate, no regression, no terminal state record that is not the last
/// state. A [`RecordKind::Response`] record may follow a terminal state once;
/// it carries the saved answer and does not change the state.
pub fn journal_state(records: &[Record]) -> Result<JournalState, String> {
    let mut state = JournalState::Unknown;
    let mut saved_response = false;
    for (index, record) in records.iter().enumerate() {
        if record.kind == RecordKind::Response {
            if index == 0 {
                return Err("journal starts with 'response', not 'reserved'".to_owned());
            }
            if !state.is_terminal() {
                return Err("journal records a response before a terminal state".to_owned());
            }
            if saved_response {
                return Err("journal holds more than one response record".to_owned());
            }
            saved_response = true;
            continue;
        }
        let next = JournalState::from_kind(record.kind)
            .expect("a non-response record kind always names a state");
        if index == 0 {
            if next != JournalState::Reserved {
                return Err(format!(
                    "journal starts with '{}', not 'reserved'",
                    next.as_str()
                ));
            }
            state = next;
            continue;
        }
        if state.is_terminal() {
            return Err(format!(
                "journal continues after the terminal '{}' record",
                state.as_str()
            ));
        }
        if !allowed(state, next) {
            return Err(format!(
                "journal moves '{}' -> '{}', which is not an allowed transition",
                state.as_str(),
                next.as_str()
            ));
        }
        state = next;
    }
    Ok(state)
}

/// The transitions D4's table admits; every other pair is a corrupt journal.
fn allowed(from: JournalState, to: JournalState) -> bool {
    use JournalState::{
        Committed, Compensated, Compensating, Executing, Prepared, Rejected, Reserved,
    };
    matches!(
        (from, to),
        (Reserved, Prepared)
            | (Reserved, Rejected)
            | (Prepared, Executing)
            | (Prepared, Rejected)
            | (Prepared, Compensating)
            | (Prepared, Committed)
            | (Executing, Compensating)
            | (Executing, Committed)
            | (Compensating, Compensated)
    )
}

/// The one `Reserved` payload: compact canonical JSON, fixed key order.
///
/// The four keys are written in the order their names sort in, which is what
/// makes the bytes canonical without a sorting pass. Values go through
/// `serde_json`'s string encoder, so a verb or workspace carrying quotes or
/// control characters still produces valid JSON.
fn reserved_payload(verb: &str, workspace: &str, digest: &RequestDigest) -> Vec<u8> {
    let mut text = String::from("{\"digest\":");
    push_json_string(&mut text, &digest.hex());
    text.push_str(",\"schema\":");
    push_json_string(&mut text, RESERVED_SCHEMA);
    text.push_str(",\"verb\":");
    push_json_string(&mut text, verb);
    text.push_str(",\"workspace\":");
    push_json_string(&mut text, workspace);
    text.push('}');
    text.into_bytes()
}

fn push_json_string(out: &mut String, text: &str) {
    out.push_str(&serde_json::to_string(text).expect("a string is always valid JSON"));
}

/// What a `Reserved` payload binds: the request digest plus the two naming
/// fields that make a conflict diagnosable without recomputing it.
struct ReservedBinding {
    digest: String,
    verb: String,
    workspace: String,
}

fn parse_reserved(payload: &[u8]) -> Result<ReservedBinding, String> {
    let value: Value = serde_json::from_slice(payload)
        .map_err(|error| format!("reserved payload is not JSON: {error}"))?;
    let object = value
        .as_object()
        .ok_or_else(|| "reserved payload is not a JSON object".to_owned())?;
    if object.get("schema").and_then(Value::as_str) != Some(RESERVED_SCHEMA) {
        return Err(format!(
            "reserved payload does not declare {RESERVED_SCHEMA}"
        ));
    }
    let field = |name: &str| -> Result<String, String> {
        object
            .get(name)
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| format!("reserved payload has no string '{name}'"))
    };
    Ok(ReservedBinding {
        digest: field("digest")?,
        verb: field("verb")?,
        workspace: field("workspace")?,
    })
}

fn parse_quota(bytes: &[u8], path: &Path) -> Result<Quota, String> {
    let value: Value = serde_json::from_slice(bytes)
        .map_err(|error| format!("quota config {} is not JSON: {error}", path.display()))?;
    let object = value
        .as_object()
        .ok_or_else(|| "quota config is not a JSON object".to_owned())?;
    if object.get("schema").and_then(Value::as_str) != Some(QUOTA_SCHEMA) {
        return Err(format!("quota config schema is not {QUOTA_SCHEMA}"));
    }
    let max_records = object
        .get("max_records")
        .and_then(Value::as_u64)
        .filter(|value| *value > 0)
        .ok_or_else(|| "quota config max_records must be a positive integer".to_owned())?;
    let max_bytes = object
        .get("max_bytes")
        .and_then(Value::as_u64)
        .filter(|value| *value > 0)
        .ok_or_else(|| "quota config max_bytes must be a positive integer".to_owned())?;
    let max_records = usize::try_from(max_records)
        .map_err(|_| "quota config max_records does not fit this platform".to_owned())?;
    Ok(Quota {
        max_records,
        max_bytes,
    })
}

fn write_quota(root: &Path, quota: Quota) -> io::Result<()> {
    let dir = outcomes_dir(root);
    fs::create_dir_all(&dir)?;
    let path = dir.join(QUOTA_FILE);
    let body = format!(
        "{{\"max_bytes\":{},\"max_records\":{},\"schema\":\"{QUOTA_SCHEMA}\"}}",
        quota.max_bytes, quota.max_records
    );
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&path)?;
    file.write_all(body.as_bytes())?;
    file.sync_all()?;
    sync_dir(&dir)?;
    Ok(())
}

fn tombstone_path(root: &Path, id: &RequestId) -> PathBuf {
    let text = id.as_str();
    outcomes_dir(root)
        .join(&text[..2])
        .join(format!("{text}.tombstone"))
}

fn parse_tombstone(bytes: &[u8], id: &RequestId) -> Result<Tombstone, String> {
    let value: Value =
        serde_json::from_slice(bytes).map_err(|error| format!("tombstone is not JSON: {error}"))?;
    let object = value
        .as_object()
        .ok_or_else(|| "tombstone is not a JSON object".to_owned())?;
    if object.get("schema").and_then(Value::as_str) != Some(TOMBSTONE_SCHEMA) {
        return Err(format!("tombstone schema is not {TOMBSTONE_SCHEMA}"));
    }
    if object.get("request_id").and_then(Value::as_str) != Some(id.as_str()) {
        return Err("tombstone names another request id".to_owned());
    }
    let digest = object
        .get("digest")
        .and_then(Value::as_str)
        .ok_or_else(|| "tombstone carries no digest".to_owned())?;
    if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("tombstone digest is not 64 hex characters".to_owned());
    }
    let terminal = match object.get("terminal").and_then(Value::as_str) {
        Some("committed") => JournalState::Committed,
        Some("rejected") => JournalState::Rejected,
        Some("compensated") => JournalState::Compensated,
        other => return Err(format!("tombstone terminal is not terminal: {other:?}")),
    };
    Ok(Tombstone {
        digest: digest.to_owned(),
        terminal,
    })
}

/// `<root>/outcomes`, the directory the fence, journals and tombstones share.
fn outcomes_dir(root: &Path) -> PathBuf {
    root.join("outcomes")
}

/// Ensure an existing service path is a regular file (never a symlink or
/// directory) before opening it.
fn reject_non_regular_service_file(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_file() => Ok(()),
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "refusing non-regular outcome registry service path: {}",
                path.display()
            ),
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn acquire_registry_lock(root: &Path) -> io::Result<RegistryLock> {
    let dir = outcomes_dir(root);
    fs::create_dir_all(&dir)?;
    let path = dir.join(REGISTRY_LOCK_FILE);
    reject_non_regular_service_file(&path)?;
    let file = match OpenOptions::new().write(true).create_new(true).open(&path) {
        Ok(file) => {
            sync_dir(&dir)?;
            file
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            reject_non_regular_service_file(&path)?;
            OpenOptions::new().read(true).write(true).open(&path)?
        }
        Err(error) => return Err(error),
    };
    fs4::fs_std::FileExt::lock_exclusive(&file)?;
    Ok(RegistryLock { _file: file })
}

/// Write and sync the write-ahead dirty marker before any journal or tombstone
/// mutation so a crash at any subsequent point forces an authoritative rebuild.
///
/// Uses `create_new` and never truncates an existing file; if `usage.dirty`
/// already exists, validates via `symlink_metadata` that it is a regular file
/// (refusing symlinks/directories) and syncs it in place without requiring new
/// disk blocks.
pub fn mark_usage_dirty(root: &Path) -> io::Result<()> {
    let dir = outcomes_dir(root);
    fs::create_dir_all(&dir)?;
    let path = dir.join(USAGE_DIRTY_FILE);
    match OpenOptions::new().write(true).create_new(true).open(&path) {
        Ok(mut file) => {
            file.write_all(b"dirty\n")?;
            file.sync_all()?;
            sync_dir(&dir)?;
            Ok(())
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            reject_non_regular_service_file(&path)?;
            File::open(&path)?.sync_all()?;
            sync_dir(&dir)?;
            Ok(())
        }
        Err(error) => Err(error),
    }
}

fn usage_dirty_exists(root: &Path) -> bool {
    let path = outcomes_dir(root).join(USAGE_DIRTY_FILE);
    !matches!(
        fs::symlink_metadata(&path),
        Err(error) if error.kind() == io::ErrorKind::NotFound
    )
}

fn cleanup_stale_usage_temps(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with(USAGE_INDEX_TMP_PREFIX)
            && name.ends_with(USAGE_INDEX_TMP_SUFFIX)
            && entry.file_type().is_ok_and(|ft| ft.is_file())
        {
            let _ = fs::remove_file(entry.path());
        }
    }
}

fn usage_crc32(records: usize, bytes: u64) -> u32 {
    let canonical = format!("{USAGE_INDEX_SCHEMA}:{records}:{bytes}");
    crate::outcome_journal::crc32(canonical.as_bytes())
}

fn parse_usage_index(raw: &[u8]) -> Option<(usize, u64)> {
    let value: Value = serde_json::from_slice(raw).ok()?;
    let object = value.as_object()?;
    if object.get("schema").and_then(Value::as_str) != Some(USAGE_INDEX_SCHEMA) {
        return None;
    }
    let records_u64 = object.get("records").and_then(Value::as_u64)?;
    let records = usize::try_from(records_u64).ok()?;
    let bytes = object.get("bytes").and_then(Value::as_u64)?;
    let crc = u32::try_from(object.get("crc32").and_then(Value::as_u64)?).ok()?;
    if crc != usage_crc32(records, bytes) {
        return None;
    }
    Some((records, bytes))
}

fn read_clean_usage_index(root: &Path) -> Option<(usize, u64)> {
    if usage_dirty_exists(root) {
        return None;
    }
    let path = outcomes_dir(root).join(USAGE_INDEX_FILE);
    let meta = fs::symlink_metadata(&path).ok()?;
    if !meta.file_type().is_file() {
        return None;
    }
    let raw = fs::read(&path).ok()?;
    parse_usage_index(&raw)
}

/// Atomically publish `usage.json` via an exclusively created unique temp file
/// (`create_new`, never following symlinks or truncating an existing file) +
/// `sync_all` + `rename` + `sync_dir`, then remove `usage.dirty` + `sync_dir`.
fn write_usage_index_atomic(root: &Path, records: usize, bytes: u64) -> io::Result<()> {
    let dir = outcomes_dir(root);
    fs::create_dir_all(&dir)?;
    let final_path = dir.join(USAGE_INDEX_FILE);
    let dirty_path = dir.join(USAGE_DIRTY_FILE);
    reject_non_regular_service_file(&final_path)?;
    reject_non_regular_service_file(&dirty_path)?;
    let crc = usage_crc32(records, bytes);
    let body = format!(
        "{{\"bytes\":{bytes},\"crc32\":{crc},\"records\":{records},\"schema\":\"{USAGE_INDEX_SCHEMA}\"}}\n"
    );
    let pid = std::process::id();
    let mut tmp_path = None;
    let mut file = None;
    for _ in 0..64 {
        let seq = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let candidate = dir.join(format!(
            "{USAGE_INDEX_TMP_PREFIX}{pid}.{seq}{USAGE_INDEX_TMP_SUFFIX}"
        ));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(opened) => {
                tmp_path = Some(candidate);
                file = Some(opened);
                break;
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    let (Some(tmp_path), Some(mut file)) = (tmp_path, file) else {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not allocate unique usage index temp file",
        ));
    };
    if let Err(error) = file
        .write_all(body.as_bytes())
        .and_then(|()| file.sync_all())
    {
        drop(file);
        let _ = fs::remove_file(&tmp_path);
        return Err(error);
    }
    drop(file);
    if let Err(error) = fs::rename(&tmp_path, &final_path) {
        let _ = fs::remove_file(&tmp_path);
        return Err(error);
    }
    sync_dir(&dir)?;
    match fs::remove_file(&dirty_path) {
        Ok(()) => sync_dir(&dir)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    Ok(())
}

/// A created entry becomes durable by syncing the directory that holds it,
/// which is a POSIX operation; elsewhere it is left to the platform. The same
/// rule the journal applies to a created journal file.
#[cfg(unix)]
fn sync_dir(dir: &Path) -> io::Result<()> {
    std::fs::File::open(dir)?.sync_all()
}

#[cfg(not(unix))]
fn sync_dir(_dir: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const ID: &str = "0123456789abcdef0123456789abcdef";
    const OTHER_ID: &str = "fedcba9876543210fedcba9876543210";

    fn request_id(text: &str) -> RequestId {
        RequestId::parse(text).expect("valid request id")
    }

    fn args() -> Value {
        json!({"title": "hello", "body": "text"})
    }

    fn preconditions() -> Value {
        json!({"expected_absent": true})
    }

    fn enabled(root: &Path) -> Registry {
        enable(root).expect("enable the registry");
        Registry::at(root)
    }

    fn fence_path(root: &Path) -> PathBuf {
        outcomes_dir(root).join(FENCE_FILE)
    }

    fn journal_path(root: &Path, id: &str) -> PathBuf {
        outcomes_dir(root)
            .join(&id[..2])
            .join(format!("{id}.journal"))
    }

    fn records_of(root: &Path, id: &str) -> Vec<Record> {
        OutcomeJournal::records(root, &request_id(id))
            .expect("read journal")
            .expect("journal exists")
    }

    fn record(kind: RecordKind) -> Record {
        Record {
            kind,
            payload: Vec::new(),
        }
    }

    #[test]
    fn enable_writes_the_fence_and_reserve_creates_the_journal() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let registry = enabled(root);

        assert_eq!(
            fs::read(fence_path(root)).expect("read fence"),
            FENCE_LINE.as_bytes()
        );
        assert_eq!(
            registry
                .reserve(
                    &request_id(ID),
                    "note.create",
                    "ws",
                    &args(),
                    &preconditions()
                )
                .expect("reserve"),
            Reservation::Created
        );
        assert!(
            journal_path(root, ID).is_file(),
            "the journal sits where the registry reads it"
        );
        assert_eq!(
            registry.observe(&request_id(ID)).expect("observe"),
            Some(JournalState::Reserved)
        );
        assert_eq!(
            registry.observe(&request_id(OTHER_ID)).expect("observe"),
            None,
            "an id with no journal is absent, not unknown"
        );
    }

    #[test]
    fn the_reserved_record_binds_the_request_digest_canonically() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let registry = enabled(root);
        registry
            .reserve(
                &request_id(ID),
                "note.create",
                "ws",
                &args(),
                &preconditions(),
            )
            .expect("reserve");

        let records = records_of(root, ID);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].kind, RecordKind::Reserved);
        let digest =
            request_digest("note.create", "ws", &args(), &preconditions()).expect("digest");
        let mut expected = String::from("{\"digest\":");
        expected.push_str(&serde_json::to_string(&digest.hex()).expect("json string"));
        expected.push_str(
            ",\"schema\":\"2brain.outcome-reserved/v1\",\"verb\":\"note.create\",\"workspace\":\"ws\"}",
        );
        assert_eq!(
            String::from_utf8(records[0].payload.clone()).expect("utf8"),
            expected,
            "compact canonical JSON with sorted keys and no whitespace"
        );
    }

    #[test]
    fn enable_accepts_its_own_fence_and_refuses_another_format() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        enable(root).expect("first enable");
        enable(root).expect("an identical fence is already enabled");

        fs::write(fence_path(root), "2brain.outcome-registry/v2\n").expect("rewrite fence");
        let error = enable(root).expect_err("another format is refused");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("2brain.outcome-registry/v2"));
    }

    #[test]
    fn a_retry_of_the_same_request_observes_it_without_touching_the_journal() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let registry = enabled(root);
        let id = request_id(ID);
        assert_eq!(
            registry
                .reserve(&id, "note.create", "ws", &args(), &preconditions())
                .expect("reserve"),
            Reservation::Created
        );
        let bytes = fs::read(journal_path(root, ID)).expect("read journal");

        assert_eq!(
            registry
                .reserve(&id, "note.create", "ws", &args(), &preconditions())
                .expect("reserve again"),
            Reservation::Existing {
                state: JournalState::Reserved
            }
        );
        assert_eq!(
            fs::read(journal_path(root, ID)).expect("read journal"),
            bytes,
            "a retry appends nothing"
        );
        assert_eq!(
            records_of(root, ID).len(),
            1,
            "never a second reserved record"
        );
    }

    #[test]
    fn the_same_id_with_another_request_conflicts_and_appends_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let registry = enabled(root);
        let id = request_id(ID);
        registry
            .reserve(&id, "note.create", "ws", &args(), &preconditions())
            .expect("reserve");
        let bytes = fs::read(journal_path(root, ID)).expect("read journal");
        let conflict = |result: Result<Reservation, RegistryError>| match result {
            Err(RegistryError::Conflict { request_id }) => assert_eq!(request_id, ID),
            other => panic!("expected a conflict, got {other:?}"),
        };

        conflict(registry.reserve(
            &id,
            "note.create",
            "ws",
            &json!({"title": "other"}),
            &preconditions(),
        ));
        conflict(registry.reserve(&id, "note.update", "ws", &args(), &preconditions()));
        conflict(registry.reserve(&id, "note.create", "other-ws", &args(), &preconditions()));
        conflict(registry.reserve(
            &id,
            "note.create",
            "ws",
            &args(),
            &json!({"expected_hash": "abc"}),
        ));
        assert_eq!(
            fs::read(journal_path(root, ID)).expect("read journal"),
            bytes
        );
    }

    #[test]
    fn quota_refuses_a_new_id_but_keeps_existing_recovery() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        enable(root).expect("enable the registry");
        let registry = Registry::with_quota(
            root,
            Quota {
                max_records: 1,
                max_bytes: 1 << 20,
            },
        );
        registry
            .reserve(
                &request_id(ID),
                "note.create",
                "ws",
                &args(),
                &preconditions(),
            )
            .expect("first reservation is admitted");
        let refused = registry.reserve(
            &request_id(OTHER_ID),
            "note.create",
            "ws",
            &args(),
            &preconditions(),
        );
        assert!(
            matches!(
                refused,
                Err(RegistryError::Quota {
                    records: 1,
                    max_records: 1,
                    ..
                })
            ),
            "quota refusal: {refused:?}"
        );
        assert_eq!(
            registry.observe(&request_id(ID)).expect("observe"),
            Some(JournalState::Reserved),
            "quota pressure never discards live recovery evidence"
        );
        assert_eq!(
            registry
                .reserve(
                    &request_id(ID),
                    "note.create",
                    "ws",
                    &args(),
                    &preconditions()
                )
                .expect("an existing id is replayed, not admitted anew"),
            Reservation::Existing {
                state: JournalState::Reserved
            }
        );
    }

    #[test]
    fn a_quota_config_file_limits_admission() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        enable(root).expect("enable the registry");
        configure_quota(root, 1, 1 << 20).expect("write quota");
        let registry = Registry::at(root);
        registry
            .reserve(
                &request_id(ID),
                "note.create",
                "ws",
                &args(),
                &preconditions(),
            )
            .expect("first reservation");
        assert!(matches!(
            registry.reserve(
                &request_id(OTHER_ID),
                "note.create",
                "ws",
                &args(),
                &preconditions()
            ),
            Err(RegistryError::Quota { max_records: 1, .. })
        ));
    }

    #[test]
    fn a_malformed_quota_config_fails_closed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        enable(root).expect("enable the registry");
        fs::write(outcomes_dir(root).join(QUOTA_FILE), b"{}").expect("write bad config");
        assert!(matches!(
            Registry::at(root).reserve(
                &request_id(ID),
                "note.create",
                "ws",
                &args(),
                &preconditions()
            ),
            Err(RegistryError::Corrupt(detail)) if detail.contains("quota config")
        ));
    }

    #[test]
    fn a_byte_quota_refuses_the_next_new_id() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        enable(root).expect("enable the registry");
        let registry = Registry::with_quota(
            root,
            Quota {
                max_records: 100,
                max_bytes: 1,
            },
        );
        registry
            .reserve(
                &request_id(ID),
                "note.create",
                "ws",
                &args(),
                &preconditions(),
            )
            .expect("the first journal starts below the byte quota");
        let refused = registry.reserve(
            &request_id(OTHER_ID),
            "note.create",
            "ws",
            &args(),
            &preconditions(),
        );
        assert!(
            matches!(&refused, Err(RegistryError::Quota { bytes, .. }) if *bytes > 1),
            "byte quota refusal: {refused:?}"
        );
        let message = refused.unwrap_err().to_string();
        assert!(message.contains("quota reached"), "{message}");
    }

    #[test]
    fn reserve_without_a_fence_is_not_enabled_and_writes_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let registry = Registry::at(root);

        assert!(matches!(
            registry.reserve(
                &request_id(ID),
                "note.create",
                "ws",
                &args(),
                &preconditions()
            ),
            Err(RegistryError::NotEnabled)
        ));
        assert!(
            !journal_path(root, ID).exists(),
            "a refused reservation creates no journal"
        );
    }

    #[test]
    fn a_fence_naming_another_format_refuses_the_reservation() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        fs::create_dir_all(outcomes_dir(root)).expect("create outcomes dir");
        fs::write(fence_path(root), "2brain.outcome-registry/v2\n").expect("write fence");
        let registry = Registry::at(root);

        assert!(matches!(
            registry.reserve(
                &request_id(ID),
                "note.create",
                "ws",
                &args(),
                &preconditions()
            ),
            Err(RegistryError::NotEnabled)
        ));
        assert!(!journal_path(root, ID).exists());
    }

    #[test]
    fn a_flipped_journal_byte_is_corrupt() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let registry = enabled(root);
        registry
            .reserve(
                &request_id(ID),
                "note.create",
                "ws",
                &args(),
                &preconditions(),
            )
            .expect("reserve");

        let path = journal_path(root, ID);
        let mut bytes = fs::read(&path).expect("read journal");
        let last = bytes.len() - 1;
        bytes[last] ^= 0xFF;
        fs::write(&path, &bytes).expect("rewrite journal");

        let observed = registry.observe(&request_id(ID));
        assert!(
            matches!(observed, Err(RegistryError::Corrupt(_))),
            "got {observed:?}"
        );
        assert!(matches!(
            registry.reserve(
                &request_id(ID),
                "note.create",
                "ws",
                &args(),
                &preconditions()
            ),
            Err(RegistryError::Corrupt(_))
        ));
    }

    #[test]
    fn observe_reports_the_state_the_journal_holds() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let registry = Registry::at(root);
        let mut journal = OutcomeJournal::create(root, &request_id(ID)).expect("create journal");
        journal
            .append(RecordKind::Reserved, b"reserved")
            .expect("append");
        journal
            .append(RecordKind::Prepared, b"prepared")
            .expect("append");

        // Observation reads a journal as it stands; only a reservation needs
        // the fence.
        assert_eq!(
            registry.observe(&request_id(ID)).expect("observe"),
            Some(JournalState::Prepared)
        );
    }

    #[test]
    fn an_existing_journal_with_no_record_observes_unknown() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let _journal = OutcomeJournal::create(root, &request_id(ID)).expect("create journal");

        assert_eq!(
            Registry::at(root)
                .observe(&request_id(ID))
                .expect("observe"),
            Some(JournalState::Unknown)
        );
    }

    #[test]
    fn journal_state_accepts_every_documented_sequence() {
        use RecordKind::{
            Committed, Compensated, Compensating, Executing, Prepared, Rejected, Reserved,
        };
        let cases: [(&[RecordKind], JournalState); 11] = [
            (&[], JournalState::Unknown),
            (&[Reserved], JournalState::Reserved),
            (&[Reserved, Prepared], JournalState::Prepared),
            (&[Reserved, Prepared, Executing], JournalState::Executing),
            (
                &[Reserved, Prepared, Executing, Compensating],
                JournalState::Compensating,
            ),
            (&[Reserved, Rejected], JournalState::Rejected),
            (&[Reserved, Prepared, Rejected], JournalState::Rejected),
            (
                &[Reserved, Prepared, Compensating, Compensated],
                JournalState::Compensated,
            ),
            (&[Reserved, Prepared, Committed], JournalState::Committed),
            (
                &[Reserved, Prepared, Executing, Committed],
                JournalState::Committed,
            ),
            (
                &[Reserved, Prepared, Executing, Compensating, Compensated],
                JournalState::Compensated,
            ),
        ];
        for (kinds, expected) in cases {
            let records: Vec<Record> = kinds.iter().copied().map(record).collect();
            assert_eq!(journal_state(&records), Ok(expected), "sequence {kinds:?}");
        }
    }

    #[test]
    fn journal_state_rejects_duplicates_regressions_and_misplaced_terminals() {
        use RecordKind::{Committed, Compensated, Compensating, Executing, Prepared, Reserved};
        let sequences: [&[RecordKind]; 11] = [
            &[Prepared],
            &[Committed],
            &[Reserved, Reserved],
            &[Reserved, Prepared, Prepared],
            &[Reserved, Prepared, Executing, Prepared],
            &[Reserved, Prepared, Executing, Compensating, Executing],
            &[Reserved, Prepared, Executing, Committed, Prepared],
            &[Reserved, Prepared, Committed, Committed],
            &[Reserved, Executing],
            &[Reserved, Committed],
            &[Reserved, Prepared, Compensated],
        ];
        for kinds in sequences {
            let records: Vec<Record> = kinds.iter().copied().map(record).collect();
            let state = journal_state(&records);
            assert!(
                state.is_err(),
                "sequence {kinds:?} was accepted as {state:?}"
            );
            assert!(!state.unwrap_err().is_empty(), "a refusal carries a reason");
        }
    }

    #[test]
    fn a_response_record_follows_a_terminal_state_without_changing_it() {
        use RecordKind::{Committed, Rejected, Response};
        let records = vec![
            record(RecordKind::Reserved),
            record(RecordKind::Prepared),
            record(Committed),
            Record {
                kind: Response,
                payload: b"{\"type\":\"response\"}".to_vec(),
            },
        ];
        assert_eq!(journal_state(&records), Ok(JournalState::Committed));

        let rejected = vec![
            record(RecordKind::Reserved),
            record(Rejected),
            Record {
                kind: Response,
                payload: b"{}".to_vec(),
            },
        ];
        assert_eq!(journal_state(&rejected), Ok(JournalState::Rejected));
    }

    #[test]
    fn a_response_record_before_a_terminal_or_twice_is_corrupt() {
        use RecordKind::{Committed, Response};
        let response = || Record {
            kind: Response,
            payload: b"{}".to_vec(),
        };
        let before_terminal = vec![
            record(RecordKind::Reserved),
            record(RecordKind::Prepared),
            response(),
        ];
        assert!(journal_state(&before_terminal).is_err());

        let twice = vec![
            record(RecordKind::Reserved),
            record(RecordKind::Prepared),
            record(Committed),
            response(),
            response(),
        ];
        assert!(journal_state(&twice).is_err());
    }

    #[test]
    fn saved_response_reads_the_record_or_refuses_a_bad_payload() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let registry = Registry::at(root);
        let id = request_id(ID);
        let mut journal = OutcomeJournal::create(root, &id).expect("create journal");
        journal.append(RecordKind::Reserved, b"").expect("append");
        journal.append(RecordKind::Prepared, b"").expect("append");
        journal.append(RecordKind::Committed, b"").expect("append");
        assert_eq!(registry.saved_response(&id).expect("read"), None);

        journal
            .append_response(br#"{"type":"response","replay":"available"}"#)
            .expect("append response");
        let saved = registry
            .saved_response(&id)
            .expect("read")
            .expect("a saved response");
        assert_eq!(saved["type"], "response");
        assert_eq!(saved["replay"], "available");
        assert_eq!(
            registry.observe(&id).expect("observe"),
            Some(JournalState::Committed),
            "a saved response does not change the state"
        );
        drop(journal);

        let bad_id = request_id(OTHER_ID);
        let mut journal = OutcomeJournal::create(root, &bad_id).expect("create journal");
        journal.append(RecordKind::Reserved, b"").expect("append");
        journal.append(RecordKind::Prepared, b"").expect("append");
        journal.append(RecordKind::Committed, b"").expect("append");
        journal
            .append_response(b"not json")
            .expect("append response");
        assert!(matches!(
            registry.saved_response(&bad_id),
            Err(RegistryError::Corrupt(detail)) if detail.contains("saved response")
        ));
    }

    fn age_file(path: &Path, seconds: u64) {
        let file = OpenOptions::new()
            .write(true)
            .open(path)
            .expect("open file");
        file.set_times(
            fs::FileTimes::new()
                .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)),
        )
        .expect("set file mtime");
    }

    fn age_journal(path: &Path) {
        age_file(path, 1);
    }

    #[test]
    fn a_terminal_tombstone_answers_after_the_journal_is_gone() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let registry = enabled(root);
        let id = request_id(ID);
        registry
            .reserve(&id, "note.create", "ws", &args(), &preconditions())
            .expect("reserve");
        let mut journal = OutcomeJournal::open(root, &id).expect("open journal");
        journal.append(RecordKind::Prepared, b"").expect("prepared");
        journal
            .append(RecordKind::Committed, b"")
            .expect("committed");
        drop(journal);
        age_journal(&journal_path(root, ID));

        assert_eq!(
            registry.compact_before(SystemTime::now()).expect("compact"),
            1
        );
        assert!(!journal_path(root, ID).exists(), "the journal was replaced");
        assert!(tombstone_path(root, &id).is_file(), "tombstone exists");

        let reopened = Registry::at(root);
        assert_eq!(
            reopened.observe(&id).expect("observe"),
            Some(JournalState::Committed),
            "a restart still answers committed"
        );
        assert_eq!(reopened.saved_response(&id).expect("saved"), None);
        assert_eq!(
            reopened
                .reserve(&id, "note.create", "ws", &args(), &preconditions())
                .expect("same request"),
            Reservation::Existing {
                state: JournalState::Committed
            }
        );
        assert!(matches!(
            reopened.reserve(&id, "note.update", "ws", &args(), &preconditions()),
            Err(RegistryError::Conflict { .. })
        ));
    }

    #[test]
    fn automatic_compaction_is_inert_without_a_fence() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert_eq!(
            crate::auto_compact_outcome_registry(dir.path()).expect("auto compact"),
            0
        );
    }

    #[test]
    fn startup_compaction_replaces_an_aged_terminal_journal() {
        let dir = tempfile::tempdir().expect("tempdir");
        let registry_root = crate::outcome_registry_root(dir.path());
        let registry = enabled(&registry_root);
        let id = request_id(ID);
        registry
            .reserve(&id, "note.create", "ws", &args(), &preconditions())
            .expect("reserve");
        let mut journal = OutcomeJournal::open(&registry_root, &id).expect("open journal");
        journal.append(RecordKind::Prepared, b"").expect("prepared");
        journal
            .append(RecordKind::Committed, b"")
            .expect("committed");
        drop(journal);
        age_journal(&journal_path(&registry_root, ID));

        assert_eq!(
            crate::auto_compact_outcome_registry(dir.path()).expect("auto compact"),
            1
        );
        assert!(!journal_path(&registry_root, ID).exists());
        assert!(tombstone_path(&registry_root, &id).is_file());
        assert_eq!(
            Registry::at(&registry_root).observe(&id).expect("observe"),
            Some(JournalState::Committed)
        );
    }

    #[test]
    fn recovery_is_inert_without_a_fence() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert_eq!(
            crate::recover_outcome_registry(dir.path()).expect("recover"),
            OutcomeRecovery::default()
        );
    }

    #[test]
    fn startup_recovery_rejects_only_a_reserved_journal() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let registry = enabled(root);
        let reserved = request_id("11111111111111111111111111111111");
        let prepared = request_id("22222222222222222222222222222222");
        let executing = request_id("33333333333333333333333333333333");
        for id in [&reserved, &prepared, &executing] {
            registry
                .reserve(id, "note.create", "ws", &args(), &preconditions())
                .expect("reserve");
        }
        let mut journal = OutcomeJournal::open(root, &prepared).expect("prepared journal");
        journal.append(RecordKind::Prepared, b"").expect("prepared");
        drop(journal);
        let mut journal = OutcomeJournal::open(root, &executing).expect("executing journal");
        journal.append(RecordKind::Prepared, b"").expect("prepared");
        journal
            .append(RecordKind::Executing, b"")
            .expect("executing");
        drop(journal);

        let report = registry.recover_pre_effect().expect("recover");
        assert_eq!(report.rejected, 1);
        assert_eq!(report.unresolved, 2);
        assert_eq!(report.corrupt, 0);
        assert_eq!(
            registry.observe(&reserved).expect("observe reserved"),
            Some(JournalState::Rejected)
        );
        assert_eq!(
            registry.observe(&prepared).expect("observe prepared"),
            Some(JournalState::Prepared)
        );
        assert_eq!(
            registry.observe(&executing).expect("observe executing"),
            Some(JournalState::Executing)
        );

        let again = registry.recover_pre_effect().expect("recover again");
        assert_eq!(again.rejected, 0, "a rejected journal is already terminal");
        assert_eq!(again.unresolved, 2);
    }

    #[test]
    fn startup_recovery_counts_a_corrupt_journal_without_guessing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let registry = enabled(root);
        let id = request_id(ID);
        registry
            .reserve(&id, "note.create", "ws", &args(), &preconditions())
            .expect("reserve");
        fs::write(journal_path(root, ID), b"not a journal").expect("corrupt journal");

        let report = registry.recover_pre_effect().expect("recover");
        assert_eq!(report.rejected, 0);
        assert_eq!(report.unresolved, 0);
        assert_eq!(report.corrupt, 1);
        assert!(matches!(
            registry.observe(&id),
            Err(RegistryError::Corrupt(_))
        ));
    }

    #[test]
    fn pruning_removes_oldest_tombstones_but_never_journals() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let registry = enabled(root);
        let first = request_id(ID);
        let second = request_id(OTHER_ID);
        let digest = "a".repeat(64);
        registry
            .write_tombstone(&first, &digest, JournalState::Committed)
            .expect("first tombstone");
        registry
            .write_tombstone(&second, &digest, JournalState::Rejected)
            .expect("second tombstone");
        age_file(&tombstone_path(root, &first), 1);
        age_file(&tombstone_path(root, &second), 2);
        let second_size = fs::metadata(tombstone_path(root, &second)).unwrap().len();
        let target = second_size;
        let (removed, remaining) = registry.prune_tombstones_to(target).expect("prune");
        assert_eq!(removed, 1);
        assert_eq!(remaining, second_size);
        assert!(!tombstone_path(root, &first).exists());
        assert!(tombstone_path(root, &second).exists());

        registry
            .reserve(&first, "note.create", "ws", &args(), &preconditions())
            .expect("journal");
        registry
            .write_tombstone(&second, &digest, JournalState::Rejected)
            .expect("tombstone");
        let (removed, _) = registry.prune_tombstones_to(0).expect("prune all");
        assert_eq!(removed, 1);
        assert!(
            journal_path(root, ID).is_file(),
            "a journal is live recovery evidence and is never pruned"
        );
    }

    #[test]
    fn compaction_skips_non_terminal_and_recent_journals() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let registry = enabled(root);
        registry
            .reserve(
                &request_id(ID),
                "note.create",
                "ws",
                &args(),
                &preconditions(),
            )
            .expect("reserve");
        age_journal(&journal_path(root, ID));
        assert_eq!(
            registry.compact_before(SystemTime::now()).expect("compact"),
            0,
            "a reserved journal is live recovery evidence"
        );
        assert!(journal_path(root, ID).is_file());
    }

    #[test]
    fn a_corrupt_tombstone_fails_closed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let registry = enabled(root);
        let id = request_id(ID);
        let path = tombstone_path(root, &id);
        fs::create_dir_all(path.parent().expect("parent")).expect("dirs");
        fs::write(&path, b"{}").expect("write tombstone");
        assert!(matches!(
            registry.observe(&id),
            Err(RegistryError::Corrupt(detail)) if detail.contains("tombstone")
        ));
        assert!(matches!(
            registry.reserve(&id, "note.create", "ws", &args(), &preconditions()),
            Err(RegistryError::Corrupt(_))
        ));
    }

    #[test]
    fn state_names_are_the_snake_case_ones() {
        assert_eq!(JournalState::Unknown.as_str(), "unknown");
        assert_eq!(JournalState::Reserved.as_str(), "reserved");
        assert_eq!(JournalState::Prepared.as_str(), "prepared");
        assert_eq!(JournalState::Executing.as_str(), "executing");
        assert_eq!(JournalState::Compensating.as_str(), "compensating");
        assert_eq!(JournalState::Committed.as_str(), "committed");
        assert_eq!(JournalState::Rejected.as_str(), "rejected");
        assert_eq!(JournalState::Compensated.as_str(), "compensated");
    }

    #[test]
    fn usage_index_serves_o1_admissions_without_rescanning_and_matches_disk() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let registry = enabled(root);

        for idx in 0..6u8 {
            let id = request_id(&format!("{idx:032x}"));
            assert_eq!(
                registry
                    .reserve(&id, "note.create", "ws", &args(), &preconditions())
                    .expect("reserve"),
                Reservation::Created
            );
            assert!(
                !outcomes_dir(root).join(USAGE_DIRTY_FILE).exists(),
                "clean admission clears usage.dirty"
            );
            assert!(
                outcomes_dir(root).join(USAGE_INDEX_FILE).is_file(),
                "usage.json exists after admission"
            );
        }

        // Only the very first reserve scanned the empty tree; the 5 subsequent
        // admissions hit the O(1) usage.json fast path with zero extra scans.
        assert_eq!(
            registry.scan_count(),
            1,
            "clean steady-state admissions must not rescan outcomes/"
        );
        let indexed = registry.usage().expect("indexed usage");
        assert_eq!(
            registry.scan_count(),
            1,
            "reading clean index does not scan"
        );
        let scanned = registry.scan_usage().expect("ground-truth scan");
        assert_eq!(indexed, scanned, "usage.json must match disk byte-for-byte");
        assert_eq!(indexed.0, 6);
    }

    #[test]
    fn usage_index_rebuilds_after_dirty_crash_temp_leftover_and_corrupt_crc() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let registry = enabled(root);

        let id1 = request_id("10000000000000000000000000000001");
        registry
            .reserve(&id1, "note.create", "ws", &args(), &preconditions())
            .expect("reserve id1");

        // Crash simulation A: dirty marker left after an extra journal appeared on disk.
        let id2 = request_id("10000000000000000000000000000002");
        let mut raw_journal = OutcomeJournal::create(root, &id2).expect("create raw journal");
        raw_journal
            .append(RecordKind::Reserved, b"raw")
            .expect("append raw");
        drop(raw_journal);
        assert!(
            outcomes_dir(root).join(USAGE_DIRTY_FILE).exists(),
            "OutcomeJournal::create must mark usage.dirty before creating file"
        );
        let reopened = Registry::at(root);
        let rebuilt = reopened.usage().expect("rebuild after dirty");
        assert_eq!(rebuilt.0, 2);
        assert_eq!(rebuilt, reopened.scan_usage().expect("scan"));
        assert!(!outcomes_dir(root).join(USAGE_DIRTY_FILE).exists());

        // Crash simulation B: tampered CRC / truncated JSON / stray temp file.
        for bad_payload in [
            b"{\"bytes\":0,\"crc32\":0,\"records\":0,\"schema\":\"2brain.outcome-usage/v1\"}"
                .as_slice(),
            b"{truncated".as_slice(),
            b"{\"bytes\":0,\"crc32\":0,\"records\":0,\"schema\":\"other/v9\"}".as_slice(),
        ] {
            let stray_tmp = outcomes_dir(root).join("usage.999.0.tmp");
            fs::write(outcomes_dir(root).join(USAGE_INDEX_FILE), bad_payload)
                .expect("corrupt usage.json");
            fs::write(&stray_tmp, b"partial tmp").expect("stray tmp");
            let fresh = Registry::at(root);
            let recovered = fresh.usage().expect("rebuild after corrupt usage.json");
            assert_eq!(recovered, rebuilt);
            assert_eq!(fresh.scan_count(), 1, "corrupt index triggers one rebuild");
            assert!(
                !stray_tmp.exists(),
                "rebuild cleans up stray usage.*.tmp files"
            );
        }
    }

    #[test]
    fn poisoned_journal_append_keeps_dirty_marker_and_rebuilds_exact_bytes_on_restart() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let registry = enabled(root);
        let id = request_id(ID);
        registry
            .reserve(&id, "note.create", "ws", &args(), &preconditions())
            .expect("reserve");
        let base = registry.snapshot_clean_usage();
        assert!(base.is_some());

        let mut journal = OutcomeJournal::open(root, &id).expect("open");
        journal.append(RecordKind::Prepared, b"").expect("prepared");
        journal.fail_next_append();
        assert!(journal.append(RecordKind::Committed, b"").is_err());
        assert!(journal.is_poisoned());

        // Committing a poisoned handle MUST leave usage.dirty in place so restart rebuilds.
        registry.commit_journal_append(base, &journal);
        assert!(
            outcomes_dir(root).join(USAGE_DIRTY_FILE).exists(),
            "poisoned journal must keep usage.dirty on disk"
        );
        drop(journal);

        // Simulated restart: startup recovery rebuilds exact usage and clears usage.dirty.
        let restarted = Registry::at(root);
        let report = restarted.recover_pre_effect().expect("recover");
        assert_eq!(report.unresolved, 1);
        assert!(!outcomes_dir(root).join(USAGE_DIRTY_FILE).exists());
        let after = restarted.usage().expect("usage after restart");
        assert_eq!(after, restarted.scan_usage().expect("scan"));
        assert!(after.1 > base.unwrap().1, "Prepared frame bytes accounted");
    }

    #[test]
    fn quota_index_tracks_compaction_pruning_and_restart_without_evicting_existing_ids() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        enable(root).expect("enable");
        configure_quota(root, 2, 64 * 1024).expect("quota");
        let registry = Registry::at(root);

        let id1 = request_id("20000000000000000000000000000001");
        let id2 = request_id("20000000000000000000000000000002");
        let id3 = request_id("20000000000000000000000000000003");
        registry
            .reserve(&id1, "note.create", "ws", &args(), &preconditions())
            .expect("reserve 1");
        let base = registry.snapshot_clean_usage();
        let mut j1 = OutcomeJournal::open(root, &id1).expect("open 1");
        j1.append(RecordKind::Prepared, b"").expect("prep");
        j1.append(RecordKind::Committed, b"").expect("commit");
        registry.commit_journal_append(base, &j1);
        drop(j1);

        registry
            .reserve(&id2, "note.create", "ws", &args(), &preconditions())
            .expect("reserve 2");

        // At quota (2/2): new id is refused, existing id still replays.
        assert!(matches!(
            registry.reserve(&id3, "note.create", "ws", &args(), &preconditions()),
            Err(RegistryError::Quota {
                records: 2,
                max_records: 2,
                ..
            })
        ));
        assert_eq!(
            registry
                .reserve(&id1, "note.create", "ws", &args(), &preconditions())
                .expect("existing id allowed at quota"),
            Reservation::Existing {
                state: JournalState::Committed
            }
        );

        // Compact aged id1 into a tombstone -> live records drop to 1, freeing admission.
        age_journal(&journal_path(root, id1.as_str()));
        assert_eq!(
            registry.compact_before(SystemTime::now()).expect("compact"),
            1
        );
        assert!(!outcomes_dir(root).join(USAGE_DIRTY_FILE).exists());
        let after_compact = Registry::at(root);
        let (recs, bytes_after_compact) = after_compact.usage().expect("usage after compact");
        assert_eq!(recs, 1);
        assert_eq!(
            after_compact.scan_count(),
            0,
            "compact published clean index"
        );
        assert_eq!(
            (recs, bytes_after_compact),
            after_compact.scan_usage().expect("scan")
        );

        // Prune tombstone -> bytes shrink cleanly in usage.json.
        let (removed, remaining) = after_compact.prune_tombstones_to(0).expect("prune");
        assert_eq!(removed, 1);
        let reopened = Registry::at(root);
        let (recs2, bytes2) = reopened.usage().expect("usage after prune");
        assert_eq!(recs2, 1);
        assert_eq!(bytes2, remaining);
        assert_eq!(reopened.scan_count(), 0, "prune published clean index");
        assert_eq!(
            reopened
                .reserve(&id3, "note.create", "ws", &args(), &preconditions())
                .expect("id3 admitted after compaction"),
            Reservation::Created
        );
    }

    #[test]
    fn prune_uses_ground_truth_disk_size_even_when_usage_index_is_stale_and_dirty() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let registry = enabled(root);
        let first = request_id(ID);
        let second = request_id(OTHER_ID);
        let digest = "b".repeat(64);
        registry
            .write_tombstone(&first, &digest, JournalState::Committed)
            .expect("first tombstone");
        registry
            .write_tombstone(&second, &digest, JournalState::Rejected)
            .expect("second tombstone");
        age_file(&tombstone_path(root, &first), 1);
        age_file(&tombstone_path(root, &second), 2);
        let first_size = fs::metadata(tombstone_path(root, &first)).unwrap().len();
        let second_size = fs::metadata(tombstone_path(root, &second)).unwrap().len();
        let actual_bytes = first_size + second_size;

        // Write a syntactically valid CRC-32 usage.json that overstates disk
        // usage (e.g. left behind by an older CLI). Prune MUST use ground truth
        // and refuse to over-prune when actual disk bytes already fit max_bytes.
        write_usage_index_atomic(root, 0, actual_bytes + 50_000).expect("write stale index");
        let (removed, remaining) = registry
            .prune_tombstones_to(actual_bytes + 100)
            .expect("prune under actual bytes");
        assert_eq!(
            removed, 0,
            "stale overstated usage.json must not over-prune"
        );
        assert_eq!(remaining, actual_bytes);
        assert!(tombstone_path(root, &first).is_file());
        assert!(tombstone_path(root, &second).is_file());

        // When actual bytes exceed target (even with pre-existing usage.dirty),
        // prune deletes only the oldest tombstone and publishes exact usage.json.
        mark_usage_dirty(root).expect("mark dirty");
        let (removed, remaining) = registry.prune_tombstones_to(second_size).expect("prune");
        assert_eq!(removed, 1);
        assert_eq!(remaining, second_size);
        assert!(!tombstone_path(root, &first).exists());
        assert!(tombstone_path(root, &second).is_file());
        assert!(!outcomes_dir(root).join(USAGE_DIRTY_FILE).exists());
        let reopened = Registry::at(root);
        assert_eq!(reopened.usage().expect("usage"), (0, second_size));
        assert_eq!(reopened.scan_count(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_service_files_fail_closed_without_truncating_journals() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let registry = enabled(root);
        let id = request_id(ID);
        registry
            .reserve(&id, "note.create", "ws", &args(), &preconditions())
            .expect("reserve");
        let jpath = journal_path(root, ID);
        let original = fs::read(&jpath).expect("read journal");

        // Symlinked usage.dirty pointing at a journal must fail closed and
        // never truncate or modify the journal target.
        let dirty_path = outcomes_dir(root).join(USAGE_DIRTY_FILE);
        symlink(&jpath, &dirty_path).expect("symlink usage.dirty");
        let err = mark_usage_dirty(root).expect_err("symlinked usage.dirty must fail");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert_eq!(fs::read(&jpath).expect("journal intact"), original);
        fs::remove_file(&dirty_path).expect("remove dirty symlink");

        // Dangling symlink at usage.dirty must still count as dirty.
        symlink(outcomes_dir(root).join("nonexistent-target"), &dirty_path)
            .expect("dangling dirty symlink");
        assert!(
            usage_dirty_exists(root),
            "dangling symlink at usage.dirty must be treated as dirty"
        );
        fs::remove_file(&dirty_path).expect("remove dangling symlink");

        // Symlinked usage.json pointing at a journal must fail closed without
        // overwriting or unlinking the target journal.
        let index_path = outcomes_dir(root).join(USAGE_INDEX_FILE);
        fs::remove_file(&index_path).expect("remove regular usage.json");
        symlink(&jpath, &index_path).expect("symlink usage.json");
        let err =
            write_usage_index_atomic(root, 1, 100).expect_err("symlinked usage.json must fail");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert_eq!(fs::read(&jpath).expect("journal intact"), original);
    }

    #[test]
    fn truncated_final_frame_keeps_dirty_marker_and_restart_counts_exact_file_length() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let registry = enabled(root);
        let id = request_id(ID);
        registry
            .reserve(&id, "note.create", "ws", &args(), &preconditions())
            .expect("reserve");
        let base = registry.snapshot_clean_usage();
        assert!(base.is_some());

        let mut journal = OutcomeJournal::open(root, &id).expect("open");
        journal.append(RecordKind::Prepared, b"").expect("prepared");
        registry.commit_journal_append(base, &journal);
        drop(journal);

        // Simulate a crash mid-write_all: dirty marker was synced before write,
        // and 7 truncated frame bytes landed at the journal tail.
        mark_usage_dirty(root).expect("mark dirty");
        let jpath = journal_path(root, ID);
        let mut raw = OpenOptions::new()
            .append(true)
            .open(&jpath)
            .expect("append");
        raw.write_all(&[0x55; 7]).expect("write partial tail");
        raw.sync_all().expect("sync partial tail");
        drop(raw);
        let actual_len = fs::metadata(&jpath).expect("metadata").len();

        let restarted = Registry::at(root);
        let report = restarted.recover_pre_effect().expect("recover");
        assert_eq!(report.corrupt, 1);
        assert!(matches!(
            restarted.observe(&id),
            Err(RegistryError::Corrupt(_))
        ));
        assert!(!outcomes_dir(root).join(USAGE_DIRTY_FILE).exists());
        assert_eq!(
            restarted.usage().expect("usage after restart"),
            (1, actual_len)
        );
    }

    #[test]
    fn registry_lock_serializes_compaction_with_active_mutation() {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };
        use std::thread;

        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().to_path_buf();
        let registry = enabled(&root);

        let id1 = request_id("30000000000000000000000000000001");
        let id2 = request_id("30000000000000000000000000000002");
        registry
            .reserve(&id1, "note.create", "ws", &args(), &preconditions())
            .expect("reserve id1");
        let base = registry.snapshot_clean_usage();
        let mut j1 = OutcomeJournal::open(&root, &id1).expect("open id1");
        j1.append(RecordKind::Prepared, b"").expect("prep");
        j1.append(RecordKind::Committed, b"").expect("commit");
        registry.commit_journal_append(base, &j1);
        drop(j1);
        age_journal(&journal_path(&root, id1.as_str()));

        // Hold the workspace registry lock as handle_mutation_registered does
        // while spawning a concurrent background compaction thread.
        let lock = registry.acquire_lock().expect("acquire lock");
        let finished = Arc::new(AtomicBool::new(false));
        let finished_bg = Arc::clone(&finished);
        let root_bg = root.clone();
        let compactor = thread::spawn(move || {
            let count = Registry::at(&root_bg)
                .compact_before(SystemTime::now())
                .expect("compact");
            finished_bg.store(true, Ordering::SeqCst);
            count
        });

        thread::sleep(Duration::from_millis(40));
        assert!(
            !finished.load(Ordering::SeqCst),
            "background compaction must block while registry lock is held"
        );

        // Commit a new reservation under the lock before compaction can run.
        assert_eq!(
            registry
                .reserve_locked(&lock, &id2, "note.create", "ws", &args(), &preconditions())
                .expect("reserve id2 under lock"),
            Reservation::Created
        );
        drop(lock);

        let compacted = compactor.join().expect("join compactor");
        assert_eq!(compacted, 1, "aged terminal id1 is compacted");
        assert!(finished.load(Ordering::SeqCst));

        // Reopened registry must observe exact post-compaction + post-id2 usage
        // cleanly from usage.json (compaction must not overwrite id2 with stale 0).
        let reopened = Registry::at(&root);
        let indexed = reopened.usage().expect("usage after serialized compaction");
        assert_eq!(reopened.scan_count(), 0);
        let scanned = reopened.scan_usage().expect("ground-truth scan");
        assert_eq!(indexed, scanned);
        assert_eq!(indexed.0, 1, "id2 remains counted after id1 is compacted");
    }
}
