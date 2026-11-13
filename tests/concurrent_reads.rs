#![cfg(unix)]

use std::{
    io::{BufRead, BufReader, Read, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::Path,
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, Instant},
};

use serde_json::{Value, json};

const TIMEOUT: Duration = Duration::from_secs(5);
const VERSION_KEY: &str = "io.arsumbris/engineVersion";

struct Client {
    child: Child,
    input: ChildStdin,
    responses: Receiver<Value>,
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
                let line = line.expect("read MCP stdout");
                if tx
                    .send(serde_json::from_str(&line).expect("MCP JSON"))
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
        client.send(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"concurrent-test","version":"1"}}}));
        let init = client.receive();
        assert_eq!(init["id"], 1, "{init}");
        assert!(init.get("result").is_some(), "{init}");
        client.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
        client
    }

    fn send(&mut self, request: Value) {
        writeln!(self.input, "{request}").unwrap();
        self.input.flush().unwrap();
    }

    fn call(&mut self, id: u32, offset: u32) {
        self.send(json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"au_types","arguments":{"limit":1,"offset":offset}}}));
    }

    fn receive(&self) -> Value {
        self.responses
            .recv_timeout(TIMEOUT)
            .expect("MCP response timeout")
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn accept(listener: &UnixListener, offset: u32) -> UnixStream {
    let deadline = Instant::now() + TIMEOUT;
    let mut stream = loop {
        match listener.accept() {
            Ok((stream, _)) => break stream,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(
                    Instant::now() < deadline,
                    "engine accept timeout for offset {offset}"
                );
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("engine accept failed: {error}"),
        }
    };
    stream.set_read_timeout(Some(TIMEOUT)).unwrap();
    stream.set_write_timeout(Some(TIMEOUT)).unwrap();
    let mut len = [0; 4];
    stream.read_exact(&mut len).unwrap();
    let mut body = vec![0; u32::from_be_bytes(len) as usize];
    stream.read_exact(&mut body).unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap(),
        json!({"read":"types","id":1,"summary":true,"limit":1,"offset":offset})
    );
    stream
}

fn respond(stream: &mut UnixStream, value: Value) {
    let body = serde_json::to_vec(&value).unwrap();
    stream
        .write_all(&(body.len() as u32).to_be_bytes())
        .unwrap();
    stream.write_all(&body).unwrap();
}

fn success(version: u64, name: &str) -> Value {
    json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":version,"result":{"types":[{"name":name}]}})
}

fn assert_success(response: &Value, id: u32, version: u64, name: &str) {
    assert_eq!(response["id"], id, "{response}");
    assert!(response.get("error").is_none(), "{response}");
    assert_ne!(response["result"]["isError"], true, "{response}");
    assert_eq!(
        response["result"]["_meta"][VERSION_KEY],
        version.to_string(),
        "{response}"
    );
    assert_eq!(
        response["result"]["content"][0]["type"], "text",
        "{response}"
    );
    let text = response["result"]["content"][0]["text"].as_str().unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(text).unwrap(),
        json!({"types":[{"name":name}]})
    );
}

fn exercise_concurrent_reads(first_fails: bool) {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let (first_seen_tx, first_seen_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let daemon = thread::spawn(move || {
        let mut first = accept(&listener, 10);
        first_seen_tx.send(()).unwrap();
        let mut second = accept(&listener, 20);
        respond(&mut second, success(202, "second"));
        release_rx
            .recv_timeout(TIMEOUT)
            .expect("release first response timeout");
        if first_fails {
            respond(
                &mut first,
                json!({"type":"error","schema_version":29,"id":1,"for":"read","error":"isolated failure"}),
            );
        } else {
            respond(&mut first, success(101, "first"));
        }
        let mut recovery = accept(&listener, 30);
        respond(&mut recovery, success(303, "recovered"));
    });
    let mut client = Client::start(&socket);
    client.call(11, 10);
    first_seen_rx
        .recv_timeout(TIMEOUT)
        .expect("first request timeout");
    client.call(22, 20);
    // The first response is withheld until the second MCP response has arrived.
    assert_success(&client.receive(), 22, 202, "second");
    release_tx.send(()).unwrap();
    let first = client.receive();
    if first_fails {
        assert_eq!(first["id"], 11, "{first}");
        assert_eq!(first["result"]["isError"], true, "{first}");
        assert!(
            first["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("isolated failure"),
            "{first}"
        );
        assert!(
            first["result"]
                .get("_meta")
                .and_then(|meta| meta.get(VERSION_KEY))
                .is_none(),
            "{first}"
        );
    } else {
        assert_success(&first, 11, 101, "first");
    }
    client.call(33, 30);
    assert_success(&client.receive(), 33, 303, "recovered");
    daemon.join().unwrap();
}

#[test]
fn overlapping_reads_keep_their_payload_and_version() {
    exercise_concurrent_reads(false);
}

#[test]
fn overlapping_error_is_isolated_and_server_recovers() {
    exercise_concurrent_reads(true);
}
