use std::{io::Write, os::unix::net::UnixListener, thread, time::Duration};

use serde_json::{Value, json};

mod support;

use support::{Client, assert_error, read_request, result_payload};

fn fake_daemon(listener: UnixListener, cases: Vec<(Value, Value)>) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        for (request, reply) in cases {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            assert_eq!(read_request(&mut stream), request);
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

fn default_request() -> Value {
    json!({"read":"types","id":1,"summary":true,"limit":50,"offset":0})
}

#[test]
fn invalid_arguments_never_contact_engine() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut client = Client::start(&socket);
    for (index, args) in [
        json!({"repo":3}),
        json!({"summary":null}),
        json!({"summary":"true"}),
        json!({"limit":0}),
        json!({"limit":501}),
        json!({"limit":-1}),
        json!({"limit":1.5}),
        json!({"limit":null}),
        json!({"offset":-1}),
        json!({"offset":1.5}),
        json!({"offset":null}),
        json!({"unknown":true}),
        json!({"read":"overview"}),
        json!({"id":9}),
        json!({"write":"delete"}),
        json!([]),
        json!("types"),
        json!(123),
    ]
    .into_iter()
    .enumerate()
    {
        assert_error(&client.call(index as u32 + 2, "tb_types", args));
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }
}

#[test]
fn defaults_pagination_repo_and_results_are_preserved() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let cases = [
        (json!({}), default_request(), json!({"types":[]})),
        (json!(null), default_request(), json!({"types":null})),
        (
            json!({"repo":null}),
            default_request(),
            json!({"types":null}),
        ),
        (
            json!({"repo":""}),
            json!({"read":"types","id":1,"repo":"","summary":true,"limit":50,"offset":0}),
            json!({"types":null}),
        ),
        (
            json!({"repo":"work","summary":false,"limit":1,"offset":7}),
            json!({"read":"types","id":1,"repo":"work","summary":false,"limit":1,"offset":7}),
            json!({"types":[{"name":"task","repo":"work","extra":true}],"extra":{"future":true}}),
        ),
        (
            json!({"limit":500,"offset":0}),
            json!({"read":"types","id":1,"summary":true,"limit":500,"offset":0}),
            json!({"types":[]}),
        ),
    ];
    let daemon = fake_daemon(
        listener,
        cases
            .iter()
            .map(|(_, request, payload)| (request.clone(), response(true, payload.clone())))
            .chain(std::iter::once((
                default_request(),
                response(true, json!({"types":[]})),
            )))
            .collect(),
    );
    let mut client = Client::start(&socket);
    for (index, (args, _, payload)) in cases.into_iter().enumerate() {
        assert_eq!(
            result_payload(&client.call(index as u32 + 2, "tb_types", args)),
            payload
        );
    }
    let omitted = client.request(
        json!({"jsonrpc":"2.0","id":8,"method":"tools/call","params":{"name":"tb_types"}}),
    );
    assert_eq!(omitted["id"], 8);
    assert_eq!(result_payload(&omitted), json!({"types":[]}));
    daemon.join().unwrap();
}

#[test]
fn malformed_results_engine_error_and_not_ready_recover() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let replies = [
        response(true, json!({})),
        response(true, json!({"types":{}})),
        response(true, json!({"types":"task"})),
        response(true, json!({"types":1})),
        json!({"type":"error","schema_version":29,"id":1,"for":"read","error":"engine failure"}),
        response(false, json!({"types":[]})),
        response(true, json!({"types":[{"name":"task"}]})),
    ];
    let daemon = fake_daemon(
        listener,
        replies
            .into_iter()
            .map(|reply| (default_request(), reply))
            .collect(),
    );
    let mut client = Client::start(&socket);
    for id in 2..8 {
        assert_error(&client.call(id, "tb_types", json!({})));
    }
    assert_eq!(
        result_payload(&client.call(8, "tb_types", json!({}))),
        json!({"types":[{"name":"task"}]})
    );
    daemon.join().unwrap();
}
