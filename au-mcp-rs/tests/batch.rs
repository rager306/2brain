#![cfg(unix)]

//! `au_batch`: several file ops armed one by one, sent as one engine saga.

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
                if tx
                    .send(serde_json::from_str(&line.unwrap()).unwrap())
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
        let init = client.request(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"batch-test","version":"1"}}}));
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

fn members_frame(root: &str) -> Value {
    json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":5,
        "result":{"members":[{"repo":"fixture","root":root,"editable":true,"role":"entry"}]}})
}

fn content_frame(hash: &str) -> Value {
    json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":6,
        "result":{"content":{"text":"x","hash":hash,"commit":null}}})
}

fn error_text(response: &Value) -> String {
    assert_eq!(response["result"]["isError"], true, "{response}");
    response["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[test]
fn batch_sends_one_saga_with_absolute_paths_and_every_hash() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("ws");
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let root_string = root.to_str().unwrap().to_owned();
    let expected = root_string.clone();
    let daemon = thread::spawn(move || {
        // Two au_content reads: members then content, apiece.
        for hash in ["hash-a", "hash-b"] {
            let (mut members, _) = listener.accept().unwrap();
            assert_eq!(
                read_request(&mut members),
                json!({"read":"members","id":1,"repo":"fixture"})
            );
            write_frame(&mut members, members_frame(&expected));
            let (mut content, _) = listener.accept().unwrap();
            let _ = read_request(&mut content);
            write_frame(&mut content, content_frame(hash));
        }
        // au_batch arms each path: one editability read per op.
        for _ in 0..2 {
            let (mut members, _) = listener.accept().unwrap();
            assert_eq!(
                read_request(&mut members),
                json!({"read":"members","id":1,"repo":"fixture"})
            );
            write_frame(&mut members, members_frame(&expected));
        }
        let (mut mutate, _) = listener.accept().unwrap();
        assert_eq!(
            read_request(&mut mutate),
            json!({"mutate":"batch","id":1,"ops":[
                {"op":"write_file","path":format!("{expected}/notes/a.md"),"content":"A","expected_hash":"hash-a"},
                {"op":"delete_file","path":format!("{expected}/notes/b.md"),"expected_hash":"hash-b"}
            ]})
        );
        write_frame(
            &mut mutate,
            json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":9,
                "result":{"ops":[{"op":"write_file","path":format!("{expected}/notes/a.md"),"hash":"h1"},
                                  {"op":"delete_file","path":format!("{expected}/notes/b.md"),"hash":null}],
                          "diagnostics":[],"reflected":true,"commit":null,
                          "commits":{"fixture":"c0ffee"},"untracked_files":[]}}),
        );
    });
    let mut client = Client::start(&socket);
    let _ = client.call(
        2,
        "au_content",
        json!({"repo":"fixture","path":"notes/a.md"}),
    );
    let _ = client.call(
        3,
        "au_content",
        json!({"repo":"fixture","path":"notes/b.md"}),
    );
    let reply = client.call(
        4,
        "au_batch",
        json!({"repo":"fixture","ops":[
            {"op":"write_file","path":"notes/a.md","content":"A","expected_hash":"hash-a"},
            {"op":"delete_file","path":"notes/b.md","expected_hash":"hash-b"}
        ]}),
    );
    assert_ne!(reply["result"]["isError"], true, "{reply}");
    let payload: Value =
        serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(payload["ops"].as_array().unwrap().len(), 2);
    assert_eq!(payload["commits"]["fixture"], "c0ffee");
    daemon.join().unwrap();
}

#[test]
fn batch_refuses_when_one_op_was_never_read() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut client = Client::start(&socket);
    let text = error_text(&client.call(
        2,
        "au_batch",
        json!({"repo":"fixture","ops":[
            {"op":"write_file","path":"notes/a.md","content":"A","expected_hash":"hash-a"}
        ]}),
    ));
    assert!(text.contains("au_content"), "{text}");
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock,
        "nothing may reach the engine when an op is unarmed"
    );
}

#[test]
fn batch_refuses_an_empty_list_before_the_engine() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut client = Client::start(&socket);
    let text = error_text(&client.call(2, "au_batch", json!({"repo":"fixture","ops":[]})));
    assert!(text.contains("'ops' must not be empty"), "{text}");
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}
