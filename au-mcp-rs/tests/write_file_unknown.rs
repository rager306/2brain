#![cfg(unix)]

use std::{
    io::{BufRead, BufReader, Read, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::Path,
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver},
    thread,
    time::Duration,
};

use serde_json::{Value, json};

const TIMEOUT: Duration = Duration::from_secs(5);

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
        let init = client.request(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"write-unknown-test","version":"1"}}}));
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
            .recv_timeout(TIMEOUT)
            .expect("MCP response timeout")
    }

    fn call(&mut self, id: u32, name: &str, arguments: Value) -> Value {
        let response = self.request(json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":name,"arguments":arguments}}));
        assert_eq!(response["id"], id, "{response}");
        response
    }

    fn send_call(&mut self, id: u32, name: &str, arguments: Value) {
        self.send(json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":name,"arguments":arguments}}));
    }

    fn receive_id(&self, id: u32) -> Value {
        let response = self
            .responses
            .recv_timeout(TIMEOUT)
            .expect("MCP response timeout");
        assert_eq!(response["id"], id, "{response}");
        response
    }
}

fn accept(listener: &UnixListener) -> UnixStream {
    let (stream, _) = listener.accept().unwrap();
    stream.set_read_timeout(Some(TIMEOUT)).unwrap();
    stream.set_write_timeout(Some(TIMEOUT)).unwrap();
    stream
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

fn members(root: &str, editable: bool, version: u64) -> Value {
    json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":version,
        "result":{"members":[{"repo":"fixture","root":root,"editable":editable,"role":"entry"}]}})
}

fn error_text(response: &Value) -> String {
    assert_eq!(response["result"]["isError"], true, "{response}");
    response["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// Answer one `members` read for `fixture`.
fn serve_members(listener: &UnixListener, root: &str, editable: bool, version: u64) {
    let mut stream = accept(listener);
    assert_eq!(
        read_request(&mut stream),
        json!({"read":"members","id":1,"repo":"fixture"})
    );
    write_frame(&mut stream, members(root, editable, version));
}

/// Read the file once through the adapter, leaving a recorded hash behind.
/// The request is sent first: the fake engine answers connections the adapter
/// opens only after it has the request.
fn prime(client: &mut Client, listener: &UnixListener, root: &str) {
    client.send_call(2, "au_content", json!({"repo":"fixture","path":"a.md"}));
    serve_members(listener, root, true, 5);
    let mut content = accept(listener);
    assert_eq!(
        read_request(&mut content),
        json!({"read":"content","id":1,"path":format!("{root}/a.md")})
    );
    write_frame(
        &mut content,
        json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":6,
            "result":{"content":{"text":"old","hash":"h1","commit":null}}}),
    );
    let read = client.receive_id(2);
    assert_ne!(read["result"]["isError"], true, "{read}");
}

#[test]
fn a_lost_mutation_reply_is_reported_as_outcome_unknown_and_not_retried() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("ws");
    let root_string = root.to_str().unwrap().to_owned();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let driver = thread::spawn(move || {
        let mut client = Client::start(&socket);
        prime(&mut client, &listener, &root_string);
        client.send_call(
            3,
            "au_write_file",
            json!({"repo":"fixture","path":"a.md","content":"new","expected_hash":"h1"}),
        );
        serve_members(&listener, &root_string, true, 8);
        let mut mutate = accept(&listener);
        assert_eq!(
            read_request(&mut mutate),
            json!({"mutate":"write_file","id":1,"path":format!("{root_string}/a.md"),
                "content":"new","expected_hash":"h1"})
        );
        // Drop the connection without answering: the write may have landed.
        drop(mutate);
        let reply = client.receive_id(3);
        assert!(error_text(&reply).starts_with("outcome_unknown"), "{reply}");
        let again = client.call(
            4,
            "au_write_file",
            json!({"repo":"fixture","path":"a.md","content":"new","expected_hash":"h1"}),
        );
        assert!(error_text(&again).contains("au_content"), "{again}");
    });
    driver.join().unwrap();
}

#[test]
fn an_engine_rejection_is_not_reported_as_unknown() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("ws");
    let root_string = root.to_str().unwrap().to_owned();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let driver = thread::spawn(move || {
        let mut client = Client::start(&socket);
        prime(&mut client, &listener, &root_string);
        client.send_call(
            3,
            "au_write_file",
            json!({"repo":"fixture","path":"a.md","content":"new","expected_hash":"h1"}),
        );
        serve_members(&listener, &root_string, true, 8);
        let mut mutate = accept(&listener);
        read_request(&mut mutate);
        write_frame(
            &mut mutate,
            json!({"type":"error","schema_version":29,"id":1,"for":"mutate",
                "error":"hash mismatch","detail":{"current_hash":"h9"}}),
        );
        let reply = client.receive_id(3);
        let text = error_text(&reply);
        assert!(text.contains("hash mismatch"), "{text}");
        assert!(!text.contains("outcome_unknown"), "{text}");
    });
    driver.join().unwrap();
}

#[test]
fn a_consumed_member_is_refused_before_the_mutation() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("dep");
    let root_string = root.to_str().unwrap().to_owned();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let driver = thread::spawn(move || {
        let mut client = Client::start(&socket);
        prime(&mut client, &listener, &root_string);
        client.send_call(
            3,
            "au_write_file",
            json!({"repo":"fixture","path":"a.md","content":"new","expected_hash":"h1"}),
        );
        serve_members(&listener, &root_string, false, 9);
        let reply = client.receive_id(3);
        let text = error_text(&reply);
        assert!(text.contains("not editable"), "{text}");
        listener.set_nonblocking(true).unwrap();
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock,
            "no mutation may be sent to a consumed member"
        );
    });
    driver.join().unwrap();
}
