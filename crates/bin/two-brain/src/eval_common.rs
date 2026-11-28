//! Shared G5 paired evaluation suite types and honest token accounting used by both
//! `2brain enrich evaluate` and `2brain experiment hybrid-eval`.

use serde::Deserialize;
use tb_app::VaultSnapshot;
use tb_contracts::Locator;
use tb_search::{EXCERPT_CHARS, SearchHit, distinct_terms, excerpt};

#[derive(Debug, Deserialize)]
pub struct EvalSuite {
    pub experiment_id: String,
    pub rollback_ref: String,
    #[serde(default = "default_min_tasks")]
    pub min_tasks: usize,
    #[serde(default = "default_min_families")]
    pub min_families: usize,
    #[serde(default)]
    pub min_utility_gain: f64,
    #[serde(default = "default_max_overhead")]
    pub max_token_overhead_ratio: f64,
    #[cfg_attr(
        not(feature = "hybrid-rerank"),
        expect(
            dead_code,
            reason = "deserialized for suite parity; consumed under hybrid-rerank feature"
        )
    )]
    #[serde(default)]
    pub synonym_clusters: Vec<Vec<String>>,
    pub tasks: Vec<EvalTask>,
}

#[derive(Debug, Deserialize)]
pub struct EvalTask {
    pub task_id: String,
    pub family: String,
    pub query: String,
    pub expected_paths: Vec<String>,
    #[serde(default)]
    pub forbidden_top1_paths: Vec<String>,
    #[serde(default = "default_budget_cap")]
    pub budget_cap_tokens: u64,
}

fn default_min_tasks() -> usize {
    4
}
fn default_min_families() -> usize {
    2
}
fn default_max_overhead() -> f64 {
    1.15
}
fn default_budget_cap() -> u64 {
    4000
}

/// Match either a repo-qualified spec (`repo::path`) or a repo-relative path (`path`).
#[must_use]
pub fn matches_locator_spec(spec: &str, loc: &Locator) -> bool {
    match spec.split_once("::") {
        Some((r, p)) => loc.repo == r && loc.path == p,
        None => loc.path == spec,
    }
}

/// Evaluate retrieved hits for one paired task:
/// returns `(fulfilled, critical_violation, best_reciprocal_rank, estimated_tokens)`.
///
/// Token accounting measures actual query + returned locator/description/excerpt characters
/// (`ceil(chars / 4)`) rather than a constant per-hit stub.
#[must_use]
pub fn evaluate_search_hits(
    snapshot: &VaultSnapshot,
    task: &EvalTask,
    hits: &[SearchHit],
) -> (bool, bool, f64, u64) {
    let mut best_rr = 0.0f64;
    for (rank, h) in hits.iter().enumerate() {
        if task
            .expected_paths
            .iter()
            .any(|p| matches_locator_spec(p, &h.locator))
        {
            best_rr = 1.0 / ((rank + 1) as f64);
            break;
        }
    }
    let fulfilled = best_rr > 0.0;
    let critical = hits.first().is_some_and(|top| {
        task.forbidden_top1_paths
            .iter()
            .any(|f| matches_locator_spec(f, &top.locator))
    });

    let q_terms = distinct_terms(&task.query);
    let mut total_chars: usize = task.query.chars().count() + 64;
    for h in hits {
        total_chars = total_chars
            .saturating_add(h.locator.repo.chars().count() + h.locator.path.chars().count() + 24);
        if let Some(file) = snapshot.files.get(&h.locator) {
            if let Some(ref desc) = file.description {
                total_chars = total_chars.saturating_add(desc.chars().count());
            }
            let snip = excerpt(file.body(), &q_terms, EXCERPT_CHARS);
            total_chars = total_chars.saturating_add(snip.chars().count());
        }
    }
    let tokens = (total_chars.div_ceil(4) as u64).max(1);
    (fulfilled, critical, best_rr, tokens)
}
