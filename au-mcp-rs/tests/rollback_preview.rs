#![cfg(unix)]

//! `au_rollback_preview`: the engine's plan passed through, nothing written.

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
        let init = client.request(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"rollback-test","version":"1"}}}));
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
            .recv_timeout(Duration::from_secs(5))
            .expect("MCP response timeout")
    }

    fn call(&mut self, id: u32, arguments: Value) -> Value {
        let response = self.request(json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"au_rollback_preview","arguments":arguments}}));
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

#[test]
fn rollback_preview_resolves_the_member_and_passes_the_plan_through() {
    let temp = tempfile::tempdir().unwrap();
    let root = "/tmp/ws";
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let daemon = thread::spawn(move || {
        // The member table is held state, so the read is one frame, not two.
        let (mut read, _) = listener.accept().unwrap();
        assert_eq!(
            read_request(&mut read),
            json!({"read":"rollback_preview","id":1,"repo":"fixture","commit":"abc123"})
        );
        write_frame(
            &mut read,
            json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":3,
                "result":{"rollback_preview":{
                    "repo":root,"commit":"abc123","subject":"write_file notes/a.md",
                    "parent":"def456",
                    "paths":[{"path":"/tmp/ws/notes/a.md","action":"restore","state":"clean"}],
                    "safe":true}}}),
        );
    });
    let mut client = Client::start(&socket);
    let reply = client.call(2, json!({"repo":"fixture","commit":"abc123"}));
    assert_ne!(reply["result"]["isError"], true, "{reply}");
    let payload: Value =
        serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(payload["rollback_preview"]["safe"], true);
    assert_eq!(payload["rollback_preview"]["paths"][0]["action"], "restore");
    daemon.join().unwrap();
}

#[test]
fn rollback_preview_refuses_empty_arguments_before_the_engine() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut client = Client::start(&socket);
    for (index, arguments) in [
        json!({"repo": "", "commit": "abc"}),
        json!({"repo": "fixture", "commit": ""}),
        json!({"repo": "fixture"}),
        json!({"repo": "fixture", "commit": "abc", "depth": 1}),
    ]
    .into_iter()
    .enumerate()
    {
        let reply = client.call(index as u32 + 2, arguments.clone());
        assert_eq!(reply["result"]["isError"], true, "{arguments}: {reply}");
    }
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}
