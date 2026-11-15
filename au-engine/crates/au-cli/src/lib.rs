//! Library surface for the `au` binary. The binary is the per-repo daemon plus
//! its management commands; `main.rs` is a thin clap dispatcher over this.

#[cfg(unix)]
pub mod daemon;

#[cfg(unix)]
pub mod members;

/// Derive a workspace's daemon socket exactly as `au daemon start` does.
///
/// Exposed so a client that spawns over stdio (`2brain mcp <workspace>`) resolves
/// the same path without duplicating the hash rule.
#[cfg(unix)]
pub use au_engine::socket_path;
