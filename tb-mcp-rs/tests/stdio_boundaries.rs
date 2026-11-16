use std::{
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixListener,
    process::{Child, Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use serde_json::{Value, json};

mod support;

use support::{Client, read_request};

struct OwnedChild(Child);

impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn assert_tool_error(response: &Value) {
    assert_eq!(response["result"]["isError"], true, "{response}");
    assert!(
        response["result"]["content"][0]["text"].is_string(),
        "{response}"
    );
}

#[test]
fn inline_metadata_cannot_bypass_legacy_initialize_for_tools() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&path).unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut client = Client::spawn(&path);
    let meta = json!({"io.modelcontextprotocol/protocolVersion":"2025-06-18","io.modelcontextprotocol/clientCapabilities":{},"io.modelcontextprotocol/clientInfo":{"name":"probe","version":"1"}});
    let denied = client.request(json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"tb_overview","arguments":{},"_meta":meta}}));
    assert_eq!(denied["id"], 2, "{denied}");
    assert!(denied.get("error").is_some(), "{denied}");
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    let denied_list = client
        .request(json!({"jsonrpc":"2.0","id":3,"method":"tools/list","params":{"_meta":meta}}));
    assert_eq!(denied_list["id"], 3, "{denied_list}");
    assert!(denied_list.get("error").is_some(), "{denied_list}");
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    let second = client.call(4, "tb_overview", json!({}));
    assert!(second.get("error").is_some(), "{second}");
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}

#[test]
fn initialized_legacy_session_accepts_call_before_initialized_notification() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&path).unwrap();
    let daemon = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        assert_eq!(read_request(&mut stream), json!({"read":"overview","id":1}));
        let body = serde_json::to_vec(&json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":1,"result":{"overview":{}}})).unwrap();
        stream
            .write_all(&(body.len() as u32).to_be_bytes())
            .unwrap();
        stream.write_all(&body).unwrap();
    });
    let mut client = Client::spawn(&path);
    let initialized = client.request(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"early-call-test","version":"1"}}}));
    assert!(initialized.get("result").is_some(), "{initialized}");
    let response = client.call(2, "tb_overview", json!({}));
    assert_ne!(response["result"]["isError"], true, "{response}");
    assert!(response.get("error").is_none(), "{response}");
    daemon.join().unwrap();
}

#[test]
fn repeated_initialize_does_not_change_peer_session() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("engine.sock");
    let mut client = Client::start(&path);
    let second = client.request(json!({"jsonrpc":"2.0","id":2,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"other","version":"1"}}}));
    assert_eq!(second["id"], 2, "{second}");
    assert!(
        second.get("error").is_some(),
        "second initialize should be rejected: {second}"
    );
    let supported = client.request(json!({"jsonrpc":"2.0","id":3,"method":"tools/list","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2025-06-18"}}}));
    assert!(supported.get("result").is_some(), "{supported}");
}

#[test]
fn queued_responses_are_delivered_when_client_resumes_draining_stdout() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("unused.sock");
    let mut child = OwnedChild(
        Command::new(env!("CARGO_BIN_EXE_2brain"))
            .arg("mcp")
            .arg(path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let mut input = child.0.stdin.take().unwrap();
    let stdout = child.0.stdout.take().unwrap();
    let (line_tx, line_rx) = mpsc::channel();
    let (drain_tx, drain_rx) = mpsc::channel();
    let reader = thread::spawn(move || {
        let mut output = BufReader::new(stdout);
        let mut line = String::new();
        output.read_line(&mut line).unwrap();
        line_tx.send(line).unwrap();
        if drain_rx.recv().is_err() {
            return;
        }
        for line in output.lines() {
            if line_tx.send(line.unwrap()).is_err() {
                break;
            }
        }
    });
    writeln!(input, "{}", json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"backpressure-test","version":"1"}}})).unwrap();
    input.flush().unwrap();
    let init: Value =
        serde_json::from_str(&line_rx.recv_timeout(Duration::from_secs(5)).unwrap()).unwrap();
    assert_eq!(init["id"], 1);
    writeln!(
        input,
        "{}",
        json!({"jsonrpc":"2.0","method":"notifications/initialized"})
    )
    .unwrap();
    for id in 2..102 {
        writeln!(
            input,
            "{}",
            json!({"jsonrpc":"2.0","id":id,"method":"tools/list","params":{}})
        )
        .unwrap();
    }
    input.flush().unwrap();
    drain_tx.send(()).unwrap();
    let mut ids = std::collections::BTreeSet::new();
    for _ in 2..102 {
        let response: Value =
            serde_json::from_str(&line_rx.recv_timeout(Duration::from_secs(10)).unwrap()).unwrap();
        assert!(response["result"]["tools"].is_array(), "{response}");
        assert!(
            ids.insert(response["id"].as_u64().unwrap()),
            "duplicate: {response}"
        );
    }
    assert_eq!(ids, (2..102).collect());
    drop(input);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        if Instant::now() >= deadline {
            let _ = child.0.kill();
            let _ = child.0.wait();
            panic!("server failed to exit after stdout resumed and stdin closed");
        }
        thread::sleep(Duration::from_millis(10));
    }
    reader.join().unwrap();
}

#[test]
fn eof_with_blocked_stdout_terminates_within_bound() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("unused.sock");
    let mut child = Command::new(env!("CARGO_BIN_EXE_2brain"))
        .arg("mcp")
        .arg(path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut stdout = std::io::BufReader::new(child.stdout.take().unwrap());
    writeln!(input, "{}", json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"eof-test","version":"1"}}})).unwrap();
    input.flush().unwrap();
    let mut line = String::new();
    stdout.read_line(&mut line).unwrap();
    assert!(line.contains("\"id\":1"), "{line}");
    writeln!(
        input,
        "{}",
        json!({"jsonrpc":"2.0","method":"notifications/initialized"})
    )
    .unwrap();
    // Enough responses to fill the standard-output pipe several times over.
    for id in 2..402 {
        writeln!(
            input,
            "{}",
            json!({"jsonrpc":"2.0","id":id,"method":"tools/list","params":{}})
        )
        .unwrap();
    }
    input.flush().unwrap();
    // Stop draining stdout and close input: the server must give up rather than hang.
    drop(stdout);
    drop(input);
    let deadline = Instant::now() + Duration::from_secs(12);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success(), "{status}");
            break;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("server did not exit within 12s after stdin EOF with blocked stdout");
        }
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn unknown_tools_and_invalid_arguments_do_not_contact_engine() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&path).unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut client = Client::start(&path);

    for (id, name, args) in [
        (2, "shutdown", json!({})),
        (3, "tb_overview", json!(["write", "delete"])),
        (4, "tb_top_level_dirs", json!("write")),
        (
            5,
            "tb_overview",
            json!({"write":"delete", "path":"/root/vault", "id":99, "read":"shutdown", "nested":{"request":{"delete":"all"}}}),
        ),
        (6, "tb_top_level_dirs", json!({"path":"/root/vault"})),
    ] {
        let response = client.call(id, name, args);
        assert!(
            response.get("error").is_some() || response["result"]["isError"] == true,
            "{response}"
        );
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }
}

#[test]
fn empty_arguments_produce_exact_read_requests() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&path).unwrap();
    let daemon = thread::spawn(move || {
        for expected in ["overview", "top_level_dirs"] {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            assert_eq!(read_request(&mut stream), json!({"read":expected,"id":1}));
            let payload = if expected == "overview" {
                json!({})
            } else {
                json!([])
            };
            let body = serde_json::to_vec(&json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":1,"result":{expected:payload}})).unwrap();
            stream
                .write_all(&(body.len() as u32).to_be_bytes())
                .unwrap();
            stream.write_all(&body).unwrap();
        }
    });
    let mut client = Client::start(&path);
    for (id, name) in [(2, "tb_overview"), (3, "tb_top_level_dirs")] {
        let response = client.call(id, name, json!({}));
        assert_ne!(response["result"]["isError"], true, "{response}");
    }
    daemon.join().unwrap();
}

#[test]
fn engine_errors_and_partial_frames_are_tool_errors_and_server_recovers() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&path).unwrap();
    let daemon = thread::spawn(move || {
        for case in 0..5 {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            assert_eq!(read_request(&mut stream), json!({"read":"overview","id":1}));
            match case {
                0 => {
                    let body = serde_json::to_vec(&json!({"type":"error","schema_version":29,"id":1,"for":"read","error":"late engine failure"})).unwrap();
                    stream
                        .write_all(&(body.len() as u32).to_be_bytes())
                        .unwrap();
                    stream.write_all(&body).unwrap();
                }
                1 => stream.write_all(&[0, 0]).unwrap(),
                2 => {
                    stream.write_all(&20_u32.to_be_bytes()).unwrap();
                    stream.write_all(b"{\"type\":").unwrap();
                }
                3 => stream
                    .write_all(&(16_u32 * 1024 * 1024 + 1).to_be_bytes())
                    .unwrap(),
                4 => {
                    let body = serde_json::to_vec(&json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":1,"result":{"overview":{"members":[]}}})).unwrap();
                    for byte in (body.len() as u32).to_be_bytes().iter().chain(body.iter()) {
                        stream.write_all(&[*byte]).unwrap();
                    }
                }
                _ => unreachable!(),
            }
        }
    });
    let mut client = Client::start(&path);
    for id in 2..6 {
        let response = client.call(id, "tb_overview", json!({}));
        assert_tool_error(&response);
        if id == 2 {
            assert!(
                response["result"]["content"][0]["text"]
                    .as_str()
                    .unwrap()
                    .contains("late engine failure")
            );
        }
    }
    let response = client.call(6, "tb_overview", json!({}));
    assert_ne!(response["result"]["isError"], true, "{response}");
    assert_eq!(
        serde_json::from_str::<Value>(response["result"]["content"][0]["text"].as_str().unwrap())
            .unwrap(),
        json!({"overview":{"members":[]}})
    );
    daemon.join().unwrap();
}
