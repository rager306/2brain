#![cfg(unix)]

use std::{
    io::{Read, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::Path,
    thread,
    time::{Duration, Instant},
};

use serde_json::{Value, json};

mod support;

use support::Client;

const TIMEOUT: Duration = Duration::from_secs(5);
const QUIET_PERIOD: Duration = Duration::from_millis(250);

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

fn connect(socket: &Path) -> Client {
    let mut client = Client::spawn(socket);
    let init = client.initialize();
    assert_eq!(init["result"]["protocolVersion"], "2025-06-18", "{init}");
    client
}

#[test]
fn cancelled_read_closes_engine_socket_without_reply_and_server_recovers() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut client = connect(&socket);

    client.send_call(2, "tb_overview", json!({}));
    let mut cancelled = accept_read(&listener);
    client.send(json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":2,"reason":"client cancelled"}}));
    let mut byte = [0; 1];
    assert_eq!(
        cancelled
            .read(&mut byte)
            .expect("cancelled engine socket did not close in time"),
        0
    );

    client.send_call(3, "tb_overview", json!({}));
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
    assert!(
        client.quiet_for(QUIET_PERIOD),
        "unexpected response or server exit after recovery"
    );
}

#[test]
fn read_capacity_fails_fast_and_cancellation_releases_a_slot() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut client = connect(&socket);
    let mut streams = Vec::new();
    for id in 10..18 {
        client.send_call(id, "tb_overview", json!({}));
        streams.push(accept_read(&listener));
    }
    client.send(json!({"jsonrpc":"2.0","id":30,"method":"ping"}));
    assert_eq!(client.receive()["id"], 30);
    client.send_call(18, "tb_overview", json!({}));
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
    client.send_call(19, "tb_overview", json!({}));
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

    client.send_call(20, "tb_overview", json!({}));
    let _extra1 = accept_read(&listener);
    client.send_call(21, "tb_overview", json!({}));
    let _extra2 = accept_read(&listener);
    client.send_call(22, "tb_overview", json!({}));
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
    client.send_call(23, "tb_overview", json!({}));
    let _recovered = accept_read(&listener);
    assert!(client.quiet_for(QUIET_PERIOD));
}

#[test]
fn cancellation_is_isolated_from_another_in_flight_read() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut client = connect(&socket);

    let call = |client: &mut Client, id, offset| {
        client.send(json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"tb_types","arguments":{"limit":1,"offset":offset}}}));
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
    assert!(
        client.quiet_for(QUIET_PERIOD),
        "unexpected response or server exit after recovery"
    );
}
