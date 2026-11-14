//! The workspace composition verbs over the socket: `add_workspace_member`,
//! `remove_workspace_member`, `set_workspace_member_role`,
//! `set_workspace_member_disabled`, authoring
//! `<member>/.arsumbris/workspace.yaml`. A guard-bypass like `set_config`, but the
//! file is a first-class node, so the change re-resolves and rebuilds; an absent
//! file is created self-complete (the containing repo seeded into `edit:`).
//!
//! See [[spec - composition authorship - governed list verbs author workspace members and repo deps, rebuilding on change]].

#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};

use au_engine::{Client, ConfigSource, Engine, ServeHandle, serve};
use serde_json::{Value, json};

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
    crate::seed_repo(&root); // declares `name: v`, no workspace.yaml
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

fn ws_yaml(h: &Harness) -> String {
    fs::read_to_string(h.root.join(".arsumbris").join("workspace.yaml")).unwrap()
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
fn add_member_creates_a_self_complete_workspace_and_switches_mode() {
    let mut h = started();
    let resp = h
        .client
        .query(&json!({ "mutate": "add_workspace_member", "name": "notes", "role": "edit" }))
        .unwrap();
    assert_eq!(resp["ready"], true, "{resp}");
    assert!(
        resp["result"]["commit"].is_string(),
        "a commit landed: {resp}"
    );

    // Created self-describing and self-complete: the containing repo `v` is in
    // edit:, the new member appended, plus the type: claim (no drift).
    assert_eq!(
        ws_yaml(&h),
        "type: au.engine.workspace::au-engine\nedit:\n  - v\n  - notes\n"
    );

    // The engine switched into workspace mode on the rebuild: `notes` is now a
    // declared edit member that does not resolve, so it surfaces unmounted.
    let diags = h.client.query(&json!({ "read": "diagnostics" })).unwrap();
    assert!(
        codes(&diags).iter().any(|c| c == "edit-member-unmounted"),
        "the created workspace mounts, the absent member surfaces: {diags}"
    );
}

#[test]
fn add_discover_member_seeds_containing_in_edit() {
    let mut h = started();
    h.client
        .query(&json!({ "mutate": "add_workspace_member", "name": "shared", "role": "discover" }))
        .unwrap();
    assert_eq!(
        ws_yaml(&h),
        "type: au.engine.workspace::au-engine\nedit:\n  - v\ndiscover:\n  - shared\n",
        "the containing repo is seeded into edit, the member into discover"
    );
}

#[test]
fn add_member_appends_to_the_existing_list() {
    let mut h = started();
    h.client
        .query(&json!({ "mutate": "add_workspace_member", "name": "a", "role": "edit" }))
        .unwrap();
    h.client
        .query(&json!({ "mutate": "add_workspace_member", "name": "b", "role": "edit" }))
        .unwrap();
    assert_eq!(
        ws_yaml(&h),
        "type: au.engine.workspace::au-engine\nedit:\n  - v\n  - a\n  - b\n"
    );
}

#[test]
fn add_member_rejects_a_duplicate_in_the_same_list() {
    let mut h = started();
    h.client
        .query(&json!({ "mutate": "add_workspace_member", "name": "a", "role": "edit" }))
        .unwrap();
    let dup = h
        .client
        .query(&json!({ "mutate": "add_workspace_member", "name": "a", "role": "edit" }))
        .unwrap();
    assert_eq!(dup["type"], "error", "a same-list duplicate rejects: {dup}");
}

#[test]
fn add_member_rejects_a_cross_role_duplicate() {
    let mut h = started();
    h.client
        .query(&json!({ "mutate": "add_workspace_member", "name": "a", "role": "edit" }))
        .unwrap();
    // Already an edit member; adding it to discover must reject (use re-role).
    let cross = h
        .client
        .query(&json!({ "mutate": "add_workspace_member", "name": "a", "role": "discover" }))
        .unwrap();
    assert_eq!(cross["type"], "error", "a cross-role add rejects: {cross}");
}

#[test]
fn add_member_rejects_the_containing_repo_as_discover() {
    let mut h = started();
    let resp = h
        .client
        .query(&json!({ "mutate": "add_workspace_member", "name": "v", "role": "discover" }))
        .unwrap();
    assert_eq!(
        resp["type"], "error",
        "the containing repo cannot be a discover member: {resp}"
    );
}

#[test]
fn remove_member_drops_it_from_its_list() {
    let mut h = started();
    h.client
        .query(&json!({ "mutate": "add_workspace_member", "name": "a", "role": "edit" }))
        .unwrap();
    let resp = h
        .client
        .query(&json!({ "mutate": "remove_workspace_member", "name": "a" }))
        .unwrap();
    assert_eq!(resp["ready"], true, "{resp}");
    assert_eq!(
        ws_yaml(&h),
        "type: au.engine.workspace::au-engine\nedit:\n  - v\n",
        "the member is removed, the containing repo stays"
    );
}

#[test]
fn remove_member_rejects_an_absent_name() {
    let mut h = started();
    // Seed a workspace.yaml first so the file exists.
    h.client
        .query(&json!({ "mutate": "add_workspace_member", "name": "a", "role": "edit" }))
        .unwrap();
    let resp = h
        .client
        .query(&json!({ "mutate": "remove_workspace_member", "name": "ghost" }))
        .unwrap();
    assert_eq!(resp["type"], "error", "an absent member rejects: {resp}");
}

/// Author `workspace.yaml` by hand and commit it, the state a verb then edits. The
/// saga refuses a dirty target, so the seed is committed; the engine rebuilds so
/// the held view matches.
fn seed_ws_yaml(h: &Harness, content: &str) {
    fs::write(h.root.join(".arsumbris").join("workspace.yaml"), content).unwrap();
    git(&h.root, &["add", "-A"]);
    git(&h.root, &["commit", "-q", "-m", "seed workspace.yaml"]);
    h._engine.rebuild();
}

fn head(root: &Path) -> String {
    let out = std::process::Command::new("git")
        .current_dir(root)
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn remove_member_refuses_the_containing_repo() {
    let mut h = started();
    h.client
        .query(&json!({ "mutate": "add_workspace_member", "name": "a", "role": "edit" }))
        .unwrap();
    let before_yaml = ws_yaml(&h);
    let before_head = head(&h.root);
    let resp = h
        .client
        .query(&json!({ "mutate": "remove_workspace_member", "name": "v" }))
        .unwrap();
    assert_eq!(
        resp["type"], "error",
        "the containing repo cannot be removed: {resp}"
    );
    assert_eq!(ws_yaml(&h), before_yaml, "nothing written");
    assert_eq!(head(&h.root), before_head, "no commit");
}

#[test]
fn set_role_refuses_the_containing_repo_into_discover() {
    let mut h = started();
    h.client
        .query(&json!({ "mutate": "add_workspace_member", "name": "a", "role": "edit" }))
        .unwrap();
    let resp = h
        .client
        .query(&json!({ "mutate": "set_workspace_member_role", "name": "v", "role": "discover" }))
        .unwrap();
    assert_eq!(
        resp["type"], "error",
        "the containing repo stays in edit: {resp}"
    );
}

#[test]
fn remove_member_also_drops_it_from_the_disabled_overlay() {
    let mut h = started();
    seed_ws_yaml(
        &h,
        "type: au.engine.workspace::au-engine\nedit:\n  - v\n  - a\ndisabled:\n  - a\n",
    );
    let resp = h
        .client
        .query(&json!({ "mutate": "remove_workspace_member", "name": "a" }))
        .unwrap();
    assert_eq!(resp["ready"], true, "{resp}");
    assert_eq!(
        ws_yaml(&h),
        "type: au.engine.workspace::au-engine\nedit:\n  - v\ndisabled: []\n",
        "the member left edit and disabled in one splice"
    );
    let diags = h.client.query(&json!({ "read": "diagnostics" })).unwrap();
    assert!(
        diags["result"]["diagnostics"].is_array(),
        "the diagnostics read answered: {diags}"
    );
    assert!(
        !codes(&diags)
            .iter()
            .any(|c| c == "disabled-member-not-declared"),
        "no stranded disabled entry: {diags}"
    );
}

fn member_disabled(h: &mut Harness, name: &str) -> bool {
    let resp = h
        .client
        .query(&json!({ "read": "members", "repo": name }))
        .unwrap();
    let members = resp["result"]["members"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let m = members
        .iter()
        .find(|m| m["repo"] == name)
        .unwrap_or_else(|| panic!("'{name}' is a declared member: {resp}"));
    m["disabled"].as_bool().unwrap()
}

#[test]
fn remove_member_leaves_every_role_list_holding_it() {
    let mut h = started();
    // A hand-authored role conflict, also disabled. Removing it must not leave it
    // declared in `discover:` with the overlay gone, a silent re-mount.
    seed_ws_yaml(
        &h,
        "type: au.engine.workspace::au-engine\nedit:\n  - v\n  - a\ndiscover:\n  - a\ndisabled:\n  - a\n",
    );
    let resp = h
        .client
        .query(&json!({ "mutate": "remove_workspace_member", "name": "a" }))
        .unwrap();
    assert_eq!(resp["ready"], true, "{resp}");
    assert_eq!(
        ws_yaml(&h),
        "type: au.engine.workspace::au-engine\nedit:\n  - v\ndiscover: []\ndisabled: []\n",
        "the name left both role lists and the overlay in one commit"
    );
}

#[test]
fn set_disabled_switches_a_declared_member_off_and_back_on() {
    let mut h = started_with_siblings();
    h.client
        .query(&json!({ "mutate": "add_workspace_member", "name": "peer", "role": "edit" }))
        .unwrap();
    assert!(
        !member_disabled(&mut h, "peer"),
        "mounted before the switch"
    );

    let off = h
        .client
        .query(
            &json!({ "mutate": "set_workspace_member_disabled", "name": "peer", "disabled": true }),
        )
        .unwrap();
    assert_eq!(off["ready"], true, "{off}");
    assert!(off["result"]["commit"].is_string(), "one commit: {off}");
    assert_eq!(
        ws_yaml(&h),
        "type: au.engine.workspace::au-engine\nedit:\n  - ws\n  - peer\ndisabled:\n  - peer\n",
        "the role list is untouched, the overlay gains the name"
    );
    assert!(member_disabled(&mut h, "peer"), "the rebuild unmounted it");

    let on = h
        .client
        .query(&json!({ "mutate": "set_workspace_member_disabled", "name": "peer", "disabled": false }))
        .unwrap();
    assert_eq!(on["ready"], true, "{on}");
    assert_eq!(
        ws_yaml(&h),
        "type: au.engine.workspace::au-engine\nedit:\n  - ws\n  - peer\ndisabled: []\n",
        "re-enabling the last one leaves an empty list"
    );
    assert!(!member_disabled(&mut h, "peer"), "mounted again");
}

#[test]
fn set_disabled_rejects_what_it_cannot_author() {
    let mut h = started();
    // No workspace.yaml yet: nothing declares a member to switch.
    let absent = h
        .client
        .query(&json!({ "mutate": "set_workspace_member_disabled", "name": "a", "disabled": true }))
        .unwrap();
    assert_eq!(absent["type"], "error", "absent workspace.yaml: {absent}");

    h.client
        .query(&json!({ "mutate": "add_workspace_member", "name": "a", "role": "edit" }))
        .unwrap();
    let before = ws_yaml(&h);
    for (req, why) in [
        (
            json!({ "name": "v", "disabled": true }),
            "the containing repo",
        ),
        (
            json!({ "name": "ghost", "disabled": true }),
            "an undeclared name",
        ),
        (
            json!({ "name": "a", "disabled": false }),
            "re-enabling a name not disabled",
        ),
    ] {
        let mut req = req;
        req["mutate"] = json!("set_workspace_member_disabled");
        let resp = h.client.query(&req).unwrap();
        assert_eq!(resp["type"], "error", "{why} rejects: {resp}");
    }
    assert_eq!(ws_yaml(&h), before, "nothing written by a rejection");

    h.client
        .query(&json!({ "mutate": "set_workspace_member_disabled", "name": "a", "disabled": true }))
        .unwrap();
    let twice = h
        .client
        .query(&json!({ "mutate": "set_workspace_member_disabled", "name": "a", "disabled": true }))
        .unwrap();
    assert_eq!(
        twice["type"], "error",
        "an already-disabled name rejects: {twice}"
    );
}

#[test]
fn set_role_moves_a_member_between_lists() {
    let mut h = started();
    h.client
        .query(&json!({ "mutate": "add_workspace_member", "name": "a", "role": "edit" }))
        .unwrap();
    let resp = h
        .client
        .query(&json!({ "mutate": "set_workspace_member_role", "name": "a", "role": "discover" }))
        .unwrap();
    assert_eq!(resp["ready"], true, "{resp}");
    assert_eq!(
        ws_yaml(&h),
        "type: au.engine.workspace::au-engine\nedit:\n  - v\ndiscover:\n  - a\n",
        "the member moved from edit to discover in one commit"
    );
}

/// The entry `ws` beside two sibling repos, `peer` and `other`, all at one
/// depth. Both siblings sort before `ws`, so a default scope chosen by path depth
/// (or by root order) lands on a sibling, never the entry.
fn started_with_siblings() -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let base = fs::canonicalize(dir.path()).unwrap();
    for name in ["ws", "peer", "other"] {
        let reg = base.join(name).join(".arsumbris");
        fs::create_dir_all(&reg).unwrap();
        fs::write(reg.join("repo.yaml"), format!("name: {name}\n")).unwrap();
    }
    let root = base.join("ws");
    git(&root, &["init", "-q"]);
    git(&root, &["config", "user.email", "t@t"]);
    git(&root, &["config", "user.name", "t"]);
    git(&root, &["add", "-A"]);
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
        root,
    }
}

#[test]
fn an_absent_root_authors_the_served_entry_beside_sibling_members() {
    let mut h = started_with_siblings();
    // Mount a discover sibling first: a default scope that fell to it would
    // refuse every later verb as not editable.
    let first = h
        .client
        .query(&json!({ "mutate": "add_workspace_member", "name": "other", "role": "discover" }))
        .unwrap();
    assert_eq!(first["ready"], true, "{first}");
    // Then an edit sibling: a default scope that fell to it would author
    // `peer/.arsumbris/workspace.yaml` and commit in `peer`.
    let second = h
        .client
        .query(&json!({ "mutate": "add_workspace_member", "name": "peer", "role": "edit" }))
        .unwrap();
    assert_eq!(second["ready"], true, "{second}");
    let third = h
        .client
        .query(&json!({ "mutate": "set_workspace_member_role", "name": "other", "role": "edit" }))
        .unwrap();
    assert_eq!(third["ready"], true, "{third}");

    assert_eq!(
        ws_yaml(&h),
        "type: au.engine.workspace::au-engine\nedit:\n  - ws\n  - peer\n  - other\ndiscover: []\n",
        "every verb authored the entry's workspace.yaml"
    );
    let base = h.root.parent().unwrap();
    for sibling in ["peer", "other"] {
        assert!(
            !base
                .join(sibling)
                .join(".arsumbris/workspace.yaml")
                .exists(),
            "no workspace.yaml was authored in sibling '{sibling}'"
        );
    }
}

#[test]
fn a_root_argument_resolves_by_canonical_path() {
    let mut h = started_with_siblings();
    // A second spelling of the entry root, through a symlink. The member root is
    // recorded canonical, so a raw comparison would miss it.
    let link = h.root.parent().unwrap().join("ws-link");
    std::os::unix::fs::symlink(&h.root, &link).unwrap();
    let resp = h
        .client
        .query(&json!({
            "mutate": "add_workspace_member",
            "name": "peer",
            "role": "edit",
            "root": link.display().to_string(),
        }))
        .unwrap();
    assert_eq!(
        resp["ready"], true,
        "a symlinked root spelling resolves: {resp}"
    );
    assert_eq!(
        ws_yaml(&h),
        "type: au.engine.workspace::au-engine\nedit:\n  - ws\n  - peer\n",
        "the write landed in the entry the link points at"
    );
}

#[test]
fn an_absent_root_repo_config_read_resolves_to_the_served_entry() {
    let mut h = started_with_siblings();
    h.client
        .query(&json!({ "mutate": "add_workspace_member", "name": "peer", "role": "edit" }))
        .unwrap();
    let resp = h
        .client
        .query(&json!({
            "read": "config",
            "scope": "repo",
            "consumer": "host-app",
            "file": "panel.yaml",
            "type": "anything",
        }))
        .unwrap();
    assert_eq!(
        resp["result"]["config"]["path"],
        json!(
            h.root
                .join(".arsumbris/host-app/config/panel.yaml")
                .display()
                .to_string()
        ),
        "repo scope with no root resolves under the entry, not a sibling: {resp}"
    );
}

#[test]
fn set_role_rejects_a_non_member() {
    let mut h = started();
    h.client
        .query(&json!({ "mutate": "add_workspace_member", "name": "a", "role": "edit" }))
        .unwrap();
    let resp = h
        .client
        .query(
            &json!({ "mutate": "set_workspace_member_role", "name": "ghost", "role": "discover" }),
        )
        .unwrap();
    assert_eq!(
        resp["type"], "error",
        "re-roling a non-member rejects: {resp}"
    );
}
