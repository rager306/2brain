#![cfg(unix)]

//! Structural mutations: assign_block_id, rename_block_id, rename. Each goes
//! through the same gate as the write tools and sends the engine's mutate frame.

use std::{
    io::{BufRead, BufReader, Read, Write},
    os::unix::net::UnixListener,
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver},
    thread,
    time::Duration,
};

use serde_json::{Value, json};

const TIMEOUT: Duration = Duration::from_secs(10);
const ROOT: &str = "/tmp/fixture";

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
    fn start(socket: &std::path::Path, extra: &[&str]) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_au-mcp-rs"))
            .arg(socket)
            .args(extra)
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
                let Ok(line) = line else { break };
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
        let init = client.request(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"structural","version":"1"}}}));
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
        self.request(json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":name,"arguments":arguments}}))["result"].clone()
    }
}

fn read_request(stream: &mut impl Read) -> Value {
    let mut len = [0; 4];
    stream.read_exact(&mut len).unwrap();
    let mut body = vec![0; u32::from_be_bytes(len) as usize];
    stream.read_exact(&mut body).unwrap();
    serde_json::from_slice(&body).unwrap()
}

fn frame(stream: &mut impl Write, payload: Value) {
    let body = serde_json::to_vec(&json!({
        "type":"response","schema_version":29,"id":1,"ready":true,"version":11,"result":payload
    }))
    .unwrap();
    stream
        .write_all(&(body.len() as u32).to_be_bytes())
        .unwrap();
    stream.write_all(&body).unwrap();
}

/// Serve `members`, the arming `content` read, the gate's `members` read, then
/// the mutation, asserting the exact mutate frame.
fn serve_mutation(
    expected_mutation: Value,
    result: Value,
) -> (
    tempfile::TempDir,
    std::path::PathBuf,
    thread::JoinHandle<()>,
) {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let handle = thread::spawn(move || {
        for verb in ["members", "content", "members"] {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(TIMEOUT)).unwrap();
            let request = read_request(&mut stream);
            assert_eq!(request["read"], verb, "{request}");
            match verb {
                "members" => frame(
                    &mut stream,
                    json!({"members":[{"repo":"fixture","editable":true,"root":ROOT}]}),
                ),
                _ => frame(
                    &mut stream,
                    json!({"content":{"text":"x","hash":"hash-one","commit":null}}),
                ),
            }
        }
        let (mut stream, _) = listener.accept().unwrap();
        stream.set_read_timeout(Some(TIMEOUT)).unwrap();
        let request = read_request(&mut stream);
        assert_eq!(request, expected_mutation, "mutate frame");
        frame(&mut stream, result);
    });
    (temp, socket, handle)
}

fn payload(result: &Value) -> Value {
    serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap()
}

#[test]
fn assign_block_id_sends_at_and_returns_the_ref() {
    let (temp, socket, handle) = serve_mutation(
        json!({"mutate":"assign_block_id","id":1,"path":"/tmp/fixture/notes/a.md","at":84}),
        json!({"path":"/tmp/fixture/notes/a.md","hash":"hash-two","id":"b-1","ref":"[[a^b-1]]"}),
    );
    let mut client = Client::start(&socket, &["--allow-mutations"]);
    let read = client.call(
        2,
        "au_content",
        json!({"repo":"fixture","path":"notes/a.md"}),
    );
    assert_ne!(read["isError"], true, "{read}");
    let result = client.call(
        3,
        "au_assign_block_id",
        json!({"repo":"fixture","path":"notes/a.md","at":84,"expected_hash":"hash-one"}),
    );
    assert_ne!(result["isError"], true, "{result}");
    assert_eq!(payload(&result)["id"], "b-1");
    drop(client);
    handle.join().unwrap();
    drop(temp);
}

#[test]
fn rename_block_id_sends_both_ids() {
    let (temp, socket, handle) = serve_mutation(
        json!({"mutate":"rename_block_id","id":1,"path":"/tmp/fixture/notes/a.md",
               "block_id":"old","to_block_id":"new"}),
        json!({"path":"/tmp/fixture/notes/a.md","hash":"hash-two"}),
    );
    let mut client = Client::start(&socket, &["--allow-mutations"]);
    let _ = client.call(
        2,
        "au_content",
        json!({"repo":"fixture","path":"notes/a.md"}),
    );
    let result = client.call(
        3,
        "au_rename_block_id",
        json!({"repo":"fixture","path":"notes/a.md","block_id":"old","to_block_id":"new","expected_hash":"hash-one"}),
    );
    assert_ne!(result["isError"], true, "{result}");
    drop(client);
    handle.join().unwrap();
    drop(temp);
}

#[test]
fn rename_sends_the_target_as_an_absolute_path_in_the_same_member() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let handle = thread::spawn(move || {
        // The `to` resolution reads members first, then the gate runs.
        for verb in ["members", "content", "members", "members"] {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(TIMEOUT)).unwrap();
            assert_eq!(read_request(&mut stream)["read"], verb);
            match verb {
                "members" => frame(
                    &mut stream,
                    json!({"members":[{"repo":"fixture","editable":true,"root":ROOT}]}),
                ),
                _ => frame(
                    &mut stream,
                    json!({"content":{"text":"x","hash":"hash-one","commit":null}}),
                ),
            }
        }
        let (mut stream, _) = listener.accept().unwrap();
        let request = read_request(&mut stream);
        assert_eq!(
            request,
            json!({"mutate":"rename","id":1,"path":"/tmp/fixture/notes/a.md","to":"/tmp/fixture/notes/b.md"})
        );
        frame(
            &mut stream,
            json!({"path":"/tmp/fixture/notes/b.md","hash":"hash-two"}),
        );
    });
    let mut client = Client::start(&socket, &["--allow-mutations"]);
    let _ = client.call(
        2,
        "au_content",
        json!({"repo":"fixture","path":"notes/a.md"}),
    );
    let result = client.call(
        3,
        "au_rename",
        json!({"repo":"fixture","path":"notes/a.md","to":"notes/b.md","expected_hash":"hash-one"}),
    );
    assert_ne!(result["isError"], true, "{result}");
    drop(client);
    handle.join().unwrap();
    drop(temp);
}

#[test]
fn a_read_only_launch_hides_and_refuses_the_structural_mutations() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    UnixListener::bind(&socket).unwrap();
    let mut client = Client::start(&socket, &[]);
    let tools =
        client.request(json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}))["result"]
            ["tools"]
            .clone();
    for name in ["au_assign_block_id", "au_rename_block_id", "au_rename"] {
        assert!(
            !tools
                .as_array()
                .unwrap()
                .iter()
                .any(|tool| tool["name"] == name),
            "{name} is advertised without a grant"
        );
        let refused = client.request(json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":name,"arguments":{}}}));
        assert!(refused.get("error").is_some(), "{name}: {refused}");
    }
}
