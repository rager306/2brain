#![cfg(unix)]

//! The composition mutations: rename_type, add_dep, remove_dep.

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
        let init = client.request(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"composition","version":"1"}}}));
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
        "type":"response","schema_version":29,"id":1,"ready":true,"version":12,"result":payload
    }))
    .unwrap();
    stream
        .write_all(&(body.len() as u32).to_be_bytes())
        .unwrap();
    stream.write_all(&body).unwrap();
}

fn payload(result: &Value) -> Value {
    serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap()
}

#[test]
fn rename_type_verifies_the_type_hash_then_mutates() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        assert_eq!(
            read_request(&mut stream),
            json!({"read":"type","id":1,"name":"note"})
        );
        frame(
            &mut stream,
            json!({"type":{"name":"note","hash":"type-hash","repo":"fixture"}}),
        );
        let (mut stream, _) = listener.accept().unwrap();
        assert_eq!(
            read_request(&mut stream),
            json!({"mutate":"rename_type","id":1,"old_name":"note","new_name":"memo"})
        );
        frame(
            &mut stream,
            json!({"path":"/tmp/fixture/type/memo.type.yaml","hash":"h2"}),
        );
    });
    let mut client = Client::start(&socket, &["--allow-mutations"]);
    let result = client.call(
        2,
        "au_rename_type",
        json!({"old_name":"note","new_name":"memo","expected_hash":"type-hash"}),
    );
    assert_ne!(result["isError"], true, "{result}");
    assert_eq!(payload(&result)["path"], "/tmp/fixture/type/memo.type.yaml");
    drop(client);
    handle.join().unwrap();
    drop(temp);
}

#[test]
fn rename_type_refuses_a_stale_hash_without_mutating() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        assert_eq!(read_request(&mut stream)["read"], "type");
        frame(
            &mut stream,
            json!({"type":{"name":"note","hash":"newer","repo":"fixture"}}),
        );
        // The refusal must be local: no second connection may arrive.
        listener.set_nonblocking(true).unwrap();
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    });
    let mut client = Client::start(&socket, &["--allow-mutations"]);
    let result = client.call(
        2,
        "au_rename_type",
        json!({"old_name":"note","new_name":"memo","expected_hash":"older"}),
    );
    assert_eq!(result["isError"], true, "{result}");
    assert!(
        result["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("does not match"),
        "{result}"
    );
    drop(client);
    handle.join().unwrap();
    drop(temp);
}

#[test]
fn add_dep_resolves_the_root_and_sends_the_entry() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        assert_eq!(read_request(&mut stream)["read"], "members");
        frame(
            &mut stream,
            json!({"members":[{"repo":"fixture","editable":true,"root":ROOT}]}),
        );
        let (mut stream, _) = listener.accept().unwrap();
        assert_eq!(
            read_request(&mut stream),
            json!({"mutate":"add_dep","id":1,"name":"peer","root":ROOT,"expected_hash":"h1"})
        );
        frame(
            &mut stream,
            json!({"path":"/tmp/fixture/.arsumbris/repo.yaml","hash":"h2"}),
        );
    });
    let mut client = Client::start(&socket, &["--allow-mutations", "--allow-composition"]);
    let result = client.call(
        2,
        "au_add_dep",
        json!({"repo":"fixture","dep":"peer","expected_hash":"h1"}),
    );
    assert_ne!(result["isError"], true, "{result}");
    drop(client);
    handle.join().unwrap();
    drop(temp);
}

#[test]
fn composition_needs_its_own_grant() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    UnixListener::bind(&socket).unwrap();
    let mut client = Client::start(&socket, &["--allow-mutations"]);
    let tools =
        client.request(json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}))["result"]
            ["tools"]
            .clone();
    for name in ["au_add_dep", "au_remove_dep"] {
        assert!(
            !tools
                .as_array()
                .unwrap()
                .iter()
                .any(|tool| tool["name"] == name),
            "{name} is advertised without the composition grant"
        );
        let refused = client.request(json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":name,"arguments":{"repo":"fixture","dep":"x"}}}));
        assert!(
            refused["error"]["message"]
                .as_str()
                .unwrap_or_default()
                .contains("composition"),
            "{refused}"
        );
    }
    // A write tool stays available with only the mutation grant.
    assert!(
        tools
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool["name"] == "au_write_file")
    );
}

#[test]
fn workspace_shape_needs_its_own_grant() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    UnixListener::bind(&socket).unwrap();
    // Mutations plus composition, but no workspace-shape grant.
    let mut client = Client::start(&socket, &["--allow-mutations", "--allow-composition"]);
    let tools =
        client.request(json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}))["result"]
            ["tools"]
            .clone();
    for name in [
        "au_add_workspace_member",
        "au_remove_workspace_member",
        "au_set_workspace_member_role",
    ] {
        assert!(
            !tools
                .as_array()
                .unwrap()
                .iter()
                .any(|tool| tool["name"] == name),
            "{name} is advertised without the workspace-shape grant"
        );
        let refused = client.request(json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":name,"arguments":{"repo":"fixture","name":"x","role":"edit"}}}));
        assert!(
            refused["error"]["message"]
                .as_str()
                .unwrap_or_default()
                .contains("workspace"),
            "{refused}"
        );
    }
    // Composition tools stay available with only that grant.
    assert!(
        tools
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool["name"] == "au_add_dep")
    );
}

#[test]
fn add_workspace_member_sends_the_role_and_root() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        assert_eq!(read_request(&mut stream)["read"], "members");
        frame(
            &mut stream,
            json!({"members":[{"repo":"fixture","editable":true,"root":ROOT}]}),
        );
        let (mut stream, _) = listener.accept().unwrap();
        assert_eq!(
            read_request(&mut stream),
            json!({"mutate":"add_workspace_member","id":1,"name":"peer","role":"discover","root":ROOT})
        );
        frame(
            &mut stream,
            json!({"path":"/tmp/fixture/.arsumbris/workspace.yaml","hash":"h2"}),
        );
    });
    let mut client = Client::start(&socket, &["--allow-mutations", "--allow-workspace-shape"]);
    let result = client.call(
        2,
        "au_add_workspace_member",
        json!({"repo":"fixture","name":"peer","role":"discover"}),
    );
    assert_ne!(result["isError"], true, "{result}");
    drop(client);
    handle.join().unwrap();
    drop(temp);
}
