//! Composition root producing the single `2brain` binary (MIT).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use clap::{Parser, Subcommand};
use notify::{RecursiveMode, Watcher};
use tb_app::App;
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
    /// Serve MCP 2.0 (2026-07-28 stateless + legacy + SEP-2640 skills) over stdio.
    Mcp {
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
        Command::Check { path } => {
            let started = Instant::now();
            match tb_fs_git::check_workspace(&path) {
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
        Command::Mcp {
            in_process,
            allow_mutations,
            allow_composition,
            allow_workspace_shape,
        } => {
            let Some(workspace) = in_process else {
                eprintln!(
                    "error: pass --in-process <workspace> (Unix-socket daemon transport is wired in Phase 3)"
                );
                std::process::exit(2);
            };
            let (store, snapshot) = match tb_fs_git::load_snapshot(&workspace) {
                Ok(pair) => pair,
                Err(err) => {
                    eprintln!("error loading workspace '{}': {err}", workspace.display());
                    std::process::exit(1);
                }
            };
            let app = Arc::new(App::new(store, snapshot));
            let policy = LaunchPolicy {
                allow_mutations,
                allow_composition,
                allow_workspace_shape,
            };
            let server = TwoBrainMcp::new(Arc::clone(&app), policy);
            let runtime = match tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(err) => {
                    eprintln!("error starting runtime: {err}");
                    std::process::exit(1);
                }
            };
            let watch_root = workspace.clone();
            let result = runtime.block_on(async move {
                // Keep snapshot fresh on external edits via debounced filesystem watcher.
                let (tx, mut rx) = tokio::sync::mpsc::channel::<()>(16);
                let mut watcher =
                    notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
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
                let app_bg = Arc::clone(&app);
                let bg_root = watch_root.clone();
                let refresh_task = tokio::spawn(async move {
                    while rx.recv().await.is_some() {
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        while rx.try_recv().is_ok() {}
                        let next_gen = app_bg.generation().next();
                        let root_clone = bg_root.clone();
                        if let Ok(Ok(fresh)) = tokio::task::spawn_blocking(move || {
                            tb_fs_git::load_snapshot_with_generation(&root_clone, next_gen)
                        })
                        .await
                        {
                            app_bg.swap_snapshot(fresh);
                        }
                    }
                });

                let res = server.serve_stdio().await;
                drop(watcher);
                refresh_task.abort();
                res
            });
            if let Err(err) = result {
                eprintln!("mcp server error: {err}");
                std::process::exit(1);
            }
        }
    }
}
