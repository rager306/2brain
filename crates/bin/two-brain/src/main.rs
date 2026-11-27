//! Composition root producing the single `2brain` binary (MIT).

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
    }
}
