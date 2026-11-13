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
        let response = client.request(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"version-metadata-test","version":"1"}}}));
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

    fn call(&mut self, id: u32, offset: u32) -> Value {
        let response = self.request(json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"au_types","arguments":{"limit":1,"offset":offset}}}));
        assert_eq!(response["id"], id, "{response}");
        response
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn fake_daemon(listener: UnixListener, cases: Vec<(u32, Value)>) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        for (offset, reply) in cases {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut len = [0; 4];
            stream.read_exact(&mut len).unwrap();
            let mut body = vec![0; u32::from_be_bytes(len) as usize];
            stream.read_exact(&mut body).unwrap();
            let request: Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(
                request,
                json!({"read":"types","id":1,"summary":true,"limit":1,"offset":offset})
            );
            let body = serde_json::to_vec(&reply).unwrap();
            stream
                .write_all(&(body.len() as u32).to_be_bytes())
                .unwrap();
            stream.write_all(&body).unwrap();
        }
    })
}

fn response(ready: bool, version: Value, result: Value) -> Value {
    json!({"type":"response","schema_version":29,"id":1,"ready":ready,"version":version,"result":result})
}

fn assert_success(response: &Value, version: u64, payload: &Value) {
    assert!(response.get("error").is_none(), "{response}");
    assert_ne!(response["result"]["isError"], true, "{response}");
    assert_eq!(
        response["result"]["_meta"][VERSION_KEY].as_str(),
        Some(version.to_string().as_str()),
        "{response}"
    );
    assert_eq!(
        response["result"]["content"][0]["type"], "text",
        "{response}"
    );
    assert_eq!(
        response["result"]["content"][0]["text"].as_str(),
        Some(payload.to_string().as_str()),
        "{response}"
    );
}

fn assert_error_without_version(response: &Value) {
    assert!(
        response.get("error").is_some() || response["result"]["isError"] == true,
        "{response}"
    );
    assert!(
        response
            .get("result")
            .and_then(|result| result.get("_meta"))
            .and_then(|meta| meta.get(VERSION_KEY))
            .is_none(),
        "{response}"
    );
}

#[test]
fn successive_pages_expose_each_response_version_without_changing_text() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let pages = [
        (
            42_u64,
            json!({"types":[{"name":"first"}],"extra":{"next":1}}),
        ),
        (42, json!({"types":[{"name":"second"}],"extra":{"next":2}})),
        (
            43,
            json!({"types":[{"name":"third"}],"extra":{"next":null}}),
        ),
        (u64::MAX, json!({"types":[]})),
    ];
    let daemon = fake_daemon(
        listener,
        pages
            .iter()
            .enumerate()
            .map(|(offset, (version, payload))| {
                (
                    offset as u32,
                    response(true, json!(version), payload.clone()),
                )
            })
            .collect(),
    );
    let mut client = Client::start(&socket);
    for (offset, (version, payload)) in pages.iter().enumerate() {
        assert_success(
            &client.call(offset as u32 + 2, offset as u32),
            *version,
            payload,
        );
    }
    daemon.join().unwrap();
}

#[test]
fn failed_responses_do_not_invent_version_metadata_and_success_recovers() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let payload = json!({"types":[{"name":"recovered"}]});
    let replies = [
        response(true, json!(123), json!({"types":[]})),
        json!({"type":"error","schema_version":29,"id":1,"for":"read","error":"engine failure"}),
        response(false, json!(456), json!({"types":[]})),
        response(true, json!(789), json!({"types":{}})),
        response(true, json!(null), json!({"types":[]})),
        response(true, json!(0), payload.clone()),
    ];
    let daemon = fake_daemon(
        listener,
        replies.into_iter().map(|reply| (0, reply)).collect(),
    );
    let mut client = Client::start(&socket);
    assert_success(&client.call(2, 0), 123, &json!({"types":[]}));
    for id in 3..7 {
        assert_error_without_version(&client.call(id, 0));
    }
    assert_success(&client.call(7, 0), 0, &payload);
    daemon.join().unwrap();
}
