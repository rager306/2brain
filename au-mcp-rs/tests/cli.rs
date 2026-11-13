#![cfg(unix)]

//! `--help`, `--version`, `--info`, and the exit codes of a bad launch.

use std::process::Command;

fn binary() -> Command {
    Command::new(env!("CARGO_BIN_EXE_au-mcp-rs"))
}

fn unified() -> Command {
    Command::new(env!("CARGO_BIN_EXE_arsumbris"))
}

#[test]
fn help_lists_the_contract_without_a_socket() {
    let output = binary().arg("--help").output().unwrap();
    assert!(output.status.success(), "{output:?}");
    let text = String::from_utf8(output.stdout.clone()).unwrap();
    for expected in [
        "au-mcp-rs",
        "USAGE",
        "--allow-mutations",
        "--tools full|concise",
        "--payload both|text|structured",
        "--info",
        "2026-07-28",
        "docs/connecting.md",
    ] {
        assert!(text.contains(expected), "help lacks {expected}");
    }
    // Nothing goes to stderr on a requested help.
    assert!(output.stderr.is_empty(), "{output:?}");
}

#[test]
fn version_is_one_line() {
    let output = binary().arg("--version").output().unwrap();
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout.clone()).unwrap();
    assert_eq!(
        text.trim(),
        format!("au-mcp-rs {}", env!("CARGO_PKG_VERSION"))
    );
    // The short form is the same.
    let short = binary().arg("-V").output().unwrap();
    assert_eq!(short.stdout, output.stdout);
}

#[test]
fn info_describes_the_server_without_an_engine() {
    let output = binary().arg("--info").output().unwrap();
    assert!(output.status.success(), "{output:?}");
    let text = String::from_utf8(output.stdout).unwrap();
    for expected in [
        "MCP stdio server",
        "2026-07-28",
        "registered,",
        "advertised:",
        "no socket given",
    ] {
        assert!(text.contains(expected), "info lacks {expected}: {text}");
    }
}

#[test]
fn info_reports_an_unreachable_engine_without_failing() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("missing.sock");
    let output = binary().arg("--info").arg(&socket).output().unwrap();
    assert!(output.status.success(), "{output:?}");
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("engine"), "{text}");
    assert!(text.contains("unusable"), "{text}");
}

#[test]
fn a_bad_option_exits_two_and_points_at_help() {
    let output = binary().arg("--bogus").output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    let text = String::from_utf8(output.stderr).unwrap();
    assert!(text.contains("unknown option: --bogus"), "{text}");
    assert!(text.contains("try 'au-mcp-rs --help'"), "{text}");
}

#[test]
fn a_missing_socket_argument_exits_two() {
    let output = binary().output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("usage: au-mcp-rs")
    );
}

#[test]
fn the_unified_binary_exposes_both_halves() {
    let output = unified().arg("--help").output().unwrap();
    assert!(output.status.success(), "{output:?}");
    let text = String::from_utf8(output.stdout).unwrap();
    for expected in ["daemon", "open", "members", "mcp", "info"] {
        assert!(text.contains(expected), "help lacks {expected}: {text}");
    }
    let version = unified().arg("--version").output().unwrap();
    assert_eq!(
        String::from_utf8(version.stdout).unwrap().trim(),
        format!("arsumbris {}", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn the_unified_binary_names_itself_in_info() {
    let output = unified().arg("info").output().unwrap();
    assert!(output.status.success(), "{output:?}");
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.starts_with("arsumbris "), "{text}");
    assert!(text.contains("2026-07-28"), "{text}");
}

#[test]
fn the_mcp_subcommand_takes_the_same_flags() {
    let output = unified().arg("mcp").arg("--help").output().unwrap();
    assert!(output.status.success(), "{output:?}");
    let text = String::from_utf8(output.stdout).unwrap();
    for expected in [
        "--policy",
        "--allow-mutations",
        "--principal",
        "--tools",
        "--payload",
        "<SOCKET>",
    ] {
        assert!(text.contains(expected), "mcp help lacks {expected}: {text}");
    }
}

#[test]
fn engine_subcommands_are_delegated_not_reimplemented() {
    // `members` on a directory without `.arsumbris/repo.yaml` fails the same way
    // the engine's own binary does, which shows the delegation is real.
    let temp = tempfile::tempdir().unwrap();
    let output = unified().arg("members").arg(temp.path()).output().unwrap();
    assert!(!output.status.success());
    let text = String::from_utf8(output.stderr).unwrap();
    assert!(text.contains("not a folder-repo"), "{text}");
}
