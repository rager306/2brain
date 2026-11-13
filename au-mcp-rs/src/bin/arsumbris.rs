//! One binary for the whole arsumbris runtime: the engine daemon and the MCP
//! server, chosen by subcommand.
//!
//! `arsumbris daemon …` delegates to the engine's own implementation
//! (`au_cli`), and `arsumbris mcp …` delegates to this crate's server, so the
//! two cannot drift from their standalone binaries.

use std::{path::PathBuf, process::ExitCode};

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "arsumbris",
    version,
    about = "Ars Umbris engine daemon and MCP server",
    long_about = "One binary for both halves: serve a typed-markdown workspace over the engine wire, \
or expose it to agents over MCP. The engine commands behave exactly like `au`; the MCP command \
behaves exactly like `au-mcp-rs`."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run and manage the per-workspace engine daemon (same as `au daemon`).
    Daemon {
        #[command(subcommand)]
        command: DaemonCommand,
    },
    /// Open a named workspace from the user's workspace index (same as `au open`).
    Open {
        /// The workspace name from `~/.arsumbris/au-engine/config/workspaces.yaml`.
        name: String,
    },
    /// Show a workspace's resolved members without a daemon (same as `au members`).
    Members {
        /// The entry: a folder-repo directory. Defaults to the current directory.
        #[arg(default_value = ".")]
        entry: PathBuf,
        /// Print machine-readable JSON.
        #[arg(long)]
        json: bool,
    },
    /// Serve one engine socket over MCP on stdio (same as `au-mcp-rs`).
    Mcp {
        /// Unix socket of a running engine daemon.
        socket: PathBuf,
        /// JSON policy file: allow_mutations, members, audit.
        #[arg(long)]
        policy: Option<PathBuf>,
        /// Grant the three mutating tools (off by default).
        #[arg(long)]
        allow_mutations: bool,
        /// Label for audit records; not authentication.
        #[arg(long)]
        principal: Option<String>,
        /// Definition detail in `tools/list`.
        #[arg(long, value_parser = ["full", "concise"])]
        tools: Option<String>,
        /// Result representation.
        #[arg(long, value_parser = ["both", "text", "structured"])]
        payload: Option<String>,
    },
    /// Describe the MCP surface, and probe an engine when a socket is given.
    Info {
        /// Unix socket of a running engine daemon.
        socket: Option<PathBuf>,
    },
    /// Serve over MCP on stdio with the same flags as the `au-mcp-rs` binary.
    ///
    /// Kept so an existing configuration can point at `arsumbris` by replacing
    /// only the command name.
    #[command(hide = true)]
    Serve {
        /// Everything after the binary name, passed through unchanged.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        arguments: Vec<String>,
    },
}

#[derive(Subcommand)]
enum DaemonCommand {
    /// Serve the entry's workspace in the foreground.
    Start {
        /// The entry: a folder-repo directory. Defaults to the current directory.
        #[arg(default_value = ".")]
        entry: PathBuf,
    },
    /// Ask a serving daemon to stop.
    Stop {
        /// The entry: a folder-repo directory. Defaults to the current directory.
        #[arg(default_value = ".")]
        entry: PathBuf,
        /// Also reclaim a booting or wedged daemon by signalling its recorded pid.
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

fn main() -> ExitCode {
    let cli = Cli::parse();
    if !matches!(cli.command, Command::Mcp { .. } | Command::Serve { .. }) {
        au_mcp_rs::reset_sigpipe();
    }
    match cli.command {
        Command::Daemon { command } => match command {
            DaemonCommand::Start { entry } => au_cli::daemon::start(&entry),
            DaemonCommand::Stop { entry, force } => au_cli::daemon::stop(&entry, force),
            DaemonCommand::Status { entry } => au_cli::daemon::status(&entry),
        },
        Command::Open { name } => au_cli::daemon::open(&name),
        Command::Members { entry, json } => au_cli::members::members(&entry, json),
        Command::Mcp {
            socket,
            policy,
            allow_mutations,
            principal,
            tools,
            payload,
        } => {
            let mut arguments: Vec<std::ffi::OsString> = vec![socket.into_os_string()];
            if let Some(policy) = policy {
                arguments.push("--policy".into());
                arguments.push(policy.into_os_string());
            }
            if allow_mutations {
                arguments.push("--allow-mutations".into());
            }
            if let Some(principal) = principal {
                arguments.push("--principal".into());
                arguments.push(principal.into());
            }
            if let Some(tools) = tools {
                arguments.push("--tools".into());
                arguments.push(tools.into());
            }
            if let Some(payload) = payload {
                arguments.push("--payload".into());
                arguments.push(payload.into());
            }
            mcp(arguments)
        }
        Command::Info { socket } => {
            let mut arguments: Vec<std::ffi::OsString> = vec!["--info".into()];
            if let Some(socket) = socket {
                arguments.push(socket.into_os_string());
            }
            mcp(arguments)
        }
        Command::Serve { arguments } => mcp(arguments
            .into_iter()
            .map(std::ffi::OsString::from)
            .collect()),
    }
}

/// Run the MCP server half with `arguments` and map its result to an exit code.
fn mcp(arguments: Vec<std::ffi::OsString>) -> ExitCode {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("arsumbris: cannot start a runtime: {error}");
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(au_mcp_rs::run(arguments)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("arsumbris: {error}");
            ExitCode::FAILURE
        }
    }
}
