use std::{io::Write, os::unix::net::UnixListener, thread, time::Duration};

use serde_json::{Value, json};

mod support;

use support::{Client, assert_error, read_request, result_payload};

fn fake_daemon(listener: UnixListener, replies: Vec<Value>) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        for reply in replies {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            // Exact comparison also rejects every write verb and injected field.
            assert_eq!(
                read_request(&mut stream),
                json!({"read":"type_tree","id":1})
            );
            let body = serde_json::to_vec(&reply).unwrap();
            stream
                .write_all(&(body.len() as u32).to_be_bytes())
                .unwrap();
            stream.write_all(&body).unwrap();
        }
    })
}

fn response(ready: bool, result: Value) -> Value {
    json!({"type":"response","schema_version":29,"id":1,"ready":ready,"version":1,"result":result})
}

#[test]
fn invalid_arguments_never_contact_engine() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut client = Client::start(&socket);
    for (index, args) in [
        json!({"name":"task"}),
        json!({"repo":"work"}),
        json!({"unknown":true}),
        json!({"read":"overview"}),
        json!({"id":99}),
        json!({"write":"delete"}),
        json!([]),
        json!("tree"),
        json!(123),
    ]
    .into_iter()
    .enumerate()
    {
        assert_error(&client.call(index as u32 + 2, "tb_type_tree", args));
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }
}

#[test]
fn null_and_omitted_arguments_are_accepted() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let good = json!({"type_tree":{"roots":[],"nodes":[]}});
    let daemon = fake_daemon(
        listener,
        vec![response(true, good.clone()), response(true, good.clone())],
    );
    let mut client = Client::start(&socket);
    assert_eq!(
        result_payload(&client.call(2, "tb_type_tree", json!(null))),
        good
    );
    let omitted = client.request(
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"tb_type_tree"}}),
    );
    assert_eq!(omitted["id"], 3);
    assert_eq!(result_payload(&omitted), good);
    daemon.join().unwrap();
}

#[test]
fn type_tree_preserves_roots_nodes_empty_and_extra_fields() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let trees = [
        json!({"roots":[],"nodes":[]}),
        json!({"roots":["thing","tag"],"nodes":[
            {"repo":"base","name":"thing","hash":"abc","parents":[],"children":["task"]},
            {"repo":"app","name":"task","hash":"def","parents":["thing","tag"],"children":[]},
            {"name":"future-shape","additional":true}
        ],"extra":{"future":true}}),
    ];
    let daemon = fake_daemon(
        listener,
        trees
            .iter()
            .map(|tree| response(true, json!({"type_tree":tree})))
            .collect(),
    );
    let mut client = Client::start(&socket);
    for (index, tree) in trees.into_iter().enumerate() {
        assert_eq!(
            result_payload(&client.call(index as u32 + 2, "tb_type_tree", json!({}))),
            json!({"type_tree":tree})
        );
    }
    daemon.join().unwrap();
}

#[test]
fn malformed_tree_payloads_are_errors_and_not_ready_recovers() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let bad = [
        json!({}),
        json!({"type_tree":null}),
        json!({"type_tree":[]}),
        json!({"type_tree":{"nodes":[]}}),
        json!({"type_tree":{"roots":null,"nodes":[]}}),
        json!({"type_tree":{"roots":{},"nodes":[]}}),
        json!({"type_tree":{"roots":[]}}),
        json!({"type_tree":{"roots":[],"nodes":null}}),
        json!({"type_tree":{"roots":[],"nodes":{}}}),
    ];
    let good = json!({"type_tree":{"roots":[],"nodes":[]}});
    let mut replies: Vec<_> = bad
        .iter()
        .map(|result| response(true, result.clone()))
        .collect();
    replies.push(response(false, good.clone()));
    replies.push(response(true, good.clone()));
    let daemon = fake_daemon(listener, replies);
    let mut client = Client::start(&socket);
    for id in 2..(bad.len() as u32 + 3) {
        assert_error(&client.call(id, "tb_type_tree", json!({})));
    }
    assert_eq!(
        result_payload(&client.call(bad.len() as u32 + 3, "tb_type_tree", json!({}))),
        good
    );
    daemon.join().unwrap();
}
