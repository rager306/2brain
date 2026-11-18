#![cfg(unix)]

//! `tb_batch`: several file ops armed one by one, sent as one engine saga.

use std::{os::unix::net::UnixListener, thread};

use serde_json::{Value, json};

mod support;

use support::{Client, error_text, members_frame, read_request, write_frame};

fn content_frame(hash: &str) -> Value {
    json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":6,
        "result":{"content":{"text":"x","hash":hash,"commit":null}}})
}

#[test]
fn batch_sends_one_saga_with_absolute_paths_and_every_hash() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("ws");
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let root_string = root.to_str().unwrap().to_owned();
    let expected = root_string.clone();
    let daemon = thread::spawn(move || {
        // Two tb_content reads: members then content, apiece.
        for hash in ["hash-a", "hash-b"] {
            let (mut members, _) = listener.accept().unwrap();
            assert_eq!(
                read_request(&mut members),
                json!({"read":"members","id":1,"repo":"fixture"})
            );
            write_frame(&mut members, members_frame(&expected, 5, true));
            let (mut content, _) = listener.accept().unwrap();
            let _ = read_request(&mut content);
            write_frame(&mut content, content_frame(hash));
        }
        // tb_batch arms each path: one editability read per op.
        for _ in 0..2 {
            let (mut members, _) = listener.accept().unwrap();
            assert_eq!(
                read_request(&mut members),
                json!({"read":"members","id":1,"repo":"fixture"})
            );
            write_frame(&mut members, members_frame(&expected, 5, true));
        }
        let (mut mutate, _) = listener.accept().unwrap();
        assert_eq!(
            read_request(&mut mutate),
            json!({"mutate":"batch","id":1,"ops":[
                {"op":"write_file","path":format!("{expected}/notes/a.md"),"content":"A","expected_hash":"hash-a"},
                {"op":"delete_file","path":format!("{expected}/notes/b.md"),"expected_hash":"hash-b"}
            ]})
        );
        write_frame(
            &mut mutate,
            json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":9,
                "result":{"ops":[{"op":"write_file","path":format!("{expected}/notes/a.md"),"hash":"h1"},
                                  {"op":"delete_file","path":format!("{expected}/notes/b.md"),"hash":null}],
                          "diagnostics":[],"reflected":true,"commit":null,
                          "commits":{"fixture":"c0ffee"},"untracked_files":[]}}),
        );
    });
    let mut client = Client::start_with(&socket, &["--allow-mutations"]);
    let _ = client.call(
        2,
        "tb_content",
        json!({"repo":"fixture","path":"notes/a.md"}),
    );
    let _ = client.call(
        3,
        "tb_content",
        json!({"repo":"fixture","path":"notes/b.md"}),
    );
    let reply = client.call(
        4,
        "tb_batch",
        json!({"repo":"fixture","ops":[
            {"op":"write_file","path":"notes/a.md","content":"A","expected_hash":"hash-a"},
            {"op":"delete_file","path":"notes/b.md","expected_hash":"hash-b"}
        ]}),
    );
    assert_ne!(reply["result"]["isError"], true, "{reply}");
    let payload: Value =
        serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(payload["ops"].as_array().unwrap().len(), 2);
    assert_eq!(payload["commits"]["fixture"], "c0ffee");
    daemon.join().unwrap();
}

#[test]
fn batch_refuses_when_one_op_was_never_read() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut client = Client::start_with(&socket, &["--allow-mutations"]);
    let text = error_text(&client.call(
        2,
        "tb_batch",
        json!({"repo":"fixture","ops":[
            {"op":"write_file","path":"notes/a.md","content":"A","expected_hash":"hash-a"}
        ]}),
    ));
    assert!(text.contains("tb_content"), "{text}");
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock,
        "nothing may reach the engine when an op is unarmed"
    );
}

#[test]
fn batch_refuses_an_empty_list_before_the_engine() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut client = Client::start_with(&socket, &["--allow-mutations"]);
    let text = error_text(&client.call(2, "tb_batch", json!({"repo":"fixture","ops":[]})));
    assert!(text.contains("'ops' must not be empty"), "{text}");
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}

#[test]
fn batch_carries_require_clean_only_when_the_caller_asked() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("ws");
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let root_string = root.to_str().unwrap().to_owned();
    let expected = root_string.clone();
    let daemon = thread::spawn(move || {
        for with_flag in [true, false] {
            let (mut members, _) = listener.accept().unwrap();
            assert_eq!(
                read_request(&mut members),
                json!({"read":"members","id":1,"repo":"fixture"})
            );
            write_frame(&mut members, members_frame(&expected, 5, true));
            let (mut content, _) = listener.accept().unwrap();
            let _ = read_request(&mut content);
            write_frame(&mut content, content_frame("hash-a"));
            let (mut members, _) = listener.accept().unwrap();
            assert_eq!(
                read_request(&mut members),
                json!({"read":"members","id":1,"repo":"fixture"})
            );
            write_frame(&mut members, members_frame(&expected, 5, true));
            let (mut mutate, _) = listener.accept().unwrap();
            let sent = read_request(&mut mutate);
            assert_eq!(
                sent,
                if with_flag {
                    json!({"mutate":"batch","id":1,
                        "ops":[{"op":"write_file","path":format!("{expected}/notes/a.md"),
                                "content":"A","expected_hash":"hash-a"}],
                        "require_clean":true})
                } else {
                    json!({"mutate":"batch","id":1,
                        "ops":[{"op":"write_file","path":format!("{expected}/notes/a.md"),
                                "content":"A","expected_hash":"hash-a"}]})
                },
                "an optional flag travels only when the caller set it"
            );
            write_frame(
                &mut mutate,
                json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":9,
                    "result":{"ops":[{"op":"write_file","path":format!("{expected}/notes/a.md"),"hash":"h1"}],
                              "diagnostics":[],"reflected":true,"commit":null,
                              "commits":{"fixture":"c0ffee"},"untracked_files":[]}}),
            );
        }
    });
    let mut client = Client::start_with(&socket, &["--allow-mutations"]);
    for (call, with_flag) in [(2, true), (5, false)] {
        let _ = client.call(
            call,
            "tb_content",
            json!({"repo":"fixture","path":"notes/a.md"}),
        );
        let mut args = json!({"repo":"fixture","ops":[
            {"op":"write_file","path":"notes/a.md","content":"A","expected_hash":"hash-a"}
        ]});
        if with_flag {
            args["require_clean"] = json!(true);
        }
        let reply = client.call(call + 1, "tb_batch", args);
        assert_ne!(reply["result"]["isError"], true, "{reply}");
    }
    daemon.join().unwrap();
}
