use std::process::ExitCode;

#[cfg(unix)]
use std::path::PathBuf;

#[cfg(unix)]
use clap::{Parser, Subcommand};

/// The `tb-engine` binary: the per-repo daemon and its management commands. Consumers
/// start a daemon and speak the wire (`crates/tb-engine/WIRE.md`) themselves.
#[cfg(unix)]
#[derive(Parser)]
#[command(name = "tb-engine", version, about = "2brain engine daemon")]
struct Cli {
    #[command(subcommand)]
    command: TopCommand,
}

#[cfg(unix)]
#[derive(Subcommand)]
enum TopCommand {
    /// Run and manage the per-repo daemon.
    Daemon {
        #[command(subcommand)]
        cmd: DaemonCommand,
    },
    /// Open a named workspace: resolve it through the per-user
    /// `<state root>/engine/config/workspaces.yaml` index to its workspace-repo folder,
    /// then serve it in the foreground like `daemon start <folder>`.
    Open {
        /// The workspace name, an entry in `workspaces.yaml` (an explicit `name`,
        /// else the folder's basename).
        name: String,
    },
    /// Resolve the entry's declared members without a daemon: each member's role,
    /// the tier that located it (entry / sibling / registry / cache / unmounted /
    /// disabled), its path, and why an unmounted one did not mount. Reads only.
    Members {
        /// The entry: a folder-repo directory. Defaults to the current directory.
        #[arg(default_value = ".")]
        entry: PathBuf,
        /// Print the JSON contract instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// One-shot cold build of the entry with a hotpath profile report on exit.
    /// Requires the `hotpath` cargo feature; without it, a plain build + stats.
    #[cfg(feature = "hotpath")]
    BuildProfile {
        /// The entry: a folder-repo directory. Defaults to the current directory.
        #[arg(default_value = ".")]
        entry: PathBuf,
    },
    /// Check that repositories carry the identity directory.
    Migrate {
        #[command(subcommand)]
        cmd: MigrateCommand,
    },
}

/// The identity check: with one spelling (ADR-0005) there is nothing to migrate, so this
/// reports whether every repository in scope carries `.2brain/repo.yaml`.
#[cfg(unix)]
#[derive(Subcommand)]
enum MigrateCommand {
    /// Report whether every repository in scope is current.
    ///
    /// Reads the per-user registry and the filesystem only: no daemon, no graph, no
    /// network. A repository with `.2brain/repo.yaml` is `current`; one without is refused.
    /// `--apply` writes nothing, because there is no legacy spelling left to rewrite.
    Identity {
        /// Report and write nothing.
        #[arg(long, conflicts_with = "apply")]
        dry_run: bool,
        /// Accepted for compatibility; there is nothing to write.
        #[arg(long)]
        apply: bool,
        /// One repository root to include. Defaults to the current directory when neither
        /// --root nor --all is given.
        #[arg(long, value_name = "PATH")]
        root: Option<PathBuf>,
        /// Include every repository the per-user registry knows about.
        #[arg(long)]
        all: bool,
    },
}

/// Per-workspace daemon lifecycle. The daemon serves the engine's reads and
/// subscriptions over a Unix socket at a hashed path under `<state root>/engine/run/`.
#[cfg(unix)]
#[derive(Subcommand)]
enum DaemonCommand {
    /// Boot a daemon over the entry and serve in the foreground until stopped.
    Start {
        /// The entry: a folder-repo directory (a repo carrying
        /// `.2brain/repo.yaml`, composed by its optional
        /// `.2brain/workspace.yaml`). Defaults to the current directory.
        #[arg(default_value = ".")]
        entry: PathBuf,
    },
    /// Ask the daemon serving the entry to shut down.
    Stop {
        /// The entry: a folder-repo directory. Defaults to the current directory.
        #[arg(default_value = ".")]
        entry: PathBuf,
        /// Also reclaim a booting or wedged daemon that has not bound its
        /// socket, by signalling its recorded pid (a graceful window, then
        /// SIGTERM, then SIGKILL). Plain `stop` reaches only a serving daemon
        /// over the socket.
        #[arg(long)]
        force: bool,
    },
    /// Report whether a daemon is serving the entry.
    Status {
        /// The entry: a folder-repo directory. Defaults to the current directory.
        #[arg(default_value = ".")]
        entry: PathBuf,
    },
}

#[cfg(unix)]
fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        TopCommand::Daemon { cmd } => match cmd {
            DaemonCommand::Start { entry } => tb_cli::daemon::start(&entry),
            DaemonCommand::Stop { entry, force } => tb_cli::daemon::stop(&entry, force),
            DaemonCommand::Status { entry } => tb_cli::daemon::status(&entry),
        },
        TopCommand::Open { name } => tb_cli::daemon::open(&name),
        TopCommand::Members { entry, json } => tb_cli::members::members(&entry, json),
        TopCommand::Migrate { cmd } => match cmd {
            MigrateCommand::Identity {
                dry_run,
                apply,
                root,
                all,
            } => tb_cli::migrate::identity(dry_run, apply, root, all),
        },
        #[cfg(feature = "hotpath")]
        TopCommand::BuildProfile { entry } => tb_cli::daemon::build_profile(&entry),
    }
}

#[cfg(not(unix))]
fn main() -> ExitCode {
    eprintln!("tb-engine: only Unix is supported — the daemon serves over a Unix domain socket");
    ExitCode::FAILURE
}
