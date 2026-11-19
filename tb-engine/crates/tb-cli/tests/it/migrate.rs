//! `tb-engine migrate identity`: after the final rename it is a check, not a rewrite. It reports
//! `current` for a repository that carries `.2brain/repo.yaml` and refuses one that does not.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

fn run(home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_tb-engine"))
        .args(["migrate", "identity"])
        .args(args)
        .env("HOME", home)
        .env_remove("2BRAIN_HOME")
        .output()
        .unwrap()
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn repo_with_identity(root: &Path, name: &str) {
    fs::create_dir_all(root.join(".2brain")).unwrap();
    fs::write(root.join(".2brain/repo.yaml"), format!("name: {name}\n")).unwrap();
}

#[test]
fn a_current_repository_reports_current_and_exits_zero() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    repo_with_identity(&root, "repo");

    let out = run(tmp.path(), &["--root", root.to_str().unwrap()]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(stdout(&out).contains("current"), "{}", stdout(&out));
    assert!(stdout(&out).contains("nothing to do"), "{}", stdout(&out));
}

#[test]
fn a_missing_identity_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("empty");
    fs::create_dir_all(&root).unwrap();

    let out = run(tmp.path(), &["--root", root.to_str().unwrap()]);
    assert!(
        !out.status.success(),
        "a missing identity must exit nonzero"
    );
    assert!(stdout(&out).contains("missing"), "{}", stdout(&out));
    assert!(
        stderr(&out).contains("there is no legacy spelling"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn a_dry_run_writes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    repo_with_identity(&root, "repo");
    let before = fs::read(root.join(".2brain/repo.yaml")).unwrap();

    let out = run(tmp.path(), &["--root", root.to_str().unwrap(), "--dry-run"]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(fs::read(root.join(".2brain/repo.yaml")).unwrap(), before);
}
