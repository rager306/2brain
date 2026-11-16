//! A repository whose identity directory is the migrated spelling: `.2brain/`.
//!
//! Tier C accepts both spellings for one release (`docs/research/tier-c-seams.md`). The engine
//! reads identity per ROOT, so every join that finds `repo.yaml`, `workspace.yaml`, the locks, or
//! `.auignore` must look under `.2brain/` when that is the directory the root holds — otherwise a
//! migrated repository is invisible (no member) or, worse, its marker is walked as content.
//!
//! This is the end-to-end proof, on a tempdir fixture: the entry is discovered as its declared
//! repo, its content is served, and the member's `.auignore` is read from the new spelling.

#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use tb_engine::{Client, ConfigSource, Engine, ServeHandle, serve};

struct Harness {
    _dir: tempfile::TempDir,
    _sock_dir: tempfile::TempDir,
    _server: ServeHandle,
    client: Client,
}

fn harness(root: &Path) -> Harness {
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

/// A single-repo knowledge base whose identity directory is `.2brain/`, carrying a scope file
/// under the same directory and one note whose rare term proves the content is served.
fn migrated_repo() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap();
    fs::create_dir_all(root.join(".2brain")).unwrap();
    fs::write(
        root.join(".2brain/repo.yaml"),
        "name: v\ntype: au.engine.repo::au-engine\n",
    )
    .unwrap();
    fs::write(root.join(".2brain/.auignore"), "docs/\n").unwrap();
    fs::write(
        root.join("README.md"),
        "---\ntype: au.engine.readme::au-engine\ntldr: t\n---\n\n# Repo Overview\n\n## What this is\n\nt\n\n## How to use this\n\nt\n\n## How to extend this\n\nt\n",
    )
    .unwrap();
    fs::write(
        root.join("alpha.md"),
        "---\ntitle: alpha\n---\n\nthe zarquon protocol.\n",
    )
    .unwrap();
    (dir, root)
}

fn search(client: &mut Client, query: &str) -> Value {
    let resp = client
        .query(&json!({ "read": "search", "query": query }))
        .unwrap();
    assert_eq!(resp["ready"], true, "{resp}");
    resp["result"]["search"].clone()
}

#[test]
fn a_marker_under_the_new_identity_directory_is_discovered_and_served() {
    let (_dir, root) = migrated_repo();
    let mut h = harness(&root);

    // Discovery: the entry is its DECLARED repo, `v`, not an implicit repo named after the
    // temporary directory. An engine that only looked under `.arsumbris` would find no marker and
    // serve the directory as a fallback repo, or refuse the entry outright.
    let resp = h.client.query(&json!({ "read": "overview" })).unwrap();
    assert_eq!(resp["ready"], true, "{resp}");
    let members = resp["result"]["overview"]["members"]
        .as_array()
        .expect("members is an array");
    assert!(
        members.iter().any(|m| m["repo"] == "v"),
        "the migrated repo is discovered under its declared name: {members:?}"
    );

    // Content: the note is catalogued and searchable, so the walk entered the repository rather
    // than skipping it as a repository boundary it could not identify.
    let found = search(&mut h.client, "zarquon");
    assert_eq!(found["total"], 1, "{found}");
    assert_eq!(found["matches"][0]["path"], json!(root.join("alpha.md")));

    // The scope read resolves `.auignore` under the same directory the marker lives in.
    let resp = h
        .client
        .query(&json!({ "read": "ignores", "repo": "v" }))
        .unwrap();
    assert_eq!(resp["ready"], true, "{resp}");
    let view = &resp["result"]["ignores"][0];
    assert_eq!(view["root"], root.display().to_string());
    assert_eq!(view["patterns"], json!(["docs/"]), "{view}");
}
