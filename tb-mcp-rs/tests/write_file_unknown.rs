#![cfg(unix)]

use std::{
    os::unix::net::{UnixListener, UnixStream},
    thread,
    time::Duration,
};

use serde_json::{Value, json};

mod support;

use support::{Client, error_text, members_frame, read_request, write_frame};

const TIMEOUT: Duration = Duration::from_secs(5);

fn accept(listener: &UnixListener) -> UnixStream {
    let (stream, _) = listener.accept().unwrap();
    stream.set_read_timeout(Some(TIMEOUT)).unwrap();
    stream.set_write_timeout(Some(TIMEOUT)).unwrap();
    stream
}

/// Answer one `members` read for `fixture`.
fn serve_members(listener: &UnixListener, root: &str, editable: bool, version: u64) {
    let mut stream = accept(listener);
    assert_eq!(
        read_request(&mut stream),
        json!({"read":"members","id":1,"repo":"fixture"})
    );
    write_frame(&mut stream, members_frame(root, version, editable));
}

/// Read the file once through the adapter, leaving a recorded hash behind.
/// The request is sent first: the fake engine answers connections the adapter
/// opens only after it has the request.
fn prime(client: &mut Client, listener: &UnixListener, root: &str) {
    client.send_call(2, "au_content", json!({"repo":"fixture","path":"a.md"}));
    serve_members(listener, root, true, 5);
    let mut content = accept(listener);
    assert_eq!(
        read_request(&mut content),
        json!({"read":"content","id":1,"path":format!("{root}/a.md")})
    );
    write_frame(
        &mut content,
        json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":6,
            "result":{"content":{"text":"old","hash":"h1","commit":null}}}),
    );
    let read = receive_id(client, 2);
    assert_ne!(read["result"]["isError"], true, "{read}");
}

fn receive_id(client: &Client, id: u32) -> Value {
    let response = client.receive();
    assert_eq!(response["id"], id, "{response}");
    response
}

#[test]
fn a_lost_mutation_reply_is_reported_as_outcome_unknown_and_not_retried() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("ws");
    let root_string = root.to_str().unwrap().to_owned();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let driver = thread::spawn(move || {
        let mut client = Client::start_with(&socket, &["--allow-mutations"]);
        prime(&mut client, &listener, &root_string);
        client.send_call(
            3,
            "au_write_file",
            json!({"repo":"fixture","path":"a.md","content":"new","expected_hash":"h1"}),
        );
        serve_members(&listener, &root_string, true, 8);
        let mut mutate = accept(&listener);
        assert_eq!(
            read_request(&mut mutate),
            json!({"mutate":"write_file","id":1,"path":format!("{root_string}/a.md"),
                "content":"new","expected_hash":"h1"})
        );
        // Drop the connection without answering: the write may have landed.
        drop(mutate);
        let reply = receive_id(&client, 3);
        assert!(error_text(&reply).starts_with("outcome_unknown"), "{reply}");
        let again = client.call(
            4,
            "au_write_file",
            json!({"repo":"fixture","path":"a.md","content":"new","expected_hash":"h1"}),
        );
        assert!(error_text(&again).contains("au_content"), "{again}");
    });
    driver.join().unwrap();
}

#[test]
fn an_engine_rejection_is_not_reported_as_unknown() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("ws");
    let root_string = root.to_str().unwrap().to_owned();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let driver = thread::spawn(move || {
        let mut client = Client::start_with(&socket, &["--allow-mutations"]);
        prime(&mut client, &listener, &root_string);
        client.send_call(
            3,
            "au_write_file",
            json!({"repo":"fixture","path":"a.md","content":"new","expected_hash":"h1"}),
        );
        serve_members(&listener, &root_string, true, 8);
        let mut mutate = accept(&listener);
        read_request(&mut mutate);
        write_frame(
            &mut mutate,
            json!({"type":"error","schema_version":29,"id":1,"for":"mutate",
                "error":"hash mismatch","detail":{"current_hash":"h9"}}),
        );
        let reply = receive_id(&client, 3);
        let text = error_text(&reply);
        assert!(text.contains("hash mismatch"), "{text}");
        assert!(!text.contains("outcome_unknown"), "{text}");
    });
    driver.join().unwrap();
}

#[test]
fn a_consumed_member_is_refused_before_the_mutation() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("dep");
    let root_string = root.to_str().unwrap().to_owned();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let driver = thread::spawn(move || {
        let mut client = Client::start_with(&socket, &["--allow-mutations"]);
        prime(&mut client, &listener, &root_string);
        client.send_call(
            3,
            "au_write_file",
            json!({"repo":"fixture","path":"a.md","content":"new","expected_hash":"h1"}),
        );
        serve_members(&listener, &root_string, false, 9);
        let reply = receive_id(&client, 3);
        let text = error_text(&reply);
        assert!(text.contains("not editable"), "{text}");
        listener.set_nonblocking(true).unwrap();
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock,
            "no mutation may be sent to a consumed member"
        );
    });
    driver.join().unwrap();
}
