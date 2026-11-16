//! `au migrate identity [--dry-run] [--root <path>] [--all]`: tier C's on-disk identity
//! migration, as a plan plus its refusals.
//!
//! Two spellings move together: the repository's identity directory, `.arsumbris/` -> `.2brain/`
//! (a rename), and the engine-schema claims it writes, `au.engine.*::au-engine` ->
//! `tb.engine.*::tb-engine` (`tb_engine::rewrite_engine_claims`, an exact-byte splice). See
//! `docs/research/tier-c-seams.md` section 4.
//!
//! Reads the per-user registry (`repos.yaml`, `workspaces.yaml`) and the filesystem: no daemon,
//! no graph, no network. `--dry-run` prints the plan and writes nothing; this build PLANS only,
//! so a plan that needs writing is refused rather than half-applied — the apply step (`git mv`,
//! the splices, one commit per repository) is a later commit.
//!
//! A repository is IN SCOPE when its plan is non-empty, and the plan derives only from the
//! legacy spelling (`.arsumbris/` present, or a claim that still spells `au.engine.*` /
//! `::au-engine`). A migrated repository therefore derives an empty plan, so a second run writes
//! nothing and exits `0`.
//!
//! Exit codes: `0` a plan (including the empty one) was printed, `2` a repository was refused or
//! the run could not be planned.

use std::collections::BTreeSet;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use tb_engine::repo::{
    ConfigSource, parse_user_registry, parse_user_workspaces, user_registry_path,
    user_workspaces_path,
};
use tb_engine::{rewrite_engine_claims, socket_path};
use tb_parser::{FileSystem, RealFileSystem, WalkFilter};

/// A repository in scope was refused, or the run could not be planned at all.
const FAILED: u8 = 2;

/// The files inside a repository's identity directory that carry an engine-schema `type:` claim.
const IDENTITY_CLAIM_FILES: &[&str] =
    &["repo.yaml", "workspace.yaml", "repo.lock", "workspace.lock"];

/// The extensions a claim can hide in: a type-def's `extends:`, a README's `type:`. Everything
/// else is an asset and is never read.
const CLAIM_EXTENSIONS: &[&str] = &["md", "markdown", "yaml", "yml"];

/// Print the plan for every repository in scope, or refuse with a named reason.
pub fn identity(dry_run: bool, root: Option<PathBuf>, all: bool) -> ExitCode {
    let candidates = match candidates(root.as_deref(), all) {
        Ok(candidates) => candidates,
        Err(reason) => {
            eprintln!("au: migrate identity: {reason}");
            return ExitCode::from(FAILED);
        }
    };
    let plans: Vec<RepoPlan> = candidates
        .iter()
        .map(|candidate| plan(&candidate.path, &candidate.label))
        .collect();

    println!(
        "au: migrate identity: plan for {} {}",
        plans.len(),
        plural(plans.len(), "repository", "repositories"),
    );
    for repo_plan in &plans {
        print_plan(repo_plan);
    }
    summarize(&plans, dry_run)
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
    // The label is the name the repository is KNOWN by: a registry name when one is given, else
    // the directory's own name. `required`: an explicitly named root must resolve, while a
    // registry entry that has gone away is reported as skipped — the registry is advisory by
    // contract.
    let mut named: Vec<(PathBuf, Option<String>, bool)> = Vec::new();
    if let Some(root) = root {
        named.push((root.to_path_buf(), None, true));
    }
    if all {
        if let Err(e) = tb_engine::device_root() {
            return Err(format!("cannot read the per-user registry: {e}"));
        }
        let source = ConfigSource::User;
        let registry = user_registry_path(&source)
            .and_then(|path| std::fs::read(path).ok())
            .map(|bytes| parse_user_registry(&bytes))
            .unwrap_or_default();
        for (name, location) in registry {
            named.push((location.path, Some(name.as_str().to_string()), false));
        }
        let workspaces = user_workspaces_path(&source)
            .and_then(|path| std::fs::read(path).ok())
            .map(|bytes| parse_user_workspaces(&bytes))
            .unwrap_or_default();
        for (name, path) in workspaces {
            named.push((path, Some(name), false));
        }
    }
    if named.is_empty() {
        named.push((PathBuf::from("."), None, true));
    }

    let mut seen: BTreeSet<PathBuf> = BTreeSet::new();
    let mut unique: Vec<Candidate> = Vec::new();
    for (path, label, required) in named {
        let label = label.unwrap_or_else(|| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string())
        });
        match std::fs::canonicalize(&path) {
            Ok(path) => {
                if !seen.insert(path.clone()) {
                    continue;
                }
                unique.push(Candidate { path, label });
            }
            Err(e) if required => {
                return Err(format!("cannot resolve {}: {e}", path.display()));
            }
            Err(_) => unique.push(Candidate { path, label }),
        }
    }
    Ok(unique)
}

/// What the run would do to one repository.
struct RepoPlan {
    root: PathBuf,
    label: String,
    rename: Option<Rename>,
    rewrites: Vec<FileRewrite>,
    outcome: Outcome,
}

/// The directory rename the migration performs, both sides relative to the repository root.
struct Rename {
    from: PathBuf,
    to: PathBuf,
}

/// One claim line in one file, ready to print as `file:line before -> after`.
struct FileRewrite {
    file: PathBuf,
    line: usize,
    before: String,
    after: String,
}

enum Outcome {
    /// There is work to do.
    Planned,
    /// The identity directory is already the new name and no legacy claim remains.
    AlreadyMigrated,
    /// The repository cannot be migrated now, and the reason names why.
    Refused(String),
    /// The path is not a repository this tool can plan for.
    Skipped(String),
}

impl RepoPlan {
    fn refused(root: &Path, label: &str, reason: String) -> Self {
        Self {
            root: root.to_path_buf(),
            label: label.to_string(),
            rename: None,
            rewrites: Vec::new(),
            outcome: Outcome::Refused(reason),
        }
    }

    fn skipped(root: &Path, label: &str, reason: String) -> Self {
        Self {
            root: root.to_path_buf(),
            label: label.to_string(),
            rename: None,
            rewrites: Vec::new(),
            outcome: Outcome::Skipped(reason),
        }
    }
}

/// Build one repository's plan: the rename it needs, every claim line to splice, or the named
/// reason it is refused. Reads only — nothing here writes, and nothing here creates `.2brain`.
fn plan(root: &Path, label: &str) -> RepoPlan {
    if !root.is_dir() {
        return RepoPlan::skipped(root, label, "path does not exist".to_string());
    }
    let legacy = root.join(tb_parser::LEGACY_IDENTITY_DIR);
    let current = root.join(tb_parser::IDENTITY_DIR);
    let legacy_repo = has_marker(&legacy);
    let current_repo = has_marker(&current);
    if legacy_repo && current_repo {
        return RepoPlan::refused(
            root,
            label,
            "two identity directories, .arsumbris and .2brain, both holding a repo.yaml; \
             resolve the half-finished move first"
                .to_string(),
        );
    }
    if !legacy_repo && !current_repo {
        return RepoPlan::skipped(
            root,
            label,
            "no identity directory (.arsumbris or .2brain holding a repo.yaml)".to_string(),
        );
    }
    if let Some(reason) = refusal(root) {
        return RepoPlan::refused(root, label, reason);
    }

    let identity = if current_repo { current } else { legacy };
    let rename = legacy_repo.then(|| Rename {
        from: PathBuf::from(tb_parser::LEGACY_IDENTITY_DIR),
        to: PathBuf::from(tb_parser::IDENTITY_DIR),
    });

    let mut rewrites = Vec::new();
    for name in IDENTITY_CLAIM_FILES {
        collect_rewrites(root, &identity.join(name), &mut rewrites);
    }
    let walk = match RealFileSystem.walk_files(root, &WalkFilter::default_excludes(root)) {
        Ok(walk) => walk,
        Err(e) => {
            return RepoPlan::refused(root, label, format!("cannot walk the repository: {e}"));
        }
    };
    let mut files: Vec<PathBuf> = walk
        .files
        .into_iter()
        .filter(|file| is_claim_file(file))
        .collect();
    files.sort();
    for file in files {
        collect_rewrites(root, &file, &mut rewrites);
    }

    let outcome = if rename.is_some() || !rewrites.is_empty() {
        Outcome::Planned
    } else {
        Outcome::AlreadyMigrated
    };
    RepoPlan {
        root: root.to_path_buf(),
        label: label.to_string(),
        rename,
        rewrites,
        outcome,
    }
}

/// The marker presence that makes a directory a repository, under either spelling.
fn has_marker(identity_dir: &Path) -> bool {
    identity_dir.join(tb_parser::IDENTITY_MARKER).is_file()
}

/// The three refusals the migration makes before it plans, plus one structural precondition: the
/// root must be in a git work tree, because the migration commits what it rewrites.
///
/// The root need NOT be the work-tree root: a monorepo holds many member repositories in one work
/// tree, and tier C migrates those one member at a time (`docs/research/tier-c-seams.md` step 5).
/// The clean-tree check covers the whole work tree either way, so a member is never migrated out
/// of a tree that is carrying someone else's unfinished edit.
fn refusal(root: &Path) -> Option<String> {
    if let Err(reason) = git_toplevel(root) {
        return Some(reason);
    }

    let status = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["status", "--porcelain"])
        .output();
    match status {
        Err(e) => return Some(format!("cannot read the git status: {e}")),
        Ok(out) if !out.status.success() => {
            return Some("cannot read the git status: git exited non-zero".to_string());
        }
        Ok(out) if !out.stdout.is_empty() => {
            return Some("dirty working tree: `git status --porcelain` is not empty".to_string());
        }
        Ok(_) => {}
    }

    if let Some(marker) = tb_engine::in_flight_saga_marker(root) {
        return Some(format!("a mutation is in flight: {}", marker.display()));
    }

    let socket = socket_path(root);
    if UnixStream::connect(&socket).is_ok() {
        return Some(format!(
            "a daemon is serving this root: {}",
            socket.display()
        ));
    }
    None
}

/// The work-tree root `root` belongs to, canonical, or the reason git cannot answer.
fn git_toplevel(root: &Path) -> Result<PathBuf, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .map_err(|e| format!("git is unavailable: {e}"))?;
    if !out.status.success() {
        return Err("not a git work tree".to_string());
    }
    let toplevel = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Ok(std::fs::canonicalize(&toplevel).unwrap_or_else(|_| PathBuf::from(toplevel)))
}

/// The content files a claim can live in. The identity files are added by name, since the walk
/// floors their directory.
fn is_claim_file(file: &Path) -> bool {
    file.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            CLAIM_EXTENSIONS
                .iter()
                .any(|known| extension.eq_ignore_ascii_case(known))
        })
}

/// Read one file, splice its claims, and record each rewritten line relative to the root. A file
/// that is absent, not UTF-8, or carries no legacy claim contributes nothing.
fn collect_rewrites(root: &Path, file: &Path, out: &mut Vec<FileRewrite>) {
    let Ok(bytes) = std::fs::read(file) else {
        return;
    };
    let Ok(text) = std::str::from_utf8(&bytes) else {
        return;
    };
    let Some((_, lines)) = rewrite_engine_claims(file, text) else {
        return;
    };
    let relative = file.strip_prefix(root).unwrap_or(file).to_path_buf();
    for line in lines {
        out.push(FileRewrite {
            file: relative.clone(),
            line: line.line,
            before: line.before,
            after: line.after,
        });
    }
}

fn print_plan(plan: &RepoPlan) {
    println!();
    println!("{}  {}", plan.label, plan.root.display());
    match &plan.outcome {
        Outcome::Planned => {
            if let Some(rename) = &plan.rename {
                println!(
                    "  rename   {} -> {}",
                    rename.from.display(),
                    rename.to.display()
                );
            }
            for rewrite in &plan.rewrites {
                println!(
                    "  rewrite  {}:{}  {} -> {}",
                    rewrite.file.display(),
                    rewrite.line,
                    rewrite.before,
                    rewrite.after,
                );
            }
            println!(
                "  plan     {} {}",
                plan.rewrites.len(),
                plural(plan.rewrites.len(), "claim line", "claim lines"),
            );
        }
        Outcome::AlreadyMigrated => println!("  already migrated"),
        Outcome::Refused(reason) => println!("  refused  {reason}"),
        Outcome::Skipped(reason) => println!("  skipped  {reason}"),
    }
}

/// Count the outcomes and state what happens next. An empty plan exits `0`: a second run over a
/// migrated tree must be a no-op, not an error.
fn summarize(plans: &[RepoPlan], dry_run: bool) -> ExitCode {
    let mut planned = 0usize;
    let mut already = 0usize;
    let mut refused = 0usize;
    let mut skipped = 0usize;
    for plan in plans {
        match &plan.outcome {
            Outcome::Planned => planned += 1,
            Outcome::AlreadyMigrated => already += 1,
            Outcome::Refused(_) => refused += 1,
            Outcome::Skipped(_) => skipped += 1,
        }
    }

    println!();
    if planned == 0 && refused == 0 && skipped == 0 {
        println!(
            "au: migrate identity: nothing to do, every repository in scope is already migrated"
        );
        return ExitCode::SUCCESS;
    }
    println!(
        "au: migrate identity: {planned} to migrate, {already} already migrated, {refused} refused, \
         {skipped} skipped",
    );
    if dry_run {
        println!("au: migrate identity: dry run, nothing was written");
        return if refused > 0 {
            ExitCode::from(FAILED)
        } else {
            ExitCode::SUCCESS
        };
    }
    if planned > 0 {
        eprintln!(
            "au: migrate identity: applying is not implemented in this build; nothing was written \
             (re-run with --dry-run to print the plan)"
        );
        return ExitCode::from(FAILED);
    }
    if refused > 0 {
        ExitCode::from(FAILED)
    } else {
        ExitCode::SUCCESS
    }
}

fn plural(count: usize, one: &'static str, many: &'static str) -> &'static str {
    if count == 1 { one } else { many }
}
