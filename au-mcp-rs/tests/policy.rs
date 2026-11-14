#![cfg(unix)]

use std::{
    io::Read,
    os::unix::net::UnixListener,
    process::{Command, Stdio},
    time::Duration,
};

use serde_json::{Value, json};

mod support;

use support::Client;

fn assert_not_connected(listener: &UnixListener) {
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}

fn tool_names(client: &mut Client) -> Vec<String> {
    let listed = client.request(json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}));
    listed["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap().to_owned())
        .collect()
}

fn audit_lines(client: &Client) -> Vec<Value> {
    let stderr = client.stderr().expect("policy client captures stderr");
    let mut lines = Vec::new();
    while let Ok(line) = stderr.recv_timeout(Duration::from_millis(400)) {
        if let Ok(value) = serde_json::from_str::<Value>(&line)
            && value["event"] == "mcp_policy"
        {
            lines.push(value);
        }
    }
    lines
}

#[test]
fn without_a_grant_the_server_is_read_only() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut client = Client::start_capturing_stderr(&socket, &[]);
    let names = tool_names(&mut client);
    for tool in [
        "au_write_file",
        "au_edit_file",
        "au_delete_file",
        "au_assign_block_id",
        "au_rename_block_id",
        "au_rename",
        "au_inline",
        "au_promote",
        "au_create_file",
        "au_batch",
    ] {
        assert!(
            !names.contains(&tool.to_owned()),
            "{tool} must be hidden: {names:?}"
        );
    }
    assert!(names.contains(&"au_content".to_owned()), "{names:?}");

    let denied = client.call(
        3,
        "au_write_file",
        json!({"repo":"fixture","path":"a.md","content":"x","expected_hash":"h"}),
    );
    assert!(denied.get("error").is_some(), "{denied}");
    assert!(
        denied["error"]["message"]
            .as_str()
            .unwrap()
            .contains("disabled"),
        "{denied}"
    );
    assert_not_connected(&listener);
    let audit = audit_lines(&client);
    assert!(
        audit.iter().any(|line| line["outcome"] == "denied"),
        "{audit:?}"
    );
}

#[test]
fn an_explicit_grant_exposes_the_mutating_tools() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut client = Client::start_capturing_stderr(&socket, &["--allow-mutations"]);
    let names = tool_names(&mut client);
    for tool in [
        "au_write_file",
        "au_edit_file",
        "au_delete_file",
        "au_assign_block_id",
        "au_rename_block_id",
        "au_rename",
        "au_inline",
        "au_promote",
        "au_create_file",
        "au_batch",
    ] {
        assert!(
            names.contains(&tool.to_owned()),
            "{tool} missing: {names:?}"
        );
    }
}

#[test]
fn a_member_allowlist_refuses_other_members_before_any_write() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let policy_path = temp.path().join("policy.json");
    std::fs::write(
        &policy_path,
        r#"{"allow_mutations":true,"members":["allowed"]}"#,
    )
    .unwrap();
    let mut client = Client::start_capturing_stderr(
        &socket,
        &[
            "--policy",
            policy_path.to_str().unwrap(),
            "--principal",
            "tester",
        ],
    );
    // Reads still work.
    assert!(tool_names(&mut client).contains(&"au_content".to_owned()));
    // A mutation in a member that was never read first is refused by the
    // read-before-write guard, which runs before the member check.
    let unread = client.call(
        3,
        "au_write_file",
        json!({"repo":"other","path":"a.md","content":"x","expected_hash":"h"}),
    );
    assert_eq!(unread["result"]["isError"], true, "{unread}");
    assert_not_connected(&listener);
}

#[test]
fn a_malformed_policy_stops_the_process_before_serving() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let policy_path = temp.path().join("policy.json");
    std::fs::write(&policy_path, "{not json").unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_au-mcp-rs"))
        .arg(&socket)
        .args(["--policy", policy_path.to_str().unwrap()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stdin.take());
    let status = child.wait().unwrap();
    assert!(!status.success(), "{status}");
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    assert!(stderr.contains("policy"), "{stderr}");
}

#[test]
fn unknown_options_are_rejected() {
    let output = Command::new(env!("CARGO_BIN_EXE_au-mcp-rs"))
        .args(["/tmp/none.sock", "--danger"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unknown option"), "{stderr}");
}

/// The gate order is observable: an earlier refusal wins even when a later one
/// would also apply.
#[test]
fn the_gate_order_is_observable_in_errors_and_audit() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let policy_path = temp.path().join("policy.json");
    std::fs::write(
        &policy_path,
        r#"{"allow_mutations":true,"members":["allowed"]}"#,
    )
    .unwrap();
    let mut client = Client::start_capturing_stderr(
        &socket,
        &[
            "--policy",
            policy_path.to_str().unwrap(),
            "--principal",
            "tester",
        ],
    );

    // path_scope beats prior_read: the path escapes before any read is required.
    let escape = client.call(
        3,
        "au_write_file",
        json!({"repo":"allowed","path":"../x.md","content":"x","expected_hash":"h"}),
    );
    let text = escape["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.starts_with("policy/path_scope:"), "{text}");

    // member_grant beats prior_read: the member is refused before the read check.
    let member = client.call(
        4,
        "au_write_file",
        json!({"repo":"other","path":"a.md","content":"x","expected_hash":"h"}),
    );
    let text = member["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.starts_with("policy/member_grant:"), "{text}");

    // prior_read is the remaining gate for an allowed member.
    let unread = client.call(
        5,
        "au_write_file",
        json!({"repo":"allowed","path":"a.md","content":"x","expected_hash":"h"}),
    );
    let text = unread["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.starts_with("policy/prior_read:"), "{text}");

    assert_not_connected(&listener);
    let stages: Vec<String> = audit_lines(&client)
        .iter()
        .map(|line| line["outcome"].as_str().unwrap().to_owned())
        .collect();
    assert!(
        stages.contains(&"denied:path_scope".to_owned()),
        "{stages:?}"
    );
    assert!(
        stages.contains(&"denied:member_grant".to_owned()),
        "{stages:?}"
    );
    assert!(
        stages.contains(&"denied:prior_read".to_owned()),
        "{stages:?}"
    );
}
