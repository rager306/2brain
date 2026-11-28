//! Pure-domain `TaskPacket` contract (`D08`, `U10`/`R17`/`AE17`, 0 I/O, Miri-clean).
//!
//! Represents durable cross-client handoff state with monotonic optimistic
//! concurrency (`revision`), critical obligations (e.g. `publish=false`),
//! monotonic spent budget accounting, rejected route fingerprints, and
//! pending ADR-0009 operation references.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::ContractError;

pub const TASK_PACKET_SCHEMA_VERSION: &str = "2brain.task-packet/v1";

#[derive(Copy, Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    #[default]
    Open,
    Paused,
    Blocked,
    Completed,
    Cancelled,
}

/// Carried constraint or obligation that MUST survive handoff (e.g. `publish=false`).
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub struct TaskObligation {
    pub key: String,
    pub value: String,
    #[serde(default = "default_true")]
    pub critical: bool,
}

const fn default_true() -> bool {
    true
}

/// Carried whole-task budget counters (`R9`/`CV1`/`AE17`).
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct TaskBudget {
    pub max_actions: u32,
    pub spent_actions: u32,
    pub max_tokens: u64,
    pub spent_tokens: u64,
}

/// Link to an in-flight or unverified ADR-0009 operation that a resuming client must reconcile.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PendingOperationRef {
    pub request_id: String,
    pub verb: String,
    pub target: String,
    pub state: String,
}

/// Durable cross-client task handoff packet (`D08`).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TaskPacket {
    pub schema_version: String,
    pub task_id: String,
    pub revision: u64,
    pub title: String,
    #[serde(default)]
    pub status: TaskStatus,
    #[serde(default)]
    pub criterion_revision: String,
    #[serde(default)]
    pub scope_repos: Vec<String>,
    #[serde(default)]
    pub obligations: Vec<TaskObligation>,
    #[serde(default)]
    pub budget: TaskBudget,
    #[serde(default)]
    pub completed_steps: Vec<String>,
    #[serde(default)]
    pub unchecked_steps: Vec<String>,
    #[serde(default)]
    pub rejected_routes: Vec<String>,
    #[serde(default)]
    pub pending_operations: Vec<PendingOperationRef>,
    #[serde(default)]
    pub evidence_digests: BTreeMap<String, String>,
    #[serde(default)]
    pub updated_by: String,
    #[serde(default)]
    pub updated_unix: u64,
}

impl TaskPacket {
    pub fn validate_id(task_id: &str) -> Result<(), ContractError> {
        if task_id.is_empty()
            || task_id.len() > 64
            || !task_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err(ContractError::InvalidLocator(format!(
                "invalid task_id '{task_id}' (expected 1..=64 ASCII [a-zA-Z0-9_-])"
            )));
        }
        Ok(())
    }

    pub fn validate(&self) -> Result<(), ContractError> {
        Self::validate_id(&self.task_id)?;
        if self.schema_version != TASK_PACKET_SCHEMA_VERSION {
            return Err(ContractError::InvalidLocator(format!(
                "unsupported task packet schema_version '{}'",
                self.schema_version
            )));
        }
        if self.title.trim().is_empty() {
            return Err(ContractError::InvalidLocator(
                "task packet title must be non-empty".into(),
            ));
        }
        Ok(())
    }

    /// Verify that a candidate checkpoint update preserves all critical obligations
    /// and never decreases spent budget (`AE17`/`CV1`).
    pub fn validate_transition_from(
        &self,
        previous: &Self,
        allow_drop_critical: bool,
    ) -> Result<(), String> {
        if self.task_id != previous.task_id {
            return Err(format!(
                "task_id mismatch: expected '{}', got '{}'",
                previous.task_id, self.task_id
            ));
        }
        if self.budget.spent_actions < previous.budget.spent_actions {
            return Err(format!(
                "spent_actions cannot decrease across checkpoint ({} -> {})",
                previous.budget.spent_actions, self.budget.spent_actions
            ));
        }
        if self.budget.spent_tokens < previous.budget.spent_tokens {
            return Err(format!(
                "spent_tokens cannot decrease across checkpoint ({} -> {})",
                previous.budget.spent_tokens, self.budget.spent_tokens
            ));
        }
        if !allow_drop_critical {
            for prev_ob in previous.obligations.iter().filter(|o| o.critical) {
                let preserved = self.obligations.iter().any(|cur| {
                    cur.critical && cur.key == prev_ob.key && cur.value == prev_ob.value
                });
                if !preserved {
                    return Err(format!(
                        "critical obligation '{}={}' cannot be dropped silently on checkpoint",
                        prev_ob.key, prev_ob.value
                    ));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_packet() -> TaskPacket {
        TaskPacket {
            schema_version: TASK_PACKET_SCHEMA_VERSION.to_owned(),
            task_id: "release-prep_01".to_owned(),
            revision: 1,
            title: "Prepare release checklist".to_owned(),
            status: TaskStatus::Open,
            criterion_revision: "2brain.criterion/v1".to_owned(),
            scope_repos: vec!["vault".to_owned()],
            obligations: vec![TaskObligation {
                key: "publish".to_owned(),
                value: "false".to_owned(),
                critical: true,
            }],
            budget: TaskBudget {
                max_actions: 10,
                spent_actions: 2,
                max_tokens: 10_000,
                spent_tokens: 1_200,
            },
            completed_steps: vec!["read_changelog".to_owned()],
            unchecked_steps: vec!["verify_backup".to_owned()],
            rejected_routes: vec![],
            pending_operations: vec![],
            evidence_digests: BTreeMap::new(),
            updated_by: "client-a".to_owned(),
            updated_unix: 1_700_000_000,
        }
    }

    #[test]
    fn task_packet_enforces_critical_obligations_and_monotonic_budget() {
        let prev = sample_packet();
        assert!(prev.validate().is_ok());

        let mut drop_critical = prev.clone();
        drop_critical.obligations.clear();
        assert!(
            drop_critical
                .validate_transition_from(&prev, false)
                .unwrap_err()
                .contains("publish=false")
        );
        assert!(drop_critical.validate_transition_from(&prev, true).is_ok());

        let mut shrink_budget = prev.clone();
        shrink_budget.budget.spent_actions = 1;
        assert!(
            shrink_budget
                .validate_transition_from(&prev, false)
                .unwrap_err()
                .contains("spent_actions")
        );

        assert!(TaskPacket::validate_id("../escape").is_err());
    }
}
