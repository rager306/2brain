#![cfg(unix)]

//! Response budgets: a text window for `au_content`, paging for
//! `au_instances_of`, the payload modes, and the argument-error hint.

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
        let init = client.request(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"budget-test","version":"1"}}}));
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

fn frame(stream: &mut impl Write, payload: Value) {
    let body = serde_json::to_vec(&payload).unwrap();
    stream
        .write_all(&(body.len() as u32).to_be_bytes())
        .unwrap();
    stream.write_all(&body).unwrap();
}

fn read_request(stream: &mut impl Read) -> Value {
    let mut len = [0; 4];
    stream.read_exact(&mut len).unwrap();
    let mut body = vec![0; u32::from_be_bytes(len) as usize];
    stream.read_exact(&mut body).unwrap();
    serde_json::from_slice(&body).unwrap()
}

/// Serve engine answers by read verb; `count` includes the startup probe.
fn daemon(
    listener: UnixListener,
    answers: Vec<(&'static str, Value)>,
    count: usize,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        for _ in 0..count {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(TIMEOUT)).unwrap();
            let request = read_request(&mut stream);
            let verb = request["read"].as_str().expect("read verb").to_owned();
            let answer = answers
                .iter()
                .find(|(name, _)| *name == verb)
                .map(|(_, payload)| payload.clone())
                .unwrap_or_else(|| panic!("unexpected engine read: {verb}"));
            frame(
                &mut stream,
                json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":5,"result":answer}),
            );
        }
    })
}

const LONG: &str = "abcdefghijklmnopqrstuvwxyz";

#[test]
fn content_returns_an_explicit_window_and_keeps_the_hash() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let handle = daemon(
        listener,
        vec![
            ("overview", json!({"overview":{}})),
            (
                "members",
                json!({"members":[{"repo":"fixture","editable":true,"root":"/tmp/fixture"}]}),
            ),
            (
                "content",
                json!({"content":{"text":LONG,"hash":"hash-full","commit":"abc"}}),
            ),
        ],
        2,
    );
    let mut client = Client::start(&socket, &[]);
    let result = client.call(
        2,
        "au_content",
        json!({"repo":"fixture","path":"a.md","max_chars":10}),
    );
    assert_ne!(result["isError"], true, "{result}");
    let payload: Value =
        serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(payload["content"]["text"], "abcdefghij");
    assert_eq!(payload["content"]["truncated"], true);
    assert_eq!(payload["content"]["total_chars"], LONG.len());
    assert_eq!(payload["content"]["next_offset"], 10);
    // The hash still covers the whole file, so a write based on it is safe.
    assert_eq!(payload["content"]["hash"], "hash-full");
    handle.join().unwrap();

    // Without max_chars the answer is the engine's, unchanged.
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let handle = daemon(
        listener,
        vec![
            ("overview", json!({"overview":{}})),
            (
                "members",
                json!({"members":[{"repo":"fixture","editable":true,"root":"/tmp/fixture"}]}),
            ),
            (
                "content",
                json!({"content":{"text":LONG,"hash":"hash-full","commit":"abc"}}),
            ),
        ],
        2,
    );
    let mut client = Client::start(&socket, &[]);
    let result = client.call(2, "au_content", json!({"repo":"fixture","path":"a.md"}));
    let payload: Value =
        serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(payload["content"]["text"], LONG);
    assert!(payload["content"].get("truncated").is_none(), "{payload}");
    handle.join().unwrap();
}

#[test]
fn instances_of_pages_and_reports_the_total_and_continuation() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let records: Vec<Value> = (0..5)
        .map(|index| json!({"path": format!("/f{index}.md")}))
        .collect();
    let handle = daemon(
        listener,
        vec![
            ("overview", json!({"overview":{}})),
            ("instances_of", json!({"instances_of": records})),
        ],
        1,
    );
    let mut client = Client::start(&socket, &[]);
    let result = client.call(
        2,
        "au_instances_of",
        json!({"type_name":"note","limit":2,"offset":0}),
    );
    let payload: Value =
        serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(payload["total"], 5);
    assert_eq!(payload["offset"], 0);
    assert_eq!(payload["next_offset"], 2);
    assert_eq!(payload["instances_of"].as_array().unwrap().len(), 2);
    handle.join().unwrap();

    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let records: Vec<Value> = (0..3)
        .map(|index| json!({"path": format!("/f{index}.md")}))
        .collect();
    let handle = daemon(
        listener,
        vec![
            ("overview", json!({"overview":{}})),
            ("instances_of", json!({"instances_of": records})),
        ],
        1,
    );
    let mut client = Client::start(&socket, &[]);
    let result = client.call(
        2,
        "au_instances_of",
        json!({"type_name":"note","limit":2,"offset":2}),
    );
    let payload: Value =
        serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(payload["next_offset"], Value::Null, "{payload}");
    assert_eq!(payload["instances_of"].as_array().unwrap().len(), 1);
    handle.join().unwrap();
}

#[test]
fn payload_modes_select_which_representation_is_returned() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let handle = daemon(listener, vec![("overview", json!({"overview":{}}))], 1);

    let mut structured = Client::start(&socket, &["--payload", "structured"]);
    let result = structured.call(2, "au_overview", json!({}));
    assert!(
        result["content"]
            .as_array()
            .is_none_or(|items| items.is_empty()),
        "{result}"
    );
    assert_eq!(result["structuredContent"], json!({"overview": {}}));
    handle.join().unwrap();

    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let handle = daemon(listener, vec![("overview", json!({"overview":{}}))], 1);
    let mut text = Client::start(&socket, &["--payload", "text"]);
    let result = text.call(2, "au_overview", json!({}));
    assert!(result.get("structuredContent").is_none(), "{result}");
    assert_eq!(
        result["content"][0]["text"],
        json!({"overview":{}}).to_string()
    );
    handle.join().unwrap();
}

#[test]
fn an_argument_error_names_the_tool_and_the_required_fields() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    UnixListener::bind(&socket).unwrap();
    let mut client = Client::start(&socket, &[]);
    let result = client.call(2, "au_type", json!({}));
    assert_eq!(result["isError"], true, "{result}");
    let text = result["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("missing field `name`"), "{text}");
    assert!(text.contains("Tool 'au_type' requires: name"), "{text}");
    assert!(text.contains("au_describe_tool"), "{text}");
}
