#![cfg(unix)]

//! Structural mutations: assign_block_id, rename_block_id, rename. Each goes
//! through the same gate as the write tools and sends the engine's mutate frame.

use std::{os::unix::net::UnixListener, thread, time::Duration};

use serde_json::{Value, json};

mod support;

use support::{Client, payload, read_request, write_response_frame};

const TIMEOUT: Duration = Duration::from_secs(10);
const ROOT: &str = "/tmp/fixture";

/// Serve `members`, the arming `content` read, the gate's `members` read, then
/// the mutation, asserting the exact mutate frame.
fn serve_mutation(
    expected_mutation: Value,
    result: Value,
) -> (
    tempfile::TempDir,
    std::path::PathBuf,
    thread::JoinHandle<()>,
) {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let handle = thread::spawn(move || {
        for verb in ["members", "content", "members"] {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(TIMEOUT)).unwrap();
            let request = read_request(&mut stream);
            assert_eq!(request["read"], verb, "{request}");
            match verb {
                "members" => write_response_frame(
                    &mut stream,
                    11,
                    json!({"members":[{"repo":"fixture","editable":true,"root":ROOT}]}),
                ),
                _ => write_response_frame(
                    &mut stream,
                    11,
                    json!({"content":{"text":"x","hash":"hash-one","commit":null}}),
                ),
            }
        }
        let (mut stream, _) = listener.accept().unwrap();
        stream.set_read_timeout(Some(TIMEOUT)).unwrap();
        let request = read_request(&mut stream);
        assert_eq!(request, expected_mutation, "mutate frame");
        write_response_frame(&mut stream, 11, result);
    });
    (temp, socket, handle)
}

#[test]
fn assign_block_id_sends_at_and_returns_the_ref() {
    let (temp, socket, handle) = serve_mutation(
        json!({"mutate":"assign_block_id","id":1,"path":"/tmp/fixture/notes/a.md","at":84}),
        json!({"path":"/tmp/fixture/notes/a.md","hash":"hash-two","id":"b-1","ref":"[[a^b-1]]"}),
    );
    let mut client = Client::start_with_timeout(&socket, &["--allow-mutations"], TIMEOUT);
    let read = client.call_result(
        2,
        "tb_content",
        json!({"repo":"fixture","path":"notes/a.md"}),
    );
    assert_ne!(read["isError"], true, "{read}");
    let result = client.call_result(
        3,
        "tb_assign_block_id",
        json!({"repo":"fixture","path":"notes/a.md","at":84,"expected_hash":"hash-one"}),
    );
    assert_ne!(result["isError"], true, "{result}");
    assert_eq!(payload(&result)["id"], "b-1");
    drop(client);
    handle.join().unwrap();
    drop(temp);
}

#[test]
fn rename_block_id_sends_both_ids() {
    let (temp, socket, handle) = serve_mutation(
        json!({"mutate":"rename_block_id","id":1,"path":"/tmp/fixture/notes/a.md",
               "block_id":"old","to_block_id":"new"}),
        json!({"path":"/tmp/fixture/notes/a.md","hash":"hash-two"}),
    );
    let mut client = Client::start_with_timeout(&socket, &["--allow-mutations"], TIMEOUT);
    let _ = client.call_result(
        2,
        "tb_content",
        json!({"repo":"fixture","path":"notes/a.md"}),
    );
    let result = client.call_result(
        3,
        "tb_rename_block_id",
        json!({"repo":"fixture","path":"notes/a.md","block_id":"old","to_block_id":"new","expected_hash":"hash-one"}),
    );
    assert_ne!(result["isError"], true, "{result}");
    drop(client);
    handle.join().unwrap();
    drop(temp);
}

#[test]
fn rename_sends_the_target_as_an_absolute_path_in_the_same_member() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let handle = thread::spawn(move || {
        // The `to` resolution reads members first, then the gate runs.
        for verb in ["members", "content", "members", "members"] {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(TIMEOUT)).unwrap();
            assert_eq!(read_request(&mut stream)["read"], verb);
            match verb {
                "members" => write_response_frame(
                    &mut stream,
                    11,
                    json!({"members":[{"repo":"fixture","editable":true,"root":ROOT}]}),
                ),
                _ => write_response_frame(
                    &mut stream,
                    11,
                    json!({"content":{"text":"x","hash":"hash-one","commit":null}}),
                ),
            }
        }
        let (mut stream, _) = listener.accept().unwrap();
        let request = read_request(&mut stream);
        assert_eq!(
            request,
            json!({"mutate":"rename","id":1,"path":"/tmp/fixture/notes/a.md","to":"/tmp/fixture/notes/b.md"})
        );
        write_response_frame(
            &mut stream,
            11,
            json!({"path":"/tmp/fixture/notes/b.md","hash":"hash-two"}),
        );
    });
    let mut client = Client::start_with_timeout(&socket, &["--allow-mutations"], TIMEOUT);
    let _ = client.call_result(
        2,
        "tb_content",
        json!({"repo":"fixture","path":"notes/a.md"}),
    );
    let result = client.call_result(
        3,
        "tb_rename",
        json!({"repo":"fixture","path":"notes/a.md","to":"notes/b.md","expected_hash":"hash-one"}),
    );
    assert_ne!(result["isError"], true, "{result}");
    drop(client);
    handle.join().unwrap();
    drop(temp);
}

#[test]
fn a_read_only_launch_hides_and_refuses_the_structural_mutations() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    UnixListener::bind(&socket).unwrap();
    let mut client = Client::start_with_timeout(&socket, &[], TIMEOUT);
    let tools =
        client.request(json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}))["result"]
            ["tools"]
            .clone();
    for name in ["tb_assign_block_id", "tb_rename_block_id", "tb_rename"] {
        assert!(
            !tools
                .as_array()
                .unwrap()
                .iter()
                .any(|tool| tool["name"] == name),
            "{name} is advertised without a grant"
        );
        let refused = client.request(json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":name,"arguments":{}}}));
        assert!(refused.get("error").is_some(), "{name}: {refused}");
    }
}

#[test]
fn inline_arms_both_files_then_sends_one_saga() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let handle = thread::spawn(move || {
        // tb_content(path), tb_content(into), then one editability read per file.
        for _ in 0..6 {
            let (mut stream, _) = listener.accept().unwrap();
            let request = read_request(&mut stream);
            match request["read"].as_str() {
                Some("members") => write_response_frame(
                    &mut stream,
                    11,
                    json!({"members":[{"repo":"fixture","editable":true,"root":ROOT}]}),
                ),
                _ => write_response_frame(
                    &mut stream,
                    11,
                    json!({"content":{"text":"x","hash":"hash-one","commit":null}}),
                ),
            }
        }
        let (mut stream, _) = listener.accept().unwrap();
        assert_eq!(
            read_request(&mut stream),
            json!({"mutate":"inline","id":1,"path":"/tmp/fixture/notes/b.md","into":"/tmp/fixture/notes/a.md"})
        );
        write_response_frame(
            &mut stream,
            11,
            json!({"path":"/tmp/fixture/notes/a.md","hash":"hash-two"}),
        );
    });
    let mut client = Client::start_with_timeout(&socket, &["--allow-mutations"], TIMEOUT);
    let _ = client.call_result(
        2,
        "tb_content",
        json!({"repo":"fixture","path":"notes/b.md"}),
    );
    let _ = client.call_result(
        3,
        "tb_content",
        json!({"repo":"fixture","path":"notes/a.md"}),
    );
    let result = client.call_result(
        4,
        "tb_inline",
        json!({"repo":"fixture","path":"notes/b.md","into":"notes/a.md",
               "expected_hash":"hash-one","into_expected_hash":"hash-one"}),
    );
    assert_ne!(result["isError"], true, "{result}");
    assert_eq!(payload(&result)["hash"], "hash-two");
    drop(client);
    handle.join().unwrap();
    drop(temp);
}

#[test]
fn promote_needs_exactly_one_record_locator() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    UnixListener::bind(&socket).unwrap();
    let mut client = Client::start_with_timeout(&socket, &["--allow-mutations"], TIMEOUT);
    for arguments in [
        json!({"repo":"fixture","path":"notes/a.md","to":"notes/x.md","expected_hash":"hash-one"}),
        json!({"repo":"fixture","path":"notes/a.md","to":"notes/x.md","at":10,"block_id":"b","expected_hash":"hash-one"}),
    ] {
        let result = client.call_result(3, "tb_promote", arguments.clone());
        assert_eq!(result["isError"], true, "{arguments}: {result}");
        assert!(
            result["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("exactly one"),
            "{result}"
        );
    }
}
