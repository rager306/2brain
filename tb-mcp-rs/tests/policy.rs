#![cfg(unix)]

use std::{
    io::Read,
    os::unix::net::UnixListener,
    process::{Command, Stdio},
    thread,
    time::Duration,
};

use serde_json::{Value, json};

mod support;

use support::{Client, error_text, read_request, write_frame};

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
        "tb_write_file",
        "tb_edit_file",
        "tb_delete_file",
        "tb_assign_block_id",
        "tb_rename_block_id",
        "tb_rename",
        "tb_inline",
        "tb_promote",
        "tb_create_file",
        "tb_batch",
    ] {
        assert!(
            !names.contains(&tool.to_owned()),
            "{tool} must be hidden: {names:?}"
        );
    }
    assert!(names.contains(&"tb_content".to_owned()), "{names:?}");

    let denied = client.call(
        3,
        "tb_write_file",
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
        "tb_write_file",
        "tb_edit_file",
        "tb_delete_file",
        "tb_assign_block_id",
        "tb_rename_block_id",
        "tb_rename",
        "tb_inline",
        "tb_promote",
        "tb_create_file",
        "tb_batch",
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
    assert!(tool_names(&mut client).contains(&"tb_content".to_owned()));
    // A mutation in a member that was never read first is refused by the
    // read-before-write guard, which runs before the member check.
    let unread = client.call(
        3,
        "tb_write_file",
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
    let mut child = Command::new(env!("CARGO_BIN_EXE_2brain"))
        .arg("mcp")
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
    let output = Command::new(env!("CARGO_BIN_EXE_2brain"))
        .arg("mcp")
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
        "tb_write_file",
        json!({"repo":"allowed","path":"../x.md","content":"x","expected_hash":"h"}),
    );
    let text = escape["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.starts_with("policy/path_scope:"), "{text}");

    // member_grant beats prior_read: the member is refused before the read check.
    let member = client.call(
        4,
        "tb_write_file",
        json!({"repo":"other","path":"a.md","content":"x","expected_hash":"h"}),
    );
    let text = member["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.starts_with("policy/member_grant:"), "{text}");

    // prior_read is the remaining gate for an allowed member.
    let unread = client.call(
        5,
        "tb_write_file",
        json!({"repo":"allowed","path":"a.md","content":"x","expected_hash":"h"}),
    );
    let text = unread["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.starts_with("policy/prior_read:"), "{text}");

    // A second mutating tool, refused at the same gate, so the ledger has two tools to tell apart.
    let escaped_delete = client.call(
        6,
        "tb_delete_file",
        json!({"repo":"allowed","path":"../y.md","expected_hash":"h"}),
    );
    let text = escaped_delete["result"]["content"][0]["text"]
        .as_str()
        .unwrap();
    assert!(text.starts_with("policy/path_scope:"), "{text}");

    assert_not_connected(&listener);
    // The ledger names the tool that was refused, not a shared placeholder: an audit record reading
    // `mutation` cannot say which of the six mutating tools ran, which is the one thing it is for.
    let audit = audit_lines(&client);
    let tools: Vec<String> = audit
        .iter()
        .map(|line| line["tool"].as_str().unwrap_or_default().to_owned())
        .collect();
    assert!(
        tools
            .iter()
            .all(|tool| tool == "tb_write_file" || tool == "tb_delete_file"),
        "{tools:?}"
    );
    assert!(tools.contains(&"tb_delete_file".to_owned()), "{tools:?}");
    let stages: Vec<String> = audit
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

/// A grant for the parent member must not authorise a write inside a NESTED member the policy
/// never listed (review finding #21). The engine writes into the DEEPEST owning member, so the
/// adapter has to ask who really owns the joined path, not trust the name in the arguments.
#[test]
fn a_path_owned_by_an_ungranted_nested_member_is_refused() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("parent");
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let listener_for_daemon = listener;
    let policy_path = temp.path().join("policy.json");
    std::fs::write(
        &policy_path,
        r#"{"allow_mutations":true,"members":["parent"]}"#,
    )
    .unwrap();
    let root_string = root.to_str().unwrap().to_owned();
    let daemon = thread::spawn(move || {
        // tb_content read: members, then content.
        let (mut members, _) = listener_for_daemon.accept().unwrap();
        let _ = read_request(&mut members);
        write_frame(
            &mut members,
            json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":5,
                "result":{"members":[{"repo":"parent","root":root_string,"editable":true,
                    "role":"entry","tier":"entry","scattered":false,"local":true,
                    "disabled":false,"git":{"root":root_string,"tracked":true}}]}}),
        );
        let (mut content, _) = listener_for_daemon.accept().unwrap();
        let _ = read_request(&mut content);
        write_frame(
            &mut content,
            json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":6,
                "result":{"content":{"text":"x","hash":"hash-1","commit":null}}}),
        );
        // The write arms itself: members, then the owner question, which answers the NESTED
        // member. Nothing else may arrive: the refusal happens before the mutation.
        let (mut members2, _) = listener_for_daemon.accept().unwrap();
        let _ = read_request(&mut members2);
        write_frame(
            &mut members2,
            json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":5,
                "result":{"members":[{"repo":"parent","root":root_string,"editable":true,
                    "role":"entry","tier":"entry","scattered":false,"local":true,
                    "disabled":false,"git":{"root":root_string,"tracked":true}}]}}),
        );
        let (mut resolve, _) = listener_for_daemon.accept().unwrap();
        let request = read_request(&mut resolve);
        assert_eq!(request["read"], "resolve_member", "{request}");
        write_frame(
            &mut resolve,
            json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":6,
                "result":{"resolve_member":{"repo":"child","root":format!("{root_string}/child"),
                    "editable":true,"local":true,"role":"edit"}}}),
        );
    });
    let mut client = Client::start_with(&socket, &["--policy", policy_path.to_str().unwrap()]);
    let _ = client.call(
        2,
        "tb_content",
        json!({"repo":"parent","path":"child/note.md"}),
    );
    let reply = client.call(
        3,
        "tb_write_file",
        json!({"repo":"parent","path":"child/note.md","content":"x","expected_hash":"hash-1"}),
    );
    let text = error_text(&reply);
    assert!(
        text.contains("child") && text.contains("does not permit"),
        "the refusal names the ungranted owner: {text}"
    );
    daemon.join().unwrap();
}

/// `tb_rename` rewrites inbound referrers, which can live in another member, so its request
/// carries the policy's member allowlist the way `tb_rename_type` does (review finding #16) —
/// and carries nothing when the policy has no allowlist, so a policy-free launch is unchanged.
#[test]
fn tb_rename_sends_the_member_grant_only_when_the_policy_has_one() {
    for with_policy in [true, false] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("parent");
        let socket = temp.path().join("engine.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let root_string = root.to_str().unwrap().to_owned();
        let daemon = thread::spawn(move || {
            loop {
                let (mut stream, _) = listener.accept().unwrap();
                let request = read_request(&mut stream);
                let reply = match request["read"].as_str() {
                    Some("members") => json!({"type":"response","schema_version":29,"id":1,
                    "ready":true,"version":5,"result":{"members":[{"repo":"parent",
                        "root":root_string,"editable":true,"role":"entry","tier":"entry",
                        "scattered":false,"local":true,"disabled":false,
                        "git":{"root":root_string,"tracked":true}}]}}),
                    Some("resolve_member") => json!({"type":"response","schema_version":29,"id":1,
                    "ready":true,"version":5,"result":{"resolve_member":{"repo":"parent",
                        "root":root_string,"editable":true,"local":true,"role":"entry"}}}),
                    Some("content") => json!({"type":"response","schema_version":29,"id":1,
                    "ready":true,"version":6,"result":{"content":{"text":"x",
                        "hash":"hash-1","commit":null}}}),
                    _ => {
                        assert_eq!(request["mutate"], "rename", "{request}");
                        assert_eq!(
                            request
                                .get("allowed_members")
                                .cloned()
                                .unwrap_or(Value::Null),
                            if with_policy {
                                json!(["parent"])
                            } else {
                                Value::Null
                            },
                            "the grant travels only when the policy lists members: {request}"
                        );
                        json!({"type":"response","schema_version":29,"id":1,"ready":true,
                        "version":7,"result":{"path":format!("{root_string}/b.md"),
                            "hash":"h2","diagnostics":[],"reflected":true,"commit":null,
                            "commits":{},"untracked_files":[]}})
                    }
                };
                write_frame(&mut stream, reply);
            }
        });
        let policy_path = temp.path().join("policy.json");
        std::fs::write(
            &policy_path,
            r#"{"allow_mutations":true,"members":["parent"]}"#,
        )
        .unwrap();
        let mut extra: Vec<&str> = vec!["--allow-mutations"];
        if with_policy {
            extra.push("--policy");
            extra.push(policy_path.to_str().unwrap());
        }
        let mut client = Client::start_with(&socket, &extra);
        let _ = client.call(2, "tb_content", json!({"repo":"parent","path":"a.md"}));
        let reply = client.call(
            3,
            "tb_rename",
            json!({"repo":"parent","path":"a.md","to":"b.md","expected_hash":"hash-1"}),
        );
        assert_ne!(reply["result"]["isError"], true, "{reply}");
        drop(client);
        drop(daemon);
    }
}
