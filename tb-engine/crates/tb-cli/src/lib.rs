//! Library surface for the `au` binary. The binary is the per-repo daemon plus
//! its management commands; `main.rs` is a thin clap dispatcher over this.

#[cfg(unix)]
pub mod daemon;

#[cfg(unix)]
pub mod members;

#[cfg(unix)]
pub mod migrate;

/// Derive a workspace's daemon socket exactly as `tb-engine daemon start` does.
///
/// Exposed so a client that spawns over stdio (`2brain mcp <workspace>`) resolves
/// the same path without duplicating the hash rule.
#[cfg(unix)]
pub use tb_engine::socket_path;

/// The entry preflight `tb-engine daemon start` runs, re-exported so a client that
/// creates a workspace (`2brain init`) can prove its own output the same way the
/// daemon will.
#[cfg(unix)]
pub use tb_engine::verify_entry;
