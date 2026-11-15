//! One binary for the whole 2brain runtime: the engine daemon and the MCP
//! server, chosen by subcommand.
//!
//! `2brain daemon …` delegates to the engine's own implementation (`au_cli`),
//! and `2brain mcp …` delegates to this crate's server, so the two cannot drift
//! from their standalone binaries.
//!
//! `mcp` and `info` take a *target* rather than a socket: a path ending in
//! `.sock` is used as given, anything else is a workspace entry, and the socket
//! is derived exactly as `au daemon start` derives it. A client configuration can
//! therefore say `2brain mcp /root/vault` and never carry a hash that changes
//! with the entry path.

use std::{
    path::{Path, PathBuf},
    process::ExitCode,
};

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "2brain",
    version,
    about = "2brain engine daemon and MCP server",
    long_about = "One binary for both halves: serve a typed-markdown workspace over the engine wire, \
or expose it to agents over MCP. The engine commands behave exactly like `au`; the MCP command \
behaves exactly like the `au-mcp-rs` binary."
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
        /// A `.sock` path, or a workspace entry whose socket is derived.
        #[arg(value_name = "SOCKET_OR_WORKSPACE")]
        target: PathBuf,
        /// JSON policy file: allow_mutations, members, audit.
        #[arg(long)]
        policy: Option<PathBuf>,
        /// Grant the mutating tools (off by default).
        #[arg(long)]
        allow_mutations: bool,
        /// Grant composition tools (add_dep, remove_dep); needs mutations.
        #[arg(long)]
        allow_composition: bool,
        /// Grant workspace-shape tools (members, roles); needs mutations.
        #[arg(long)]
        allow_workspace_shape: bool,
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
    /// Describe the MCP surface, and probe an engine when a target is given.
    Info {
        /// A `.sock` path, or a workspace entry whose socket is derived.
        target: Option<PathBuf>,
    },
    /// Serve over MCP on stdio with the same flags as the `au-mcp-rs` binary.
    ///
    /// Kept so an existing configuration can point at `2brain` by replacing
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
    // clap would print the crate version alone; report the build identity too.
    if std::env::args()
        .skip(1)
        .any(|argument| argument == "--version" || argument == "-V")
    {
        au_mcp_rs::reset_sigpipe();
        println!("2brain {}", au_mcp_rs::version_string());
        return ExitCode::SUCCESS;
    }
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
            target,
            policy,
            allow_mutations,
            allow_composition,
            allow_workspace_shape,
            principal,
            tools,
            payload,
        } => {
            let socket = match resolve_socket(&target) {
                Ok(socket) => socket,
                Err(error) => {
                    eprintln!("2brain: {error}");
                    return ExitCode::FAILURE;
                }
            };
            let mut arguments: Vec<std::ffi::OsString> = vec![socket.into_os_string()];
            if let Some(policy) = policy {
                arguments.push("--policy".into());
                arguments.push(policy.into_os_string());
            }
            if allow_mutations {
                arguments.push("--allow-mutations".into());
            }
            if allow_composition {
                arguments.push("--allow-composition".into());
            }
            if allow_workspace_shape {
                arguments.push("--allow-workspace-shape".into());
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
        Command::Info { target } => {
            let mut arguments: Vec<std::ffi::OsString> = vec!["--info".into()];
            if let Some(target) = target {
                match resolve_socket(&target) {
                    Ok(socket) => arguments.push(socket.into_os_string()),
                    Err(error) => {
                        eprintln!("2brain: {error}");
                        return ExitCode::FAILURE;
                    }
                }
            }
            mcp(arguments)
        }
        Command::Serve { arguments } => mcp(arguments
            .into_iter()
            .map(std::ffi::OsString::from)
            .collect()),
    }
}

/// The socket a target names: a `.sock` path as given, anything else a workspace
/// entry whose socket is derived the way `au daemon start` derives it.
///
/// The entry is canonicalized first because the daemon hashes a canonical path
/// (`au_engine::serve::socket_path` documents this), and a symlinked or relative
/// spelling would otherwise hash to a socket nobody bound.
fn resolve_socket(target: &Path) -> Result<PathBuf, String> {
    if target.extension().is_some_and(|extension| extension == "sock") {
        return Ok(target.to_path_buf());
    }
    let canonical = std::fs::canonicalize(target)
        .map_err(|error| format!("{}: {error}", target.display()))?;
    Ok(au_cli::socket_path(&canonical))
}

/// Run the MCP server half with `arguments` and map its result to an exit code.
fn mcp(arguments: Vec<std::ffi::OsString>) -> ExitCode {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("2brain: cannot start a runtime: {error}");
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(au_mcp_rs::run(arguments)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("2brain: {error}");
            ExitCode::FAILURE
        }
    }
}
