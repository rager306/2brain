#![cfg(unix)]

//! Replay the production-captured payloads from `au-engine/tests-golden`
//! through the MCP layer.
//!
//! The fixtures are tool payloads captured on the live stack before any fork
//! change. This test does not re-capture them: it proves the adapter passes the
//! same data through unchanged, so a silent normalisation (re-sorting, dropping
//! fields, renaming keys) fails here instead of at a client.

use std::{
    io::{BufRead, BufReader, Read, Write},
    os::unix::net::UnixListener,
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver},
    thread,
    time::Duration,
};

use serde_json::{Value, json};

const TIMEOUT: Duration = Duration::from_secs(10);

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../au-engine/tests-golden")
        .canonicalize()
        .expect("the golden fixtures must exist next to au-mcp-rs")
}

fn golden(name: &str) -> Value {
    let path = golden_dir().join(name);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("reading {}: {error}", path.display()));
    serde_json::from_str(&text)
        .unwrap_or_else(|error| panic!("parsing {}: {error}", path.display()))
}

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
        let init = client.request(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"golden-test","version":"1"}}}));
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

fn run_case(tool: &str, request: Value, result_payload: Value, expected: Value) {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let daemon = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream.set_read_timeout(Some(TIMEOUT)).unwrap();
        assert_eq!(read_request(&mut stream), request);
        write_frame(
            &mut stream,
            json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":42,
                "result": result_payload}),
        );
    });
    let mut client = Client::start(&socket);
    let reply = client.call(2, tool, json!({}));
    assert_ne!(reply["result"]["isError"], true, "{reply}");
    let payload: Value =
        serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(payload, expected, "{tool} altered the captured payload");
    assert_eq!(reply["result"]["_meta"]["io.arsumbris/engineVersion"], "42");
    daemon.join().unwrap();
}

#[test]
fn the_adapter_passes_captured_production_payloads_through_unchanged() {
    let overview = golden("au_overview.json");
    run_case(
        "au_overview",
        json!({"read":"overview","id":1}),
        json!({"overview": overview.clone()}),
        json!({"overview": overview}),
    );

    let hubs = golden("au_hubs.json");
    run_case(
        "au_hubs",
        json!({"read":"hubs","id":1,"limit":20,"offset":0}),
        json!({"hubs": hubs.clone()}),
        json!({"hubs": hubs}),
    );

    // The diagnostics fixture is the bridge's envelope; the engine serves the
    // same array under the read's own key.
    let diagnostics = golden("au_diagnostics.json");
    let array = diagnostics["diagnostics"].clone();
    assert!(array.is_array(), "{diagnostics}");
    run_case(
        "au_diagnostics",
        json!({"read":"diagnostics","id":1,"limit":50,"offset":0}),
        json!({"diagnostics": array.clone()}),
        json!({"diagnostics": array}),
    );
}
