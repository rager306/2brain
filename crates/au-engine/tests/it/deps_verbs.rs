//! The `add_dep` / `remove_dep` composition mutations over the socket: author a
//! repo's `deps:` in `<member>/.arsumbris/repo.yaml`. A guard-bypass like
//! `set_config`, but the file is a first-class node, so the change re-resolves the
//! mounted set and rebuilds (a newly-declared peer surfaces `peer-unmounted`).
//! `add_dep` does NOT fetch; a duplicate name rejects; `remove_dep` of an absent
//! name rejects.
//!
//! See [[spec - composition authorship - governed list verbs author workspace members and repo deps, rebuilding on change]].

#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};

use au_engine::{serve, Client, ConfigSource, Engine, ServeHandle};
use serde_json::{json, Value};

fn git(root: &Path, args: &[&str]) {
    let ok = std::process::Command::new("git")
        .current_dir(root)
        .args(args)
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {args:?} failed");
}

fn fixture() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap();
    crate::seed_repo(&root);
    git(&root, &["init", "-q"]);
    git(&root, &["config", "user.email", "t@t"]);
    git(&root, &["config", "user.name", "t"]);
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-q", "-m", "seed"]);
    (dir, root)
}

struct Harness {
    _dir: tempfile::TempDir,
    _sock_dir: tempfile::TempDir,
    _engine: Engine,
    _server: ServeHandle,
    client: Client,
    root: PathBuf,
}

fn started() -> Harness {
    let (dir, root) = fixture();
    let sock_dir = tempfile::tempdir().unwrap();
    let socket = sock_dir.path().join("s");
    let engine = Engine::new(&root, ConfigSource::Empty);
    let server = serve(engine.handle(), &socket).expect("serve");
    engine.rebuild();
    let client = Client::connect(&socket).expect("connect");
    Harness {
        _dir: dir,
        _sock_dir: sock_dir,
        _engine: engine,
        _server: server,
        client,
        root,
    }
}

fn repo_yaml(h: &Harness) -> String {
    fs::read_to_string(h.root.join(".arsumbris").join("repo.yaml")).unwrap()
}

fn codes(resp: &Value) -> Vec<String> {
    resp["result"]["diagnostics"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|d| d["code"].as_str().unwrap_or("").to_string())
        .collect()
}

#[test]
fn add_dep_seeds_deps_commits_and_the_peer_surfaces_unmounted() {
    let mut h = started();
    let resp = h
        .client
        .query(&json!({ "mutate": "add_dep", "name": "lib" }))
        .unwrap();
    assert_eq!(resp["ready"], true, "{resp}");
    assert!(
        resp["result"]["commit"].is_string(),
        "a commit landed: {resp}"
    );
    assert!(resp["result"]["hash"].is_string(), "{resp}");

    assert_eq!(
        repo_yaml(&h),
        "name: v\ntype: au.engine.repo::au-engine\ndeps:\n  - name: lib\n",
        "the deps field is seeded with the entry"
    );

    // The change rebuilt: the newly-declared, unmounted peer now surfaces.
    let diags = h.client.query(&json!({ "read": "diagnostics" })).unwrap();
    assert!(
        codes(&diags).iter().any(|c| c == "peer-unmounted"),
        "the added dep is declared-but-unmounted after the rebuild: {diags}"
    );
}

#[test]
fn add_dep_carries_remote_and_ref() {
    let mut h = started();
    h.client
        .query(&json!({
            "mutate": "add_dep", "name": "lib",
            "remote": "git@example.com:o/lib.git", "ref": "main",
        }))
        .unwrap();
    let y = repo_yaml(&h);
    assert!(y.contains("- name: lib"), "{y}");
    assert!(y.contains("remote: git@example.com:o/lib.git"), "{y}");
    assert!(y.contains("ref: main"), "{y}");
}

#[test]
fn add_dep_appends_a_second_keeping_the_first() {
    let mut h = started();
    h.client
        .query(&json!({ "mutate": "add_dep", "name": "one" }))
        .unwrap();
    h.client
        .query(&json!({ "mutate": "add_dep", "name": "two" }))
        .unwrap();
    assert_eq!(
        repo_yaml(&h),
        "name: v\ntype: au.engine.repo::au-engine\ndeps:\n  - name: one\n  - name: two\n",
        "the second appends after the first"
    );
}

#[test]
fn add_dep_rejects_a_duplicate_name() {
    let mut h = started();
    h.client
        .query(&json!({ "mutate": "add_dep", "name": "lib" }))
        .unwrap();
    let dup = h
        .client
        .query(&json!({ "mutate": "add_dep", "name": "lib" }))
        .unwrap();
    assert_eq!(dup["type"], "error", "a duplicate name rejects: {dup}");
    // Nothing appended twice.
    assert_eq!(repo_yaml(&h).matches("- name: lib").count(), 1);
}

#[test]
fn remove_dep_removes_the_sole_entry_leaving_empty_list() {
    let mut h = started();
    h.client
        .query(&json!({ "mutate": "add_dep", "name": "lib" }))
        .unwrap();
    let resp = h
        .client
        .query(&json!({ "mutate": "remove_dep", "name": "lib" }))
        .unwrap();
    assert_eq!(resp["ready"], true, "{resp}");
    assert!(resp["result"]["commit"].is_string(), "{resp}");
    assert_eq!(
        repo_yaml(&h),
        "name: v\ntype: au.engine.repo::au-engine\ndeps: []\n",
        "the sole element removal collapses deps to []"
    );
}

#[test]
fn remove_dep_keeps_the_other_entries() {
    let mut h = started();
    for name in ["one", "two", "three"] {
        h.client
            .query(&json!({ "mutate": "add_dep", "name": name }))
            .unwrap();
    }
    h.client
        .query(&json!({ "mutate": "remove_dep", "name": "two" }))
        .unwrap();
    assert_eq!(
        repo_yaml(&h),
        "name: v\ntype: au.engine.repo::au-engine\ndeps:\n  - name: one\n  - name: three\n",
        "only the named dep is removed"
    );
}

#[test]
fn remove_dep_rejects_an_absent_name() {
    let mut h = started();
    let resp = h
        .client
        .query(&json!({ "mutate": "remove_dep", "name": "ghost" }))
        .unwrap();
    assert_eq!(resp["type"], "error", "an absent name rejects: {resp}");
}

#[test]
fn add_dep_expected_hash_compare_and_set() {
    let mut h = started();
    // The first add returns the new repo.yaml content hash.
    let first = h
        .client
        .query(&json!({ "mutate": "add_dep", "name": "a" }))
        .unwrap();
    let hash = first["result"]["hash"].as_str().unwrap().to_string();

    // A stale expected_hash rejects, nothing added.
    let bad = h
        .client
        .query(&json!({ "mutate": "add_dep", "name": "b", "expected_hash": "deadbeef" }))
        .unwrap();
    assert_eq!(bad["type"], "error", "a stale expected_hash rejects: {bad}");
    assert!(
        !repo_yaml(&h).contains("- name: b"),
        "nothing added on reject"
    );

    // The matching hash succeeds.
    let good = h
        .client
        .query(&json!({ "mutate": "add_dep", "name": "b", "expected_hash": hash }))
        .unwrap();
    assert_eq!(good["ready"], true, "the matching hash succeeds: {good}");
    assert!(
        repo_yaml(&h).contains("- name: b"),
        "the matching write landed"
    );
}
