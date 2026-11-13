#![cfg(unix)]

//! Transport faults that a real engine or a real host can produce: the engine
//! dies after a mutation request is sent, dies during a read, or writes a
//! truncated length header. Each case asserts the answer the caller gets and
//! that the server keeps serving afterwards.

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

const TIMEOUT: Duration = Duration::from_secs(15);

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
        let init = client.request(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"fault-test","version":"1"}}}));
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

fn text(reply: &Value) -> String {
    reply["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("no text content: {reply}"))
        .to_owned()
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

fn reply(result: Value) -> Value {
    json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":7,"result":result})
}

#[test]
fn a_mutation_whose_reply_never_arrives_is_unknown_and_cannot_be_replayed_blindly() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let root = temp.path().to_path_buf();
    let daemon = thread::spawn(move || {
        // Each engine exchange is its own connection: the arming read resolves
        // the member root and reads the file, and the gate resolves the member
        // again before the mutation.
        for _ in 0..3 {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(TIMEOUT)).unwrap();
            let request = read_request(&mut stream);
            match request["read"].as_str() {
                Some("members") => write_frame(
                    &mut stream,
                    reply(json!({"members":[{"repo":"fixture","editable":true,"root":root}]})),
                ),
                Some("content") => write_frame(
                    &mut stream,
                    reply(json!({"content":{"text":"---\ntype: note\n---\n","hash":"hash-one"}})),
                ),
                other => panic!("unexpected read {other:?}: {request}"),
            }
        }
        let (mut stream, _) = listener.accept().unwrap();
        stream.set_read_timeout(Some(TIMEOUT)).unwrap();
        let mutation = read_request(&mut stream);
        assert_eq!(mutation["mutate"], "write_file", "{mutation}");
        // The engine dies after accepting the write: the caller cannot know
        // whether it landed.
        drop(stream);
    });

    let mut client = Client::start(&socket);
    let read = client.call(2, "au_content", json!({"repo":"fixture","path":"a.md"}));
    assert_ne!(read["result"]["isError"], true, "{read}");

    let write = client.call(
        3,
        "au_write_file",
        json!({"repo":"fixture","path":"a.md","content":"x","expected_hash":"hash-one"}),
    );
    assert_eq!(write["result"]["isError"], true, "{write}");
    let error = text(&write);
    assert!(error.starts_with("outcome_unknown:"), "{error}");
    assert!(error.contains("read the file again"), "{error}");

    // The recorded read is gone, so a blind replay is refused locally instead
    // of reaching the engine a second time.
    let replay = client.call(
        4,
        "au_write_file",
        json!({"repo":"fixture","path":"a.md","content":"x","expected_hash":"hash-one"}),
    );
    assert_eq!(replay["result"]["isError"], true, "{replay}");
    assert_eq!(
        text(&replay),
        "policy/prior_read: read this file with au_content before writing it"
    );

    daemon.join().unwrap();
}

#[test]
fn an_engine_that_dies_during_a_read_is_an_error_and_the_server_recovers() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let daemon = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream.set_read_timeout(Some(TIMEOUT)).unwrap();
        assert_eq!(read_request(&mut stream)["read"], "overview");
        drop(stream);
        // The next connection answers normally, so the failure was a transport
        // fault rather than a wedged server.
        let (mut stream, _) = listener.accept().unwrap();
        stream.set_read_timeout(Some(TIMEOUT)).unwrap();
        assert_eq!(read_request(&mut stream)["read"], "overview");
        write_frame(&mut stream, reply(json!({"overview":{}})));
    });

    let mut client = Client::start(&socket);
    let first = client.call(2, "au_overview", json!({}));
    assert_eq!(first["result"]["isError"], true, "{first}");
    let error = text(&first);
    assert!(!error.is_empty(), "{first}");
    assert!(!error.contains("outcome_unknown"), "{error}");

    let second = client.call(3, "au_overview", json!({}));
    assert_ne!(second["result"]["isError"], true, "{second}");
    assert_eq!(second["result"]["_meta"]["io.arsumbris/engineVersion"], "7");
    daemon.join().unwrap();
}

#[test]
fn a_truncated_length_header_is_an_error_not_a_hang() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let daemon = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream.set_read_timeout(Some(TIMEOUT)).unwrap();
        assert_eq!(read_request(&mut stream)["read"], "types");
        // Two bytes of a four-byte length, then EOF.
        stream.write_all(&[0, 0]).unwrap();
        drop(stream);
    });

    let mut client = Client::start(&socket);
    let reply = client.call(2, "au_types", json!({}));
    assert_eq!(reply["result"]["isError"], true, "{reply}");
    let error = text(&reply);
    assert!(
        error.contains("length") || error.contains("frame"),
        "{error}"
    );
    daemon.join().unwrap();
}
