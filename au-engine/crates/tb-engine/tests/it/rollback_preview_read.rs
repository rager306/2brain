//! The `rollback_preview` read: what reverting a commit would touch, before
//! anything is touched.

#![cfg(unix)]

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use tb_engine::{Client, ConfigSource, Engine, ServeHandle, serve};
use serde_json::{Value, json};

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

fn git(root: &std::path::Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .expect("git runs");
    assert!(out.status.success(), "git {args:?}: {out:?}");
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

fn commit(root: &std::path::Path, message: &str) -> String {
    git(root, &["add", "-A"]);
    git(
        root,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-qm",
            message,
        ],
    );
    git(root, &["rev-parse", "HEAD"])
}

/// One commit adding a note, a second editing it and adding another.
fn seeded() -> (tempfile::TempDir, PathBuf, String, String) {
    let dir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap();
    crate::seed_repo(&root);
    fs::create_dir_all(root.join("notes")).unwrap();
    fs::write(root.join("notes/one.md"), "---\ntitle: one\n---\n\nfirst\n").unwrap();
    git(&root, &["init", "-q"]);
    let first = commit(&root, "add one");
    fs::write(
        root.join("notes/one.md"),
        "---\ntitle: one\n---\n\nfirst, edited\n",
    )
    .unwrap();
    fs::write(
        root.join("notes/two.md"),
        "---\ntitle: two\n---\n\nsecond\n",
    )
    .unwrap();
    let second = commit(&root, "edit one, add two");
    (dir, root, first, second)
}

#[test]
fn preview_names_every_path_and_what_a_revert_would_do() {
    let (_dir, root, _first, second) = seeded();
    let mut h = harness(&root);
    let resp = h
        .client
        .query(&json!({ "read": "rollback_preview", "repo": "v", "commit": second }))
        .unwrap();
    assert_eq!(resp["ready"], true, "{resp}");
    let view = &resp["result"]["rollback_preview"];
    assert_eq!(view["commit"], second);
    assert_eq!(view["subject"], "edit one, add two");
    assert!(view["parent"].is_string(), "{view}");
    assert_eq!(view["safe"], true, "{view}");
    let paths = view["paths"].as_array().unwrap();
    assert_eq!(paths.len(), 2, "{paths:?}");
    let one = paths
        .iter()
        .find(|p| p["path"].as_str().unwrap().ends_with("notes/one.md"))
        .expect("one.md");
    assert_eq!(one["action"], "restore", "an edit reverts by restoring");
    assert_eq!(one["state"], "clean");
    let two = paths
        .iter()
        .find(|p| p["path"].as_str().unwrap().ends_with("notes/two.md"))
        .expect("two.md");
    assert_eq!(two["action"], "delete", "an addition reverts by deleting");
    assert_eq!(two["state"], "clean");
}

#[test]
fn preview_refuses_to_call_a_dirty_or_superseded_path_safe() {
    let (_dir, root, first, second) = seeded();
    let mut h = harness(&root);
    // Uncommitted edit on disk: a revert would clobber it.
    fs::write(root.join("notes/one.md"), "human work in progress\n").unwrap();
    let dirty = h
        .client
        .query(&json!({ "read": "rollback_preview", "repo": "v", "commit": second }))
        .unwrap();
    let view = &dirty["result"]["rollback_preview"];
    assert_eq!(view["safe"], false, "{view}");
    let one = view["paths"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["path"].as_str().unwrap().ends_with("notes/one.md"))
        .unwrap();
    assert_eq!(one["state"], "dirty");
    // Put it back, then look at the FIRST commit: its own path was rewritten by
    // the second, so reverting it is not safe either.
    git(&root, &["checkout", "--", "notes/one.md"]);
    let superseded = h
        .client
        .query(&json!({ "read": "rollback_preview", "repo": "v", "commit": first }))
        .unwrap();
    let view = &superseded["result"]["rollback_preview"];
    assert_eq!(view["safe"], false, "{view}");
    let one = view["paths"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["path"].as_str().unwrap().ends_with("notes/one.md"))
        .unwrap();
    assert_eq!(one["state"], "changed_since");
}

#[test]
fn preview_rejects_an_unknown_commit_and_member() {
    let (_dir, root, _first, _second) = seeded();
    let mut h = harness(&root);
    let unknown = h
        .client
        .query(&json!({ "read": "rollback_preview", "repo": "v", "commit": "deadbeef" }))
        .unwrap();
    assert_eq!(unknown["type"], "error", "{unknown}");
    assert!(
        unknown["error"].as_str().unwrap().contains("not present"),
        "{unknown}"
    );
    let member = h
        .client
        .query(&json!({ "read": "rollback_preview", "repo": "nope", "commit": "HEAD" }))
        .unwrap();
    assert_eq!(member["type"], "error", "{member}");
    assert!(
        member["error"]
            .as_str()
            .unwrap()
            .contains("unknown workspace member")
    );
    let empty = h
        .client
        .query(&json!({ "read": "rollback_preview", "repo": "", "commit": "HEAD" }))
        .unwrap();
    assert_eq!(empty["type"], "error", "{empty}");
}
