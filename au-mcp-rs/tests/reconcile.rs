#![cfg(unix)]

use std::{os::unix::net::UnixListener, thread, time::Duration};

use serde_json::{Value, json};

mod support;

use support::{Client, error_text, read_request, write_frame};

const TIMEOUT: Duration = Duration::from_secs(5);

fn accept(listener: &UnixListener) -> std::os::unix::net::UnixStream {
    let (stream, _) = listener.accept().unwrap();
    stream.set_read_timeout(Some(TIMEOUT)).unwrap();
    stream.set_write_timeout(Some(TIMEOUT)).unwrap();
    stream
}

/// Answer the `members` + `content` pair a reconcile performs.
fn serve_content(listener: &UnixListener, root: &str, content: Value) {
    let mut members = accept(listener);
    assert_eq!(
        read_request(&mut members),
        json!({"read":"members","id":1,"repo":"fixture"})
    );
    write_frame(
        &mut members,
        json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":5,
            "result":{"members":[{"repo":"fixture","root":root,"editable":true,"role":"entry"}]}}),
    );
    let mut body = accept(listener);
    assert_eq!(
        read_request(&mut body),
        json!({"read":"content","id":1,"path":format!("{root}/a.md")})
    );
    write_frame(
        &mut body,
        json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":6,
            "result":{"content": content}}),
    );
}

#[test]
fn reconcile_requires_exactly_one_expectation() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut client = Client::start_with(&socket, &["--allow-mutations"]);
    for (i, args) in [
        json!({"repo":"fixture","path":"a.md"}),
        json!({"repo":"fixture","path":"a.md","expected_hash":"h","expect_absent":true}),
        json!({"repo":"fixture","path":"a.md","expected_hash":""}),
        json!({"repo":"","path":"a.md","expect_absent":true}),
        json!({"repo":"fixture","path":"","expect_absent":true}),
        json!({"repo":"fixture","path":"../a.md","expect_absent":true}),
        json!({"repo":"fixture","path":"a.md","expect_absent":true,"extra":1}),
    ]
    .into_iter()
    .enumerate()
    {
        assert!(!error_text(&client.call(i as u32 + 2, "au_reconcile", args)).is_empty());
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }
}

#[test]
fn reconcile_reports_applied_when_the_hash_matches() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("ws");
    let root_string = root.to_str().unwrap().to_owned();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let daemon = thread::spawn(move || {
        serve_content(
            &listener,
            &root_string,
            json!({"text":"new","hash":"h-new","commit":null}),
        );
    });
    let mut client = Client::start_with(&socket, &["--allow-mutations"]);
    let reply = client.call(
        2,
        "au_reconcile",
        json!({"repo":"fixture","path":"a.md","expected_hash":"h-new"}),
    );
    assert_ne!(reply["result"]["isError"], true, "{reply}");
    let payload: Value =
        serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(payload["outcome"], "applied");
    assert_eq!(payload["current_hash"], "h-new");
    daemon.join().unwrap();
}

#[test]
fn a_differing_file_reports_not_applied_and_rearms_the_guard() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("ws");
    let root_string = root.to_str().unwrap().to_owned();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let daemon = thread::spawn(move || {
        serve_content(
            &listener,
            &root_string,
            json!({"text":"other","hash":"h-actual","commit":"abc"}),
        );
        // The reconcile re-armed the guard, so a write matching the current hash
        // proceeds without another read.
        let mut members = accept(&listener);
        assert_eq!(
            read_request(&mut members),
            json!({"read":"members","id":1,"repo":"fixture"})
        );
        write_frame(
            &mut members,
            json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":7,
                "result":{"members":[{"repo":"fixture","root":root_string,"editable":true,"role":"entry"}]}}),
        );
        let mut mutate = accept(&listener);
        assert_eq!(
            read_request(&mut mutate),
            json!({"mutate":"write_file","id":1,"path":format!("{root_string}/a.md"),
                "content":"new","expected_hash":"h-actual"})
        );
        write_frame(
            &mut mutate,
            json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":8,
                "result":{"path":format!("{root_string}/a.md"),"hash":"h-new","diagnostics":[],
                    "reflected":true,"commit":null,"commits":{},"untracked_files":[]}}),
        );
    });
    let mut client = Client::start_with(&socket, &["--allow-mutations"]);
    let reply = client.call(
        2,
        "au_reconcile",
        json!({"repo":"fixture","path":"a.md","expected_hash":"h-intended"}),
    );
    assert_ne!(reply["result"]["isError"], true, "{reply}");
    let payload: Value =
        serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(payload["outcome"], "not_applied");
    assert_eq!(payload["current_hash"], "h-actual");
    let write = client.call(
        3,
        "au_write_file",
        json!({"repo":"fixture","path":"a.md","content":"new","expected_hash":"h-actual"}),
    );
    assert_ne!(write["result"]["isError"], true, "{write}");
    daemon.join().unwrap();
}

#[test]
fn reconcile_handles_a_delete_two_ways() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("ws");
    let root_string = root.to_str().unwrap().to_owned();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let daemon = thread::spawn(move || {
        serve_content(&listener, &root_string, Value::Null);
    });
    let mut client = Client::start_with(&socket, &["--allow-mutations"]);
    let gone = client.call(
        2,
        "au_reconcile",
        json!({"repo":"fixture","path":"a.md","expect_absent":true}),
    );
    let payload: Value =
        serde_json::from_str(gone["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(payload["outcome"], "applied");
    assert!(payload["current_hash"].is_null(), "{payload}");
    daemon.join().unwrap();

    let temp2 = tempfile::tempdir().unwrap();
    let root2 = temp2.path().join("ws");
    let root2_string = root2.to_str().unwrap().to_owned();
    let socket2 = temp2.path().join("engine.sock");
    let listener2 = UnixListener::bind(&socket2).unwrap();
    let daemon2 = thread::spawn(move || {
        serve_content(
            &listener2,
            &root2_string,
            json!({"text":"still","hash":"h1"}),
        );
    });
    let mut client2 = Client::start_with(&socket2, &["--allow-mutations"]);
    let present = client2.call(
        2,
        "au_reconcile",
        json!({"repo":"fixture","path":"a.md","expect_absent":true}),
    );
    let payload: Value =
        serde_json::from_str(present["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(payload["outcome"], "not_applied");
    assert_eq!(payload["current_hash"], "h1");
    daemon2.join().unwrap();
}
