//! Pure-domain Verified Experience Promotion (`U13`/`R18`/`AE18`/`M17`, 0 I/O, Miri-clean).
//!
//! Enforces strict scientific controls before promoting a lesson derived from an
//! episode into reusable guidance:
//! 1. **Whole-task holdout isolation:** if ANY observation for a `task_id` belongs to
//!    `source_episode_ids` or `source_families`, the ENTIRE `task_id` is excluded from
//!    holdout scoring (no partial-arm substitution).
//! 2. **Strict input integrity:** rejects duplicate `(task_id, arm)` records, conflicting
//!    `task_id -> family` assignments, reused holdout `episode_id`s, zero sample minimums,
//!    and `u128` checked token accounting.
//! 3. **Scope & authority invariance:** rejects empty `scope_label`, enforces `allowed_scopes`
//!    subset membership when configured, and rejects any `requested_grants` outside
//!    `baseline_grants`.
//! 4. **Zero held-out critical regressions:** checked across ALL holdout observations
//!    BEFORE pairing so a critical violation cannot be hidden by dropping a pair.
//! 5. **Paired holdout gain vs BOTH `NoMemory` and `RawEpisode` controls** + finite
//!    defined token overhead (zero baseline tokens can NEVER pass silently).

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

/// Experimental arm for paired experience evaluation (`AE18`).
#[derive(Copy, Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExperienceArm {
    NoMemory,
    RawEpisode,
    CandidateLesson,
}

/// One observed episode outcome under a named experimental arm.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct EpisodeObservation {
    pub episode_id: String,
    pub task_id: String,
    pub family: String,
    pub arm: ExperienceArm,
    pub fulfilled: bool,
    pub critical_violation: bool,
    pub tokens_used: u64,
}

/// Candidate lesson proposed for promotion from prior successful episode(s).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LessonCandidate {
    pub lesson_id: String,
    pub scope_label: String,
    pub source_episode_ids: Vec<String>,
    pub source_families: Vec<String>,
    #[serde(default)]
    pub requested_grants: Vec<String>,
    pub rollback_ref: String,
}

/// Preregistered promotion thresholds and trusted authority/scope baseline (`G5`/`M17`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PromotionPolicy {
    pub min_holdout_tasks: usize,
    pub min_holdout_families: usize,
    /// Minimum paired fulfillment rate gain required over BOTH `NoMemory` and `RawEpisode`.
    pub min_gain_margin: f64,
    /// Maximum allowed ratio of candidate mean tokens to `NoMemory` mean tokens (e.g. `1.25`).
    pub max_token_overhead_ratio: f64,
    /// Trusted baseline authority grants; lessons cannot request grants outside this set.
    #[serde(default)]
    pub baseline_grants: Vec<String>,
    /// Optional trusted allowed scopes; when non-empty, `lesson.scope_label` must belong to it.
    #[serde(default)]
    pub allowed_scopes: Vec<String>,
}

impl Default for PromotionPolicy {
    fn default() -> Self {
        Self {
            min_holdout_tasks: 4,
            min_holdout_families: 2,
            min_gain_margin: 0.10,
            max_token_overhead_ratio: 1.50,
            baseline_grants: Vec::new(),
            allowed_scopes: Vec::new(),
        }
    }
}

/// Machine-readable reason why promotion failed or remained inconclusive.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum PromotionReason {
    MissingRollbackRef,
    InvalidScope { detail: String },
    GrantExpansion { unauthorized_grants: Vec<String> },
    InvalidObservationSet { detail: String },
    SelfCorroborationOnly { excluded_observations: usize },
    InsufficientHoldoutTasks { observed: usize, required: usize },
    InsufficientHoldoutFamilies { observed: usize, required: usize },
    HeldOutCriticalRegression { task_ids: Vec<String> },
    InsufficientGainVsNoMemory { gain_bps: i64, required_bps: i64 },
    InsufficientGainVsRawEpisode { gain_bps: i64, required_bps: i64 },
    TokenOverheadExceeded { ratio_bps: i64, max_bps: i64 },
    UndefinedTokenOverhead { detail: String },
    InvalidPolicy { detail: String },
}

/// Final G5 promotion verdict (`U13`/`AE18`).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromotionVerdict {
    Promote,
    Reject,
    Inconclusive,
}

/// Complete, auditable evaluation report for a [`LessonCandidate`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PromotionEvaluation {
    pub schema_version: String,
    pub lesson_id: String,
    pub scope_label: String,
    pub verdict: PromotionVerdict,
    pub rollback_ref: String,
    pub excluded_training_observations: usize,
    pub paired_holdout_tasks: usize,
    pub holdout_families: Vec<String>,
    pub no_memory_rate: Option<f64>,
    pub raw_episode_rate: Option<f64>,
    pub candidate_rate: Option<f64>,
    pub gain_vs_no_memory: Option<f64>,
    pub gain_vs_raw_episode: Option<f64>,
    pub token_overhead_ratio: Option<f64>,
    pub reasons: Vec<PromotionReason>,
}

pub const PROMOTION_SCHEMA_VERSION: &str = "2brain.experience-promotion/v1";

/// Evaluate a [`LessonCandidate`] against paired holdout observations (`AE18`/`G5`).
#[must_use]
pub fn evaluate_experience_promotion(
    lesson: &LessonCandidate,
    observations: &[EpisodeObservation],
    policy: &PromotionPolicy,
) -> PromotionEvaluation {
    let mut reject_reasons: Vec<PromotionReason> = Vec::new();
    let mut inconclusive_reasons: Vec<PromotionReason> = Vec::new();

    // 0. Policy sanity: zero minimums or non-finite thresholds are rejected fail-closed.
    if policy.min_holdout_tasks == 0 || policy.min_holdout_families == 0 {
        reject_reasons.push(PromotionReason::InvalidPolicy {
            detail: "min_holdout_tasks and min_holdout_families must each be >= 1".to_owned(),
        });
    }
    if !policy.min_gain_margin.is_finite()
        || policy.min_gain_margin < 0.0
        || !policy.max_token_overhead_ratio.is_finite()
        || policy.max_token_overhead_ratio <= 0.0
    {
        reject_reasons.push(PromotionReason::InvalidPolicy {
            detail: "policy thresholds must be finite and valid".to_owned(),
        });
    }

    if lesson.rollback_ref.trim().is_empty() {
        reject_reasons.push(PromotionReason::MissingRollbackRef);
    }

    // 1. Scope & authority invariance.
    let scope_trimmed = lesson.scope_label.trim();
    if scope_trimmed.is_empty() {
        reject_reasons.push(PromotionReason::InvalidScope {
            detail: "lesson scope_label must be non-empty".to_owned(),
        });
    } else if !policy.allowed_scopes.is_empty()
        && !policy
            .allowed_scopes
            .iter()
            .any(|s| s.trim().eq_ignore_ascii_case(scope_trimmed))
    {
        reject_reasons.push(PromotionReason::InvalidScope {
            detail: format!("lesson scope '{scope_trimmed}' is outside trusted allowed_scopes"),
        });
    }

    let allowed_grants: BTreeSet<String> = policy
        .baseline_grants
        .iter()
        .map(|g| g.trim().to_ascii_lowercase())
        .collect();
    let unauthorized: Vec<String> = lesson
        .requested_grants
        .iter()
        .filter(|g| !allowed_grants.contains(&g.trim().to_ascii_lowercase()))
        .cloned()
        .collect();
    if !unauthorized.is_empty() {
        reject_reasons.push(PromotionReason::GrantExpansion {
            unauthorized_grants: unauthorized,
        });
    }

    // 2. Validate structural integrity & whole-task contamination BEFORE pairing.
    let src_episodes: BTreeSet<&str> = lesson
        .source_episode_ids
        .iter()
        .map(String::as_str)
        .collect();
    let src_families: BTreeSet<&str> = lesson.source_families.iter().map(String::as_str).collect();

    let mut task_family_map: BTreeMap<&str, &str> = BTreeMap::new();
    let mut contaminated_tasks: BTreeSet<&str> = BTreeSet::new();

    for obs in observations {
        if obs.task_id.trim().is_empty() || obs.family.trim().is_empty() {
            reject_reasons.push(PromotionReason::InvalidObservationSet {
                detail: "observation has empty task_id or family".to_owned(),
            });
            continue;
        }
        if let Some(&prev_fam) = task_family_map.get(obs.task_id.as_str()) {
            if prev_fam != obs.family.as_str() {
                reject_reasons.push(PromotionReason::InvalidObservationSet {
                    detail: format!(
                        "task '{}' assigned to conflicting families '{}' and '{}'",
                        obs.task_id, prev_fam, obs.family
                    ),
                });
            }
        } else {
            task_family_map.insert(obs.task_id.as_str(), obs.family.as_str());
        }
        if src_episodes.contains(obs.episode_id.as_str())
            || src_families.contains(obs.family.as_str())
        {
            contaminated_tasks.insert(obs.task_id.as_str());
        }
    }

    let mut excluded_training = 0usize;
    let mut seen_holdout_episodes: BTreeSet<&str> = BTreeSet::new();
    let mut by_task: BTreeMap<&str, (&str, BTreeMap<ExperienceArm, &EpisodeObservation>)> =
        BTreeMap::new();
    let mut unpaired_crit_tasks: BTreeSet<String> = BTreeSet::new();

    for obs in observations {
        if contaminated_tasks.contains(obs.task_id.as_str()) {
            excluded_training += 1;
            continue;
        }
        if obs.episode_id.trim().is_empty() {
            reject_reasons.push(PromotionReason::InvalidObservationSet {
                detail: format!("holdout task '{}' has empty episode_id", obs.task_id),
            });
            continue;
        }
        if !seen_holdout_episodes.insert(obs.episode_id.as_str()) {
            reject_reasons.push(PromotionReason::InvalidObservationSet {
                detail: format!("duplicate holdout episode_id '{}'", obs.episode_id),
            });
            continue;
        }
        if obs.arm == ExperienceArm::CandidateLesson && obs.critical_violation {
            unpaired_crit_tasks.insert(obs.task_id.clone());
        }
        let entry = by_task
            .entry(obs.task_id.as_str())
            .or_insert_with(|| (obs.family.as_str(), BTreeMap::new()));
        if entry.1.insert(obs.arm, obs).is_some() {
            reject_reasons.push(PromotionReason::InvalidObservationSet {
                detail: format!(
                    "duplicate observation for task '{}' arm {:?}",
                    obs.task_id, obs.arm
                ),
            });
        }
    }

    // Keep ONLY complete 3-arm paired tasks (NoMemory + RawEpisode + CandidateLesson).
    let mut paired_tasks: Vec<(
        &str,
        &str,
        &EpisodeObservation,
        &EpisodeObservation,
        &EpisodeObservation,
    )> = Vec::new();
    let mut holdout_families: BTreeSet<String> = BTreeSet::new();
    for (task_id, (family, arms)) in &by_task {
        if let (Some(&no_mem), Some(&raw_ep), Some(&cand)) = (
            arms.get(&ExperienceArm::NoMemory),
            arms.get(&ExperienceArm::RawEpisode),
            arms.get(&ExperienceArm::CandidateLesson),
        ) {
            paired_tasks.push((*task_id, *family, no_mem, raw_ep, cand));
            holdout_families.insert((*family).to_owned());
        }
    }

    // 3. Check held-out critical regressions across ALL candidate holdout observations
    //    (cannot be hidden by leaving an arm unpaired).
    let mut regressed_tasks: BTreeSet<String> = unpaired_crit_tasks;
    for (tid, _, no_mem, _, cand) in &paired_tasks {
        if cand.critical_violation && no_mem.critical_violation {
            // Both arms had the same baseline violation (not a new regression).
            regressed_tasks.remove(*tid);
        } else if cand.critical_violation {
            regressed_tasks.insert((*tid).to_owned());
        }
    }
    if !regressed_tasks.is_empty() {
        reject_reasons.push(PromotionReason::HeldOutCriticalRegression {
            task_ids: regressed_tasks.into_iter().collect(),
        });
    }

    let n = paired_tasks.len();
    let fam_count = holdout_families.len();

    if n == 0 && excluded_training > 0 {
        reject_reasons.push(PromotionReason::SelfCorroborationOnly {
            excluded_observations: excluded_training,
        });
    }

    let req_tasks = policy.min_holdout_tasks.max(1);
    let req_fams = policy.min_holdout_families.max(1);
    if n < req_tasks {
        inconclusive_reasons.push(PromotionReason::InsufficientHoldoutTasks {
            observed: n,
            required: req_tasks,
        });
    }
    if fam_count < req_fams {
        inconclusive_reasons.push(PromotionReason::InsufficientHoldoutFamilies {
            observed: fam_count,
            required: req_fams,
        });
    }

    // 4. Aggregate rates & checked u128 token totals (zero denominator stays None & non-promoting).
    let (
        no_memory_rate,
        raw_episode_rate,
        candidate_rate,
        gain_vs_no_memory,
        gain_vs_raw_episode,
        token_overhead_ratio,
    ) = if n > 0 {
        let nf = n as f64;
        let nm_ok = paired_tasks
            .iter()
            .filter(|(_, _, nm, _, _)| nm.fulfilled && !nm.critical_violation)
            .count() as f64;
        let raw_ok = paired_tasks
            .iter()
            .filter(|(_, _, _, raw, _)| raw.fulfilled && !raw.critical_violation)
            .count() as f64;
        let cand_ok = paired_tasks
            .iter()
            .filter(|(_, _, _, _, cand)| cand.fulfilled && !cand.critical_violation)
            .count() as f64;

        let nm_r = nm_ok / nf;
        let raw_r = raw_ok / nf;
        let cand_r = cand_ok / nf;
        let g_nm = cand_r - nm_r;
        let g_raw = cand_r - raw_r;

        let mut nm_tok: u128 = 0;
        let mut cand_tok: u128 = 0;
        let mut tok_overflow = false;
        for (_, _, nm, _, cand) in &paired_tasks {
            match nm_tok.checked_add(u128::from(nm.tokens_used)) {
                Some(v) => nm_tok = v,
                None => tok_overflow = true,
            }
            match cand_tok.checked_add(u128::from(cand.tokens_used)) {
                Some(v) => cand_tok = v,
                None => tok_overflow = true,
            }
        }

        let overhead = if tok_overflow {
            reject_reasons.push(PromotionReason::InvalidObservationSet {
                detail: "token total overflow".to_owned(),
            });
            None
        } else if nm_tok == 0 {
            if cand_tok > 0 {
                reject_reasons.push(PromotionReason::UndefinedTokenOverhead {
                    detail: "baseline NoMemory tokens totaled 0 while candidate used >0 tokens"
                        .to_owned(),
                });
            } else {
                inconclusive_reasons.push(PromotionReason::UndefinedTokenOverhead {
                    detail: "cannot evaluate token overhead ratio with 0 baseline and 0 candidate tokens"
                        .to_owned(),
                });
            }
            None
        } else {
            let ratio = (cand_tok as f64) / (nm_tok as f64);
            if !ratio.is_finite() {
                reject_reasons.push(PromotionReason::UndefinedTokenOverhead {
                    detail: "computed token overhead ratio is non-finite".to_owned(),
                });
                None
            } else {
                if ratio > policy.max_token_overhead_ratio + 1e-12 {
                    reject_reasons.push(PromotionReason::TokenOverheadExceeded {
                        ratio_bps: (ratio * 10_000.0).round() as i64,
                        max_bps: (policy.max_token_overhead_ratio * 10_000.0).round() as i64,
                    });
                }
                Some(ratio)
            }
        };

        let req_bps = (policy.min_gain_margin * 10_000.0).round() as i64;
        if g_nm + 1e-12 < policy.min_gain_margin {
            reject_reasons.push(PromotionReason::InsufficientGainVsNoMemory {
                gain_bps: (g_nm * 10_000.0).round() as i64,
                required_bps: req_bps,
            });
        }
        if g_raw + 1e-12 < policy.min_gain_margin {
            reject_reasons.push(PromotionReason::InsufficientGainVsRawEpisode {
                gain_bps: (g_raw * 10_000.0).round() as i64,
                required_bps: req_bps,
            });
        }

        (
            Some(nm_r),
            Some(raw_r),
            Some(cand_r),
            Some(g_nm),
            Some(g_raw),
            overhead,
        )
    } else {
        (None, None, None, None, None, None)
    };

    let (verdict, reasons) = if !reject_reasons.is_empty() {
        let mut all = reject_reasons;
        all.extend(inconclusive_reasons);
        (PromotionVerdict::Reject, all)
    } else if !inconclusive_reasons.is_empty() {
        (PromotionVerdict::Inconclusive, inconclusive_reasons)
    } else {
        (PromotionVerdict::Promote, Vec::new())
    };

    PromotionEvaluation {
        schema_version: PROMOTION_SCHEMA_VERSION.to_owned(),
        lesson_id: lesson.lesson_id.clone(),
        scope_label: lesson.scope_label.clone(),
        verdict,
        rollback_ref: lesson.rollback_ref.clone(),
        excluded_training_observations: excluded_training,
        paired_holdout_tasks: n,
        holdout_families: holdout_families.into_iter().collect(),
        no_memory_rate,
        raw_episode_rate,
        candidate_rate,
        gain_vs_no_memory,
        gain_vs_raw_episode,
        token_overhead_ratio,
        reasons,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obs(
        ep: &str,
        task: &str,
        fam: &str,
        arm: ExperienceArm,
        ok: bool,
        crit: bool,
        tok: u64,
    ) -> EpisodeObservation {
        EpisodeObservation {
            episode_id: ep.to_owned(),
            task_id: task.to_owned(),
            family: fam.to_owned(),
            arm,
            fulfilled: ok,
            critical_violation: crit,
            tokens_used: tok,
        }
    }

    fn base_lesson() -> LessonCandidate {
        LessonCandidate {
            lesson_id: "lesson-exception-check".to_owned(),
            scope_label: "rule-exception-reads".to_owned(),
            source_episode_ids: vec!["ep-train-1".to_owned()],
            source_families: vec!["fam-train".to_owned()],
            requested_grants: vec!["read".to_owned()],
            rollback_ref: "git:revert-lesson-01".to_owned(),
        }
    }

    fn base_policy() -> PromotionPolicy {
        PromotionPolicy {
            min_holdout_tasks: 4,
            min_holdout_families: 2,
            min_gain_margin: 0.20,
            max_token_overhead_ratio: 1.30,
            baseline_grants: vec!["read".to_owned()],
            allowed_scopes: vec!["rule-exception-reads".to_owned()],
        }
    }

    #[test]
    fn training_only_self_corroboration_is_rejected() {
        let lesson = base_lesson();
        let data = vec![
            obs(
                "ep-train-1",
                "t1",
                "fam-train",
                ExperienceArm::NoMemory,
                false,
                false,
                100,
            ),
            obs(
                "ep-train-2",
                "t1",
                "fam-train",
                ExperienceArm::RawEpisode,
                false,
                false,
                100,
            ),
            obs(
                "ep-train-3",
                "t1",
                "fam-train",
                ExperienceArm::CandidateLesson,
                true,
                false,
                100,
            ),
        ];
        let eval = evaluate_experience_promotion(&lesson, &data, &base_policy());
        assert_eq!(eval.verdict, PromotionVerdict::Reject);
        assert_eq!(eval.paired_holdout_tasks, 0);
        assert_eq!(eval.excluded_training_observations, 3);
        assert!(eval.no_memory_rate.is_none());
    }

    #[test]
    fn grant_expansion_and_held_out_exception_regression_are_rejected() {
        let mut expanded = base_lesson();
        expanded.requested_grants.push("mutations".to_owned());
        let data = vec![
            obs(
                "e1",
                "t1",
                "fam-a",
                ExperienceArm::NoMemory,
                true,
                false,
                100,
            ),
            obs(
                "e2",
                "t1",
                "fam-a",
                ExperienceArm::RawEpisode,
                true,
                false,
                100,
            ),
            obs(
                "e3",
                "t1",
                "fam-a",
                ExperienceArm::CandidateLesson,
                true,
                true,
                100,
            ),
        ];
        let eval = evaluate_experience_promotion(&expanded, &data, &base_policy());
        assert_eq!(eval.verdict, PromotionVerdict::Reject);
        assert!(
            eval.reasons
                .iter()
                .any(|r| matches!(r, PromotionReason::GrantExpansion { .. }))
        );
        assert!(
            eval.reasons
                .iter()
                .any(|r| matches!(r, PromotionReason::HeldOutCriticalRegression { .. }))
        );
    }

    #[test]
    fn duplicate_gaming_zero_minimums_and_zero_baseline_tokens_cannot_promote() {
        let lesson = base_lesson();
        // Zero minimums with empty data MUST reject.
        let mut zero_pol = base_policy();
        zero_pol.min_holdout_tasks = 0;
        zero_pol.min_holdout_families = 0;
        assert_eq!(
            evaluate_experience_promotion(&lesson, &[], &zero_pol).verdict,
            PromotionVerdict::Reject
        );

        // Duplicate arm overwriting a critical violation MUST reject.
        let dup = vec![
            obs(
                "e1",
                "t1",
                "fam-a",
                ExperienceArm::NoMemory,
                false,
                false,
                100,
            ),
            obs(
                "e2",
                "t1",
                "fam-a",
                ExperienceArm::RawEpisode,
                false,
                false,
                100,
            ),
            obs(
                "e3",
                "t1",
                "fam-a",
                ExperienceArm::CandidateLesson,
                false,
                true,
                100,
            ),
            obs(
                "e4",
                "t1",
                "fam-a",
                ExperienceArm::CandidateLesson,
                true,
                false,
                100,
            ),
        ];
        let dup_eval = evaluate_experience_promotion(&lesson, &dup, &base_policy());
        assert_eq!(dup_eval.verdict, PromotionVerdict::Reject);
        assert!(
            dup_eval
                .reasons
                .iter()
                .any(|r| matches!(r, PromotionReason::InvalidObservationSet { .. }))
        );
        assert!(
            dup_eval
                .reasons
                .iter()
                .any(|r| matches!(r, PromotionReason::HeldOutCriticalRegression { .. }))
        );

        // Zero baseline tokens with >0 candidate tokens MUST reject.
        let zero_tok = vec![
            obs(
                "e1",
                "t1",
                "fam-a",
                ExperienceArm::NoMemory,
                false,
                false,
                0,
            ),
            obs(
                "e2",
                "t1",
                "fam-a",
                ExperienceArm::RawEpisode,
                false,
                false,
                0,
            ),
            obs(
                "e3",
                "t1",
                "fam-a",
                ExperienceArm::CandidateLesson,
                true,
                false,
                500,
            ),
        ];
        let mut pol1 = base_policy();
        pol1.min_holdout_tasks = 1;
        pol1.min_holdout_families = 1;
        let z_eval = evaluate_experience_promotion(&lesson, &zero_tok, &pol1);
        assert_eq!(z_eval.verdict, PromotionVerdict::Reject);
        assert!(
            z_eval
                .reasons
                .iter()
                .any(|r| matches!(r, PromotionReason::UndefinedTokenOverhead { .. }))
        );
    }

    #[test]
    fn family_separated_holdout_gain_promotes_and_insufficient_sample_is_inconclusive() {
        let lesson = base_lesson();
        let mut data = Vec::new();
        for (idx, (tid, fam, base_ok)) in [
            ("t1", "fam-holdout-a", true),
            ("t2", "fam-holdout-a", false),
            ("t3", "fam-holdout-b", true),
            ("t4", "fam-holdout-b", false),
        ]
        .iter()
        .enumerate()
        {
            data.push(obs(
                &format!("nm-{idx}"),
                tid,
                fam,
                ExperienceArm::NoMemory,
                *base_ok,
                false,
                100,
            ));
            data.push(obs(
                &format!("rw-{idx}"),
                tid,
                fam,
                ExperienceArm::RawEpisode,
                *base_ok,
                false,
                120,
            ));
            data.push(obs(
                &format!("cd-{idx}"),
                tid,
                fam,
                ExperienceArm::CandidateLesson,
                true,
                false,
                110,
            ));
        }

        let promoted = evaluate_experience_promotion(&lesson, &data, &base_policy());
        assert_eq!(promoted.verdict, PromotionVerdict::Promote);
        assert_eq!(promoted.paired_holdout_tasks, 4);
        assert_eq!(promoted.holdout_families.len(), 2);
        assert!((promoted.gain_vs_no_memory.unwrap() - 0.5).abs() < 1e-9);
        assert!((promoted.gain_vs_raw_episode.unwrap() - 0.5).abs() < 1e-9);

        // With only 2 tasks (< min_holdout_tasks=4) and positive gain, verdict is Inconclusive.
        let small = evaluate_experience_promotion(&lesson, &data[..6], &base_policy());
        assert_eq!(small.verdict, PromotionVerdict::Inconclusive);
    }
}
