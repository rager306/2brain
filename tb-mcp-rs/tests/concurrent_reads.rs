#![cfg(unix)]

use std::{
    io::{Read, Write},
    os::unix::net::{UnixListener, UnixStream},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use serde_json::{Value, json};

mod support;

use support::Client;

const TIMEOUT: Duration = Duration::from_secs(5);
const VERSION_KEY: &str = "io.2brain/engineVersion";

fn accept(listener: &UnixListener, offset: u32) -> UnixStream {
    let deadline = Instant::now() + TIMEOUT;
    let mut stream = loop {
        match listener.accept() {
            Ok((stream, _)) => break stream,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(
                    Instant::now() < deadline,
                    "engine accept timeout for offset {offset}"
                );
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("engine accept failed: {error}"),
        }
    };
    stream.set_read_timeout(Some(TIMEOUT)).unwrap();
    stream.set_write_timeout(Some(TIMEOUT)).unwrap();
    let mut len = [0; 4];
    stream.read_exact(&mut len).unwrap();
    let mut body = vec![0; u32::from_be_bytes(len) as usize];
    stream.read_exact(&mut body).unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap(),
        json!({"read":"types","id":1,"summary":true,"limit":1,"offset":offset})
    );
    stream
}

fn respond(stream: &mut UnixStream, value: Value) {
    let body = serde_json::to_vec(&value).unwrap();
    stream
        .write_all(&(body.len() as u32).to_be_bytes())
        .unwrap();
    stream.write_all(&body).unwrap();
}

fn success(version: u64, name: &str) -> Value {
    json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":version,"result":{"types":[{"name":name}]}})
}

fn assert_success(response: &Value, id: u32, version: u64, name: &str) {
    assert_eq!(response["id"], id, "{response}");
    assert!(response.get("error").is_none(), "{response}");
    assert_ne!(response["result"]["isError"], true, "{response}");
    assert_eq!(
        response["result"]["_meta"][VERSION_KEY],
        version.to_string(),
        "{response}"
    );
    assert_eq!(
        response["result"]["content"][0]["type"], "text",
        "{response}"
    );
    let text = response["result"]["content"][0]["text"].as_str().unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(text).unwrap(),
        json!({"types":[{"name":name}]})
    );
}

fn exercise_concurrent_reads(first_fails: bool) {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let (first_seen_tx, first_seen_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let daemon = thread::spawn(move || {
        let mut first = accept(&listener, 10);
        first_seen_tx.send(()).unwrap();
        let mut second = accept(&listener, 20);
        respond(&mut second, success(202, "second"));
        release_rx
            .recv_timeout(TIMEOUT)
            .expect("release first response timeout");
        if first_fails {
            respond(
                &mut first,
                json!({"type":"error","schema_version":29,"id":1,"for":"read","error":"isolated failure"}),
            );
        } else {
            respond(&mut first, success(101, "first"));
        }
        let mut recovery = accept(&listener, 30);
        respond(&mut recovery, success(303, "recovered"));
    });
    let mut client = Client::start(&socket);
    client.send_call(11, "tb_types", json!({"limit":1,"offset":10}));
    first_seen_rx
        .recv_timeout(TIMEOUT)
        .expect("first request timeout");
    client.send_call(22, "tb_types", json!({"limit":1,"offset":20}));
    // The first response is withheld until the second MCP response has arrived.
    assert_success(&client.receive(), 22, 202, "second");
    release_tx.send(()).unwrap();
    let first = client.receive();
    if first_fails {
        assert_eq!(first["id"], 11, "{first}");
        assert_eq!(first["result"]["isError"], true, "{first}");
        assert!(
            first["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("isolated failure"),
            "{first}"
        );
        assert!(
            first["result"]
                .get("_meta")
                .and_then(|meta| meta.get(VERSION_KEY))
                .is_none(),
            "{first}"
        );
    } else {
        assert_success(&first, 11, 101, "first");
    }
    client.send_call(33, "tb_types", json!({"limit":1,"offset":30}));
    assert_success(&client.receive(), 33, 303, "recovered");
    daemon.join().unwrap();
}

#[test]
fn overlapping_reads_keep_their_payload_and_version() {
    exercise_concurrent_reads(false);
}

#[test]
fn overlapping_error_is_isolated_and_server_recovers() {
    exercise_concurrent_reads(true);
}

/// Eight file-addressed reads in flight must all be answered. A tool that takes a permit and
/// then calls a helper that takes a second charges one call twice, so the eighth call would be
/// refused with "capacity reached" even though only four reads were in flight.
#[test]
fn eight_file_reads_in_flight_do_not_exhaust_the_read_capacity() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("ws");
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let root_string = root.to_str().unwrap().to_owned();

    let daemon = thread::spawn(move || {
        // Each call arms itself: a `members` exchange, then its own read. Eight calls in
        // flight interleave, so answer by verb rather than by position.
        for _ in 0..16 {
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("accept failed: {error}"),
                }
            };
            stream.set_read_timeout(Some(TIMEOUT)).unwrap();
            stream.set_write_timeout(Some(TIMEOUT)).unwrap();
            let mut len = [0; 4];
            stream.read_exact(&mut len).unwrap();
            let mut body = vec![0; u32::from_be_bytes(len) as usize];
            stream.read_exact(&mut body).unwrap();
            let request: Value = serde_json::from_slice(&body).unwrap();
            let reply = if request["read"] == "members" {
                json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":5,
                    "result":{"members":[{"repo":"fixture","root":root_string,
                        "editable":true,"role":"entry","tier":"entry","scattered":false,
                        "local":true,"disabled":false,"git":{"root":root_string,"tracked":true}}]}})
            } else {
                json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":6,
                    "result":{"frontmatter":{}}})
            };
            respond(&mut stream, reply);
        }
    });

    let mut client = Client::start(&socket);
    for index in 0..8u32 {
        client.send_call(
            index + 100,
            "tb_frontmatter",
            json!({"repo":"fixture","path":format!("notes/{index}.md")}),
        );
    }
    for index in 0..8u32 {
        let reply = client.receive();
        assert_ne!(
            reply["result"]["isError"], true,
            "read {index} was refused: {reply}"
        );
    }
    daemon.join().unwrap();
}
