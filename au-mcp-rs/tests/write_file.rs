#![cfg(unix)]

use std::{
    io::{BufRead, BufReader, Read, Write},
    os::unix::net::UnixListener,
    path::Path,
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver},
    thread,
    time::Duration,
};

use serde_json::{Value, json};

struct Client {
    child: Child,
    input: ChildStdin,
    responses: Receiver<Value>,
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Client {
    fn start(socket: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_au-mcp-rs"))
            .arg(socket)
            .arg("--allow-mutations")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, responses) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let line = line.unwrap();
                if tx
                    .send(serde_json::from_str(&line).expect("stdout must contain JSON"))
                    .is_err()
                {
                    break;
                }
            }
        });
        let mut client = Self {
            child,
            input,
            responses,
        };
        let init = client.request(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"write-test","version":"1"}}}));
        assert!(init.get("result").is_some(), "{init}");
        client.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
        client
    }

    fn send(&mut self, value: Value) {
        writeln!(self.input, "{value}").unwrap();
        self.input.flush().unwrap();
    }

    fn request(&mut self, value: Value) -> Value {
        self.send(value);
        self.responses
            .recv_timeout(Duration::from_secs(5))
            .expect("MCP response timeout")
    }

    fn call(&mut self, id: u32, name: &str, arguments: Value) -> Value {
        let response = self.request(json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":name,"arguments":arguments}}));
        assert_eq!(response["id"], id, "{response}");
        response
    }
}

fn read_request(stream: &mut impl Read) -> Value {
    let mut len = [0; 4];
    stream.read_exact(&mut len).unwrap();
    let mut body = vec![0; u32::from_be_bytes(len) as usize];
    stream.read_exact(&mut body).unwrap();
    serde_json::from_slice(&body).unwrap()
}

fn write_frame(stream: &mut impl Write, payload: Value) {
    let body = serde_json::to_vec(&payload).unwrap();
    stream
        .write_all(&(body.len() as u32).to_be_bytes())
        .unwrap();
    stream.write_all(&body).unwrap();
}

fn members_frame(root: &str, editable: bool) -> Value {
    json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":5,
        "result":{"members":[{"repo":"fixture","root":root,"editable":editable,"role":"entry"}]}})
}

fn error_text(response: &Value) -> String {
    assert_eq!(response["result"]["isError"], true, "{response}");
    response["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn assert_not_connected(listener: &UnixListener) {
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}

#[test]
fn write_requires_a_prior_read_of_the_same_path() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut client = Client::start(&socket);
    let text = error_text(&client.call(
        2,
        "au_write_file",
        json!({"repo":"fixture","path":"notes/a.md","content":"x","expected_hash":"abc"}),
    ));
    assert!(text.contains("au_content"), "{text}");
    assert_not_connected(&listener);
}

#[test]
fn write_rejects_a_stale_or_invalid_request_without_connecting() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut client = Client::start(&socket);
    for (i, args) in [
        json!({"repo":"","path":"a.md","content":"x","expected_hash":"abc"}),
        json!({"repo":"f","path":"","content":"x","expected_hash":"abc"}),
        json!({"repo":"f","path":"/abs.md","content":"x","expected_hash":"abc"}),
        json!({"repo":"f","path":"../x.md","content":"x","expected_hash":"abc"}),
        json!({"repo":"f","path":"a.md","content":"x","expected_hash":""}),
        json!({"repo":"f","path":"a.md","content":"x"}),
        json!({"repo":"f","path":"a.md","content":1,"expected_hash":"abc"}),
        json!({"repo":"f","path":"a.md","content":"x","expected_hash":"abc","force":true}),
    ]
    .into_iter()
    .enumerate()
    {
        assert!(!error_text(&client.call(i as u32 + 2, "au_write_file", args)).is_empty());
        assert_not_connected(&listener);
    }
}

#[test]
fn write_sends_the_guarded_mutation_and_invalidates_the_read() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("ws");
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let root_string = root.to_str().unwrap().to_owned();
    let daemon = thread::spawn(move || {
        // au_content: members, then content.
        let (mut members, _) = listener.accept().unwrap();
        assert_eq!(
            read_request(&mut members),
            json!({"read":"members","id":1,"repo":"fixture"})
        );
        write_frame(&mut members, members_frame(&root_string, true));
        let (mut content, _) = listener.accept().unwrap();
        assert_eq!(
            read_request(&mut content),
            json!({"read":"content","id":1,"path":format!("{root_string}/notes/a.md")})
        );
        write_frame(
            &mut content,
            json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":6,
                "result":{"content":{"text":"old","hash":"hash-1","commit":null}}}),
        );
        // au_write_file: members, then the mutation.
        let (mut members2, _) = listener.accept().unwrap();
        assert_eq!(
            read_request(&mut members2),
            json!({"read":"members","id":1,"repo":"fixture"})
        );
        write_frame(&mut members2, members_frame(&root_string, true));
        let (mut mutate, _) = listener.accept().unwrap();
        assert_eq!(
            read_request(&mut mutate),
            json!({"mutate":"write_file","id":1,"path":format!("{root_string}/notes/a.md"),
                "content":"new text","expected_hash":"hash-1"})
        );
        write_frame(
            &mut mutate,
            json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":7,
                "result":{"path":format!("{root_string}/notes/a.md"),"hash":"hash-2",
                    "diagnostics":[],"reflected":true,"commit":null,"commits":{},"untracked_files":[]}}),
        );
    });
    let mut client = Client::start(&socket);
    let read = client.call(
        2,
        "au_content",
        json!({"repo":"fixture","path":"notes/a.md"}),
    );
    assert_ne!(read["result"]["isError"], true, "{read}");
    let write = client.call(
        3,
        "au_write_file",
        json!({"repo":"fixture","path":"notes/a.md","content":"new text","expected_hash":"hash-1"}),
    );
    assert_ne!(write["result"]["isError"], true, "{write}");
    let payload: Value =
        serde_json::from_str(write["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(payload["hash"], "hash-2");
    assert_eq!(write["result"]["_meta"]["io.arsumbris/engineVersion"], "7");
    // The recorded read is gone: the same write must not be replayed.
    let replay = error_text(&client.call(
        4,
        "au_write_file",
        json!({"repo":"fixture","path":"notes/a.md","content":"new text","expected_hash":"hash-1"}),
    ));
    assert!(replay.contains("au_content"), "{replay}");
    daemon.join().unwrap();
}

#[test]
fn edit_requires_a_prior_read_and_sends_the_exact_edit() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("ws");
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let root_string = root.to_str().unwrap().to_owned();

    let mut client = Client::start(&socket);
    // Without a read, the edit is refused before any connection.
    listener.set_nonblocking(true).unwrap();
    let unread = error_text(&client.call(
        2,
        "au_edit_file",
        json!({"repo":"fixture","path":"notes/a.md","old_string":"old","new_string":"new","expected_hash":"hash-1"}),
    ));
    assert!(unread.contains("au_content"), "{unread}");
    assert_not_connected(&listener);
    listener.set_nonblocking(false).unwrap();

    let daemon = thread::spawn(move || {
        let (mut members, _) = listener.accept().unwrap();
        assert_eq!(
            read_request(&mut members),
            json!({"read":"members","id":1,"repo":"fixture"})
        );
        write_frame(&mut members, members_frame(&root_string, true));
        let (mut content, _) = listener.accept().unwrap();
        assert_eq!(
            read_request(&mut content),
            json!({"read":"content","id":1,"path":format!("{root_string}/notes/a.md")})
        );
        write_frame(
            &mut content,
            json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":6,
                "result":{"content":{"text":"old text","hash":"hash-1","commit":null}}}),
        );
        let (mut members2, _) = listener.accept().unwrap();
        assert_eq!(
            read_request(&mut members2),
            json!({"read":"members","id":1,"repo":"fixture"})
        );
        write_frame(&mut members2, members_frame(&root_string, true));
        let (mut mutate, _) = listener.accept().unwrap();
        // edit_file takes no expected_hash: the exact match is its precondition.
        assert_eq!(
            read_request(&mut mutate),
            json!({"mutate":"edit_file","id":1,"path":format!("{root_string}/notes/a.md"),
                "old_string":"old","new_string":"new","replace_all":false})
        );
        write_frame(
            &mut mutate,
            json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":8,
                "result":{"path":format!("{root_string}/notes/a.md"),"hash":"hash-2",
                    "diagnostics":[],"reflected":true,"commit":null,"commits":{},"untracked_files":[]}}),
        );
    });
    let read = client.call(
        3,
        "au_content",
        json!({"repo":"fixture","path":"notes/a.md"}),
    );
    assert_ne!(read["result"]["isError"], true, "{read}");
    let edit = client.call(
        4,
        "au_edit_file",
        json!({"repo":"fixture","path":"notes/a.md","old_string":"old","new_string":"new","expected_hash":"hash-1"}),
    );
    assert_ne!(edit["result"]["isError"], true, "{edit}");
    let payload: Value =
        serde_json::from_str(edit["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(payload["hash"], "hash-2");
    daemon.join().unwrap();
}

#[test]
fn delete_requires_a_prior_read_and_sends_the_guarded_delete() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("ws");
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let root_string = root.to_str().unwrap().to_owned();

    let mut client = Client::start(&socket);
    listener.set_nonblocking(true).unwrap();
    let unread = error_text(&client.call(
        2,
        "au_delete_file",
        json!({"repo":"fixture","path":"notes/a.md","expected_hash":"hash-1"}),
    ));
    assert!(unread.contains("au_content"), "{unread}");
    assert_not_connected(&listener);
    listener.set_nonblocking(false).unwrap();

    let daemon = thread::spawn(move || {
        let (mut members, _) = listener.accept().unwrap();
        assert_eq!(
            read_request(&mut members),
            json!({"read":"members","id":1,"repo":"fixture"})
        );
        write_frame(&mut members, members_frame(&root_string, true));
        let (mut content, _) = listener.accept().unwrap();
        assert_eq!(
            read_request(&mut content),
            json!({"read":"content","id":1,"path":format!("{root_string}/notes/a.md")})
        );
        write_frame(
            &mut content,
            json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":6,
                "result":{"content":{"text":"old","hash":"hash-1","commit":null}}}),
        );
        let (mut members2, _) = listener.accept().unwrap();
        assert_eq!(
            read_request(&mut members2),
            json!({"read":"members","id":1,"repo":"fixture"})
        );
        write_frame(&mut members2, members_frame(&root_string, true));
        let (mut mutate, _) = listener.accept().unwrap();
        assert_eq!(
            read_request(&mut mutate),
            json!({"mutate":"delete_file","id":1,"path":format!("{root_string}/notes/a.md"),
                "expected_hash":"hash-1"})
        );
        write_frame(
            &mut mutate,
            json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":9,
                "result":{"path":format!("{root_string}/notes/a.md"),"hash":null,
                    "diagnostics":[],"reflected":true,"commit":null,"commits":{},
                    "untracked_files":[],"last_live_commit":null}}),
        );
    });
    let read = client.call(
        3,
        "au_content",
        json!({"repo":"fixture","path":"notes/a.md"}),
    );
    assert_ne!(read["result"]["isError"], true, "{read}");
    let deleted = client.call(
        4,
        "au_delete_file",
        json!({"repo":"fixture","path":"notes/a.md","expected_hash":"hash-1"}),
    );
    assert_ne!(deleted["result"]["isError"], true, "{deleted}");
    let payload: Value =
        serde_json::from_str(deleted["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert!(payload["hash"].is_null(), "{payload}");
    daemon.join().unwrap();
}

#[test]
fn preview_rejects_incomplete_requests_without_connecting() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut client = Client::start(&socket);
    for (i, args) in [
        json!({"repo":"fixture","path":"a.md","op":"write_file"}),
        json!({"repo":"fixture","path":"a.md","op":"edit_file","new_string":"x"}),
        json!({"repo":"fixture","path":"a.md","op":"edit_file","old_string":"x"}),
        json!({"repo":"fixture","path":"a.md","op":"rename"}),
        json!({"repo":"","path":"a.md","op":"delete_file"}),
        json!({"repo":"fixture","path":"","op":"delete_file"}),
        json!({"repo":"fixture","path":"/abs.md","op":"delete_file"}),
        json!({"repo":"fixture","path":"../x.md","op":"delete_file"}),
        json!({"repo":"fixture","path":"a.md","op":"delete_file","content":"x"}),
        json!({"repo":"fixture","path":"a.md","op":"write_file","content":"x","old_string":"y"}),
    ]
    .into_iter()
    .enumerate()
    {
        let text = error_text(&client.call(i as u32 + 2, "au_preview_mutation", args));
        assert!(!text.is_empty());
        assert_not_connected(&listener);
    }
}

#[test]
fn preview_sends_the_op_and_returns_the_engine_product() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("ws");
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let root_string = root.to_str().unwrap().to_owned();
    let cases = [
        (
            json!({"repo":"fixture","path":"notes/a.md","op":"write_file","content":"new"}),
            json!({"read":"preview_mutation","id":1,"op":"write_file","path":format!("{root_string}/notes/a.md"),"content":"new"}),
        ),
        (
            json!({"repo":"fixture","path":"notes/a.md","op":"edit_file","old_string":"a","new_string":"b","replace_all":true}),
            json!({"read":"preview_mutation","id":1,"op":"edit_file","path":format!("{root_string}/notes/a.md"),"old_string":"a","new_string":"b","replace_all":true}),
        ),
        (
            json!({"repo":"fixture","path":"notes/a.md","op":"delete_file"}),
            json!({"read":"preview_mutation","id":1,"op":"delete_file","path":format!("{root_string}/notes/a.md")}),
        ),
    ];
    let expected = cases.clone();
    let daemon = thread::spawn(move || {
        for (_, request) in expected {
            let (mut members, _) = listener.accept().unwrap();
            assert_eq!(
                read_request(&mut members),
                json!({"read":"members","id":1,"repo":"fixture"})
            );
            write_frame(&mut members, members_frame(&root_string, true));
            let (mut preview, _) = listener.accept().unwrap();
            assert_eq!(read_request(&mut preview), request);
            write_frame(
                &mut preview,
                json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":11,
                    "result":{"preview_mutation":{"target":{"path":"x","hash":"h","identities":[],"diagnostics":[]},"blast_radius":[]}}}),
            );
        }
    });
    let mut client = Client::start(&socket);
    for (i, (args, _)) in cases.into_iter().enumerate() {
        let response = client.call(i as u32 + 2, "au_preview_mutation", args);
        assert_ne!(response["result"]["isError"], true, "{response}");
        let payload: Value =
            serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap())
                .unwrap();
        assert!(
            payload["preview_mutation"]["target"].is_object(),
            "{payload}"
        );
        assert_eq!(
            response["result"]["_meta"]["io.arsumbris/engineVersion"],
            "11"
        );
    }
    daemon.join().unwrap();
}
