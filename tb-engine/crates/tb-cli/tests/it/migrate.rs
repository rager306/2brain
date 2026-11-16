//! `au migrate identity`: the plan and its refusals, `--apply`'s one commit per repository, and
//! the no-op on a tree that is already migrated. Every fixture is a real git work tree, because
//! the command refuses a repository it cannot commit to.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// Run `au migrate identity` against a fixture, with `$HOME` isolated so no real device root or
/// socket is ever consulted.
fn run(home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_au"))
        .args(["migrate", "identity"])
        .args(args)
        .env("HOME", home)
        .env_remove("2BRAIN_HOME")
        .output()
        .unwrap()
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn git(root: &Path, args: &[&str]) -> Output {
    Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap()
}

fn git_ok(root: &Path, args: &[&str]) {
    let out = git(root, args);
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A committed git work tree, so the clean-tree check has a baseline.
fn init_repo(root: &Path) {
    fs::create_dir_all(root).unwrap();
    git_ok(root, &["init", "-q", "-b", "main"]);
    git_ok(root, &["config", "user.email", "migrate@example.test"]);
    git_ok(root, &["config", "user.name", "migrate test"]);
}

fn commit(root: &Path, message: &str) {
    git_ok(root, &["add", "-A"]);
    git_ok(root, &["commit", "-q", "-m", message]);
}

/// Every working-tree file and its bytes, `.git` excluded: the dry run must leave the tree
/// byte-identical, and `.git` is git's own bookkeeping (a status check may rewrite the index).
fn tree(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut files = BTreeMap::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            let relative = path.strip_prefix(root).unwrap().to_path_buf();
            if relative
                .components()
                .next()
                .is_some_and(|first| first.as_os_str() == ".git")
            {
                continue;
            }
            if path.is_dir() {
                pending.push(path);
            } else {
                files.insert(relative, fs::read(&path).unwrap());
            }
        }
    }
    files
}

const LEGACY_REPO_YAML: &str =
    "#: the repo identity\ntype: au.engine.repo::au-engine\nname: legacy\n";
const LEGACY_TYPE_DEF: &str = "\
#: a knowledge base type that mixes in the engine meta base
extends: au.engine.meta::au-engine
fields:
  name: String
";

const LEGACY_README: &str = "\
---
type: au.engine.readme::au-engine
tldr: the legacy fixture
---

# Legacy

```yaml
type: au.engine.repo::au-engine
```
";

/// A live legacy repository: a legacy identity directory, a type-def extending the engine meta
/// base, and a README whose type self-declares. The fenced sample in the README body is prose and
/// must never be touched.
fn legacy_fixture(top: &Path) -> PathBuf {
    legacy_repo(top, "legacy")
}

/// The same fixture under `top/<name>`, so a run can cover several repositories.
fn legacy_repo(top: &Path, name: &str) -> PathBuf {
    let repo = top.join(name);
    init_repo(&repo);
    fs::create_dir_all(repo.join(".arsumbris")).unwrap();
    fs::write(repo.join(".arsumbris/repo.yaml"), LEGACY_REPO_YAML).unwrap();
    fs::create_dir_all(repo.join("type")).unwrap();
    fs::write(repo.join("type/custom.type.yaml"), LEGACY_TYPE_DEF).unwrap();
    fs::write(repo.join("README.md"), LEGACY_README).unwrap();
    commit(&repo, "legacy fixture");
    repo
}

/// The commit sha at HEAD.
fn head(root: &Path) -> String {
    git_stdout(root, &["rev-parse", "HEAD"])
}

/// How many commits HEAD carries.
fn commits(root: &Path) -> usize {
    git_stdout(root, &["rev-list", "--count", "HEAD"])
        .parse()
        .unwrap()
}

/// Empty iff the working tree and the index match HEAD.
fn status(root: &Path) -> String {
    git_stdout(root, &["status", "--porcelain"])
}

fn subject(root: &Path) -> String {
    git_stdout(root, &["log", "-1", "--format=%s"])
}

fn commit_body(root: &Path) -> String {
    git_stdout(root, &["log", "-1", "--format=%b"])
}

fn git_stdout(root: &Path, args: &[&str]) -> String {
    let out = git(root, args);
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A `pre-commit` hook that refuses, so a repository fails AFTER its files are written: the
/// case the rollback exists for.
fn refuse_commits(repo: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let hook = repo.join(".git/hooks/pre-commit");
    fs::write(
        &hook,
        "#!/bin/sh\necho refusing from the fixture hook >&2\nexit 1\n",
    )
    .unwrap();
    let mut permissions = fs::metadata(&hook).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&hook, permissions).unwrap();
}

/// The migration changes two things and nothing else: the identity directory's NAME, and the
/// claim lines' spelling. Every other byte, and the whole file set, is carried through.
///
/// `before` is the tree as the plan read it (`.arsumbris/...`), `after` the tree the run left;
/// the comparison normalises the rename away and then requires every differing line to be a
/// respelling the migration is allowed to make.
fn assert_only_claims_changed(
    before: &BTreeMap<PathBuf, Vec<u8>>,
    after: &BTreeMap<PathBuf, Vec<u8>>,
) {
    let renamed: BTreeMap<PathBuf, &Vec<u8>> = before
        .iter()
        .map(|(path, bytes)| {
            let path = match path.strip_prefix(".arsumbris") {
                Ok(rest) => Path::new(".2brain").join(rest),
                Err(_) => path.clone(),
            };
            (path, bytes)
        })
        .collect();
    let expected: Vec<&PathBuf> = renamed.keys().collect();
    let actual: Vec<&PathBuf> = after.keys().collect();
    assert_eq!(expected, actual, "the file set changed beyond the rename");

    for (path, before_bytes) in &renamed {
        let before_text = String::from_utf8_lossy(before_bytes);
        let after_text = String::from_utf8_lossy(&after[path]);
        let before_lines: Vec<&str> = before_text.lines().collect();
        let after_lines: Vec<&str> = after_text.lines().collect();
        assert_eq!(
            before_lines.len(),
            after_lines.len(),
            "{}: the line count changed",
            path.display()
        );
        for (index, (old, new)) in before_lines.iter().zip(&after_lines).enumerate() {
            if old == new {
                continue;
            }
            assert_eq!(
                respelled(old),
                *new,
                "{}:{} changed something other than a claim line",
                path.display(),
                index + 1
            );
        }
    }
}

/// The release-2 spelling of a legacy claim line: the only edit the migration may make to a
/// file's bytes.
fn respelled(line: &str) -> String {
    line.replace("au.engine.", "tb.engine.")
        .replace("::au-engine", "::tb-engine")
}

#[test]
fn a_legacy_repo_plans_the_rename_and_every_claim_line() {
    let dir = tempfile::tempdir().unwrap();
    let top = fs::canonicalize(dir.path()).unwrap();
    let home = top.join("home");
    let repo = legacy_fixture(&top);

    let out = run(&home, &["--dry-run", "--root", repo.to_str().unwrap()]);
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);

    assert!(
        text.contains("rename   .arsumbris -> .2brain"),
        "the directory rename is named: {text}"
    );
    assert!(
        text.contains(".arsumbris/repo.yaml:2")
            && text.contains("type: au.engine.repo::au-engine -> type: tb.engine.repo::tb-engine"),
        "the identity claim is named: {text}"
    );
    assert!(
        text.contains("type/custom.type.yaml:2")
            && text.contains(
                "extends: au.engine.meta::au-engine -> extends: tb.engine.meta::tb-engine"
            ),
        "the type-def extends line is named: {text}"
    );
    assert!(
        text.contains("README.md:2")
            && text
                .contains("type: au.engine.readme::au-engine -> type: tb.engine.readme::tb-engine"),
        "the README claim is named: {text}"
    );
    assert!(
        text.contains("3 claim lines"),
        "exactly the three claim lines, never the fenced sample: {text}"
    );
}

#[test]
fn a_dry_run_leaves_the_tree_byte_identical() {
    let dir = tempfile::tempdir().unwrap();
    let top = fs::canonicalize(dir.path()).unwrap();
    let home = top.join("home");
    let repo = legacy_fixture(&top);

    let before = tree(&repo);
    let out = run(&home, &["--dry-run", "--root", repo.to_str().unwrap()]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(before, tree(&repo), "the dry run wrote something");
    assert!(
        !repo.join(".2brain").exists(),
        "a dry run must not create .2brain to check whether it works"
    );
    assert!(repo.join(".arsumbris/repo.yaml").is_file());
}

#[test]
fn an_already_migrated_repo_yields_an_empty_plan_twice() {
    let dir = tempfile::tempdir().unwrap();
    let top = fs::canonicalize(dir.path()).unwrap();
    let home = top.join("home");
    let repo = top.join("migrated");
    init_repo(&repo);
    fs::create_dir_all(repo.join(".2brain")).unwrap();
    fs::write(
        repo.join(".2brain/repo.yaml"),
        "type: tb.engine.repo::tb-engine\nname: migrated\n",
    )
    .unwrap();
    fs::write(
        repo.join("README.md"),
        "---\ntype: tb.engine.readme::tb-engine\n---\n\n# Migrated\n",
    )
    .unwrap();
    commit(&repo, "migrated fixture");

    let before = tree(&repo);
    for pass in 1..=2 {
        let out = run(&home, &["--dry-run", "--root", repo.to_str().unwrap()]);
        assert!(out.status.success(), "pass {pass}: {}", stderr(&out));
        let text = stdout(&out);
        assert!(
            text.contains("nothing to do, every repository in scope is already migrated"),
            "pass {pass}: {text}"
        );
        assert!(
            !text.contains("rewrite"),
            "pass {pass} planned work: {text}"
        );
        assert!(
            !text.contains("rename"),
            "pass {pass} planned a rename: {text}"
        );
        assert_eq!(before, tree(&repo), "pass {pass} wrote something");
    }
}

#[test]
fn a_member_inside_a_monorepo_work_tree_is_planned() {
    // One work tree, several member repositories: tier C migrates a member at a time and commits
    // the outer tree, so an inner root is planned, not refused for not being the work-tree root.
    let dir = tempfile::tempdir().unwrap();
    let top = fs::canonicalize(dir.path()).unwrap();
    let home = top.join("home");
    init_repo(&top);
    let member = top.join("tb-type-system");
    fs::create_dir_all(member.join(".arsumbris")).unwrap();
    fs::write(member.join(".arsumbris/repo.yaml"), LEGACY_REPO_YAML).unwrap();
    commit(&top, "monorepo fixture");

    let out = run(&home, &["--dry-run", "--root", member.to_str().unwrap()]);
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("rename   .arsumbris -> .2brain"), "{text}");
    assert!(text.contains(".arsumbris/repo.yaml:2"), "{text}");
}

#[test]
fn a_dirty_tree_is_refused_with_the_reason() {
    let dir = tempfile::tempdir().unwrap();
    let top = fs::canonicalize(dir.path()).unwrap();
    let home = top.join("home");
    let repo = legacy_fixture(&top);
    fs::write(repo.join("uncommitted.md"), "a work in progress\n").unwrap();

    let before = tree(&repo);
    let out = run(&home, &["--dry-run", "--root", repo.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("refused  dirty working tree"), "{text}");
    assert_eq!(before, tree(&repo), "a refused run wrote something");
}

#[test]
fn a_mutation_in_flight_is_refused_with_the_reason() {
    let dir = tempfile::tempdir().unwrap();
    let top = fs::canonicalize(dir.path()).unwrap();
    let home = top.join("home");
    let repo = legacy_fixture(&top);
    let marker = repo.join(".arsumbris/au-engine/run/saga.intent");
    fs::create_dir_all(marker.parent().unwrap()).unwrap();
    fs::write(&marker, "mutation-id=test\n").unwrap();
    commit(&repo, "a saga marker, which the engine leaves mid-write");

    let out = run(&home, &["--dry-run", "--root", repo.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("refused  a mutation is in flight"), "{text}");
    assert!(text.contains("saga.intent"), "{text}");
}

#[test]
fn a_live_daemon_is_refused_with_the_reason() {
    let dir = tempfile::tempdir().unwrap();
    let top = fs::canonicalize(dir.path()).unwrap();
    let home = top.join("home");
    let repo = legacy_fixture(&top);

    // The socket the child derives from `$HOME` and the canonical root, bound here so a
    // connect succeeds. `.2brain` exists, so the child's device root is the new spelling.
    let sockets = home.join(".2brain/au-engine/run");
    fs::create_dir_all(&sockets).unwrap();
    let socket = sockets.join(tb_engine::socket_file_name(&repo));
    let _listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();

    let out = run(&home, &["--dry-run", "--root", repo.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains("refused  a daemon is serving this root"),
        "{text}"
    );
}

#[test]
fn all_walks_the_user_registry() {
    let dir = tempfile::tempdir().unwrap();
    let top = fs::canonicalize(dir.path()).unwrap();
    let home = top.join("home");
    let repo = legacy_fixture(&top);
    let config = home.join(".2brain/au-engine/config");
    fs::create_dir_all(&config).unwrap();
    fs::write(
        config.join("repos.yaml"),
        format!("repos:\n  - name: legacy\n    path: {}\n", repo.display()),
    )
    .unwrap();

    let out = run(&home, &["--dry-run", "--all"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains("legacy  /"),
        "the registered name labels the plan: {text}"
    );
    assert!(text.contains("rename   .arsumbris -> .2brain"), "{text}");
}

#[test]
fn apply_migrates_a_legacy_repo_in_one_commit() {
    let dir = tempfile::tempdir().unwrap();
    let top = fs::canonicalize(dir.path()).unwrap();
    let home = top.join("home");
    let repo = legacy_fixture(&top);
    let head_before = head(&repo);
    let commits_before = commits(&repo);

    let out = run(&home, &["--apply", "--root", repo.to_str().unwrap()]);
    assert!(out.status.success(), "{}", stderr(&out));

    assert!(
        !repo.join(".arsumbris").exists(),
        "the legacy identity directory is gone"
    );
    assert!(
        repo.join(".2brain/repo.yaml").is_file(),
        "the new identity directory holds the marker"
    );
    assert_eq!(
        fs::read_to_string(repo.join(".2brain/repo.yaml")).unwrap(),
        "#: the repo identity\ntype: tb.engine.repo::tb-engine\nname: legacy\n",
        "the identity claim is respelled in place"
    );

    assert_eq!(
        commits(&repo),
        commits_before + 1,
        "exactly one new commit, not one per rewritten file"
    );
    assert_ne!(head(&repo), head_before);
    assert_eq!(subject(&repo), "migrate identity: legacy");
    let body = commit_body(&repo);
    assert!(body.contains("Renamed .arsumbris -> .2brain."), "{body}");
    assert!(
        body.contains("3 claim lines rewritten in 3 files:"),
        "{body}"
    );
    for file in [".2brain/repo.yaml", "README.md", "type/custom.type.yaml"] {
        assert!(body.contains(file), "the body lists {file}: {body}");
    }
    assert_eq!(status(&repo), "", "the working tree is clean");

    // The README's fenced sample is prose, and it survived byte for byte.
    let readme = fs::read_to_string(repo.join("README.md")).unwrap();
    assert_eq!(
        readme.matches("type: au.engine.repo::au-engine").count(),
        1,
        "the fenced sample in the body is untouched: {readme}"
    );
    assert_eq!(
        readme,
        LEGACY_README.replace(
            "type: au.engine.readme::au-engine",
            "type: tb.engine.readme::tb-engine"
        )
    );
}

#[test]
fn apply_changes_only_the_rename_and_the_claim_lines() {
    let dir = tempfile::tempdir().unwrap();
    let top = fs::canonicalize(dir.path()).unwrap();
    let home = top.join("home");
    let repo = legacy_fixture(&top);
    let before = tree(&repo);

    let out = run(&home, &["--apply", "--root", repo.to_str().unwrap()]);
    assert!(out.status.success(), "{}", stderr(&out));

    // The whole tree, normalised for the rename: the file set is unchanged apart from the
    // identity directory's name, and every differing line is one of the two respellings.
    assert_only_claims_changed(&before, &tree(&repo));
}

#[test]
fn a_second_apply_is_a_noop() {
    let dir = tempfile::tempdir().unwrap();
    let top = fs::canonicalize(dir.path()).unwrap();
    let home = top.join("home");
    let repo = legacy_fixture(&top);

    let first = run(&home, &["--apply", "--root", repo.to_str().unwrap()]);
    assert!(first.status.success(), "{}", stderr(&first));
    let head_after = head(&repo);
    let tree_after = tree(&repo);

    let second = run(&home, &["--apply", "--root", repo.to_str().unwrap()]);
    assert!(second.status.success(), "{}", stderr(&second));
    let text = stdout(&second);
    assert!(
        text.contains("nothing to do, every repository in scope is already migrated"),
        "{text}"
    );
    assert_eq!(head(&repo), head_after, "the no-op committed nothing");
    assert_eq!(tree(&repo), tree_after, "the no-op wrote nothing");
}

#[test]
fn apply_refuses_a_dirty_tree_and_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let top = fs::canonicalize(dir.path()).unwrap();
    let home = top.join("home");
    let repo = legacy_fixture(&top);
    fs::write(repo.join("uncommitted.md"), "a work in progress\n").unwrap();

    let before = tree(&repo);
    let head_before = head(&repo);
    let out = run(&home, &["--apply", "--root", repo.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(
        stdout(&out).contains("refused  dirty working tree"),
        "{}",
        stdout(&out)
    );
    assert_eq!(before, tree(&repo), "a refused apply wrote something");
    assert_eq!(head(&repo), head_before, "a refused apply committed");
}

#[test]
fn apply_leaves_a_repository_untouched_when_its_commit_fails() {
    let dir = tempfile::tempdir().unwrap();
    let top = fs::canonicalize(dir.path()).unwrap();
    let home = top.join("home");
    let alpha = legacy_repo(&top, "alpha");
    let beta = legacy_repo(&top, "beta");
    refuse_commits(&beta);

    let config = home.join(".2brain/au-engine/config");
    fs::create_dir_all(&config).unwrap();
    fs::write(
        config.join("repos.yaml"),
        format!(
            "repos:\n  - name: alpha\n    path: {}\n  - name: beta\n    path: {}\n",
            alpha.display(),
            beta.display()
        ),
    )
    .unwrap();

    let beta_before = tree(&beta);
    let beta_head = head(&beta);
    let out = run(&home, &["--apply", "--all"]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));

    // The first repository was committed...
    let text = stdout(&out);
    assert!(text.contains("migrated  alpha"), "{text}");
    assert!(text.contains("1 migrated"), "{text}");
    assert!(alpha.join(".2brain/repo.yaml").is_file());
    assert_eq!(subject(&alpha), "migrate identity: alpha");
    assert_eq!(status(&alpha), "");

    // ...the second was rolled back whole: the rename reversed, the bytes restored, the index
    // reset. One commit or untouched, never half.
    let errors = stderr(&out);
    assert!(errors.contains("beta"), "{errors}");
    assert!(errors.contains("cannot commit"), "{errors}");
    assert!(
        errors.contains("refusing from the fixture hook"),
        "{errors}"
    );
    assert!(
        text.contains("stopped at beta"),
        "the report says where the run stopped: {text}"
    );
    assert_eq!(
        beta_before,
        tree(&beta),
        "the failed repository was written"
    );
    assert_eq!(head(&beta), beta_head);
    assert_eq!(status(&beta), "", "the failed repository is clean");
}

#[test]
fn a_repo_whose_new_identity_directory_already_exists_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let top = fs::canonicalize(dir.path()).unwrap();
    let home = top.join("home");
    let repo = legacy_fixture(&top);
    // An empty directory is invisible to git, so the tree stays clean and this is the
    // migration's own refusal: `git mv` would move `.arsumbris` INSIDE the existing `.2brain`.
    fs::create_dir_all(repo.join(".2brain")).unwrap();

    let before = tree(&repo);
    let head_before = head(&repo);
    let out = run(&home, &["--apply", "--root", repo.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(
        stdout(&out).contains("refused  the new identity directory .2brain already exists"),
        "{}",
        stdout(&out)
    );
    assert!(!repo.join(".2brain/repo.yaml").exists());
    assert!(repo.join(".arsumbris/repo.yaml").is_file());
    assert_eq!(before, tree(&repo));
    assert_eq!(head(&repo), head_before);
}

#[test]
fn dry_run_and_apply_are_mutually_exclusive() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let out = run(&home, &["--dry-run", "--apply"]);
    assert_eq!(out.status.code(), Some(2), "{}", stdout(&out));
    assert!(
        stderr(&out).contains("cannot be used with"),
        "{}",
        stderr(&out)
    );
}
