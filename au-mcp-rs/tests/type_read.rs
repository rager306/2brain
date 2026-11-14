use std::{io::Write, os::unix::net::UnixListener, thread, time::Duration};

use serde_json::json;

mod support;

use support::{Client, assert_error, read_request, result_payload};

#[test]
fn invalid_arguments_are_rejected_without_engine_connection() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut client = Client::start(&socket);

    for (id, args) in [
        (2, json!({})),
        (3, json!({"name":""})),
        (4, json!({"name":null})),
        (5, json!({"name":123})),
        (6, json!({"name":"task","repo":12})),
        (7, json!({"name":"task","unknown":true})),
        (8, json!({"name":"task","read":"overview","id":99})),
        (9, json!(["task"])),
        (10, json!("task")),
        (11, json!({"name":"task","repo":""})),
        (12, json!({"name":"task::other","repo":""})),
    ] {
        assert_error(&client.call(id, "au_type", args));
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }
}

#[test]
fn type_tool_schema_requires_nonempty_name_and_nonempty_nullable_repo() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut client = Client::start(&socket);
    let response =
        client.request(json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}));
    let tools = response["result"]["tools"].as_array().unwrap();
    let schema = &tools.iter().find(|tool| tool["name"] == "au_type").unwrap()["inputSchema"];
    assert_eq!(schema["required"], json!(["name"]));
    assert_eq!(schema["properties"]["name"]["type"], "string");
    assert_eq!(schema["properties"]["name"]["minLength"], 1);

    let repo = &schema["properties"]["repo"];
    if let Some(branches) = repo["anyOf"].as_array() {
        let string = branches
            .iter()
            .find(|branch| branch["type"] == "string")
            .unwrap();
        assert_eq!(string["minLength"], 1);
        assert!(branches.iter().any(|branch| branch["type"] == "null"));
    } else {
        let types = repo["type"].as_array().unwrap();
        assert!(types.contains(&json!("string")));
        assert!(types.contains(&json!("null")));
        assert_eq!(repo["minLength"], 1);
    }
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}

#[test]
fn type_reads_qualify_names_and_preserve_object_or_null() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let cases = [
        (
            json!({"name":"task"}),
            "task",
            json!({"name":"task","fields":[]}),
        ),
        (
            json!({"name":"task","repo":null}),
            "task",
            json!({"name":"task"}),
        ),
        (
            json!({"name":"task","repo":"work"}),
            "task::work",
            json!({"name":"task"}),
        ),
        (
            json!({"name":"task::other","repo":"work"}),
            "task::other",
            json!(null),
        ),
        (
            json!({"name":"overview"}),
            "overview",
            json!({"name":"overview"}),
        ),
    ];
    let expected = cases.clone();
    let daemon = thread::spawn(move || {
        for (_, name, payload) in expected {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            assert_eq!(
                read_request(&mut stream),
                json!({"read":"type","name":name,"id":1})
            );
            let body = serde_json::to_vec(&json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":1,"result":{"type":payload}})).unwrap();
            stream
                .write_all(&(body.len() as u32).to_be_bytes())
                .unwrap();
            stream.write_all(&body).unwrap();
        }
    });
    let mut client = Client::start(&socket);
    for (index, (args, _, payload)) in cases.into_iter().enumerate() {
        assert_eq!(
            result_payload(&client.call(index as u32 + 2, "au_type", args)),
            json!({"type":payload})
        );
    }
    daemon.join().unwrap();
}

#[test]
fn subtypes_invalid_arguments_never_contact_engine() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut client = Client::start(&socket);
    for (i, args) in [
        json!({}),
        json!({"base":""}),
        json!({"base":null}),
        json!({"base":1}),
        json!({"base":"note","scope":"other"}),
        json!({"base":"note","repo":"other"}),
        json!({"base":"note","read":"shutdown"}),
    ]
    .into_iter()
    .enumerate()
    {
        assert_error(&client.call(i as u32 + 2, "au_subtypes", args));
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }
}

#[test]
fn subtypes_preserve_envelope_and_reject_bad_results_before_recovery() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let cases = [
        (
            json!({"base":"widget"}),
            json!({"read":"subtypes","id":1,"base":"widget"}),
            json!({"base":"widget","subtypes":[{"name":"widget.child","repo":"ra","hash":"same"},{"name":"widget.child","repo":"rb","hash":"same"}],"extra":1}),
            true,
        ),
        (
            json!({"base":"widget::ra","scope":"own"}),
            json!({"read":"subtypes","id":1,"base":"widget::ra","scope":"own"}),
            json!({"base":"widget::ra","subtypes":[]}),
            true,
        ),
        (
            json!({"base":"widget","scope":"all"}),
            json!({"read":"subtypes","id":1,"base":"widget","scope":"all"}),
            json!({"base":"other","subtypes":[]}),
            false,
        ),
        (
            json!({"base":"widget"}),
            json!({"read":"subtypes","id":1,"base":"widget"}),
            json!({"subtypes":[]}),
            false,
        ),
        (
            json!({"base":"widget"}),
            json!({"read":"subtypes","id":1,"base":"widget"}),
            json!({"base":"widget","subtypes":null}),
            false,
        ),
        (
            json!({"base":"missing"}),
            json!({"read":"subtypes","id":1,"base":"missing"}),
            json!({"base":"missing","subtypes":[]}),
            true,
        ),
    ];
    let expected = cases.clone();
    let daemon = thread::spawn(move || {
        for (_, request, payload, _) in expected {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            assert_eq!(read_request(&mut stream), request);
            let body = serde_json::to_vec(&json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":42,"result":payload})).unwrap();
            stream
                .write_all(&(body.len() as u32).to_be_bytes())
                .unwrap();
            stream.write_all(&body).unwrap();
        }
    });
    let mut client = Client::start(&socket);
    for (i, (args, _, payload, success)) in cases.into_iter().enumerate() {
        let response = client.call(i as u32 + 2, "au_subtypes", args);
        if success {
            assert_eq!(result_payload(&response), payload);
            assert_eq!(
                response["result"]["_meta"]["io.arsumbris/engineVersion"],
                "42"
            );
        } else {
            assert_error(&response);
            assert!(response["result"].get("_meta").is_none());
        }
    }
    daemon.join().unwrap();
}

#[test]
fn members_reject_invalid_arguments_without_connecting() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut client = Client::start(&socket);
    for (i, args) in [
        json!({"repo":""}),
        json!({"repo":42}),
        json!({"scope":"own"}),
        json!({"path":"/tmp"}),
        json!({"read":"shutdown"}),
    ]
    .into_iter()
    .enumerate()
    {
        assert_error(&client.call(i as u32 + 2, "au_members", args));
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }
}

#[test]
fn members_preserve_envelope_and_recover_after_invalid_responses() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let cases = [
        (
            json!({}),
            json!({"read":"members","id":1}),
            json!({"members":[{"repo":"fixture","root":"/tmp/fixture","role":"entry","editable":true},{"repo":"disabled","disabled":true}],"extra":true}),
            true,
        ),
        (
            json!({"repo":null}),
            json!({"read":"members","id":1}),
            json!({"members":[]}),
            true,
        ),
        (
            json!({"repo":"missing"}),
            json!({"read":"members","id":1,"repo":"missing"}),
            json!({"members":[]}),
            true,
        ),
        (
            json!({"repo":"fixture"}),
            json!({"read":"members","id":1,"repo":"fixture"}),
            json!({"members":null}),
            false,
        ),
        (
            json!({}),
            json!({"read":"members","id":1}),
            json!({}),
            false,
        ),
        (
            json!({}),
            json!({"read":"members","id":1}),
            json!({"members":[{"repo":"recovered"}]}),
            true,
        ),
    ];
    let expected = cases.clone();
    let daemon = thread::spawn(move || {
        for (_, request, payload, _) in expected {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            assert_eq!(read_request(&mut stream), request);
            let body = serde_json::to_vec(&json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":42,"result":payload})).unwrap();
            stream
                .write_all(&(body.len() as u32).to_be_bytes())
                .unwrap();
            stream.write_all(&body).unwrap();
        }
    });
    let mut client = Client::start(&socket);
    for (i, (args, _, payload, success)) in cases.into_iter().enumerate() {
        let response = client.call(i as u32 + 2, "au_members", args);
        if success {
            assert_eq!(result_payload(&response), payload);
            assert_eq!(
                response["result"]["_meta"]["io.arsumbris/engineVersion"],
                "42"
            );
        } else {
            assert_error(&response);
            assert!(response["result"].get("_meta").is_none());
        }
    }
    daemon.join().unwrap();
}

#[test]
fn imports_invalid_arguments_never_contact_engine() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut client = Client::start(&socket);
    for (i, args) in [
        json!({"scope":"bad"}),
        json!({"scope":3}),
        json!({"repo":"fixture"}),
        json!({"limit":1}),
        json!({"path":"/tmp"}),
        json!({"read":"shutdown"}),
    ]
    .into_iter()
    .enumerate()
    {
        assert_error(&client.call(i as u32 + 2, "au_imports", args));
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }
}

#[test]
fn imports_preserve_scope_envelope_and_recover_after_errors() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let cases = [
        (
            json!({}),
            json!({"read":"imports","id":1}),
            json!({"imports":[{"importer":"app","name":"widget","owner":"base","hash":"a"}],"extra":true}),
            true,
        ),
        (
            json!({"scope":null}),
            json!({"read":"imports","id":1}),
            json!({"imports":[]}),
            true,
        ),
        (
            json!({"scope":"own"}),
            json!({"read":"imports","id":1,"scope":"own"}),
            json!({"imports":[{"importer":"app","owner":"base"}]}),
            true,
        ),
        (
            json!({"scope":"all"}),
            json!({"read":"imports","id":1,"scope":"all"}),
            json!({"imports":null}),
            false,
        ),
        (
            json!({}),
            json!({"read":"imports","id":1}),
            json!({}),
            false,
        ),
        (
            json!({}),
            json!({"read":"imports","id":1}),
            json!({"imports":[]}),
            true,
        ),
    ];
    let expected = cases.clone();
    let daemon = thread::spawn(move || {
        for (_, request, payload, _) in expected {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            assert_eq!(read_request(&mut stream), request);
            let body = serde_json::to_vec(&json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":42,"result":payload})).unwrap();
            stream
                .write_all(&(body.len() as u32).to_be_bytes())
                .unwrap();
            stream.write_all(&body).unwrap();
        }
    });
    let mut client = Client::start(&socket);
    for (i, (args, _, payload, success)) in cases.into_iter().enumerate() {
        let response = client.call(i as u32 + 2, "au_imports", args);
        if success {
            assert_eq!(result_payload(&response), payload);
            assert_eq!(
                response["result"]["_meta"]["io.arsumbris/engineVersion"],
                "42"
            );
        } else {
            assert_error(&response);
            assert!(response["result"].get("_meta").is_none());
        }
    }
    daemon.join().unwrap();
}

#[test]
fn files_invalid_arguments_never_contact_engine() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut client = Client::start(&socket);
    for (i, args) in [
        json!({"repo":""}),
        json!({"repo":1}),
        json!({"scope":"bad"}),
        json!({"limit":0}),
        json!({"limit":501}),
        json!({"limit":null}),
        json!({"offset":-1}),
        json!({"offset":1.5}),
        json!({"path":"/tmp"}),
        json!({"read":"shutdown"}),
    ]
    .into_iter()
    .enumerate()
    {
        assert_error(&client.call(i as u32 + 2, "au_files", args));
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }
}

#[test]
fn files_preserve_paging_scope_order_and_recover_after_bad_envelopes() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let cases = [
        (
            json!({}),
            json!({"read":"files","id":1,"limit":50,"offset":0}),
            json!({"files":[{"path":"/a/x","repo":"fixture"},{"path":"/a x","repo":null}],"extra":true}),
            true,
        ),
        (
            json!({"scope":"own","repo":"fixture","limit":1,"offset":1}),
            json!({"read":"files","id":1,"scope":"own","repo":"fixture","limit":1,"offset":1}),
            json!({"files":[{"path":"/fixture/a.pdf","kind":"asset"}]}),
            true,
        ),
        (
            json!({"scope":"all","limit":500}),
            json!({"read":"files","id":1,"scope":"all","limit":500,"offset":0}),
            json!({"files":null}),
            false,
        ),
        (
            json!({}),
            json!({"read":"files","id":1,"limit":50,"offset":0}),
            json!({}),
            false,
        ),
        (
            json!({"repo":"missing"}),
            json!({"read":"files","id":1,"repo":"missing","limit":50,"offset":0}),
            json!({"files":[]}),
            true,
        ),
    ];
    let expected = cases.clone();
    let daemon = thread::spawn(move || {
        for (_, request, payload, _) in expected {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            assert_eq!(read_request(&mut stream), request);
            let body = serde_json::to_vec(&json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":42,"result":payload})).unwrap();
            stream
                .write_all(&(body.len() as u32).to_be_bytes())
                .unwrap();
            stream.write_all(&body).unwrap();
        }
    });
    let mut client = Client::start(&socket);
    for (i, (args, _, payload, success)) in cases.into_iter().enumerate() {
        let response = client.call(i as u32 + 2, "au_files", args);
        if success {
            assert_eq!(result_payload(&response), payload);
            assert_eq!(
                response["result"]["_meta"]["io.arsumbris/engineVersion"],
                "42"
            );
        } else {
            assert_error(&response);
            assert!(response["result"].get("_meta").is_none());
        }
    }
    daemon.join().unwrap();
}

#[test]
fn content_rejects_invalid_paths_before_connecting() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut client = Client::start(&socket);
    for (i, args) in [
        json!({"repo": "fixture"}),
        json!({"path": "a.md"}),
        json!({"repo": "", "path": "a.md"}),
        json!({"repo": "fixture", "path": ""}),
        json!({"repo": "fixture", "path": "/etc/passwd"}),
        json!({"repo": "fixture", "path": "../outside.md"}),
        json!({"repo": "fixture", "path": "notes/../../outside.md"}),
        json!({"repo": "fixture", "path": 1}),
        json!({"repo": "fixture", "path": "a.md", "scope": "own"}),
    ]
    .into_iter()
    .enumerate()
    {
        assert_error(&client.call(i as u32 + 2, "au_content", args));
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }
}

#[test]
fn content_resolves_the_member_root_and_preserves_envelope() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let daemon = thread::spawn(move || {
        let (mut members, _) = listener.accept().unwrap();
        members
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        assert_eq!(
            read_request(&mut members),
            json!({"read":"members","id":1,"repo":"fixture"})
        );
        let body = serde_json::to_vec(&json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":5,"result":{"members":[{"repo":"fixture","root":"/tmp/ws/fixture","role":"entry"}]}})).unwrap();
        members
            .write_all(&(body.len() as u32).to_be_bytes())
            .unwrap();
        members.write_all(&body).unwrap();

        let (mut content, _) = listener.accept().unwrap();
        content
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        assert_eq!(
            read_request(&mut content),
            json!({"read":"content","id":1,"path":"/tmp/ws/fixture/notes/a.md"})
        );
        let body = serde_json::to_vec(&json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":6,"result":{"content":{"text":"hello","hash":"abc","commit":null}}})).unwrap();
        content
            .write_all(&(body.len() as u32).to_be_bytes())
            .unwrap();
        content.write_all(&body).unwrap();
    });
    let mut client = Client::start(&socket);
    let response = client.call(
        2,
        "au_content",
        json!({"repo":"fixture","path":"notes/a.md"}),
    );
    assert_eq!(
        result_payload(&response),
        json!({"content":{"text":"hello","hash":"abc","commit":null}})
    );
    assert_eq!(
        response["result"]["_meta"]["io.arsumbris/engineVersion"],
        "6"
    );
    daemon.join().unwrap();
}

#[test]
fn content_unknown_member_is_a_tool_error_without_a_content_read() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let (count_tx, count_rx) = std::sync::mpsc::channel();
    let daemon = thread::spawn(move || {
        listener.set_nonblocking(true).unwrap();
        let mut accepted = 0;
        let deadline = std::time::Instant::now() + Duration::from_millis(400);
        loop {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    accepted += 1;
                    stream
                        .set_read_timeout(Some(Duration::from_secs(5)))
                        .unwrap();
                    assert_eq!(
                        read_request(&mut stream),
                        json!({"read":"members","id":1,"repo":"missing"})
                    );
                    let body = serde_json::to_vec(&json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":5,"result":{"members":[]}})).unwrap();
                    stream
                        .write_all(&(body.len() as u32).to_be_bytes())
                        .unwrap();
                    stream.write_all(&body).unwrap();
                }
                Err(_) if std::time::Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(_) => break,
            }
        }
        count_tx.send(accepted).unwrap();
    });
    let mut client = Client::start(&socket);
    let response = client.call(
        2,
        "au_content",
        json!({"repo":"missing","path":"notes/a.md"}),
    );
    assert_error(&response);
    assert!(
        response["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("unknown workspace member"),
        "{response}"
    );
    daemon.join().unwrap();
    assert_eq!(count_rx.recv().unwrap(), 1);
}

#[test]
fn missing_or_array_type_payload_is_rejected() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let daemon = thread::spawn(move || {
        for result in [json!({}), json!({"type":[]})] {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            assert_eq!(
                read_request(&mut stream),
                json!({"read":"type","name":"task","id":1})
            );
            let body = serde_json::to_vec(&json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":1,"result":result})).unwrap();
            stream
                .write_all(&(body.len() as u32).to_be_bytes())
                .unwrap();
            stream.write_all(&body).unwrap();
        }
    });
    let mut client = Client::start(&socket);
    for id in 2..4 {
        assert_error(&client.call(id, "au_type", json!({"name":"task"})));
    }
    daemon.join().unwrap();
}

#[test]
fn instances_of_rejects_invalid_arguments_before_connecting() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut client = Client::start(&socket);
    for (i, args) in [
        json!({}),
        json!({"type_name":""}),
        json!({"type_name":"note","origins":["file","body"]}),
        json!({"type_name":"note","origins":"file"}),
        json!({"type_name":"note","instance":"yes"}),
        json!({"type_name":"note","type":"note"}),
        json!({"type_name":"note","path":"/tmp/x"}),
    ]
    .into_iter()
    .enumerate()
    {
        assert_error(&client.call(i as u32 + 2, "au_instances_of", args));
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }
}

#[test]
fn instances_of_forwards_the_exact_read_and_preserves_the_envelope() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let cases = [
        (
            json!({"type_name":"note"}),
            json!({"read":"instances_of","id":1,"type":"note","instance":false,"body":false}),
            json!({"instances_of":[{"path":"/a.md","claim":"note"},{"path":"/b.md","claim":"note"}]}),
        ),
        (
            json!({"type_name":"note::base","origins":["file"],"instance":true,"body":true}),
            json!({"read":"instances_of","id":1,"type":"note::base","instance":true,"body":true,"origins":["file"]}),
            json!({"instances_of":[]}),
        ),
        (
            json!({"type_name":"note","origins":["nested","meta"]}),
            json!({"read":"instances_of","id":1,"type":"note","instance":false,"body":false,"origins":["nested","meta"]}),
            json!({"instances_of":[{"path":"/c.md","claim":"note"}]}),
        ),
    ];
    let expected = cases.clone();
    let daemon = thread::spawn(move || {
        for (_, request, payload) in expected {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            assert_eq!(read_request(&mut stream), request);
            let body = serde_json::to_vec(&json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":21,"result":payload})).unwrap();
            stream
                .write_all(&(body.len() as u32).to_be_bytes())
                .unwrap();
            stream.write_all(&body).unwrap();
        }
    });
    let mut client = Client::start(&socket);
    for (i, (args, _, payload)) in cases.into_iter().enumerate() {
        let reply = client.call(i as u32 + 2, "au_instances_of", args);
        assert_eq!(result_payload(&reply), payload);
        assert_eq!(reply["result"]["_meta"]["io.arsumbris/engineVersion"], "21");
    }
    daemon.join().unwrap();
}

#[test]
fn validate_value_forwards_the_value_and_reports_a_null_identity() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let verdicts = json!([{"identity":null,"diagnostics":[{"code":"unknown-type-claim","severity":"error"}],"undeclared_fields":[]}]);
    let expected = verdicts.clone();
    let daemon = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        assert_eq!(
            read_request(&mut stream),
            json!({"read":"validate_value","id":1,"type_name":"ghost","value":{"title":"x"}})
        );
        let body = serde_json::to_vec(&json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":22,"result":{"validate_value": expected}})).unwrap();
        stream
            .write_all(&(body.len() as u32).to_be_bytes())
            .unwrap();
        stream.write_all(&body).unwrap();
    });
    let mut client = Client::start(&socket);
    let reply = client.call(
        2,
        "au_validate_value",
        json!({"type_name":"ghost","value":{"title":"x"}}),
    );
    assert_eq!(result_payload(&reply), json!({"validate_value": verdicts}));
    daemon.join().unwrap();
}

#[test]
fn validate_value_rejects_invalid_arguments_before_connecting() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut client = Client::start(&socket);
    for (i, args) in [
        json!({"type_name":"note"}),
        json!({"value":{}}),
        json!({"type_name":"","value":{}}),
        json!({"type_name":"note","value":{},"repo":""}),
        json!({"type_name":"note","value":{},"path":"/tmp"}),
    ]
    .into_iter()
    .enumerate()
    {
        assert_error(&client.call(i as u32 + 2, "au_validate_value", args));
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }
}
