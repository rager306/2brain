//! `au migrate identity --dry-run`: the plan, its three refusals, and the no-op on a tree that
//! is already migrated. Every fixture is a real git work tree, because the command refuses a
//! repository it cannot commit to.

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
    let repo = top.join("legacy");
    init_repo(&repo);
    fs::create_dir_all(repo.join(".arsumbris")).unwrap();
    fs::write(repo.join(".arsumbris/repo.yaml"), LEGACY_REPO_YAML).unwrap();
    fs::create_dir_all(repo.join("type")).unwrap();
    fs::write(repo.join("type/custom.type.yaml"), LEGACY_TYPE_DEF).unwrap();
    fs::write(repo.join("README.md"), LEGACY_README).unwrap();
    commit(&repo, "legacy fixture");
    repo
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
