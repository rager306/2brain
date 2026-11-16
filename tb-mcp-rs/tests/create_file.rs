#![cfg(unix)]

use std::{os::unix::net::UnixListener, thread, time::Duration};

use serde_json::{Value, json};

mod support;

use support::{Client, error_text, members_frame, read_request, write_frame};

fn content_frame(content: Value) -> Value {
    json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":6,
        "result":{"content":content}})
}

fn write_response(root: &str) -> Value {
    json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":7,
        "result":{"path":format!("{root}/inbox/new.md"),"hash":"hash-new",
            "diagnostics":[],"reflected":true,"commit":null,"commits":{},"untracked_files":[]}})
}

#[test]
fn create_writes_a_new_file_without_a_hash_precondition() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("ws");
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let root_string = root.to_str().unwrap().to_owned();
    let expected_root = root_string.clone();
    let daemon = thread::spawn(move || {
        // Absence: resolve the root, then ask the engine for the text.
        let (mut members, _) = listener.accept().unwrap();
        assert_eq!(
            read_request(&mut members),
            json!({"read":"members","id":1,"repo":"fixture"})
        );
        write_frame(&mut members, members_frame(&expected_root, 5, true));
        let (mut content, _) = listener.accept().unwrap();
        assert_eq!(
            read_request(&mut content),
            json!({"read":"content","id":1,"path":format!("{expected_root}/inbox/new.md")})
        );
        write_frame(&mut content, content_frame(Value::Null));
        // Editability: the member must be authored by the engine.
        let (mut members2, _) = listener.accept().unwrap();
        assert_eq!(
            read_request(&mut members2),
            json!({"read":"members","id":1,"repo":"fixture"})
        );
        write_frame(&mut members2, members_frame(&expected_root, 5, true));
        let (mut mutate, _) = listener.accept().unwrap();
        let frame = read_request(&mut mutate);
        assert_eq!(
            frame,
            json!({"mutate":"write_file","id":1,
                "path":format!("{expected_root}/inbox/new.md"),"content":"# new\n"}),
            "a create must send no expected_hash"
        );
        write_frame(&mut mutate, write_response(&expected_root));
    });
    let mut client = Client::start_with(&socket, &["--allow-mutations"]);
    let created = client.call(
        2,
        "tb_create_file",
        json!({"repo":"fixture","path":"inbox/new.md","content":"# new\n"}),
    );
    assert_ne!(created["result"]["isError"], true, "{created}");
    let payload: Value =
        serde_json::from_str(created["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(payload["hash"], "hash-new");
    daemon.join().unwrap();
}

#[test]
fn create_refuses_an_existing_path_and_writes_nothing() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("ws");
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let root_string = root.to_str().unwrap().to_owned();
    let expected_root = root_string.clone();
    let daemon = thread::spawn(move || {
        let (mut members, _) = listener.accept().unwrap();
        assert_eq!(
            read_request(&mut members),
            json!({"read":"members","id":1,"repo":"fixture"})
        );
        write_frame(&mut members, members_frame(&expected_root, 5, true));
        let (mut content, _) = listener.accept().unwrap();
        assert_eq!(
            read_request(&mut content),
            json!({"read":"content","id":1,"path":format!("{expected_root}/inbox/new.md")})
        );
        write_frame(
            &mut content,
            content_frame(json!({"text":"already here","hash":"hash-old","commit":null})),
        );
        // No mutation frame may follow: the create is refused locally.
        listener.set_nonblocking(true).unwrap();
        thread::sleep(Duration::from_millis(150));
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    });
    let mut client = Client::start_with(&socket, &["--allow-mutations"]);
    let text = error_text(&client.call(
        2,
        "tb_create_file",
        json!({"repo":"fixture","path":"inbox/new.md","content":"# new\n"}),
    ));
    assert!(text.starts_with("policy/absence:"), "{text}");
    assert!(text.contains("tb_write_file"), "{text}");
    daemon.join().unwrap();
}

#[test]
fn create_needs_the_mutation_grant() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut client = Client::start(&socket);
    let tools = client.request(json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}));
    assert!(
        !tools["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool["name"] == "tb_create_file"),
        "a read-only launch must not advertise tb_create_file"
    );
    let refused = client.request(json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"tb_create_file","arguments":{"repo":"fixture","path":"inbox/new.md","content":"x"}}}));
    assert!(refused["error"]["message"].is_string(), "{refused}");
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}
