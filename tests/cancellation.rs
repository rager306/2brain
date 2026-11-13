#![cfg(unix)]

use std::{
    io::{BufRead, BufReader, Read, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::Path,
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver, RecvTimeoutError},
    thread,
    time::{Duration, Instant},
};

use serde_json::{Value, json};

const TIMEOUT: Duration = Duration::from_secs(5);
const QUIET_PERIOD: Duration = Duration::from_millis(250);

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
                let line = line.expect("read MCP stdout");
                if tx
                    .send(serde_json::from_str(&line).expect("MCP JSON"))
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
        client.send(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"cancellation-test","version":"1"}}}));
        let init = client.receive();
        assert_eq!(init["id"], 1, "{init}");
        assert_eq!(init["result"]["protocolVersion"], "2025-06-18", "{init}");
        client.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
        client
    }

    fn send(&mut self, request: Value) {
        writeln!(self.input, "{request}").unwrap();
        self.input.flush().unwrap();
    }

    fn call(&mut self, id: u32) {
        self.send(json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"au_overview","arguments":{}}}));
    }

    fn receive(&self) -> Value {
        self.responses
            .recv_timeout(TIMEOUT)
            .expect("MCP response timeout")
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn accept_request(listener: &UnixListener) -> (UnixStream, Value) {
    let deadline = Instant::now() + TIMEOUT;
    let mut stream = loop {
        match listener.accept() {
            Ok((stream, _)) => break stream,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(Instant::now() < deadline, "engine accept timeout");
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("engine accept failed: {error}"),
        }
    };
    stream.set_read_timeout(Some(TIMEOUT)).unwrap();
    stream.set_write_timeout(Some(TIMEOUT)).unwrap();
    let mut len = [0; 4];
    stream.read_exact(&mut len).unwrap();
    let len = u32::from_be_bytes(len) as usize;
    assert!(len <= 4096, "unexpected engine request length: {len}");
    let mut body = vec![0; len];
    stream.read_exact(&mut body).unwrap();
    let request = serde_json::from_slice::<Value>(&body).unwrap();
    (stream, request)
}

fn accept_read(listener: &UnixListener) -> UnixStream {
    let (stream, request) = accept_request(listener);
    assert_eq!(request, json!({"read":"overview","id":1}));
    stream
}

#[test]
fn cancelled_read_closes_engine_socket_without_reply_and_server_recovers() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut client = Client::start(&socket);

    client.call(2);
    let mut cancelled = accept_read(&listener);
    client.send(json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":2,"reason":"client cancelled"}}));
    let mut byte = [0; 1];
    assert_eq!(
        cancelled
            .read(&mut byte)
            .expect("cancelled engine socket did not close in time"),
        0
    );

    client.call(3);
    let mut recovery = accept_read(&listener);
    let body = serde_json::to_vec(&json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":202,"result":{"overview":{"members":[]}}})).unwrap();
    recovery
        .write_all(&(body.len() as u32).to_be_bytes())
        .unwrap();
    recovery.write_all(&body).unwrap();
    let response = client.receive();
    assert_eq!(
        response["id"], 3,
        "unexpected response (including cancelled id): {response}"
    );
    assert_ne!(response["result"]["isError"], true, "{response}");
    let text = response["result"]["content"][0]["text"].as_str().unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(text).unwrap(),
        json!({"overview":{"members":[]}})
    );
    match client.responses.recv_timeout(QUIET_PERIOD) {
        Err(RecvTimeoutError::Timeout) => {}
        other => panic!("unexpected response or server exit after recovery: {other:?}"),
    }
}

#[test]
fn read_capacity_fails_fast_and_cancellation_releases_a_slot() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut client = Client::start(&socket);
    let mut streams = Vec::new();
    for id in 10..18 {
        client.call(id);
        streams.push(accept_read(&listener));
    }
    client.send(json!({"jsonrpc":"2.0","id":30,"method":"ping"}));
    assert_eq!(client.receive()["id"], 30);
    client.call(18);
    let busy = client.receive();
    assert_eq!(busy["id"], 18, "{busy}");
    assert_eq!(busy["result"]["isError"], true, "{busy}");
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    client.send(
        json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":10}}),
    );
    let mut byte = [0];
    assert_eq!(streams[0].read(&mut byte).unwrap(), 0);
    client.call(19);
    let mut replacement = accept_read(&listener);
    let body = serde_json::to_vec(&json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":9,"result":{"overview":{"members":[]}}})).unwrap();
    replacement
        .write_all(&(body.len() as u32).to_be_bytes())
        .unwrap();
    replacement.write_all(&body).unwrap();
    let reply = client.receive();
    assert_eq!(reply["id"], 19, "{reply}");
    assert_ne!(reply["result"]["isError"], true, "{reply}");
    let body = serde_json::to_vec(&json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":10,"result":{"overview":{"members":[]}}})).unwrap();
    streams[1]
        .write_all(&(body.len() as u32).to_be_bytes())
        .unwrap();
    streams[1].write_all(&body).unwrap();
    let reply = client.receive();
    assert_eq!(reply["id"], 11, "{reply}");
    assert_ne!(reply["result"]["isError"], true, "{reply}");

    client.call(20);
    let _extra1 = accept_read(&listener);
    client.call(21);
    let _extra2 = accept_read(&listener);
    client.call(22);
    let busy = client.receive();
    assert_eq!(busy["id"], 22, "{busy}");
    assert_eq!(busy["result"]["isError"], true, "{busy}");
    let error = serde_json::to_vec(
        &json!({"type":"error","schema_version":29,"id":1,"for":"read","error":"failed"}),
    )
    .unwrap();
    streams[2]
        .write_all(&(error.len() as u32).to_be_bytes())
        .unwrap();
    streams[2].write_all(&error).unwrap();
    let failed = client.receive();
    assert_eq!(failed["id"], 12, "{failed}");
    assert_eq!(failed["result"]["isError"], true, "{failed}");
    client.call(23);
    let _recovered = accept_read(&listener);
    assert!(matches!(
        client.responses.recv_timeout(QUIET_PERIOD),
        Err(RecvTimeoutError::Timeout)
    ));
}

#[test]
fn cancellation_is_isolated_from_another_in_flight_read() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut client = Client::start(&socket);

    let call = |client: &mut Client, id, offset| {
        client.send(json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"au_types","arguments":{"limit":1,"offset":offset}}}));
    };
    let accept_types = |offset| {
        let (stream, request) = accept_request(&listener);
        assert_eq!(
            request,
            json!({"read":"types","id":1,"summary":true,"limit":1,"offset":offset})
        );
        stream
    };
    let respond = |stream: &mut UnixStream, version, name| {
        let body = serde_json::to_vec(&json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":version,"result":{"types":[{"name":name}]}})).unwrap();
        stream
            .write_all(&(body.len() as u32).to_be_bytes())
            .unwrap();
        stream.write_all(&body).unwrap();
    };
    let assert_response = |response: Value, id, version: u64, name| {
        assert_eq!(response["id"], id, "{response}");
        assert!(response.get("error").is_none(), "{response}");
        assert_ne!(response["result"]["isError"], true, "{response}");
        assert_eq!(
            response["result"]["_meta"]["io.arsumbris/engineVersion"],
            version.to_string(),
            "{response}"
        );
        let text = response["result"]["content"][0]["text"].as_str().unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(text).unwrap(),
            json!({"types":[{"name":name}]})
        );
    };

    call(&mut client, 11, 10);
    let mut first = accept_types(10);
    call(&mut client, 22, 20);
    let mut second = accept_types(20);
    client.send(json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":11,"reason":"client cancelled"}}));
    let mut byte = [0; 1];
    assert_eq!(
        first
            .read(&mut byte)
            .expect("cancelled engine socket did not close in time"),
        0
    );
    respond(&mut second, 202, "second");
    assert_response(client.receive(), 22, 202, "second");

    client.send(
        json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":22}}),
    );
    client.send(
        json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":999}}),
    );
    call(&mut client, 33, 30);
    let mut recovery = accept_types(30);
    respond(&mut recovery, 303, "recovered");
    assert_response(client.receive(), 33, 303, "recovered");
    match client.responses.recv_timeout(QUIET_PERIOD) {
        Err(RecvTimeoutError::Timeout) => {}
        other => panic!("unexpected response or server exit after recovery: {other:?}"),
    }
}
