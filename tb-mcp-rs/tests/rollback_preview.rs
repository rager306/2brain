#![cfg(unix)]

//! `tb_rollback_preview`: the engine's plan passed through, nothing written.

use std::{os::unix::net::UnixListener, thread};

use serde_json::{Value, json};

mod support;

use support::{Client, read_request, write_frame};

#[test]
fn rollback_preview_resolves_the_member_and_passes_the_plan_through() {
    let temp = tempfile::tempdir().unwrap();
    let root = "/tmp/ws";
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let daemon = thread::spawn(move || {
        // The member table is held state, so the read is one frame, not two.
        let (mut read, _) = listener.accept().unwrap();
        assert_eq!(
            read_request(&mut read),
            json!({"read":"rollback_preview","id":1,"repo":"fixture","commit":"abc123"})
        );
        write_frame(
            &mut read,
            json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":3,
                "result":{"rollback_preview":{
                    "repo":root,"commit":"abc123","subject":"write_file notes/a.md",
                    "parent":"def456",
                    "paths":[{"path":"/tmp/ws/notes/a.md","action":"restore","state":"clean"}],
                    "safe":true}}}),
        );
    });
    let mut client = Client::start(&socket);
    let reply = client.call(
        2,
        "tb_rollback_preview",
        json!({"repo":"fixture","commit":"abc123"}),
    );
    assert_ne!(reply["result"]["isError"], true, "{reply}");
    let payload: Value =
        serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(payload["rollback_preview"]["safe"], true);
    assert_eq!(payload["rollback_preview"]["paths"][0]["action"], "restore");
    daemon.join().unwrap();
}

#[test]
fn rollback_preview_refuses_empty_arguments_before_the_engine() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut client = Client::start(&socket);
    for (index, arguments) in [
        json!({"repo": "", "commit": "abc"}),
        json!({"repo": "fixture", "commit": ""}),
        json!({"repo": "fixture"}),
        json!({"repo": "fixture", "commit": "abc", "depth": 1}),
    ]
    .into_iter()
    .enumerate()
    {
        let reply = client.call(index as u32 + 2, "tb_rollback_preview", arguments.clone());
        assert_eq!(reply["result"]["isError"], true, "{arguments}: {reply}");
    }
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}
