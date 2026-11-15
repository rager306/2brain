//! `au members <entry> --json`: the daemon-less member resolution through the
//! binary, reading the per-user registry and package cache under `$HOME` exactly as
//! `au daemon start` would.

use std::fs;
use std::path::Path;
use std::process::Command;

use serde_json::Value;

fn repo(root: &Path, yaml: &str) {
    fs::create_dir_all(root.join(".arsumbris")).unwrap();
    fs::write(root.join(".arsumbris/repo.yaml"), yaml).unwrap();
}

fn run(entry: &Path, home: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_au"))
        .arg("members")
        .arg(entry)
        .args(args)
        .env("HOME", home)
        .output()
        .unwrap()
}

#[test]
fn json_reports_each_member_tier_from_the_user_registry_and_siblings() {
    let dir = tempfile::tempdir().unwrap();
    let top = fs::canonicalize(dir.path()).unwrap();
    let home = top.join("home");
    let entry = top.join("ws");
    repo(&entry, "name: ws\n");
    fs::write(
        entry.join(".arsumbris/workspace.yaml"),
        "edit:\n  - ws\ndiscover:\n  - sib\n  - reg\n  - gone\n",
    )
    .unwrap();
    repo(&top.join("sib"), "name: sib\n");
    let reg = top.join("elsewhere").join("reg-checkout");
    repo(&reg, "name: reg\n");
    let config = home.join(".arsumbris/au-engine/config");
    fs::create_dir_all(&config).unwrap();
    fs::write(
        config.join("repos.yaml"),
        format!("repos:\n  - name: reg\n    path: {}\n", reg.display()),
    )
    .unwrap();

    let out = run(&entry, &home, &["--json"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(v["schema_version"].is_u64());
    assert_eq!(v["complete"], true);
    let tier = |name: &str| {
        v["members"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["name"] == name)
            .unwrap_or_else(|| panic!("no member {name}: {v}"))["tier"]
            .clone()
    };
    assert_eq!(tier("ws"), "entry");
    assert_eq!(tier("sib"), "sibling");
    assert_eq!(tier("reg"), "registry");
    assert_eq!(tier("gone"), "unmounted");
    let gone = v["members"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["name"] == "gone")
        .unwrap();
    assert_eq!(gone["code"], "discover-member-unmounted");
    assert!(gone.get("path").is_none());
}

#[test]
fn a_non_repo_entry_exits_not_a_repo() {
    let dir = tempfile::tempdir().unwrap();
    let out = run(dir.path(), dir.path(), &["--json"]);
    assert_eq!(out.status.code(), Some(5));
    assert!(out.stdout.is_empty());
}

/// `au daemon start` refuses without `$HOME` (no registry, no cache), so
/// `au members` refuses too rather than answering something the daemon would not.
#[test]
fn an_unset_home_exits_env_error_like_the_daemon() {
    let dir = tempfile::tempdir().unwrap();
    let entry = dir.path().join("ws");
    repo(&entry, "name: ws\n");
    let out = Command::new(env!("CARGO_BIN_EXE_au"))
        .arg("members")
        .arg(&entry)
        .arg("--json")
        .env_remove("HOME")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty());
}
