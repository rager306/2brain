//! Transport harness shared by the adapter's integration tests.
//!
//! Every test file is its own crate, so this module is included with
//! `mod support;`. It holds only the plumbing the tests share: the stdio
//! client that spawns the adapter and speaks MCP over it, and the engine-frame
//! helpers the fake daemons use. Scenarios, fake-daemon closures, and per-tool
//! assertions stay in the test files.

#![allow(dead_code)]

use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver, RecvTimeoutError},
    thread,
    time::{Duration, Instant},
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
        Self::spawn_configured(socket, &[], DEFAULT_TIMEOUT, false, None)
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

    /// `start_capturing_stderr` for a *workspace* target launched under `home`:
    /// the target is not a `.sock`, so the adapter derives one under that home's
    /// state root — which is how a wrong `$HOME` reaches the connect failure.
    #[track_caller]
    pub fn start_capturing_stderr_at_home(home: &Path, target: &Path) -> Self {
        Self::start_configured_at_home(home, target, &[], true)
    }

    #[track_caller]
    fn start_configured(
        socket: &Path,
        extra: &[&str],
        timeout: Duration,
        capture_stderr: bool,
    ) -> Self {
        let mut client = Self::spawn_configured(socket, extra, timeout, capture_stderr, None);
        client.initialize();
        client
    }

    #[track_caller]
    fn start_configured_at_home(
        home: &Path,
        target: &Path,
        extra: &[&str],
        capture_stderr: bool,
    ) -> Self {
        let mut client =
            Self::spawn_configured(target, extra, DEFAULT_TIMEOUT, capture_stderr, Some(home));
        client.initialize();
        client
    }

    #[track_caller]
    fn spawn_configured(
        socket: &Path,
        extra: &[&str],
        timeout: Duration,
        capture_stderr: bool,
        home: Option<&Path>,
    ) -> Self {
        // One binary for the product: `2brain mcp <target> …` (there is no separate server binary).
        let mut command = Command::new(env!("CARGO_BIN_EXE_2brain"));
        command.arg("mcp");
        command
            .arg(socket)
            .args(extra)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped());
        if let Some(home) = home {
            // The state root is the only thing this changes: the engine reads raw
            // `$HOME`, and the override that would win over it is cleared so the
            // launch derives from `home` and nowhere else.
            command.env("HOME", home).env_remove("2BRAIN_HOME");
        }
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

// ------------------------------------------------------------- real daemon

/// A private engine daemon serving one fixture workspace.
///
/// `HOME` is the fixture's, so the socket, the registries and the package cache
/// all live under the temporary directory: a test can never reach the live
/// device area or a production socket.
pub struct Daemon {
    child: Child,
    socket: PathBuf,
}

impl Daemon {
    /// Start `2brain daemon start <entry>` under `home`, and wait until it answers.
    #[track_caller]
    pub fn start(home: &Path, entry: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_2brain"))
            .arg("daemon")
            .arg("start")
            .arg(entry)
            .env("HOME", home)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("start a private daemon");
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            if let Some(socket) = socket_under(home)
                && exchange(&socket, json!({"read":"lifecycle","id":1}))
                    .is_ok_and(|answer| answer["ready"] == true)
            {
                return Self { child, socket };
            }
            assert!(
                child.try_wait().unwrap().is_none(),
                "the daemon exited before it was ready"
            );
            assert!(Instant::now() < deadline, "the daemon did not become ready");
            thread::sleep(Duration::from_millis(50));
        }
    }

    /// The socket this daemon bound.
    pub fn socket(&self) -> &Path {
        &self.socket
    }

    /// One engine read, asserting the engine answered.
    #[track_caller]
    pub fn read(&self, request: Value) -> Value {
        exchange(&self.socket, request).expect("the private engine answers")
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The first `.sock` a device area holds, under either identity spelling.
fn socket_under(home: &Path) -> Option<PathBuf> {
    for state in [".2brain", ".arsumbris"] {
        let run = home.join(state).join("au-engine").join("run");
        let Ok(entries) = fs::read_dir(&run) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path
                .extension()
                .is_some_and(|extension| extension == "sock")
            {
                return Some(path);
            }
        }
    }
    None
}

/// One request frame to a real engine, one answer frame back.
pub fn exchange(socket: &Path, request: Value) -> std::io::Result<Value> {
    let mut stream = UnixStream::connect(socket)?;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    write_frame(&mut stream, request);
    Ok(read_request(&mut stream))
}
