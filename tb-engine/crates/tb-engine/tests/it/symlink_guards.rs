//! A symlink on a mutation's path, or a destination the graph cannot hold,
//! refuses and touches nothing.
//!
//! Git passes a path beyond a symlink as clean and then refuses to stage it, so
//! a saga over one could neither commit nor roll back: a folder move lost the
//! moved files on rollback, a folder delete removed the link target's files
//! with no commit. The engine refuses before writing instead.

#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::move_dir::{Harness, git_seed, git_state, note, serve_at, wr};

/// A served git repo with `files` plus a COMMITTED symlink `link` to an outside
/// directory holding `outside`. The outside root rides along so a test checks
/// its files survive.
fn with_link(
    link: &str,
    outside: &[(&str, &str)],
    files: &[(&str, &str)],
) -> (Harness, tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap();
    let ext_dir = tempfile::tempdir().unwrap();
    let ext = fs::canonicalize(ext_dir.path()).unwrap();
    for (p, c) in outside {
        wr(&ext, p, c);
    }
    crate::seed_repo(&root);
    wr(
        &root,
        "type/note.type.yaml",
        "fields:\n  description: String\n",
    );
    for (p, c) in files {
        wr(&root, p, c);
    }
    let at = root.join(link);
    fs::create_dir_all(at.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&ext, &at).unwrap();
    git_seed(&root);
    (serve_at(dir, root), ext_dir, ext)
}

fn mutate(h: &mut Harness, req: Value) -> Value {
    h.client.query(&req).unwrap()
}

/// The request refused, naming `cause`, and neither the repo nor the outside
/// directory changed.
fn assert_refused(h: &mut Harness, req: Value, cause: &str, ext: &Path, outside: &[&str]) {
    let before = git_state(&h.root);
    let resp = mutate(h, req.clone());
    assert_eq!(resp["type"], "error", "{req}: {resp:?}");
    let err = resp["error"].as_str().unwrap();
    assert!(err.contains(cause), "{req}: expected {cause:?} in {err}");
    assert_eq!(git_state(&h.root), before, "{req}: git state changed");
    for f in outside {
        assert!(ext.join(f).is_file(), "{req}: the outside file {f} is gone");
    }
}

#[test]
fn a_symlinked_folder_refuses_each_folder_verb() {
    for req in [
        json!({"mutate": "move_dir", "path": "drafts", "to": "notes"}),
        json!({"mutate": "delete_dir", "path": "drafts"}),
    ] {
        let (mut h, _ext_dir, ext) = with_link(
            "drafts",
            &[("a.md", &note("a"))],
            &[("c.md", &note("see [[drafts/a]]"))],
        );
        assert_refused(&mut h, req.clone(), "drafts (symlink)", &ext, &["a.md"]);
        let p = preview(&mut h, &req);
        assert!(
            p["reject"]["message"]
                .as_str()
                .unwrap_or_default()
                .contains("drafts (symlink)"),
            "the preview refuses alike: {p:?}"
        );
    }
}

#[test]
fn a_folder_under_a_symlinked_directory_refuses_each_folder_verb() {
    for req in [
        json!({"mutate": "move_dir", "path": "linked/drafts", "to": "notes"}),
        json!({"mutate": "delete_dir", "path": "linked/drafts"}),
    ] {
        let (mut h, _ext_dir, ext) = with_link("linked", &[("drafts/a.md", &note("a"))], &[]);
        assert_refused(&mut h, req, "linked (symlink)", &ext, &["drafts/a.md"]);
    }
}

#[test]
fn a_destination_the_graph_cannot_hold_refuses_move_dir_and_rename() {
    // Each destination hazard, for a folder move and a file rename.
    let cases: [(&str, &str); 3] = [
        ("linked/moved", "linked (symlink)"),
        ("nest/moved", "nest (nested repo)"),
        ("Notes/moved", "collides case-insensitively"),
    ];
    for (to, cause) in cases {
        let (mut h, _ext_dir, ext) = with_link(
            "linked",
            &[("keep.md", "k")],
            &[
                ("drafts/a.md", &note("a")),
                ("c.md", &note("see [[drafts/a]]")),
                ("notes/x.md", &note("x")),
                ("nest/.2brain/repo.yaml", "name: nest\n"),
            ],
        );
        assert_refused(
            &mut h,
            json!({"mutate": "move_dir", "path": "drafts", "to": to}),
            cause,
            &ext,
            &["keep.md"],
        );
        assert_refused(
            &mut h,
            json!({"mutate": "rename", "path": "drafts/a.md", "to": format!("{to}/a.md")}),
            cause,
            &ext,
            &["keep.md"],
        );
        assert!(h.root.join("drafts/a.md").is_file());
    }
}

#[test]
fn a_file_write_under_a_symlinked_directory_refuses() {
    // Git cannot stage or restore a path beyond a symlink, so no write verb
    // may act there: it would land uncommitted, or roll back into loss.
    for req in [
        json!({"mutate": "write_file", "path": "linked/a.md", "content": note("new")}),
        json!({"mutate": "edit_file", "path": "linked/a.md", "old_string": "d\n---", "new_string": "e\n---"}),
        json!({"mutate": "delete_file", "path": "linked/a.md"}),
        json!({"mutate": "rename", "path": "linked/a.md", "to": "moved.md"}),
    ] {
        let (mut h, _ext_dir, ext) = with_link(
            "linked",
            &[("a.md", &note("a"))],
            &[("c.md", &note("see [[linked/a]]"))],
        );
        let p = preview(&mut h, &req);
        assert!(
            p["reject"]["message"]
                .as_str()
                .unwrap_or_default()
                .contains("beyond a symbolic link"),
            "{req}: the preview refuses alike: {p:?}"
        );
        assert_refused(&mut h, req, "beyond a symbolic link", &ext, &["a.md"]);
        assert_eq!(fs::read_to_string(ext.join("a.md")).unwrap(), note("a"));
    }
}

#[test]
fn a_referrer_the_move_leaves_untouched_under_a_symlink_never_refuses_it() {
    // The bare `[[a]]` / `[[b]]` re-emit byte-identical when the files keep
    // their names, so the move never stages or restores the referrer beyond the
    // link, and git's refusal to classify it has no bearing on the move.
    for (req, moved) in [
        (
            json!({"mutate": "rename", "path": "a.md", "to": "sub/a.md"}),
            "sub/a.md",
        ),
        (
            json!({"mutate": "move_dir", "path": "drafts", "to": "notes"}),
            "notes/b.md",
        ),
    ] {
        let (mut h, _ext_dir, ext) = with_link(
            "linked",
            &[("r.md", &note("see [[a]] and [[b]]"))],
            &[("a.md", &note("a")), ("drafts/b.md", &note("b"))],
        );
        let p = preview(&mut h, &req);
        assert!(p["reject"].is_null(), "{req}: the preview lands: {p:?}");
        let resp = mutate(&mut h, req.clone());
        assert_eq!(resp["type"], "response", "{req}: the move lands: {resp:?}");
        assert!(h.root.join(moved).is_file(), "{req}: moved to {moved}");
        assert_eq!(
            fs::read_to_string(ext.join("r.md")).unwrap(),
            note("see [[a]] and [[b]]"),
            "{req}: the referrer beyond the link is untouched"
        );
    }
}

#[test]
fn a_referrer_the_move_rewrites_under_a_symlink_still_refuses_it() {
    let req = json!({"mutate": "move_dir", "path": "drafts", "to": "notes"});
    let (mut h, _ext_dir, ext) = with_link(
        "linked",
        &[("r.md", &note("see [[drafts/b]]"))],
        &[("drafts/b.md", &note("b"))],
    );
    assert_refused(&mut h, req, "beyond a symbolic link", &ext, &["r.md"]);
    assert_eq!(
        fs::read_to_string(ext.join("r.md")).unwrap(),
        note("see [[drafts/b]]")
    );
}

/// The `preview_mutation` payload for a `mutate` request's op.
fn preview(h: &mut Harness, req: &Value) -> Value {
    let mut req = req.clone();
    let op = req["mutate"].clone();
    req.as_object_mut().unwrap().remove("mutate");
    req["op"] = op;
    req["read"] = json!("preview_mutation");
    let resp = h.client.query(&req).unwrap();
    assert_eq!(resp["ready"], true, "{resp:?}");
    resp["result"]["preview_mutation"].clone()
}
