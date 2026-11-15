//! One binary for the whole 2brain runtime: the engine daemon and the MCP
//! server, chosen by subcommand.
//!
//! `2brain daemon …` delegates to the engine's own implementation (`tb_cli`),
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
    /// Serve one engine socket over MCP on stdio.
    ///
    /// The first argument is a `.sock` path or a workspace entry; everything after it is handed to
    /// the MCP server unchanged, so `2brain mcp --help` and `2brain mcp --info` are the server's own
    /// CLI rather than a second grammar. That is why the arguments are passed through verbatim
    /// instead of being re-declared here: one binary should not need two spellings of one contract.
    Mcp {
        /// A target (a `.sock` path or a workspace entry) followed by the server's own options.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        arguments: Vec<String>,
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
        tb_mcp_rs::reset_sigpipe();
        println!("2brain {}", tb_mcp_rs::version_string());
        return ExitCode::SUCCESS;
    }
    // `2brain mcp --help|--info|--version` belongs to the MCP server, not to this dispatcher: route
    // the flags straight through so the server's own USAGE answers them (clap would otherwise answer
    // `--help` for the subcommand, giving one contract two spellings).
    let argv: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    if matches!(argv.first().and_then(|a| a.to_str()), Some("mcp"))
        && matches!(argv.get(1).and_then(|a| a.to_str()), Some(flag) if flag.starts_with('-'))
    {
        tb_mcp_rs::reset_sigpipe();
        return mcp(argv[1..].to_vec());
    }
    let cli = Cli::parse();
    if !matches!(cli.command, Command::Mcp { .. } | Command::Serve { .. }) {
        tb_mcp_rs::reset_sigpipe();
    }
    match cli.command {
        Command::Daemon { command } => match command {
            DaemonCommand::Start { entry } => tb_cli::daemon::start(&entry),
            DaemonCommand::Stop { entry, force } => tb_cli::daemon::stop(&entry, force),
            DaemonCommand::Status { entry } => tb_cli::daemon::status(&entry),
        },
        Command::Open { name } => tb_cli::daemon::open(&name),
        Command::Members { entry, json } => tb_cli::members::members(&entry, json),
        Command::Mcp { arguments } => {
            // The target is the first argument unless that argument is itself an option
            // (`--help`, `--info`, …), in which case the server answers it directly.
            let mut rest = arguments.into_iter().peekable();
            let mut forwarded: Vec<std::ffi::OsString> = Vec::with_capacity(rest.len());
            if let Some(first) = rest.peek() {
                if !first.starts_with('-') {
                    let target = PathBuf::from(rest.next().expect("peeked"));
                    match resolve_socket(&target) {
                        Ok(socket) => forwarded.push(socket.into_os_string()),
                        Err(error) => {
                            eprintln!("2brain: {error}");
                            return ExitCode::FAILURE;
                        }
                    }
                }
            }
            forwarded.extend(rest.map(std::ffi::OsString::from));
            mcp(forwarded)
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
/// (`tb_engine::serve::socket_path` documents this), and a symlinked or relative
/// spelling would otherwise hash to a socket nobody bound.
fn resolve_socket(target: &Path) -> Result<PathBuf, String> {
    if target.extension().is_some_and(|extension| extension == "sock") {
        return Ok(target.to_path_buf());
    }
    let canonical = std::fs::canonicalize(target)
        .map_err(|error| format!("{}: {error}", target.display()))?;
    Ok(tb_cli::socket_path(&canonical))
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
    match runtime.block_on(tb_mcp_rs::run(arguments)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("2brain: {error}");
            ExitCode::FAILURE
        }
    }
}
