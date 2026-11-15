//! Transport harness shared by the adapter's integration tests.
//!
//! Every test file is its own crate, so this module is included with
//! `mod support;`. It holds only the plumbing the tests share: the stdio
//! client that spawns the adapter and speaks MCP over it, and the engine-frame
//! helpers the fake daemons use. Scenarios, fake-daemon closures, and per-tool
//! assertions stay in the test files.

#![allow(dead_code)]

use std::{
    io::{BufRead, BufReader, Read, Write},
    path::Path,
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver, RecvTimeoutError},
    thread,
    time::Duration,
};

use serde_json::{Value, json};

/// How long a client waits for one reply unless the test asks for another wait.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

/// The protocol version the legacy handshake asks for.
const PROTOCOL_VERSION: &str = "2025-06-18";

/// A running adapter process spoken to over stdio.
///
/// Dropping it kills the child, which is what ends every test's server.
pub struct Client {
    child: Child,
    input: ChildStdin,
    responses: Receiver<Value>,
    stderr: Option<Receiver<String>>,
    timeout: Duration,
}

impl Client {
    /// Spawns the adapter on `socket` without any handshake; the caller drives
    /// the conversation, including `initialize`.
    #[track_caller]
    pub fn spawn(socket: &Path) -> Self {
        Self::spawn_configured(socket, &[], DEFAULT_TIMEOUT, false)
    }

    /// Spawns the adapter and completes the MCP initialize handshake.
    #[track_caller]
    pub fn start(socket: &Path) -> Self {
        Self::start_with(socket, &[])
    }

    /// `start` with extra command-line arguments after the socket.
    #[track_caller]
    pub fn start_with(socket: &Path, extra: &[&str]) -> Self {
        Self::start_configured(socket, extra, DEFAULT_TIMEOUT, false)
    }

    /// `start_with` with a per-reply timeout other than the default.
    #[track_caller]
    pub fn start_with_timeout(socket: &Path, extra: &[&str], timeout: Duration) -> Self {
        Self::start_configured(socket, extra, timeout, false)
    }

    /// `start_with`, keeping the adapter's stderr for audit assertions.
    #[track_caller]
    pub fn start_capturing_stderr(socket: &Path, extra: &[&str]) -> Self {
        Self::start_configured(socket, extra, DEFAULT_TIMEOUT, true)
    }

    #[track_caller]
    fn start_configured(
        socket: &Path,
        extra: &[&str],
        timeout: Duration,
        capture_stderr: bool,
    ) -> Self {
        let mut client = Self::spawn_configured(socket, extra, timeout, capture_stderr);
        client.initialize();
        client
    }

    #[track_caller]
    fn spawn_configured(
        socket: &Path,
        extra: &[&str],
        timeout: Duration,
        capture_stderr: bool,
    ) -> Self {
        // One binary for the product: `2brain mcp <target> …` (there is no separate server binary).
        let mut command = Command::new(env!("CARGO_BIN_EXE_2brain"));
        command.arg("mcp");
        command
            .arg(socket)
            .args(extra)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped());
        if capture_stderr {
            command.stderr(Stdio::piped());
        } else {
            command.stderr(Stdio::null());
        }
        let mut child = command.spawn().unwrap();
        let input = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, responses) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                let Ok(value) = serde_json::from_str(&line) else {
                    panic!("adapter stdout must be JSON only: {line}");
                };
                if tx.send(value).is_err() {
                    break;
                }
            }
        });
        let stderr = capture_stderr.then(|| {
            let stderr = child.stderr.take().unwrap();
            let (tx, rx) = mpsc::channel();
            thread::spawn(move || {
                for line in BufReader::new(stderr).lines() {
                    let Ok(line) = line else { break };
                    if tx.send(line).is_err() {
                        break;
                    }
                }
            });
            rx
        });
        Self {
            child,
            input,
            responses,
            stderr,
            timeout,
        }
    }

    /// Runs the MCP initialize handshake and returns its reply.
    #[track_caller]
    pub fn initialize(&mut self) -> Value {
        let init = self.request(json!({
            "jsonrpc":"2.0","id":1,"method":"initialize",
            "params":{"protocolVersion":PROTOCOL_VERSION,"capabilities":{},
                "clientInfo":{"name":"au-mcp-rs test","version":"1"}}
        }));
        assert_eq!(init["id"], 1, "{init}");
        assert!(init.get("result").is_some(), "{init}");
        self.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
        init
    }

    /// Writes one JSON-RPC message to the adapter.
    pub fn send(&mut self, value: Value) {
        writeln!(self.input, "{value}").unwrap();
        self.input.flush().unwrap();
    }

    /// Writes a `tools/call` without waiting for it.
    pub fn send_call(&mut self, id: u32, name: &str, arguments: Value) {
        self.send(json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":name,"arguments":arguments}}));
    }

    /// Writes one message and returns the next reply.
    #[track_caller]
    pub fn request(&mut self, value: Value) -> Value {
        self.send(value);
        self.receive()
    }

    /// Returns the next reply.
    #[track_caller]
    pub fn receive(&self) -> Value {
        self.responses
            .recv_timeout(self.timeout)
            .expect("MCP response timeout")
    }

    /// True when the adapter sent nothing during `period`.
    pub fn quiet_for(&self, period: Duration) -> bool {
        matches!(
            self.responses.recv_timeout(period),
            Err(RecvTimeoutError::Timeout)
        )
    }

    /// Calls a tool and returns the whole reply, asserting the reply id.
    #[track_caller]
    pub fn call(&mut self, id: u32, name: &str, arguments: Value) -> Value {
        self.send_call(id, name, arguments);
        let response = self.receive();
        assert_eq!(response["id"], id, "{response}");
        response
    }

    /// Calls a tool and returns the reply's `result` member.
    #[track_caller]
    pub fn call_result(&mut self, id: u32, name: &str, arguments: Value) -> Value {
        self.call(id, name, arguments)["result"].clone()
    }

    /// The captured stderr lines, when the client was started with them.
    pub fn stderr(&self) -> Option<&Receiver<String>> {
        self.stderr.as_ref()
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Reads one length-prefixed engine frame and parses its JSON body.
pub fn read_request(stream: &mut impl Read) -> Value {
    let mut len = [0; 4];
    stream.read_exact(&mut len).unwrap();
    let mut body = vec![0; u32::from_be_bytes(len) as usize];
    stream.read_exact(&mut body).unwrap();
    serde_json::from_slice(&body).unwrap()
}

/// Writes one length-prefixed engine frame.
pub fn write_frame(stream: &mut impl Write, payload: Value) {
    let body = serde_json::to_vec(&payload).unwrap();
    stream
        .write_all(&(body.len() as u32).to_be_bytes())
        .unwrap();
    stream.write_all(&body).unwrap();
}

/// Writes a ready `response` frame carrying `result` at engine version `version`.
pub fn write_response_frame(stream: &mut impl Write, version: u64, result: Value) {
    write_frame(
        stream,
        json!({
            "type":"response","schema_version":29,"id":1,"ready":true,"version":version,"result":result
        }),
    );
}

/// The `members` response frame naming one fixture member.
pub fn members_frame(root: &str, version: u64, editable: bool) -> Value {
    json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":version,
        "result":{"members":[{"repo":"fixture","root":root,"editable":editable,"role":"entry"}]}})
}

/// The decoded payload of a tool result's first text block.
#[track_caller]
pub fn payload(result: &Value) -> Value {
    serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap()
}

/// The decoded payload of a successful tool response.
#[track_caller]
pub fn result_payload(response: &Value) -> Value {
    assert!(response.get("error").is_none(), "{response}");
    assert_ne!(response["result"]["isError"], true, "{response}");
    serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap()).unwrap()
}

/// The error text of a failed tool result.
#[track_caller]
pub fn error_text(response: &Value) -> String {
    assert_eq!(response["result"]["isError"], true, "{response}");
    response["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// Asserts the reply is an error, whether JSON-RPC or a tool error.
#[track_caller]
pub fn assert_error(response: &Value) {
    assert!(
        response.get("error").is_some() || response["result"]["isError"] == true,
        "{response}"
    );
}
