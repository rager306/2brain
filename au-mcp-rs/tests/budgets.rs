#![cfg(unix)]

//! Response budgets: a text window for `au_content`, paging for
//! `au_instances_of`, the payload modes, and the argument-error hint.

use std::{os::unix::net::UnixListener, thread, time::Duration};

use serde_json::{Value, json};

mod support;

use support::{Client, read_request, write_frame};

const TIMEOUT: Duration = Duration::from_secs(10);

/// Serve engine answers by read verb; `count` includes the startup probe.
fn daemon(
    listener: UnixListener,
    answers: Vec<(&'static str, Value)>,
    count: usize,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        for _ in 0..count {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(TIMEOUT)).unwrap();
            let request = read_request(&mut stream);
            let verb = request["read"].as_str().expect("read verb").to_owned();
            let answer = answers
                .iter()
                .find(|(name, _)| *name == verb)
                .map(|(_, payload)| payload.clone())
                .unwrap_or_else(|| panic!("unexpected engine read: {verb}"));
            write_frame(
                &mut stream,
                json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":5,"result":answer}),
            );
        }
    })
}

const LONG: &str = "abcdefghijklmnopqrstuvwxyz";

#[test]
fn content_returns_an_explicit_window_and_keeps_the_hash() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let handle = daemon(
        listener,
        vec![
            ("overview", json!({"overview":{}})),
            (
                "members",
                json!({"members":[{"repo":"fixture","editable":true,"root":"/tmp/fixture"}]}),
            ),
            (
                "content",
                json!({"content":{"text":LONG,"hash":"hash-full","commit":"abc"}}),
            ),
        ],
        2,
    );
    let mut client = Client::start_with_timeout(&socket, &[], TIMEOUT);
    let result = client.call_result(
        2,
        "au_content",
        json!({"repo":"fixture","path":"a.md","max_chars":10}),
    );
    assert_ne!(result["isError"], true, "{result}");
    let payload: Value =
        serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(payload["content"]["text"], "abcdefghij");
    assert_eq!(payload["content"]["truncated"], true);
    assert_eq!(payload["content"]["total_chars"], LONG.len());
    assert_eq!(payload["content"]["next_offset"], 10);
    // The hash still covers the whole file, so a write based on it is safe.
    assert_eq!(payload["content"]["hash"], "hash-full");
    handle.join().unwrap();

    // Without max_chars the answer is the engine's, unchanged.
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let handle = daemon(
        listener,
        vec![
            ("overview", json!({"overview":{}})),
            (
                "members",
                json!({"members":[{"repo":"fixture","editable":true,"root":"/tmp/fixture"}]}),
            ),
            (
                "content",
                json!({"content":{"text":LONG,"hash":"hash-full","commit":"abc"}}),
            ),
        ],
        2,
    );
    let mut client = Client::start_with_timeout(&socket, &[], TIMEOUT);
    let result = client.call_result(2, "au_content", json!({"repo":"fixture","path":"a.md"}));
    let payload: Value =
        serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(payload["content"]["text"], LONG);
    assert!(payload["content"].get("truncated").is_none(), "{payload}");
    handle.join().unwrap();
}

#[test]
fn instances_of_pages_and_reports_the_total_and_continuation() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let records: Vec<Value> = (0..5)
        .map(|index| json!({"path": format!("/f{index}.md")}))
        .collect();
    let handle = daemon(
        listener,
        vec![
            ("overview", json!({"overview":{}})),
            ("instances_of", json!({"instances_of": records})),
        ],
        1,
    );
    let mut client = Client::start_with_timeout(&socket, &[], TIMEOUT);
    let result = client.call_result(
        2,
        "au_instances_of",
        json!({"type_name":"note","limit":2,"offset":0}),
    );
    let payload: Value =
        serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(payload["total"], 5);
    assert_eq!(payload["offset"], 0);
    assert_eq!(payload["next_offset"], 2);
    assert_eq!(payload["instances_of"].as_array().unwrap().len(), 2);
    handle.join().unwrap();

    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let records: Vec<Value> = (0..3)
        .map(|index| json!({"path": format!("/f{index}.md")}))
        .collect();
    let handle = daemon(
        listener,
        vec![
            ("overview", json!({"overview":{}})),
            ("instances_of", json!({"instances_of": records})),
        ],
        1,
    );
    let mut client = Client::start_with_timeout(&socket, &[], TIMEOUT);
    let result = client.call_result(
        2,
        "au_instances_of",
        json!({"type_name":"note","limit":2,"offset":2}),
    );
    let payload: Value =
        serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(payload["next_offset"], Value::Null, "{payload}");
    assert_eq!(payload["instances_of"].as_array().unwrap().len(), 1);
    handle.join().unwrap();
}

#[test]
fn payload_modes_select_which_representation_is_returned() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let handle = daemon(listener, vec![("overview", json!({"overview":{}}))], 1);

    let mut structured = Client::start_with_timeout(&socket, &["--payload", "structured"], TIMEOUT);
    let result = structured.call_result(2, "au_overview", json!({}));
    assert!(
        result["content"]
            .as_array()
            .is_none_or(|items| items.is_empty()),
        "{result}"
    );
    assert_eq!(result["structuredContent"], json!({"overview": {}}));
    handle.join().unwrap();

    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let handle = daemon(listener, vec![("overview", json!({"overview":{}}))], 1);
    let mut text = Client::start_with_timeout(&socket, &["--payload", "text"], TIMEOUT);
    let result = text.call_result(2, "au_overview", json!({}));
    assert!(result.get("structuredContent").is_none(), "{result}");
    assert_eq!(
        result["content"][0]["text"],
        json!({"overview":{}}).to_string()
    );
    handle.join().unwrap();
}

#[test]
fn an_argument_error_names_the_tool_and_the_required_fields() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    UnixListener::bind(&socket).unwrap();
    let mut client = Client::start_with_timeout(&socket, &[], TIMEOUT);
    let result = client.call_result(2, "au_type", json!({}));
    assert_eq!(result["isError"], true, "{result}");
    let text = result["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("missing field `name`"), "{text}");
    assert!(text.contains("Tool 'au_type' requires: name"), "{text}");
    assert!(text.contains("au_describe_tool"), "{text}");
}
