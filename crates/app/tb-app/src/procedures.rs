//! Granted Procedures, Independent Postcondition Verification, and `RunReceipt`
//! (`D09`/`D10`, `U11`/`R15`/`AE15`, clean-room MIT).
//!
//! Core invariant (`SEMANTIC-ACCEPTANCE.md` AE15):
//! `OperationOutcome ≠ Postcondition`.
//! - Source-injected text NEVER confers grants: `required_grants` are checked strictly
//!   against caller-held launch grants (`held_grants`).
//! - `dry_run: true` executes ZERO mutations and returns `operation_outcome = "preview"`,
//!   `postcondition_outcome = "unverified"`, `verified_success = false`.
//! - After committed steps, every [`Postcondition`] is independently verified against disk
//!   (`store.read_exact`) and a reloaded snapshot (`ZeroDiagnostics` / `LinkResolves`).
//! - If all steps commit cleanly but any postcondition fails, `operation_outcome == "committed"`,
//!   `postcondition_outcome == "failed"`, and `verified_success == false`.

use serde::{Deserialize, Serialize};
use tb_contracts::{Locator, RevisionDigest, TaskPacket};

use crate::{App, AppError, LinkResolution, WorkspaceStore};

pub const PROCEDURE_SCHEMA_VERSION: &str = "2brain.procedure/v1";
pub const RUN_RECEIPT_SCHEMA_VERSION: &str = "2brain.run-receipt/v1";

/// Repository access restrictions enforced atomically inside [`TaskStorePort::checkpoint_task`].
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TaskScopeGuard {
    pub allowed_repos: Vec<String>,
    pub revoked_repos: Vec<String>,
}

impl TaskScopeGuard {
    #[must_use]
    pub fn has_restrictions(&self) -> bool {
        !self.allowed_repos.is_empty() || !self.revoked_repos.is_empty()
    }

    #[must_use]
    pub fn is_repo_revoked(&self, repo: &str) -> bool {
        if self.revoked_repos.iter().any(|r| r == repo) {
            return true;
        }
        !self.allowed_repos.is_empty() && !self.allowed_repos.iter().any(|r| r == repo)
    }

    pub fn check_scope(&self, scope_repos: &[String], label: &str) -> Result<(), AppError> {
        if !self.has_restrictions() {
            return Ok(());
        }
        if scope_repos.is_empty() {
            return Err(AppError::InvalidArguments(format!(
                "policy/revoked: {label} task packet has empty scope_repos while repository restrictions are active"
            )));
        }
        for repo in scope_repos {
            if self.is_repo_revoked(repo) {
                return Err(AppError::InvalidArguments(format!(
                    "policy/revoked: {label} task packet references revoked repository '{repo}'"
                )));
            }
        }
        Ok(())
    }
}

/// Driven port for durable [`TaskPacket`] persistence (`D08`/`U10`/`R17`).
pub trait TaskStorePort: Send + Sync {
    fn create_task(&self, packet: TaskPacket) -> Result<TaskPacket, AppError>;
    fn load_task(&self, task_id: &str) -> Result<TaskPacket, AppError>;
    fn list_tasks(&self) -> Result<Vec<TaskPacket>, AppError>;
    fn checkpoint_task(
        &self,
        task_id: &str,
        expected_revision: u64,
        next: TaskPacket,
        allow_drop_critical: bool,
        scope_guard: Option<&TaskScopeGuard>,
    ) -> Result<TaskPacket, AppError>;
}

/// Declarative granted procedure (`D09`).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProcedureSpec {
    pub schema_version: String,
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub required_grants: Vec<String>,
    #[serde(default)]
    pub dry_run: bool,
    pub steps: Vec<ProcedureStep>,
    #[serde(default)]
    pub postconditions: Vec<Postcondition>,
}

/// Single step inside a [`ProcedureSpec`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ProcedureStep {
    CreateFile {
        repo: String,
        path: String,
        content: String,
        #[serde(default)]
        request_id: Option<String>,
    },
    WriteFile {
        repo: String,
        path: String,
        expected_hash: String,
        content: String,
        #[serde(default)]
        request_id: Option<String>,
    },
    EditFile {
        repo: String,
        path: String,
        expected_hash: String,
        old_str: String,
        new_str: String,
        #[serde(default)]
        request_id: Option<String>,
    },
}

/// Independent post-execution assertion verified directly against disk/snapshot (`R15`/`AE15`).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Postcondition {
    FileExists {
        repo: String,
        path: String,
    },
    ContainsText {
        repo: String,
        path: String,
        needle: String,
    },
    NotContainsText {
        repo: String,
        path: String,
        needle: String,
    },
    Sha256Matches {
        repo: String,
        path: String,
        expected_sha256: String,
    },
    ZeroDiagnostics {
        repo: String,
        path: String,
    },
    LinkResolves {
        repo: String,
        path: String,
        target: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OperationStepResult {
    pub index: usize,
    pub op: String,
    pub target: String,
    pub status: String,
    pub commit: Option<String>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PostconditionResult {
    pub index: usize,
    pub kind: String,
    pub target: String,
    pub status: String,
    pub detail: String,
}

/// Immutable receipt separating operation outcome from independent postcondition verification (`D10`).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RunReceipt {
    pub schema_version: String,
    pub procedure_id: String,
    pub dry_run: bool,
    /// `"committed" | "preview" | "failed"`
    pub operation_outcome: String,
    /// `"verified" | "failed" | "unverified"`
    pub postcondition_outcome: String,
    /// `true` iff `!dry_run && operation_outcome == "committed" && postcondition_outcome == "verified"`.
    pub verified_success: bool,
    pub steps: Vec<OperationStepResult>,
    pub postconditions: Vec<PostconditionResult>,
    pub receipt_sha256: String,
}

impl ProcedureSpec {
    pub fn validate(&self) -> Result<(), AppError> {
        if self.schema_version != PROCEDURE_SCHEMA_VERSION {
            return Err(AppError::InvalidArguments(format!(
                "unsupported procedure schema_version '{}' (expected '{PROCEDURE_SCHEMA_VERSION}')",
                self.schema_version
            )));
        }
        if self.id.trim().is_empty() {
            return Err(AppError::InvalidArguments(
                "procedure 'id' must be non-empty".into(),
            ));
        }
        if self.steps.is_empty() {
            return Err(AppError::InvalidArguments(
                "procedure must contain at least one step".into(),
            ));
        }
        Ok(())
    }
}

impl<W: WorkspaceStore + 'static> App<W> {
    /// Execute a granted [`ProcedureSpec`], strictly separating operation outcome from
    /// independently observed [`Postcondition`]s (`D09`/`D10`/`R15`/`AE15`).
    pub fn run_procedure(
        &self,
        spec: &ProcedureSpec,
        held_grants: &[&str],
    ) -> Result<RunReceipt, AppError> {
        spec.validate()?;

        // 1. Fail-closed launch/authority grant check (never conferred by note content).
        for req in &spec.required_grants {
            if !held_grants.iter().any(|g| g.eq_ignore_ascii_case(req)) {
                return Err(AppError::InvalidArguments(format!(
                    "policy/launch_grant: procedure '{}' requires missing grant '{req}'",
                    spec.id
                )));
            }
        }

        // 2. Preview mode makes ZERO mutations and leaves postconditions explicitly unverified.
        if spec.dry_run {
            let step_results: Vec<OperationStepResult> = spec
                .steps
                .iter()
                .enumerate()
                .map(|(i, s)| {
                    let (op, target) = step_label(s);
                    OperationStepResult {
                        index: i,
                        op: op.to_owned(),
                        target,
                        status: "preview".to_owned(),
                        commit: None,
                        error: None,
                    }
                })
                .collect();
            let post_results: Vec<PostconditionResult> = spec
                .postconditions
                .iter()
                .enumerate()
                .map(|(i, p)| {
                    let (kind, target) = postcondition_label(p);
                    PostconditionResult {
                        index: i,
                        kind: kind.to_owned(),
                        target,
                        status: "unverified".to_owned(),
                        detail: "dry_run preview did not execute mutations".to_owned(),
                    }
                })
                .collect();
            return Ok(finalize_receipt(
                &spec.id,
                true,
                "preview",
                "unverified",
                step_results,
                post_results,
            ));
        }

        // 3. Execute steps in order; stop on first failure.
        let mut step_results = Vec::with_capacity(spec.steps.len());
        let mut op_outcome = "committed";
        for (idx, step) in spec.steps.iter().enumerate() {
            let (op, target) = step_label(step);
            let res = match step {
                ProcedureStep::CreateFile {
                    repo,
                    path,
                    content,
                    request_id,
                } => Locator::new(repo, path)
                    .map_err(|e| AppError::InvalidArguments(e.to_string()))
                    .and_then(|loc| self.create_file(&loc, content, request_id.as_deref())),
                ProcedureStep::WriteFile {
                    repo,
                    path,
                    expected_hash,
                    content,
                    request_id,
                } => Locator::new(repo, path)
                    .map_err(|e| AppError::InvalidArguments(e.to_string()))
                    .and_then(|loc| {
                        self.write_file(&loc, expected_hash, content, request_id.as_deref())
                    }),
                ProcedureStep::EditFile {
                    repo,
                    path,
                    expected_hash,
                    old_str,
                    new_str,
                    request_id,
                } => Locator::new(repo, path)
                    .map_err(|e| AppError::InvalidArguments(e.to_string()))
                    .and_then(|loc| {
                        self.edit_file(&loc, expected_hash, old_str, new_str, request_id.as_deref())
                    }),
            };
            match res {
                Ok(outcome) => {
                    let commit = outcome
                        .payload
                        .as_object()
                        .and_then(|m| m.values().next())
                        .and_then(|v| v.get("commit"))
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned);
                    step_results.push(OperationStepResult {
                        index: idx,
                        op: op.to_owned(),
                        target,
                        status: "committed".to_owned(),
                        commit,
                        error: None,
                    });
                }
                Err(err) => {
                    let is_unknown = err.is_post_effect();
                    op_outcome = if is_unknown { "unknown" } else { "failed" };
                    step_results.push(OperationStepResult {
                        index: idx,
                        op: op.to_owned(),
                        target,
                        status: if is_unknown {
                            "unknown".to_owned()
                        } else {
                            "failed".to_owned()
                        },
                        commit: None,
                        error: Some(err.to_string()),
                    });
                    break;
                }
            }
        }

        if op_outcome != "committed" {
            let skip_reason = if op_outcome == "unknown" {
                "skipped: prior step returned outcome_unknown and requires reconciliation"
            } else {
                "skipped because an earlier step failed"
            };
            let post_results: Vec<PostconditionResult> = spec
                .postconditions
                .iter()
                .enumerate()
                .map(|(i, p)| {
                    let (kind, target) = postcondition_label(p);
                    PostconditionResult {
                        index: i,
                        kind: kind.to_owned(),
                        target,
                        status: "unverified".to_owned(),
                        detail: skip_reason.to_owned(),
                    }
                })
                .collect();
            return Ok(finalize_receipt(
                &spec.id,
                false,
                op_outcome,
                "unverified",
                step_results,
                post_results,
            ));
        }

        // 4. Independent Postcondition Verification (re-reads disk & requires fresh snapshot).
        if spec.postconditions.is_empty() {
            return Ok(finalize_receipt(
                &spec.id,
                false,
                "committed",
                "unverified",
                step_results,
                Vec::new(),
            ));
        }

        let needs_snapshot = spec.postconditions.iter().any(|pc| {
            matches!(
                pc,
                Postcondition::ZeroDiagnostics { .. } | Postcondition::LinkResolves { .. }
            )
        });
        let refreshed_snap = if needs_snapshot {
            match self.store.reload_snapshot(self.generation().next()) {
                Ok(Some(fresh)) => {
                    self.swap_snapshot(fresh);
                    Ok(self.snapshot())
                }
                Ok(None) => Err("snapshot reload unsupported by store".to_owned()),
                Err(e) => Err(format!("snapshot reload failed: {e}")),
            }
        } else {
            Ok(self.snapshot())
        };

        let mut post_results = Vec::with_capacity(spec.postconditions.len());
        let mut any_failed = false;
        let mut any_unverified = false;
        for (idx, pc) in spec.postconditions.iter().enumerate() {
            let (kind, target) = postcondition_label(pc);
            let (status, detail): (&str, String) = match pc {
                Postcondition::FileExists { repo, path } => match Locator::new(repo, path) {
                    Ok(loc) => match self.store.read_exact(&loc) {
                        Ok((_, rec)) => (
                            "verified",
                            format!("exists (sha256={})", rec.sha256.to_hex()),
                        ),
                        Err(e) => ("failed", format!("missing or unreadable: {e}")),
                    },
                    Err(e) => ("failed", e.to_string()),
                },
                Postcondition::ContainsText { repo, path, needle } => {
                    match Locator::new(repo, path) {
                        Ok(loc) => match self.store.read_exact(&loc) {
                            Ok((bytes, _)) => match std::str::from_utf8(&bytes) {
                                Ok(txt) if txt.contains(needle.as_str()) => (
                                    "verified",
                                    format!("found required substring ({} chars)", needle.len()),
                                ),
                                Ok(_) => {
                                    ("failed", format!("required substring '{needle}' not found"))
                                }
                                Err(_) => ("failed", "file is not valid UTF-8".to_owned()),
                            },
                            Err(e) => ("failed", format!("read failed: {e}")),
                        },
                        Err(e) => ("failed", e.to_string()),
                    }
                }
                Postcondition::NotContainsText { repo, path, needle } => {
                    match Locator::new(repo, path) {
                        Ok(loc) => match self.store.read_exact(&loc) {
                            Ok((bytes, _)) => match std::str::from_utf8(&bytes) {
                                Ok(txt) if !txt.contains(needle.as_str()) => {
                                    ("verified", "forbidden substring absent".to_owned())
                                }
                                Ok(_) => (
                                    "failed",
                                    format!("forbidden substring '{needle}' still present"),
                                ),
                                Err(_) => ("failed", "file is not valid UTF-8".to_owned()),
                            },
                            Err(e) => ("failed", format!("read failed: {e}")),
                        },
                        Err(e) => ("failed", e.to_string()),
                    }
                }
                Postcondition::Sha256Matches {
                    repo,
                    path,
                    expected_sha256,
                } => match Locator::new(repo, path) {
                    Ok(loc) => match self.store.read_exact(&loc) {
                        Ok((_, rec)) => {
                            let actual = rec.sha256.to_hex();
                            if actual.eq_ignore_ascii_case(expected_sha256) {
                                ("verified", format!("sha256 matched {actual}"))
                            } else {
                                (
                                    "failed",
                                    format!(
                                        "sha256 mismatch: expected={expected_sha256} actual={actual}"
                                    ),
                                )
                            }
                        }
                        Err(e) => ("failed", format!("read failed: {e}")),
                    },
                    Err(e) => ("failed", e.to_string()),
                },
                Postcondition::ZeroDiagnostics { repo, path } => match &refreshed_snap {
                    Err(reason) => ("unverified", reason.clone()),
                    Ok(snap) => match Locator::new(repo, path) {
                        Err(e) => ("failed", e.to_string()),
                        Ok(loc) => {
                            if !snap.files.contains_key(&loc) {
                                (
                                    "failed",
                                    format!(
                                        "file '{}/{}' does not exist in refreshed snapshot",
                                        loc.repo, loc.path
                                    ),
                                )
                            } else {
                                let count = snap
                                    .diagnostics
                                    .iter()
                                    .filter(|d| d.repo == *repo && d.path == *path)
                                    .count();
                                if count == 0 {
                                    (
                                        "verified",
                                        "0 schema diagnostics on existing file".to_owned(),
                                    )
                                } else {
                                    (
                                        "failed",
                                        format!("found {count} schema diagnostic(s) on file"),
                                    )
                                }
                            }
                        }
                    },
                },
                Postcondition::LinkResolves { repo, path, target } => match &refreshed_snap {
                    Err(reason) => ("unverified", reason.clone()),
                    Ok(snap) => match Locator::new(repo, path) {
                        Err(e) => ("failed", e.to_string()),
                        Ok(loc) => match snap.files.get(&loc) {
                            None => (
                                "failed",
                                format!(
                                    "source file '{}/{}' does not exist in refreshed snapshot",
                                    loc.repo, loc.path
                                ),
                            ),
                            Some(file) => {
                                let matched_link = file.doc.wikilinks.iter().find(|w| {
                                    let qualified = match &w.repo {
                                        Some(r) => format!("{}::{r}", w.target),
                                        None => w.target.clone(),
                                    };
                                    w.target == *target || qualified == *target
                                });
                                match matched_link {
                                    None => (
                                        "failed",
                                        format!(
                                            "source file '{}/{}' does not contain wikilink to '{target}'",
                                            loc.repo, loc.path
                                        ),
                                    ),
                                    Some(wl) => match snap.resolve_link(
                                        &loc.repo,
                                        &wl.target,
                                        wl.repo.as_deref(),
                                    ) {
                                        LinkResolution::Resolved(dest) => (
                                            "verified",
                                            format!("resolved to {}/{}", dest.repo, dest.path),
                                        ),
                                        LinkResolution::Ambiguous(c) => (
                                            "failed",
                                            format!("ambiguous link ({} candidates)", c.len()),
                                        ),
                                        LinkResolution::Unresolved => {
                                            ("failed", format!("unresolved link target '{target}'"))
                                        }
                                    },
                                }
                            }
                        },
                    },
                },
            };
            match status {
                "failed" => any_failed = true,
                "unverified" => any_unverified = true,
                _ => {}
            }
            post_results.push(PostconditionResult {
                index: idx,
                kind: kind.to_owned(),
                target,
                status: status.to_owned(),
                detail,
            });
        }

        let post_outcome = if any_failed {
            "failed"
        } else if any_unverified {
            "unverified"
        } else {
            "verified"
        };
        Ok(finalize_receipt(
            &spec.id,
            false,
            "committed",
            post_outcome,
            step_results,
            post_results,
        ))
    }
}

fn step_label(step: &ProcedureStep) -> (&'static str, String) {
    match step {
        ProcedureStep::CreateFile { repo, path, .. } => ("create_file", format!("{repo}/{path}")),
        ProcedureStep::WriteFile { repo, path, .. } => ("write_file", format!("{repo}/{path}")),
        ProcedureStep::EditFile { repo, path, .. } => ("edit_file", format!("{repo}/{path}")),
    }
}

fn postcondition_label(pc: &Postcondition) -> (&'static str, String) {
    match pc {
        Postcondition::FileExists { repo, path } => ("file_exists", format!("{repo}/{path}")),
        Postcondition::ContainsText { repo, path, .. } => {
            ("contains_text", format!("{repo}/{path}"))
        }
        Postcondition::NotContainsText { repo, path, .. } => {
            ("not_contains_text", format!("{repo}/{path}"))
        }
        Postcondition::Sha256Matches { repo, path, .. } => {
            ("sha256_matches", format!("{repo}/{path}"))
        }
        Postcondition::ZeroDiagnostics { repo, path } => {
            ("zero_diagnostics", format!("{repo}/{path}"))
        }
        Postcondition::LinkResolves { repo, path, target } => {
            ("link_resolves", format!("{repo}/{path} -> {target}"))
        }
    }
}

fn finalize_receipt(
    procedure_id: &str,
    dry_run: bool,
    operation_outcome: &str,
    postcondition_outcome: &str,
    steps: Vec<OperationStepResult>,
    postconditions: Vec<PostconditionResult>,
) -> RunReceipt {
    let verified_success =
        !dry_run && operation_outcome == "committed" && postcondition_outcome == "verified";
    let mut receipt = RunReceipt {
        schema_version: RUN_RECEIPT_SCHEMA_VERSION.to_owned(),
        procedure_id: procedure_id.to_owned(),
        dry_run,
        operation_outcome: operation_outcome.to_owned(),
        postcondition_outcome: postcondition_outcome.to_owned(),
        verified_success,
        steps,
        postconditions,
        receipt_sha256: String::new(),
    };
    let canonical = serde_json::to_vec(&receipt).unwrap_or_default();
    receipt.receipt_sha256 = RevisionDigest::of_bytes(&canonical).to_hex();
    receipt
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    use tb_contracts::{CasVerified, EvidenceReceipt, Generation, WriteGuardHash, WriteIntent};

    use super::*;
    use crate::{MutationReceipt, StoreError, VaultSnapshot};

    struct MemStore {
        files: Mutex<BTreeMap<Locator, Vec<u8>>>,
    }

    impl MemStore {
        fn new() -> Self {
            Self {
                files: Mutex::new(BTreeMap::new()),
            }
        }
    }

    impl WorkspaceStore for MemStore {
        fn commit_verified(&self, _intent: WriteIntent<CasVerified>) -> Result<String, StoreError> {
            Err(StoreError::Unsupported("memstore".into()))
        }
        fn read_exact(&self, locator: &Locator) -> Result<(Vec<u8>, EvidenceReceipt), StoreError> {
            let guard = self.files.lock().unwrap();
            let bytes = guard
                .get(locator)
                .ok_or_else(|| StoreError::NotFound(locator.path.clone()))?
                .clone();
            let rec = EvidenceReceipt {
                locator: locator.clone(),
                sha256: RevisionDigest::of_bytes(&bytes),
                write_guard: WriteGuardHash::of_bytes(&bytes),
                total_bytes: bytes.len(),
                byte_start: 0,
                byte_end: bytes.len(),
                truncated: false,
                generation: Generation(1),
            };
            Ok((bytes, rec))
        }

        fn create_file(
            &self,
            locator: &Locator,
            content: &str,
            _commit_msg: &str,
        ) -> Result<MutationReceipt, StoreError> {
            let mut guard = self.files.lock().unwrap();
            if guard.contains_key(locator) {
                return Err(StoreError::AlreadyExists(locator.path.clone()));
            }
            let bytes = content.as_bytes().to_vec();
            let sha = RevisionDigest::of_bytes(&bytes).to_hex();
            let hash = WriteGuardHash::of_bytes(&bytes).to_hex();
            guard.insert(locator.clone(), bytes);
            Ok(MutationReceipt {
                repo: locator.repo.clone(),
                path: locator.path.clone(),
                sha256: sha,
                hash,
                commit: Some("deadbeef".to_owned()),
            })
        }
    }

    #[test]
    fn procedure_enforces_grants_preview_and_separates_commit_from_postcondition() {
        let app = App::new(MemStore::new(), VaultSnapshot::empty());

        // 1. Missing required grant fails closed.
        let spec = ProcedureSpec {
            schema_version: PROCEDURE_SCHEMA_VERSION.to_owned(),
            id: "proc-1".to_owned(),
            title: "Create checklist".to_owned(),
            required_grants: vec!["mutations".to_owned(), "procedures".to_owned()],
            dry_run: false,
            steps: vec![ProcedureStep::CreateFile {
                repo: "vault".to_owned(),
                path: "notes/out.md".to_owned(),
                content: "---\ndescription: Out\n---\nActual body.\n".to_owned(),
                request_id: None,
            }],
            postconditions: vec![Postcondition::ContainsText {
                repo: "vault".to_owned(),
                path: "notes/out.md".to_owned(),
                needle: "EXPECTED_ARTIFACT_TOKEN".to_owned(),
            }],
        };
        let err = app.run_procedure(&spec, &["mutations"]).unwrap_err();
        assert!(err.to_string().contains("launch_grant"));

        // 2. Preview makes ZERO mutations and leaves postconditions unverified.
        let mut preview_spec = spec.clone();
        preview_spec.dry_run = true;
        let preview = app
            .run_procedure(&preview_spec, &["mutations", "procedures"])
            .unwrap();
        assert_eq!(preview.operation_outcome, "preview");
        assert_eq!(preview.postcondition_outcome, "unverified");
        assert!(!preview.verified_success);
        assert!(
            app.store
                .read_exact(&Locator::new("vault", "notes/out.md").unwrap())
                .is_err()
        );

        // 3. Committed step with WRONG artifact -> operation_outcome="committed"
        //    but postcondition_outcome="failed" and verified_success=false (AE15!).
        let failed_post = app
            .run_procedure(&spec, &["mutations", "procedures"])
            .unwrap();
        assert_eq!(failed_post.operation_outcome, "committed");
        assert_eq!(failed_post.postcondition_outcome, "failed");
        assert!(!failed_post.verified_success);
        assert_eq!(failed_post.receipt_sha256.len(), 64);

        // 4. Matching postcondition -> verified_success=true!
        let mut good_spec = spec;
        good_spec.steps = vec![ProcedureStep::CreateFile {
            repo: "vault".to_owned(),
            path: "notes/good.md".to_owned(),
            content: "---\ndescription: Good\n---\nContains EXPECTED_ARTIFACT_TOKEN.\n".to_owned(),
            request_id: None,
        }];
        good_spec.postconditions = vec![
            Postcondition::FileExists {
                repo: "vault".to_owned(),
                path: "notes/good.md".to_owned(),
            },
            Postcondition::ContainsText {
                repo: "vault".to_owned(),
                path: "notes/good.md".to_owned(),
                needle: "EXPECTED_ARTIFACT_TOKEN".to_owned(),
            },
        ];
        let verified = app
            .run_procedure(&good_spec, &["mutations", "procedures"])
            .unwrap();
        assert_eq!(verified.operation_outcome, "committed");
        assert_eq!(verified.postcondition_outcome, "verified");
        assert!(verified.verified_success);
    }
}
