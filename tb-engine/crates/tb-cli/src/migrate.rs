//! `au migrate identity [--dry-run | --apply] [--root <path>] [--all]`: tier C's on-disk
//! identity migration, as a plan plus, with `--apply`, the write.
//!
//! Two spellings move together: the repository's identity directory, `.arsumbris/` -> `.2brain/`
//! (a `git mv`, so the index carries it), and the engine-schema claims it writes,
//! `au.engine.*::au-engine` -> `tb.engine.*::tb-engine` (`tb_engine::rewrite_engine_claims`, an
//! exact-byte splice). See `docs/research/tier-c-seams.md` section 4.
//!
//! Reads the per-user registry (`repos.yaml`, `workspaces.yaml`) and the filesystem: no daemon,
//! no graph, no network. `--dry-run` prints the plan and writes nothing, and so does the command
//! with neither flag — a bare run names `--apply` rather than moving anything. `--apply` performs
//! the plan one repository at a time: the rename, the splices, then `git add` and one commit per
//! repository. It never pushes, and every precondition is re-checked immediately before that
//! repository's first write, because the printed plan was computed earlier and the tree may have
//! moved since.
//!
//! A repository is IN SCOPE when its plan is non-empty, and the plan derives only from the
//! legacy spelling (`.arsumbris/` present, or a claim that still spells `au.engine.*` /
//! `::au-engine`). A migrated repository therefore derives an empty plan, so `--apply` on one is
//! a no-op that exits `0`.
//!
//! Exit codes: `0` the plan was printed and nothing needed doing, or every planned repository was
//! migrated; `2` a repository was refused, the run could not be planned, or a repository failed
//! to write.

use std::collections::BTreeSet;
use std::ffi::OsStr;
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

/// Print the plan for every repository in scope, then apply it when `apply` is set.
pub fn identity(dry_run: bool, apply: bool, root: Option<PathBuf>, all: bool) -> ExitCode {
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

    let counts = Counts::of(&plans);
    println!();
    if counts.planned == 0 && counts.refused == 0 && counts.skipped == 0 {
        println!(
            "au: migrate identity: nothing to do, every repository in scope is already migrated"
        );
        return ExitCode::SUCCESS;
    }
    println!(
        "au: migrate identity: {} to migrate, {} already migrated, {} refused, {} skipped",
        counts.planned, counts.already, counts.refused, counts.skipped,
    );
    if dry_run {
        println!("au: migrate identity: dry run, nothing was written");
        return if counts.refused > 0 {
            ExitCode::from(FAILED)
        } else {
            ExitCode::SUCCESS
        };
    }
    if !apply {
        eprintln!(
            "au: migrate identity: nothing was written; re-run with --apply to perform the \
             migration"
        );
        return ExitCode::from(FAILED);
    }
    apply_plans(&plans)
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
    // `git mv .arsumbris .2brain` follows `mv` semantics: an existing `.2brain` directory would
    // receive `.arsumbris` INSIDE it, and the repository would end up with `.2brain/.arsumbris/`.
    // Any `.2brain` beside a legacy repository therefore blocks the rename until a human says
    // which directory is the real one.
    if legacy_repo && current.exists() {
        return RepoPlan::refused(
            root,
            label,
            "the new identity directory .2brain already exists beside .arsumbris, so the rename \
             would move one inside the other; resolve it first"
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

/// The printed plan's outcomes, one tally per kind.
struct Counts {
    planned: usize,
    already: usize,
    refused: usize,
    skipped: usize,
}

impl Counts {
    fn of(plans: &[RepoPlan]) -> Self {
        let mut counts = Self {
            planned: 0,
            already: 0,
            refused: 0,
            skipped: 0,
        };
        for plan in plans {
            match &plan.outcome {
                Outcome::Planned => counts.planned += 1,
                Outcome::AlreadyMigrated => counts.already += 1,
                Outcome::Refused(_) => counts.refused += 1,
                Outcome::Skipped(_) => counts.skipped += 1,
            }
        }
        counts
    }
}

/// Apply the printed plan: one commit per repository, stopping at the first repository that
/// fails to write.
///
/// A refused or skipped repository is reported and does not stop the run — the repositories are
/// independent, and each commit stands alone — but a repository whose WRITE fails does: the run
/// stops there, names what it committed and what it never attempted, and exits `2`. Each
/// repository is re-planned immediately before its first write, so every precondition is judged
/// on the tree as it is then rather than as it was when the plan was printed.
fn apply_plans(plans: &[RepoPlan]) -> ExitCode {
    let mut migrated = 0usize;
    let mut already = 0usize;
    let mut refused = 0usize;
    let mut skipped = 0usize;
    let mut failure: Option<(String, String)> = None;
    let mut not_attempted: Vec<String> = Vec::new();

    println!();
    for printed in plans {
        if failure.is_some() {
            not_attempted.push(printed.label.clone());
            continue;
        }
        match &printed.outcome {
            Outcome::Planned => {
                // The printed plan is what this run was asked about; the fresh one is what it
                // may write. They differ only if another writer slipped in between.
                let fresh = plan(&printed.root, &printed.label);
                match &fresh.outcome {
                    Outcome::Planned => match apply_repo(&fresh) {
                        Ok(committed) => {
                            if committed.claims == 0 {
                                println!(
                                    "  migrated  {}  the directory rename alone",
                                    committed.label
                                );
                            } else {
                                println!(
                                    "  migrated  {}  {} {} in {} {}",
                                    committed.label,
                                    committed.claims,
                                    plural(committed.claims, "claim line", "claim lines"),
                                    committed.files.len(),
                                    plural(committed.files.len(), "file", "files"),
                                );
                            }
                            println!("  commit    {}", committed.subject);
                            for (file, claims) in &committed.files {
                                println!(
                                    "  rewrote   {}  ({} {})",
                                    file.display(),
                                    claims,
                                    plural(*claims, "claim line", "claim lines"),
                                );
                            }
                            migrated += 1;
                        }
                        Err(reason) => failure = Some((fresh.label.clone(), reason)),
                    },
                    Outcome::AlreadyMigrated => {
                        println!(
                            "  already   {}  migrated since the plan was printed",
                            printed.label
                        );
                        already += 1;
                    }
                    Outcome::Refused(reason) => {
                        println!("  refused   {}  {reason}", printed.label);
                        refused += 1;
                    }
                    Outcome::Skipped(reason) => {
                        println!("  skipped   {}  {reason}", printed.label);
                        skipped += 1;
                    }
                }
            }
            Outcome::AlreadyMigrated => already += 1,
            Outcome::Refused(reason) => {
                println!("  refused   {}  {reason}", printed.label);
                refused += 1;
            }
            Outcome::Skipped(reason) => {
                println!("  skipped   {}  {reason}", printed.label);
                skipped += 1;
            }
        }
    }

    println!();
    if let Some((label, reason)) = &failure {
        eprintln!("au: migrate identity: {label}: {reason}");
    }
    println!(
        "au: migrate identity: {migrated} migrated, {already} already migrated, {refused} refused, \
         {skipped} skipped{}",
        if failure.is_some() { ", 1 failed" } else { "" },
    );
    if let Some((label, _)) = &failure {
        if not_attempted.is_empty() {
            println!(
                "au: migrate identity: stopped at {label}; there was no other repository left to \
                 attempt"
            );
        } else {
            println!(
                "au: migrate identity: stopped at {label}; not attempted: {}",
                not_attempted.join(", ")
            );
        }
        return ExitCode::from(FAILED);
    }
    if refused > 0 {
        ExitCode::from(FAILED)
    } else {
        ExitCode::SUCCESS
    }
}

/// One repository as the migration left it.
struct Committed {
    label: String,
    /// The commit message's subject line.
    subject: String,
    /// Every rewritten file, under the name it has after the rename, with its claim count.
    files: Vec<(PathBuf, usize)>,
    /// The claim lines rewritten across all of them.
    claims: usize,
}

/// Write one repository's plan in a single commit: the rename through git, then every claim
/// splice, then the stage and the commit. Never a push.
///
/// The splices are computed in memory before the first write, so a file that is unreadable or no
/// longer carries a legacy claim leaves the repository untouched. A failure after the first
/// write unwinds what was written, so a run that stops midway leaves no half-migrated repository
/// behind: a repository is either fully migrated in one commit or as the plan found it.
fn apply_repo(plan: &RepoPlan) -> Result<Committed, String> {
    let root = &plan.root;
    let splices = prepare(plan)?;
    let mut undo = Undo::default();

    if let Some(rename) = &plan.rename {
        git(
            root,
            &[
                OsStr::new("mv"),
                rename.from.as_os_str(),
                rename.to.as_os_str(),
            ],
        )
        .map_err(|e| format!("cannot rename {}: {e}", rename.from.display()))?;
        undo.rename = Some((rename.from.clone(), rename.to.clone()));
    }

    for splice in &splices {
        if let Err(e) = std::fs::write(root.join(&splice.written), &splice.next) {
            let warnings = undo.finish(root);
            return Err(explain(
                format!("cannot write {}: {e}", splice.written.display()),
                &warnings,
            ));
        }
        undo.files
            .push((splice.written.clone(), splice.original.clone()));
    }

    // Stage exactly the files the splice rewrote. `-f` is required: `git add` refuses a path an
    // ignore rule covers even when `git mv` has just staged it, and a repository mid-rebrand can
    // well carry `.2brain/` in its `.gitignore`. The paths are the ones this migration read and
    // spliced, already tracked by the rename, so the override reaches nothing else.
    if !splices.is_empty() {
        let mut stage: Vec<&OsStr> = vec![OsStr::new("add"), OsStr::new("-f"), OsStr::new("--")];
        stage.extend(splices.iter().map(|splice| splice.written.as_os_str()));
        if let Err(e) = git(root, &stage) {
            let warnings = undo.finish(root);
            return Err(explain(
                format!("cannot stage the rewritten files: {e}"),
                &warnings,
            ));
        }
    }

    let subject = format!("migrate identity: {}", plan.label);
    let body = commit_body(plan, &splices);
    if let Err(e) = git(
        root,
        &[
            OsStr::new("commit"),
            OsStr::new("-q"),
            OsStr::new("-m"),
            OsStr::new(&subject),
            OsStr::new("-m"),
            OsStr::new(&body),
        ],
    ) {
        let warnings = undo.finish(root);
        return Err(explain(
            format!("cannot commit the migration: {e}"),
            &warnings,
        ));
    }

    Ok(Committed {
        label: plan.label.clone(),
        subject,
        claims: splices.iter().map(|splice| splice.claims).sum(),
        files: splices
            .iter()
            .map(|splice| (splice.written.clone(), splice.claims))
            .collect(),
    })
}

/// One file's byte splice, computed but not yet written.
struct Splice {
    /// The file after the rename: where the new bytes are written.
    written: PathBuf,
    original: Vec<u8>,
    next: Vec<u8>,
    claims: usize,
}

/// Read and splice every file the plan rewrites, writing nothing.
///
/// Every read and every splice happens here, before the first write, so a stale plan (a file
/// that is gone, not UTF-8, or no longer carries a legacy claim) fails with the repository
/// untouched.
fn prepare(plan: &RepoPlan) -> Result<Vec<Splice>, String> {
    let mut seen: BTreeSet<&Path> = BTreeSet::new();
    let mut splices = Vec::new();
    for rewrite in &plan.rewrites {
        if !seen.insert(rewrite.file.as_path()) {
            continue;
        }
        let planned = &rewrite.file;
        let path = plan.root.join(planned);
        let original = std::fs::read(&path)
            .map_err(|e| format!("cannot read {}: {e}; the plan is stale", planned.display()))?;
        let text = std::str::from_utf8(&original)
            .map_err(|_| format!("{} is not UTF-8; the plan is stale", planned.display()))?;
        let (next, lines) = rewrite_engine_claims(&path, text).ok_or_else(|| {
            format!(
                "{} carries no legacy claim any more; the plan is stale",
                planned.display()
            )
        })?;
        splices.push(Splice {
            written: written_path(plan, planned),
            original,
            next: next.into_bytes(),
            claims: lines.len(),
        });
    }
    Ok(splices)
}

/// Where a planned file lives once the identity directory has moved: a path inside the renamed
/// directory is re-based on the new name, and every other path is unchanged.
fn written_path(plan: &RepoPlan, planned: &Path) -> PathBuf {
    match &plan.rename {
        Some(rename) => planned
            .strip_prefix(&rename.from)
            .map(|rest| rename.to.join(rest))
            .unwrap_or_else(|_| planned.to_path_buf()),
        None => planned.to_path_buf(),
    }
}

/// The commit message's body: the rename, then the claim count and the files it names.
fn commit_body(plan: &RepoPlan, splices: &[Splice]) -> String {
    let claims: usize = splices.iter().map(|splice| splice.claims).sum();
    let mut lines: Vec<String> = Vec::new();
    if let Some(rename) = &plan.rename {
        lines.push(format!(
            "Renamed {} -> {}.",
            rename.from.display(),
            rename.to.display()
        ));
    }
    if claims > 0 {
        if !lines.is_empty() {
            lines.push(String::new());
        }
        lines.push(format!(
            "{claims} {} rewritten in {} {}:",
            plural(claims, "claim line", "claim lines"),
            splices.len(),
            plural(splices.len(), "file", "files"),
        ));
        for splice in splices {
            lines.push(format!("  {}", splice.written.display()));
        }
    }
    lines.join("\n")
}

/// What a failed apply has to put back: the rename it performed and the bytes it already wrote.
#[derive(Default)]
struct Undo {
    /// The directory rename already done, `(from, to)` relative to the repository root.
    rename: Option<(PathBuf, PathBuf)>,
    /// The files already rewritten, with the bytes to put back.
    files: Vec<(PathBuf, Vec<u8>)>,
}

impl Undo {
    /// Return the repository to the state the plan read: the bytes put back, the rename
    /// reversed, then the index reset — the tree was verified clean before the first write, so
    /// HEAD is the state to return to. Best effort: a step that fails is returned as a warning,
    /// because a rollback that cannot complete must still be reported rather than hidden.
    fn finish(self, root: &Path) -> Vec<String> {
        let mut warnings = Vec::new();
        for (path, bytes) in &self.files {
            if let Err(e) = std::fs::write(root.join(path), bytes) {
                warnings.push(format!(
                    "cannot restore {}; it may carry a half-written splice",
                    path.display()
                ));
                warnings.push(e.to_string());
            }
        }
        if let Some((from, to)) = &self.rename {
            if let Err(e) = git(root, &[OsStr::new("mv"), to.as_os_str(), from.as_os_str()]) {
                warnings.push(format!(
                    "cannot rename {} back to {}",
                    to.display(),
                    from.display()
                ));
                warnings.push(e);
            }
        }
        if let Err(e) = git(root, &[OsStr::new("reset"), OsStr::new("-q")]) {
            warnings.push("cannot unstage the migration".to_string());
            warnings.push(e);
        }
        warnings
    }
}

/// A failure plus whatever the rollback could not put back, as one message.
fn explain(reason: String, warnings: &[String]) -> String {
    if warnings.is_empty() {
        return format!("{reason}; the repository was left as the plan found it");
    }
    format!(
        "{reason}; the rollback is incomplete: {}",
        warnings.join("; ")
    )
}

/// `git -C <root> <args>`, every pathspec read literally. A failure returns git's own stderr,
/// so the operator sees the repository's reason (a hook, a missing identity) rather than a
/// paraphrase.
fn git(root: &Path, args: &[&OsStr]) -> Result<(), String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .env("GIT_LITERAL_PATHSPECS", "1")
        .args(args)
        .output()
        .map_err(|e| format!("git is unavailable: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stderr = stderr.trim();
    let command = args
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(" ");
    if stderr.is_empty() {
        Err(format!("`git {command}` failed"))
    } else {
        Err(format!("`git {command}` failed: {stderr}"))
    }
}

fn plural(count: usize, one: &'static str, many: &'static str) -> &'static str {
    if count == 1 { one } else { many }
}
