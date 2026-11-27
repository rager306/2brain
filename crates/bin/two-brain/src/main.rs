//! Composition root producing the single `2brain` binary (MIT).

use std::path::PathBuf;
use std::time::Instant;

use clap::{Parser, Subcommand};

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
    }
}
