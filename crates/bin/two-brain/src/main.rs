//! Composition root producing the single `2brain` binary (MIT).

mod backup;
mod bench;
mod enrich;
mod ingest;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use clap::{Parser, Subcommand};
use notify::{RecursiveMode, Watcher};
use tb_app::{App, AppDispatcher, ProcedureSpec, TaskStorePort};
use tb_contracts::{
    TASK_PACKET_SCHEMA_VERSION, TaskBudget, TaskObligation, TaskPacket, TaskStatus,
};
use tb_ipc::IpcClient;
use tb_journal::{FileOutcomeStore, FileTaskStore};
use tb_mcp::{
    AuditRecorder, EgressPolicy, HttpServerConfig, LaunchPolicy, PayloadMode, ResourceUpdateHub,
    TwoBrainMcp,
};

#[derive(Debug, Parser)]
#[command(
    name = "2brain",
    version,
    about = "Single-binary typed knowledge graph and MCP 2.0 server (MIT)"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Print build, architecture, and protocol surface details.
    Info,
    /// Read-only parse, schema validation, CSR graph health, and description quality check on a workspace.
    Check {
        /// Path to the workspace directory.
        path: PathBuf,
        /// List individual notes with missing, weak, or stale (disconnected) frontmatter descriptions.
        #[arg(long = "audit-descriptions")]
        audit_descriptions: bool,
        /// Maximum number of description audit findings to list.
        #[arg(long = "limit", default_value_t = 25)]
        limit: usize,
        /// Emit machine-readable JSON output.
        #[arg(long = "json")]
        json: bool,
    },
    /// Run InfraNodus & cognitive topology + dual-layer gap analysis on a workspace.
    Analyze {
        /// Path to the workspace directory.
        path: PathBuf,
        /// Number of top-betweenness hubs to mask for Peel-the-Onion.
        #[arg(long = "peel-top-k", default_value_t = 5)]
        peel_top_k: usize,
        /// Maximum rows per section.
        #[arg(long = "limit", default_value_t = 5)]
        limit: usize,
    },
    /// Audit D03 claims, R14 temporal validity, and Polysingularity contradictions.
    AuditClaims {
        /// Path to the workspace directory.
        path: PathBuf,
        /// Optional ISO-8601 date (`YYYY-MM-DD`) to evaluate temporal validity against.
        #[arg(long = "as-of")]
        as_of: Option<String>,
        /// Maximum conflicts to print.
        #[arg(long = "limit", default_value_t = 25)]
        limit: usize,
    },
    /// Analyze Hub Fragility (articulation points & overloaded MOC map+satellite splits).
    Fragility {
        /// Path to the workspace directory.
        path: PathBuf,
        /// Maximum fragile hubs to print.
        #[arg(long = "limit", default_value_t = 15)]
        limit: usize,
    },
    /// Compute structural & topological Difference Graph (`G(T1) -> G(T2)`) and Change-Driven Decay.
    Diff {
        /// Path to the workspace directory (must be a Git repository).
        path: PathBuf,
        /// Baseline Git ref (e.g. `HEAD~10`, commit SHA, tag).
        from_ref: String,
        /// Optional target Git ref (defaults to live working tree).
        to_ref: Option<String>,
        /// Maximum rows per section.
        #[arg(long = "limit", default_value_t = 10)]
        limit: usize,
    },
    /// Run or inspect the Unix-socket workspace daemon.
    Daemon {
        #[command(subcommand)]
        command: DaemonCommand,
    },
    /// Manage the opt-in ADR-0009 durable outcome registry for a workspace.
    Outcomes {
        #[command(subcommand)]
        command: OutcomesCommand,
    },
    /// Create a SHA-256 verified backup snapshot of a workspace (refuses symlinks and non-empty targets).
    Backup {
        /// Path to the source workspace directory.
        workspace: PathBuf,
        /// Empty or non-existent output directory for the backup bundle.
        #[arg(long = "out")]
        out: PathBuf,
        /// Emit machine-readable JSON summary.
        #[arg(long = "json")]
        json: bool,
    },
    /// Verify a backup's SHA-256 manifest and restore it into a new/empty directory, validating workspace load.
    Restore {
        /// Path to the backup directory containing `manifest.sha256.json`.
        backup: PathBuf,
        /// Empty or non-existent target directory to restore into.
        #[arg(long = "to")]
        to: PathBuf,
        /// Emit machine-readable JSON summary.
        #[arg(long = "json")]
        json: bool,
    },
    /// Create, inspect, list, and CAS-checkpoint durable D08 TaskPackets for cross-client handoff.
    Task {
        #[command(subcommand)]
        command: TaskCommand,
    },
    /// Execute or preview a declarative D09 ProcedureSpec with independent postconditions and D10 RunReceipt.
    Procedure {
        #[command(subcommand)]
        command: ProcedureCommand,
    },
    /// Run G5 evaluation gates (U13 verified experience promotion, U14 paired experiments & diagnostic PPR).
    Experiment {
        #[command(subcommand)]
        command: ExperimentCommand,
    },
    /// Read-only performance benchmark on a workspace (cold load, incremental refresh, BM25, cognitive suite, RSS).
    Bench {
        /// Path to the workspace directory.
        path: PathBuf,
        /// Number of repeated iterations for incremental/search timings.
        #[arg(long = "iterations", default_value_t = 5)]
        iterations: usize,
        /// Emit machine-readable JSON output.
        #[arg(long = "json")]
        json: bool,
    },
    /// Offline note enrichment (`keywords`, `search_questions`) with deterministic `(note_hash, model, prompt_version)` cache and G5 evaluation.
    Enrich {
        #[command(subcommand)]
        command: enrich::EnrichCommand,
    },
    /// Convert external sources (.md, .txt, .html, .csv, .json, .jsonl, error-fix logs) into typed Markdown notes with fail-closed schema/link/dedup validation.
    Ingest {
        /// Path to the target workspace directory.
        workspace: PathBuf,
        /// Path to a source file or directory to ingest.
        source: PathBuf,
        /// Destination folder inside the target repo (default: `inbox`).
        #[arg(long = "dest", default_value = "inbox")]
        dest: String,
        /// Target member repository name (defaults to workspace entry repo).
        #[arg(long = "repo")]
        repo: Option<String>,
        /// Default frontmatter `type` when the source lacks one.
        #[arg(long = "type")]
        default_type: Option<String>,
        /// Optional capture date (`YYYY-MM-DD`) stored in frontmatter `captured`.
        #[arg(long = "date")]
        date: Option<String>,
        /// Input format (`auto`, `md`, `txt`, `html`, `csv`, `json`, `jsonl`, `error-fix`).
        #[arg(long = "format", default_value = "auto")]
        format: String,
        /// Downgrade unresolved outgoing `[[wikilinks]]` from hard error to warning.
        #[arg(long = "allow-unresolved-links")]
        allow_unresolved_links: bool,
        /// Downgrade ambiguous stem `[[wikilinks]]` from hard error to warning.
        #[arg(long = "allow-ambiguous-links")]
        allow_ambiguous_links: bool,
        /// Overwrite existing target notes via CAS `write_file`.
        #[arg(long = "refresh")]
        refresh: bool,
        /// Reject notes whose top BM25 near-duplicate score meets or exceeds this threshold.
        #[arg(long = "dedup-threshold")]
        dedup_threshold: Option<f64>,
        /// Validate and report without writing any files.
        #[arg(long = "dry-run")]
        dry_run: bool,
        /// Emit machine-readable JSON report.
        #[arg(long = "json")]
        json: bool,
    },
    /// Serve MCP 2.0 (2026-07-28 stateless + legacy + SEP-2640 skills + SEP-2663 tasks) over stdio or Streamable HTTP.
    Mcp {
        /// Unix socket path OR workspace directory of a running `2brain daemon` (omit with --in-process or --info).
        socket: Option<PathBuf>,
        /// Load the workspace directly in-process (zero IPC daemon needed).
        #[arg(long = "in-process")]
        in_process: Option<PathBuf>,
        /// Serve MCP 2.0 over Streamable HTTP at `<ip>:<port>` instead of stdio (e.g. `127.0.0.1:8787`).
        #[arg(long = "http")]
        http: Option<SocketAddr>,
        /// Path to a file containing the Bearer auth token for `--http` (alternative to `TWOBRAIN_HTTP_TOKEN` env var).
        #[arg(long = "http-token-file")]
        http_token_file: Option<PathBuf>,
        /// Additional allowed `Host` header entries for `--http` DNS-rebinding protection (repeatable).
        #[arg(long = "http-allow-host")]
        http_allow_hosts: Vec<String>,
        /// Allowed RFC 6454 browser `Origin` entries for `--http` (repeatable; empty rejects all browser origins).
        #[arg(long = "http-allow-origin")]
        http_allow_origins: Vec<String>,
        /// Enable stateful legacy HTTP sessions (`Mcp-Session-Id` + `GET /mcp` SSE) for pre-2026-07-28 clients.
        #[arg(long = "http-legacy-sessions")]
        http_legacy_sessions: bool,
        /// Enable the 11 core mutating tools (fail-closed by default).
        #[arg(long = "allow-mutations")]
        allow_mutations: bool,
        /// Enable repository dependency mutations (implies --allow-mutations).
        #[arg(long = "allow-composition")]
        allow_composition: bool,
        /// Enable workspace member shape mutations (implies --allow-mutations).
        #[arg(long = "allow-workspace-shape")]
        allow_workspace_shape: bool,
        /// Enable the fail-closed inline `tb_ingest` MCP tool (implies --allow-mutations).
        #[arg(long = "allow-ingest")]
        allow_ingest: bool,
        /// Enable the 4 opt-in D08/D09/D10 workflow tools (tb_task_checkpoint, tb_task_resume, tb_task_list, tb_run_procedure).
        #[arg(long = "allow-workflows")]
        allow_workflows: bool,
        /// Acknowledge durable outcome-registry tracking for request_id mutations.
        #[arg(long = "outcome-registry")]
        outcome_registry: bool,
        /// Enable the 9 v2 cognitive & epistemic tools (tb_orient, tb_analyze_topology, tb_detect_gaps, tb_audit_bias, tb_audit_contradictions, tb_fragility_report, tb_diff_graphs, tb_record_decision, tb_list_decisions).
        #[arg(long = "allow-cognitive")]
        allow_cognitive: bool,
        /// Optional audit principal label (accepted for client compatibility; not authentication).
        #[arg(long = "principal")]
        principal: Option<String>,
        /// Tool list schema verbosity (`concise` or `full`).
        #[arg(long = "tools", default_value = "concise")]
        tools: String,
        /// Tool payload representation (`both`, `text`, or `structured`).
        #[arg(long = "payload", default_value = "both")]
        payload: String,
        /// Optional JSON policy file path (revoked_repos, allowed_repos, egress_destination, allow_external_egress, audit_log_path).
        #[arg(long = "policy")]
        policy: Option<PathBuf>,
        /// Optional hash-chained JSONL audit log path for mutations and policy refusals.
        #[arg(long = "audit-log")]
        audit_log: Option<PathBuf>,
        /// Print MCP protocol surface and launch information, then exit.
        #[arg(long = "info")]
        info: bool,
    },
}

#[derive(Debug, Subcommand)]
enum DaemonCommand {
    /// Start the workspace daemon listening on `$2BRAIN_HOME/engine/run/<slug>.sock`.
    Start {
        /// Path to the workspace directory.
        workspace: PathBuf,
    },
    /// Print the expected socket path and connectivity status for a workspace.
    Status {
        /// Path to the workspace directory.
        workspace: PathBuf,
    },
}

#[derive(Debug, Subcommand)]
enum OutcomesCommand {
    /// Enable the opt-in outcome registry fence (`registry.version`) for a workspace.
    Enable {
        /// Path to the workspace directory.
        workspace: PathBuf,
    },
    /// Configure the admission quota (`quota.json`) for a workspace's outcome registry.
    Quota {
        /// Maximum number of live journal records.
        #[arg(long = "max-records")]
        max_records: usize,
        /// Maximum total bytes across live journals and tombstones.
        #[arg(long = "max-bytes")]
        max_bytes: u64,
        /// Path to the workspace directory.
        workspace: PathBuf,
    },
    /// Show enabled state and live journal/tombstone usage for a workspace's outcome registry.
    Status {
        /// Path to the workspace directory.
        workspace: PathBuf,
        /// Emit machine-readable JSON output.
        #[arg(long = "json")]
        json: bool,
    },
    /// Run crash-safe startup recovery on pre-effect outcome journals.
    Recover {
        /// Path to the workspace directory.
        workspace: PathBuf,
    },
    /// Compact terminal outcome journals older than `--max-age-secs` into tombstones.
    Compact {
        /// Path to the workspace directory.
        workspace: PathBuf,
        /// Retention age in seconds before compacting terminal journals (default: 7 days).
        #[arg(long = "max-age-secs", default_value_t = 7 * 24 * 60 * 60)]
        max_age_secs: u64,
    },
    /// Prune oldest tombstones until total tombstone bytes <= `--max-bytes`.
    Prune {
        /// Path to the workspace directory.
        workspace: PathBuf,
        /// Maximum tombstone bytes to retain.
        #[arg(long = "max-bytes")]
        max_bytes: u64,
    },
}

#[derive(Debug, Subcommand)]
enum TaskCommand {
    /// Create a new durable D08 TaskPacket (revision 1).
    Create {
        /// Path to the workspace directory.
        workspace: PathBuf,
        /// Task identifier (`[a-zA-Z0-9_-]{1,64}`).
        #[arg(long = "id")]
        id: String,
        /// Human-readable task title.
        #[arg(long = "title")]
        title: String,
        /// Declared scope repositories (repeatable).
        #[arg(long = "scope-repo")]
        scope_repos: Vec<String>,
        /// Critical obligations in `key=value` form (repeatable, e.g. `--obligation publish=false`).
        #[arg(long = "obligation")]
        obligations: Vec<String>,
        /// Maximum action budget.
        #[arg(long = "max-actions", default_value_t = 16)]
        max_actions: u32,
        /// Maximum token budget.
        #[arg(long = "max-tokens", default_value_t = 32_000)]
        max_tokens: u64,
        /// Principal/client label creating this task.
        #[arg(long = "by", default_value = "cli")]
        by: String,
        /// Emit machine-readable JSON output.
        #[arg(long = "json")]
        json: bool,
    },
    /// Show a persisted D08 TaskPacket by id.
    Show {
        /// Path to the workspace directory.
        workspace: PathBuf,
        /// Task identifier.
        id: String,
        /// Emit machine-readable JSON output.
        #[arg(long = "json")]
        json: bool,
    },
    /// List persisted D08 TaskPackets in a workspace.
    List {
        /// Path to the workspace directory.
        workspace: PathBuf,
        /// Emit machine-readable JSON output.
        #[arg(long = "json")]
        json: bool,
    },
    /// Optimistically CAS-checkpoint a D08 TaskPacket from a JSON file (`--expected-revision`).
    Checkpoint {
        /// Path to the workspace directory.
        workspace: PathBuf,
        /// Expected current revision for optimistic concurrency check.
        #[arg(long = "expected-revision")]
        expected_revision: u64,
        /// Path to a JSON file containing the updated TaskPacket.
        #[arg(long = "packet")]
        packet_file: PathBuf,
        /// Explicitly allow dropping critical obligations (default: false).
        #[arg(long = "allow-drop-critical")]
        allow_drop_critical: bool,
        /// Emit machine-readable JSON output.
        #[arg(long = "json")]
        json: bool,
    },
}

#[derive(Debug, Subcommand)]
enum ProcedureCommand {
    /// Execute or dry-run a declarative D09 ProcedureSpec JSON file and emit a D10 RunReceipt.
    Run {
        /// Path to the workspace directory.
        workspace: PathBuf,
        /// Path to a `2brain.procedure/v1` JSON specification file.
        spec: PathBuf,
        /// Preview steps without mutating disk (cannot weaken `dry_run: true` in spec).
        #[arg(long = "dry-run")]
        dry_run: bool,
        /// Explicit authority grants held by caller (repeatable, e.g. `--grant mutations --grant procedures`).
        #[arg(long = "grant")]
        grants: Vec<String>,
        /// Emit machine-readable JSON RunReceipt.
        #[arg(long = "json")]
        json: bool,
    },
}

#[derive(Debug, Subcommand)]
enum ExperimentCommand {
    /// Evaluate a U13/AE18 experience promotion candidate JSON (`{lesson, observations, policy}`).
    EvaluateLesson {
        /// Path to the JSON evaluation input file.
        input: PathBuf,
        /// Trusted baseline grants overriding `policy.baseline_grants` from input JSON (repeatable).
        #[arg(long = "baseline-grant")]
        baseline_grants: Vec<String>,
        /// Trusted allowed scopes overriding `policy.allowed_scopes` from input JSON (repeatable).
        #[arg(long = "allowed-scope")]
        allowed_scopes: Vec<String>,
        /// Emit machine-readable JSON report.
        #[arg(long = "json")]
        json: bool,
    },
    /// Evaluate a U14/AE19-20 paired experiment JSON (`{policy, outcomes, null_z_score}`).
    EvaluateArm {
        /// Path to the JSON evaluation input file.
        input: PathBuf,
        /// Emit machine-readable JSON report.
        #[arg(long = "json")]
        json: bool,
    },
    /// Run diagnostic bounded Personalized PageRank (CV4/M23) with duplicate-origin normalization.
    Ppr {
        /// Path to the workspace directory.
        workspace: PathBuf,
        /// Seed note path(s) (`path` or `repo::path`, repeatable).
        #[arg(long = "seed", required = true)]
        seeds: Vec<String>,
        /// Damping factor alpha in (0, 1).
        #[arg(long = "damping", default_value_t = 0.85)]
        damping: f64,
        /// Maximum iterations.
        #[arg(long = "max-iter", default_value_t = 100)]
        max_iter: usize,
        /// Disable duplicate-origin seed normalization (for ablation comparisons).
        #[arg(long = "no-origin-dedup")]
        no_origin_dedup: bool,
        /// Maximum ranked rows to return.
        #[arg(long = "limit", default_value_t = 20)]
        limit: usize,
        /// Emit machine-readable JSON report.
        #[arg(long = "json")]
        json: bool,
    },
    /// Run isolated U14 hybrid reranker experiment (requires `--features hybrid-rerank`) under the G5 paired gate.
    HybridEval {
        /// Path to the workspace directory.
        workspace: PathBuf,
        /// Path to a paired query suite JSON (`tasks` with `expected_paths` and `forbidden_top1_paths` for SEM-48).
        #[arg(long = "suite")]
        suite: PathBuf,
        /// Optional JSON static embedding table (`{dim, vectors: {token: [f32; dim]}}`); defaults to deterministic subword projection.
        #[arg(long = "model-vectors")]
        model_vectors: Option<PathBuf>,
        /// Semantic blend weight alpha in [0.0, 1.0].
        #[arg(long = "alpha", default_value_t = 0.25)]
        alpha: f64,
        /// BM25 candidate pool size to rerank.
        #[arg(long = "pool", default_value_t = 32)]
        pool: usize,
        /// Top-k cutoff.
        #[arg(long = "top-k", default_value_t = 5)]
        top_k: usize,
        /// Disable SEM-48 negation guard (ablation only).
        #[arg(long = "no-negation-guard")]
        no_negation_guard: bool,
        /// Hard RAM ceiling in MiB for model + document vectors.
        #[arg(long = "max-ram-mib", default_value_t = 32)]
        max_ram_mib: usize,
        /// Emit machine-readable JSON report.
        #[arg(long = "json")]
        json: bool,
    },
}

fn main() {
    let cli = Cli::parse();
    match cli.command {
        Command::Info => {
            println!(
                "2brain {} (clean-room MIT monorepo)",
                env!("CARGO_PKG_VERSION")
            );
            println!("  architecture hexagonal/onion (13 crates, 1 binary)");
            println!(
                "  protocol     MCP 2026-07-28 (stateless) + legacy + SEP-2640 skills + tasks"
            );
            println!("  layers       domain (0 I/O) -> app (ports) -> adapters -> cli");
        }
        Command::Check {
            path,
            audit_descriptions,
            limit,
            json,
        } => run_check(&path, audit_descriptions, limit, json),
        Command::Analyze {
            path,
            peel_top_k,
            limit,
        } => run_analyze(&path, peel_top_k, limit),
        Command::AuditClaims { path, as_of, limit } => {
            run_audit_claims(&path, as_of.as_deref(), limit);
        }
        Command::Fragility { path, limit } => run_fragility(&path, limit),
        Command::Diff {
            path,
            from_ref,
            to_ref,
            limit,
        } => run_diff(&path, &from_ref, to_ref.as_deref(), limit),
        Command::Daemon { command } => match command {
            DaemonCommand::Start { workspace } => run_daemon_start(&workspace),
            DaemonCommand::Status { workspace } => run_daemon_status(&workspace),
        },
        Command::Outcomes { command } => match command {
            OutcomesCommand::Enable { workspace } => {
                let root = tb_journal::outcome_registry_root(&workspace);
                if let Err(err) = tb_journal::enable(&root) {
                    eprintln!("error enabling outcome registry: {err}");
                    std::process::exit(1);
                }
                println!("enabled outcome registry at {}", root.display());
            }
            OutcomesCommand::Quota {
                max_records,
                max_bytes,
                workspace,
            } => {
                let root = tb_journal::outcome_registry_root(&workspace);
                if let Err(err) = tb_journal::configure_quota(&root, max_records, max_bytes) {
                    eprintln!("error configuring outcome quota: {err}");
                    std::process::exit(1);
                }
                println!(
                    "configured outcome quota at {} (max_records={max_records}, max_bytes={max_bytes})",
                    root.display()
                );
            }
            OutcomesCommand::Status { workspace, json } => {
                let root = tb_journal::outcome_registry_root(&workspace);
                let reg = tb_journal::open_registry(&root);
                let enabled = reg.is_enabled();
                let (records, bytes) = if enabled {
                    match reg.usage() {
                        Ok(u) => u,
                        Err(err) => {
                            eprintln!("error reading outcome usage: {err}");
                            std::process::exit(1);
                        }
                    }
                } else {
                    (0, 0)
                };
                if json {
                    let payload = serde_json::json!({
                        "root": root.display().to_string(),
                        "enabled": enabled,
                        "live_records": records,
                        "total_bytes": bytes,
                    });
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&payload).expect("json serialization")
                    );
                } else {
                    println!(
                        "outcomes status ({}): enabled={enabled} live_records={records} total_bytes={bytes}",
                        root.display()
                    );
                }
            }
            OutcomesCommand::Recover { workspace } => {
                match tb_journal::recover_outcome_registry(&workspace) {
                    Ok(rec) => println!(
                        "recovered outcome registry: rejected={} unresolved={} corrupt={}",
                        rec.rejected, rec.unresolved, rec.corrupt
                    ),
                    Err(err) => {
                        eprintln!("error recovering outcome registry: {err}");
                        std::process::exit(1);
                    }
                }
            }
            OutcomesCommand::Compact {
                workspace,
                max_age_secs,
            } => {
                let root = tb_journal::outcome_registry_root(&workspace);
                let reg = tb_journal::open_registry(&root);
                if !reg.is_enabled() {
                    println!("outcome registry is not enabled at {}", root.display());
                    return;
                }
                match reg.compact(Duration::from_secs(max_age_secs)) {
                    Ok(n) => println!("compacted {n} terminal journal(s) at {}", root.display()),
                    Err(err) => {
                        eprintln!("error compacting outcome registry: {err}");
                        std::process::exit(1);
                    }
                }
            }
            OutcomesCommand::Prune {
                workspace,
                max_bytes,
            } => {
                let root = tb_journal::outcome_registry_root(&workspace);
                let reg = tb_journal::open_registry(&root);
                if !reg.is_enabled() {
                    println!("outcome registry is not enabled at {}", root.display());
                    return;
                }
                match reg.prune_tombstones_to(max_bytes) {
                    Ok((removed, freed)) => println!(
                        "pruned {removed} tombstone(s), freed {freed} byte(s) at {}",
                        root.display()
                    ),
                    Err(err) => {
                        eprintln!("error pruning outcome tombstones: {err}");
                        std::process::exit(1);
                    }
                }
            }
        },
        Command::Backup {
            workspace,
            out,
            json,
        } => backup::run_backup(&workspace, &out, json),
        Command::Restore { backup, to, json } => backup::run_restore(&backup, &to, json),
        Command::Task { command } => run_task_command(command),
        Command::Procedure { command } => run_procedure_command(command),
        Command::Experiment { command } => run_experiment_command(command),
        Command::Bench {
            path,
            iterations,
            json,
        } => bench::run_bench(&path, iterations, json),
        Command::Enrich { command } => enrich::run_enrich_command(command),
        Command::Ingest {
            workspace,
            source,
            dest,
            repo,
            default_type,
            date,
            format,
            allow_unresolved_links,
            allow_ambiguous_links,
            refresh,
            dedup_threshold,
            dry_run,
            json,
        } => ingest::run_ingest(ingest::IngestCliArgs {
            workspace,
            source,
            dest,
            repo,
            default_type,
            date,
            format,
            allow_unresolved_links,
            allow_ambiguous_links,
            refresh,
            dedup_threshold,
            dry_run,
            as_json: json,
        }),
        Command::Mcp {
            socket,
            in_process,
            http,
            http_token_file,
            http_allow_hosts,
            http_allow_origins,
            http_legacy_sessions,
            allow_mutations,
            allow_composition,
            allow_workspace_shape,
            allow_ingest,
            allow_workflows,
            outcome_registry,
            allow_cognitive,
            principal,
            tools,
            payload,
            policy: policy_file,
            audit_log,
            info,
        } => {
            let payload_mode = match PayloadMode::parse(&payload) {
                Ok(m) => m,
                Err(err) => {
                    eprintln!("error: {err}");
                    std::process::exit(2);
                }
            };
            let egress = match policy_file {
                Some(ref path) => match load_egress_policy(path) {
                    Ok(p) => Some(p),
                    Err(err) => {
                        eprintln!("error loading --policy '{}': {err}", path.display());
                        std::process::exit(2);
                    }
                },
                None => None,
            };
            let audit_path = audit_log.or_else(|| {
                egress
                    .as_ref()
                    .and_then(|e| e.audit_log_path.as_ref())
                    .map(PathBuf::from)
            });
            let audit: Option<Arc<dyn AuditRecorder>> = audit_path.map(|p| {
                Arc::new(FileAuditAdapter {
                    log: tb_journal::FileAuditLog::new(p),
                }) as Arc<dyn AuditRecorder>
            });
            let effective_mutations =
                allow_mutations || allow_composition || allow_workspace_shape || allow_ingest;
            let policy = LaunchPolicy {
                allow_mutations: effective_mutations,
                allow_composition,
                allow_workspace_shape,
                allow_ingest,
                allow_workflows,
                outcome_registry,
                allow_cognitive,
                full_schemas: tools.eq_ignore_ascii_case("full"),
                payload_mode,
                principal,
                egress,
            };
            if info {
                println!(
                    "2brain {} (clean-room MIT single binary)",
                    env!("CARGO_PKG_VERSION")
                );
                println!(
                    "  tools        54 core (38 read-only, 16 mutating) + 9 cognitive (--allow-cognitive) + 1 ingest (--allow-ingest) + 4 workflow (--allow-workflows)"
                );
                println!(
                    "  protocol     MCP 2026-07-28 (stateless) + legacy + SEP-2640 skills + SEP-2663 tasks"
                );
                println!("  transports   stdio (default), streamable-http (--http <ip>:<port>)");
                return;
            }
            let http_cfg = match http {
                Some(bind_addr) => {
                    let bearer_token = match resolve_http_token(http_token_file.as_deref()) {
                        Ok(t) => t,
                        Err(err) => {
                            eprintln!("error loading HTTP auth token: {err}");
                            std::process::exit(2);
                        }
                    };
                    let mut cfg = HttpServerConfig::loopback(bind_addr);
                    cfg.bearer_token = bearer_token;
                    cfg.allowed_hosts = http_allow_hosts;
                    cfg.allowed_origins = http_allow_origins;
                    cfg.legacy_session_mode = http_legacy_sessions;
                    if let Err(err) = cfg.validate() {
                        eprintln!("error validating HTTP configuration: {err}");
                        std::process::exit(2);
                    }
                    Some(cfg)
                }
                None => None,
            };
            if let Some(workspace) = in_process {
                run_mcp_in_process(&workspace, policy, http_cfg, audit);
            } else if let Some(target) = socket {
                let (resolved_sock, task_store) = if target.is_dir() {
                    match std::fs::canonicalize(&target) {
                        Ok(canonical) => (
                            tb_ipc::socket_path_for(&canonical),
                            Some(Arc::new(FileTaskStore::for_workspace(&canonical))
                                as Arc<dyn TaskStorePort>),
                        ),
                        Err(err) => {
                            eprintln!("error resolving workspace '{}': {err}", target.display());
                            std::process::exit(2);
                        }
                    }
                } else {
                    (target, None)
                };
                run_mcp_over_socket(&resolved_sock, policy, http_cfg, audit, task_store);
            } else {
                eprintln!(
                    "error: pass <socket-or-workspace> or --in-process <workspace> to '2brain mcp'"
                );
                std::process::exit(2);
            }
        }
    }
}

fn run_analyze(path: &std::path::Path, peel_top_k: usize, limit: usize) {
    let load_start = Instant::now();
    let (_, snap) = match tb_fs_git::load_snapshot(path) {
        Ok(pair) => pair,
        Err(err) => {
            eprintln!("error loading workspace: {err}");
            std::process::exit(1);
        }
    };
    let load_ms = load_start.elapsed().as_secs_f64() * 1000.0;

    let topo_start = Instant::now();
    let topo = tb_cognition::analyze_topology(&snap.graph, peel_top_k, limit);
    let topo_ms = topo_start.elapsed().as_secs_f64() * 1000.0;

    let gap_start = Instant::now();
    let gaps = tb_cognition::detect_structural_gaps(
        &snap.graph,
        &snap.search,
        snap.files.iter().map(|(loc, f)| (loc, f.body())),
        &std::collections::HashSet::new(),
        limit,
    );
    let gap_ms = gap_start.elapsed().as_secs_f64() * 1000.0;

    println!(
        "2brain analyze (load={load_ms:.1} ms, topology={topo_ms:.1} ms, gaps={gap_ms:.1} ms)"
    );
    println!(
        "  graph        nodes={} edges={} communities={} Q={:.4} (null={:.4}±{:.4}, Z={:.2}) NMI={:.3} (min={:.3})",
        topo.node_count,
        topo.edge_count,
        topo.communities.community_count,
        topo.communities.modularity,
        topo.communities.null_mean_modularity,
        topo.communities.null_std_modularity,
        topo.communities.modularity_z_score,
        topo.communities.mean_nmi,
        topo.communities.min_nmi,
    );
    println!("  top_communities:");
    for c in &topo.communities.communities {
        let reps: Vec<&str> = c.representatives.iter().map(|l| l.path.as_str()).collect();
        println!(
            "    #{} size={} ext={} dens={:.3} quadrant={:?} reps={}",
            c.id,
            c.size,
            c.external_edges,
            c.density,
            c.quadrant,
            reps.join(", ")
        );
    }
    println!("  top_gateways (bc/deg):");
    for g in &topo.top_gateways {
        println!(
            "    {} bc={:.4} deg={} ratio={:.4}",
            g.locator.path, g.betweenness, g.degree, g.gateway_ratio
        );
    }
    println!(
        "  peel_the_onion (masked={} residual_communities={}):",
        topo.peel.masked_hubs.len(),
        topo.peel.residual_community_count
    );
    for g in &topo.peel.residual_gateways {
        println!(
            "    {} bc={:.4} deg={} ratio={:.4}",
            g.locator.path, g.betweenness, g.degree, g.gateway_ratio
        );
    }
    println!(
        "  dual_layer_gaps (evaluated={} anti_tautology_filtered={} template_terms_filtered={}):",
        gaps.candidate_pairs_evaluated,
        gaps.filtered_by_anti_tautology,
        gaps.template_terms_filtered
    );
    for gap in &gaps.gaps {
        println!(
            "    [{}] {} <-> {} (sim={:.3}, dist={:?}, terms={})",
            gap.question_id,
            gap.from.path,
            gap.to.path,
            gap.lexical_similarity,
            gap.graph_distance,
            gap.shared_terms.join(",")
        );
    }
    println!("  fragile_hubs (articulation points):");
    for h in &topo.fragile_hubs {
        println!("    {} deg={}", h.locator.path, h.degree);
    }
}

fn run_audit_claims(path: &std::path::Path, as_of: Option<&str>, limit: usize) {
    let started = Instant::now();
    let (_, snap) = match tb_fs_git::load_snapshot(path) {
        Ok(pair) => pair,
        Err(err) => {
            eprintln!("error loading workspace: {err}");
            std::process::exit(1);
        }
    };
    let claims = tb_app::extract_claims(&snap);
    let report = tb_cognition::audit_contradictions(&claims, as_of, limit);
    let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
    println!("2brain audit-claims ({elapsed_ms:.1} ms)");
    println!(
        "  claims       total={} active={} expired={} not_yet_valid={} superseded={} unknown_window={} as_of={}",
        report.temporal_summary.total_claims,
        report.temporal_summary.active,
        report.temporal_summary.expired,
        report.temporal_summary.not_yet_valid,
        report.temporal_summary.superseded,
        report.temporal_summary.unknown_window,
        report.temporal_summary.as_of.as_deref().unwrap_or("none"),
    );
    println!("  conflicts    total={}", report.total_conflicts);
    for c in &report.conflicts {
        println!(
            "    [{}] ({}/{:?}) {}",
            c.conflict_id, c.severity, c.kind, c.explanation
        );
    }
}

fn run_fragility(path: &std::path::Path, limit: usize) {
    let started = Instant::now();
    let (_, snap) = match tb_fs_git::load_snapshot(path) {
        Ok(pair) => pair,
        Err(err) => {
            eprintln!("error loading workspace: {err}");
            std::process::exit(1);
        }
    };
    let kinds: std::collections::BTreeMap<_, _> = snap
        .files
        .iter()
        .map(|(loc, f)| (loc.clone(), f.declared_type.clone()))
        .collect();
    let report = tb_cognition::analyze_fragility(&snap.graph, &kinds, limit);
    let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
    println!("2brain fragility ({elapsed_ms:.1} ms)");
    println!(
        "  summary      nodes={} edges={} articulation_points={} bridges={} overloaded_hubs={}",
        report.total_nodes,
        report.total_edges,
        report.articulation_point_count,
        report.bridge_edge_count,
        report.overloaded_hub_count
    );
    for h in &report.fragile_hubs {
        let hints: Vec<&str> = h.satellite_hints.iter().map(|l| l.path.as_str()).collect();
        println!(
            "    {} deg={} fragments={} detached={} largest_island={} action={} hints=[{}]",
            h.locator.path,
            h.degree,
            h.fragmentation_components,
            h.detached_nodes,
            h.largest_detached_island,
            h.recommended_action,
            hints.join(", ")
        );
    }
}

fn run_diff(path: &std::path::Path, from_ref: &str, to_ref: Option<&str>, limit: usize) {
    let started = Instant::now();
    let before = match tb_fs_git::load_snapshot_at_git_ref(path, from_ref) {
        Ok(snap) => snap,
        Err(err) => {
            eprintln!("error loading baseline git ref '{from_ref}': {err}");
            std::process::exit(1);
        }
    };
    let after = match to_ref {
        Some(r) => match tb_fs_git::load_snapshot_at_git_ref(path, r) {
            Ok(snap) => snap,
            Err(err) => {
                eprintln!("error loading target git ref '{r}': {err}");
                std::process::exit(1);
            }
        },
        None => match tb_fs_git::load_snapshot(path) {
            Ok((_, snap)) => snap,
            Err(err) => {
                eprintln!("error loading working workspace: {err}");
                std::process::exit(1);
            }
        },
    };
    let before_meta = tb_app::snapshot_node_meta(&before);
    let after_meta = tb_app::snapshot_node_meta(&after);
    let diff = tb_cognition::diff_snapshots(
        &before.graph,
        &before_meta,
        &after.graph,
        &after_meta,
        limit,
    );
    let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
    println!(
        "2brain diff {} -> {} ({elapsed_ms:.1} ms)",
        from_ref,
        to_ref.unwrap_or("WORKING_TREE")
    );
    println!(
        "  nodes        +{} -{} ~{} (total {} -> {})",
        diff.added_nodes_count,
        diff.removed_nodes_count,
        diff.modified_nodes_count,
        diff.topology_delta.nodes_before,
        diff.topology_delta.nodes_after,
    );
    println!(
        "  edges        +{} -{} (total {} -> {}, delta_Q={:+.4}, delta_bridges={:+})",
        diff.added_edges_count,
        diff.removed_edges_count,
        diff.topology_delta.edges_before,
        diff.topology_delta.edges_after,
        diff.topology_delta.delta_modularity,
        diff.topology_delta.delta_bridges,
    );
    println!(
        "  decay        candidates={} durable_protected={}",
        diff.decay_candidates.len(),
        diff.durable_protected_count
    );
    for d in &diff.decay_candidates {
        println!(
            "    {} (durable={}): {}",
            d.locator.path, d.durable_kind, d.reason
        );
    }
    println!("  top_movers (betweenness):");
    for m in &diff.top_movers {
        println!(
            "    {} [{}] bc {:.4} -> {:.4} ({:+.4})",
            m.locator.path, m.role, m.bc_before, m.bc_after, m.delta_bc
        );
    }
}

fn run_check(path: &std::path::Path, audit_descriptions: bool, limit: usize, as_json: bool) {
    let started = Instant::now();
    let summary = match tb_fs_git::check_workspace(path) {
        Ok(s) => s,
        Err(err) => {
            eprintln!("error: {err}");
            std::process::exit(1);
        }
    };
    let desc_report = tb_fs_git::load_snapshot(path).ok().map(|(_, snap)| {
        let report_limit = if audit_descriptions { limit } else { 0 };
        tb_app::ingest::audit_descriptions(&snap, None, report_limit)
    });
    let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;

    if as_json {
        let payload = serde_json::json!({
            "repo": summary.repo_name,
            "elapsed_ms": elapsed_ms,
            "corpus": {
                "markdown_files": summary.markdown_files,
                "types_loaded": summary.types_loaded,
            },
            "diagnostics": {
                "schema": summary.schema_diagnostics,
                "broken_links": summary.broken_links,
                "ambiguous_links": summary.ambiguous_links,
                "unresolved_qualified_links": summary.unresolved_qualified_links,
                "utf8_errors": summary.utf8_errors,
                "symlink_refusals": summary.symlink_refusals,
            },
            "graph_shape": summary.shape,
            "graph_health": summary.health,
            "descriptions": desc_report,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&payload).expect("json serialization")
        );
        return;
    }

    println!(
        "2brain check '{}' ({:.1} ms)",
        summary.repo_name, elapsed_ms
    );
    println!(
        "  corpus       {} markdown files, {} types loaded",
        summary.markdown_files, summary.types_loaded
    );
    println!(
        "  diagnostics  {} schema, {} unresolved local links, {} ambiguous links, {} unresolved qualified links, {} utf8 errors, {} symlink refusals",
        summary.schema_diagnostics,
        summary.broken_links,
        summary.ambiguous_links,
        summary.unresolved_qualified_links,
        summary.utf8_errors,
        summary.symlink_refusals
    );
    println!(
        "  graph_shape  nodes={} edges={} (structural={} navigational={}) components={} largest={} isolated={} density={:.2}",
        summary.shape.node_count,
        summary.shape.edge_count,
        summary.shape.edges_structural,
        summary.shape.edges_navigational,
        summary.shape.components,
        summary.shape.largest_component,
        summary.shape.isolated,
        summary.shape.density
    );
    println!(
        "  graph_health cycles={} dead_ends={} sources={} bridges={} articulation_points={} triangles={} transitivity={:.4}",
        summary.health.directed_cycles,
        summary.health.dead_ends,
        summary.health.sources,
        summary.health.bridges.len(),
        summary.health.articulation_points.len(),
        summary.health.triangles,
        summary.health.transitivity
    );
    if let Some(ref rep) = desc_report {
        println!(
            "  descriptions total={} ok={} missing={} weak={} stale={}",
            rep.summary.total,
            rep.summary.ok,
            rep.summary.missing,
            rep.summary.weak,
            rep.summary.stale
        );
        if audit_descriptions {
            for finding in &rep.findings {
                let kind = match finding.kind {
                    tb_app::ingest::DescriptionIssueKind::Missing => "missing",
                    tb_app::ingest::DescriptionIssueKind::Weak => "weak",
                    tb_app::ingest::DescriptionIssueKind::Stale => "stale",
                };
                match &finding.description {
                    Some(d) => println!("    [{kind}] {} ({d})", finding.locator.path),
                    None => println!("    [{kind}] {}", finding.locator.path),
                }
            }
        }
    }
}

fn run_daemon_start(workspace: &std::path::Path) {
    let canonical = match std::fs::canonicalize(workspace) {
        Ok(c) => c,
        Err(err) => {
            eprintln!("error resolving workspace '{}': {err}", workspace.display());
            std::process::exit(1);
        }
    };
    // Run G4 crash-safe startup recovery (rebuilds usage.json if usage.dirty is present).
    if let Err(err) = tb_journal::recover_outcome_registry(&canonical) {
        eprintln!("error recovering outcome registry: {err}");
        std::process::exit(1);
    }

    let (store, snapshot) = match tb_fs_git::load_snapshot(&canonical) {
        Ok(pair) => pair,
        Err(err) => {
            eprintln!("error loading workspace '{}': {err}", canonical.display());
            std::process::exit(1);
        }
    };
    let outcomes = Arc::new(FileOutcomeStore::for_workspace(&canonical));
    let app = Arc::new(App::with_outcomes(store, snapshot, outcomes));
    let sock_path = tb_ipc::socket_path_for(&canonical);

    let runtime = build_runtime();
    let watch_root = canonical.clone();
    let result = runtime.block_on(async move {
        let (_watcher, refresh_task) = spawn_watcher(watch_root, Arc::clone(&app), None);
        let disp: Arc<dyn AppDispatcher> = app;
        let res = tb_ipc::serve_unix_socket(&sock_path, disp).await;
        refresh_task.abort();
        res
    });
    if let Err(err) = result {
        eprintln!("daemon error: {err}");
        std::process::exit(1);
    }
}

fn run_daemon_status(workspace: &std::path::Path) {
    let canonical = match std::fs::canonicalize(workspace) {
        Ok(c) => c,
        Err(err) => {
            eprintln!("error resolving workspace: {err}");
            std::process::exit(1);
        }
    };
    let sock = tb_ipc::socket_path_for(&canonical);
    let client = IpcClient::new(sock.clone());
    match client.call_read("overview", &serde_json::Map::new()) {
        Ok(_) => println!("running: {}", sock.display()),
        Err(_) => {
            println!("stopped: {}", sock.display());
            std::process::exit(4);
        }
    }
}

fn resolve_http_token(token_file: Option<&std::path::Path>) -> Result<Option<String>, String> {
    if let Some(path) = token_file {
        let raw = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read token file '{}': {e}", path.display()))?;
        let token = raw.trim_end_matches(['\r', '\n']).to_owned();
        if token.trim().is_empty() {
            return Err(format!("token file '{}' is empty", path.display()));
        }
        return Ok(Some(token));
    }
    match std::env::var("TWOBRAIN_HTTP_TOKEN") {
        Ok(raw) => {
            let token = raw.trim_end_matches(['\r', '\n']).to_owned();
            if token.trim().is_empty() {
                return Err("TWOBRAIN_HTTP_TOKEN is set but empty".to_owned());
            }
            Ok(Some(token))
        }
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => {
            Err("TWOBRAIN_HTTP_TOKEN is not valid UTF-8".to_owned())
        }
    }
}

struct FileAuditAdapter {
    log: tb_journal::FileAuditLog,
}

impl AuditRecorder for FileAuditAdapter {
    fn check_ready(&self) -> Result<(), String> {
        self.log
            .check_ready()
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    fn record(
        &self,
        principal: &str,
        tool: &str,
        repo: &str,
        path: &str,
        outcome: &str,
        reason_code: &str,
    ) -> Result<(), String> {
        self.log
            .append(&tb_journal::AuditEntryInput {
                principal,
                tool,
                repo,
                path,
                outcome,
                reason_code,
            })
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

fn load_egress_policy(path: &std::path::Path) -> Result<EgressPolicy, String> {
    let raw = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    serde_json::from_str(&raw).map_err(|e| e.to_string())
}

fn run_task_command(command: TaskCommand) {
    match command {
        TaskCommand::Create {
            workspace,
            id,
            title,
            scope_repos,
            obligations,
            max_actions,
            max_tokens,
            by,
            json,
        } => {
            let store = FileTaskStore::for_workspace(&workspace);
            let mut parsed_obs = Vec::with_capacity(obligations.len());
            for raw in &obligations {
                let Some((k, v)) = raw.split_once('=') else {
                    eprintln!("error: invalid --obligation '{raw}' (expected key=value)");
                    std::process::exit(2);
                };
                parsed_obs.push(TaskObligation {
                    key: k.trim().to_owned(),
                    value: v.trim().to_owned(),
                    critical: true,
                });
            }
            let pkt = TaskPacket {
                schema_version: TASK_PACKET_SCHEMA_VERSION.to_owned(),
                task_id: id,
                revision: 0,
                title,
                status: TaskStatus::Open,
                criterion_revision: "2brain.criterion/v1".to_owned(),
                scope_repos,
                obligations: parsed_obs,
                budget: TaskBudget {
                    max_actions,
                    spent_actions: 0,
                    max_tokens,
                    spent_tokens: 0,
                },
                completed_steps: Vec::new(),
                unchecked_steps: Vec::new(),
                rejected_routes: Vec::new(),
                pending_operations: Vec::new(),
                evidence_digests: std::collections::BTreeMap::new(),
                updated_by: by,
                updated_unix: 0,
            };
            match store.create_task(pkt) {
                Ok(saved) => {
                    if json {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&saved).expect("json serialization")
                        );
                    } else {
                        println!(
                            "created task '{}' rev={} status={:?} obligations={}",
                            saved.task_id,
                            saved.revision,
                            saved.status,
                            saved.obligations.len()
                        );
                    }
                }
                Err(err) => {
                    eprintln!("error creating task: {err}");
                    std::process::exit(1);
                }
            }
        }
        TaskCommand::Show {
            workspace,
            id,
            json,
        } => {
            let store = FileTaskStore::for_workspace(&workspace);
            match store.load_task(&id) {
                Ok(pkt) => {
                    if json {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&pkt).expect("json serialization")
                        );
                    } else {
                        println!(
                            "task '{}' rev={} status={:?} by={} spent={}/{} actions, {}/{} tokens",
                            pkt.task_id,
                            pkt.revision,
                            pkt.status,
                            pkt.updated_by,
                            pkt.budget.spent_actions,
                            pkt.budget.max_actions,
                            pkt.budget.spent_tokens,
                            pkt.budget.max_tokens
                        );
                    }
                }
                Err(err) => {
                    eprintln!("error loading task '{id}': {err}");
                    std::process::exit(1);
                }
            }
        }
        TaskCommand::List { workspace, json } => {
            let store = FileTaskStore::for_workspace(&workspace);
            match store.list_tasks() {
                Ok(tasks) => {
                    if json {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&tasks).expect("json serialization")
                        );
                    } else if tasks.is_empty() {
                        println!("no task packets in {}", workspace.display());
                    } else {
                        for pkt in tasks {
                            println!(
                                "{}  rev={}  status={:?}  by={}  title={}",
                                pkt.task_id, pkt.revision, pkt.status, pkt.updated_by, pkt.title
                            );
                        }
                    }
                }
                Err(err) => {
                    eprintln!("error listing tasks: {err}");
                    std::process::exit(1);
                }
            }
        }
        TaskCommand::Checkpoint {
            workspace,
            expected_revision,
            packet_file,
            allow_drop_critical,
            json,
        } => {
            let raw = match std::fs::read_to_string(&packet_file) {
                Ok(s) => s,
                Err(err) => {
                    eprintln!("error reading '{}': {err}", packet_file.display());
                    std::process::exit(2);
                }
            };
            let pkt: TaskPacket = match serde_json::from_str(&raw) {
                Ok(p) => p,
                Err(err) => {
                    eprintln!("error parsing task packet JSON: {err}");
                    std::process::exit(2);
                }
            };
            let task_id = pkt.task_id.clone();
            let store = FileTaskStore::for_workspace(&workspace);
            match store.checkpoint_task(&task_id, expected_revision, pkt, allow_drop_critical, None)
            {
                Ok(saved) => {
                    if json {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&saved).expect("json serialization")
                        );
                    } else {
                        println!(
                            "checkpointed task '{}' rev={} status={:?}",
                            saved.task_id, saved.revision, saved.status
                        );
                    }
                }
                Err(err) => {
                    eprintln!("error checkpointing task '{task_id}': {err}");
                    std::process::exit(1);
                }
            }
        }
    }
}

fn run_procedure_command(command: ProcedureCommand) {
    match command {
        ProcedureCommand::Run {
            workspace,
            spec: spec_path,
            dry_run,
            grants,
            json,
        } => {
            let raw = match std::fs::read_to_string(&spec_path) {
                Ok(s) => s,
                Err(err) => {
                    eprintln!(
                        "error reading procedure spec '{}': {err}",
                        spec_path.display()
                    );
                    std::process::exit(2);
                }
            };
            let mut spec: ProcedureSpec = match serde_json::from_str(&raw) {
                Ok(s) => s,
                Err(err) => {
                    eprintln!("error parsing procedure spec JSON: {err}");
                    std::process::exit(2);
                }
            };
            if dry_run {
                spec.dry_run = true;
            }
            let canonical = match std::fs::canonicalize(&workspace) {
                Ok(c) => c,
                Err(err) => {
                    eprintln!("error resolving workspace '{}': {err}", workspace.display());
                    std::process::exit(1);
                }
            };
            let _ = tb_journal::recover_outcome_registry(&canonical);
            let (store, snapshot) = match tb_fs_git::load_snapshot(&canonical) {
                Ok(pair) => pair,
                Err(err) => {
                    eprintln!("error loading workspace '{}': {err}", canonical.display());
                    std::process::exit(1);
                }
            };
            let outcomes = Arc::new(FileOutcomeStore::for_workspace(&canonical));
            let app = App::with_outcomes(store, snapshot, outcomes);
            let grant_refs: Vec<&str> = grants.iter().map(String::as_str).collect();
            match app.run_procedure(&spec, &grant_refs) {
                Ok(receipt) => {
                    if json {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&receipt).expect("json serialization")
                        );
                    } else {
                        println!(
                            "procedure '{}' dry_run={} operation={} postconditions={} verified_success={} receipt={}",
                            receipt.procedure_id,
                            receipt.dry_run,
                            receipt.operation_outcome,
                            receipt.postcondition_outcome,
                            receipt.verified_success,
                            &receipt.receipt_sha256[..16]
                        );
                    }
                    if !receipt.dry_run && !receipt.verified_success {
                        std::process::exit(1);
                    }
                }
                Err(err) => {
                    eprintln!("procedure error: {err}");
                    std::process::exit(1);
                }
            }
        }
    }
}

fn run_experiment_command(command: ExperimentCommand) {
    match command {
        ExperimentCommand::EvaluateLesson {
            input,
            baseline_grants,
            allowed_scopes,
            json,
        } => {
            #[derive(serde::Deserialize)]
            struct LessonEvalInput {
                lesson: tb_cognition::LessonCandidate,
                observations: Vec<tb_cognition::EpisodeObservation>,
                #[serde(default)]
                policy: tb_cognition::PromotionPolicy,
            }
            let raw = match std::fs::read_to_string(&input) {
                Ok(s) => s,
                Err(err) => {
                    eprintln!("error reading '{}': {err}", input.display());
                    std::process::exit(2);
                }
            };
            let mut parsed: LessonEvalInput = match serde_json::from_str(&raw) {
                Ok(v) => v,
                Err(err) => {
                    eprintln!("error parsing lesson evaluation JSON: {err}");
                    std::process::exit(2);
                }
            };
            if !baseline_grants.is_empty() {
                parsed.policy.baseline_grants = baseline_grants;
            }
            if !allowed_scopes.is_empty() {
                parsed.policy.allowed_scopes = allowed_scopes;
            }
            let report = tb_cognition::evaluate_experience_promotion(
                &parsed.lesson,
                &parsed.observations,
                &parsed.policy,
            );
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&report).expect("json serialization")
                );
            } else {
                println!(
                    "experience '{}' verdict={:?} holdout_tasks={} families={} excluded_training={}",
                    report.lesson_id,
                    report.verdict,
                    report.paired_holdout_tasks,
                    report.holdout_families.len(),
                    report.excluded_training_observations
                );
            }
        }
        ExperimentCommand::EvaluateArm { input, json } => {
            #[derive(serde::Deserialize)]
            struct ArmEvalInput {
                policy: tb_cognition::ExperimentPolicy,
                outcomes: Vec<tb_cognition::PairedTaskOutcome>,
                #[serde(default)]
                null_z_score: Option<f64>,
            }
            let raw = match std::fs::read_to_string(&input) {
                Ok(s) => s,
                Err(err) => {
                    eprintln!("error reading '{}': {err}", input.display());
                    std::process::exit(2);
                }
            };
            let parsed: ArmEvalInput = match serde_json::from_str(&raw) {
                Ok(v) => v,
                Err(err) => {
                    eprintln!("error parsing experiment evaluation JSON: {err}");
                    std::process::exit(2);
                }
            };
            let report = tb_cognition::evaluate_paired_experiment(
                &parsed.policy,
                &parsed.outcomes,
                parsed.null_z_score,
            );
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&report).expect("json serialization")
                );
            } else {
                println!(
                    "experiment '{}' arm='{}' verdict={:?} tasks={} families={}",
                    report.experiment_id,
                    report.arm_name,
                    report.verdict,
                    report.task_count,
                    report.families.len()
                );
            }
        }
        ExperimentCommand::Ppr {
            workspace,
            seeds,
            damping,
            max_iter,
            no_origin_dedup,
            limit,
            json,
        } => {
            let (_, snap) = match tb_fs_git::load_snapshot(&workspace) {
                Ok(pair) => pair,
                Err(err) => {
                    eprintln!("error loading workspace '{}': {err}", workspace.display());
                    std::process::exit(1);
                }
            };
            let default_repo = snap
                .members
                .keys()
                .next()
                .cloned()
                .unwrap_or_else(|| "vault".to_owned());
            let mut seed_locs = Vec::with_capacity(seeds.len());
            for raw in &seeds {
                let (repo, path) = match raw.split_once("::") {
                    Some((r, p)) => (r, p),
                    None => (default_repo.as_str(), raw.as_str()),
                };
                match tb_contracts::Locator::new(repo, path) {
                    Ok(loc) => seed_locs.push(loc),
                    Err(err) => {
                        eprintln!("invalid seed locator '{raw}': {err}");
                        std::process::exit(2);
                    }
                }
            }
            let mut origins = std::collections::BTreeMap::new();
            for (loc, file) in &snap.files {
                let origin_id = if let Some(fm) = file.doc.frontmatter.as_ref()
                    && let Some(origin_val) = fm.get("origin").or_else(|| fm.get("source"))
                    && let tb_syntax::FrontmatterValue::String(s) = origin_val
                    && !s.trim().is_empty()
                {
                    format!("meta:{}", s.trim().to_ascii_lowercase())
                } else {
                    // Content-identity fallback: identical copies without explicit origin metadata
                    // collapse onto one normalized content SHA-256 origin so they cannot amplify rank.
                    let norm_body: String =
                        file.body().split_whitespace().collect::<Vec<_>>().join(" ");
                    let digest =
                        tb_contracts::RevisionDigest::of_bytes(norm_body.as_bytes()).to_hex();
                    format!("content_sha256:{digest}")
                };
                origins.insert(loc.clone(), origin_id);
            }
            let cfg = tb_cognition::PprConfig {
                damping,
                tolerance: 1e-6,
                max_iterations: max_iter,
                deduplicate_origins: !no_origin_dedup,
                top_k: limit.clamp(1, 200),
            };
            let report =
                tb_cognition::run_bounded_ppr_solver(&snap.graph, &seed_locs, &origins, &cfg);
            if matches!(
                report.stop_reason,
                tb_cognition::SolverStopReason::UnresolvedSeeds { .. }
                    | tb_cognition::SolverStopReason::InvalidConfig { .. }
            ) {
                if json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&report).expect("json serialization")
                    );
                } else {
                    eprintln!("ppr error: {:?}", report.stop_reason);
                }
                std::process::exit(2);
            }
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&report).expect("json serialization")
                );
            } else {
                println!(
                    "ppr (diagnostic_only={}) stop={:?} iters={} unique_origins={} top={}",
                    report.diagnostic_only,
                    report.stop_reason,
                    report.iterations_run,
                    report.unique_seed_origins,
                    report.ranked.len()
                );
                for row in &report.ranked {
                    println!(
                        "  {:.6}  {}/{}  [origin={}]",
                        row.score, row.locator.repo, row.locator.path, row.origin_id
                    );
                }
            }
        }
        ExperimentCommand::HybridEval {
            workspace,
            suite,
            model_vectors,
            alpha,
            pool,
            top_k,
            no_negation_guard,
            max_ram_mib,
            json,
        } => run_hybrid_eval(
            &workspace,
            &suite,
            model_vectors.as_deref(),
            alpha,
            pool,
            top_k,
            !no_negation_guard,
            max_ram_mib,
            json,
        ),
    }
}

#[cfg(not(feature = "hybrid-rerank"))]
#[allow(clippy::too_many_arguments)]
fn run_hybrid_eval(
    _workspace: &std::path::Path,
    _suite: &std::path::Path,
    _model_vectors: Option<&std::path::Path>,
    _alpha: f64,
    _pool: usize,
    _top_k: usize,
    _negation_guard: bool,
    _max_ram_mib: usize,
    _json: bool,
) {
    eprintln!(
        "error: '2brain experiment hybrid-eval' requires building with `--features hybrid-rerank` (U14 optional experiment arm is disabled in default serving builds)"
    );
    std::process::exit(2);
}

#[cfg(feature = "hybrid-rerank")]
#[allow(clippy::too_many_arguments)]
fn run_hybrid_eval(
    workspace: &std::path::Path,
    suite: &std::path::Path,
    model_vectors: Option<&std::path::Path>,
    alpha: f64,
    pool: usize,
    top_k: usize,
    negation_guard: bool,
    max_ram_mib: usize,
    as_json: bool,
) {
    use tb_search::hybrid::{
        HybridDocVectors, HybridRerankConfig, StaticEmbeddingModel, rerank_hits,
    };

    #[derive(serde::Deserialize)]
    struct HybridSuite {
        experiment_id: String,
        rollback_ref: String,
        #[serde(default = "default_h_tasks")]
        min_tasks: usize,
        #[serde(default = "default_h_fams")]
        min_families: usize,
        #[serde(default)]
        min_utility_gain: f64,
        #[serde(default = "default_h_overhead")]
        max_token_overhead_ratio: f64,
        #[serde(default)]
        synonym_clusters: Vec<Vec<String>>,
        tasks: Vec<HybridTask>,
    }
    #[derive(serde::Deserialize)]
    struct HybridTask {
        task_id: String,
        family: String,
        query: String,
        expected_paths: Vec<String>,
        #[serde(default)]
        forbidden_top1_paths: Vec<String>,
        #[serde(default = "default_h_budget")]
        budget_cap_tokens: u64,
    }
    #[derive(serde::Deserialize)]
    struct ModelVectorsJson {
        dim: usize,
        vectors: std::collections::BTreeMap<String, Vec<f32>>,
        #[serde(default = "default_true")]
        use_char_ngram_fallback: bool,
    }
    fn default_h_tasks() -> usize {
        4
    }
    fn default_h_fams() -> usize {
        2
    }
    fn default_h_overhead() -> f64 {
        1.15
    }
    fn default_h_budget() -> u64 {
        4000
    }
    fn default_true() -> bool {
        true
    }

    let (_, snap) = match tb_fs_git::load_snapshot(workspace) {
        Ok(pair) => pair,
        Err(err) => {
            eprintln!("error loading workspace '{}': {err}", workspace.display());
            std::process::exit(1);
        }
    };
    let suite_raw = match std::fs::read_to_string(suite) {
        Ok(s) => s,
        Err(err) => {
            eprintln!("error reading suite '{}': {err}", suite.display());
            std::process::exit(2);
        }
    };
    let parsed_suite: HybridSuite = match serde_json::from_str(&suite_raw) {
        Ok(s) => s,
        Err(err) => {
            eprintln!("error parsing hybrid suite JSON: {err}");
            std::process::exit(2);
        }
    };

    let max_ram_bytes = max_ram_mib.saturating_mul(1024 * 1024);
    let model = if let Some(vec_path) = model_vectors {
        let raw = match std::fs::read_to_string(vec_path) {
            Ok(s) => s,
            Err(err) => {
                eprintln!(
                    "error reading model vectors '{}': {err}",
                    vec_path.display()
                );
                std::process::exit(2);
            }
        };
        let mv: ModelVectorsJson = match serde_json::from_str(&raw) {
            Ok(v) => v,
            Err(err) => {
                eprintln!("error parsing model vectors JSON: {err}");
                std::process::exit(2);
            }
        };
        match StaticEmbeddingModel::from_vectors(
            mv.dim,
            mv.vectors,
            mv.use_char_ngram_fallback,
            max_ram_bytes,
        ) {
            Ok(m) => m,
            Err(err) => {
                eprintln!("model validation error: {err}");
                std::process::exit(2);
            }
        }
    } else {
        let cluster_refs: Vec<Vec<&str>> = parsed_suite
            .synonym_clusters
            .iter()
            .map(|c| c.iter().map(String::as_str).collect())
            .collect();
        let slice_refs: Vec<&[&str]> = cluster_refs.iter().map(Vec::as_slice).collect();
        StaticEmbeddingModel::deterministic_subword(128, &slice_refs)
    };

    let index_start = Instant::now();
    let doc_vecs = match HybridDocVectors::build(
        &model,
        snap.files.iter().map(|(loc, f)| (loc.clone(), f.body())),
        max_ram_bytes,
    ) {
        Ok(dv) => dv,
        Err(err) => {
            eprintln!("hybrid RAM budget exceeded: {err}");
            std::process::exit(1);
        }
    };
    let vector_build_ms = index_start.elapsed().as_secs_f64() * 1000.0;
    let total_ram_bytes = model.estimated_ram_bytes() + doc_vecs.estimated_ram_bytes();

    let cfg = HybridRerankConfig {
        alpha,
        candidate_pool: pool,
        negation_guard,
        max_ram_bytes,
    };
    let k = top_k.max(1);

    let mut outcomes = Vec::with_capacity(parsed_suite.tasks.len());
    let mut base_total_us = 0.0f64;
    let mut cand_total_us = 0.0f64;

    for task in &parsed_suite.tasks {
        let t0 = Instant::now();
        let base_hits = snap.search.query(&task.query, false, None, None, k);
        base_total_us += t0.elapsed().as_secs_f64() * 1_000_000.0;

        let t1 = Instant::now();
        // Use the exact same `min_should_match` as baseline so `alpha=0.0` is an exact
        // identity and paired G5 evaluation is unconfounded.
        let pool_hits =
            snap.search
                .query(&task.query, false, None, None, cfg.candidate_pool.max(k));
        let cand_hits = rerank_hits(&task.query, &pool_hits, &model, &doc_vecs, &cfg, k);
        cand_total_us += t1.elapsed().as_secs_f64() * 1_000_000.0;

        let eval_hits = |hits: &[tb_search::SearchHit]| -> (bool, bool, f64, u64) {
            let mut best_rr = 0.0f64;
            for (rank, h) in hits.iter().enumerate() {
                if task.expected_paths.iter().any(|p| p == &h.locator.path) {
                    best_rr = 1.0 / ((rank + 1) as f64);
                    break;
                }
            }
            let fulfilled = best_rr > 0.0;
            let crit = hits.first().is_some_and(|top| {
                task.forbidden_top1_paths
                    .iter()
                    .any(|f| f == &top.locator.path)
            });
            let tokens = 80u64 + (hits.len() as u64) * 120;
            (fulfilled, crit, best_rr, tokens)
        };

        let (b_ok, b_crit, b_rr, b_tok) = eval_hits(&base_hits);
        let (c_ok, c_crit, c_rr, c_tok) = eval_hits(&cand_hits);
        outcomes.push(tb_cognition::PairedTaskOutcome {
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

    let policy = tb_cognition::ExperimentPolicy {
        experiment_id: parsed_suite.experiment_id,
        arm_name: format!(
            "hybrid-rerank(dim={},alpha={alpha:.2},neg_guard={negation_guard})",
            model.dim()
        ),
        rollback_ref: parsed_suite.rollback_ref,
        min_tasks: parsed_suite.min_tasks,
        min_families: parsed_suite.min_families,
        min_utility_gain: parsed_suite.min_utility_gain,
        max_token_overhead_ratio: parsed_suite.max_token_overhead_ratio,
        min_null_z_score: None,
    };
    let evaluation = tb_cognition::evaluate_paired_experiment(&policy, &outcomes, None);

    if as_json {
        let payload = serde_json::json!({
            "feature": "hybrid-rerank",
            "dim": model.dim(),
            "vocab_len": model.vocab_len(),
            "docs_indexed": doc_vecs.len(),
            "vector_build_ms": vector_build_ms,
            "estimated_ram_bytes": total_ram_bytes,
            "max_ram_bytes": max_ram_bytes,
            "baseline_mean_query_us": base_total_us / (parsed_suite.tasks.len().max(1) as f64),
            "candidate_mean_query_us": cand_total_us / (parsed_suite.tasks.len().max(1) as f64),
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
            "hybrid-eval arm='{}' verdict={:?} tasks={} ram_kb={:.1} build_ms={:.2} base_rate={:?} cand_rate={:?} base_mrr={:?} cand_mrr={:?}",
            evaluation.arm_name,
            evaluation.verdict,
            evaluation.task_count,
            (total_ram_bytes as f64) / 1024.0,
            vector_build_ms,
            evaluation.baseline_fulfillment_rate,
            evaluation.candidate_fulfillment_rate,
            evaluation.baseline_mean_rank,
            evaluation.candidate_mean_rank
        );
        for note in &evaluation.notes {
            println!("  note: {note}");
        }
    }
}

fn run_mcp_over_socket(
    socket: &std::path::Path,
    policy: LaunchPolicy,
    http_cfg: Option<HttpServerConfig>,
    audit: Option<Arc<dyn AuditRecorder>>,
    task_store: Option<Arc<dyn TaskStorePort>>,
) {
    let client: Arc<dyn AppDispatcher> = Arc::new(IpcClient::new(socket.to_path_buf()));
    let mut server = match TwoBrainMcp::try_new_with_audit(client, policy, audit) {
        Ok(s) => s,
        Err(err) => {
            eprintln!("mcp init error: {err}");
            std::process::exit(1);
        }
    };
    if let Some(ts) = task_store {
        server = server.with_task_store(ts);
    }
    let runtime = build_runtime();
    let result = match http_cfg {
        Some(cfg) => runtime.block_on(server.serve_http(cfg)),
        None => runtime.block_on(server.serve_stdio()),
    };
    if let Err(err) = result {
        eprintln!("mcp server error: {err}");
        std::process::exit(1);
    }
}

fn run_mcp_in_process(
    workspace: &std::path::Path,
    policy: LaunchPolicy,
    http_cfg: Option<HttpServerConfig>,
    audit: Option<Arc<dyn AuditRecorder>>,
) {
    let canonical = match std::fs::canonicalize(workspace) {
        Ok(c) => c,
        Err(err) => {
            eprintln!("error resolving workspace '{}': {err}", workspace.display());
            std::process::exit(1);
        }
    };
    let _ = tb_journal::recover_outcome_registry(&canonical);
    let (store, snapshot) = match tb_fs_git::load_snapshot(&canonical) {
        Ok(pair) => pair,
        Err(err) => {
            eprintln!("error loading workspace '{}': {err}", canonical.display());
            std::process::exit(1);
        }
    };
    let outcomes = Arc::new(FileOutcomeStore::for_workspace(&canonical));
    let app = Arc::new(App::with_outcomes(store, snapshot, outcomes));
    let disp: Arc<dyn AppDispatcher> = Arc::clone(&app) as Arc<dyn AppDispatcher>;
    let task_store: Arc<dyn TaskStorePort> = Arc::new(FileTaskStore::for_workspace(&canonical));
    let resource_hub = ResourceUpdateHub::new();
    let server = match TwoBrainMcp::try_new_with_audit(disp, policy, audit) {
        Ok(s) => s
            .with_task_store(task_store)
            .with_resource_updates(resource_hub.clone()),
        Err(err) => {
            eprintln!("mcp init error: {err}");
            std::process::exit(1);
        }
    };
    let runtime = build_runtime();
    let result = runtime.block_on(async move {
        let (_watcher, refresh_task) =
            spawn_watcher(canonical, Arc::clone(&app), Some(resource_hub));
        let res = match http_cfg {
            Some(cfg) => server.serve_http(cfg).await,
            None => server.serve_stdio().await,
        };
        refresh_task.abort();
        res
    });
    if let Err(err) = result {
        eprintln!("mcp server error: {err}");
        std::process::exit(1);
    }
}

fn build_runtime() -> tokio::runtime::Runtime {
    match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(err) => {
            eprintln!("error starting runtime: {err}");
            std::process::exit(1);
        }
    }
}

fn is_mutating_fs_event(kind: &notify::EventKind) -> bool {
    use notify::EventKind;
    use notify::event::{AccessKind, AccessMode, ModifyKind};
    match kind {
        EventKind::Create(_) | EventKind::Remove(_) => true,
        EventKind::Modify(ModifyKind::Metadata(_)) => false,
        EventKind::Modify(_) => true,
        EventKind::Access(AccessKind::Close(AccessMode::Write)) => true,
        EventKind::Access(_) | EventKind::Any | EventKind::Other => false,
    }
}

fn is_relevant_watch_path(path: &std::path::Path) -> bool {
    if path.components().any(|c| c.as_os_str() == ".git") {
        return false;
    }
    let lossy = path.to_string_lossy();
    if lossy.contains(".2brain/engine") || lossy.contains(".2brain\\engine") {
        return false;
    }
    let fname = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    fname.ends_with(".md")
        || fname.ends_with(".type.yaml")
        || fname == "repo.yaml"
        || fname == "ignore"
        || fname == "workspace.yaml"
        || path.is_dir()
}

fn diff_changed_locators(
    prev: &tb_app::VaultSnapshot,
    fresh: &tb_app::VaultSnapshot,
) -> Vec<tb_contracts::Locator> {
    let mut changed = Vec::new();
    for (loc, f_new) in &fresh.files {
        match prev.files.get(loc) {
            Some(f_old) if f_old.sha256 == f_new.sha256 => {}
            _ => changed.push(loc.clone()),
        }
    }
    for loc in prev.files.keys() {
        if !fresh.files.contains_key(loc) {
            changed.push(loc.clone());
        }
    }
    changed.sort();
    changed
}

fn spawn_watcher(
    watch_root: PathBuf,
    app: Arc<App<tb_fs_git::FsWorkspaceStore>>,
    resource_hub: Option<ResourceUpdateHub>,
) -> (
    Option<notify::RecommendedWatcher>,
    tokio::task::JoinHandle<()>,
) {
    use std::sync::atomic::{AtomicBool, Ordering};
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<PathBuf>>(64);
    let overflowed = Arc::new(AtomicBool::new(false));
    let wake = Arc::new(tokio::sync::Notify::new());
    let overflowed_cb = Arc::clone(&overflowed);
    let wake_cb = Arc::clone(&wake);
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(ev) = res {
            if !is_mutating_fs_event(&ev.kind) {
                return;
            }
            let relevant: Vec<PathBuf> = ev
                .paths
                .into_iter()
                .filter(|p| is_relevant_watch_path(p))
                .collect();
            if !relevant.is_empty() {
                match tx.try_send(relevant) {
                    Ok(()) => {}
                    Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                        // Record overflow and wake the consumer so a full reload is guaranteed
                        // even if a burst filled the channel mid-scan.
                        overflowed_cb.store(true, Ordering::SeqCst);
                        wake_cb.notify_one();
                    }
                    Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {}
                }
            }
        }
    })
    .ok();
    if let Some(ref mut w) = watcher {
        let _ = w.watch(&watch_root, RecursiveMode::Recursive);
    }
    let task = tokio::spawn(async move {
        loop {
            let mut changed: Vec<PathBuf> = tokio::select! {
                maybe = rx.recv() => match maybe {
                    Some(paths) => paths,
                    None => break,
                },
                () = wake.notified() => Vec::new(),
            };
            tokio::time::sleep(Duration::from_millis(60)).await;
            while let Ok(more) = rx.try_recv() {
                changed.extend(more);
            }
            // Clear overflow BEFORE scanning so any concurrent overflow arriving during the
            // scan sets the flag again and re-notifies `wake` for a follow-up full pass.
            let force_full = overflowed.swap(false, Ordering::SeqCst);
            let prev = app.snapshot();
            let prev_for_diff = Arc::clone(&prev);
            let next_gen = prev.generation.next();
            let root_clone = watch_root.clone();
            let res = tokio::task::spawn_blocking(move || {
                if force_full {
                    tb_fs_git::load_snapshot_with_generation(&root_clone, next_gen)
                } else {
                    tb_fs_git::refresh_snapshot_incremental(&root_clone, &prev, &changed, next_gen)
                }
            })
            .await;
            if let Ok(Ok(fresh)) = res {
                let updated_locs = if resource_hub.is_some() {
                    diff_changed_locators(&prev_for_diff, &fresh)
                } else {
                    Vec::new()
                };
                app.swap_snapshot(fresh);
                if let Some(ref hub) = resource_hub {
                    for loc in &updated_locs {
                        hub.notify_locator(loc);
                    }
                }
            }
        }
    });
    (watcher, task)
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::EventKind;
    use notify::event::{
        AccessKind, AccessMode, CreateKind, DataChange, MetadataKind, ModifyKind, RemoveKind,
        RenameMode,
    };

    #[test]
    fn watcher_filter_ignores_read_access_and_atime_metadata_to_prevent_reload_storm() {
        assert!(!is_mutating_fs_event(&EventKind::Access(AccessKind::Open(
            AccessMode::Any
        ))));
        assert!(!is_mutating_fs_event(&EventKind::Access(
            AccessKind::Close(AccessMode::Read)
        )));
        assert!(!is_mutating_fs_event(&EventKind::Modify(
            ModifyKind::Metadata(MetadataKind::AccessTime)
        )));

        assert!(is_mutating_fs_event(&EventKind::Create(CreateKind::File)));
        assert!(is_mutating_fs_event(&EventKind::Remove(RemoveKind::File)));
        assert!(is_mutating_fs_event(&EventKind::Modify(ModifyKind::Data(
            DataChange::Content
        ))));
        assert!(is_mutating_fs_event(&EventKind::Modify(ModifyKind::Name(
            RenameMode::Both
        ))));
        assert!(is_mutating_fs_event(&EventKind::Access(AccessKind::Close(
            AccessMode::Write
        ))));
    }

    #[test]
    fn diff_changed_locators_detects_added_modified_and_removed_notes() {
        let make_indexed = |path: &str, body: &str| {
            let loc = tb_contracts::Locator::new("vault", path).unwrap();
            let doc = tb_syntax::parse_document(body.as_bytes()).unwrap();
            (
                loc.clone(),
                tb_app::IndexedFile {
                    locator: loc,
                    sha256: tb_contracts::RevisionDigest::of_bytes(body.as_bytes()),
                    short_hash: "1111222233334444".to_owned(),
                    size_bytes: body.len(),
                    raw_text: body.to_owned(),
                    doc,
                    description: None,
                    declared_type: None,
                },
            )
        };
        let mut prev = tb_app::VaultSnapshot::empty();
        let (la, fa) = make_indexed("a.md", "unchanged");
        let (lb, fb) = make_indexed("b.md", "v1");
        let (lc, fc) = make_indexed("c.md", "removed");
        prev.files.insert(la.clone(), fa.clone());
        prev.files.insert(lb.clone(), fb);
        prev.files.insert(lc.clone(), fc);

        let mut fresh = tb_app::VaultSnapshot::empty();
        let (_, fb2) = make_indexed("b.md", "v2");
        let (ld, fd) = make_indexed("d.md", "added");
        fresh.files.insert(la, fa);
        fresh.files.insert(lb.clone(), fb2);
        fresh.files.insert(ld.clone(), fd);

        let diff = diff_changed_locators(&prev, &fresh);
        assert_eq!(diff, vec![lb, lc, ld]);
    }
}
