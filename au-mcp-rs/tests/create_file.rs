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
    fn start(socket: &Path, grants: &[&str]) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_au-mcp-rs"));
        command.arg(socket);
        for grant in grants {
            command.arg(grant);
        }
        let mut child = command
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
        let init = client.request(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"create-test","version":"1"}}}));
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

fn content_frame(content: Value) -> Value {
    json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":6,
        "result":{"content":content}})
}

fn error_text(response: &Value) -> String {
    assert_eq!(response["result"]["isError"], true, "{response}");
    response["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .to_owned()
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
        write_frame(&mut members, members_frame(&expected_root));
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
        write_frame(&mut members2, members_frame(&expected_root));
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
    let mut client = Client::start(&socket, &["--allow-mutations"]);
    let created = client.call(
        2,
        "au_create_file",
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
        write_frame(&mut members, members_frame(&expected_root));
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
    let mut client = Client::start(&socket, &["--allow-mutations"]);
    let text = error_text(&client.call(
        2,
        "au_create_file",
        json!({"repo":"fixture","path":"inbox/new.md","content":"# new\n"}),
    ));
    assert!(text.starts_with("policy/absence:"), "{text}");
    assert!(text.contains("au_write_file"), "{text}");
    daemon.join().unwrap();
}

#[test]
fn create_needs_the_mutation_grant() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut client = Client::start(&socket, &[]);
    let tools = client.request(json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}));
    assert!(
        !tools["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool["name"] == "au_create_file"),
        "a read-only launch must not advertise au_create_file"
    );
    let refused = client.request(json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"au_create_file","arguments":{"repo":"fixture","path":"inbox/new.md","content":"x"}}}));
    assert!(refused["error"]["message"].is_string(), "{refused}");
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}
