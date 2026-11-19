//! The `batch` mutation: several file-level ops in one commit, all or nothing.

#![cfg(unix)]

use std::fs;
use std::path::PathBuf;

use serde_json::{Value, json};
use tb_engine::{Client, ConfigSource, Engine, ServeHandle, serve};

struct Harness {
    _dir: tempfile::TempDir,
    _sock_dir: tempfile::TempDir,
    _server: ServeHandle,
    client: Client,
}

fn harness(root: &std::path::Path) -> Harness {
    let dir_keep = tempfile::tempdir().unwrap();
    let sock_dir = tempfile::tempdir().unwrap();
    let socket = sock_dir.path().join("s");
    let engine = Engine::new(root, ConfigSource::Empty);
    engine.rebuild();
    let server = serve(engine.handle(), &socket).expect("serve");
    let client = Client::connect(&socket).expect("connect");
    Harness {
        _dir: dir_keep,
        _sock_dir: sock_dir,
        _server: server,
        client,
    }
}

/// A git working tree with one committed note, so mutations have a clean base.
fn seeded() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap();
    crate::seed_repo(&root);
    fs::create_dir_all(root.join("notes")).unwrap();
    fs::write(root.join("notes/one.md"), "---\ntitle: one\n---\n\nfirst\n").unwrap();
    fs::write(
        root.join("notes/two.md"),
        "---\ntitle: two\n---\n\nsecond\n",
    )
    .unwrap();
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(&root)
            .output()
            .expect("git runs");
        assert!(out.status.success(), "git {args:?}: {out:?}");
    };
    git(&["init", "-q"]);
    git(&["add", "-A"]);
    git(&[
        "-c",
        "user.name=t",
        "-c",
        "user.email=t@t",
        "commit",
        "-qm",
        "seed",
    ]);
    (dir, root)
}

fn head(root: &std::path::Path) -> String {
    let out = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(root)
        .output()
        .unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

fn commit_count(root: &std::path::Path) -> usize {
    let out = std::process::Command::new("git")
        .args(["rev-list", "--count", "HEAD"])
        .current_dir(root)
        .output()
        .unwrap();
    String::from_utf8(out.stdout)
        .unwrap()
        .trim()
        .parse()
        .unwrap()
}

fn hash_of(client: &mut Client, path: &str) -> String {
    let resp = client
        .query(&json!({ "read": "content", "path": path }))
        .unwrap();
    resp["result"]["content"]["hash"]
        .as_str()
        .expect("a readable file carries a hash")
        .to_owned()
}

#[test]
fn batch_writes_every_op_in_one_commit() {
    let (_dir, root) = seeded();
    let mut h = harness(&root);
    let one = hash_of(&mut h.client, &format!("{}/notes/one.md", root.display()));
    let before = (head(&root), commit_count(&root));
    let resp = h
        .client
        .query(&json!({
            "mutate": "batch",
            "ops": [
                {"op": "write_file", "path": format!("{}/notes/one.md", root.display()),
                 "content": "---\ntitle: one\n---\n\nfirst, revised\n", "expected_hash": one},
                {"op": "write_file", "path": format!("{}/notes/three.md", root.display()),
                 "content": "---\ntitle: three\n---\n\nnew\n"},
                {"op": "edit_file", "path": format!("{}/notes/two.md", root.display()),
                 "old_string": "second", "new_string": "second, revised"},
            ]
        }))
        .unwrap();
    assert_ne!(resp["type"], "error", "{resp}");
    let result = &resp["result"];
    let ops = result["ops"].as_array().unwrap();
    assert_eq!(ops.len(), 3, "{ops:?}");
    assert_eq!(ops[0]["op"], "write_file");
    assert!(ops[0]["hash"].as_str().is_some());
    assert_eq!(ops[2]["op"], "edit_file");
    assert_eq!(result["reflected"], true);
    assert!(result["commit"].is_null(), "a batch names no single file");
    assert!(
        result["commits"].as_object().unwrap().len() == 1,
        "one commit per touched member: {result}"
    );
    assert!(
        fs::read_to_string(root.join("notes/one.md"))
            .unwrap()
            .contains("first, revised")
    );
    assert!(root.join("notes/three.md").is_file());
    assert!(
        fs::read_to_string(root.join("notes/two.md"))
            .unwrap()
            .contains("second, revised")
    );
    assert_eq!(commit_count(&root), before.1 + 1, "exactly one commit");
    assert_ne!(head(&root), before.0);
}

#[test]
fn a_rejected_op_leaves_every_file_as_it_was() {
    let (_dir, root) = seeded();
    let mut h = harness(&root);
    let stale = "0000000000000000";
    let before = (head(&root), commit_count(&root));
    let one_before = fs::read_to_string(root.join("notes/one.md")).unwrap();
    let resp = h
        .client
        .query(&json!({
            "mutate": "batch",
            "ops": [
                {"op": "write_file", "path": format!("{}/notes/one.md", root.display()),
                 "content": "---\ntitle: one\n---\n\nreplaced\n"},
                {"op": "write_file", "path": format!("{}/notes/zzz.md", root.display()),
                 "content": "never\n", "expected_hash": stale},
            ]
        }))
        .unwrap();
    assert_eq!(resp["type"], "error", "{resp}");
    assert!(
        resp["error"].as_str().unwrap().contains("expected_hash"),
        "{resp}"
    );
    assert_eq!(
        fs::read_to_string(root.join("notes/one.md")).unwrap(),
        one_before,
        "the first op must be compensated"
    );
    assert!(!root.join("notes/zzz.md").exists());
    assert_eq!(head(&root), before.0, "nothing was committed");
    assert_eq!(commit_count(&root), before.1);
}

#[test]
fn batch_refuses_an_empty_or_oversized_list_and_an_unknown_op() {
    let (_dir, root) = seeded();
    let mut h = harness(&root);
    let empty = h
        .client
        .query(&json!({ "mutate": "batch", "ops": [] }))
        .unwrap();
    assert_eq!(empty["type"], "error", "{empty}");
    assert!(empty["error"].as_str().unwrap().contains("empty"));
    let ops: Vec<Value> = (0..51)
        .map(|i| json!({"op": "write_file", "path": format!("{}/notes/n{i}.md", root.display()), "content": "x"}))
        .collect();
    let oversized = h
        .client
        .query(&json!({ "mutate": "batch", "ops": ops }))
        .unwrap();
    assert_eq!(oversized["type"], "error", "{oversized}");
    assert!(oversized["error"].as_str().unwrap().contains("bound"));
    let unknown = h
        .client
        .query(&json!({ "mutate": "batch", "ops": [{"op": "rename", "path": "x"}] }))
        .unwrap();
    assert_eq!(unknown["type"], "error", "{unknown}");
}

#[test]
fn a_batch_that_touches_one_path_twice_reports_reflected() {
    // `reflected` says the held catalog agrees with every hash the batch wrote. Two ops on
    // one path write two hashes, and the catalog can only hold the LAST one, so comparing
    // every intermediate hash reports false forever — a consumer waiting for the write to
    // be reflected would wait without end.
    let (_dir, root) = seeded();
    let mut h = harness(&root);
    let one = format!("{}/notes/one.md", root.display());
    let resp = h
        .client
        .query(&json!({
            "mutate": "batch",
            "ops": [
                {"op": "write_file", "path": one, "content": "---\ntitle: one\n---\n\nstep 1\n"},
                {"op": "write_file", "path": one, "content": "---\ntitle: one\n---\n\nstep 2\n"},
            ]
        }))
        .unwrap();
    assert_eq!(resp["type"], "response", "{resp}");
    assert_eq!(
        resp["result"]["reflected"], true,
        "the catalog holds the final content, so every path is reflected: {resp}"
    );
    assert!(
        fs::read_to_string(root.join("notes/one.md"))
            .unwrap()
            .contains("step 2")
    );
}

#[test]
fn require_clean_refuses_a_batch_and_names_the_op_that_would_dirty_it() {
    let (_dir, root) = seeded();
    let mut h = harness(&root);
    let before = (head(&root), commit_count(&root));
    let one_before = fs::read_to_string(root.join("notes/one.md")).unwrap();
    let resp = h
        .client
        .query(&json!({
            "mutate": "batch",
            "require_clean": true,
            "ops": [
                {"op": "write_file", "path": format!("{}/notes/one.md", root.display()),
                 "content": "---\ntitle: one\n---\n\nclean, revised\n"},
                {"op": "write_file", "path": format!("{}/notes/bad.md", root.display()),
                 "content": "---\ntype: note::ghost\ndescription: must not land\n---\n"},
            ]
        }))
        .unwrap();
    assert_eq!(resp["type"], "error", "{resp}");
    let message = resp["error"].as_str().unwrap();
    assert!(message.contains("policy/diagnostics"), "{message}");
    assert!(
        message.contains("batch op 2 (write_file"),
        "the refusal names the op that would dirty the state: {message}"
    );
    assert!(message.contains("type-repo-unknown"), "{message}");
    assert!(!root.join("notes/bad.md").exists(), "nothing landed");
    assert_eq!(
        fs::read_to_string(root.join("notes/one.md")).unwrap(),
        one_before,
        "the clean op ahead of it is compensated too"
    );
    assert_eq!(head(&root), before.0, "nothing was committed");
    assert_eq!(commit_count(&root), before.1);
}

#[test]
fn require_clean_lets_a_clean_batch_land() {
    let (_dir, root) = seeded();
    let mut h = harness(&root);
    let resp = h
        .client
        .query(&json!({
            "mutate": "batch",
            "require_clean": true,
            "ops": [
                {"op": "write_file", "path": format!("{}/notes/one.md", root.display()),
                 "content": "---\ntitle: one\n---\n\nclean, revised\n"},
                {"op": "edit_file", "path": format!("{}/notes/two.md", root.display()),
                 "old_string": "second", "new_string": "second, revised"},
            ]
        }))
        .unwrap();
    assert_eq!(resp["type"], "response", "{resp}");
    assert_eq!(resp["ready"], true, "{resp}");
    assert!(
        fs::read_to_string(root.join("notes/two.md"))
            .unwrap()
            .contains("second, revised")
    );
}

#[test]
fn a_delete_inside_a_batch_uses_its_own_hash_guard() {
    let (_dir, root) = seeded();
    let mut h = harness(&root);
    let two = hash_of(&mut h.client, &format!("{}/notes/two.md", root.display()));
    let stale = h
        .client
        .query(&json!({
            "mutate": "batch",
            "ops": [{"op": "delete_file", "path": format!("{}/notes/two.md", root.display()),
                     "expected_hash": "0000000000000000"}]
        }))
        .unwrap();
    assert_eq!(stale["type"], "error", "{stale}");
    assert!(
        root.join("notes/two.md").is_file(),
        "a stale delete writes nothing"
    );
    let ok = h
        .client
        .query(&json!({
            "mutate": "batch",
            "ops": [{"op": "delete_file", "path": format!("{}/notes/two.md", root.display()),
                     "expected_hash": two}]
        }))
        .unwrap();
    assert_ne!(ok["type"], "error", "{ok}");
    assert!(!root.join("notes/two.md").exists());
    assert!(ok["result"]["ops"][0]["hash"].is_null());
}
