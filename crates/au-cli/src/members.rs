//! `au members [entry] [--json]`: which of an entry's declared members mount, where,
//! and through which resolution tier, answered WITHOUT a daemon.
//!
//! A launcher checks a workspace before it starts the engine. This runs the
//! build's own composition over the entry (`au_engine::members::resolve_members`),
//! with the inputs a daemon over the same entry would use: the per-user
//! `~/.arsumbris/au-engine/config/repos.yaml` registry and the device package cache.
//! So the answer is what `au daemon start` would mount, never a re-derivation.
//!
//! Reads only: no write, no registry change, no fetch.
//!
//! Exit codes match `au daemon`: `0` answered (unmounted members included, they
//! are an answer, not a failure), `2` an environment or IO error (`$HOME` unset,
//! as `au daemon start` refuses too, or a non-UTF-8 path under `--json`), `5` the
//! entry is not a folder-repo.

use std::path::Path;
use std::process::ExitCode;

use au_engine::members::{resolve_members, MemberResolution};
use au_engine::{BuildError, ConfigSource, SCHEMA_VERSION};
use au_parser::RealFileSystem;

const ENV_ERROR: u8 = 2;
const NOT_A_REPO: u8 = 5;

/// Resolve the entry's members and print them, a table by default, the JSON
/// contract with `json`.
pub fn members(entry: &Path, json: bool) -> ExitCode {
    let entry = match std::fs::canonicalize(entry) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("au: cannot resolve entry {}: {e}", entry.display());
            return ExitCode::from(ENV_ERROR);
        }
    };
    // The same preflight `au daemon start` runs: the registry and the package
    // cache both live under `$HOME`, so without it the answer would silently
    // differ from what the daemon mounts.
    if let Err(e) = au_engine::device_root() {
        eprintln!("au: cannot resolve members of {}: {e}", entry.display());
        return ExitCode::from(ENV_ERROR);
    }
    let registry = au_engine::repo::load_user_registry(&ConfigSource::User, &RealFileSystem);
    let cache_root = au_engine::default_package_cache_root();
    let resolution =
        match resolve_members(&entry, &RealFileSystem, &registry, cache_root.as_deref()) {
            Ok(r) => r,
            Err(e @ (BuildError::EntryNotARepo { .. } | BuildError::EntryReserved { .. })) => {
                eprintln!("au: cannot resolve members of {}: {e}", entry.display());
                return ExitCode::from(NOT_A_REPO);
            }
            Err(e) => {
                eprintln!("au: cannot resolve members of {}: {e}", entry.display());
                return ExitCode::from(ENV_ERROR);
            }
        };
    if json {
        if let Err(e) = print_json(&resolution) {
            eprintln!("au: cannot serialize members of {}: {e}", entry.display());
            return ExitCode::from(ENV_ERROR);
        }
    } else {
        print_table(&resolution);
    }
    ExitCode::SUCCESS
}

/// The JSON contract: the resolution with the wire's `schema_version`, so a
/// consumer checks it against the same version it checks the daemon against.
///
/// Fails on a path JSON cannot carry, a non-UTF-8 member root.
fn print_json(r: &MemberResolution) -> Result<(), serde_json::Error> {
    let mut value = serde_json::to_value(r)?;
    value["schema_version"] = SCHEMA_VERSION.into();
    println!("{value}");
    Ok(())
}

fn print_table(r: &MemberResolution) {
    let width = r.members.iter().map(|m| m.name.len()).max().unwrap_or(0);
    for m in &r.members {
        let mut line = format!("{:width$}  {:8}  {:9}", m.name, m.role, m.tier);
        if let Some(path) = &m.path {
            line.push_str(&format!("  {}", path.display()));
        }
        if let Some(code) = &m.code {
            line.push_str(&format!("  [{code}]"));
        }
        println!("{}", line.trim_end());
    }
    if !r.complete {
        println!("incomplete, run resolve to fetch:");
        for d in &r.incomplete {
            println!("  {}", d.message);
        }
    }
}
