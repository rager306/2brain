//! Offline CLI driver for `2brain enrich` (Backlog P1-6, strictly outside serving path).

use std::fs;
use std::path::{Path, PathBuf};

use clap::Subcommand;
use serde::Deserialize;
use serde_json::json;
use tb_app::enrich::{
    ENRICH_CACHE_SCHEMA_VERSION, EnrichmentCache, EnrichmentRecord, PlanStatus,
    build_enrichment_apply_plan, build_simulated_enriched_index, heuristic_enrich, plan_enrichment,
};
use tb_cognition::{ExperimentPolicy, PairedTaskOutcome, evaluate_paired_experiment};

#[derive(Debug, Subcommand)]
pub enum EnrichCommand {
    /// Scan workspace notes and compare body SHA-256 against the deterministic `(note_hash, model, prompt_version)` cache.
    Plan {
        /// Path to the workspace directory.
        workspace: PathBuf,
        /// Cache JSONL file path (created if absent when writing).
        #[arg(long = "cache")]
        cache: PathBuf,
        /// Model identifier included in the cache key.
        #[arg(long = "model", default_value = "heuristic-v1")]
        model: String,
        /// Prompt version included in the cache key.
        #[arg(long = "prompt-version", default_value = "v1")]
        prompt_version: String,
        /// Optional single repository filter.
        #[arg(long = "repo")]
        repo: Option<String>,
        /// Skip notes that already declare both `keywords` and `search_questions`.
        #[arg(long = "only-missing")]
        only_missing: bool,
        /// Optional output JSONL path containing only uncached `pending` work items.
        #[arg(long = "out")]
        out: Option<PathBuf>,
        /// Emit machine-readable JSON summary.
        #[arg(long = "json")]
        json: bool,
    },
    /// Populate the deterministic cache offline using the pure-Rust heuristic extractor (zero network/API cost).
    Heuristic {
        /// Path to the workspace directory.
        workspace: PathBuf,
        /// Cache JSONL file path to read and update.
        #[arg(long = "cache")]
        cache: PathBuf,
        /// Model identifier recorded in the cache key.
        #[arg(long = "model", default_value = "heuristic-v1")]
        model: String,
        /// Prompt version recorded in the cache key.
        #[arg(long = "prompt-version", default_value = "v1")]
        prompt_version: String,
        /// Optional single repository filter.
        #[arg(long = "repo")]
        repo: Option<String>,
        /// Skip notes that already declare both `keywords` and `search_questions`.
        #[arg(long = "only-missing")]
        only_missing: bool,
        /// Emit machine-readable JSON summary.
        #[arg(long = "json")]
        json: bool,
    },
    /// Apply cached `(keywords, search_questions)` into note frontmatter with CAS safety and body-hash verification.
    Apply {
        /// Path to the workspace directory.
        workspace: PathBuf,
        /// Cache JSONL file path.
        #[arg(long = "cache")]
        cache: PathBuf,
        /// Model identifier included in the cache key.
        #[arg(long = "model", default_value = "heuristic-v1")]
        model: String,
        /// Prompt version included in the cache key.
        #[arg(long = "prompt-version", default_value = "v1")]
        prompt_version: String,
        /// Optional single repository filter.
        #[arg(long = "repo")]
        repo: Option<String>,
        /// Overwrite existing `keywords`/`search_questions` frontmatter fields.
        #[arg(long = "overwrite")]
        overwrite: bool,
        /// Preview planned writes without mutating disk.
        #[arg(long = "dry-run")]
        dry_run: bool,
        /// Emit machine-readable JSON summary.
        #[arg(long = "json")]
        json: bool,
    },
    /// Run an isolated paired G5 evaluation (`evaluate_paired_experiment`) comparing baseline vs cached enrichment without mutating disk.
    Evaluate {
        /// Path to the workspace directory.
        workspace: PathBuf,
        /// Cache JSONL file path.
        #[arg(long = "cache")]
        cache: PathBuf,
        /// Model identifier included in the cache key.
        #[arg(long = "model", default_value = "heuristic-v1")]
        model: String,
        /// Prompt version included in the cache key.
        #[arg(long = "prompt-version", default_value = "v1")]
        prompt_version: String,
        /// Path to a paired query suite JSON (`{experiment_id, rollback_ref, min_tasks, min_families, min_utility_gain, max_token_overhead_ratio, tasks: [...]}`).
        #[arg(long = "suite")]
        suite: PathBuf,
        /// Top-k cutoff for retrieval evaluation (default: 5).
        #[arg(long = "top-k", default_value_t = 5)]
        top_k: usize,
        /// Simulate overwriting existing `keywords`/`search_questions` frontmatter fields.
        #[arg(long = "overwrite")]
        overwrite: bool,
        /// Optional explicit BM25 `min_should_match` applied symmetrically to both baseline and candidate.
        #[arg(long = "min-should-match")]
        min_should_match: Option<usize>,
        /// Emit machine-readable JSON report (compatible with `2brain experiment evaluate-arm`).
        #[arg(long = "json")]
        json: bool,
    },
}

#[derive(Debug, Deserialize)]
struct EvalSuite {
    experiment_id: String,
    rollback_ref: String,
    #[serde(default = "default_min_tasks")]
    min_tasks: usize,
    #[serde(default = "default_min_families")]
    min_families: usize,
    #[serde(default)]
    min_utility_gain: f64,
    #[serde(default = "default_max_overhead")]
    max_token_overhead_ratio: f64,
    tasks: Vec<EvalTask>,
}

#[derive(Debug, Deserialize)]
struct EvalTask {
    task_id: String,
    family: String,
    query: String,
    expected_paths: Vec<String>,
    #[serde(default)]
    forbidden_top1_paths: Vec<String>,
    #[serde(default = "default_budget_cap")]
    budget_cap_tokens: u64,
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

fn load_cache(path: &Path) -> Result<EnrichmentCache, String> {
    use rustix::fs::{Mode, OFlags, openat};
    use std::io::Read;

    let (parent, file_name) = split_parent_and_name(path)?;
    let dir_fd = match openat(
        rustix::fs::CWD,
        parent,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW,
        Mode::empty(),
    ) {
        Ok(fd) => fd,
        Err(rustix::io::Errno::NOENT) => return Ok(EnrichmentCache::new()),
        Err(e) => {
            return Err(format!(
                "cannot open cache dir '{}' without following symlinks: {e}",
                parent.display()
            ));
        }
    };
    let fd = match openat(
        &dir_fd,
        file_name,
        OFlags::RDONLY | OFlags::NOFOLLOW,
        Mode::empty(),
    ) {
        Ok(fd) => fd,
        Err(rustix::io::Errno::NOENT) => return Ok(EnrichmentCache::new()),
        Err(e) => {
            return Err(format!(
                "cannot open cache '{}' without following symlinks: {e}",
                path.display()
            ));
        }
    };
    let mut file = fs::File::from(fd);
    let meta = file
        .metadata()
        .map_err(|e| format!("cannot fstat cache '{}': {e}", path.display()))?;
    if !meta.is_file() {
        return Err(format!(
            "refusing non-regular cache file '{}'",
            path.display()
        ));
    }
    let mut raw = String::new();
    file.read_to_string(&mut raw)
        .map_err(|e| format!("cannot read cache '{}': {e}", path.display()))?;
    EnrichmentCache::from_jsonl(&raw)
}

fn split_parent_and_name(path: &Path) -> Result<(&Path, &str), String> {
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| !n.is_empty() && *n != "." && *n != "..")
        .ok_or_else(|| format!("invalid file path '{}'", path.display()))?;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    Ok((parent, file_name))
}

fn write_file_atomic_nofollow(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use rustix::fs::{AtFlags, Mode, OFlags, fsync, openat, renameat, unlinkat};
    use std::io::Write;
    use std::time::{SystemTime, UNIX_EPOCH};

    let (parent, file_name) = split_parent_and_name(path)?;
    if parent != Path::new(".") {
        fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create parent dir '{}': {e}", parent.display()))?;
    }
    let dir_fd = openat(
        rustix::fs::CWD,
        parent,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .map_err(|e| {
        format!(
            "cannot open directory '{}' without following symlinks: {e}",
            parent.display()
        )
    })?;

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp_name = format!(".{file_name}.{}.{nanos}.tmp", std::process::id());
    let fd = openat(
        &dir_fd,
        &tmp_name,
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW,
        Mode::from_raw_mode(0o600),
    )
    .map_err(|e| format!("cannot create atomic temp file '{}': {e}", path.display()))?;

    let mut file = fs::File::from(fd);
    let write_res = file.write_all(bytes).and_then(|()| file.sync_all());
    drop(file);
    if let Err(err) = write_res {
        let _ = unlinkat(&dir_fd, &tmp_name, AtFlags::empty());
        return Err(format!(
            "cannot write temp file '{}': {err}",
            path.display()
        ));
    }
    if let Err(err) = renameat(&dir_fd, &tmp_name, &dir_fd, file_name) {
        let _ = unlinkat(&dir_fd, &tmp_name, AtFlags::empty());
        return Err(format!(
            "cannot atomically replace '{}': {err}",
            path.display()
        ));
    }
    fsync(&dir_fd).map_err(|e| format!("cannot fsync parent dir '{}': {e}", parent.display()))?;
    Ok(())
}

fn save_cache_atomic(path: &Path, cache: &EnrichmentCache) -> Result<(), String> {
    write_file_atomic_nofollow(path, cache.to_jsonl().as_bytes())
}

pub fn run_enrich_command(command: EnrichCommand) {
    match command {
        EnrichCommand::Plan {
            workspace,
            cache,
            model,
            prompt_version,
            repo,
            only_missing,
            out,
            json: as_json,
        } => {
            let (_, snap) = load_workspace_or_exit(&workspace);
            let loaded_cache = load_cache(&cache).unwrap_or_else(|e| {
                eprintln!("error: {e}");
                std::process::exit(1);
            });
            let report = plan_enrichment(
                &snap,
                &loaded_cache,
                &model,
                &prompt_version,
                repo.as_deref(),
                only_missing,
            );
            if let Some(out_path) = out {
                let mut buf = String::new();
                for item in report
                    .items
                    .iter()
                    .filter(|i| i.status == PlanStatus::Pending)
                {
                    buf.push_str(&serde_json::to_string(item).expect("json"));
                    buf.push('\n');
                }
                if let Err(err) = write_file_atomic_nofollow(&out_path, buf.as_bytes()) {
                    eprintln!("error writing '{}': {err}", out_path.display());
                    std::process::exit(1);
                }
            }
            if as_json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&report).expect("json serialization")
                );
            } else {
                println!(
                    "2brain enrich plan model={} prompt={} total={} cached={} pending={} already_enriched={}",
                    report.model,
                    report.prompt_version,
                    report.total_notes,
                    report.cached_hits,
                    report.pending_misses,
                    report.already_enriched
                );
            }
        }
        EnrichCommand::Heuristic {
            workspace,
            cache,
            model,
            prompt_version,
            repo,
            only_missing,
            json: as_json,
        } => {
            let (_, snap) = load_workspace_or_exit(&workspace);
            let mut loaded_cache = load_cache(&cache).unwrap_or_else(|e| {
                eprintln!("error: {e}");
                std::process::exit(1);
            });
            let plan = plan_enrichment(
                &snap,
                &loaded_cache,
                &model,
                &prompt_version,
                repo.as_deref(),
                only_missing,
            );
            let mut generated = 0usize;
            for item in &plan.items {
                if item.status != PlanStatus::Pending {
                    continue;
                }
                let Ok(loc) = tb_contracts::Locator::new(&item.repo, &item.path) else {
                    continue;
                };
                let Some(file) = snap.files.get(&loc) else {
                    continue;
                };
                let (keywords, search_questions) =
                    heuristic_enrich(&item.title, item.description.as_deref(), file.body());
                let mut rec = EnrichmentRecord {
                    schema_version: ENRICH_CACHE_SCHEMA_VERSION.to_owned(),
                    repo: item.repo.clone(),
                    path: item.path.clone(),
                    note_hash: item.note_hash.clone(),
                    model: model.clone(),
                    prompt_version: prompt_version.clone(),
                    keywords,
                    search_questions,
                };
                match rec.sanitize() {
                    Ok(()) => {
                        loaded_cache.insert(rec);
                        generated += 1;
                    }
                    Err(err) => {
                        eprintln!(
                            "warning: skipping invalid heuristic record for '{}/{}': {err}",
                            item.repo, item.path
                        );
                    }
                }
            }
            if let Err(err) = save_cache_atomic(&cache, &loaded_cache) {
                eprintln!("error: {err}");
                std::process::exit(1);
            }
            if as_json {
                let payload = json!({
                    "cache": cache.display().to_string(),
                    "model": model,
                    "prompt_version": prompt_version,
                    "generated": generated,
                    "cache_hits": plan.cached_hits,
                    "total_cache_entries": loaded_cache.len(),
                });
                println!(
                    "{}",
                    serde_json::to_string_pretty(&payload).expect("json serialization")
                );
            } else {
                println!(
                    "2brain enrich heuristic model={} prompt={} generated={} cached={} total_entries={}",
                    model,
                    prompt_version,
                    generated,
                    plan.cached_hits,
                    loaded_cache.len()
                );
            }
        }
        EnrichCommand::Apply {
            workspace,
            cache,
            model,
            prompt_version,
            repo,
            overwrite,
            dry_run,
            json: as_json,
        } => {
            let canonical = fs::canonicalize(&workspace).unwrap_or_else(|e| {
                eprintln!("error resolving workspace '{}': {e}", workspace.display());
                std::process::exit(1);
            });
            let (store, snap) = load_workspace_or_exit(&canonical);
            let loaded_cache = load_cache(&cache).unwrap_or_else(|e| {
                eprintln!("error: {e}");
                std::process::exit(1);
            });
            let plan = build_enrichment_apply_plan(
                &snap,
                &loaded_cache,
                &model,
                &prompt_version,
                repo.as_deref(),
                overwrite,
            );
            let mut written = 0usize;
            let mut write_errors = plan.errors.clone();
            if !dry_run && write_errors.is_empty() {
                for (target_repo, ops) in &plan.ops_by_repo {
                    if ops.is_empty() {
                        continue;
                    }
                    let msg = format!(
                        "2brain: enrich {} note(s) ({model}/{prompt_version})",
                        ops.len()
                    );
                    match store.commit_ingest_batch(target_repo, ops, &msg) {
                        Ok(receipts) => written += receipts.len(),
                        Err(err) => write_errors.push(format!("{target_repo}: {err}")),
                    }
                }
            }
            let ok = write_errors.is_empty();
            if as_json {
                let payload = json!({
                    "ok": ok,
                    "dry_run": dry_run,
                    "model": model,
                    "prompt_version": prompt_version,
                    "planned_writes": plan.planned_writes,
                    "written": written,
                    "skipped_unchanged": plan.skipped_unchanged,
                    "skipped_already_enriched": plan.skipped_already_enriched,
                    "skipped_missing_cache": plan.skipped_missing_cache,
                    "errors": write_errors,
                });
                println!(
                    "{}",
                    serde_json::to_string_pretty(&payload).expect("json serialization")
                );
            } else {
                println!(
                    "2brain enrich apply ({}) model={} prompt={} planned={} written={} skipped_missing_cache={} errors={}",
                    if dry_run { "dry-run" } else { "commit" },
                    model,
                    prompt_version,
                    plan.planned_writes,
                    written,
                    plan.skipped_missing_cache,
                    write_errors.len()
                );
                for err in &write_errors {
                    eprintln!("  [error] {err}");
                }
            }
            if !ok {
                std::process::exit(1);
            }
        }
        EnrichCommand::Evaluate {
            workspace,
            cache,
            model,
            prompt_version,
            suite,
            top_k,
            overwrite,
            min_should_match,
            json: as_json,
        } => {
            let (_, snap) = load_workspace_or_exit(&workspace);
            let loaded_cache = load_cache(&cache).unwrap_or_else(|e| {
                eprintln!("error: {e}");
                std::process::exit(1);
            });
            let suite_raw = fs::read_to_string(&suite).unwrap_or_else(|e| {
                eprintln!("error reading suite '{}': {e}", suite.display());
                std::process::exit(2);
            });
            let parsed_suite: EvalSuite = serde_json::from_str(&suite_raw).unwrap_or_else(|e| {
                eprintln!("error parsing suite JSON: {e}");
                std::process::exit(2);
            });
            let (enriched_index, enriched_notes) = build_simulated_enriched_index(
                &snap,
                &loaded_cache,
                &model,
                &prompt_version,
                overwrite,
            );
            let k = top_k.max(1);
            let mut outcomes = Vec::with_capacity(parsed_suite.tasks.len());
            let matches_spec = |spec: &str, loc: &tb_contracts::Locator| -> bool {
                match spec.split_once("::") {
                    Some((r, p)) => loc.repo == r && loc.path == p,
                    None => loc.path == spec,
                }
            };
            for task in &parsed_suite.tasks {
                let base_hits = snap
                    .search
                    .query(&task.query, false, min_should_match, None, k);
                let cand_hits = enriched_index.query(&task.query, false, min_should_match, None, k);

                let eval_hits = |hits: &[tb_search::SearchHit]| -> (bool, bool, f64, u64) {
                    let mut best_rr = 0.0f64;
                    for (rank, h) in hits.iter().enumerate() {
                        if task
                            .expected_paths
                            .iter()
                            .any(|p| matches_spec(p, &h.locator))
                        {
                            best_rr = 1.0 / ((rank + 1) as f64);
                            break;
                        }
                    }
                    let fulfilled = best_rr > 0.0;
                    let crit = hits.first().is_some_and(|top| {
                        task.forbidden_top1_paths
                            .iter()
                            .any(|f| matches_spec(f, &top.locator))
                    });
                    // Deterministic token proxy: query + retrieved hit excerpts/metadata (~120 tokens/hit).
                    let tokens = 80u64 + (hits.len() as u64) * 120;
                    (fulfilled, crit, best_rr, tokens)
                };

                let (b_ok, b_crit, b_rr, b_tok) = eval_hits(&base_hits);
                let (c_ok, c_crit, c_rr, c_tok) = eval_hits(&cand_hits);
                outcomes.push(PairedTaskOutcome {
                    task_id: task.task_id.clone(),
                    family: task.family.clone(),
                    budget_cap_tokens: task.budget_cap_tokens,
                    baseline_fulfilled: b_ok,
                    candidate_fulfilled: c_ok,
                    baseline_critical_violation: b_crit,
                    candidate_critical_violation: c_crit,
                    baseline_rank_score: b_rr,
                    candidate_rank_score: c_rr,
                    baseline_tokens: b_tok,
                    candidate_tokens: c_tok,
                });
            }

            let policy = ExperimentPolicy {
                experiment_id: parsed_suite.experiment_id,
                arm_name: format!("enrich:{model}:{prompt_version}"),
                rollback_ref: parsed_suite.rollback_ref,
                min_tasks: parsed_suite.min_tasks,
                min_families: parsed_suite.min_families,
                min_utility_gain: parsed_suite.min_utility_gain,
                max_token_overhead_ratio: parsed_suite.max_token_overhead_ratio,
                min_null_z_score: None,
            };
            let evaluation = evaluate_paired_experiment(&policy, &outcomes, None);

            if as_json {
                let payload = json!({
                    "enriched_notes_in_simulation": enriched_notes,
                    "policy": policy,
                    "outcomes": outcomes,
                    "evaluation": evaluation,
                });
                println!(
                    "{}",
                    serde_json::to_string_pretty(&payload).expect("json serialization")
                );
            } else {
                println!(
                    "2brain enrich evaluate arm='{}' enriched_notes={} verdict={:?} tasks={} base_rate={:?} cand_rate={:?} gain={:?} base_mrr={:?} cand_mrr={:?}",
                    evaluation.arm_name,
                    enriched_notes,
                    evaluation.verdict,
                    evaluation.task_count,
                    evaluation.baseline_fulfillment_rate,
                    evaluation.candidate_fulfillment_rate,
                    evaluation.utility_gain,
                    evaluation.baseline_mean_rank,
                    evaluation.candidate_mean_rank
                );
                for note in &evaluation.notes {
                    println!("  note: {note}");
                }
            }
        }
    }
}

fn load_workspace_or_exit(
    workspace: &Path,
) -> (tb_fs_git::FsWorkspaceStore, tb_app::VaultSnapshot) {
    match tb_fs_git::load_snapshot(workspace) {
        Ok(pair) => pair,
        Err(err) => {
            eprintln!("error loading workspace '{}': {err}", workspace.display());
            std::process::exit(1);
        }
    }
}
