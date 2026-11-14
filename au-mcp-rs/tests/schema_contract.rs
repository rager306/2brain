#![cfg(unix)]

//! Every captured payload must be describable by the schema its tool declares.
//!
//! A client validates `structuredContent` against `outputSchema`, so an answer
//! that carries a field the schema does not declare is a protocol error at the
//! client, not a cosmetic mismatch. This test catches that class without a JSON
//! Schema engine: for each synthetic fixture, every top-level key of the payload
//! must be declared, and every declared `required` key must be present.

use std::{
    collections::BTreeMap,
    io::{BufRead, BufReader, Write},
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
            .args([
                "--allow-mutations",
                "--allow-composition",
                "--allow-workspace-shape",
            ])
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
        let init = client.request(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"schema-test","version":"1"}}}));
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
}

fn fixture_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/synthetic-reads")
}

#[test]
fn every_captured_payload_fits_the_schema_its_tool_declares() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    // tools/list needs no engine, so a dead socket path is enough here.
    let mut client = Client::start(&socket);
    let listed = client.request(json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}));
    let mut schemas: BTreeMap<String, Value> = BTreeMap::new();
    for tool in listed["result"]["tools"].as_array().unwrap() {
        let name = tool["name"].as_str().unwrap().to_owned();
        schemas.insert(name, tool["outputSchema"].clone());
    }
    assert!(!schemas.is_empty(), "{listed}");

    let mut checked = 0;
    let mut problems: Vec<String> = Vec::new();
    for entry in std::fs::read_dir(fixture_dir()).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "json") {
            continue;
        }
        let stored: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let tool = stored["tool"].as_str().unwrap();
        let Some(schema) = schemas.get(tool) else {
            problems.push(format!("{tool}: no such advertised tool"));
            continue;
        };
        // A tool that declares no closed schema promises nothing to validate.
        if schema["additionalProperties"] != json!(false) {
            continue;
        }
        let properties = &schema["properties"];
        let payload = &stored["payload"];
        let Some(object) = payload.as_object() else {
            problems.push(format!("{tool}: payload is not an object"));
            continue;
        };
        for key in object.keys() {
            if properties.get(key).is_none() {
                problems.push(format!("{tool}: payload field `{key}` is not declared"));
            }
        }
        for required in schema["required"].as_array().into_iter().flatten() {
            let key = required.as_str().unwrap();
            if !object.contains_key(key) {
                problems.push(format!("{tool}: declared key `{key}` is missing"));
            }
        }
        // The read's own envelope key must be the one the schema requires.
        if let Some(first) = schema["required"].as_array().and_then(|r| r.first()) {
            let key = first.as_str().unwrap();
            if let Some(inner) = object.get(key)
                && inner.is_null()
                && tool != "au_content"
                && tool != "au_frontmatter"
            {
                problems.push(format!("{tool}: `{key}` is null in a captured answer"));
            }
        }
        checked += 1;
    }
    assert!(checked >= 25, "only {checked} fixtures were checked");
    assert!(
        problems.is_empty(),
        "schema drift:\n{}",
        problems.join("\n")
    );
}
