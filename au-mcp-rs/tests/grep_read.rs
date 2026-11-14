#![cfg(unix)]

//! The `au_grep` read: a literal text search over the catalogued files.

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
        let response = client.request(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"grep-test","version":"1"}}}));
        assert!(response.get("result").is_some(), "{response}");
        writeln!(
            client.input,
            "{}",
            json!({"jsonrpc":"2.0","method":"notifications/initialized"})
        )
        .unwrap();
        client.input.flush().unwrap();
        client
    }

    fn request(&mut self, request: Value) -> Value {
        writeln!(self.input, "{request}").unwrap();
        self.input.flush().unwrap();
        self.responses
            .recv_timeout(Duration::from_secs(5))
            .expect("MCP response timeout")
    }

    fn call(&mut self, id: u32, arguments: Value) -> Value {
        self.request(json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"au_grep","arguments":arguments}}))
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

fn response(version: u64, result: Value) -> Value {
    json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":version,"result":result})
}

fn payload_of(reply: &Value) -> Value {
    assert_ne!(reply["result"]["isError"], true, "{reply}");
    serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap()
}

#[test]
fn grep_sends_the_literal_search_and_passes_the_answer_through() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let daemon = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        assert_eq!(
            read_request(&mut stream),
            json!({"read":"grep","id":1,"query":"retry policy","limit":20,"offset":0,
                "case_sensitive":true,"repo":"fixture","scope":"own"})
        );
        write_frame(
            &mut stream,
            response(
                7,
                json!({"grep":{
                    "matches":[{"path":"/tmp/ws/notes/a.md","repo":"fixture","line":5,"column":5,
                        "text":"the Retry Policy matters","text_truncated":false}],
                    "total":1,"truncated":false,"skipped_files":2}}),
            ),
        );
    });
    let mut client = Client::start(&socket);
    let reply = client.call(
        2,
        json!({"query":"retry policy","case_sensitive":true,"repo":"fixture","scope":"own"}),
    );
    let payload = payload_of(&reply);
    assert_eq!(payload["grep"]["total"], 1);
    assert_eq!(payload["grep"]["matches"][0]["line"], 5);
    assert_eq!(payload["grep"]["skipped_files"], 2);
    assert_eq!(reply["result"]["_meta"]["io.arsumbris/engineVersion"], "7");
    daemon.join().unwrap();
}

#[test]
fn grep_refuses_bad_arguments_before_the_engine() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut client = Client::start(&socket);
    for (index, arguments) in [
        json!({"query":""}),
        json!({"query":"x","repo":""}),
        json!({"query":"x","limit":0}),
        json!({"query":"x","limit":201}),
        json!({"query":"x","regex":true}),
        json!({"query":"x","case_sensitive":"yes"}),
    ]
    .into_iter()
    .enumerate()
    {
        let reply = client.call(index as u32 + 2, arguments.clone());
        assert_eq!(reply["result"]["isError"], true, "{arguments}: {reply}");
    }
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock,
        "a refused argument must not reach the engine"
    );
}
