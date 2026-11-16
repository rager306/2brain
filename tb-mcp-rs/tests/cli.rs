#![cfg(unix)]

//! `--help`, `--version`, `--info`, and the exit codes of a bad launch.

use std::process::Command;

/// The MCP server, reached through the one binary the product ships: `2brain mcp …`.
fn binary() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_2brain"));
    command.arg("mcp");
    command
}

fn unified() -> Command {
    Command::new(env!("CARGO_BIN_EXE_2brain"))
}

#[test]
fn help_lists_the_contract_without_a_socket() {
    let output = binary().arg("--help").output().unwrap();
    assert!(output.status.success(), "{output:?}");
    let text = String::from_utf8(output.stdout.clone()).unwrap();
    for expected in [
        "tb-mcp-rs",
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
    // Version plus the build identity: `0.1.0+<commit>` and `.dirty` when the
    // tree had uncommitted changes.
    let line = text.trim();
    let (package, build) = line
        .split_once('+')
        .unwrap_or_else(|| panic!("version carries no build metadata: {line}"));
    // The package name, which is `tb-mcp-rs` since 2026-11-16 (docs/naming.md): the binary says the
    // name it was compiled as, so this assertion tracks `CARGO_PKG_NAME` rather than a literal.
    // `2brain mcp --version` names the binary a client launches, which is `2brain` (the server's own
    // usage text still carries the package name, and that is what `--help` is for).
    assert_eq!(package, format!("2brain {}", env!("CARGO_PKG_VERSION")));
    assert!(
        build == "unknown"
            || build
                .trim_end_matches(".dirty")
                .chars()
                .all(|c| c.is_ascii_hexdigit()),
        "unexpected build id: {build}"
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
    assert!(
        text.contains(&format!("try '{} --help'", env!("CARGO_PKG_NAME"))),
        "{text}"
    );
}

#[test]
fn a_missing_socket_argument_exits_two() {
    let output = binary().output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("usage: tb-mcp-rs")
    );
}

#[test]
fn the_unified_binary_exposes_both_halves() {
    let output = unified().arg("--help").output().unwrap();
    assert!(output.status.success(), "{output:?}");
    let text = String::from_utf8(output.stdout).unwrap();
    for expected in ["daemon", "open", "members", "mcp", "info", "init", "doctor"] {
        assert!(text.contains(expected), "help lacks {expected}: {text}");
    }
    let version = unified().arg("--version").output().unwrap();
    let version = String::from_utf8(version.stdout).unwrap();
    assert!(
        version
            .trim()
            .starts_with(&format!("2brain {}+", env!("CARGO_PKG_VERSION"))),
        "{version}"
    );
}

#[test]
fn the_unified_binary_names_itself_in_info() {
    let output = unified().arg("info").output().unwrap();
    assert!(output.status.success(), "{output:?}");
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.starts_with("2brain "), "{text}");
    assert!(text.contains("2026-07-28"), "{text}");
}

#[test]
fn the_mcp_subcommand_documents_the_server_contract() {
    // `2brain mcp --help` is the MCP server's own USAGE — the same text a bare server would print —
    // so the flags are documented once, by the server, not twice by the dispatcher and the server.
    let output = binary().arg("--help").output().unwrap();
    assert!(output.status.success(), "{output:?}");
    let text = String::from_utf8(output.stdout).unwrap();
    for expected in [
        "--policy",
        "--allow-mutations",
        "--allow-composition",
        "--allow-workspace-shape",
        "--principal",
        "--tools full|concise",
        "--payload both|text|structured",
        "<engine-socket>",
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
