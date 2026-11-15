//! The `move_dir` mutation over the socket: a folder moved as one saga, every
//! path-addressed reference into it following, its directory skeleton with it.

#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use tb_engine::{Client, ConfigSource, Engine, serve};

pub(crate) struct Harness {
    _dir: tempfile::TempDir,
    _sock_dir: tempfile::TempDir,
    pub engine: Engine,
    _server: tb_engine::ServeHandle,
    pub client: Client,
    pub root: PathBuf,
}

/// A git repo holding a `note` type and `files`, committed, then served.
pub(crate) fn started(files: &[(&str, &str)]) -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap();
    crate::seed_repo(&root);
    wr(
        &root,
        "type/note.type.yaml",
        "fields:\n  description: String\n",
    );
    for (p, c) in files {
        wr(&root, p, c);
    }
    git_seed(&root);
    serve_at(dir, root)
}

/// Make `repo` a git repo holding everything under it, in one seed commit.
pub(crate) fn git_seed(repo: &Path) {
    git(repo, &["init", "-q", "-b", "main"]);
    git(repo, &["config", "user.name", "Tester"]);
    git(repo, &["config", "user.email", "tester@example.com"]);
    git(repo, &["add", "."]);
    git(repo, &["commit", "-q", "-m", "seed"]);
}

/// Serve the prepared tree at `root`.
pub(crate) fn serve_at(dir: tempfile::TempDir, root: PathBuf) -> Harness {
    let sock_dir = tempfile::tempdir().unwrap();
    let socket = sock_dir.path().join("s");
    let engine = Engine::new(&root, ConfigSource::Empty);
    let server = serve(engine.handle(), &socket).expect("serve");
    engine.rebuild();
    let client = Client::connect(&socket).expect("connect");
    Harness {
        _dir: dir,
        _sock_dir: sock_dir,
        engine,
        _server: server,
        client,
        root,
    }
}

pub(crate) fn wr(root: &Path, rel: &str, content: &str) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, content).unwrap();
}

pub(crate) fn git(repo: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

pub(crate) fn git_out(repo: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success());
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

pub(crate) fn note(body: &str) -> String {
    format!("---\ntype: note\ndescription: d\n---\n{body}\n")
}

pub(crate) fn rel(root: &Path, v: &Value) -> String {
    Path::new(v.as_str().unwrap())
        .strip_prefix(root)
        .unwrap()
        .to_string_lossy()
        .into_owned()
}

#[test]
fn a_folder_moves_whole_with_its_references_following() {
    let mut h = started(&[
        // A self-link and a mutual link between two moved files, both by path.
        (
            "drafts/a.md",
            &note("self [[drafts/a]], sibling [[drafts/sub/b]]"),
        ),
        ("drafts/sub/b.md", &note("back to [[drafts/a]]")),
        ("drafts/pic.png", "png"),
        // An outside referrer by path, re-spelled; one by bare name, untouched.
        ("c.md", &note("see [[drafts/sub/b]] and [[drafts/pic.png]]")),
        ("d.md", &note("see [[a]]")),
    ]);
    fs::create_dir_all(h.root.join("drafts/empty")).unwrap();
    h.engine.rebuild();

    let resp = h
        .client
        .query(&json!({"mutate": "move_dir", "path": "drafts", "to": "notes/drafts"}))
        .unwrap();
    assert_eq!(resp["ready"], true, "got {resp:?}");
    let result = &resp["result"];

    // Every file at its new path, links re-pointed, the two-phase case included.
    let read = |p: &str| fs::read_to_string(h.root.join(p)).unwrap();
    assert_eq!(
        read("notes/drafts/a.md"),
        note("self [[notes/drafts/a]], sibling [[notes/drafts/sub/b]]")
    );
    assert_eq!(
        read("notes/drafts/sub/b.md"),
        note("back to [[notes/drafts/a]]")
    );
    assert_eq!(read("notes/drafts/pic.png"), "png");
    assert_eq!(
        read("c.md"),
        note("see [[notes/drafts/sub/b]] and [[notes/drafts/pic.png]]")
    );
    assert_eq!(read("d.md"), note("see [[a]]"), "a bare name is untouched");

    // The skeleton followed: the empty directory moved, the source is gone.
    assert!(h.root.join("notes/drafts/empty").is_dir());
    assert!(!h.root.join("drafts").exists());
    let untracked: Vec<String> = result["untracked_dirs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| rel(&h.root, p))
        .collect();
    assert_eq!(untracked, vec!["notes/drafts/empty"]);

    // One commit, one Mutation-Id, one Moved: trailer per file.
    let body = git_out(&h.root, &["log", "-1", "--format=%B"]);
    assert_eq!(body.matches("Mutation-Id:").count(), 1, "{body}");
    assert_eq!(body.matches("Moved:").count(), 3, "{body}");
    let changed = git_out(&h.root, &["show", "--name-only", "--format=", "HEAD"]);
    assert!(!changed.lines().any(|l| l == "d.md"), "{changed}");
    assert_eq!(result["commits"].as_object().unwrap().len(), 1);

    // The rewrites report every referrer whose bytes changed, at its final path.
    let rewritten: Vec<String> = result["rewrites"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| rel(&h.root, &r["path"]))
        .collect();
    assert_eq!(
        rewritten,
        vec!["c.md", "notes/drafts/a.md", "notes/drafts/sub/b.md"]
    );

    // Nothing dangles after the rebuild.
    let diags = h.client.query(&json!({"read": "diagnostics"})).unwrap();
    assert_eq!(diags["ready"], true, "got {diags:?}");
    let text = diags["result"].to_string();
    assert!(!text.contains("target-missing"), "{text}");
    assert!(!text.contains("target-not-found"), "{text}");
}

#[test]
fn a_dirty_bare_name_referrer_is_left_out_of_the_move() {
    let mut h = started(&[("drafts/a.md", &note("a")), ("d.md", &note("see [[a]]"))]);
    // A human's uncommitted edit to a referrer the move leaves byte-identical.
    wr(&h.root, "d.md", &note("see [[a]], mid-edit"));
    h.engine.rebuild();
    let resp = h
        .client
        .query(&json!({"mutate": "move_dir", "path": "drafts", "to": "archive"}))
        .unwrap();
    assert_eq!(resp["ready"], true, "got {resp:?}");
    assert!(h.root.join("archive/a.md").is_file());
    assert_eq!(
        fs::read_to_string(h.root.join("d.md")).unwrap(),
        note("see [[a]], mid-edit"),
        "the edit survives"
    );
    let changed = git_out(&h.root, &["show", "--name-only", "--format=", "HEAD"]);
    assert!(!changed.lines().any(|l| l == "d.md"), "{changed}");
}

#[test]
fn a_folder_with_no_files_moves_with_no_commit() {
    let mut h = started(&[]);
    fs::create_dir_all(h.root.join("scratch/inner")).unwrap();
    h.engine.rebuild();
    let head = git_out(&h.root, &["rev-parse", "HEAD"]);
    let resp = h
        .client
        .query(&json!({"mutate": "move_dir", "path": "scratch", "to": "later/scratch"}))
        .unwrap();
    assert_eq!(resp["ready"], true, "got {resp:?}");
    let result = &resp["result"];
    assert_eq!(result["commits"], json!({}));
    assert_eq!(result["hash"], Value::Null);
    assert_eq!(result["reflected"], true);
    assert_eq!(git_out(&h.root, &["rev-parse", "HEAD"]), head, "no commit");
    assert!(h.root.join("later/scratch/inner").is_dir());
    assert!(!h.root.join("scratch").exists());
    let untracked: Vec<String> = result["untracked_dirs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| rel(&h.root, p))
        .collect();
    assert_eq!(
        untracked,
        vec!["later", "later/scratch", "later/scratch/inner"]
    );
}

/// The served knowledge base equals a from-scratch build of the same tree.
pub(crate) fn assert_matches_a_full_build(h: &Harness, ctx: &str) {
    let full = tb_engine::build(&h.root, &tb_parser::RealFileSystem).expect("full build");
    h.engine
        .read(|held| crate::scale_fuzz::assert_same_projections(held, &full, ctx))
        .value()
        .expect("the engine is ready");
}

/// `git status --porcelain` and HEAD, the state a refused op must not change.
pub(crate) fn git_state(repo: &Path) -> (String, String) {
    (
        git_out(repo, &["status", "--porcelain", "--ignored"]),
        git_out(repo, &["rev-parse", "HEAD"]),
    )
}

#[test]
fn the_served_state_after_a_move_matches_a_full_build() {
    let mut h = started(&[
        (
            "drafts/a.md",
            &note("self [[drafts/a]], sibling [[drafts/sub/b]]"),
        ),
        ("drafts/sub/b.md", &note("back to [[drafts/a]]")),
        ("drafts/pic.png", "png"),
        (
            "c.md",
            &note("see [[drafts/sub/b]], [[b]], [[drafts/pic.png]]"),
        ),
        ("d.md", &note("see [[a]]")),
    ]);
    let resp = h
        .client
        .query(&json!({"mutate": "move_dir", "path": "drafts", "to": "notes/drafts"}))
        .unwrap();
    assert_eq!(resp["ready"], true, "got {resp:?}");
    assert_eq!(resp["result"]["reflected"], true);
    assert_matches_a_full_build(&h, "after move_dir");
}

#[test]
fn type_defs_move_with_the_folder_and_every_claim_still_resolves() {
    let mut h = started(&[
        ("type/kinds/task.type.yaml", "fields:\n  title: String\n"),
        ("type/kinds/step.type.yaml", "extends: task\nfields: {}\n"),
        ("t.md", "---\ntype: step\ntitle: t\n---\n"),
    ]);
    let resp = h
        .client
        .query(&json!({"mutate": "move_dir", "path": "type/kinds", "to": "type/work/kinds"}))
        .unwrap();
    assert_eq!(resp["ready"], true, "got {resp:?}");
    assert!(h.root.join("type/work/kinds/task.type.yaml").is_file());
    assert!(h.root.join("type/work/kinds/step.type.yaml").is_file());
    let diags = h.client.query(&json!({"read": "diagnostics"})).unwrap();
    assert_eq!(diags["ready"], true, "got {diags:?}");
    let text = diags["result"].to_string();
    for code in [
        "unknown-type-claim",
        "parent-references-absent-type",
        "duplicate-type-def",
        "type-def-outside-type-dir",
    ] {
        assert!(!text.contains(code), "{code}: {text}");
    }
    assert_matches_a_full_build(&h, "after moving type-defs");
}

#[test]
fn a_git_ignored_file_moves_with_the_folder_outside_git() {
    let mut h = started(&[(".gitignore", ".DS_Store\n"), ("drafts/a.md", &note("a"))]);
    wr(&h.root, "drafts/.DS_Store", "finder");
    h.engine.rebuild();
    let resp = h
        .client
        .query(&json!({"mutate": "move_dir", "path": "drafts", "to": "moved"}))
        .unwrap();
    assert_eq!(resp["ready"], true, "got {resp:?}");
    let result = &resp["result"];
    assert_eq!(
        fs::read_to_string(h.root.join("moved/.DS_Store")).unwrap(),
        "finder"
    );
    assert!(!h.root.join("drafts").exists(), "the source folder is gone");
    let untracked: Vec<String> = result["untracked_files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| rel(&h.root, p))
        .collect();
    assert_eq!(untracked, vec!["drafts/.DS_Store", "moved/.DS_Store"]);
    assert_eq!(
        result["untracked_dirs"],
        json!([]),
        "moved/ holds a tracked file"
    );
    let changed = git_out(&h.root, &["show", "--name-only", "--format=", "HEAD"]);
    assert!(!changed.contains(".DS_Store"), "{changed}");
    assert!(
        git_out(&h.root, &["status", "--porcelain", "--untracked-files=all"]).is_empty(),
        "the tree is clean: the ignored file uncommitted, the saga's run state ignored"
    );
    assert!(
        h.root.join(".arsumbris/au-engine/run/.gitignore").is_file(),
        "the saga's run directory ignores itself"
    );
}

#[test]
fn each_refusal_names_its_cause_and_touches_nothing() {
    type Setup = fn(&Path);
    let cases: [(&str, &str, Setup, &str); 7] = [
        (
            "drafts",
            "moved",
            |r| wr(r, "drafts/inner/.arsumbris/repo.yaml", "name: inner\n"),
            "drafts/inner (nested repo)",
        ),
        (
            "drafts",
            "moved",
            |r| wr(r, "drafts/node_modules/x.js", ""),
            "drafts/node_modules (scoped out)",
        ),
        (
            "drafts",
            "moved",
            |r| fs::create_dir_all(r.join("drafts/clone/.git")).unwrap(),
            "drafts/clone/.git (git or engine directory)",
        ),
        (
            "drafts",
            "moved",
            |r| std::os::unix::fs::symlink(r.join("c.md"), r.join("drafts/link.md")).unwrap(),
            "drafts/link.md (symlink)",
        ),
        ("drafts", "c", |r| wr(r, "c/x.md", "x"), "already exists"),
        ("drafts", "drafts/deeper", |_| {}, "inside it"),
        (
            "drafts",
            "moved",
            |r| wr(r, "drafts/a.md", &note("an uncommitted edit")),
            "uncommitted changes",
        ),
    ];
    for (path, to, setup, cause) in cases {
        let mut h = started(&[("drafts/a.md", &note("a")), ("c.md", &note("[[drafts/a]]"))]);
        setup(&h.root);
        h.engine.rebuild();
        let before = git_state(&h.root);
        let c_before = fs::read_to_string(h.root.join("c.md")).unwrap();
        let resp = h
            .client
            .query(&json!({"mutate": "move_dir", "path": path, "to": to}))
            .unwrap();
        assert_eq!(resp["type"], "error", "{cause}: {resp:?}");
        let err = resp["error"].as_str().unwrap();
        assert!(err.contains(cause), "expected {cause:?} in {err}");
        assert_eq!(git_state(&h.root), before, "{cause}: git state changed");
        assert_eq!(fs::read_to_string(h.root.join("c.md")).unwrap(), c_before);
        assert!(
            h.root.join("drafts/a.md").is_file(),
            "{cause}: the folder stays"
        );
        assert!(
            !h.root.join(to).join("a.md").exists(),
            "{cause}: nothing moved"
        );
    }
}

/// Two member repos, each its own working tree: `ra` holds the folder, `rb`
/// references a file in it by path, across the boundary.
pub(crate) fn two_tree_workspace() -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap();
    crate::seed_workspace(&root, &["ra", "rb"]);
    wr(&root, "ra/.arsumbris/repo.yaml", "name: ra\n");
    wr(
        &root,
        "ra/type/note.type.yaml",
        "fields:\n  description: String\n",
    );
    wr(&root, "ra/drafts/a.md", &note("self [[drafts/a]]"));
    wr(&root, "ra/drafts/sub/b.md", &note("b"));
    fs::create_dir_all(root.join("ra/drafts/empty")).unwrap();
    wr(
        &root,
        "rb/.arsumbris/repo.yaml",
        "name: rb\ndeps:\n  - name: ra\n",
    );
    wr(&root, "rb/doc.md", "see [[drafts/sub/b::ra]]\n");
    git_seed(&root.join("ra"));
    git_seed(&root.join("rb"));
    serve_at(dir, root)
}

#[test]
fn a_move_across_two_working_trees_is_one_saga() {
    let mut h = two_tree_workspace();
    let resp = h
        .client
        .query(&json!({"mutate": "move_dir", "path": "ra/drafts", "to": "ra/notes"}))
        .unwrap();
    assert_eq!(resp["ready"], true, "got {resp:?}");
    assert_eq!(
        fs::read_to_string(h.root.join("rb/doc.md")).unwrap(),
        "see [[notes/sub/b::ra]]\n"
    );
    let id = |r: &str| {
        git_out(&h.root.join(r), &["log", "-1", "--format=%B"])
            .lines()
            .find_map(|l| l.strip_prefix("Mutation-Id: ").map(str::to_string))
            .expect("a Mutation-Id")
    };
    assert_eq!(id("ra"), id("rb"), "one Mutation-Id across both trees");
    assert_eq!(resp["result"]["commits"].as_object().unwrap().len(), 2);
    assert!(h.root.join("ra/notes/empty").is_dir());
    assert_matches_a_full_build(&h, "after a cross-tree move_dir");
}

#[test]
fn a_move_failing_mid_way_leaves_both_trees_and_the_skeleton_untouched() {
    let mut h = two_tree_workspace();
    let before_ra = git_state(&h.root.join("ra"));
    let before_rb = git_state(&h.root.join("rb"));
    // `drafts/a.md` moves first; `drafts/sub/b.md` then cannot leave its
    // read-only directory. By then the referrers are rewritten in both trees and
    // a file and a directory already moved.
    let sub = h.root.join("ra/drafts/sub");
    fs::set_permissions(&sub, std::os::unix::fs::PermissionsExt::from_mode(0o555)).unwrap();
    let resp = h
        .client
        .query(&json!({"mutate": "move_dir", "path": "ra/drafts", "to": "ra/notes"}))
        .unwrap();
    fs::set_permissions(&sub, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    assert_eq!(resp["type"], "error", "got {resp:?}");
    let err = resp["error"].as_str().unwrap();
    assert!(
        err.contains("cannot move") && err.contains("sub/b.md"),
        "{err}"
    );
    assert!(!err.contains("incomplete"), "the rollback completed: {err}");

    assert_eq!(git_state(&h.root.join("ra")), before_ra, "ra untouched");
    assert_eq!(git_state(&h.root.join("rb")), before_rb, "rb untouched");
    assert_eq!(
        fs::read_to_string(h.root.join("ra/drafts/a.md")).unwrap(),
        note("self [[drafts/a]]")
    );
    assert_eq!(
        fs::read_to_string(h.root.join("rb/doc.md")).unwrap(),
        "see [[drafts/sub/b::ra]]\n"
    );
    assert!(
        h.root.join("ra/drafts/empty").is_dir(),
        "the empty directory stays"
    );
    assert!(
        !h.root.join("ra/notes").exists(),
        "no created directory is left"
    );
}
