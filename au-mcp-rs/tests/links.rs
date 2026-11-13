#![cfg(unix)]

//! The graph and addressability reads: `frontmatter`, `anchors`, `block_ids`,
//! `references`, `neighborhood`. Each must send the exact wire request and pass
//! the engine payload through.

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
        let init = client.request(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"links-test","version":"1"}}}));
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
        "type":"response","schema_version":29,"id":1,"ready":true,"version":9,"result":payload
    }))
    .unwrap();
    stream
        .write_all(&(body.len() as u32).to_be_bytes())
        .unwrap();
    stream.write_all(&body).unwrap();
}

/// Serve `members` first (every path-addressed read resolves the root), then the
/// expected read, asserting the exact wire request for the second.
fn serve(
    expected: Value,
    answer: Value,
) -> (
    tempfile::TempDir,
    std::path::PathBuf,
    thread::JoinHandle<()>,
) {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream.set_read_timeout(Some(TIMEOUT)).unwrap();
        let members = read_request(&mut stream);
        assert_eq!(
            members,
            json!({"read":"members","id":1,"repo":"fixture"}),
            "members first"
        );
        frame(
            &mut stream,
            json!({"members":[{"repo":"fixture","editable":true,"root":ROOT}]}),
        );
        let (mut stream, _) = listener.accept().unwrap();
        stream.set_read_timeout(Some(TIMEOUT)).unwrap();
        assert_eq!(read_request(&mut stream), expected, "wire request");
        frame(&mut stream, answer);
    });
    (temp, socket, handle)
}

fn payload(result: &Value) -> Value {
    serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap()
}

#[test]
fn frontmatter_sends_the_absolute_path_and_passes_the_map() {
    let answer = json!({"frontmatter": {"type":"note","title":"x"}});
    let (temp, socket, handle) = serve(
        json!({"read":"frontmatter","id":1,"path":"/tmp/fixture/notes/a.md"}),
        answer.clone(),
    );
    let mut client = Client::start(&socket);
    let result = client.call(
        2,
        "au_frontmatter",
        json!({"repo":"fixture","path":"notes/a.md"}),
    );
    assert_ne!(result["isError"], true, "{result}");
    assert_eq!(payload(&result), answer);
    drop(client);
    handle.join().unwrap();
    drop(temp);
}

#[test]
fn anchors_and_block_ids_address_the_target() {
    for (tool, verb, answer) in [
        (
            "au_anchors",
            "anchors",
            json!({"anchors":[{"text":"Head","level":1,"span":{"start":0,"end":6}}]}),
        ),
        (
            "au_block_ids",
            "block_ids",
            json!({"block_ids":[{"id":"intro","kind":"marker","type_claim":"","span":{"start":9,"end":15}}]}),
        ),
    ] {
        let (temp, socket, handle) = serve(
            json!({"read":verb,"id":1,"target":"","origin":"/tmp/fixture/notes/a.md"}),
            answer.clone(),
        );
        let mut client = Client::start(&socket);
        let result = client.call(2, tool, json!({"repo":"fixture","path":"notes/a.md"}));
        assert_ne!(result["isError"], true, "{tool}: {result}");
        assert_eq!(payload(&result), answer, "{tool}");
        drop(client);
        handle.join().unwrap();
        drop(temp);
    }
}

#[test]
fn references_reads_both_directions_and_keeps_the_two_keys() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let out = json!({"references_out":[{"target":"[[b]]","resolved":"/tmp/fixture/b.md","kind":"navigational"}]});
    let inb = json!({"references_in":[{"target":"[[a]]","resolved":"/tmp/fixture/a.md","kind":"navigational"}]});
    let expected_out = out.clone();
    let expected_in = inb.clone();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream.set_read_timeout(Some(TIMEOUT)).unwrap();
        assert_eq!(read_request(&mut stream)["read"], "members");
        frame(
            &mut stream,
            json!({"members":[{"repo":"fixture","editable":true,"root":ROOT}]}),
        );
        let (mut stream, _) = listener.accept().unwrap();
        assert_eq!(
            read_request(&mut stream),
            json!({"read":"references_out","id":1,"path":"/tmp/fixture/notes/a.md"})
        );
        frame(&mut stream, expected_out);
        let (mut stream, _) = listener.accept().unwrap();
        assert_eq!(
            read_request(&mut stream),
            json!({"read":"references_in","id":1,"path":"/tmp/fixture/notes/a.md"})
        );
        frame(&mut stream, expected_in);
    });
    let mut client = Client::start(&socket);
    let result = client.call(
        2,
        "au_references",
        json!({"repo":"fixture","path":"notes/a.md"}),
    );
    assert_ne!(result["isError"], true, "{result}");
    let payload = payload(&result);
    assert_eq!(
        payload,
        json!({"references_out": out["references_out"], "references_in": inb["references_in"]})
    );
    drop(client);
    handle.join().unwrap();
    drop(temp);
}

#[test]
fn references_direction_out_reads_once() {
    let (temp, socket, handle) = serve(
        json!({"read":"references_out","id":1,"path":"/tmp/fixture/notes/a.md"}),
        json!({"references_out":[]}),
    );
    let mut client = Client::start(&socket);
    let result = client.call(
        2,
        "au_references",
        json!({"repo":"fixture","path":"notes/a.md","direction":"out"}),
    );
    assert_eq!(payload(&result), json!({"references_out": []}));
    drop(client);
    handle.join().unwrap();
    drop(temp);
}

#[test]
fn neighborhood_forwards_the_walk_arguments() {
    let (temp, socket, handle) = serve(
        json!({"read":"neighborhood","id":1,"path":"/tmp/fixture/notes/a.md",
               "direction":"both","depth":2,"kinds":["navigational"],"scope":"own","max_nodes":50}),
        json!({"neighborhood":{"nodes":[],"edges":[],"truncated":false}}),
    );
    let mut client = Client::start(&socket);
    let result = client.call(
        2,
        "au_neighborhood",
        json!({"repo":"fixture","path":"notes/a.md","direction":"both","depth":2,
               "kinds":["navigational"],"scope":"own","max_nodes":50}),
    );
    assert_ne!(result["isError"], true, "{result}");
    let payload = payload(&result);
    assert_eq!(payload["neighborhood"]["truncated"], false);
    drop(client);
    handle.join().unwrap();
    drop(temp);
}

#[test]
fn the_new_reads_reject_paths_before_connecting() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut client = Client::start(&socket);
    for (index, tool) in [
        "au_frontmatter",
        "au_anchors",
        "au_block_ids",
        "au_references",
        "au_neighborhood",
    ]
    .into_iter()
    .enumerate()
    {
        for bad in ["/etc/passwd", "../x.md"] {
            let result = client.call(
                index as u32 + 10,
                tool,
                json!({"repo":"fixture","path":bad}),
            );
            assert_eq!(result["isError"], true, "{tool} {bad}: {result}");
        }
        let empty = client.call(index as u32 + 20, tool, json!({"repo":"fixture","path":""}));
        assert_eq!(empty["isError"], true, "{tool}: {empty}");
    }
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}
