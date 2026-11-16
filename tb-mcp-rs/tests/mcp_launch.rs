#![cfg(unix)]

//! `2brain mcp <workspace>` derives its socket from the state root under `$HOME`.
//! When that root is not the daemon's, the only symptom was `connecting to
//! engine: No such file or directory` on every call, with nothing saying which
//! path was tried or why. The first failure now names the socket, the state root
//! it came from, and the directory it looked in — and it says it once, because
//! repeating it per call is the noise it replaces.

use std::{fs, time::Duration};

use serde_json::json;

mod support;

use support::{Client, assert_error};

/// The stderr lines the adapter has written so far.
#[track_caller]
fn stderr_lines(client: &Client) -> Vec<String> {
    let stderr = client.stderr().expect("the launch test captures stderr");
    let mut lines = Vec::new();
    while let Ok(line) = stderr.recv_timeout(Duration::from_millis(600)) {
        lines.push(line);
    }
    lines
}

#[test]
fn the_first_failed_connection_names_the_state_root_and_the_socket() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    // A workspace target, not a `.sock` path: the socket must be derived, which
    // is where a wrong `$HOME` enters.
    let workspace = temp.path().join("workspace");
    fs::create_dir_all(&workspace).unwrap();

    let mut client = Client::start_capturing_stderr_at_home(&home, &workspace);
    // Two calls that fail the same way: the diagnosis belongs to the first.
    assert_error(&client.call(2, "tb_overview", json!({})));
    assert_error(&client.call(3, "tb_overview", json!({})));

    let lines = stderr_lines(&client);
    let reported = lines.join("\n");
    // The state root an empty `$HOME` yields: the engine falls back to the legacy
    // spelling when the new one does not exist. The socket file itself is hashed
    // from the workspace, so it is read out of the message and checked where it
    // lives, rather than re-derived here against this test's own `$HOME`.
    let state_root = home.join(".arsumbris");
    let run_dir = state_root.join("au-engine").join("run");
    let socket = lines
        .iter()
        .find_map(|line| line.split_once("socket  "))
        .and_then(|(_, rest)| rest.split(" — ").next())
        .unwrap_or_else(|| panic!("no socket line: {reported}"));

    assert!(
        reported.contains(&state_root.display().to_string()),
        "the diagnosis must name the state root {state_root:?}: {reported}"
    );
    assert!(
        reported.contains(&format!("$HOME={}", home.display())),
        "the diagnosis must name where the state root came from: {reported}"
    );
    assert!(
        socket.starts_with(run_dir.to_str().unwrap()) && socket.ends_with(".sock"),
        "the diagnosis must name the socket it tried, in {run_dir:?}: {reported}"
    );
    assert!(
        reported.contains("au-engine/run/*.sock"),
        "the diagnosis must name the run directory it looked in: {reported}"
    );
    assert!(
        reported.contains("2brain:   next"),
        "the diagnosis must say what to do next: {reported}"
    );
    let diagnoses = lines
        .iter()
        .filter(|line| line.starts_with("2brain: no engine answers"))
        .count();
    assert_eq!(
        diagnoses, 1,
        "the diagnosis is printed once, not per call: {lines:#?}"
    );
}

#[test]
fn an_explicit_socket_says_no_state_root_was_consulted() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let socket = temp.path().join("given.sock");

    let mut client = Client::start_capturing_stderr_at_home(&home, &socket);
    assert_error(&client.call(2, "tb_overview", json!({})));
    let lines = stderr_lines(&client);
    let reported = lines.join("\n");
    assert!(
        reported.contains(&format!("socket  {} ", socket.display())),
        "an explicit socket is named as given: {reported}"
    );
    assert!(
        reported.contains("given explicitly"),
        "an explicit socket has no state root to blame: {reported}"
    );
    assert!(
        !reported.contains(&home.display().to_string()),
        "no state root was consulted, so none is named: {reported}"
    );
}

/// The per-call answer stays the short one: the diagnosis is a log line, and a
/// tool error that carried four lines of it would be harder to read, not easier.
#[test]
fn the_tool_error_stays_the_engine_error() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let workspace = temp.path().join("workspace");
    fs::create_dir_all(&workspace).unwrap();

    let mut client = Client::start_capturing_stderr_at_home(&home, &workspace);
    let text = support::error_text(&client.call(2, "tb_overview", json!({})));
    assert!(
        text.starts_with("connecting to engine:"),
        "the tool answer names the transport failure it hit: {text}"
    );
    assert!(
        !text.contains("state root"),
        "the diagnosis belongs to the log, not to every answer: {text}"
    );
}
