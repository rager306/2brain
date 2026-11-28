//! Pure-domain Bounded Investigation Controller (`U8`/`R13`/`CV1`/`AE13`, 0 I/O, Miri-clean).
//!
//! Enforces finite resource budgets (`max_actions`, `max_calls`, `max_tokens`),
//! deterministic termination reasons, route-exhaustion fingerprinting invariant to
//! timestamps or query rewording, and budget-preserving resumption across handoffs.

use std::collections::{BTreeSet, HashSet};
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Explicit finite allowance for a bounded investigation episode (`CV1`).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct InvestigationBudget {
    pub max_actions: u32,
    pub max_calls: u32,
    pub max_tokens: u64,
    pub spent_actions: u32,
    pub spent_calls: u32,
    pub spent_tokens: u64,
}

impl InvestigationBudget {
    #[must_use]
    pub const fn new(max_actions: u32, max_calls: u32, max_tokens: u64) -> Self {
        Self {
            max_actions,
            max_calls,
            max_tokens,
            spent_actions: 0,
            spent_calls: 0,
            spent_tokens: 0,
        }
    }

    #[must_use]
    pub const fn remaining_actions(&self) -> u32 {
        self.max_actions.saturating_sub(self.spent_actions)
    }

    #[must_use]
    pub const fn remaining_calls(&self) -> u32 {
        self.max_calls.saturating_sub(self.spent_calls)
    }

    #[must_use]
    pub const fn remaining_tokens(&self) -> u64 {
        self.max_tokens.saturating_sub(self.spent_tokens)
    }

    #[must_use]
    pub const fn can_afford(&self, actions: u32, calls: u32, tokens: u64) -> bool {
        self.spent_actions.saturating_add(actions) <= self.max_actions
            && self.spent_calls.saturating_add(calls) <= self.max_calls
            && self.spent_tokens.saturating_add(tokens) <= self.max_tokens
    }

    #[must_use]
    pub const fn is_exhausted(&self) -> bool {
        self.remaining_actions() == 0 || self.remaining_calls() == 0 || self.remaining_tokens() == 0
    }

    pub fn consume(&mut self, actions: u32, calls: u32, tokens: u64) {
        self.spent_actions = self.spent_actions.saturating_add(actions);
        self.spent_calls = self.spent_calls.saturating_add(calls);
        self.spent_tokens = self.spent_tokens.saturating_add(tokens);
    }
}

/// Deterministic terminal stop reason (`CV1`).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum TerminationReason {
    CriterionMet,
    BudgetExhausted,
    UserCancelled,
    NoPermittedRoute,
    MissingCapability { capability: String },
    RouteExhausted { fingerprint: String },
    PendingReconciliation,
}

/// Candidate investigation step offered to the controller.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CandidateRoute {
    /// Route class (e.g. `"tb_search"`, `"tb_context"`, `"tb_content"`).
    pub route_kind: String,
    /// Canonical normalized parameters (MUST NOT contain wall-clock timestamps).
    pub route_params: String,
    /// Estimated cost to reserve before execution.
    pub estimated_calls: u32,
    pub estimated_tokens: u64,
    /// Whether the active launch/policy grants permit this route.
    pub permitted: bool,
    /// Optional capability required if not permitted.
    pub missing_capability: Option<String>,
    /// Target obligation keys this route is intended to check.
    pub targets: Vec<String>,
}

/// Recorded execution attempt preserved across resumes (`AE13`).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct InvestigationAttempt {
    pub step: u32,
    pub route_kind: String,
    pub fingerprint: String,
    pub actions_cost: u32,
    pub calls_cost: u32,
    pub tokens_cost: u64,
    pub resolved_obligations: Vec<String>,
    pub progress: bool,
    pub note: String,
}

/// Next action selected by the controller decision rule (`SEMANTIC-ACCEPTANCE.md` §2).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ControllerDecision {
    Execute {
        route: CandidateRoute,
        fingerprint: String,
    },
    Stop(TerminationReason),
}

/// Deterministic retry fingerprint bound to material dependencies only.
///
/// Timestamps and superficial query rewording are intentionally excluded so they
/// cannot reset an exhausted route (`CV1`/`AE13`).
#[must_use]
pub fn route_fingerprint(
    task_id: &str,
    criterion_revision: &str,
    scope_policy_digest: &str,
    source_generation: u64,
    route_kind: &str,
    route_params: &str,
) -> String {
    let mut h = Sha256::new();
    for field in [
        task_id,
        criterion_revision,
        scope_policy_digest,
        route_kind,
        route_params,
    ] {
        h.update((field.len() as u64).to_be_bytes());
        h.update(field.as_bytes());
    }
    h.update(source_generation.to_be_bytes());
    let digest = h.finalize();
    let mut hex = String::with_capacity(64);
    for b in digest {
        let _ = write!(hex, "{b:02x}");
    }
    hex
}

/// Pure-domain bounded investigation controller state (`U8`/`R13`/`CV1`).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct InvestigationSession {
    pub task_id: String,
    pub criterion_revision: String,
    pub scope_policy_digest: String,
    pub source_generation: u64,
    pub mandatory_obligations: BTreeSet<String>,
    pub resolved_obligations: BTreeSet<String>,
    pub budget: InvestigationBudget,
    pub exhausted_routes: BTreeSet<String>,
    pub attempts: Vec<InvestigationAttempt>,
    pub pending_reconciliation: bool,
    pub terminated: Option<TerminationReason>,
}

impl InvestigationSession {
    #[must_use]
    pub fn new(
        task_id: impl Into<String>,
        criterion_revision: impl Into<String>,
        scope_policy_digest: impl Into<String>,
        source_generation: u64,
        mandatory_obligations: impl IntoIterator<Item = String>,
        budget: InvestigationBudget,
    ) -> Self {
        Self {
            task_id: task_id.into(),
            criterion_revision: criterion_revision.into(),
            scope_policy_digest: scope_policy_digest.into(),
            source_generation,
            mandatory_obligations: mandatory_obligations.into_iter().collect(),
            resolved_obligations: BTreeSet::new(),
            budget,
            exhausted_routes: BTreeSet::new(),
            attempts: Vec::new(),
            pending_reconciliation: false,
            terminated: None,
        }
    }

    #[must_use]
    pub fn unmet_obligations(&self) -> Vec<String> {
        self.mandatory_obligations
            .difference(&self.resolved_obligations)
            .cloned()
            .collect()
    }

    #[must_use]
    pub fn is_criterion_met(&self) -> bool {
        !self.mandatory_obligations.is_empty()
            && self
                .mandatory_obligations
                .is_subset(&self.resolved_obligations)
    }

    /// Reopen exhausted routes ONLY when a material dependency changes (`CV3`/`AE13`).
    /// Returns `true` if a material change actually occurred.
    pub fn apply_material_change(
        &mut self,
        new_source_generation: u64,
        new_criterion_revision: &str,
        new_scope_policy_digest: &str,
    ) -> bool {
        let changed = new_source_generation != self.source_generation
            || new_criterion_revision != self.criterion_revision
            || new_scope_policy_digest != self.scope_policy_digest;
        if changed {
            self.source_generation = new_source_generation;
            self.criterion_revision = new_criterion_revision.to_owned();
            self.scope_policy_digest = new_scope_policy_digest.to_owned();
            self.exhausted_routes.clear();
            if matches!(
                self.terminated,
                Some(TerminationReason::RouteExhausted { .. } | TerminationReason::CriterionMet)
            ) {
                self.terminated = None;
            }
        }
        changed
    }

    /// Controller decision rule:
    /// assess obligations -> check cancellation / pending reconciliation / budget ->
    /// pick cheapest permitted, unexhausted route targeting an unmet obligation.
    #[must_use]
    pub fn decide_next(
        &mut self,
        candidates: &[CandidateRoute],
        user_cancelled: bool,
    ) -> ControllerDecision {
        // Pending reconciliation blocks ALL progress (including criterion-met claims)
        // regardless of whether the user cancelled (CV1/AE13).
        if self.pending_reconciliation {
            self.terminated = Some(TerminationReason::PendingReconciliation);
            return ControllerDecision::Stop(TerminationReason::PendingReconciliation);
        }
        if user_cancelled {
            self.terminated = Some(TerminationReason::UserCancelled);
            return ControllerDecision::Stop(TerminationReason::UserCancelled);
        }
        if self.is_criterion_met() {
            self.terminated = Some(TerminationReason::CriterionMet);
            return ControllerDecision::Stop(TerminationReason::CriterionMet);
        }
        if self.budget.is_exhausted() {
            self.terminated = Some(TerminationReason::BudgetExhausted);
            return ControllerDecision::Stop(TerminationReason::BudgetExhausted);
        }

        let unmet: HashSet<&str> = self
            .mandatory_obligations
            .difference(&self.resolved_obligations)
            .map(String::as_str)
            .collect();

        let mut missing_cap: Option<String> = None;
        let mut last_exhausted_fp: Option<String> = None;
        let mut best: Option<(&CandidateRoute, String)> = None;
        let mut affordable_found = false;

        for cand in candidates {
            // Skip routes that cannot address any remaining unmet obligation (if targets declared).
            if !cand.targets.is_empty() && !cand.targets.iter().any(|t| unmet.contains(t.as_str()))
            {
                continue;
            }
            if !cand.permitted {
                if missing_cap.is_none() {
                    missing_cap = cand.missing_capability.clone();
                }
                continue;
            }
            let fp = route_fingerprint(
                &self.task_id,
                &self.criterion_revision,
                &self.scope_policy_digest,
                self.source_generation,
                &cand.route_kind,
                &cand.route_params,
            );
            if self.exhausted_routes.contains(&fp) {
                last_exhausted_fp = Some(fp);
                continue;
            }
            if !self
                .budget
                .can_afford(1, cand.estimated_calls.max(1), cand.estimated_tokens)
            {
                continue;
            }
            affordable_found = true;
            let replace = match &best {
                None => true,
                Some((cur, _)) => {
                    (
                        cand.estimated_tokens,
                        cand.estimated_calls,
                        &cand.route_kind,
                    ) < (cur.estimated_tokens, cur.estimated_calls, &cur.route_kind)
                }
            };
            if replace {
                best = Some((cand, fp));
            }
        }

        if let Some((route, fingerprint)) = best {
            return ControllerDecision::Execute {
                route: route.clone(),
                fingerprint,
            };
        }

        let reason = if let Some(fp) = last_exhausted_fp {
            TerminationReason::RouteExhausted { fingerprint: fp }
        } else if let Some(cap) = missing_cap {
            TerminationReason::MissingCapability { capability: cap }
        } else if !affordable_found && !candidates.is_empty() {
            TerminationReason::BudgetExhausted
        } else {
            TerminationReason::NoPermittedRoute
        };
        self.terminated = Some(reason.clone());
        ControllerDecision::Stop(reason)
    }

    /// Record an executed attempt's actual consumption and newly resolved obligations.
    /// Zero-progress attempts mark their route fingerprint exhausted so retries cannot loop.
    pub fn record_attempt(
        &mut self,
        route_kind: impl Into<String>,
        fingerprint: impl Into<String>,
        actual_calls: u32,
        actual_tokens: u64,
        newly_resolved: Vec<String>,
        note: impl Into<String>,
    ) -> Option<TerminationReason> {
        let fp = fingerprint.into();
        let kind = route_kind.into();
        self.budget.consume(1, actual_calls.max(1), actual_tokens);

        let mut truly_new = Vec::new();
        for ob in newly_resolved {
            if self.mandatory_obligations.contains(&ob)
                && self.resolved_obligations.insert(ob.clone())
            {
                truly_new.push(ob);
            }
        }
        let progress = !truly_new.is_empty();
        if !progress {
            self.exhausted_routes.insert(fp.clone());
        }

        let step = (self.attempts.len() as u32).saturating_add(1);
        self.attempts.push(InvestigationAttempt {
            step,
            route_kind: kind,
            fingerprint: fp,
            actions_cost: 1,
            calls_cost: actual_calls.max(1),
            tokens_cost: actual_tokens,
            resolved_obligations: truly_new,
            progress,
            note: note.into(),
        });

        if self.is_criterion_met() {
            self.terminated = Some(TerminationReason::CriterionMet);
            return self.terminated.clone();
        }
        if self.budget.is_exhausted() {
            self.terminated = Some(TerminationReason::BudgetExhausted);
            return self.terminated.clone();
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_route(kind: &str, params: &str, tokens: u64, target: &str) -> CandidateRoute {
        CandidateRoute {
            route_kind: kind.to_owned(),
            route_params: params.to_owned(),
            estimated_calls: 1,
            estimated_tokens: tokens,
            permitted: true,
            missing_capability: None,
            targets: vec![target.to_owned()],
        }
    }

    #[test]
    fn controller_selects_cheapest_route_and_terminates_when_criterion_met() {
        let mut sess = InvestigationSession::new(
            "task-1",
            "crit/v1",
            "policy-local",
            1,
            ["premise_a".to_owned(), "exception_b".to_owned()],
            InvestigationBudget::new(4, 4, 2000),
        );
        let candidates = vec![
            sample_route("tb_context", "q=deploy", 600, "premise_a"),
            sample_route("tb_search", "q=deploy", 150, "premise_a"),
            sample_route("tb_content", "notes/exception.md", 200, "exception_b"),
        ];

        let d1 = sess.decide_next(&candidates, false);
        let ControllerDecision::Execute { route, fingerprint } = d1 else {
            panic!("expected execute, got {d1:?}");
        };
        assert_eq!(route.route_kind, "tb_search", "must pick cheapest route");
        assert!(
            sess.record_attempt(
                route.route_kind,
                fingerprint,
                1,
                120,
                vec!["premise_a".into()],
                "found rule"
            )
            .is_none()
        );

        let d2 = sess.decide_next(&candidates, false);
        let ControllerDecision::Execute { route, fingerprint } = d2 else {
            panic!("expected second execute, got {d2:?}");
        };
        assert_eq!(route.route_kind, "tb_content");
        let term = sess.record_attempt(
            route.route_kind,
            fingerprint,
            1,
            180,
            vec!["exception_b".into()],
            "found exception",
        );
        assert_eq!(term, Some(TerminationReason::CriterionMet));
        assert!(sess.unmet_obligations().is_empty());
        assert_eq!(sess.budget.spent_actions, 2);
        assert_eq!(sess.budget.spent_tokens, 300);
    }

    #[test]
    fn stagnation_locks_route_until_material_dependency_changes() {
        let mut sess = InvestigationSession::new(
            "task-2",
            "crit/v1",
            "policy-local",
            7,
            ["missing_premise".to_owned()],
            InvestigationBudget::new(5, 5, 5000),
        );
        let cands = vec![sample_route("tb_search", "q=alpha", 100, "missing_premise")];

        let ControllerDecision::Execute { route, fingerprint } = sess.decide_next(&cands, false)
        else {
            panic!("first attempt must execute");
        };
        // Zero progress -> fingerprint becomes exhausted.
        assert!(
            sess.record_attempt(route.route_kind, fingerprint, 1, 100, vec![], "no hits")
                .is_none()
        );

        // Retrying without material change stops with RouteExhausted.
        let stop = sess.decide_next(&cands, false);
        assert!(matches!(
            stop,
            ControllerDecision::Stop(TerminationReason::RouteExhausted { .. })
        ));

        // Same generation/criterion/policy cannot unlock retry.
        assert!(!sess.apply_material_change(7, "crit/v1", "policy-local"));
        assert!(matches!(
            sess.decide_next(&cands, false),
            ControllerDecision::Stop(TerminationReason::RouteExhausted { .. })
        ));

        // Material source generation increment reopens the route while preserving spent budget!
        assert!(sess.apply_material_change(8, "crit/v1", "policy-local"));
        assert_eq!(sess.budget.spent_actions, 1);
        assert!(matches!(
            sess.decide_next(&cands, false),
            ControllerDecision::Execute { .. }
        ));
    }

    #[test]
    fn resume_retains_budget_and_cancellation_preserves_pending_reconciliation() {
        let mut sess = InvestigationSession::new(
            "task-3",
            "crit/v1",
            "policy-local",
            1,
            ["check".to_owned()],
            InvestigationBudget::new(1, 1, 500),
        );
        sess.pending_reconciliation = true;
        // Pending reconciliation blocks even when user_cancelled=false AND even if criterion were met.
        assert_eq!(
            sess.decide_next(&[sample_route("tb_search", "q=x", 10, "check")], false),
            ControllerDecision::Stop(TerminationReason::PendingReconciliation)
        );
        assert_eq!(
            sess.decide_next(&[], true),
            ControllerDecision::Stop(TerminationReason::PendingReconciliation)
        );

        // Clone simulates handoff/resume retaining spent budget.
        let mut resumed = sess.clone();
        resumed.pending_reconciliation = false;
        resumed.budget.consume(1, 1, 500);
        assert_eq!(
            resumed.decide_next(&[], false),
            ControllerDecision::Stop(TerminationReason::BudgetExhausted)
        );
    }
}
