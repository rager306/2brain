#![cfg(unix)]

//! The scan and resolution reads: `instances` (paged), `candidates`,
//! `semantic_tokens`, `resolve_member`.

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
    fn start(socket: &std::path::Path) -> Self {
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
        let init = client.request(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"scan-test","version":"1"}}}));
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
        "type":"response","schema_version":29,"id":1,"ready":true,"version":4,"result":payload
    }))
    .unwrap();
    stream
        .write_all(&(body.len() as u32).to_be_bytes())
        .unwrap();
    stream.write_all(&body).unwrap();
}

fn serve(
    answers: Vec<(Option<Value>, Value)>,
) -> (
    tempfile::TempDir,
    std::path::PathBuf,
    thread::JoinHandle<()>,
) {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let handle = thread::spawn(move || {
        for (expected, answer) in answers {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(TIMEOUT)).unwrap();
            let request = read_request(&mut stream);
            if let Some(expected) = expected {
                assert_eq!(request, expected, "wire request");
            }
            frame(&mut stream, answer);
        }
    });
    (temp, socket, handle)
}

fn payload(result: &Value) -> Value {
    serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap()
}

#[test]
fn instances_pages_the_scan_and_keeps_the_count() {
    let records: Vec<Value> = (0..5)
        .map(|i| json!({"file": format!("/f{i}.md"),"claim":["note"]}))
        .collect();
    let (temp, socket, handle) = serve(vec![(
        Some(json!({"read":"instances","id":1})),
        json!({"instances": records, "count": 5, "aborted_at_load": false}),
    )]);
    let mut client = Client::start(&socket);
    let result = client.call(2, "au_instances", json!({"limit":2,"offset":0}));
    assert_ne!(result["isError"], true, "{result}");
    let payload = payload(&result);
    assert_eq!(payload["instances"].as_array().unwrap().len(), 2);
    assert_eq!(payload["total"], 5);
    assert_eq!(payload["next_offset"], 2);
    // The engine's own envelope metadata survives the page.
    assert_eq!(payload["count"], 5);
    assert_eq!(payload["aborted_at_load"], false);
    drop(client);
    handle.join().unwrap();
    drop(temp);
}

#[test]
fn candidates_forwards_summary_and_paging() {
    let (temp, socket, handle) = serve(vec![(
        Some(json!({"read":"candidates","id":1,"summary":true,"limit":10,"offset":5})),
        json!({"candidates":[{"file":"/a.md","candidates":["note"]}], "aborted_at_load": false}),
    )]);
    let mut client = Client::start(&socket);
    let result = client.call(
        2,
        "au_candidates",
        json!({"summary":true,"limit":10,"offset":5}),
    );
    assert_ne!(result["isError"], true, "{result}");
    assert_eq!(payload(&result)["candidates"].as_array().unwrap().len(), 1);
    drop(client);
    handle.join().unwrap();
    drop(temp);
}

#[test]
fn semantic_tokens_and_resolve_member_resolve_the_member_root() {
    for (tool, verb, key, answer) in [
        (
            "au_semantic_tokens",
            "semantic_tokens",
            "semantic_tokens",
            json!({"semantic_tokens":[{"kind":"anchor","text":"Details"}]}),
        ),
        (
            "au_resolve_member",
            "resolve_member",
            "resolve_member",
            json!({"resolve_member":{"repo":"fixture","root":ROOT,"editable":true,"local":true,"role":"entry"}}),
        ),
    ] {
        let (temp, socket, handle) = serve(vec![
            (
                Some(json!({"read":"members","id":1,"repo":"fixture"})),
                json!({"members":[{"repo":"fixture","editable":true,"root":ROOT}]}),
            ),
            (
                Some(json!({"read":verb,"id":1,"path":"/tmp/fixture/notes/a.md"})),
                answer.clone(),
            ),
        ]);
        let mut client = Client::start(&socket);
        let result = client.call(2, tool, json!({"repo":"fixture","path":"notes/a.md"}));
        assert_ne!(result["isError"], true, "{tool}: {result}");
        let payload = payload(&result);
        assert!(payload.get(key).is_some(), "{tool}: {payload}");
        drop(client);
        handle.join().unwrap();
        drop(temp);
    }
}

#[test]
fn typed_classifies_an_instance_and_falls_back_to_resolve_target() {
    // An instance: one read, no fallback.
    let (temp, socket, handle) = serve(vec![
        (
            Some(json!({"read":"members","id":1,"repo":"fixture"})),
            json!({"members":[{"repo":"fixture","editable":true,"root":ROOT}]}),
        ),
        (
            Some(json!({"read":"instance","id":1,"path":"/tmp/fixture/notes/a.md"})),
            json!({"instance":{"resolved":true,"claim":["note"],"closure":["note"]}}),
        ),
    ]);
    let mut client = Client::start(&socket);
    let result = client.call(2, "au_typed", json!({"repo":"fixture","path":"notes/a.md"}));
    assert_ne!(result["isError"], true, "{result}");
    let classified = payload(&result);
    assert_eq!(classified["typed"], true);
    assert_eq!(classified["kind"], "instance");
    assert_eq!(classified["claim"], json!(["note"]));
    guard(handle, temp);

    // A plain note: `instance` is null, then the local resolve form names the kind.
    let (temp, socket, handle) = serve(vec![
        (
            Some(json!({"read":"members","id":1,"repo":"fixture"})),
            json!({"members":[{"repo":"fixture","editable":true,"root":ROOT}]}),
        ),
        (
            Some(json!({"read":"instance","id":1,"path":"/tmp/fixture/notes/plain.md"})),
            json!({"instance":null}),
        ),
        (
            Some(
                json!({"read":"resolve_target","id":1,"target":"","origin":"/tmp/fixture/notes/plain.md"}),
            ),
            json!({"resolve_target":{"path":"/tmp/fixture/notes/plain.md","kind":"unclassified"}}),
        ),
    ]);
    let mut client = Client::start(&socket);
    let result = client.call(
        2,
        "au_typed",
        json!({"repo":"fixture","path":"notes/plain.md"}),
    );
    assert_ne!(result["isError"], true, "{result}");
    let classified = payload(&result);
    assert_eq!(classified["typed"], false);
    assert_eq!(classified["kind"], "unclassified");
    guard(handle, temp);
}

#[test]
fn resolved_and_children_read_their_own_verbs() {
    let (temp, socket, handle) = serve(vec![
        (
            Some(json!({"read":"members","id":1,"repo":"fixture"})),
            json!({"members":[{"repo":"fixture","editable":true,"root":ROOT}]}),
        ),
        (
            Some(json!({"read":"instance","id":1,"path":"/tmp/fixture/notes/a.md"})),
            json!({"instance":{"resolved":true,"claim":["note"]}}),
        ),
    ]);
    let mut client = Client::start(&socket);
    let result = client.call(
        2,
        "au_resolved",
        json!({"repo":"fixture","path":"notes/a.md"}),
    );
    assert_ne!(result["isError"], true, "{result}");
    assert_eq!(payload(&result)["instance"]["claim"], json!(["note"]));
    guard(handle, temp);

    let (temp, socket, handle) = serve(vec![
        (
            Some(json!({"read":"members","id":1,"repo":"fixture"})),
            json!({"members":[{"repo":"fixture","editable":true,"root":ROOT}]}),
        ),
        (
            Some(json!({"read":"dir_entries","id":1,"dir":"/tmp/fixture/notes"})),
            json!({"dir_entries":[{"path":"/tmp/fixture/notes/a.md","name":"a.md","kind":"file"}]}),
        ),
    ]);
    let mut client = Client::start(&socket);
    let result = client.call(2, "au_children", json!({"repo":"fixture","path":"notes"}));
    assert_ne!(result["isError"], true, "{result}");
    assert_eq!(payload(&result)["dir_entries"].as_array().unwrap().len(), 1);
    guard(handle, temp);
}

fn guard(handle: thread::JoinHandle<()>, temp: tempfile::TempDir) {
    handle.join().unwrap();
    drop(temp);
}
