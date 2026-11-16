#![cfg(unix)]

//! `tb_graph_health`: the engine's report passed through untouched.

use std::{os::unix::net::UnixListener, thread};

use serde_json::{Value, json};

mod support;

use support::{Client, read_request, write_frame};

#[test]
fn graph_health_sends_the_scope_and_passes_the_report_through() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let daemon = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        assert_eq!(
            read_request(&mut stream),
            json!({"read":"graph_health","id":1,"limit":5,"repo":"fixture","scope":"own"})
        );
        write_frame(
            &mut stream,
            json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":4,
                "result":{"graph_health":{
                    "repo":"fixture","scope":"own",
                    "counts":{"nodes":3,"edges":2,"components":1,"isolated":0,"dead_ends":1,
                              "sources":0,"cycles":1,"nodes_in_cycles":2,"broken_links":0},
                    "cycles":[{"length":2,"nodes":["/ws/a.md","/ws/b.md"]}],
                    "isolated":[],"dead_ends":[{"path":"/ws/c.md","in":1,"out":0,"total":1}],
                    "sources":[],"broken_links":[],
                    "central":[{"path":"/ws/a.md","in":1,"out":1,"total":2}],
                    "truncated":false}}}),
        );
    });
    let mut client = Client::start(&socket);
    let reply = client.call(
        2,
        "tb_graph_health",
        json!({"repo":"fixture","scope":"own","limit":5}),
    );
    assert_ne!(reply["result"]["isError"], true, "{reply}");
    let payload: Value =
        serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(payload["graph_health"]["counts"]["cycles"], 1);
    assert_eq!(payload["graph_health"]["cycles"][0]["length"], 2);
    daemon.join().unwrap();
}

#[test]
fn graph_health_refuses_a_bad_limit_before_the_engine() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut client = Client::start(&socket);
    for (index, arguments) in [
        json!({"limit": 0}),
        json!({"limit": 201}),
        json!({"repo": ""}),
        json!({"depth": 2}),
    ]
    .into_iter()
    .enumerate()
    {
        let reply = client.call(index as u32 + 2, "tb_graph_health", arguments.clone());
        assert_eq!(reply["result"]["isError"], true, "{arguments}: {reply}");
    }
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}
