//! Composition root producing the single `2brain` binary (MIT).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use clap::{Parser, Subcommand};
use notify::{RecursiveMode, Watcher};
use tb_app::{App, AppDispatcher};
use tb_ipc::IpcClient;
use tb_journal::FileOutcomeStore;
use tb_mcp::{LaunchPolicy, TwoBrainMcp};

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
    /// Read-only parse, schema validation, and CSR graph health check on a workspace.
    Check {
        /// Path to the workspace directory.
        path: PathBuf,
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
    /// Serve MCP 2.0 (2026-07-28 stateless + legacy + SEP-2640 skills) over stdio.
    Mcp {
        /// Unix socket path of a running `2brain daemon` (omit when using --in-process).
        socket: Option<PathBuf>,
        /// Load the workspace directly in-process (zero IPC daemon needed).
        #[arg(long = "in-process")]
        in_process: Option<PathBuf>,
        /// Enable the 11 core mutating tools (fail-closed by default).
        #[arg(long = "allow-mutations")]
        allow_mutations: bool,
        /// Enable repository dependency mutations (requires --allow-mutations).
        #[arg(long = "allow-composition")]
        allow_composition: bool,
        /// Enable workspace member shape mutations (requires --allow-mutations).
        #[arg(long = "allow-workspace-shape")]
        allow_workspace_shape: bool,
        /// Acknowledge durable outcome-registry tracking for request_id mutations.
        #[arg(long = "outcome-registry")]
        outcome_registry: bool,
        /// Enable the 4 v2 cognitive analytics tools (tb_orient, tb_analyze_topology, tb_detect_gaps, tb_audit_bias).
        #[arg(long = "allow-cognitive")]
        allow_cognitive: bool,
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
        Command::Check { path } => run_check(&path),
        Command::Analyze {
            path,
            peel_top_k,
            limit,
        } => run_analyze(&path, peel_top_k, limit),
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
        },
        Command::Mcp {
            socket,
            in_process,
            allow_mutations,
            allow_composition,
            allow_workspace_shape,
            outcome_registry,
            allow_cognitive,
        } => {
            let policy = LaunchPolicy {
                allow_mutations,
                allow_composition,
                allow_workspace_shape,
                outcome_registry,
                allow_cognitive,
            };
            if let Some(workspace) = in_process {
                run_mcp_in_process(&workspace, policy);
            } else if let Some(sock) = socket {
                run_mcp_over_socket(&sock, policy);
            } else {
                eprintln!(
                    "error: pass either <socket> or --in-process <workspace> to '2brain mcp'"
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

fn run_check(path: &std::path::Path) {
    let started = Instant::now();
    match tb_fs_git::check_workspace(path) {
        Ok(summary) => {
            let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
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
        }
        Err(err) => {
            eprintln!("error: {err}");
            std::process::exit(1);
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
        let (_watcher, refresh_task) = spawn_watcher(watch_root, Arc::clone(&app));
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
            std::process::exit(1);
        }
    }
}

fn run_mcp_over_socket(socket: &std::path::Path, policy: LaunchPolicy) {
    let client: Arc<dyn AppDispatcher> = Arc::new(IpcClient::new(socket.to_path_buf()));
    let server = match TwoBrainMcp::try_new(client, policy) {
        Ok(s) => s,
        Err(err) => {
            eprintln!("mcp init error: {err}");
            std::process::exit(1);
        }
    };
    let runtime = build_runtime();
    if let Err(err) = runtime.block_on(server.serve_stdio()) {
        eprintln!("mcp server error: {err}");
        std::process::exit(1);
    }
}

fn run_mcp_in_process(workspace: &std::path::Path, policy: LaunchPolicy) {
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
    let server = match TwoBrainMcp::try_new(disp, policy) {
        Ok(s) => s,
        Err(err) => {
            eprintln!("mcp init error: {err}");
            std::process::exit(1);
        }
    };
    let runtime = build_runtime();
    let result = runtime.block_on(async move {
        let (_watcher, refresh_task) = spawn_watcher(canonical, Arc::clone(&app));
        let res = server.serve_stdio().await;
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

fn spawn_watcher(
    watch_root: PathBuf,
    app: Arc<App<tb_fs_git::FsWorkspaceStore>>,
) -> (
    Option<notify::RecommendedWatcher>,
    tokio::task::JoinHandle<()>,
) {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<()>(16);
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(ev) = res {
            let relevant = ev.paths.iter().any(|p| {
                !p.components().any(|c| c.as_os_str() == ".git")
                    && !p.to_string_lossy().contains(".2brain/engine")
            });
            if relevant {
                let _ = tx.try_send(());
            }
        }
    })
    .ok();
    if let Some(ref mut w) = watcher {
        let _ = w.watch(&watch_root, RecursiveMode::Recursive);
    }
    let task = tokio::spawn(async move {
        while rx.recv().await.is_some() {
            tokio::time::sleep(Duration::from_millis(50)).await;
            while rx.try_recv().is_ok() {}
            let next_gen = app.generation().next();
            let root_clone = watch_root.clone();
            if let Ok(Ok(fresh)) = tokio::task::spawn_blocking(move || {
                tb_fs_git::load_snapshot_with_generation(&root_clone, next_gen)
            })
            .await
            {
                app.swap_snapshot(fresh);
            }
        }
    });
    (watcher, task)
}
