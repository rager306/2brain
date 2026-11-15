#![cfg(unix)]

//! The `au_grep` read: a literal text search over the catalogued files.

use std::{os::unix::net::UnixListener, thread};

use serde_json::{Value, json};

mod support;

use support::{Client, read_request, write_frame};

fn response(version: u64, result: Value) -> Value {
    json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":version,"result":result})
}

fn payload_of(reply: &Value) -> Value {
    assert_ne!(reply["result"]["isError"], true, "{reply}");
    serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap()
}

#[test]
fn grep_sends_the_literal_search_and_passes_the_answer_through() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let daemon = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        assert_eq!(
            read_request(&mut stream),
            json!({"read":"grep","id":1,"query":"retry policy","limit":20,"offset":0,
                "case_sensitive":true,"repo":"fixture","scope":"own"})
        );
        write_frame(
            &mut stream,
            response(
                7,
                json!({"grep":{
                    "matches":[{"path":"/tmp/ws/notes/a.md","repo":"fixture","line":5,"column":5,
                        "text":"the Retry Policy matters","text_truncated":false}],
                    "total":1,"truncated":false,"skipped_files":2}}),
            ),
        );
    });
    let mut client = Client::start(&socket);
    let reply = client.call(
        2,
        "au_grep",
        json!({"query":"retry policy","case_sensitive":true,"repo":"fixture","scope":"own"}),
    );
    let payload = payload_of(&reply);
    assert_eq!(payload["grep"]["total"], 1);
    assert_eq!(payload["grep"]["matches"][0]["line"], 5);
    assert_eq!(payload["grep"]["skipped_files"], 2);
    assert_eq!(reply["result"]["_meta"]["io.arsumbris/engineVersion"], "7");
    daemon.join().unwrap();
}

#[test]
fn grep_refuses_bad_arguments_before_the_engine() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut client = Client::start(&socket);
    for (index, arguments) in [
        json!({"query":""}),
        json!({"query":"x","repo":""}),
        json!({"query":"x","limit":0}),
        json!({"query":"x","limit":201}),
        json!({"query":"x","regex":true}),
        json!({"query":"x","case_sensitive":"yes"}),
    ]
    .into_iter()
    .enumerate()
    {
        let reply = client.call(index as u32 + 2, "au_grep", arguments.clone());
        assert_eq!(reply["result"]["isError"], true, "{arguments}: {reply}");
    }
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock,
        "a refused argument must not reach the engine"
    );
}
