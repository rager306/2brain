//! The `preview_mutation` read over the folder verbs: `move_dir` and
//! `delete_dir` previewed without writing, each product equal to what the
//! committing verb then reports, each refusal the verb's own.

#![cfg(unix)]

use std::fs;
use std::path::Path;

use serde_json::{Value, json};

use crate::move_dir::{Harness, git_state, note, rel, serve_at, started, wr};

/// The `preview_mutation` payload for a request.
fn preview(h: &mut Harness, req: Value) -> Value {
    let mut req = req;
    req["read"] = json!("preview_mutation");
    let resp = h.client.query(&req).unwrap();
    assert_eq!(resp["ready"], true, "{resp:?}");
    resp["result"]["preview_mutation"].clone()
}

fn rels(root: &Path, v: &Value) -> Vec<String> {
    v.as_array()
        .unwrap_or_else(|| panic!("expected a list: {v:?}"))
        .iter()
        .map(|p| rel(root, p))
        .collect()
}

fn codes(diags: &Value) -> Vec<String> {
    diags
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["code"].as_str().unwrap().to_string())
        .collect()
}

/// The folder the first two tests move or delete: files referencing each
/// other and themselves by path, an asset, an empty directory, and outside
/// referrers by path and by bare name.
fn drafts() -> Harness {
    let h = started(&[
        (
            "drafts/a.md",
            &note("self [[drafts/a]], sibling [[drafts/sub/b]]"),
        ),
        ("drafts/sub/b.md", &note("back to [[drafts/a]]")),
        ("drafts/pic.png", "png"),
        ("c.md", &note("see [[drafts/sub/b]] and [[drafts/pic.png]]")),
        ("d.md", &note("see [[a]]")),
    ]);
    fs::create_dir_all(h.root.join("drafts/empty")).unwrap();
    h.engine.rebuild();
    h
}

#[test]
fn a_folder_move_previews_exactly_what_the_move_then_writes() {
    let mut h = drafts();
    let before = git_state(&h.root);
    let p = preview(
        &mut h,
        json!({"op": "move_dir", "path": "drafts", "to": "notes/drafts"}),
    );

    // The verdict reads the folder at its destination: its files' diagnostics,
    // no hash and no identities, the committing verb's result shape.
    assert_eq!(rel(&h.root, &p["target"]["path"]), "notes/drafts");
    assert_eq!(p["target"]["hash"], Value::Null);
    assert_eq!(p["target"]["identities"], json!([]));
    assert_eq!(p["target"]["diagnostics"], json!([]), "{p:?}");
    // Every reference follows, nothing breaks, the source folder is no entry.
    assert_eq!(p["blast_radius"], json!([]), "{p:?}");
    assert_eq!(
        rels(
            &h.root,
            &json!(
                p["rewrites"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|r| r["path"].clone())
                    .collect::<Vec<_>>()
            )
        ),
        vec!["c.md", "notes/drafts/a.md", "notes/drafts/sub/b.md"],
        "the bare-name referrer d.md is left byte-identical"
    );
    assert_eq!(
        rels(&h.root, &p["untracked_dirs"]),
        vec!["notes/drafts/empty"]
    );
    assert_eq!(p["untracked_files"], json!([]));
    assert!(p.get("stranded").is_none(), "{p:?}");

    // A preview writes nothing and commits nothing.
    assert_eq!(git_state(&h.root), before);
    assert!(h.root.join("drafts/empty").is_dir());
    assert!(!h.root.join("notes").exists());

    // The committing verb reports the same rewrites and untracked directories.
    let resp = h
        .client
        .query(&json!({"mutate": "move_dir", "path": "drafts", "to": "notes/drafts"}))
        .unwrap();
    assert_eq!(resp["ready"], true, "got {resp:?}");
    assert_eq!(resp["result"]["rewrites"], p["rewrites"]);
    assert_eq!(resp["result"]["untracked_dirs"], p["untracked_dirs"]);
    assert_eq!(resp["result"]["diagnostics"], p["target"]["diagnostics"]);
}

#[test]
fn a_folder_delete_previews_its_stranded_report_and_the_dangling_referrers() {
    let mut h = drafts();
    let before = git_state(&h.root);
    let p = preview(&mut h, json!({"op": "delete_dir", "path": "drafts"}));

    // The folder is gone in the preview: no files, no diagnostics.
    assert_eq!(rel(&h.root, &p["target"]["path"]), "drafts");
    assert_eq!(p["target"]["diagnostics"], json!([]), "{p:?}");
    assert!(p.get("rewrites").is_none(), "{p:?}");
    assert_eq!(rels(&h.root, &p["untracked_dirs"]), vec!["drafts/empty"]);

    // The blast radius is the surviving referrers, each now dangling, and no
    // entry for a deleted file.
    let blast: Vec<(String, Vec<String>)> = p["blast_radius"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| (rel(&h.root, &e["path"]), codes(&e["diagnostics"])))
        .collect();
    let paths: Vec<&str> = blast.iter().map(|(p, _)| p.as_str()).collect();
    assert_eq!(paths, vec!["c.md", "d.md"], "{p:?}");
    for (path, codes) in &blast {
        assert!(
            codes.iter().any(|c| c == "navigational-target-not-found"),
            "{path}: {codes:?}"
        );
    }

    assert_eq!(git_state(&h.root), before, "a preview writes nothing");
    assert!(h.root.join("drafts/a.md").is_file());

    // The committing verb strands exactly the previewed references.
    let resp = h
        .client
        .query(&json!({"mutate": "delete_dir", "path": "drafts"}))
        .unwrap();
    assert_eq!(resp["ready"], true, "got {resp:?}");
    assert_eq!(resp["result"]["stranded"], p["stranded"]);
    assert_eq!(resp["result"]["untracked_dirs"], p["untracked_dirs"]);
}

#[test]
fn a_moved_type_def_folder_surfaces_its_files_diagnostics_as_the_target() {
    let mut h = started(&[("n.md", &note("n"))]);
    let p = preview(
        &mut h,
        json!({"op": "move_dir", "path": "type", "to": "schema"}),
    );
    // The def keeps its name, so every claim still resolves; only the advisory
    // placement warning appears, on the moved file, in the folder's verdict.
    assert_eq!(
        codes(&p["target"]["diagnostics"]),
        vec!["type-def-outside-type-dir"]
    );
    assert_eq!(p["blast_radius"], json!([]), "{p:?}");
}

#[test]
fn a_folder_with_no_files_previews_its_skeleton_only() {
    let mut h = started(&[]);
    fs::create_dir_all(h.root.join("scratch/inner")).unwrap();
    h.engine.rebuild();
    let mv = preview(
        &mut h,
        json!({"op": "move_dir", "path": "scratch", "to": "later/scratch"}),
    );
    assert_eq!(mv["rewrites"], json!([]));
    assert_eq!(
        rels(&h.root, &mv["untracked_dirs"]),
        vec!["later", "later/scratch", "later/scratch/inner"]
    );
    let del = preview(&mut h, json!({"op": "delete_dir", "path": "scratch"}));
    assert_eq!(del["stranded"], json!([]));
    assert_eq!(
        rels(&h.root, &del["untracked_dirs"]),
        vec!["scratch", "scratch/inner"]
    );
    assert!(h.root.join("scratch/inner").is_dir());
}

#[test]
fn a_git_ignored_file_previews_as_untracked() {
    let mut h = started(&[("drafts/a.md", &note("a")), (".gitignore", ".DS_Store\n")]);
    wr(&h.root, "drafts/.DS_Store", "finder");
    h.engine.rebuild();
    let mv = preview(
        &mut h,
        json!({"op": "move_dir", "path": "drafts", "to": "moved"}),
    );
    assert_eq!(
        rels(&h.root, &mv["untracked_files"]),
        vec!["drafts/.DS_Store", "moved/.DS_Store"]
    );
    let del = preview(&mut h, json!({"op": "delete_dir", "path": "drafts"}));
    assert_eq!(
        rels(&h.root, &del["untracked_files"]),
        vec!["drafts/.DS_Store"]
    );
}

#[test]
fn each_folder_refusal_is_previewed_as_the_verb_refuses() {
    type Setup = fn(&Path);
    let cases: [(&str, Setup, &str); 6] = [
        (
            "moved",
            |r| wr(r, "drafts/inner/.2brain/repo.yaml", "name: inner\n"),
            "drafts/inner (nested repo)",
        ),
        (
            "moved",
            |r| wr(r, "drafts/node_modules/x.js", ""),
            "drafts/node_modules (scoped out)",
        ),
        (
            "moved",
            |r| fs::create_dir_all(r.join("drafts/clone/.git")).unwrap(),
            "drafts/clone/.git (git or engine directory)",
        ),
        (
            "moved",
            |r| std::os::unix::fs::symlink(r.join("c.md"), r.join("drafts/link.md")).unwrap(),
            "drafts/link.md (symlink)",
        ),
        ("c", |r| wr(r, "c/x.md", "x"), "already exists"),
        ("drafts/deeper", |_| {}, "inside it"),
    ];
    for (to, setup, cause) in cases {
        let mut h = started(&[("drafts/a.md", &note("a")), ("c.md", &note("[[drafts/a]]"))]);
        setup(&h.root);
        h.engine.rebuild();
        let verb = h
            .client
            .query(&json!({"mutate": "move_dir", "path": "drafts", "to": to}))
            .unwrap();
        let verb_err = verb["error"]
            .as_str()
            .unwrap_or_else(|| panic!("{cause}: {verb:?}"));
        let p = preview(
            &mut h,
            json!({"op": "move_dir", "path": "drafts", "to": to}),
        );
        let msg = p["reject"]["message"].as_str().unwrap_or_default();
        assert!(msg.contains(cause), "{cause}: {p:?}");
        assert_eq!(
            msg, verb_err,
            "{cause}: the preview refuses as the verb does"
        );
        // The folder-scoped refusals bind a delete alike; a destination one cannot.
        if !matches!(to, "c" | "drafts/deeper") {
            let del = preview(&mut h, json!({"op": "delete_dir", "path": "drafts"}));
            assert!(
                del["reject"]["message"]
                    .as_str()
                    .unwrap_or_default()
                    .contains(cause),
                "{cause}: {del:?}"
            );
        }
    }
}

#[test]
fn a_folder_holding_files_off_git_previews_the_verbs_refusal() {
    let dir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap();
    crate::seed_repo(&root);
    wr(&root, "drafts/a.md", "a\n");
    let mut h = serve_at(dir, root);
    for req in [
        json!({"op": "move_dir", "path": "drafts", "to": "moved"}),
        json!({"op": "delete_dir", "path": "drafts"}),
    ] {
        let p = preview(&mut h, req.clone());
        let msg = p["reject"]["message"].as_str().unwrap_or_default();
        assert!(msg.contains("no git working tree covers"), "{req}: {p:?}");
    }
    assert!(h.root.join("drafts/a.md").is_file());
}
