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
                    .send(serde_json::from_str(&line).expect("stdout JSON"))
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
        let init = client.request(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"reconcile-test","version":"1"}}}));
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
}

fn accept(listener: &UnixListener) -> std::os::unix::net::UnixStream {
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

/// Answer the `members` + `content` pair a reconcile performs.
fn serve_content(listener: &UnixListener, root: &str, content: Value) {
    let mut members = accept(listener);
    assert_eq!(
        read_request(&mut members),
        json!({"read":"members","id":1,"repo":"fixture"})
    );
    write_frame(
        &mut members,
        json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":5,
            "result":{"members":[{"repo":"fixture","root":root,"editable":true,"role":"entry"}]}}),
    );
    let mut body = accept(listener);
    assert_eq!(
        read_request(&mut body),
        json!({"read":"content","id":1,"path":format!("{root}/a.md")})
    );
    write_frame(
        &mut body,
        json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":6,
            "result":{"content": content}}),
    );
}

fn error_text(response: &Value) -> String {
    assert_eq!(response["result"]["isError"], true, "{response}");
    response["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[test]
fn reconcile_requires_exactly_one_expectation() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut client = Client::start(&socket);
    for (i, args) in [
        json!({"repo":"fixture","path":"a.md"}),
        json!({"repo":"fixture","path":"a.md","expected_hash":"h","expect_absent":true}),
        json!({"repo":"fixture","path":"a.md","expected_hash":""}),
        json!({"repo":"","path":"a.md","expect_absent":true}),
        json!({"repo":"fixture","path":"","expect_absent":true}),
        json!({"repo":"fixture","path":"../a.md","expect_absent":true}),
        json!({"repo":"fixture","path":"a.md","expect_absent":true,"extra":1}),
    ]
    .into_iter()
    .enumerate()
    {
        assert!(!error_text(&client.call(i as u32 + 2, "au_reconcile", args)).is_empty());
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }
}

#[test]
fn reconcile_reports_applied_when_the_hash_matches() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("ws");
    let root_string = root.to_str().unwrap().to_owned();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let daemon = thread::spawn(move || {
        serve_content(
            &listener,
            &root_string,
            json!({"text":"new","hash":"h-new","commit":null}),
        );
    });
    let mut client = Client::start(&socket);
    let reply = client.call(
        2,
        "au_reconcile",
        json!({"repo":"fixture","path":"a.md","expected_hash":"h-new"}),
    );
    assert_ne!(reply["result"]["isError"], true, "{reply}");
    let payload: Value =
        serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(payload["outcome"], "applied");
    assert_eq!(payload["current_hash"], "h-new");
    daemon.join().unwrap();
}

#[test]
fn a_differing_file_reports_not_applied_and_rearms_the_guard() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("ws");
    let root_string = root.to_str().unwrap().to_owned();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let daemon = thread::spawn(move || {
        serve_content(
            &listener,
            &root_string,
            json!({"text":"other","hash":"h-actual","commit":"abc"}),
        );
        // The reconcile re-armed the guard, so a write matching the current hash
        // proceeds without another read.
        let mut members = accept(&listener);
        assert_eq!(
            read_request(&mut members),
            json!({"read":"members","id":1,"repo":"fixture"})
        );
        write_frame(
            &mut members,
            json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":7,
                "result":{"members":[{"repo":"fixture","root":root_string,"editable":true,"role":"entry"}]}}),
        );
        let mut mutate = accept(&listener);
        assert_eq!(
            read_request(&mut mutate),
            json!({"mutate":"write_file","id":1,"path":format!("{root_string}/a.md"),
                "content":"new","expected_hash":"h-actual"})
        );
        write_frame(
            &mut mutate,
            json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":8,
                "result":{"path":format!("{root_string}/a.md"),"hash":"h-new","diagnostics":[],
                    "reflected":true,"commit":null,"commits":{},"untracked_files":[]}}),
        );
    });
    let mut client = Client::start(&socket);
    let reply = client.call(
        2,
        "au_reconcile",
        json!({"repo":"fixture","path":"a.md","expected_hash":"h-intended"}),
    );
    assert_ne!(reply["result"]["isError"], true, "{reply}");
    let payload: Value =
        serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(payload["outcome"], "not_applied");
    assert_eq!(payload["current_hash"], "h-actual");
    let write = client.call(
        3,
        "au_write_file",
        json!({"repo":"fixture","path":"a.md","content":"new","expected_hash":"h-actual"}),
    );
    assert_ne!(write["result"]["isError"], true, "{write}");
    daemon.join().unwrap();
}

#[test]
fn reconcile_handles_a_delete_two_ways() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("ws");
    let root_string = root.to_str().unwrap().to_owned();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let daemon = thread::spawn(move || {
        serve_content(&listener, &root_string, Value::Null);
    });
    let mut client = Client::start(&socket);
    let gone = client.call(
        2,
        "au_reconcile",
        json!({"repo":"fixture","path":"a.md","expect_absent":true}),
    );
    let payload: Value =
        serde_json::from_str(gone["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(payload["outcome"], "applied");
    assert!(payload["current_hash"].is_null(), "{payload}");
    daemon.join().unwrap();

    let temp2 = tempfile::tempdir().unwrap();
    let root2 = temp2.path().join("ws");
    let root2_string = root2.to_str().unwrap().to_owned();
    let socket2 = temp2.path().join("engine.sock");
    let listener2 = UnixListener::bind(&socket2).unwrap();
    let daemon2 = thread::spawn(move || {
        serve_content(
            &listener2,
            &root2_string,
            json!({"text":"still","hash":"h1"}),
        );
    });
    let mut client2 = Client::start(&socket2);
    let present = client2.call(
        2,
        "au_reconcile",
        json!({"repo":"fixture","path":"a.md","expect_absent":true}),
    );
    let payload: Value =
        serde_json::from_str(present["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(payload["outcome"], "not_applied");
    assert_eq!(payload["current_hash"], "h1");
    daemon2.join().unwrap();
}
