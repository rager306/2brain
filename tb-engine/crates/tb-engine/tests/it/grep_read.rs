//! The `grep` read: literal text search over the catalogued files, served over
//! the wire.

#![cfg(unix)]

use std::fs;
use std::path::PathBuf;

use serde_json::json;
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

fn seeded() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap();
    crate::seed_repo(&root);
    fs::write(
        root.join("alpha.md"),
        "---\ntitle: alpha\n---\n\nthe Retry Policy matters\nThe retry policy again\nnothing here\n",
    )
    .unwrap();
    fs::write(
        root.join("beta.md"),
        "---\ntitle: beta\n---\n\nunrelated text\nretry policy in another note\n",
    )
    .unwrap();
    (dir, root)
}

#[test]
fn grep_finds_literal_lines_case_insensitively_by_default() {
    let (_dir, root) = seeded();
    let mut h = harness(&root);
    let resp = h
        .client
        .query(&json!({ "read": "grep", "query": "retry policy" }))
        .unwrap();
    assert_eq!(resp["ready"], true, "{resp}");
    let grep = &resp["result"]["grep"];
    assert_eq!(grep["total"], 3, "{grep}");
    assert_eq!(grep["truncated"], false);
    assert_eq!(grep["skipped_files"], 0);
    let matches = grep["matches"].as_array().unwrap();
    assert_eq!(matches.len(), 3);
    let first = &matches[0];
    assert!(
        first["path"].as_str().unwrap().ends_with("alpha.md"),
        "{first}"
    );
    assert_eq!(first["line"], 5);
    assert_eq!(first["column"], 5, "1-based character column");
    assert_eq!(first["text"], "the Retry Policy matters");
    assert_eq!(first["text_truncated"], false);
    assert_eq!(first["repo"], "v");
}

#[test]
fn grep_case_sensitive_narrows_to_the_exact_case() {
    let (_dir, root) = seeded();
    let mut h = harness(&root);
    let resp = h
        .client
        .query(&json!({ "read": "grep", "query": "Retry Policy", "case_sensitive": true }))
        .unwrap();
    let grep = &resp["result"]["grep"];
    assert_eq!(grep["total"], 1, "{grep}");
    assert_eq!(grep["matches"][0]["line"], 5);
}

#[test]
fn grep_pages_matches_and_says_so() {
    let (_dir, root) = seeded();
    let mut h = harness(&root);
    let first = h
        .client
        .query(&json!({ "read": "grep", "query": "retry policy", "limit": 2 }))
        .unwrap();
    let grep = &first["result"]["grep"];
    assert_eq!(grep["matches"].as_array().unwrap().len(), 2);
    assert_eq!(grep["total"], 3);
    assert_eq!(grep["truncated"], true);
    let second = h
        .client
        .query(&json!({ "read": "grep", "query": "retry policy", "limit": 2, "offset": 2 }))
        .unwrap();
    let grep = &second["result"]["grep"];
    assert_eq!(grep["matches"].as_array().unwrap().len(), 1);
    assert_eq!(grep["truncated"], false);
}

#[test]
fn grep_scopes_to_a_repo_and_rejects_an_unknown_argument() {
    let (_dir, root) = seeded();
    let mut h = harness(&root);
    let scoped = h
        .client
        .query(&json!({ "read": "grep", "query": "retry policy", "repo": "v" }))
        .unwrap();
    assert_eq!(scoped["result"]["grep"]["total"], 3);
    let absent = h
        .client
        .query(&json!({ "read": "grep", "query": "retry policy", "repo": "nope" }))
        .unwrap();
    assert_eq!(absent["result"]["grep"]["total"], 0);
    let unknown = h
        .client
        .query(&json!({ "read": "grep", "query": "x", "regex": true }))
        .unwrap();
    assert_eq!(unknown["type"], "error", "{unknown}");
}

#[test]
fn grep_refuses_an_empty_query_and_counts_what_it_skipped() {
    let (_dir, root) = seeded();
    // A catalogued note above the 1 MiB scan bound is skipped and counted, and
    // its text does not enter the match set. It is written before the build, so
    // the catalogue holds it.
    let mut huge = String::from("---\ntitle: huge\n---\n\nretry policy\n");
    huge.push_str(&"x".repeat(1024 * 1024));
    fs::write(root.join("huge.md"), huge).unwrap();
    let mut h = harness(&root);
    let empty = h
        .client
        .query(&json!({ "read": "grep", "query": "" }))
        .unwrap();
    assert_eq!(empty["type"], "error", "{empty}");
    let resp = h
        .client
        .query(&json!({ "read": "grep", "query": "retry policy" }))
        .unwrap();
    assert_eq!(resp["result"]["grep"]["total"], 3, "{resp}");
    assert_eq!(resp["result"]["grep"]["skipped_files"], 1, "{resp}");
}

/// `grep` shares `search`'s default: the workspace the caller asked about, not the dependencies mounted beside it.
#[test]
fn a_default_grep_scans_the_own_workspace_not_a_mounted_dependency() {
    let dir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap();
    crate::seed_repo(&root);
    fs::create_dir_all(root.join("dep/.2brain")).unwrap();
    fs::write(root.join("dep/.2brain/repo.yaml"), "name: dep\n").unwrap();
    crate::seed_readme(&root.join("dep"));
    fs::write(
        root.join(".2brain/workspace.yaml"),
        "edit:\n  - v\ndiscover:\n  - dep\ntype: tb.engine.workspace::tb-engine\n",
    )
    .unwrap();
    fs::write(
        root.join("own.md"),
        "---\ntitle: own\n---\n\nthe zarquon line\n",
    )
    .unwrap();
    fs::write(
        root.join("dep/borrowed.md"),
        "---\ntitle: borrowed\n---\n\nthe zarquon line\n",
    )
    .unwrap();
    let mut h = harness(&root);

    let default = h
        .client
        .query(&json!({ "read": "grep", "query": "zarquon" }))
        .unwrap();
    assert_eq!(default["result"]["grep"]["total"], 1, "{default}");

    let all = h
        .client
        .query(&json!({ "read": "grep", "query": "zarquon", "scope": "all" }))
        .unwrap();
    assert_eq!(all["result"]["grep"]["total"], 2, "{all}");
}
