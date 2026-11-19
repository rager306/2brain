//! `tb-engine migrate identity [--dry-run | --apply] [--root <path>] [--all]`: a report on
//! whether every repository in scope carries the one identity directory (`.2brain/`) and the
//! current engine-schema spelling (`tb.engine.*::tb-engine`).
//!
//! The migration itself is over (ADR-0005): there is one spelling, so there is nothing to
//! rewrite. The command stays as the check an operator runs after an upgrade — it walks the same
//! candidates and reports `current` or `missing`, and `--apply` writes nothing because nothing
//! needs writing. Reads the per-user registry and the filesystem: no daemon, no graph, no network.
//!
//! Exit codes: `0` every candidate is current; `2` a candidate has no `.2brain/repo.yaml`, or the
//! run could not be planned.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use tb_engine::repo::{
    ConfigSource, parse_user_registry, parse_user_workspaces, user_registry_path,
    user_workspaces_path,
};
use tb_parser::IDENTITY_DIR;

/// A candidate is not current, or the run could not be planned at all.
const FAILED: u8 = 2;

/// Report the identity state of every repository in scope.
pub fn identity(dry_run: bool, apply: bool, root: Option<PathBuf>, all: bool) -> ExitCode {
    let candidates = match candidates(root.as_deref(), all) {
        Ok(candidates) => candidates,
        Err(reason) => {
            eprintln!("tb-engine: migrate identity: {reason}");
            return ExitCode::from(FAILED);
        }
    };

    let mut current = 0usize;
    let mut missing = 0usize;
    println!(
        "tb-engine: migrate identity: {} {}",
        candidates.len(),
        plural(candidates.len(), "repository", "repositories"),
    );
    for candidate in &candidates {
        let marker = candidate.path.join(IDENTITY_DIR).join("repo.yaml");
        if marker.is_file() {
            current += 1;
            println!("  {}: current", candidate.label);
        } else {
            missing += 1;
            println!("  {}: missing {}/repo.yaml", candidate.label, IDENTITY_DIR);
        }
    }

    println!();
    if missing == 0 {
        println!(
            "tb-engine: migrate identity: nothing to do, {current} {} current",
            plural(current, "repository is", "repositories are"),
        );
        return ExitCode::SUCCESS;
    }
    if dry_run || !apply {
        eprintln!(
            "tb-engine: migrate identity: {missing} {} missing {IDENTITY_DIR}/repo.yaml; \
             create the identity directory (there is no legacy spelling to migrate)",
            plural(missing, "repository is", "repositories are"),
        );
        return ExitCode::from(FAILED);
    }
    eprintln!(
        "tb-engine: migrate identity: {missing} {} missing {IDENTITY_DIR}/repo.yaml; \
         there is no automatic rewrite — create the identity directory by hand",
        plural(missing, "repository is", "repositories are"),
    );
    ExitCode::from(FAILED)
}

/// One repository considered by the run, with the name it is known by.
struct Candidate {
    path: PathBuf,
    label: String,
}

/// The repositories this run covers: `--root`, every registry path under `--all`, or the current
/// directory when neither is given. Canonical and deduplicated, because a folder-repo can appear
/// in both `repos.yaml` and `workspaces.yaml`.
fn candidates(root: Option<&Path>, all: bool) -> Result<Vec<Candidate>, String> {
    let mut named: Vec<(PathBuf, Option<String>, bool)> = Vec::new();
    if let Some(root) = root {
        named.push((root.to_path_buf(), None, true));
    } else if all {
        let config = ConfigSource::User;
        if let Some(path) = user_registry_path(&config) {
            if let Ok(bytes) = std::fs::read(&path) {
                for (name, location) in parse_user_registry(&bytes) {
                    named.push((location.path, Some(name.0), false));
                }
            }
        }
        if let Some(path) = user_workspaces_path(&config) {
            if let Ok(bytes) = std::fs::read(&path) {
                for (name, entry) in parse_user_workspaces(&bytes) {
                    named.push((entry.clone(), Some(name.clone()), false));
                }
            }
        }
    } else {
        named.push((
            std::env::current_dir().map_err(|e| e.to_string())?,
            None,
            true,
        ));
    }

    let mut out: Vec<Candidate> = Vec::new();
    let mut seen: std::collections::BTreeSet<PathBuf> = std::collections::BTreeSet::new();
    for (path, label, required) in named {
        let canonical = match std::fs::canonicalize(&path) {
            Ok(canonical) => canonical,
            Err(_) if required => {
                return Err(format!("cannot resolve {}", path.display()));
            }
            Err(_) => continue,
        };
        if !seen.insert(canonical.clone()) {
            continue;
        }
        let label = label.unwrap_or_else(|| {
            canonical
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| canonical.display().to_string())
        });
        out.push(Candidate {
            path: canonical,
            label,
        });
    }
    Ok(out)
}

fn plural(count: usize, one: &'static str, many: &'static str) -> &'static str {
    if count == 1 { one } else { many }
}
