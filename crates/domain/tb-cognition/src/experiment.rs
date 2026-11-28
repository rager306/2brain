//! Pure-domain Optional Retrieval/Selection & Iterative Solver Experiments
//! (`U14`/`R19`/`R20`/`AE19`/`AE20`/`CV4`/`M23`, 0 I/O, Miri-clean).
//!
//! Enforces two strict scientific boundaries from `SEMANTIC-ACCEPTANCE.md`:
//! 1. **CV4 Numerical Convergence (`run_bounded_ppr_solver`):** Personalized PageRank
//!    over `u32` CSR graphs records objective/residual definition, tolerance, iteration cap,
//!    strict seed resolution (never silently dropping invalid seeds), origin deduplication
//!    (explicit provenance OR normalized content digest so duplicate copies without metadata
//!    cannot amplify rank), and explicit `SolverStopReason`. Output is strictly
//!    `diagnostic_only = true` — solver convergence NEVER certifies semantic truth.
//! 2. **G5 Paired Experiment Gate (`evaluate_paired_experiment`):** Compares one optional
//!    candidate arm against the simpler baseline under matched task/family budgets. Rejects
//!    duplicate task rows, zero minimums, non-finite metrics/thresholds, undefined token
//!    overhead, budget violations, and supported-answer regressions (even when ranking rises).

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use tb_contracts::{Locator, NodeId};
use tb_graph::TypedLinkGraph;
use tb_syntax::EdgeKind;

pub const EXPERIMENT_SCHEMA_VERSION: &str = "2brain.experiment-eval/v1";

/// Configuration for the bounded Personalized PageRank iterative selector (`CV4`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PprConfig {
    /// Teleport / damping factor `alpha` in `(0.0, 1.0)` (typical: `0.85`).
    pub damping: f64,
    /// L1 residual tolerance stopping criterion (`||r_{k+1} - r_k||_1 <= tolerance`).
    pub tolerance: f64,
    /// Hard iteration cap (`CV4`).
    pub max_iterations: usize,
    /// Normalize seed weights per unique origin so duplicate copies cannot amplify rank (`AE14`/`AE19`).
    pub deduplicate_origins: bool,
    /// Maximum ranked results to return.
    pub top_k: usize,
}

impl Default for PprConfig {
    fn default() -> Self {
        Self {
            damping: 0.85,
            tolerance: 1e-6,
            max_iterations: 100,
            deduplicate_origins: true,
            top_k: 20,
        }
    }
}

/// Explicit stopping reason for an iterative numerical solver (`CV4`/`M23`).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum SolverStopReason {
    Converged,
    IterationCapReached,
    NonFiniteDetected,
    InvalidConfig { detail: String },
    UnresolvedSeeds { missing: Vec<Locator> },
    EmptyGraphOrSeeds,
}

/// Ranked candidate produced by the bounded PPR solver (diagnostic only).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PprRankEntry {
    pub locator: Locator,
    pub score: f64,
    pub origin_id: String,
}

/// Complete CV4 solver trace (`M23`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PprSolverReport {
    /// Always `true`: numerical convergence is a diagnostic signal, never semantic proof (`AE20`).
    pub diagnostic_only: bool,
    pub residual_definition: String,
    pub damping: f64,
    pub tolerance: f64,
    pub max_iterations: usize,
    pub iterations_run: usize,
    pub final_residual: Option<f64>,
    pub stop_reason: SolverStopReason,
    pub unique_seed_origins: usize,
    /// Count of seed nodes that lacked explicit/derived origin mapping and fell back to locator path.
    pub unverified_seed_origins: usize,
    pub ranked: Vec<PprRankEntry>,
}

/// Run bounded Personalized PageRank over `TypedLinkGraph` with strict seed validation
/// and origin deduplication.
///
/// Every seed in `seeds` MUST exist in `graph` (unresolved seeds fail closed with
/// `SolverStopReason::UnresolvedSeeds`). `origins` should map each locator to its
/// normalized provenance origin or content-digest origin (`content_sha256:...`).
#[must_use]
pub fn run_bounded_ppr_solver(
    graph: &TypedLinkGraph,
    seeds: &[Locator],
    origins: &BTreeMap<Locator, String>,
    cfg: &PprConfig,
) -> PprSolverReport {
    let make_err = |stop_reason: SolverStopReason| PprSolverReport {
        diagnostic_only: true,
        residual_definition: "L1(r_{k+1} - r_k)".to_owned(),
        damping: cfg.damping,
        tolerance: cfg.tolerance,
        max_iterations: cfg.max_iterations,
        iterations_run: 0,
        final_residual: None,
        stop_reason,
        unique_seed_origins: 0,
        unverified_seed_origins: 0,
        ranked: Vec::new(),
    };

    if !cfg.damping.is_finite()
        || cfg.damping <= 0.0
        || cfg.damping >= 1.0
        || !cfg.tolerance.is_finite()
        || cfg.tolerance < 0.0
        || cfg.max_iterations == 0
    {
        return make_err(SolverStopReason::InvalidConfig {
            detail: "require 0 < damping < 1, finite tolerance >= 0, and max_iterations >= 1"
                .to_owned(),
        });
    }

    let n = graph.node_count();
    if n == 0 || seeds.is_empty() {
        return make_err(SolverStopReason::EmptyGraphOrSeeds);
    }

    // Fail-closed: never silently drop unresolved seed locators.
    let mut missing: Vec<Locator> = Vec::new();
    let mut seed_ids: Vec<(usize, String)> = Vec::new();
    let mut seen_nodes = BTreeSet::new();
    let mut unverified_seed_origins = 0usize;

    for loc in seeds {
        let Some(nid) = graph.node_id(loc) else {
            missing.push(loc.clone());
            continue;
        };
        let idx = nid.0 as usize;
        if idx >= n {
            missing.push(loc.clone());
            continue;
        }
        if seen_nodes.insert(idx) {
            let origin = match origins.get(loc).map(|s| s.trim()).filter(|s| !s.is_empty()) {
                Some(o) => o.to_ascii_lowercase(),
                None => {
                    unverified_seed_origins += 1;
                    format!("unverified:{}/{}", loc.repo, loc.path)
                }
            };
            seed_ids.push((idx, origin));
        }
    }
    if !missing.is_empty() {
        missing.sort();
        missing.dedup();
        return make_err(SolverStopReason::UnresolvedSeeds { missing });
    }
    if seed_ids.is_empty() {
        return make_err(SolverStopReason::EmptyGraphOrSeeds);
    }

    let mut origin_counts: BTreeMap<&str, usize> = BTreeMap::new();
    for (_, orig) in &seed_ids {
        *origin_counts.entry(orig.as_str()).or_insert(0) += 1;
    }
    let unique_origins = origin_counts.len();

    let mut teleport = vec![0.0f64; n];
    if cfg.deduplicate_origins {
        let origin_share = 1.0 / (unique_origins as f64);
        for (idx, orig) in &seed_ids {
            let copies = origin_counts[orig.as_str()] as f64;
            teleport[*idx] += origin_share / copies;
        }
    } else {
        let node_share = 1.0 / (seed_ids.len() as f64);
        for (idx, _) in &seed_ids {
            teleport[*idx] += node_share;
        }
    }

    // Build undirected adjacency with deterministic degree normalization for CSR traversal.
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (u, adj_u) in adj.iter_mut().enumerate().take(n) {
        let nid = NodeId(u as u32);
        let (outs, _) = graph.out_neighbors(nid);
        let (ins, _) = graph.in_neighbors(nid);
        let mut nbrs = Vec::with_capacity(outs.len() + ins.len());
        for &v in outs {
            let vi = v.0 as usize;
            if vi < n && vi != u {
                nbrs.push(vi);
            }
        }
        for &v in ins {
            let vi = v.0 as usize;
            if vi < n && vi != u {
                nbrs.push(vi);
            }
        }
        nbrs.sort_unstable();
        nbrs.dedup();
        *adj_u = nbrs;
    }

    let mut rank = teleport.clone();
    let mut next = vec![0.0f64; n];
    let mut iterations_run = 0usize;
    let mut last_residual = f64::INFINITY;
    let mut stop_reason = SolverStopReason::IterationCapReached;

    for iter in 1..=cfg.max_iterations {
        iterations_run = iter;
        next.fill(0.0);
        let mut dangling_mass = 0.0f64;

        for (u, &ru) in rank.iter().enumerate() {
            if !ru.is_finite() {
                return PprSolverReport {
                    diagnostic_only: true,
                    residual_definition: "L1(r_{k+1} - r_k)".to_owned(),
                    damping: cfg.damping,
                    tolerance: cfg.tolerance,
                    max_iterations: cfg.max_iterations,
                    iterations_run: iter,
                    final_residual: None,
                    stop_reason: SolverStopReason::NonFiniteDetected,
                    unique_seed_origins: unique_origins,
                    unverified_seed_origins,
                    ranked: Vec::new(),
                };
            }
            if adj[u].is_empty() {
                dangling_mass += ru;
            } else {
                let share = cfg.damping * ru / (adj[u].len() as f64);
                for &v in &adj[u] {
                    next[v] += share;
                }
            }
        }

        let teleport_weight = (1.0 - cfg.damping) + cfg.damping * dangling_mass;
        let mut l1 = 0.0f64;
        for i in 0..n {
            next[i] += teleport_weight * teleport[i];
            if !next[i].is_finite() {
                return PprSolverReport {
                    diagnostic_only: true,
                    residual_definition: "L1(r_{k+1} - r_k)".to_owned(),
                    damping: cfg.damping,
                    tolerance: cfg.tolerance,
                    max_iterations: cfg.max_iterations,
                    iterations_run: iter,
                    final_residual: None,
                    stop_reason: SolverStopReason::NonFiniteDetected,
                    unique_seed_origins: unique_origins,
                    unverified_seed_origins,
                    ranked: Vec::new(),
                };
            }
            l1 += (next[i] - rank[i]).abs();
        }
        std::mem::swap(&mut rank, &mut next);
        last_residual = l1;

        if l1 <= cfg.tolerance {
            stop_reason = SolverStopReason::Converged;
            break;
        }
    }

    let mut scored: Vec<PprRankEntry> = (0..n)
        .filter_map(|i| {
            let loc = graph.locator(NodeId(i as u32))?.clone();
            let origin_id = origins
                .get(&loc)
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .map(str::to_ascii_lowercase)
                .unwrap_or_else(|| format!("unverified:{}/{}", loc.repo, loc.path));
            Some(PprRankEntry {
                locator: loc,
                score: rank[i],
                origin_id,
            })
        })
        .collect();
    scored.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.locator.cmp(&b.locator))
    });
    scored.truncate(cfg.top_k);

    PprSolverReport {
        diagnostic_only: true,
        residual_definition: "L1(r_{k+1} - r_k)".to_owned(),
        damping: cfg.damping,
        tolerance: cfg.tolerance,
        max_iterations: cfg.max_iterations,
        iterations_run,
        final_residual: Some(last_residual),
        stop_reason,
        unique_seed_origins: unique_origins,
        unverified_seed_origins,
        ranked: scored,
    }
}

/// One paired baseline-vs-candidate observation under matched task & budget (`AE19`/`AE20`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PairedTaskOutcome {
    pub task_id: String,
    pub family: String,
    pub budget_cap_tokens: u64,
    pub baseline_fulfilled: bool,
    pub candidate_fulfilled: bool,
    pub baseline_critical_violation: bool,
    pub candidate_critical_violation: bool,
    /// Candidate retrieval/ranking metric (e.g. recall/nDCG) reported SEPARATELY from task answer!
    pub baseline_rank_score: f64,
    pub candidate_rank_score: f64,
    pub baseline_tokens: u64,
    pub candidate_tokens: u64,
}

/// Preregistered experiment promotion policy (`G5`/`AE19`/`AE20`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExperimentPolicy {
    pub experiment_id: String,
    pub arm_name: String,
    pub rollback_ref: String,
    pub min_tasks: usize,
    pub min_families: usize,
    /// Minimum paired task-fulfillment gain over baseline.
    pub min_utility_gain: f64,
    /// Maximum allowed token overhead ratio (`candidate_tokens / baseline_tokens`).
    pub max_token_overhead_ratio: f64,
    /// Optional null-model Z-score floor when evaluating community/graph discovery (`AE20`).
    pub min_null_z_score: Option<f64>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExperimentVerdict {
    PromoteCandidate,
    RetainBaseline,
    Inconclusive,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExperimentEvaluation {
    pub schema_version: String,
    pub experiment_id: String,
    pub arm_name: String,
    pub rollback_ref: String,
    pub verdict: ExperimentVerdict,
    pub task_count: usize,
    pub families: Vec<String>,
    pub baseline_fulfillment_rate: Option<f64>,
    pub candidate_fulfillment_rate: Option<f64>,
    pub utility_gain: Option<f64>,
    pub baseline_mean_rank: Option<f64>,
    pub candidate_mean_rank: Option<f64>,
    pub token_overhead_ratio: Option<f64>,
    pub null_z_score: Option<f64>,
    pub notes: Vec<String>,
}

/// Evaluate a paired optional-mechanism experiment against the simpler baseline (`AE19`/`AE20`/`G5`).
///
/// Ranking improvement NEVER overrides supported-answer regression or critical violations.
#[must_use]
pub fn evaluate_paired_experiment(
    policy: &ExperimentPolicy,
    outcomes: &[PairedTaskOutcome],
    null_z_score: Option<f64>,
) -> ExperimentEvaluation {
    let mut reject_notes = Vec::new();
    let mut inconclusive_notes = Vec::new();

    if policy.rollback_ref.trim().is_empty() {
        reject_notes.push("missing rollback_ref for candidate arm".to_owned());
    }
    if policy.min_tasks == 0 || policy.min_families == 0 {
        reject_notes.push("min_tasks and min_families must each be >= 1".to_owned());
    }
    if !policy.min_utility_gain.is_finite()
        || policy.min_utility_gain < 0.0
        || !policy.max_token_overhead_ratio.is_finite()
        || policy.max_token_overhead_ratio <= 0.0
    {
        reject_notes.push("invalid non-finite experiment policy thresholds".to_owned());
    }
    if let Some(min_z) = policy.min_null_z_score
        && !min_z.is_finite()
    {
        reject_notes.push("min_null_z_score must be finite".to_owned());
    }
    if let Some(z) = null_z_score
        && !z.is_finite()
    {
        reject_notes.push("observed null_z_score must be finite".to_owned());
    }

    // Reject duplicate task_ids, empty identifiers, or non-finite rank scores before aggregating.
    let mut seen_tasks: BTreeSet<&str> = BTreeSet::new();
    let mut valid_outcomes: Vec<&PairedTaskOutcome> = Vec::with_capacity(outcomes.len());
    let mut families: BTreeSet<String> = BTreeSet::new();
    for o in outcomes {
        if o.task_id.trim().is_empty() || o.family.trim().is_empty() {
            reject_notes.push("paired outcome has empty task_id or family".to_owned());
            continue;
        }
        if !seen_tasks.insert(o.task_id.as_str()) {
            reject_notes.push(format!("duplicate paired task_id '{}'", o.task_id));
            continue;
        }
        if !o.baseline_rank_score.is_finite() || !o.candidate_rank_score.is_finite() {
            reject_notes.push(format!("non-finite rank score in task '{}'", o.task_id));
            continue;
        }
        if o.candidate_critical_violation && !o.baseline_critical_violation {
            reject_notes.push(format!(
                "critical violation introduced by candidate arm on task '{}'",
                o.task_id
            ));
        }
        if o.budget_cap_tokens > 0 && o.candidate_tokens > o.budget_cap_tokens {
            reject_notes.push(format!(
                "candidate exceeded budget cap on task '{}' ({} > {})",
                o.task_id, o.candidate_tokens, o.budget_cap_tokens
            ));
        }
        families.insert(o.family.clone());
        valid_outcomes.push(o);
    }

    let n = valid_outcomes.len();
    let req_tasks = policy.min_tasks.max(1);
    let req_fams = policy.min_families.max(1);

    if n < req_tasks {
        inconclusive_notes.push(format!(
            "insufficient paired tasks: observed={n}, required={req_tasks}"
        ));
    }
    if families.len() < req_fams {
        inconclusive_notes.push(format!(
            "insufficient independent families: observed={}, required={req_fams}",
            families.len()
        ));
    }

    if let Some(min_z) = policy.min_null_z_score
        && min_z.is_finite()
    {
        match null_z_score {
            Some(z) if z.is_finite() && z >= min_z => {}
            Some(z) if z.is_finite() => reject_notes.push(format!(
                "null-graph control failed: Z={z:.2} < required {min_z:.2}"
            )),
            Some(_) => {} // already rejected above as non-finite
            None => inconclusive_notes
                .push("required null-graph control Z-score was not measured".to_owned()),
        }
    }

    let (base_rate, cand_rate, utility_gain, base_rank, cand_rank, overhead) = if n > 0 {
        let nf = n as f64;
        let b_ok = valid_outcomes
            .iter()
            .filter(|o| o.baseline_fulfilled && !o.baseline_critical_violation)
            .count() as f64;
        let c_ok = valid_outcomes
            .iter()
            .filter(|o| o.candidate_fulfilled && !o.candidate_critical_violation)
            .count() as f64;
        let br = b_ok / nf;
        let cr = c_ok / nf;
        let ug = cr - br;

        let b_rk = valid_outcomes
            .iter()
            .map(|o| o.baseline_rank_score)
            .sum::<f64>()
            / nf;
        let c_rk = valid_outcomes
            .iter()
            .map(|o| o.candidate_rank_score)
            .sum::<f64>()
            / nf;
        if !b_rk.is_finite() || !c_rk.is_finite() {
            reject_notes.push("aggregated rank mean is non-finite".to_owned());
        }

        let mut b_tok: u128 = 0;
        let mut c_tok: u128 = 0;
        let mut tok_overflow = false;
        for o in &valid_outcomes {
            match b_tok.checked_add(u128::from(o.baseline_tokens)) {
                Some(v) => b_tok = v,
                None => tok_overflow = true,
            }
            match c_tok.checked_add(u128::from(o.candidate_tokens)) {
                Some(v) => c_tok = v,
                None => tok_overflow = true,
            }
        }

        let ov = if tok_overflow {
            reject_notes.push("token total overflow".to_owned());
            None
        } else if b_tok == 0 {
            if c_tok > 0 {
                reject_notes.push(
                    "undefined token overhead: baseline totaled 0 tokens while candidate used >0"
                        .to_owned(),
                );
            } else {
                inconclusive_notes.push(
                    "undefined token overhead: both baseline and candidate totaled 0 tokens"
                        .to_owned(),
                );
            }
            None
        } else {
            let ratio = (c_tok as f64) / (b_tok as f64);
            if !ratio.is_finite() {
                reject_notes.push("computed token overhead ratio is non-finite".to_owned());
                None
            } else {
                if ratio > policy.max_token_overhead_ratio + 1e-12 {
                    reject_notes.push(format!(
                        "token overhead ratio {ratio:.3} exceeds max {:.3}",
                        policy.max_token_overhead_ratio
                    ));
                }
                Some(ratio)
            }
        };

        if cr < br {
            reject_notes.push(format!(
                "candidate regressed supported answer fulfillment ({cr:.3} < {br:.3}; rank delta={:+.3} cannot override answer regression)",
                c_rk - b_rk
            ));
        } else if ug + 1e-12 < policy.min_utility_gain {
            reject_notes.push(format!(
                "utility gain {ug:.3} below preregistered margin {:.3}; retain simpler baseline",
                policy.min_utility_gain
            ));
        }
        (
            Some(br),
            Some(cr),
            Some(ug),
            b_rk.is_finite().then_some(b_rk),
            c_rk.is_finite().then_some(c_rk),
            ov,
        )
    } else {
        (None, None, None, None, None, None)
    };

    let (verdict, notes) = if !reject_notes.is_empty() {
        reject_notes.extend(inconclusive_notes);
        (ExperimentVerdict::RetainBaseline, reject_notes)
    } else if !inconclusive_notes.is_empty() {
        (ExperimentVerdict::Inconclusive, inconclusive_notes)
    } else {
        (ExperimentVerdict::PromoteCandidate, Vec::new())
    };

    ExperimentEvaluation {
        schema_version: EXPERIMENT_SCHEMA_VERSION.to_owned(),
        experiment_id: policy.experiment_id.clone(),
        arm_name: policy.arm_name.clone(),
        rollback_ref: policy.rollback_ref.clone(),
        verdict,
        task_count: n,
        families: families.into_iter().collect(),
        baseline_fulfillment_rate: base_rate,
        candidate_fulfillment_rate: cand_rate,
        utility_gain,
        baseline_mean_rank: base_rank,
        candidate_mean_rank: cand_rank,
        token_overhead_ratio: overhead,
        null_z_score,
        notes,
    }
}

/// Compute Nadaraya-Watson kernel weights `w_i = softmax(s_i / tau_iqr)` with robust
/// interquartile-range bandwidth `tau = max(IQR(scores) / 1.35, 1e-6)` (CPU-Memorilla++).
///
/// Non-finite scores are clamped to `0.0`. Always returns a valid probability distribution
/// summing to `1.0` (or empty vector for empty input).
#[must_use]
pub fn kernel_weights_iqr(scores: &[f64]) -> (Vec<f64>, f64) {
    if scores.is_empty() {
        return (Vec::new(), 1.0);
    }
    let clean: Vec<f64> = scores
        .iter()
        .map(|&s| if s.is_finite() { s } else { 0.0 })
        .collect();
    if clean.len() == 1 {
        return (vec![1.0], 1.0);
    }

    let mut sorted = clean.clone();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let q25 = sorted[sorted.len() / 4];
    let q75 = sorted[(sorted.len() * 3) / 4];
    // Compute differences via halved values to avoid f64 overflow on extreme finite inputs.
    let iqr = ((q75 * 0.5) - (q25 * 0.5)).abs() * 2.0;
    let max_s = *sorted.last().unwrap_or(&0.0);
    let min_s = *sorted.first().unwrap_or(&0.0);
    let full_range = ((max_s * 0.5) - (min_s * 0.5)).abs() * 2.0;
    let spread = if iqr.is_finite() && iqr > 1e-9 {
        iqr / 1.35
    } else if full_range.is_finite() && full_range > 1e-9 {
        full_range / 2.0
    } else {
        1.0
    };
    let tau = if spread.is_finite() {
        spread.max(1e-6)
    } else {
        1.0
    };

    let mut weights = Vec::with_capacity(clean.len());
    let mut sum = 0.0f64;
    for &s in &clean {
        let diff = ((s * 0.5) - (max_s * 0.5)) * 2.0;
        let z = if diff.is_finite() {
            (diff / tau).clamp(-60.0, 0.0)
        } else {
            -60.0
        };
        let e = z.exp();
        weights.push(e);
        sum += e;
    }
    if sum > 0.0 && sum.is_finite() {
        for w in &mut weights {
            *w /= sum;
        }
    } else {
        let uniform = 1.0 / (clean.len() as f64);
        weights.fill(uniform);
    }
    (weights, tau)
}

/// Edge provenance weight for CPU-Memorilla++ graph diffusion (`structural > navigational`).
#[must_use]
pub const fn edge_provenance_weight(kind: EdgeKind) -> f64 {
    match kind {
        EdgeKind::Field | EdgeKind::Contributing => 1.0,
        EdgeKind::Navigational => 0.7,
    }
}

/// Run provenance-weighted Personalized PageRank over `TypedLinkGraph` driven by a
/// Nadaraya-Watson kernel restart distribution `w_i = softmax(score_i / tau_iqr)` over
/// query-scored seeds (CPU-Memorilla++ multi-entity composed-facts diffusion).
///
/// Preserves CV4 fail-closed guarantees (`UnresolvedSeeds`, `InvalidConfig`, `NonFiniteDetected`,
/// `diagnostic_only = true`) and conserves dangling mass back onto the query-conditioned restart
/// vector.
#[must_use]
pub fn run_kernel_seeded_ppr(
    graph: &TypedLinkGraph,
    scored_seeds: &[(Locator, f64)],
    cfg: &PprConfig,
) -> (PprSolverReport, f64) {
    let make_err = |stop_reason: SolverStopReason| {
        (
            PprSolverReport {
                diagnostic_only: true,
                residual_definition: "L1(r_{k+1} - r_k)".to_owned(),
                damping: cfg.damping,
                tolerance: cfg.tolerance,
                max_iterations: cfg.max_iterations,
                iterations_run: 0,
                final_residual: None,
                stop_reason,
                unique_seed_origins: 0,
                unverified_seed_origins: 0,
                ranked: Vec::new(),
            },
            1.0,
        )
    };

    if !cfg.damping.is_finite()
        || cfg.damping <= 0.0
        || cfg.damping >= 1.0
        || !cfg.tolerance.is_finite()
        || cfg.tolerance < 0.0
        || cfg.max_iterations == 0
    {
        return make_err(SolverStopReason::InvalidConfig {
            detail: "require 0 < damping < 1, finite tolerance >= 0, and max_iterations >= 1"
                .to_owned(),
        });
    }

    let n = graph.node_count();
    if n == 0 || scored_seeds.is_empty() {
        return make_err(SolverStopReason::EmptyGraphOrSeeds);
    }

    // Fail-closed on non-finite seed scores or unresolved locators (`CV4`, Rule #20).
    let mut missing = Vec::new();
    let mut best_by_idx: BTreeMap<usize, f64> = BTreeMap::new();
    for (loc, score) in scored_seeds {
        if !score.is_finite() {
            return make_err(SolverStopReason::NonFiniteDetected);
        }
        let Some(nid) = graph.node_id(loc) else {
            missing.push(loc.clone());
            continue;
        };
        let idx = nid.0 as usize;
        if idx >= n {
            missing.push(loc.clone());
            continue;
        }
        best_by_idx
            .entry(idx)
            .and_modify(|prev| {
                if *score > *prev {
                    *prev = *score;
                }
            })
            .or_insert(*score);
    }
    if !missing.is_empty() {
        missing.sort();
        missing.dedup();
        return make_err(SolverStopReason::UnresolvedSeeds { missing });
    }

    let seed_entries: Vec<(usize, f64)> = best_by_idx.into_iter().collect();
    let unverified_seed_origins = seed_entries.len();
    let raw_scores: Vec<f64> = seed_entries.iter().map(|(_, s)| *s).collect();
    let (weights, tau) = kernel_weights_iqr(&raw_scores);

    let mut teleport = vec![0.0f64; n];
    for ((idx, _), &w) in seed_entries.iter().zip(&weights) {
        teleport[*idx] = w;
    }

    // Build provenance-weighted undirected adjacency (`max` weight when parallel edges exist).
    let mut adj: Vec<Vec<(usize, f64)>> = vec![Vec::new(); n];
    let mut out_weight_sum = vec![0.0f64; n];
    for u in 0..n {
        let nid = NodeId(u as u32);
        let mut wmap: BTreeMap<usize, f64> = BTreeMap::new();
        let (outs, out_kinds) = graph.out_neighbors(nid);
        for (&v, &k) in outs.iter().zip(out_kinds.iter()) {
            let vi = v.0 as usize;
            if vi < n && vi != u {
                let w = edge_provenance_weight(k);
                wmap.entry(vi)
                    .and_modify(|prev| *prev = prev.max(w))
                    .or_insert(w);
            }
        }
        let (ins, in_kinds) = graph.in_neighbors(nid);
        for (&v, &k) in ins.iter().zip(in_kinds.iter()) {
            let vi = v.0 as usize;
            if vi < n && vi != u {
                let w = edge_provenance_weight(k);
                wmap.entry(vi)
                    .and_modify(|prev| *prev = prev.max(w))
                    .or_insert(w);
            }
        }
        let total_w: f64 = wmap.values().sum();
        out_weight_sum[u] = total_w;
        adj[u] = wmap.into_iter().collect();
    }

    let mut rank = teleport.clone();
    let mut next = vec![0.0f64; n];
    let mut iterations_run = 0usize;
    let mut last_residual = f64::INFINITY;
    let mut stop_reason = SolverStopReason::IterationCapReached;

    for iter in 1..=cfg.max_iterations {
        iterations_run = iter;
        next.fill(0.0);
        let mut dangling_mass = 0.0f64;

        for (u, &ru) in rank.iter().enumerate() {
            if !ru.is_finite() {
                return (
                    PprSolverReport {
                        diagnostic_only: true,
                        residual_definition: "L1(r_{k+1} - r_k)".to_owned(),
                        damping: cfg.damping,
                        tolerance: cfg.tolerance,
                        max_iterations: cfg.max_iterations,
                        iterations_run: iter,
                        final_residual: None,
                        stop_reason: SolverStopReason::NonFiniteDetected,
                        unique_seed_origins: seed_entries.len(),
                        unverified_seed_origins: 0,
                        ranked: Vec::new(),
                    },
                    tau,
                );
            }
            if out_weight_sum[u] <= 0.0 {
                dangling_mass += ru;
            } else {
                let scale = cfg.damping * ru / out_weight_sum[u];
                for &(v, w) in &adj[u] {
                    next[v] += scale * w;
                }
            }
        }

        let teleport_weight = (1.0 - cfg.damping) + cfg.damping * dangling_mass;
        let mut l1 = 0.0f64;
        for i in 0..n {
            next[i] += teleport_weight * teleport[i];
            if !next[i].is_finite() {
                return (
                    PprSolverReport {
                        diagnostic_only: true,
                        residual_definition: "L1(r_{k+1} - r_k)".to_owned(),
                        damping: cfg.damping,
                        tolerance: cfg.tolerance,
                        max_iterations: cfg.max_iterations,
                        iterations_run: iter,
                        final_residual: None,
                        stop_reason: SolverStopReason::NonFiniteDetected,
                        unique_seed_origins: seed_entries.len(),
                        unverified_seed_origins,
                        ranked: Vec::new(),
                    },
                    tau,
                );
            }
            l1 += (next[i] - rank[i]).abs();
        }
        if !l1.is_finite() {
            return (
                PprSolverReport {
                    diagnostic_only: true,
                    residual_definition: "L1(r_{k+1} - r_k)".to_owned(),
                    damping: cfg.damping,
                    tolerance: cfg.tolerance,
                    max_iterations: cfg.max_iterations,
                    iterations_run: iter,
                    final_residual: None,
                    stop_reason: SolverStopReason::NonFiniteDetected,
                    unique_seed_origins: seed_entries.len(),
                    unverified_seed_origins,
                    ranked: Vec::new(),
                },
                tau,
            );
        }
        std::mem::swap(&mut rank, &mut next);
        last_residual = l1;
        if l1 <= cfg.tolerance {
            stop_reason = SolverStopReason::Converged;
            break;
        }
    }

    let mut ranked: Vec<PprRankEntry> = rank
        .into_iter()
        .enumerate()
        .filter_map(|(idx, score)| {
            let loc = graph.locator(NodeId(idx as u32))?.clone();
            Some(PprRankEntry {
                origin_id: format!("unverified:{}/{}", loc.repo, loc.path),
                locator: loc,
                score,
            })
        })
        .collect();
    ranked.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.locator.cmp(&b.locator))
    });
    ranked.truncate(cfg.top_k);

    (
        PprSolverReport {
            diagnostic_only: true,
            residual_definition: "L1(r_{k+1} - r_k)".to_owned(),
            damping: cfg.damping,
            tolerance: cfg.tolerance,
            max_iterations: cfg.max_iterations,
            iterations_run,
            final_residual: Some(last_residual),
            stop_reason,
            unique_seed_origins: seed_entries.len(),
            unverified_seed_origins,
            ranked,
        },
        tau,
    )
}

/// Candidate input for submodular orthogonal slot selection (CPU-Memorilla++ discrete analog of
/// orthogonal PCA slots without dense `d x d` covariance matrices).
#[derive(Clone, Debug)]
pub struct OrthogonalSlotCandidate {
    pub locator: Locator,
    pub relevance_score: f64,
    pub community_id: usize,
    pub terms: BTreeSet<String>,
}

/// Selected slot with its marginal submodular gain and overlap telemetry.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SelectedSlot {
    pub locator: Locator,
    pub relevance_score: f64,
    pub marginal_gain: f64,
    pub community_id: usize,
    pub max_term_overlap: f64,
}

fn jaccard_overlap(a: &BTreeSet<String>, b: &BTreeSet<String>) -> f64 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let inter = a.intersection(b).count();
    let union = a.len() + b.len() - inter;
    if union == 0 {
        0.0
    } else {
        (inter as f64) / (union as f64)
    }
}

/// Greedily select up to `max_slots` orthogonal, high-relevance slots maximizing marginal
/// coverage: `relevance * (1.0 - lambda * max_term_overlap) * community_novelty_factor`.
///
/// `diversity_lambda` is clamped to `[0.0, 0.95]`. Deterministic tie-breaking by `Locator`.
#[must_use]
pub fn select_orthogonal_slots(
    candidates: &[OrthogonalSlotCandidate],
    max_slots: usize,
    diversity_lambda: f64,
) -> Vec<SelectedSlot> {
    if candidates.is_empty() || max_slots == 0 {
        return Vec::new();
    }
    let lambda = if diversity_lambda.is_finite() {
        diversity_lambda.clamp(0.0, 0.95)
    } else {
        0.5
    };

    let mut chosen_indices: Vec<usize> = Vec::with_capacity(max_slots.min(candidates.len()));
    let mut chosen_comms: BTreeSet<usize> = BTreeSet::new();
    let mut out: Vec<SelectedSlot> = Vec::with_capacity(max_slots.min(candidates.len()));

    while out.len() < max_slots && chosen_indices.len() < candidates.len() {
        let mut best: Option<(usize, f64, f64, f64)> = None;
        for (idx, cand) in candidates.iter().enumerate() {
            if chosen_indices.contains(&idx) {
                continue;
            }
            let rel = if cand.relevance_score.is_finite() && cand.relevance_score > 0.0 {
                cand.relevance_score
            } else {
                0.0
            };
            let max_ov = chosen_indices
                .iter()
                .map(|&ci| jaccard_overlap(&cand.terms, &candidates[ci].terms))
                .fold(0.0f64, f64::max);
            let comm_mult = if chosen_comms.contains(&cand.community_id) {
                0.80
            } else {
                1.0
            };
            let gain = rel * (1.0 - lambda * max_ov) * comm_mult;
            let is_better = match best {
                None => true,
                Some((bi, bg, _, _)) => match gain.total_cmp(&bg) {
                    std::cmp::Ordering::Greater => true,
                    std::cmp::Ordering::Equal => cand.locator < candidates[bi].locator,
                    std::cmp::Ordering::Less => false,
                },
            };
            if is_better {
                best = Some((idx, gain, max_ov, rel));
            }
        }
        let Some((pick, gain, max_ov, rel)) = best else {
            break;
        };
        chosen_indices.push(pick);
        chosen_comms.insert(candidates[pick].community_id);
        out.push(SelectedSlot {
            locator: candidates[pick].locator.clone(),
            relevance_score: rel,
            marginal_gain: gain,
            community_id: candidates[pick].community_id,
            max_term_overlap: max_ov,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use tb_syntax::EdgeKind;

    use super::*;

    fn loc(name: &str) -> Locator {
        Locator::new("vault", format!("{name}.md")).unwrap()
    }

    #[test]
    fn ppr_deduplicates_ten_copy_origin_amplification_and_rejects_unresolved_seeds() {
        let mut nodes = vec![loc("target_a"), loc("target_b"), loc("seed_a")];
        let mut edges = vec![(loc("seed_a"), loc("target_a"), EdgeKind::Navigational)];
        let mut origins = BTreeMap::new();
        origins.insert(loc("seed_a"), "origin-A".to_owned());

        let mut seeds = vec![loc("seed_a")];
        for i in 0..10 {
            let s = loc(&format!("dup_b_{i}"));
            nodes.push(s.clone());
            edges.push((s.clone(), loc("target_b"), EdgeKind::Navigational));
            origins.insert(s.clone(), "origin-B".to_owned());
            seeds.push(s);
        }

        let graph = TypedLinkGraph::build(nodes, &edges);
        let rep = run_bounded_ppr_solver(&graph, &seeds, &origins, &PprConfig::default());
        assert!(rep.diagnostic_only);
        assert_eq!(rep.stop_reason, SolverStopReason::Converged);
        assert_eq!(rep.unique_seed_origins, 2);
        assert_eq!(rep.unverified_seed_origins, 0);

        let score_a = rep
            .ranked
            .iter()
            .find(|r| r.locator == loc("target_a"))
            .unwrap()
            .score;
        let score_b = rep
            .ranked
            .iter()
            .find(|r| r.locator == loc("target_b"))
            .unwrap()
            .score;
        assert!(
            (score_a - score_b).abs() < 0.05,
            "ten-copy amplification leaked: score_a={score_a} vs score_b={score_b}"
        );

        // Unresolved seed MUST fail closed rather than silently running on a partial seed set.
        let mut bad_seeds = vec![loc("seed_a"), loc("typo_nonexistent")];
        let bad_rep = run_bounded_ppr_solver(&graph, &bad_seeds, &origins, &PprConfig::default());
        assert!(matches!(
            bad_rep.stop_reason,
            SolverStopReason::UnresolvedSeeds { ref missing } if missing == &[loc("typo_nonexistent")]
        ));
        bad_seeds.clear();
    }

    #[test]
    fn improved_ranking_with_worse_supported_answer_and_duplicate_or_nan_inputs_retain_baseline() {
        let policy = ExperimentPolicy {
            experiment_id: "exp-ppr-01".to_owned(),
            arm_name: "ppr-expansion".to_owned(),
            rollback_ref: "git:disable-ppr".to_owned(),
            min_tasks: 2,
            min_families: 2,
            min_utility_gain: 0.10,
            max_token_overhead_ratio: 1.50,
            min_null_z_score: Some(2.0),
        };
        let outcomes = vec![
            PairedTaskOutcome {
                task_id: "t1".to_owned(),
                family: "fam-1".to_owned(),
                budget_cap_tokens: 2000,
                baseline_fulfilled: true,
                candidate_fulfilled: false,
                baseline_critical_violation: false,
                candidate_critical_violation: false,
                baseline_rank_score: 0.50,
                candidate_rank_score: 0.95,
                baseline_tokens: 500,
                candidate_tokens: 520,
            },
            PairedTaskOutcome {
                task_id: "t2".to_owned(),
                family: "fam-2".to_owned(),
                budget_cap_tokens: 2000,
                baseline_fulfilled: true,
                candidate_fulfilled: true,
                baseline_critical_violation: false,
                candidate_critical_violation: false,
                baseline_rank_score: 0.60,
                candidate_rank_score: 0.98,
                baseline_tokens: 500,
                candidate_tokens: 510,
            },
        ];

        let eval = evaluate_paired_experiment(&policy, &outcomes, Some(3.5));
        assert_eq!(eval.verdict, ExperimentVerdict::RetainBaseline);
        assert!(
            eval.notes
                .iter()
                .any(|n| n.contains("regressed supported answer"))
        );

        // Non-finite min_null_z_score (-inf) and duplicate task_id MUST reject.
        let mut bad_pol = policy.clone();
        bad_pol.min_null_z_score = Some(f64::NEG_INFINITY);
        let dup_outcomes = vec![outcomes[1].clone(), outcomes[1].clone()];
        let bad_eval = evaluate_paired_experiment(&bad_pol, &dup_outcomes, Some(3.5));
        assert_eq!(bad_eval.verdict, ExperimentVerdict::RetainBaseline);
        assert!(
            bad_eval
                .notes
                .iter()
                .any(|n| n.contains("min_null_z_score must be finite"))
        );
        assert!(
            bad_eval
                .notes
                .iter()
                .any(|n| n.contains("duplicate paired task_id"))
        );
    }

    #[test]
    fn kernel_seeded_ppr_bridges_two_entities_and_orthogonal_slots_suppress_clones() {
        let e1 = Locator::new("v", "entity-a.md").unwrap();
        let e2 = Locator::new("v", "entity-b.md").unwrap();
        let bridge = Locator::new("v", "bridge-synthesis.md").unwrap();
        let leaf = Locator::new("v", "unrelated-leaf.md").unwrap();
        let graph = TypedLinkGraph::build(
            vec![e1.clone(), e2.clone(), bridge.clone(), leaf.clone()],
            &[
                (e1.clone(), bridge.clone(), EdgeKind::Field),
                (e2.clone(), bridge.clone(), EdgeKind::Field),
                (e1.clone(), leaf.clone(), EdgeKind::Navigational),
            ],
        );

        let (report, tau) = run_kernel_seeded_ppr(
            &graph,
            &[(e1.clone(), 12.0), (e2.clone(), 10.0)],
            &PprConfig::default(),
        );
        assert!(tau > 0.0 && tau.is_finite());
        assert_eq!(report.stop_reason, SolverStopReason::Converged);
        let bridge_score = report
            .ranked
            .iter()
            .find(|r| r.locator == bridge)
            .unwrap()
            .score;
        let leaf_score = report
            .ranked
            .iter()
            .find(|r| r.locator == leaf)
            .unwrap()
            .score;
        assert!(
            bridge_score > leaf_score,
            "structural bridge connecting both seeds ({bridge_score}) must outrank single navigational leaf ({leaf_score})"
        );

        // Orthogonal slot selection must prefer a slightly lower-scoring novel community over an
        // exact duplicate in the same community.
        let s1 = OrthogonalSlotCandidate {
            locator: Locator::new("v", "a1.md").unwrap(),
            relevance_score: 10.0,
            community_id: 1,
            terms: ["ppr", "graph", "diffusion"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
        };
        let s1_clone = OrthogonalSlotCandidate {
            locator: Locator::new("v", "a2.md").unwrap(),
            relevance_score: 9.8,
            community_id: 1,
            terms: ["ppr", "graph", "diffusion"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
        };
        let s2_novel = OrthogonalSlotCandidate {
            locator: Locator::new("v", "b1.md").unwrap(),
            relevance_score: 8.5,
            community_id: 2,
            terms: ["bm25", "freshness", "decay"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
        };
        let chosen = select_orthogonal_slots(&[s1, s1_clone, s2_novel], 2, 0.7);
        assert_eq!(chosen.len(), 2);
        assert_eq!(chosen[0].locator.path, "a1.md");
        assert_eq!(
            chosen[1].locator.path, "b1.md",
            "novel community slot must beat near-duplicate clone in same community"
        );
    }

    #[test]
    fn kernel_weights_iqr_handles_extreme_range_and_ppr_fails_closed_on_nan_and_slots_are_order_invariant()
     {
        // Extreme scores spanning near f64::MAX must not overflow IQR subtraction to infinity.
        let (weights, tau) = kernel_weights_iqr(&[-1e308, 1e308]);
        assert!(tau.is_finite() && tau > 0.0);
        assert_eq!(weights.len(), 2);
        assert!(weights.iter().all(|w| w.is_finite() && *w >= 0.0));
        let sum: f64 = weights.iter().sum();
        assert!((sum - 1.0).abs() < 1e-9);

        // Non-finite seed score must fail closed with NonFiniteDetected.
        let e1 = Locator::new("v", "a.md").unwrap();
        let e2 = Locator::new("v", "b.md").unwrap();
        let graph = TypedLinkGraph::build(
            vec![e1.clone(), e2.clone()],
            &[(e1.clone(), e2.clone(), EdgeKind::Field)],
        );
        let (bad_report, _) = run_kernel_seeded_ppr(
            &graph,
            &[(e1.clone(), f64::NAN), (e2.clone(), 1.0)],
            &PprConfig::default(),
        );
        assert_eq!(bad_report.stop_reason, SolverStopReason::NonFiniteDetected);

        // Input-order invariance on tied relevance scores (including non-finite sanitized to 0).
        let c_b = OrthogonalSlotCandidate {
            locator: Locator::new("v", "b.md").unwrap(),
            relevance_score: 5.0,
            community_id: 1,
            terms: ["x"].into_iter().map(str::to_owned).collect(),
        };
        let c_a = OrthogonalSlotCandidate {
            locator: Locator::new("v", "a.md").unwrap(),
            relevance_score: 5.0,
            community_id: 2,
            terms: ["y"].into_iter().map(str::to_owned).collect(),
        };
        let c_nan = OrthogonalSlotCandidate {
            locator: Locator::new("v", "c.md").unwrap(),
            relevance_score: f64::NAN,
            community_id: 3,
            terms: ["z"].into_iter().map(str::to_owned).collect(),
        };
        let fwd = select_orthogonal_slots(&[c_b.clone(), c_a.clone(), c_nan.clone()], 3, 0.7);
        let rev = select_orthogonal_slots(&[c_nan, c_a, c_b], 3, 0.7);
        assert_eq!(fwd, rev);
        assert_eq!(fwd[0].locator.path, "a.md");
        assert!(fwd.iter().all(|s| s.relevance_score.is_finite()));
    }
}
