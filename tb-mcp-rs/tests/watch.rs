#![cfg(unix)]

//! `tb_watch`: one bounded wait on the engine's `changes` subscription.

use std::{os::unix::net::UnixListener, thread, time::Duration};

use serde_json::{Value, json};

mod support;

use support::{Client, read_request, write_frame};

#[test]
fn watch_subscribes_skips_the_ack_and_returns_the_delta() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let daemon = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        assert_eq!(
            read_request(&mut stream),
            json!({"subscribe":"changes","id":1})
        );
        write_frame(
            &mut stream,
            json!({"type":"ack","schema_version":29,"subscription_id":1,"channel":"changes","accepted":true,"id":1}),
        );
        write_frame(
            &mut stream,
            json!({"type":"change_event","schema_version":29,"subscription_id":1,
                "kind":"knowledge-base-changed","at_version":42,
                "scope_hint":{"scope":"files","added":["/ws/new.md"],
                    "removed":[],"modified":["/ws/notes/a.md"]}}),
        );
    });
    let mut client = Client::start_with_timeout(&socket, &[], Duration::from_secs(10));
    let reply = client.call(2, "tb_watch", json!({}));
    assert_ne!(reply["result"]["isError"], true, "{reply}");
    let payload: Value =
        serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(payload["watch"]["modified"][0], "/ws/notes/a.md");
    assert_eq!(payload["watch"]["added"][0], "/ws/new.md");
    assert_eq!(payload["watch"]["version"], 42);
    assert_eq!(payload["watch"]["timed_out"], false);
    daemon.join().unwrap();
}

#[test]
fn a_timeout_is_an_answer_not_an_error() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let daemon = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let _ = read_request(&mut stream);
        write_frame(
            &mut stream,
            json!({"type":"ack","schema_version":29,"subscription_id":1,"channel":"changes","accepted":true,"id":1}),
        );
        // Hold the connection open with no event: the watch must time out.
        thread::sleep(Duration::from_millis(600));
    });
    let mut client = Client::start_with_timeout(&socket, &[], Duration::from_secs(10));
    let reply = client.call(2, "tb_watch", json!({"timeout_ms": 200}));
    assert_ne!(reply["result"]["isError"], true, "{reply}");
    let payload: Value =
        serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(payload["watch"]["timed_out"], true);
    assert_eq!(payload["watch"]["added"].as_array().unwrap().len(), 0);
    daemon.join().unwrap();
}

#[test]
fn watch_refuses_an_out_of_range_timeout_before_connecting() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut client = Client::start_with_timeout(&socket, &[], Duration::from_secs(10));
    for (index, arguments) in [json!({"timeout_ms": 0}), json!({"timeout_ms": 60_000})]
        .into_iter()
        .enumerate()
    {
        let reply = client.call(index as u32 + 2, "tb_watch", arguments.clone());
        assert_eq!(reply["result"]["isError"], true, "{arguments}: {reply}");
    }
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}
