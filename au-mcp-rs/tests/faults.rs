#![cfg(unix)]

//! Transport faults that a real engine or a real host can produce: the engine
//! dies after a mutation request is sent, dies during a read, or writes a
//! truncated length header. Each case asserts the answer the caller gets and
//! that the server keeps serving afterwards.

use std::{io::Write, os::unix::net::UnixListener, thread, time::Duration};

use serde_json::{Value, json};

mod support;

use support::{Client, read_request, write_frame};

const TIMEOUT: Duration = Duration::from_secs(15);

fn text(reply: &Value) -> String {
    reply["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("no text content: {reply}"))
        .to_owned()
}

fn reply(result: Value) -> Value {
    json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":7,"result":result})
}

#[test]
fn a_mutation_whose_reply_never_arrives_is_unknown_and_cannot_be_replayed_blindly() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let root = temp.path().to_path_buf();
    let daemon = thread::spawn(move || {
        // Each engine exchange is its own connection: the arming read resolves
        // the member root and reads the file, and the gate resolves the member
        // again before the mutation.
        for _ in 0..3 {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(TIMEOUT)).unwrap();
            let request = read_request(&mut stream);
            match request["read"].as_str() {
                Some("members") => write_frame(
                    &mut stream,
                    reply(json!({"members":[{"repo":"fixture","editable":true,"root":root}]})),
                ),
                Some("content") => write_frame(
                    &mut stream,
                    reply(json!({"content":{"text":"---\ntype: note\n---\n","hash":"hash-one"}})),
                ),
                other => panic!("unexpected read {other:?}: {request}"),
            }
        }
        let (mut stream, _) = listener.accept().unwrap();
        stream.set_read_timeout(Some(TIMEOUT)).unwrap();
        let mutation = read_request(&mut stream);
        assert_eq!(mutation["mutate"], "write_file", "{mutation}");
        // The engine dies after accepting the write: the caller cannot know
        // whether it landed.
        drop(stream);
    });

    let mut client = Client::start_with_timeout(&socket, &["--allow-mutations"], TIMEOUT);
    let read = client.call(2, "au_content", json!({"repo":"fixture","path":"a.md"}));
    assert_ne!(read["result"]["isError"], true, "{read}");

    let write = client.call(
        3,
        "au_write_file",
        json!({"repo":"fixture","path":"a.md","content":"x","expected_hash":"hash-one"}),
    );
    assert_eq!(write["result"]["isError"], true, "{write}");
    let error = text(&write);
    assert!(error.starts_with("outcome_unknown:"), "{error}");
    assert!(error.contains("read the file again"), "{error}");

    // The recorded read is gone, so a blind replay is refused locally instead
    // of reaching the engine a second time.
    let replay = client.call(
        4,
        "au_write_file",
        json!({"repo":"fixture","path":"a.md","content":"x","expected_hash":"hash-one"}),
    );
    assert_eq!(replay["result"]["isError"], true, "{replay}");
    assert_eq!(
        text(&replay),
        "policy/prior_read: read this file with au_content before writing it"
    );

    daemon.join().unwrap();
}

#[test]
fn an_engine_that_dies_during_a_read_is_an_error_and_the_server_recovers() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let daemon = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream.set_read_timeout(Some(TIMEOUT)).unwrap();
        assert_eq!(read_request(&mut stream)["read"], "overview");
        drop(stream);
        // The next connection answers normally, so the failure was a transport
        // fault rather than a wedged server.
        let (mut stream, _) = listener.accept().unwrap();
        stream.set_read_timeout(Some(TIMEOUT)).unwrap();
        assert_eq!(read_request(&mut stream)["read"], "overview");
        write_frame(&mut stream, reply(json!({"overview":{}})));
    });

    let mut client = Client::start_with_timeout(&socket, &["--allow-mutations"], TIMEOUT);
    let first = client.call(2, "au_overview", json!({}));
    assert_eq!(first["result"]["isError"], true, "{first}");
    let error = text(&first);
    assert!(!error.is_empty(), "{first}");
    assert!(!error.contains("outcome_unknown"), "{error}");

    let second = client.call(3, "au_overview", json!({}));
    assert_ne!(second["result"]["isError"], true, "{second}");
    assert_eq!(second["result"]["_meta"]["io.arsumbris/engineVersion"], "7");
    daemon.join().unwrap();
}

#[test]
fn a_truncated_length_header_is_an_error_not_a_hang() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let daemon = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream.set_read_timeout(Some(TIMEOUT)).unwrap();
        assert_eq!(read_request(&mut stream)["read"], "types");
        // Two bytes of a four-byte length, then EOF.
        stream.write_all(&[0, 0]).unwrap();
        drop(stream);
    });

    let mut client = Client::start_with_timeout(&socket, &["--allow-mutations"], TIMEOUT);
    let reply = client.call(2, "au_types", json!({}));
    assert_eq!(reply["result"]["isError"], true, "{reply}");
    let error = text(&reply);
    assert!(
        error.contains("length") || error.contains("frame"),
        "{error}"
    );
    daemon.join().unwrap();
}
