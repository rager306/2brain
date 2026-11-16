//! The `preview_mutation` read over the socket: simulate a deterministic
//! mutation over an overlay of the current snapshot and report its product, the
//! would-be file's type identities and diagnostics plus the per-file blast
//! radius, without writing disk or committing.

#![cfg(unix)]

use std::fs;
use std::path::PathBuf;

use serde_json::{Value, json};
use tb_engine::{Client, ConfigSource, Engine, ServeHandle, serve};

/// A knowledge base with:
/// - `task`: requires `title: String`, and a `body:` template with a required
///   `Details` section, so a preview exercises WHOLE-file body typing.
/// - `ref`: a `target: task*` reference field, so a delete of a referent dangles
///   its referrer.
/// - `hub.md`: a valid `task`. `referrer.md`: a `ref` pointing at `hub`.
fn fixture() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap();
    crate::seed_repo(&root);
    fs::create_dir(root.join("type")).unwrap();
    fs::write(
        root.join("type/task.type.yaml"),
        "fields:\n  title: String\nbody:\n  - section: Details\n",
    )
    .unwrap();
    fs::write(
        root.join("type/ref.type.yaml"),
        "fields:\n  target: task*\n",
    )
    .unwrap();
    fs::write(
        root.join("hub.md"),
        "---\ntype: task\ntitle: Hub\n---\n# Details\n\nbody.\n",
    )
    .unwrap();
    fs::write(
        root.join("referrer.md"),
        "---\ntype: ref\ntarget: \"[[hub]]\"\n---\n",
    )
    .unwrap();
    (dir, root)
}

struct Harness {
    _dir: tempfile::TempDir,
    _sock_dir: tempfile::TempDir,
    _engine: Engine,
    _server: ServeHandle,
    client: Client,
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
    }
}

/// The `preview_mutation` payload for a request.
fn preview(h: &mut Harness, req: Value) -> Value {
    let mut req = req;
    req["read"] = json!("preview_mutation");
    let resp = h.client.query(&req).unwrap();
    assert_eq!(resp["ready"], true, "{resp:?}");
    resp["result"]["preview_mutation"].clone()
}

fn codes(diags: &Value) -> Vec<String> {
    let mut out: Vec<String> = diags
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["code"].as_str().unwrap().to_string())
        .collect();
    out.sort();
    out
}

fn identity_names(target: &Value) -> Vec<String> {
    let mut out: Vec<String> = target["identities"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["name"].as_str().unwrap().to_string())
        .collect();
    out.sort();
    out
}

/// A two-member workspace: `base` owns the schema, `app` peers `base` and holds
/// an instance claiming `widget::base`. One enclosing git tree covers both, which
/// a structural refactor requires. Returns the root too, so a test can assert the
/// files on disk.
fn crossed() -> (Harness, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap();
    crate::seed_workspace(&root, &["base", "app"]);
    let write = |rel: &str, content: &str| {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    };
    write(
        "base/.arsumbris/repo.yaml",
        "name: base\ntype: au.engine.repo::au-engine\n",
    );
    crate::seed_readme(&root.join("base"));
    write(
        "app/.arsumbris/repo.yaml",
        "name: app\ntype: au.engine.repo::au-engine\ndeps:\n  - name: base\n",
    );
    crate::seed_readme(&root.join("app"));
    write("base/type/widget.type.yaml", "fields:\n  label: String\n");
    // A type nothing references, so a rename of it touches the def file alone.
    write("base/type/lonely.type.yaml", "fields:\n  title: String\n");
    write("app/holder.md", "---\ntype: widget::base\nlabel: x\n---\n");

    for args in [
        ["init", "-q", "-b", "main"].as_slice(),
        ["config", "user.name", "Tester"].as_slice(),
        ["config", "user.email", "tester@example.com"].as_slice(),
        ["add", "."].as_slice(),
        ["commit", "-q", "-m", "seed"].as_slice(),
    ] {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    let sock_dir = tempfile::tempdir().unwrap();
    let socket = sock_dir.path().join("s");
    let engine = Engine::new(&root, ConfigSource::Empty);
    let server = serve(engine.handle(), &socket).expect("serve");
    engine.rebuild();
    let client = Client::connect(&socket).expect("connect");
    let harness = Harness {
        _dir: dir,
        _sock_dir: sock_dir,
        _engine: engine,
        _server: server,
        client,
    };
    (harness, root)
}

#[test]
fn a_rename_type_preview_names_the_referrer_member_and_its_kind() {
    let (mut h, root) = crossed();
    let def_before = fs::read(root.join("base/type/widget.type.yaml")).unwrap();
    let holder_before = fs::read(root.join("app/holder.md")).unwrap();

    let p = preview(
        &mut h,
        json!({"op": "rename_type", "old_name": "widget::base", "new_name": "gadget"}),
    );
    assert!(
        p.get("reject").is_none(),
        "an allowed rename previews a product: {p:?}"
    );
    assert!(
        p["target"]["path"]
            .as_str()
            .is_some_and(|path| path.ends_with("base/type/gadget.type.yaml")),
        "the target is the def at its new path: {p:?}"
    );
    let affected = p["affected"].as_array().expect("affected list");
    let holder = affected
        .iter()
        .find(|entry| {
            entry["path"]
                .as_str()
                .is_some_and(|p| p.ends_with("app/holder.md"))
        })
        .unwrap_or_else(|| panic!("the cross-member referrer is missing: {affected:?}"));
    assert_eq!(holder["repo"], "app", "{holder:?}");
    assert_eq!(holder["changes"], json!(["claim"]), "{holder:?}");
    let def = affected
        .iter()
        .find(|entry| {
            entry["path"]
                .as_str()
                .is_some_and(|p| p.ends_with("widget.type.yaml"))
        })
        .expect("the def's own move");
    assert_eq!(def["repo"], "base", "{def:?}");
    assert_eq!(def["changes"], json!(["definition_move"]), "{def:?}");

    // A preview writes nothing and moves nothing.
    assert_eq!(
        fs::read(root.join("base/type/widget.type.yaml")).unwrap(),
        def_before,
        "the def stays where it is"
    );
    assert!(!root.join("base/type/gadget.type.yaml").exists());
    assert_eq!(
        fs::read(root.join("app/holder.md")).unwrap(),
        holder_before,
        "the referrer is untouched"
    );
}

#[test]
fn a_rename_type_preview_of_an_unreferenced_type_rewrites_nothing() {
    // A git-covered workspace, which a structural refactor requires; `lonely` is
    // the type nothing claims, slots or links to.
    let (mut h, root) = crossed();
    let before = h.client.query(&json!({ "read": "lifecycle" })).unwrap()["version"].clone();
    let p = preview(
        &mut h,
        json!({"op": "rename_type", "old_name": "lonely::base", "new_name": "solitary"}),
    );
    assert!(p.get("reject").is_none(), "{p:?}");
    let affected = p["affected"].as_array().expect("affected list");
    assert_eq!(affected.len(), 1, "only the def file is affected: {p:?}");
    assert!(
        affected[0]["path"]
            .as_str()
            .is_some_and(|path| path.ends_with("base/type/lonely.type.yaml")),
        "{p:?}"
    );
    assert_eq!(affected[0]["repo"], "base", "{p:?}");
    assert_eq!(
        affected[0]["changes"],
        json!(["definition_move"]),
        "no referrer means no claim or link change: {p:?}"
    );
    assert!(
        p["blast_radius"].as_array().unwrap().is_empty(),
        "nothing else's diagnostics change: {p:?}"
    );
    assert!(!root.join("base/type/solitary.type.yaml").exists());
    assert!(root.join("base/type/lonely.type.yaml").exists());
    let after = h.client.query(&json!({ "read": "lifecycle" })).unwrap()["version"].clone();
    assert_eq!(before, after, "a preview advances no version");
}

#[test]
fn a_valid_write_reports_its_identity_and_no_diagnostics() {
    let mut h = started();
    let p = preview(
        &mut h,
        json!({
            "op": "write_file",
            "path": "new.md",
            "content": "---\ntype: task\ntitle: New\n---\n# Details\n\nbody.\n",
        }),
    );
    let target = &p["target"];
    assert_eq!(identity_names(target), vec!["task"], "{p:?}");
    assert!(
        target["diagnostics"].as_array().unwrap().is_empty(),
        "a valid task previews clean: {p:?}"
    );
    assert!(target["hash"].is_string(), "the would-be hash is present");
    assert!(p["blast_radius"].as_array().unwrap().is_empty(), "{p:?}");
}

#[test]
fn a_preview_writes_no_disk_and_advances_no_version() {
    let mut h = started();
    let before = h.client.query(&json!({ "read": "lifecycle" })).unwrap()["version"].clone();
    preview(
        &mut h,
        json!({
            "op": "write_file",
            "path": "ghost.md",
            "content": "---\ntype: task\ntitle: Ghost\n---\n# Details\n\nx.\n",
        }),
    );
    // The file never landed: a content read finds nothing.
    let content = h
        .client
        .query(&json!({ "read": "content", "path": "ghost.md" }))
        .unwrap();
    assert!(
        content["result"]["content"].is_null(),
        "preview must not write disk: {content:?}"
    );
    // The version did not advance: a preview is a read, not a mutation.
    let after = h.client.query(&json!({ "read": "lifecycle" })).unwrap()["version"].clone();
    assert_eq!(before, after, "a preview advances no version");
}

#[test]
fn a_claim_over_junk_resolves_the_identity_and_reports_the_failures() {
    let mut h = started();
    // Claims `task` but omits the required title AND the required Details section:
    // the identity still resolves, and WHOLE-file validation reports both.
    let p = preview(
        &mut h,
        json!({
            "op": "write_file",
            "path": "junk.md",
            "content": "---\ntype: task\n---\nnot the details section\n",
        }),
    );
    let target = &p["target"];
    assert_eq!(
        identity_names(target),
        vec!["task"],
        "the claim resolves even over junk: {p:?}"
    );
    let cs = codes(&target["diagnostics"]);
    assert!(
        cs.contains(&"required-field-absent".to_string()),
        "missing title: {p:?}"
    );
    assert!(
        cs.contains(&"body-section-missing".to_string()),
        "body typing runs, so the missing Details section is reported: {p:?}"
    );
}

#[test]
fn a_write_with_no_type_claim_has_no_identities() {
    let mut h = started();
    let p = preview(
        &mut h,
        json!({
            "op": "write_file",
            "path": "note.md",
            "content": "# just a note\n",
        }),
    );
    assert!(
        p["target"]["identities"].as_array().unwrap().is_empty(),
        "a plain note claims no type: {p:?}"
    );
}

#[test]
fn an_edit_of_an_absent_file_rejects() {
    let mut h = started();
    let p = preview(
        &mut h,
        json!({
            "op": "edit_file",
            "path": "absent.md",
            "old_string": "x",
            "new_string": "y",
        }),
    );
    assert!(
        p["reject"]["message"]
            .as_str()
            .unwrap()
            .contains("nothing to edit"),
        "an edit of an absent file is a reject: {p:?}"
    );
}

#[test]
fn an_edit_previews_the_resulting_file() {
    let mut h = started();
    // Edit hub's title; the product is computed from the current file plus the
    // fragment, no caller reconstruction. It stays a valid task.
    let p = preview(
        &mut h,
        json!({
            "op": "edit_file",
            "path": "hub.md",
            "old_string": "title: Hub",
            "new_string": "title: Renamed",
        }),
    );
    let target = &p["target"];
    assert_eq!(identity_names(target), vec!["task"], "{p:?}");
    assert!(
        target["diagnostics"].as_array().unwrap().is_empty(),
        "the edited task is still valid: {p:?}"
    );
}

#[test]
fn a_delete_dangles_its_referrer_in_the_blast_radius() {
    let mut h = started();
    // Deleting hub.md leaves referrer.md's `target: [[hub]]` dangling.
    let p = preview(&mut h, json!({ "op": "delete_file", "path": "hub.md" }));
    // The target of a delete carries no identity, the file is gone.
    assert!(
        p["target"]["identities"].as_array().unwrap().is_empty(),
        "a delete's target has no identity: {p:?}"
    );
    // The referrer surfaces in the blast radius with the dangling-reference code.
    let blast = p["blast_radius"].as_array().unwrap();
    let referrer = blast
        .iter()
        .find(|e| e["path"].as_str().unwrap().ends_with("referrer.md"))
        .unwrap_or_else(|| panic!("referrer should be in the blast radius: {p:?}"));
    assert!(
        codes(&referrer["diagnostics"]).contains(&"reference-target-missing".to_string()),
        "the delete dangles the referrer: {p:?}"
    );
}

#[test]
fn a_type_def_edit_surfaces_a_broken_dependent_in_the_blast_radius() {
    let mut h = started();
    // Add a new required field to `task`: the existing hub.md (a task) now misses
    // it. A type-def edit goes through the whole-knowledge-base build path.
    let p = preview(
        &mut h,
        json!({
            "op": "write_file",
            "path": "type/task.type.yaml",
            "content": "fields:\n  title: String\n  owner: String\nbody:\n  - section: Details\n",
        }),
    );
    let blast = p["blast_radius"].as_array().unwrap();
    let hub = blast
        .iter()
        .find(|e| e["path"].as_str().unwrap().ends_with("hub.md"))
        .unwrap_or_else(|| panic!("hub should be broken by the new required field: {p:?}"));
    assert!(
        codes(&hub["diagnostics"]).contains(&"required-field-absent".to_string()),
        "the new required field breaks the existing task: {p:?}"
    );
}

/// The target verdict equals the `diagnostics` read of the same file after the
/// real write commits: a preview cannot disagree with what the write lands.
#[test]
fn the_preview_verdict_matches_the_post_write_verdict() {
    let mut h = started();
    let content = "---\ntype: task\n---\nno details here\n";

    let previewed = preview(
        &mut h,
        json!({ "op": "write_file", "path": "parity.md", "content": content }),
    );
    let preview_codes = codes(&previewed["target"]["diagnostics"]);

    // Now really write it, then read the file's diagnostics.
    let w = h
        .client
        .query(&json!({ "mutate": "write_file", "path": "parity.md", "content": content }))
        .unwrap();
    assert_eq!(w["type"], "response", "{w:?}");
    let real = h
        .client
        .query(&json!({ "read": "diagnostics", "path": "parity.md" }))
        .unwrap();
    let real_codes = codes(&real["result"]["diagnostics"]);

    assert!(
        !real_codes.is_empty(),
        "the file itself is missing a required field and section: {real:?}"
    );
    assert_eq!(
        preview_codes, real_codes,
        "the preview verdict must equal the post-write verdict"
    );
}

/// A repo whose `.auignore` excludes `docs/`, so files there are absent from the
/// graph with zero diagnostics. The regression fixture for the preview scope-leak.
fn scoped_started() -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap();
    crate::seed_repo(&root);
    fs::write(root.join(".arsumbris/.auignore"), "docs/\n").unwrap();
    fs::create_dir(root.join("docs")).unwrap();

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
    }
}

/// A preview of a write to an auignored (out-of-scope) target must carry zero
/// diagnostics, exactly as the committed write does. Before the scope filter was
/// applied in the preview path, the recompute parsed the would-be file and leaked
/// a `yaml-parse-error` the walk suppresses.
#[test]
fn a_preview_of_an_auignored_target_leaks_no_diagnostics() {
    let mut h = scoped_started();
    // Invalid YAML frontmatter: an unquoted `: ` colon-space, the exact shape that
    // trips `yaml-parse-error`. Under `docs/`, so the walk excludes it.
    let content = "---\ntldr: text: colon\n---\nbody\n";

    let previewed = preview(
        &mut h,
        json!({ "op": "write_file", "path": "docs/leak.md", "content": content }),
    );
    let target = &previewed["target"];
    assert!(
        target["diagnostics"].as_array().unwrap().is_empty(),
        "an auignored target previews clean, no scope leak: {previewed:?}"
    );
    assert!(
        target["identities"].as_array().unwrap().is_empty(),
        "an out-of-scope file claims no graph identity: {previewed:?}"
    );
    assert!(
        previewed["blast_radius"].as_array().unwrap().is_empty(),
        "an out-of-scope file has no blast radius: {previewed:?}"
    );

    // Parity: really write it, then read the file's diagnostics. The committed
    // write lands zero, so the preview must too.
    let w = h
        .client
        .query(&json!({ "mutate": "write_file", "path": "docs/leak.md", "content": content }))
        .unwrap();
    assert_eq!(w["type"], "response", "{w:?}");
    let real = h
        .client
        .query(&json!({ "read": "diagnostics", "path": "docs/leak.md" }))
        .unwrap();
    let real_codes = codes(&real["result"]["diagnostics"]);
    assert!(
        real_codes.is_empty(),
        "the committed write lands zero diagnostics for an auignored file: {real:?}"
    );
}

// ----- the rename preview: the would-change referrer set of a move or rename -----

/// The base fixture plus extra files, started.
/// The fixture plus `extra` files, committed in a git working tree: a rename
/// needs one, so its preview does too.
fn started_with(extra: &[(&str, &str)]) -> Harness {
    let (dir, root) = fixture();
    for (rel, content) in extra {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, content).unwrap();
    }
    git(&root, &["init", "-q", "-b", "main"]);
    git(&root, &["config", "user.name", "Tester"]);
    git(&root, &["config", "user.email", "tester@example.com"]);
    git(&root, &["add", "."]);
    git(&root, &["commit", "-q", "-m", "seed"]);
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
    }
}

/// The rewrites as `(path-suffix, [(from, to)])`, path relative to the root's
/// last component so assertions stay short.
fn rewrites(p: &Value) -> Vec<(String, Vec<(String, String)>)> {
    p["rewrites"]
        .as_array()
        .unwrap_or_else(|| panic!("a rename preview carries rewrites: {p:?}"))
        .iter()
        .map(|r| {
            let links = r["links"]
                .as_array()
                .unwrap()
                .iter()
                .map(|l| {
                    (
                        l["from"].as_str().unwrap().to_string(),
                        l["to"].as_str().unwrap().to_string(),
                    )
                })
                .collect();
            (r["path"].as_str().unwrap().to_string(), links)
        })
        .collect()
}

#[test]
fn a_pure_move_previews_no_rewrite_for_a_bare_name_referrer() {
    let mut h = started_with(&[]);
    let p = preview(
        &mut h,
        json!({ "op": "rename", "path": "hub.md", "to": "sub/hub.md" }),
    );
    // referrer.md spells `[[hub]]`, which a name-keeping move leaves identical.
    assert!(rewrites(&p).is_empty(), "{p:?}");
    // The verdict reads the moved file at its destination.
    assert!(
        p["target"]["path"]
            .as_str()
            .unwrap()
            .ends_with("sub/hub.md")
    );
    assert_eq!(identity_names(&p["target"]), vec!["task".to_string()]);
    // Nothing breaks, and the source is not a blast entry.
    assert!(
        p["blast_radius"].as_array().unwrap().is_empty(),
        "a clean move has no blast radius: {p:?}"
    );
}

#[test]
fn a_rename_previews_the_re_spelled_links_and_writes_nothing() {
    let mut h = started_with(&[]);
    let p = preview(
        &mut h,
        json!({ "op": "rename", "path": "hub.md", "to": "hub2.md" }),
    );
    let rw = rewrites(&p);
    assert_eq!(rw.len(), 1, "{p:?}");
    assert!(rw[0].0.ends_with("referrer.md"), "{rw:?}");
    assert_eq!(
        rw[0].1,
        vec![("[[hub]]".to_string(), "[[hub2]]".to_string())]
    );
    // The rewritten referrer still resolves, so it is no blast entry.
    assert!(p["blast_radius"].as_array().unwrap().is_empty(), "{p:?}");
    // A preview writes nothing.
    let status = h
        .client
        .query(&json!({ "read": "content", "path": "referrer.md" }))
        .unwrap();
    assert!(status.to_string().contains("[[hub]]"), "{status:?}");
}

#[test]
fn a_move_previews_a_path_addressed_referrers_re_spelling() {
    let mut h = started_with(&[
        (
            "x/leaf.md",
            "---\ntype: task\ntitle: Leaf\n---\n# Details\n\nbody.\n",
        ),
        ("pathref.md", "see [[x/leaf]] and [[leaf]]\n"),
    ]);
    let p = preview(
        &mut h,
        json!({ "op": "rename", "path": "x/leaf.md", "to": "y/leaf.md" }),
    );
    let rw = rewrites(&p);
    assert_eq!(rw.len(), 1, "{p:?}");
    assert!(rw[0].0.ends_with("pathref.md"), "{rw:?}");
    // Only the path-addressed link changes, the bare one is untouched.
    assert_eq!(
        rw[0].1,
        vec![("[[x/leaf]]".to_string(), "[[y/leaf]]".to_string())]
    );
}

#[test]
fn a_rename_into_a_taken_name_surfaces_the_now_ambiguous_referrer() {
    // `other/hub2.md` already exists, so renaming `hub.md` to `hub2.md` re-spells
    // referrer.md's `[[hub]]` as `[[hub2]]`, which now names two files. The
    // rewrite list alone cannot show that; the blast radius does.
    let mut h = started_with(&[(
        "other/hub2.md",
        "---\ntype: task\ntitle: Other\n---\n# Details\n\nbody.\n",
    )]);
    let p = preview(
        &mut h,
        json!({ "op": "rename", "path": "hub.md", "to": "hub2.md" }),
    );
    let blast = p["blast_radius"].as_array().unwrap();
    let referrer = blast
        .iter()
        .find(|e| e["path"].as_str().unwrap().ends_with("referrer.md"))
        .unwrap_or_else(|| panic!("the referrer turns ambiguous: {p:?}"));
    assert!(
        codes(&referrer["diagnostics"]).contains(&"reference-target-ambiguous".to_string()),
        "{p:?}"
    );
}

#[test]
fn a_rename_onto_an_existing_file_rejects() {
    let mut h = started_with(&[]);
    let p = preview(
        &mut h,
        json!({ "op": "rename", "path": "hub.md", "to": "referrer.md" }),
    );
    let msg = p["reject"]["message"].as_str().unwrap_or_default();
    assert!(msg.contains("destination already exists"), "{p:?}");
}

#[test]
fn a_rename_of_a_type_def_rejects() {
    let mut h = started_with(&[]);
    let p = preview(
        &mut h,
        json!({ "op": "rename", "path": "type/task.type.yaml", "to": "type/job.type.yaml" }),
    );
    let msg = p["reject"]["message"].as_str().unwrap_or_default();
    assert!(msg.contains("use rename_type"), "{p:?}");
}

#[test]
fn a_rename_preview_off_git_rejects_as_the_rename_would() {
    // A rename rolls back through git, so it refuses a repo no working tree
    // covers; its preview refuses alike rather than report a product.
    let mut h = started();
    let p = preview(
        &mut h,
        json!({ "op": "rename", "path": "hub.md", "to": "hub2.md" }),
    );
    let msg = p["reject"]["message"].as_str().unwrap_or_default();
    assert!(msg.contains("no git working tree covers"), "{p:?}");
}

fn git(repo: &std::path::Path, args: &[&str]) {
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

/// The fixture under git with `scratch.md` ignored (the rule committed), then
/// an ignored, never-tracked `scratch.md` written before the engine starts.
fn started_in_git_with_an_ignored_note() -> Harness {
    let (dir, root) = fixture();
    fs::write(root.join(".gitignore"), "scratch.md\n").unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    git(&root, &["config", "user.name", "Tester"]);
    git(&root, &["config", "user.email", "tester@example.com"]);
    git(&root, &["add", "."]);
    git(&root, &["commit", "-q", "-m", "seed"]);
    fs::write(
        root.join("scratch.md"),
        "---\ntype: task\ntitle: Scratch\n---\n# Details\n\nignored.\n",
    )
    .unwrap();
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
    }
}

fn untracked(p: &Value) -> Vec<String> {
    p["untracked_files"]
        .as_array()
        .unwrap_or_else(|| panic!("every product carries untracked_files: {p:?}"))
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect()
}

#[test]
fn an_edit_of_a_git_ignored_file_previews_it_as_untracked() {
    let mut h = started_in_git_with_an_ignored_note();
    let p = preview(
        &mut h,
        json!({ "op": "edit_file", "path": "scratch.md",
                "old_string": "ignored.", "new_string": "edited." }),
    );
    let listed = untracked(&p);
    assert_eq!(listed.len(), 1, "{listed:?}");
    assert!(listed[0].ends_with("scratch.md"), "{listed:?}");
}

#[test]
fn a_write_of_a_tracked_file_previews_no_untracked_path() {
    let mut h = started_in_git_with_an_ignored_note();
    let p = preview(
        &mut h,
        json!({ "op": "write_file", "path": "hub.md",
                "content": "---\ntype: task\ntitle: Hub\n---\n# Details\n\nnew.\n" }),
    );
    assert!(
        untracked(&p).is_empty(),
        "a tracked write lists nothing: {p:?}"
    );
}

#[test]
fn a_delete_of_a_git_ignored_file_previews_it_as_untracked() {
    let mut h = started_in_git_with_an_ignored_note();
    let p = preview(&mut h, json!({ "op": "delete_file", "path": "scratch.md" }));
    let listed = untracked(&p);
    assert!(
        listed.len() == 1 && listed[0].ends_with("scratch.md"),
        "{listed:?}"
    );
}

#[test]
fn a_rename_of_a_git_ignored_file_previews_what_the_rename_reports() {
    let mut h = started_in_git_with_an_ignored_note();
    let p = preview(
        &mut h,
        json!({ "op": "rename", "path": "scratch.md", "to": "moved/scratch.md" }),
    );
    let previewed = untracked(&p);
    assert_eq!(previewed.len(), 2, "both sides of the move: {previewed:?}");
    assert!(previewed.iter().any(|s| s.ends_with("moved/scratch.md")));

    let resp = h
        .client
        .query(&json!({ "mutate": "rename", "path": "scratch.md", "to": "moved/scratch.md" }))
        .unwrap();
    assert_eq!(resp["type"], "response", "{resp:?}");
    let committed: Vec<String> = resp["result"]["untracked_files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        previewed, committed,
        "the preview lists exactly what the committed rename reports"
    );
}

// --- the ensure_mixins rider ---

/// The git fixture plus `marker` (a no-field tag) and `tagged` (a required
/// `tag`), the mixins the write-path tests use.
fn started_with_mixin_types() -> Harness {
    started_with(&[
        ("type/marker.type.yaml", "fields: {}\n"),
        ("type/tagged.type.yaml", "fields:\n  tag: String\n"),
    ])
}

fn mixin_outcomes(p: &Value) -> Vec<(String, String)> {
    p["ensure_mixins"]
        .as_array()
        .unwrap_or_else(|| panic!("a mixin-carrying preview reports ensure_mixins: {p:?}"))
        .iter()
        .map(|o| {
            (
                o["mixin"].as_str().unwrap().to_string(),
                o["outcome"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

/// The previewed product is the product the write lands: the same outcomes, the
/// same bytes (hash), the mixin in the identity.
#[test]
fn a_write_preview_folds_its_mixin_as_the_write_does() {
    let mut h = started_with_mixin_types();
    let req = json!({
        "path": "n.md",
        "content": "---\ntype: task\ntitle: N\n---\n# Details\n\nx.\n",
        "ensure_mixins": ["marker"]
    });
    let mut p_req = req.clone();
    p_req["op"] = json!("write_file");
    let p = preview(&mut h, p_req);
    assert_eq!(
        mixin_outcomes(&p),
        vec![("marker".to_string(), "applied".to_string())]
    );
    assert_eq!(identity_names(&p["target"]), vec!["marker", "task"]);

    let mut w_req = req;
    w_req["mutate"] = json!("write_file");
    let w = h.client.query(&w_req).unwrap();
    assert_eq!(w["type"], "response", "{w:?}");
    assert_eq!(p["target"]["hash"], w["result"]["hash"], "same bytes land");
    assert_eq!(p["ensure_mixins"], w["result"]["ensure_mixins"]);
}

#[test]
fn a_strict_unappliable_mixin_rejects_the_preview_as_it_rejects_the_edit() {
    let mut h = started_with_mixin_types();
    let p = preview(
        &mut h,
        json!({ "op": "edit_file", "path": "hub.md",
                "old_string": "body.", "new_string": "edited.",
                "ensure_mixins": ["tagged"] }),
    );
    let previewed = p["reject"]["message"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        previewed.contains("ensure_mixin 'tagged' cannot be applied"),
        "{p:?}"
    );

    let w = h
        .client
        .query(&json!({ "mutate": "edit_file", "path": "hub.md",
                        "old_string": "body.", "new_string": "edited.",
                        "ensure_mixins": ["tagged"] }))
        .unwrap();
    assert_eq!(w["type"], "error", "{w:?}");
    assert_eq!(
        w["error"], previewed,
        "the preview names the write's reject"
    );
}

#[test]
fn a_lenient_unappliable_mixin_is_a_reported_skip() {
    let mut h = started_with_mixin_types();
    let p = preview(
        &mut h,
        json!({ "op": "edit_file", "path": "hub.md",
                "old_string": "body.", "new_string": "edited.",
                "ensure_mixins": ["tagged"], "ensure_mixins_strict": false }),
    );
    assert_eq!(
        mixin_outcomes(&p),
        vec![("tagged".to_string(), "skipped".to_string())]
    );
    assert!(p["ensure_mixins"][0]["reason"].is_string(), "{p:?}");
    assert_eq!(identity_names(&p["target"]), vec!["task"]);
}

#[test]
fn a_rename_preview_folds_its_mixin_onto_the_destination() {
    let mut h = started_with_mixin_types();
    let p = preview(
        &mut h,
        json!({ "op": "rename", "path": "hub.md", "to": "moved/hub.md",
                "ensure_mixins": ["marker"] }),
    );
    assert_eq!(
        mixin_outcomes(&p),
        vec![("marker".to_string(), "applied".to_string())]
    );
    assert!(
        p["target"]["path"]
            .as_str()
            .unwrap()
            .ends_with("moved/hub.md"),
        "{p:?}"
    );
    assert_eq!(identity_names(&p["target"]), vec!["marker", "task"]);

    let w = h
        .client
        .query(
            &json!({ "mutate": "rename", "path": "hub.md", "to": "moved/hub.md",
                        "ensure_mixins": ["marker"] }),
        )
        .unwrap();
    assert_eq!(w["type"], "response", "{w:?}");
    assert_eq!(p["target"]["hash"], w["result"]["hash"], "same bytes land");
}

#[test]
fn a_preview_without_mixins_carries_no_ensure_mixins() {
    let mut h = started_with_mixin_types();
    let p = preview(
        &mut h,
        json!({ "op": "write_file", "path": "n.md",
                "content": "---\ntype: task\ntitle: N\n---\n# Details\n\nx.\n" }),
    );
    assert!(p.get("ensure_mixins").is_none(), "{p:?}");
}

/// A rider on a type-def target is refused by the write before anything else,
/// so its preview refuses it too, stamps and mixins alike.
#[test]
fn a_rider_on_a_type_def_target_rejects_the_preview() {
    let mut h = started_with_mixin_types();
    for riders in [
        json!({ "ensure_mixins": ["marker"] }),
        json!({ "stamps": [{ "field": "log", "record": { "type": "marker" } }] }),
    ] {
        let mut req = json!({ "op": "write_file", "path": "type/extra.type.yaml",
                              "content": "fields: {}\n" });
        for (k, v) in riders.as_object().unwrap() {
            req[k] = v.clone();
        }
        let p = preview(&mut h, req);
        let msg = p["reject"]["message"].as_str().unwrap_or_default();
        assert!(msg.contains("not a type-def"), "{riders}: {p:?}");
    }
}
