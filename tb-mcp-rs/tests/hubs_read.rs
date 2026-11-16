use std::{io::Write, os::unix::net::UnixListener, thread, time::Duration};

use serde_json::{Value, json};

mod support;

use support::{Client, read_request};

const VERSION_KEY: &str = "io.arsumbris/engineVersion";

fn response(ready: bool, version: u64, result: Value) -> Value {
    json!({"type":"response","schema_version":29,"id":1,"ready":ready,"version":version,"result":result})
}

fn write_response(stream: &mut impl Write, reply: &Value) {
    let body = serde_json::to_vec(reply).unwrap();
    stream
        .write_all(&(body.len() as u32).to_be_bytes())
        .unwrap();
    stream.write_all(&body).unwrap();
}

fn assert_error(response: &Value) {
    assert!(
        response.get("error").is_some() || response["result"]["isError"] == true,
        "{response}"
    );
    assert!(
        response
            .get("result")
            .and_then(|r| r.get("_meta"))
            .and_then(|m| m.get(VERSION_KEY))
            .is_none(),
        "{response}"
    );
}

fn assert_success(response: &Value, version: u64, payload: &Value) {
    assert!(response.get("error").is_none(), "{response}");
    assert_ne!(response["result"]["isError"], true, "{response}");
    assert_eq!(
        response["result"]["_meta"][VERSION_KEY].as_str(),
        Some(version.to_string().as_str()),
        "{response}"
    );
    assert_eq!(
        response["result"]["content"][0]["type"], "text",
        "{response}"
    );
    let text = response["result"]["content"][0]["text"].as_str().unwrap();
    assert_eq!(&serde_json::from_str::<Value>(text).unwrap(), payload);
}

#[test]
fn invalid_arguments_never_contact_engine() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut client = Client::start(&socket);
    for (index, args) in [
        json!({"repo":""}),
        json!({"repo":3}),
        json!({"scope":""}),
        json!({"scope":"other"}),
        json!({"scope":3}),
        json!({"limit":0}),
        json!({"limit":501}),
        json!({"limit":-1}),
        json!({"limit":1.5}),
        json!({"limit":null}),
        json!({"offset":-1}),
        json!({"offset":1.5}),
        json!({"offset":null}),
        json!({"read":"overview"}),
        json!({"id":99}),
        json!({"write":"delete"}),
        json!({"unknown":true}),
        json!([]),
        json!("hubs"),
        json!(123),
    ]
    .into_iter()
    .enumerate()
    {
        assert_error(&client.call(index as u32 + 2, "tb_hubs", args));
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }
}

#[test]
fn default_and_scoped_paged_requests_preserve_payload_and_version() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let cases = [
        (
            json!({}),
            json!({"read":"hubs","id":1,"limit":20,"offset":0}),
            7_u64,
            json!({"hubs":[]}),
        ),
        (
            json!({"repo":"base"}),
            json!({"read":"hubs","id":1,"repo":"base","limit":20,"offset":0}),
            8,
            json!({"hubs":[{"path":"a.md","incoming":12}],"extra":true}),
        ),
        (
            json!({"scope":"own","limit":1,"offset":0}),
            json!({"read":"hubs","id":1,"scope":"own","limit":1,"offset":0}),
            9,
            json!({"hubs":[{"path":"first.md"}]}),
        ),
        (
            json!({"scope":"all","limit":1,"offset":1}),
            json!({"read":"hubs","id":1,"scope":"all","limit":1,"offset":1}),
            u64::MAX,
            json!({"hubs":[{"path":"second.md"}]}),
        ),
        (
            json!({"repo":"base","scope":"own","limit":500,"offset":10000}),
            json!({"read":"hubs","id":1,"repo":"base","scope":"own","limit":500,"offset":10000}),
            0,
            json!({"hubs":[]}),
        ),
    ];
    let expected = cases.clone();
    let daemon = thread::spawn(move || {
        for (_, request, version, payload) in expected {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            assert_eq!(read_request(&mut stream), request);
            write_response(&mut stream, &response(true, version, payload));
        }
    });
    let mut client = Client::start(&socket);
    for (index, (args, _, version, payload)) in cases.into_iter().enumerate() {
        assert_success(
            &client.call(index as u32 + 2, "tb_hubs", args),
            version,
            &payload,
        );
    }
    daemon.join().unwrap();
}

#[test]
fn diagnostics_reject_invalid_arguments_before_connecting() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut client = Client::start(&socket);
    for (index, args) in [
        json!({"repo":""}),
        json!({"code":""}),
        json!({"severity":"info"}),
        json!({"severity":1}),
        json!({"scope":"other"}),
        json!({"limit":0}),
        json!({"limit":501}),
        json!({"limit":null}),
        json!({"offset":-1}),
        json!({"path":"/tmp/file"}),
        json!({"path_prefix":"/tmp"}),
        json!({"read":"shutdown"}),
        json!({"unknown":true}),
    ]
    .into_iter()
    .enumerate()
    {
        assert_error(&client.call(index as u32 + 2, "tb_diagnostics", args));
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }
}

#[test]
fn diagnostics_requests_preserve_filters_pages_and_failures() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let cases = [
        (
            json!({}),
            json!({"read":"diagnostics","id":1,"limit":50,"offset":0}),
            response(true, 10, json!({"diagnostics":[]})),
            true,
        ),
        (
            json!({"repo":"base","severity":"warning","code":"reference-target-missing","scope":"all","limit":1,"offset":1}),
            json!({"read":"diagnostics","id":1,"limit":1,"offset":1,"repo":"base","severity":"warning","code":"reference-target-missing","scope":"all"}),
            response(
                true,
                11,
                json!({"diagnostics":[{"code":"reference-target-missing","fix":{"actions":[{"verb":"edit_file"}]}}],"extra":true}),
            ),
            true,
        ),
        (
            json!({"scope":"own","severity":"drift","limit":500}),
            json!({"read":"diagnostics","id":1,"limit":500,"offset":0,"scope":"own","severity":"drift"}),
            response(true, 12, json!({"diagnostics":null})),
            false,
        ),
        (
            json!({"severity":"error"}),
            json!({"read":"diagnostics","id":1,"limit":50,"offset":0,"severity":"error"}),
            json!({"type":"error","schema_version":29,"id":1,"for":"read","error":"failed"}),
            false,
        ),
        (
            json!({"severity":"hint"}),
            json!({"read":"diagnostics","id":1,"limit":50,"offset":0,"severity":"hint"}),
            response(true, 13, json!({"diagnostics":[]})),
            true,
        ),
    ];
    let expected = cases.clone();
    let daemon = thread::spawn(move || {
        for (_, request, reply, _) in expected {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            assert_eq!(read_request(&mut stream), request);
            write_response(&mut stream, &reply);
        }
    });
    let mut client = Client::start(&socket);
    for (index, (args, _, reply, success)) in cases.into_iter().enumerate() {
        let result = client.call(index as u32 + 2, "tb_diagnostics", args);
        if success {
            assert_success(
                &result,
                reply["version"].as_u64().unwrap(),
                &reply["result"],
            );
        } else {
            assert_error(&result);
        }
    }
    daemon.join().unwrap();
}

#[test]
fn engine_failures_and_invalid_frames_are_tool_errors_then_recover() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let daemon = thread::spawn(move || {
        for case in 0..8 {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            assert_eq!(
                read_request(&mut stream),
                json!({"read":"hubs","id":1,"limit":20,"offset":0})
            );
            match case {
                0 => write_response(
                    &mut stream,
                    &json!({"type":"error","schema_version":29,"id":1,"for":"read","error":"engine failure"}),
                ),
                1 => write_response(&mut stream, &response(false, 10, json!({"hubs":[]}))),
                2 => write_response(&mut stream, &response(true, 11, json!({}))),
                3 => write_response(&mut stream, &response(true, 12, json!({"hubs":{}}))),
                4 => stream.write_all(&[0, 0]).unwrap(),
                5 => {
                    stream.write_all(&20_u32.to_be_bytes()).unwrap();
                    stream.write_all(b"{\"type\":").unwrap();
                }
                6 => {
                    stream.write_all(&1_u32.to_be_bytes()).unwrap();
                    stream.write_all(b"!").unwrap();
                }
                7 => write_response(
                    &mut stream,
                    &response(true, 13, json!({"hubs":[{"path":"recovered.md"}]})),
                ),
                _ => unreachable!(),
            }
        }
    });
    let mut client = Client::start(&socket);
    for id in 2..9 {
        let reply = client.call(id, "tb_hubs", json!({}));
        assert_eq!(reply["result"]["isError"], true, "{reply}");
        assert_error(&reply);
    }
    assert_success(
        &client.call(9, "tb_hubs", json!({})),
        13,
        &json!({"hubs":[{"path":"recovered.md"}]}),
    );
    daemon.join().unwrap();
}
