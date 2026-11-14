#![cfg(unix)]

//! MCP resources: the catalogue as addressable URIs, and reading one back.

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
    fn start(socket: &Path) -> (Self, Value) {
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
        let init = client.request(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"resources-test","version":"1"}}}));
        assert!(init.get("result").is_some(), "{init}");
        writeln!(
            client.input,
            "{}",
            json!({"jsonrpc":"2.0","method":"notifications/initialized"})
        )
        .unwrap();
        client.input.flush().unwrap();
        (client, init)
    }

    fn request(&mut self, value: Value) -> Value {
        writeln!(self.input, "{value}").unwrap();
        self.input.flush().unwrap();
        self.responses
            .recv_timeout(Duration::from_secs(5))
            .expect("MCP response timeout")
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

fn members_frame(root: &str) -> Value {
    json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":3,
        "result":{"members":[{"repo":"fixture","root":root,"editable":true,"role":"entry"}]}})
}

#[test]
fn initialize_advertises_resources_without_subscription() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    UnixListener::bind(&socket).unwrap();
    let (client, init) = Client::start(&socket);
    let capabilities = &init["result"]["capabilities"];
    assert!(capabilities["tools"].is_object(), "{init}");
    assert!(capabilities["resources"].is_object(), "{init}");
    // Absent and false mean the same thing on the wire; what must never happen
    // is claiming a subscription this server does not serve.
    assert!(
        !capabilities["resources"]["subscribe"]
            .as_bool()
            .unwrap_or(false)
    );
    drop(client);
}

#[test]
fn list_resources_returns_member_scoped_uris() {
    let temp = tempfile::tempdir().unwrap();
    let root = "/tmp/ws";
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let daemon = thread::spawn(move || {
        let (mut files, _) = listener.accept().unwrap();
        assert_eq!(
            read_request(&mut files),
            json!({"read":"files","id":1,"limit":100,"offset":0})
        );
        write_frame(
            &mut files,
            json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":4,
                "result":{"files":[
                    {"path":"/tmp/ws/notes/a.md","stem":"a","repo":"fixture","kind":"note"},
                    {"path":"/tmp/ws/notes/b c.md","stem":"b c","repo":"fixture","kind":"note"}]}}),
        );
        let (mut members, _) = listener.accept().unwrap();
        assert_eq!(read_request(&mut members), json!({"read":"members","id":1}));
        write_frame(&mut members, members_frame(root));
    });
    let (mut client, _) = Client::start(&socket);
    let reply =
        client.request(json!({"jsonrpc":"2.0","id":2,"method":"resources/list","params":{}}));
    let resources = reply["result"]["resources"].as_array().unwrap();
    assert_eq!(resources.len(), 2, "{reply}");
    assert_eq!(resources[0]["uri"], "arsumbris://fixture/notes/a.md");
    assert_eq!(resources[0]["mimeType"], "text/markdown");
    assert_eq!(
        resources[1]["uri"], "arsumbris://fixture/notes/b%20c.md",
        "a space in a file name must not break the URI"
    );
    daemon.join().unwrap();
}

#[test]
fn read_resource_returns_the_engine_text() {
    let temp = tempfile::tempdir().unwrap();
    let root = "/tmp/ws";
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let daemon = thread::spawn(move || {
        let (mut members, _) = listener.accept().unwrap();
        assert_eq!(
            read_request(&mut members),
            json!({"read":"members","id":1,"repo":"fixture"})
        );
        write_frame(&mut members, members_frame(root));
        let (mut content, _) = listener.accept().unwrap();
        assert_eq!(
            read_request(&mut content),
            json!({"read":"content","id":1,"path":"/tmp/ws/notes/a.md"})
        );
        write_frame(
            &mut content,
            json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":5,
                "result":{"content":{"text":"# a\n","hash":"h1","commit":null}}}),
        );
    });
    let (mut client, _) = Client::start(&socket);
    let reply = client.request(json!({
        "jsonrpc":"2.0","id":2,"method":"resources/read",
        "params":{"uri":"arsumbris://fixture/notes/a.md"}}));
    let contents = reply["result"]["contents"].as_array().unwrap();
    assert_eq!(contents[0]["text"], "# a\n", "{reply}");
    assert_eq!(contents[0]["mimeType"], "text/markdown");
    daemon.join().unwrap();
}

#[test]
fn a_foreign_uri_is_refused_before_the_engine() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let (mut client, _) = Client::start(&socket);
    let reply = client.request(json!({
        "jsonrpc":"2.0","id":2,"method":"resources/read",
        "params":{"uri":"file:///etc/passwd"}}));
    assert!(reply["error"].is_object(), "{reply}");
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}
