#![cfg(unix)]

//! `au_watch`: one bounded wait on the engine's `changes` subscription.

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
        let init = client.request(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"watch-test","version":"1"}}}));
        assert!(init.get("result").is_some(), "{init}");
        writeln!(
            client.input,
            "{}",
            json!({"jsonrpc":"2.0","method":"notifications/initialized"})
        )
        .unwrap();
        client.input.flush().unwrap();
        client
    }

    fn request(&mut self, value: Value) -> Value {
        writeln!(self.input, "{value}").unwrap();
        self.input.flush().unwrap();
        self.responses
            .recv_timeout(Duration::from_secs(10))
            .expect("MCP response timeout")
    }

    fn call(&mut self, id: u32, arguments: Value) -> Value {
        let response = self.request(json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"au_watch","arguments":arguments}}));
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

#[test]
fn watch_subscribes_skips_the_ack_and_returns_the_delta() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let daemon = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        assert_eq!(
            read_request(&mut stream),
            json!({"subscribe":"changes","id":1})
        );
        write_frame(
            &mut stream,
            json!({"type":"ack","schema_version":29,"subscription_id":1,"channel":"changes","accepted":true,"id":1}),
        );
        write_frame(
            &mut stream,
            json!({"type":"change_event","schema_version":29,"subscription_id":1,
                "kind":"knowledge-base-changed","at_version":42,
                "scope_hint":{"scope":"files","added":["/ws/new.md"],
                    "removed":[],"modified":["/ws/notes/a.md"]}}),
        );
    });
    let mut client = Client::start(&socket);
    let reply = client.call(2, json!({}));
    assert_ne!(reply["result"]["isError"], true, "{reply}");
    let payload: Value =
        serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(payload["watch"]["modified"][0], "/ws/notes/a.md");
    assert_eq!(payload["watch"]["added"][0], "/ws/new.md");
    assert_eq!(payload["watch"]["version"], 42);
    assert_eq!(payload["watch"]["timed_out"], false);
    daemon.join().unwrap();
}

#[test]
fn a_timeout_is_an_answer_not_an_error() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let daemon = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let _ = read_request(&mut stream);
        write_frame(
            &mut stream,
            json!({"type":"ack","schema_version":29,"subscription_id":1,"channel":"changes","accepted":true,"id":1}),
        );
        // Hold the connection open with no event: the watch must time out.
        thread::sleep(Duration::from_millis(600));
    });
    let mut client = Client::start(&socket);
    let reply = client.call(2, json!({"timeout_ms": 200}));
    assert_ne!(reply["result"]["isError"], true, "{reply}");
    let payload: Value =
        serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(payload["watch"]["timed_out"], true);
    assert_eq!(payload["watch"]["added"].as_array().unwrap().len(), 0);
    daemon.join().unwrap();
}

#[test]
fn watch_refuses_an_out_of_range_timeout_before_connecting() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut client = Client::start(&socket);
    for (index, arguments) in [json!({"timeout_ms": 0}), json!({"timeout_ms": 60_000})]
        .into_iter()
        .enumerate()
    {
        let reply = client.call(index as u32 + 2, arguments.clone());
        assert_eq!(reply["result"]["isError"], true, "{arguments}: {reply}");
    }
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}
