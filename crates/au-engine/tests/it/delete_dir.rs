//! The `delete_dir` mutation over the socket: a folder deleted as one saga, its
//! stranded references reported, one last-live commit resolving every file.

#![cfg(unix)]

use std::fs;

use serde_json::json;

use std::path::Path;

use crate::move_dir::{
    assert_matches_a_full_build, git_out, git_state, note, rel, serve_at, started,
    two_tree_workspace, wr,
};

#[test]
fn a_folder_deletes_whole_and_reports_what_it_strands() {
    let mut h = started(&[
        // Two contained files referencing each other: nothing stranded between them.
        ("drafts/a.md", &note("see [[drafts/sub/b]]")),
        ("drafts/sub/b.md", &note("back to [[a]]")),
        ("drafts/pic.png", "png"),
        // Surviving referrers, by path and by bare name.
        ("c.md", &note("see [[drafts/sub/b]] and [[drafts/pic.png]]")),
        ("d.md", &note("see [[a]]")),
        ("e.md", &note("unrelated")),
    ]);
    fs::create_dir_all(h.root.join("drafts/empty")).unwrap();
    h.engine.rebuild();
    let before = git_out(&h.root, &["rev-parse", "HEAD"]);

    let resp = h
        .client
        .query(&json!({"mutate": "delete_dir", "path": "drafts"}))
        .unwrap();
    assert_eq!(resp["ready"], true, "got {resp:?}");
    let result = &resp["result"];
    assert!(
        !h.root.join("drafts").exists(),
        "the folder and its skeleton are gone"
    );

    // Stranded: the surviving referrers only, each link with the file it hit.
    let stranded: Vec<(String, Vec<(String, String)>)> = result["stranded"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                rel(&h.root, &r["path"]),
                r["links"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|l| {
                        (
                            l["target"].as_str().unwrap().to_string(),
                            rel(&h.root, &l["to_file"]),
                        )
                    })
                    .collect(),
            )
        })
        .collect();
    assert_eq!(
        stranded,
        vec![
            (
                "c.md".to_string(),
                vec![
                    (
                        "[[drafts/sub/b]]".to_string(),
                        "drafts/sub/b.md".to_string()
                    ),
                    (
                        "[[drafts/pic.png]]".to_string(),
                        "drafts/pic.png".to_string()
                    ),
                ]
            ),
            (
                "d.md".to_string(),
                vec![("[[a]]".to_string(), "drafts/a.md".to_string())]
            ),
        ]
    );

    // One deletion commit; its parent is the last-live commit, resolving every file.
    assert_eq!(git_out(&h.root, &["rev-parse", "HEAD~1"]), before);
    assert_eq!(result["last_live_commit"], json!(before));
    for f in ["drafts/a.md", "drafts/sub/b.md", "drafts/pic.png"] {
        git_out(&h.root, &["cat-file", "-e", &format!("{before}:{f}")]);
    }
    let changed = git_out(&h.root, &["show", "--name-status", "--format=", "HEAD"]);
    assert_eq!(
        changed.lines().count(),
        3,
        "exactly the three files: {changed}"
    );
    assert!(changed.lines().all(|l| l.starts_with('D')), "{changed}");

    let untracked: Vec<String> = result["untracked_dirs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| rel(&h.root, p))
        .collect();
    assert_eq!(untracked, vec!["drafts/empty"]);

    // The stranded references now dangle, advisory; the served state matches a
    // full build.
    let diags = h
        .client
        .query(&json!({"read": "diagnostics", "path": "c.md"}))
        .unwrap();
    assert!(
        diags["result"].to_string().contains("target-not-found"),
        "{diags:?}"
    );
    assert_matches_a_full_build(&h, "after delete_dir");
}

fn rels(root: &Path, v: &serde_json::Value) -> Vec<String> {
    v.as_array().unwrap().iter().map(|p| rel(root, p)).collect()
}

#[test]
fn a_folder_with_no_files_deletes_with_no_commit_and_no_last_live() {
    let mut h = started(&[]);
    fs::create_dir_all(h.root.join("scratch/inner")).unwrap();
    h.engine.rebuild();
    let head = git_out(&h.root, &["rev-parse", "HEAD"]);
    let resp = h
        .client
        .query(&json!({"mutate": "delete_dir", "path": "scratch"}))
        .unwrap();
    assert_eq!(resp["ready"], true, "got {resp:?}");
    let result = &resp["result"];
    assert!(!h.root.join("scratch").exists());
    assert_eq!(result["commits"], json!({}));
    assert!(result.get("last_live_commit").is_none(), "{result:?}");
    assert_eq!(git_out(&h.root, &["rev-parse", "HEAD"]), head);
    assert_eq!(
        rels(&h.root, &result["untracked_dirs"]),
        vec!["scratch", "scratch/inner"]
    );
    assert_eq!(result["stranded"], json!([]));
}

#[test]
fn a_git_ignored_file_is_deleted_outside_git_with_the_folder() {
    let mut h = started(&[(".gitignore", ".DS_Store\n"), ("drafts/a.md", &note("a"))]);
    wr(&h.root, "drafts/.DS_Store", "finder");
    wr(&h.root, "junk/.DS_Store", "finder");
    h.engine.rebuild();

    // Mixed: the tracked file commits its deletion, the ignored one is removed
    // outside git; the folder held a tracked file, so no untracked directory.
    let resp = h
        .client
        .query(&json!({"mutate": "delete_dir", "path": "drafts"}))
        .unwrap();
    assert_eq!(resp["ready"], true, "got {resp:?}");
    let result = &resp["result"];
    assert!(!h.root.join("drafts").exists());
    assert_eq!(
        rels(&h.root, &result["untracked_files"]),
        vec!["drafts/.DS_Store"]
    );
    assert_eq!(result["untracked_dirs"], json!([]));
    assert!(result["last_live_commit"].is_string());
    assert_eq!(
        result["diagnostics"],
        json!([]),
        "a gone folder has no diagnostics"
    );
    let changed = git_out(&h.root, &["show", "--name-status", "--format=", "HEAD"]);
    assert_eq!(changed, "D\tdrafts/a.md");

    // Only ignored files: no commit, no last-live, the directory untracked.
    let head = git_out(&h.root, &["rev-parse", "HEAD"]);
    let resp = h
        .client
        .query(&json!({"mutate": "delete_dir", "path": "junk"}))
        .unwrap();
    assert_eq!(resp["ready"], true, "got {resp:?}");
    let result = &resp["result"];
    assert!(!h.root.join("junk").exists());
    assert_eq!(result["commits"], json!({}));
    assert!(result.get("last_live_commit").is_none(), "{result:?}");
    assert_eq!(git_out(&h.root, &["rev-parse", "HEAD"]), head);
    assert_eq!(rels(&h.root, &result["untracked_dirs"]), vec!["junk"]);
    assert!(
        git_out(&h.root, &["status", "--porcelain", "--untracked-files=all"]).is_empty(),
        "nothing left behind"
    );
}

#[test]
fn each_refusal_names_its_cause_and_touches_nothing() {
    type Setup = fn(&Path);
    let cases: [(Setup, &str); 5] = [
        (
            |r| wr(r, "drafts/inner/.arsumbris/repo.yaml", "name: inner\n"),
            "drafts/inner (nested repo)",
        ),
        (
            |r| wr(r, "drafts/node_modules/x.js", ""),
            "drafts/node_modules (scoped out)",
        ),
        (
            |r| fs::create_dir_all(r.join("drafts/clone/.git")).unwrap(),
            "drafts/clone/.git (git or engine directory)",
        ),
        (
            |r| std::os::unix::fs::symlink(r.join("c.md"), r.join("drafts/link.md")).unwrap(),
            "drafts/link.md (symlink)",
        ),
        (
            |r| wr(r, "drafts/a.md", &note("an uncommitted edit")),
            "uncommitted changes",
        ),
    ];
    for (setup, cause) in cases {
        let mut h = started(&[("drafts/a.md", &note("a")), ("c.md", &note("[[drafts/a]]"))]);
        setup(&h.root);
        h.engine.rebuild();
        let before = git_state(&h.root);
        let resp = h
            .client
            .query(&json!({"mutate": "delete_dir", "path": "drafts"}))
            .unwrap();
        assert_eq!(resp["type"], "error", "{cause}: {resp:?}");
        let err = resp["error"].as_str().unwrap();
        assert!(err.contains(cause), "expected {cause:?} in {err}");
        assert_eq!(git_state(&h.root), before, "{cause}: git state changed");
        assert!(
            h.root.join("drafts/a.md").is_file(),
            "{cause}: nothing deleted"
        );
    }
}

#[test]
fn a_folder_holding_files_off_git_refuses() {
    let dir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap();
    crate::seed_repo(&root);
    wr(&root, "drafts/a.md", "a\n");
    let mut h = serve_at(dir, root);
    let resp = h
        .client
        .query(&json!({"mutate": "delete_dir", "path": "drafts"}))
        .unwrap();
    assert_eq!(resp["type"], "error", "got {resp:?}");
    assert!(
        resp["error"]
            .as_str()
            .unwrap()
            .contains("no git working tree covers"),
        "{resp:?}"
    );
    assert!(h.root.join("drafts/a.md").is_file());
}

#[test]
fn a_cross_repo_referrer_is_reported_and_its_tree_untouched() {
    let mut h = two_tree_workspace();
    let rb_before = git_state(&h.root.join("rb"));
    let ra_head = git_out(&h.root.join("ra"), &["rev-parse", "HEAD"]);
    let resp = h
        .client
        .query(&json!({"mutate": "delete_dir", "path": "ra/drafts"}))
        .unwrap();
    assert_eq!(resp["ready"], true, "got {resp:?}");
    let result = &resp["result"];
    assert!(!h.root.join("ra/drafts").exists());
    assert_eq!(
        result["stranded"],
        json!([{
            "path": h.root.join("rb/doc.md").display().to_string(),
            "links": [{
                "target": "[[drafts/sub/b::ra]]",
                "to_file": h.root.join("ra/drafts/sub/b.md").display().to_string(),
            }],
        }])
    );
    assert_eq!(
        git_state(&h.root.join("rb")),
        rb_before,
        "the referrer's tree is untouched"
    );
    assert_eq!(
        result["commits"].as_object().unwrap().len(),
        1,
        "only ra commits"
    );
    assert_eq!(result["last_live_commit"], json!(ra_head));
    assert_matches_a_full_build(&h, "after a cross-repo delete_dir");
}

#[test]
fn a_delete_failing_mid_way_leaves_the_folder_and_its_skeleton_untouched() {
    let mut h = two_tree_workspace();
    let before = git_state(&h.root.join("ra"));
    // `drafts/a.md` is removed first; `drafts/sub/b.md` then cannot leave its
    // read-only directory.
    let sub = h.root.join("ra/drafts/sub");
    fs::set_permissions(&sub, std::os::unix::fs::PermissionsExt::from_mode(0o555)).unwrap();
    let resp = h
        .client
        .query(&json!({"mutate": "delete_dir", "path": "ra/drafts"}))
        .unwrap();
    fs::set_permissions(&sub, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    assert_eq!(resp["type"], "error", "got {resp:?}");
    let err = resp["error"].as_str().unwrap();
    assert!(
        err.contains("cannot delete") && err.contains("sub/b.md"),
        "{err}"
    );
    assert!(!err.contains("incomplete"), "the rollback completed: {err}");
    assert_eq!(git_state(&h.root.join("ra")), before);
    assert!(
        h.root.join("ra/drafts/a.md").is_file(),
        "the removed file is back"
    );
    assert!(
        h.root.join("ra/drafts/empty").is_dir(),
        "the empty directory stays"
    );
}
