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
            json!({"mutate":"create_file","id":1,
                "path":format!("{expected_root}/inbox/new.md"),"content":"# new\n"}),
            "a create must send the engine verb that carries the absence precondition"
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

/// The engine's conflict IS the absence gate, seen a few milliseconds later by a
/// second process: the client must hear the refusal it would have heard had it
/// looked after the competitor wrote, not a raw engine message.
#[test]
fn an_engine_conflict_is_reported_as_the_absence_refusal() {
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
        // This process sees the path absent: the competitor has not written yet.
        let (mut content, _) = listener.accept().unwrap();
        assert_eq!(
            read_request(&mut content),
            json!({"read":"content","id":1,"path":format!("{expected_root}/inbox/new.md")})
        );
        write_frame(&mut content, content_frame(Value::Null));
        let (mut members2, _) = listener.accept().unwrap();
        assert_eq!(
            read_request(&mut members2),
            json!({"read":"members","id":1,"repo":"fixture"})
        );
        write_frame(&mut members2, members_frame(&expected_root, 5, true));
        // The engine's create: the competitor took the path in between.
        let (mut mutate, _) = listener.accept().unwrap();
        assert_eq!(
            read_request(&mut mutate),
            json!({"mutate":"create_file","id":1,
                "path":format!("{expected_root}/inbox/new.md"),"content":"# new\n"})
        );
        write_frame(
            &mut mutate,
            json!({"type":"error","schema_version":29,"id":1,"for":"mutate",
                "error":"file already exists: create_file never overwrites",
                "detail":{"reason":"already_exists","current_hash":"hash-winner"}}),
        );
    });
    let mut client = Client::start_capturing_stderr(&socket, &["--allow-mutations"]);
    let text = error_text(&client.call(
        2,
        "tb_create_file",
        json!({"repo":"fixture","path":"inbox/new.md","content":"# new\n"}),
    ));
    assert_eq!(
        text, "policy/absence: file already exists; use tb_write_file to change it",
        "a lost race reads as the absence gate, word for word"
    );
    // The audit names the same stage the pre-check refuses at, so one refusal has one label.
    let mut audited = false;
    while let Ok(line) = client
        .stderr()
        .expect("stderr captured")
        .recv_timeout(Duration::from_millis(400))
    {
        if let Ok(value) = serde_json::from_str::<Value>(&line)
            && value["event"] == "mcp_policy"
        {
            assert_eq!(value["tool"], "tb_create_file", "{value}");
            assert_eq!(value["outcome"], "denied:absence", "{value}");
            audited = true;
        }
    }
    assert!(audited, "the conflict must be audited");
    daemon.join().unwrap();
}

/// Any other engine refusal is the engine's own answer, passed through: the
/// mapping is narrow on purpose, keyed on `detail.reason`, never on message text.
#[test]
fn an_engine_rejection_that_is_not_a_conflict_is_passed_through() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("ws");
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let root_string = root.to_str().unwrap().to_owned();
    let expected_root = root_string.clone();
    let daemon = thread::spawn(move || {
        let (mut members, _) = listener.accept().unwrap();
        read_request(&mut members);
        write_frame(&mut members, members_frame(&expected_root, 5, true));
        let (mut content, _) = listener.accept().unwrap();
        read_request(&mut content);
        write_frame(&mut content, content_frame(Value::Null));
        let (mut members2, _) = listener.accept().unwrap();
        read_request(&mut members2);
        write_frame(&mut members2, members_frame(&expected_root, 5, true));
        let (mut mutate, _) = listener.accept().unwrap();
        read_request(&mut mutate);
        write_frame(
            &mut mutate,
            json!({"type":"error","schema_version":29,"id":1,"for":"mutate",
                "error":"git cannot classify a path this mutation writes"}),
        );
    });
    let mut client = Client::start_with(&socket, &["--allow-mutations"]);
    let text = error_text(&client.call(
        2,
        "tb_create_file",
        json!({"repo":"fixture","path":"inbox/new.md","content":"# new\n"}),
    ));
    assert!(
        text.contains("git cannot classify"),
        "the engine's own refusal reaches the client: {text}"
    );
    assert!(!text.starts_with("policy/absence:"), "{text}");
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
