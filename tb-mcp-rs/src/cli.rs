//! `init` and `doctor`: the two commands that make a first run one command each.
//!
//! Neither takes part in a request, and both live outside `lib.rs`, which
//! `tests/architecture.rs` holds to a line ratchet. `init` writes a workspace the
//! engine opens with nothing to complain about; `doctor` asks the running system
//! — this binary, the engine binary, the daemon's socket, the workspace behind it,
//! and the MCP server's own `tools/list` — what works and what to do about what
//! does not.
//!
//! The one crate dependency either adds is `crate::transport`, the adapter's own
//! engine connection, so `doctor` asks the same question a tool call asks.

use std::{
    fs,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Command, ExitCode, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

use serde_json::{Value, json};

use crate::transport::{EngineTransport, Launch, SCHEMA_VERSION, UnixTransport, announce_launch};

/// The identity directory a workspace carries, as `tb_parser::IDENTITY_DIR`
/// spells it. Repeated rather than depended on: this crate's one engine
/// dependency is `tb-cli`, and `init` proves its own output with
/// `tb_cli::verify_entry` instead of trusting this constant.
const IDENTITY_DIR: &str = ".2brain";
/// The spelling a machine that has not been migrated still carries.
const LEGACY_IDENTITY_DIR: &str = ".arsumbris";
/// The marker file that makes a directory a repository.
const IDENTITY_MARKER: &str = "repo.yaml";
/// How long `doctor` waits for one engine answer.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
/// How long `doctor` waits for the MCP server to answer `tools/list`.
const MCP_TIMEOUT: Duration = Duration::from_secs(10);

// ---------------------------------------------------------------- resolve

/// The socket a target names, and where that path came from.
pub struct Resolved {
    /// The socket every request is sent to.
    pub socket: PathBuf,
    /// What produced it: the command line, or a workspace's state root.
    pub origin: Origin,
}

/// Where a resolved socket path came from, kept so the first failed connection
/// can name the path it tried and the root behind it.
pub enum Origin {
    /// A `.sock` path given as an argument: no state root was consulted.
    Given,
    /// Derived for a workspace: `<state root>/au-engine/run/<hash>.sock`.
    Derived {
        /// The workspace entry the socket was hashed from.
        target: PathBuf,
        /// The engine's device area, `$HOME/.2brain` or `$HOME/.arsumbris`.
        state_root: PathBuf,
        /// The directory the engine's `.sock` files live in.
        run_dir: PathBuf,
        /// The variable the state root came from, spelled with its value.
        source: String,
    },
}

/// The socket a target names: a `.sock` path as given, anything else a workspace
/// entry whose socket is derived the way `au daemon start` derives it.
///
/// The entry is canonicalized first because the daemon hashes a canonical path
/// (`tb_engine::serve::socket_path` documents this), and a symlinked or relative
/// spelling would otherwise hash to a socket nobody bound.
pub fn resolve(target: &Path) -> Result<Resolved, String> {
    if target
        .extension()
        .is_some_and(|extension| extension == "sock")
    {
        return Ok(Resolved {
            socket: target.to_path_buf(),
            origin: Origin::Given,
        });
    }
    let target =
        fs::canonicalize(target).map_err(|error| format!("{}: {error}", target.display()))?;
    let socket = tb_cli::socket_path(&target);
    // The socket sits at `<state root>/au-engine/run/<hash>.sock`, so the root is
    // read back from the path the engine will be asked for rather than derived a
    // second time from `$HOME`: one derivation, and the report names it.
    let run_dir = socket
        .parent()
        .map_or_else(|| socket.clone(), Path::to_path_buf);
    let state_root = run_dir
        .parent()
        .and_then(Path::parent)
        .map_or_else(|| run_dir.clone(), Path::to_path_buf);
    Ok(Resolved {
        socket,
        origin: Origin::Derived {
            target,
            state_root,
            run_dir,
            source: state_source(),
        },
    })
}

/// [`resolve`] for a caller that needs only the path.
pub fn resolve_socket(target: &Path) -> Result<PathBuf, String> {
    resolve(target).map(|resolved| resolved.socket)
}

/// What produced the state root: the variable the engine reads, with its value.
fn state_source() -> String {
    match std::env::var_os("2BRAIN_HOME") {
        Some(value) if !value.is_empty() => {
            format!("$2BRAIN_HOME={}", PathBuf::from(value).display())
        }
        _ => match std::env::var_os("HOME") {
            Some(value) => format!("$HOME={}", PathBuf::from(value).display()),
            None => "no $HOME, so the engine fell back to the temp directory".to_owned(),
        },
    }
}

/// Tell the serving transport what this launch resolved, so its first failed
/// connection says which socket it tried and where that path came from. Nothing
/// is printed here; the failure is, once.
pub fn announce(resolved: &Resolved) {
    let (origin, next) = match &resolved.origin {
        Origin::Given => (
            "no state root was consulted: the socket path was given explicitly".to_owned(),
            "start the daemon that binds that socket, or pass the workspace directory so \
             2brain derives the socket"
                .to_owned(),
        ),
        Origin::Derived {
            target,
            state_root,
            run_dir,
            source,
        } => (
            format!(
                "{} — the state root, from {source}; the engine's sockets live in {}/*.sock",
                state_root.display(),
                run_dir.display()
            ),
            format!(
                "start it with `2brain daemon start {}`; if the daemon runs under a different \
                 HOME, pass its .sock path to `2brain mcp` instead",
                target.display()
            ),
        ),
    };
    announce_launch(Launch { origin, next });
}

/// `path` made absolute against the current directory, without requiring it to
/// exist.
fn absolute(path: &Path) -> Result<PathBuf, String> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    std::env::current_dir()
        .map(|cwd| cwd.join(path))
        .map_err(|error| format!("cannot read the working directory: {error}"))
}

// ------------------------------------------------------------------ init

/// What `init` made, for the report it prints.
struct Created {
    root: PathBuf,
    name: String,
    template: Option<Template>,
    git: Git,
    /// The adopted identity declares `deps:` or `discover:` members, which this
    /// command cannot mount: the starter's bundles are the operator's to install.
    declares_members: bool,
}

struct Template {
    name: String,
    source: PathBuf,
    copied: usize,
    skipped: usize,
}

enum Git {
    /// `git init` ran: the workspace can take writes, which commit.
    Initialised,
    /// Already inside a work tree, so nothing was run.
    Present,
    /// No usable `git`: the workspace serves reads, and writes are refused.
    Missing,
}

/// Create a workspace at `directory` and print the one command that comes next.
pub fn init(directory: &Path, template: Option<&str>, force: bool) -> ExitCode {
    match create(directory, template, force) {
        Ok(created) => {
            report_created(&created);
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("2brain: {message}");
            ExitCode::FAILURE
        }
    }
}

fn create(directory: &Path, template: Option<&str>, force: bool) -> Result<Created, String> {
    let root = absolute(directory)?;
    // An existing file is never a workspace; anything else is created on demand.
    match fs::metadata(&root) {
        Ok(metadata) if !metadata.is_dir() => {
            return Err(format!(
                "{} is a file, not a directory; pass the directory you want to create",
                root.display()
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(&root)
                .map_err(|error| format!("cannot create {}: {error}", root.display()))?;
        }
        Err(error) => {
            return Err(format!("cannot read {}: {error}", root.display()));
        }
    }
    let root = fs::canonicalize(&root)
        .map_err(|error| format!("cannot resolve {}: {error}", root.display()))?;

    // A directory that already carries an identity is a workspace, and a second
    // one on top of it would leave two identities in one place. `--force` never
    // reaches this: it is about files, not about an existing workspace.
    for identity in [IDENTITY_DIR, LEGACY_IDENTITY_DIR] {
        if root.join(identity).exists() {
            return Err(format!(
                "{} already has {identity}/, so it is already a workspace; \
                 choose an empty directory, or remove {identity}/ to start over",
                root.display()
            ));
        }
    }
    if !force && let Some(first) = first_entry(&root)? {
        return Err(format!(
            "{} is not empty (it contains {first}); pass --force to add a workspace here anyway",
            root.display()
        ));
    }

    // The template goes in first, so a template's own README.md is the one that
    // stays and this command never overwrites a file.
    let template = match template {
        Some(name) => {
            let source = template_dir(name)?;
            let (copied, skipped) = copy_tree(&source, &root)?;
            Some(Template {
                name: name.to_owned(),
                source,
                copied,
                skipped,
            })
        }
        None => None,
    };

    let name = workspace_name(&root);
    // The identity comes last, and a template may have brought its own: a starter
    // declares `deps:` and a `workspace.yaml`, so replacing that file would strip
    // both. What changes is the name — the starters' own comments state the
    // contract, "rewritten to the new name on create".
    let identity = match existing_identity(&root) {
        Some(identity) => identity,
        None => {
            let identity = root.join(IDENTITY_DIR);
            fs::create_dir_all(&identity)
                .map_err(|error| format!("cannot create {}: {error}", identity.display()))?;
            identity
        }
    };
    let mut declares_members = false;
    let marker = identity.join(IDENTITY_MARKER);
    if marker.exists() {
        declares_members = adopt_identity(&marker, &name)?;
    } else {
        write_new(&marker, &identity_yaml(&name))?;
    }
    if !root.join("README.md").exists() {
        write_new(&root.join("README.md"), &readme_markdown(&name))?;
    }

    // The same preflight `au daemon start` runs: if this passes, the engine will
    // open what was just written, and if it does not the message says why.
    tb_cli::verify_entry(&root)
        .map_err(|error| format!("the engine will not open {}: {error}", root.display()))?;

    let git = ensure_git(&root)?;
    Ok(Created {
        root,
        name,
        template,
        git,
        declares_members,
    })
}

fn report_created(created: &Created) {
    let mut line = format!(
        "2brain: created workspace {:?} in {} ({IDENTITY_DIR}/{IDENTITY_MARKER}, README.md",
        created.name,
        created.root.display()
    );
    if let Some(template) = &created.template {
        line.push_str(&format!(
            ", template {} ({} files from {})",
            template.name,
            template.copied,
            template.source.display()
        ));
        if template.skipped > 0 {
            line.push_str(&format!(
                ", {} template files left alone because they already exist",
                template.skipped
            ));
        }
    }
    match created.git {
        Git::Initialised => line.push_str(", git initialised"),
        Git::Present => line.push_str(", git already tracked"),
        Git::Missing => line.push_str(", no git found"),
    }
    line.push(')');
    println!("{line}");
    if created.declares_members {
        println!(
            "2brain: note: the starter declares dependencies (`deps:` or `discover:`); until they are \
             mounted, `2brain doctor` reports each one as an unmounted warning"
        );
    }
    if matches!(created.git, Git::Missing) {
        eprintln!(
            "2brain: warning: no usable `git`; the workspace serves reads, and every write is \
             refused until it is a git work tree"
        );
    }
    // The last line is the command itself, with no prefix, so it can be copied.
    println!("2brain: next command:");
    println!("2brain daemon start {}", created.root.display());
}

/// The first name in `root`, sorted, so the refusal is deterministic.
fn first_entry(root: &Path) -> Result<Option<String>, String> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) => return Err(format!("cannot read {}: {error}", root.display())),
    };
    let mut names: Vec<String> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    Ok(names.into_iter().next())
}

/// The repo name for `root`: its directory name, cut down to the engine's
/// type-name grammar (a name that does not match is a `repo-name-missing`
/// diagnostic and the workspace vanishes from the graph).
///
/// Lowercased, with every run of characters outside `[a-z0-9_-]` folded to one
/// `-`, and a leading non-letter prefixed, because the first character of a name
/// must be a letter.
fn workspace_name(root: &Path) -> String {
    let raw = root
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut name = String::with_capacity(raw.len());
    let mut separator = false;
    for character in raw.chars() {
        if character.is_ascii_alphanumeric() || character == '_' {
            name.push(character.to_ascii_lowercase());
            separator = false;
        } else if !separator && !name.is_empty() {
            name.push('-');
            separator = true;
        }
    }
    while name.ends_with('-') {
        name.pop();
    }
    match name.chars().next() {
        Some(first) if first.is_ascii_alphabetic() => name,
        Some(_) => format!("repo-{name}"),
        None => "workspace".to_owned(),
    }
}

fn identity_yaml(name: &str) -> String {
    format!("type: au.engine.repo::au-engine\nname: {name}\n")
}

/// The identity directory `root` holds, under either spelling.
fn existing_identity(root: &Path) -> Option<PathBuf> {
    [IDENTITY_DIR, LEGACY_IDENTITY_DIR]
        .into_iter()
        .map(|identity| root.join(identity))
        .find(|identity| identity.is_dir())
}

/// The identity marker that makes `root` a repository the engine will open.
fn identity_marker_of(root: &Path) -> Option<PathBuf> {
    [IDENTITY_DIR, LEGACY_IDENTITY_DIR]
        .into_iter()
        .map(|identity| root.join(identity).join(IDENTITY_MARKER))
        .find(|marker| marker.is_file())
}

/// Rewrite a copied starter's identity to `name`: the top-level `name:` of its
/// `repo.yaml`, and its own entry in a `workspace.yaml`'s `edit:` list. Answers
/// whether the starter declares members this command cannot mount.
///
/// Line surgery rather than a YAML round trip: this crate carries no YAML parser,
/// and the two lines it changes are the two the format fixes by contract. Every
/// other line — `deps:`, `discover:`, comments — is left exactly as the starter
/// wrote it.
fn adopt_identity(marker: &Path, name: &str) -> Result<bool, String> {
    let text = fs::read_to_string(marker)
        .map_err(|error| format!("cannot read {}: {error}", marker.display()))?;
    let previous = text
        .lines()
        .find_map(|line| line.strip_prefix("name:"))
        .map(|value| {
            value
                .split('#')
                .next()
                .unwrap_or_default()
                .trim()
                .to_owned()
        })
        .unwrap_or_default();
    let mut renamed = String::with_capacity(text.len());
    let mut found = false;
    for line in text.lines() {
        if !found && line.starts_with("name:") {
            renamed.push_str(&format!("name: {name}"));
            found = true;
        } else {
            renamed.push_str(line);
        }
        renamed.push('\n');
    }
    if !found {
        return Err(format!(
            "{} declares no top-level `name:`, so the starter's identity cannot be renamed",
            marker.display()
        ));
    }
    write_over(marker, &renamed)?;

    // The workspace manifest names its own repository as an `edit:` member; a
    // rename that misses it would leave the workspace pointing at a member that
    // no longer exists.
    let manifest = marker.with_file_name("workspace.yaml");
    let manifest_text = if manifest.exists() {
        fs::read_to_string(&manifest)
            .map_err(|error| format!("cannot read {}: {error}", manifest.display()))?
    } else {
        String::new()
    };
    let declares_members = text.lines().any(|line| line.starts_with("deps:"))
        || manifest_text
            .lines()
            .any(|line| line.starts_with("discover:"));
    if previous.is_empty() || manifest_text.is_empty() {
        return Ok(declares_members);
    }
    let renamed = manifest_text.replace(&format!("- {previous}"), &format!("- {name}"));
    if renamed != manifest_text {
        write_over(&manifest, &renamed)?;
    }
    Ok(declares_members)
}

/// The README the engine expects at an editable repo's root: it declares
/// `au.engine.readme`, carries the one required field, and holds the one body
/// template, so a fresh workspace reports no `repo-missing-readme`.
fn readme_markdown(name: &str) -> String {
    format!(
        "\
---
type: au.engine.readme::au-engine
tldr: A 2brain workspace named {name}, holding typed markdown that the engine serves and agents read over MCP.
---

# Repo Overview

## What this is

`{name}` is a 2brain workspace. Its content is markdown files; one whose frontmatter declares a
`type:` is an instance of that type, and `[[wiki-links]]` and `^block-ids` connect and address them.
The engine derives a typed graph from these files and rebuilds it whenever they change.

## How to use this

Start the engine with `2brain daemon start <this folder>`, then check it with `2brain doctor`. Read and
write through an MCP client, or edit the files directly: they are the source of truth, and a write made
elsewhere is picked up by the next rebuild.

## How to extend this

Declare a type in `type/<name>.type.yaml` and claim it from a note's frontmatter as `type: <name>`.
A repository other workspaces can mount goes in `.2brain/workspace.yaml` under `discover:`.
"
    )
}

fn write_new(path: &Path, contents: &str) -> Result<(), String> {
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .and_then(|mut file| file.write_all(contents.as_bytes()))
        .map_err(|error| format!("cannot write {}: {error}", path.display()))
}

/// Replace a file this command created or adopted; never used on a file it did
/// not just write.
fn write_over(path: &Path, contents: &str) -> Result<(), String> {
    fs::write(path, contents).map_err(|error| format!("cannot write {}: {error}", path.display()))
}

// -------------------------------------------------------------- templates

/// Where the named starter lives.
///
/// An installation copies these next to the binary (`<prefix>/share/2brain/templates`),
/// a checkout keeps them in `tb-defaults/`, and the environment override wins so a
/// test — or a machine with its own starters — never depends on either.
fn template_dir(name: &str) -> Result<PathBuf, String> {
    if name.is_empty() || name.contains('/') || name.contains('\\') || name == "." || name == ".." {
        return Err(format!(
            "template name {name:?} must be one directory name, not a path"
        ));
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(root) = std::env::var_os("2BRAIN_TEMPLATES") {
        candidates.push(PathBuf::from(root).join(name));
    }
    if let Ok(executable) = std::env::current_exe()
        && let Some(bin) = executable.parent()
    {
        if let Some(prefix) = bin.parent() {
            candidates.push(
                prefix
                    .join("share")
                    .join("2brain")
                    .join("templates")
                    .join(name),
            );
        }
        candidates.push(bin.join("templates").join(name));
    }
    let checkout = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(|root| root.join("tb-defaults").join(name));
    candidates.extend(checkout);

    for candidate in &candidates {
        if candidate.is_dir() {
            return Ok(candidate.clone());
        }
    }
    let looked: Vec<String> = candidates
        .iter()
        .map(|candidate| candidate.display().to_string())
        .collect();
    Err(format!(
        "no template named {name:?}; looked in {}",
        looked.join(", ")
    ))
}

/// Copy `source` into `destination`, never overwriting: a file that is already
/// there is left alone and counted, so `--force` cannot lose content.
fn copy_tree(source: &Path, destination: &Path) -> Result<(usize, usize), String> {
    let mut copied = 0;
    let mut skipped = 0;
    let entries = fs::read_dir(source)
        .map_err(|error| format!("cannot read template {}: {error}", source.display()))?;
    for entry in entries {
        let entry =
            entry.map_err(|error| format!("cannot read template {}: {error}", source.display()))?;
        let from = entry.path();
        let to = destination.join(entry.file_name());
        let kind = entry
            .file_type()
            .map_err(|error| format!("cannot read {}: {error}", from.display()))?;
        if kind.is_dir() {
            if to.exists() && !to.is_dir() {
                skipped += 1;
                continue;
            }
            fs::create_dir_all(&to)
                .map_err(|error| format!("cannot create {}: {error}", to.display()))?;
            let (nested_copied, nested_skipped) = copy_tree(&from, &to)?;
            copied += nested_copied;
            skipped += nested_skipped;
        } else if to.exists() {
            skipped += 1;
        } else {
            fs::copy(&from, &to).map_err(|error| {
                format!(
                    "cannot copy {} to {}: {error}",
                    from.display(),
                    to.display()
                )
            })?;
            copied += 1;
        }
    }
    Ok((copied, skipped))
}

// ------------------------------------------------------------------- git

/// Make sure the workspace is a git work tree, which every engine write needs.
///
/// A directory that is already inside one is left alone: `git init` in a subpath
/// of a repository would create a second root, and the engine would commit to it.
fn ensure_git(root: &Path) -> Result<Git, String> {
    match git_succeeds(root, &["rev-parse", "--show-toplevel"]) {
        // git ran and answered: this is already a work tree.
        Some(true) => Ok(Git::Present),
        // git ran and refused: a plain directory, so initialise one.
        Some(false) => match git_succeeds(root, &["init", "--quiet"]) {
            Some(true) => Ok(Git::Initialised),
            Some(false) => Err(format!(
                "`git init` failed in {}; the engine commits every write, so a workspace must be \
                 a git work tree",
                root.display()
            )),
            None => Ok(Git::Missing),
        },
        // git is not installed at all.
        None => Ok(Git::Missing),
    }
}

/// `Some(true)` when git ran and succeeded, `Some(false)` when it ran and
/// refused, `None` when it could not be started.
fn git_succeeds(root: &Path, arguments: &[&str]) -> Option<bool> {
    Command::new("git")
        .args(arguments)
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .ok()
        .map(|status| status.success())
}

// ---------------------------------------------------------------- doctor

/// One line's verdict. `fail` is what makes the exit code non-zero.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Status {
    Ok,
    Warn,
    Fail,
}

impl Status {
    fn word(self) -> &'static str {
        match self {
            Status::Ok => "ok",
            Status::Warn => "warn",
            Status::Fail => "fail",
        }
    }
}

/// The lines `doctor` prints, collected before printing so they can be aligned,
/// and counted once so the exit code cannot disagree with the report.
#[derive(Default)]
struct Report {
    lines: Vec<(Status, &'static str, String)>,
}

impl Report {
    fn ok(&mut self, label: &'static str, text: impl Into<String>) {
        self.lines.push((Status::Ok, label, text.into()));
    }

    fn warn(&mut self, label: &'static str, text: impl Into<String>) {
        self.lines.push((Status::Warn, label, text.into()));
    }

    fn fail(&mut self, label: &'static str, text: impl Into<String>) {
        self.lines.push((Status::Fail, label, text.into()));
    }

    fn count(&self, wanted: Status) -> usize {
        self.lines
            .iter()
            .filter(|(status, _, _)| *status == wanted)
            .count()
    }

    /// Print every line, then the tally, and answer with the exit code the
    /// tally implies: `0` only when nothing failed.
    fn finish(self) -> ExitCode {
        let width = self
            .lines
            .iter()
            .map(|(_, label, _)| label.len())
            .max()
            .unwrap_or(0);
        for (status, label, text) in &self.lines {
            println!("{:<4}  {label:<width$}  {text}", status.word());
        }
        let (ok, warn, fail) = (
            self.count(Status::Ok),
            self.count(Status::Warn),
            self.count(Status::Fail),
        );
        if fail == 0 {
            println!("2brain doctor: {ok} ok, {warn} warn, no failures");
            ExitCode::SUCCESS
        } else {
            println!("2brain doctor: {ok} ok, {warn} warn, {fail} failed");
            ExitCode::FAILURE
        }
    }
}

/// Answer "is this working, and if not what do I do" for `target`.
///
/// The checks run in the order a person would meet them: this binary, the engine
/// binary, the target, the socket it resolves to, the daemon on that socket, the
/// workspace behind the daemon, and the MCP server's own answer. An unresolvable
/// target stops the run — everything after it would be a guess.
pub fn doctor(target: &Path) -> ExitCode {
    let mut report = Report::default();
    report.ok(
        "binary",
        format!(
            "2brain {} — this build is what runs",
            crate::version_string()
        ),
    );

    match engine_binary() {
        Some(path) => match engine_version(&path) {
            Some(version) => report.ok("engine", format!("{version} at {}", path.display())),
            None => report.warn(
                "engine",
                format!(
                    "{} did not answer --version — reinstall it with ./install.sh",
                    path.display()
                ),
            ),
        },
        None => report.warn(
            "engine",
            "no `au` beside 2brain or on PATH — `2brain daemon` does not need it, and ./install.sh installs it",
        ),
    }

    let target = match fs::canonicalize(target) {
        Ok(path) => path,
        Err(error) => {
            report.fail(
                "target",
                format!(
                    "{} cannot be resolved ({error}) — pass an existing workspace directory, or create one with `2brain init <dir>`",
                    target.display()
                ),
            );
            return report.finish();
        }
    };
    let socket = if target.is_dir() {
        if identity_marker_of(&target).is_some() {
            report.ok("target", format!("workspace {}", target.display()));
        } else {
            // Every line after this one would name the wrong fix: a daemon cannot
            // serve a directory the engine refuses, so the advice would be "start
            // it" for a start that fails.
            report.fail(
                "target",
                format!(
                    "{} is not a workspace (no {IDENTITY_DIR}/{IDENTITY_MARKER}) — create one with `2brain init {}`",
                    target.display(),
                    target.display()
                ),
            );
            return report.finish();
        }
        tb_cli::socket_path(&target)
    } else if target
        .extension()
        .is_some_and(|extension| extension == "sock")
    {
        report.ok("target", format!("socket {}", target.display()));
        target.clone()
    } else {
        report.fail(
            "target",
            format!(
                "{} is not a directory or a .sock path — pass the workspace directory the daemon serves",
                target.display()
            ),
        );
        return report.finish();
    };
    report.ok("socket", format!("{}", socket.display()));

    match engine_overview(&socket) {
        Ok(response) => {
            let schema = response["schema_version"].as_u64();
            let build = response["version"].as_u64();
            let build = build.map_or_else(|| "?".to_owned(), |build| build.to_string());
            match schema {
                Some(schema) if schema == SCHEMA_VERSION => report.ok(
                    "daemon",
                    format!("answering, wire schema {schema}, engine version {build}"),
                ),
                Some(schema) => report.warn(
                    "daemon",
                    format!(
                        "answering at wire schema {schema}, and this build speaks {SCHEMA_VERSION} — restart the daemon so the two agree"
                    ),
                ),
                None => report.warn(
                    "daemon",
                    "answering without a schema version — restart the daemon, or reinstall it from this build",
                ),
            }
            let overview = &response["result"]["overview"];
            let members = overview["members"].as_array().map_or(0, Vec::len);
            let diagnostics = overview["diagnostic_counts"]["total"]
                .as_u64()
                .unwrap_or_default();
            let members = plural(members, "member");
            if diagnostics == 0 {
                report.ok("workspace", format!("{members}, no diagnostics"));
            } else {
                report.warn(
                    "workspace",
                    format!(
                        "{members}, {} — call `tb_diagnostics` through an MCP client for the file and the fix",
                        plural(diagnostics as usize, "diagnostic")
                    ),
                );
            }
        }
        Err(message) => {
            report.fail(
                "daemon",
                format!(
                    "nothing answered the socket ({message}) — start it with `2brain daemon start {}`",
                    target.display()
                ),
            );
            report.fail(
                "workspace",
                "not read, because the daemon has not answered — start it and run doctor again",
            );
        }
    }

    match mcp_tool_count(&socket) {
        Ok(tools) => report.ok(
            "mcp",
            format!(
                "{tools} tools advertised with no mutation grant — the MCP server starts and answers tools/list"
            ),
        ),
        Err(message) => report.fail(
            "mcp",
            format!(
                "{message} — run `2brain mcp {}` by hand to see the server's own error",
                socket.display()
            ),
        ),
    }

    report.finish()
}

fn plural(count: usize, noun: &str) -> String {
    match count {
        1 => format!("1 {noun}"),
        _ => format!("{count} {noun}s"),
    }
}

/// The `au` binary beside this one, else the first one on `PATH`.
fn engine_binary() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(executable) = std::env::current_exe()
        && let Some(bin) = executable.parent()
    {
        candidates.push(bin.join("au"));
    }
    if let Some(path) = std::env::var_os("PATH") {
        candidates.extend(std::env::split_paths(&path).map(|directory| directory.join("au")));
    }
    candidates.into_iter().find(|candidate| candidate.is_file())
}

/// The first line `au --version` prints, `None` when it does not run or refuses.
fn engine_version(path: &Path) -> Option<String> {
    let output = Command::new(path)
        .arg("--version")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let version = String::from_utf8_lossy(&output.stdout);
    let line = version.lines().next()?.trim();
    (!line.is_empty()).then(|| line.to_owned())
}

/// One engine read over the socket this build speaks, so `doctor` asks exactly
/// what a tool call asks.
fn engine_overview(socket: &Path) -> Result<Value, String> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("cannot start a runtime: {error}"))?;
    let transport = UnixTransport::new(socket.to_path_buf());
    runtime.block_on(async {
        match tokio::time::timeout(
            PROBE_TIMEOUT,
            transport.exchange(json!({"read": "overview", "id": 1})),
        )
        .await
        {
            Err(_) => Err(format!("no answer within {} s", PROBE_TIMEOUT.as_secs())),
            Ok(Err(error)) => Err(error.message()),
            Ok(Ok(response)) => Ok(response),
        }
    })
}

/// How many tools the MCP server itself advertises.
///
/// Asked of a child process rather than of this crate's router: the number then
/// proves the stdio server starts, completes the handshake, and answers
/// `tools/list`, which is what a client will ask it for next. No count is
/// expected here — a catalogue that changed is not a broken installation, and
/// `tests/architecture.rs` is where that number is pinned.
fn mcp_tool_count(socket: &Path) -> Result<usize, String> {
    let executable =
        std::env::current_exe().map_err(|error| format!("cannot locate this binary: {error}"))?;
    let mut child = Command::new(executable)
        .arg("mcp")
        .arg(socket)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("cannot start the MCP server: {error}"))?;
    let outcome = ask_tools_list(&mut child);
    let _ = child.kill();
    let _ = child.wait();
    outcome
}

fn ask_tools_list(child: &mut std::process::Child) -> Result<usize, String> {
    let mut input = child
        .stdin
        .take()
        .ok_or_else(|| "the MCP server has no standard input".to_owned())?;
    let output = child
        .stdout
        .take()
        .ok_or_else(|| "the MCP server has no standard output".to_owned())?;
    let (sender, replies) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(output).lines() {
            let Ok(line) = line else { break };
            if sender.send(line).is_err() {
                break;
            }
        }
    });
    for request in [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
            "protocolVersion":"2025-06-18","capabilities":{},
            "clientInfo":{"name":"2brain doctor","version":"1"}}}),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}),
    ] {
        writeln!(input, "{request}")
            .map_err(|error| format!("cannot write to the MCP server: {error}"))?;
    }
    input
        .flush()
        .map_err(|error| format!("cannot write to the MCP server: {error}"))?;

    let deadline = Instant::now() + MCP_TIMEOUT;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let line = match replies.recv_timeout(remaining) {
            Ok(line) => line,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err("the MCP server exited before answering tools/list".to_owned());
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                return Err(format!(
                    "the MCP server did not answer within {} s",
                    MCP_TIMEOUT.as_secs()
                ));
            }
        };
        let Ok(reply) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if reply["id"] == 2 {
            let tools = reply["result"]["tools"]
                .as_array()
                .map(Vec::len)
                .unwrap_or_default();
            if tools == 0 {
                return Err(format!(
                    "the MCP server answered tools/list with no tools: {reply}"
                ));
            }
            return Ok(tools);
        }
    }
}
