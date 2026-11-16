#![cfg(unix)]

//! `2brain doctor`: the exit code follows the verdicts, a missing daemon is a
//! failure with a fix, and a private daemon that answers makes it say so.
//!
//! The daemon is the test's own — `HOME` is a temporary directory, so the socket
//! under it cannot be the live one.

use std::{fs, process::Command};

mod support;

use support::Daemon;

fn doctor(home: &std::path::Path, target: &std::path::Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_2brain"))
        .arg("doctor")
        .arg(target)
        .env("HOME", home)
        .output()
        .expect("run 2brain doctor")
}

fn init(home: &std::path::Path, workspace: &std::path::Path) {
    let created = Command::new(env!("CARGO_BIN_EXE_2brain"))
        .arg("init")
        .arg(workspace)
        .env("HOME", home)
        .output()
        .expect("run 2brain init");
    assert!(
        created.status.success(),
        "{}",
        String::from_utf8_lossy(&created.stderr)
    );
}

#[test]
fn doctor_fails_when_no_daemon_answers_the_socket() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let workspace = temp.path().join("quiet");
    init(&home, &workspace);

    let report = doctor(&home, &workspace);
    let stdout = String::from_utf8_lossy(&report.stdout);
    assert!(
        !report.status.success(),
        "a missing daemon must fail the run: {stdout}"
    );
    assert!(
        stdout.lines().any(|line| line.starts_with("fail")),
        "{stdout}"
    );
    let socket_line = stdout
        .lines()
        .find(|line| line.contains("socket"))
        .unwrap_or_else(|| panic!("no socket line: {stdout}"));
    assert!(socket_line.starts_with("ok"), "{socket_line}");
    assert!(
        stdout.contains("2brain daemon start"),
        "the failure names the command that fixes it: {stdout}"
    );
    assert!(
        stdout.contains("tools advertised"),
        "the MCP server is checked without an engine: {stdout}"
    );
}

#[test]
fn doctor_reports_no_failures_when_a_private_daemon_answers() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let workspace = temp.path().join("served");
    init(&home, &workspace);

    let daemon = Daemon::start(&home, &workspace);
    let report = doctor(&home, &workspace);
    let stdout = String::from_utf8_lossy(&report.stdout);
    assert!(
        report.status.success(),
        "a serving daemon leaves nothing to fix: {stdout}"
    );
    assert!(
        !stdout.lines().any(|line| line.starts_with("fail")),
        "{stdout}"
    );
    assert!(
        stdout.contains("1 member, no diagnostics"),
        "the workspace line counts what the engine sees: {stdout}"
    );
    assert!(
        stdout.contains("tools advertised"),
        "the MCP server answered tools/list: {stdout}"
    );
    assert!(
        stdout.contains("2brain doctor:") && stdout.contains("no failures"),
        "{stdout}"
    );
    drop(daemon);
}
