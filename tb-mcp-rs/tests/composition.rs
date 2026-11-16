#![cfg(unix)]

//! The composition mutations: rename_type, add_dep, remove_dep.

use std::{os::unix::net::UnixListener, thread, time::Duration};

use serde_json::json;

mod support;

use support::{Client, payload, read_request, write_response_frame};

const TIMEOUT: Duration = Duration::from_secs(10);
const ROOT: &str = "/tmp/fixture";

#[test]
fn rename_type_verifies_the_type_hash_then_mutates() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        assert_eq!(
            read_request(&mut stream),
            json!({"read":"type","id":1,"name":"note"})
        );
        write_response_frame(
            &mut stream,
            12,
            json!({"type":{"name":"note","hash":"type-hash","repo":"fixture"}}),
        );
        // The cascade is previewed before it is attempted: the adapter checks
        // every affected member against the grant, not only the owner.
        let (mut stream, _) = listener.accept().unwrap();
        assert_eq!(
            read_request(&mut stream),
            json!({"read":"preview_mutation","id":1,"op":"rename_type","old_name":"note","new_name":"memo"})
        );
        write_response_frame(
            &mut stream,
            12,
            json!({"preview_mutation":{
                "target":{
                    "path":"/tmp/fixture/type/memo.type.yaml",
                    "hash":"h2",
                    "identities":[],
                    "diagnostics":[]
                },
                "blast_radius":[],
                "untracked_files":[],
                "affected":[
                    {"path":"/tmp/fixture/type/note.type.yaml","repo":"fixture","changes":["definition_move"]}
                ]
            }}),
        );
        let (mut stream, _) = listener.accept().unwrap();
        assert_eq!(
            read_request(&mut stream),
            json!({"mutate":"rename_type","id":1,"old_name":"note","new_name":"memo"})
        );
        write_response_frame(
            &mut stream,
            12,
            json!({"path":"/tmp/fixture/type/memo.type.yaml","hash":"h2"}),
        );
    });
    let mut client = Client::start_with_timeout(&socket, &["--allow-mutations"], TIMEOUT);
    let result = client.call_result(
        2,
        "tb_rename_type",
        json!({"old_name":"note","new_name":"memo","expected_hash":"type-hash"}),
    );
    assert_ne!(result["isError"], true, "{result}");
    assert_eq!(payload(&result)["path"], "/tmp/fixture/type/memo.type.yaml");
    drop(client);
    handle.join().unwrap();
    drop(temp);
}

#[test]
fn rename_type_refuses_a_stale_hash_without_mutating() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        assert_eq!(read_request(&mut stream)["read"], "type");
        write_response_frame(
            &mut stream,
            12,
            json!({"type":{"name":"note","hash":"newer","repo":"fixture"}}),
        );
        // The refusal must be local: no second connection may arrive.
        listener.set_nonblocking(true).unwrap();
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    });
    let mut client = Client::start_with_timeout(&socket, &["--allow-mutations"], TIMEOUT);
    let result = client.call_result(
        2,
        "tb_rename_type",
        json!({"old_name":"note","new_name":"memo","expected_hash":"older"}),
    );
    assert_eq!(result["isError"], true, "{result}");
    assert!(
        result["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("does not match"),
        "{result}"
    );
    drop(client);
    handle.join().unwrap();
    drop(temp);
}

#[test]
fn rename_type_refuses_a_cascade_outside_the_member_grant() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    // The two members, on disk: A defines the type, B holds an instance. The
    // adapter must refuse before any mutation connection, so neither file moves.
    let a = temp.path().join("ws/a");
    std::fs::create_dir_all(a.join("type")).unwrap();
    let def = a.join("type/widget.type.yaml");
    std::fs::write(&def, "fields:\n  label: String\n").unwrap();
    let b = temp.path().join("ws/b");
    std::fs::create_dir_all(&b).unwrap();
    let note = b.join("note.md");
    std::fs::write(&note, "---\ntype: widget::a\n---\n").unwrap();
    let def_payload = json!({
        "path": def.display().to_string(),
        "repo": "a",
        "changes": ["definition_move"],
    });
    let note_payload = json!({
        "path": note.display().to_string(),
        "repo": "b",
        "changes": ["claim"],
    });
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        assert_eq!(
            read_request(&mut stream),
            json!({"read":"type","id":1,"name":"widget::a"})
        );
        write_response_frame(
            &mut stream,
            12,
            json!({"type":{"name":"widget","hash":"type-hash","repo":"a"}}),
        );
        let (mut stream, _) = listener.accept().unwrap();
        let preview = read_request(&mut stream);
        assert_eq!(preview["read"], "preview_mutation", "{preview}");
        assert_eq!(preview["op"], "rename_type", "{preview}");
        write_response_frame(
            &mut stream,
            12,
            json!({"preview_mutation":{
                "target":{
                    "path": "/tmp/ws/a/type/gadget.type.yaml",
                    "hash": "h2",
                    "identities": [],
                    "diagnostics": []
                },
                "blast_radius": [],
                "untracked_files": [],
                "affected": [def_payload, note_payload]
            }}),
        );
        // The refusal is local: no mutation may follow.
        listener.set_nonblocking(true).unwrap();
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    });
    let policy = temp.path().join("policy.json");
    std::fs::write(&policy, r#"{"allow_mutations":true,"members":["a"]}"#).unwrap();
    let mut client = Client::start_capturing_stderr(
        &socket,
        &[
            "--policy",
            policy.to_str().unwrap(),
            "--principal",
            "tester",
        ],
    );
    let result = client.call_result(
        2,
        "tb_rename_type",
        json!({"old_name":"widget::a","new_name":"gadget","expected_hash":"type-hash"}),
    );
    assert_eq!(result["isError"], true, "{result}");
    let message = result["content"][0]["text"].as_str().unwrap_or_default();
    assert!(
        message.starts_with("policy/member_grant: "),
        "the refusal carries the existing stage code: {message}"
    );
    assert!(
        message.contains("'b'") && message.contains("note.md"),
        "it names the member outside the grant and a path there: {message}"
    );
    assert!(
        message.contains("'a'") && message.contains("widget::a"),
        "it says the defining member IS allowed, so the situation reads straight: {message}"
    );
    // The audit records the same vocabulary, against the member outside the grant.
    let stderr = client.stderr().expect("policy client captures stderr");
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    let mut denial = None;
    while std::time::Instant::now() < deadline {
        let Ok(line) = stderr.recv_timeout(Duration::from_millis(100)) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        if value["event"] == "mcp_policy" && value["tool"] == "tb_rename_type" {
            denial = Some(value);
            break;
        }
    }
    let denial = denial.expect("an audit record was written");
    assert_eq!(denial["outcome"], "denied:member_grant", "{denial}");
    assert_eq!(
        denial["member"], "b",
        "the denial names the cascade's member: {denial}"
    );
    drop(client);
    handle.join().unwrap();

    assert_eq!(
        std::fs::read(&def).unwrap(),
        b"fields:\n  label: String\n",
        "the defining file is untouched"
    );
    assert_eq!(
        std::fs::read(&note).unwrap(),
        b"---\ntype: widget::a\n---\n",
        "the referrer outside the grant is untouched"
    );
    assert!(
        !a.join("type/gadget.type.yaml").exists(),
        "nothing was renamed"
    );
}

#[test]
fn add_dep_resolves_the_root_and_sends_the_entry() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        assert_eq!(read_request(&mut stream)["read"], "members");
        write_response_frame(
            &mut stream,
            12,
            json!({"members":[{"repo":"fixture","editable":true,"root":ROOT}]}),
        );
        let (mut stream, _) = listener.accept().unwrap();
        assert_eq!(
            read_request(&mut stream),
            json!({"mutate":"add_dep","id":1,"name":"peer","root":ROOT,"expected_hash":"h1"})
        );
        write_response_frame(
            &mut stream,
            12,
            json!({"path":"/tmp/fixture/.arsumbris/repo.yaml","hash":"h2"}),
        );
    });
    let mut client = Client::start_with_timeout(
        &socket,
        &["--allow-mutations", "--allow-composition"],
        TIMEOUT,
    );
    let result = client.call_result(
        2,
        "tb_add_dep",
        json!({"repo":"fixture","dep":"peer","expected_hash":"h1"}),
    );
    assert_ne!(result["isError"], true, "{result}");
    drop(client);
    handle.join().unwrap();
    drop(temp);
}

#[test]
fn composition_needs_its_own_grant() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    UnixListener::bind(&socket).unwrap();
    let mut client = Client::start_with_timeout(&socket, &["--allow-mutations"], TIMEOUT);
    let tools =
        client.request(json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}))["result"]
            ["tools"]
            .clone();
    for name in ["tb_add_dep", "tb_remove_dep"] {
        assert!(
            !tools
                .as_array()
                .unwrap()
                .iter()
                .any(|tool| tool["name"] == name),
            "{name} is advertised without the composition grant"
        );
        let refused = client.request(json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":name,"arguments":{"repo":"fixture","dep":"x"}}}));
        assert!(
            refused["error"]["message"]
                .as_str()
                .unwrap_or_default()
                .contains("composition"),
            "{refused}"
        );
    }
    // A write tool stays available with only the mutation grant.
    assert!(
        tools
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool["name"] == "tb_write_file")
    );
}

#[test]
fn workspace_shape_needs_its_own_grant() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    UnixListener::bind(&socket).unwrap();
    // Mutations plus composition, but no workspace-shape grant.
    let mut client = Client::start_with_timeout(
        &socket,
        &["--allow-mutations", "--allow-composition"],
        TIMEOUT,
    );
    let tools =
        client.request(json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}))["result"]
            ["tools"]
            .clone();
    for name in [
        "tb_add_workspace_member",
        "tb_remove_workspace_member",
        "tb_set_workspace_member_role",
    ] {
        assert!(
            !tools
                .as_array()
                .unwrap()
                .iter()
                .any(|tool| tool["name"] == name),
            "{name} is advertised without the workspace-shape grant"
        );
        let refused = client.request(json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":name,"arguments":{"repo":"fixture","name":"x","role":"edit"}}}));
        assert!(
            refused["error"]["message"]
                .as_str()
                .unwrap_or_default()
                .contains("workspace"),
            "{refused}"
        );
    }
    // Composition tools stay available with only that grant.
    assert!(
        tools
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool["name"] == "tb_add_dep")
    );
}

#[test]
fn add_workspace_member_sends_the_role_and_root() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        assert_eq!(read_request(&mut stream)["read"], "members");
        write_response_frame(
            &mut stream,
            12,
            json!({"members":[{"repo":"fixture","editable":true,"root":ROOT}]}),
        );
        let (mut stream, _) = listener.accept().unwrap();
        assert_eq!(
            read_request(&mut stream),
            json!({"mutate":"add_workspace_member","id":1,"name":"peer","role":"discover","root":ROOT})
        );
        write_response_frame(
            &mut stream,
            12,
            json!({"path":"/tmp/fixture/.arsumbris/workspace.yaml","hash":"h2"}),
        );
    });
    let mut client = Client::start_with_timeout(
        &socket,
        &["--allow-mutations", "--allow-workspace-shape"],
        TIMEOUT,
    );
    let result = client.call_result(
        2,
        "tb_add_workspace_member",
        json!({"repo":"fixture","name":"peer","role":"discover"}),
    );
    assert_ne!(result["isError"], true, "{result}");
    drop(client);
    handle.join().unwrap();
    drop(temp);
}
