#![cfg(unix)]

//! The scan and resolution reads: `instances` (paged), `candidates`,
//! `semantic_tokens`, `resolve_member`.

use std::{os::unix::net::UnixListener, thread, time::Duration};

use serde_json::{Value, json};

mod support;

use support::{Client, payload, read_request, write_response_frame};

const TIMEOUT: Duration = Duration::from_secs(10);
const ROOT: &str = "/tmp/fixture";

fn serve(
    answers: Vec<(Option<Value>, Value)>,
) -> (
    tempfile::TempDir,
    std::path::PathBuf,
    thread::JoinHandle<()>,
) {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let handle = thread::spawn(move || {
        for (expected, answer) in answers {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(TIMEOUT)).unwrap();
            let request = read_request(&mut stream);
            if let Some(expected) = expected {
                assert_eq!(request, expected, "wire request");
            }
            write_response_frame(&mut stream, 4, answer);
        }
    });
    (temp, socket, handle)
}

#[test]
fn instances_pages_the_scan_and_keeps_the_count() {
    let records: Vec<Value> = (0..5)
        .map(|i| json!({"file": format!("/f{i}.md"),"claim":["note"]}))
        .collect();
    let (temp, socket, handle) = serve(vec![(
        Some(json!({"read":"instances","id":1})),
        json!({"instances": records, "count": 5, "aborted_at_load": false}),
    )]);
    let mut client = Client::start_with_timeout(&socket, &[], TIMEOUT);
    let result = client.call_result(2, "tb_instances", json!({"limit":2,"offset":0}));
    assert_ne!(result["isError"], true, "{result}");
    let payload = payload(&result);
    assert_eq!(payload["instances"].as_array().unwrap().len(), 2);
    assert_eq!(payload["total"], 5);
    assert_eq!(payload["next_offset"], 2);
    // The engine's own envelope metadata survives the page.
    assert_eq!(payload["count"], 5);
    assert_eq!(payload["aborted_at_load"], false);
    drop(client);
    handle.join().unwrap();
    drop(temp);
}

#[test]
fn candidates_forwards_summary_and_paging() {
    let (temp, socket, handle) = serve(vec![(
        Some(json!({"read":"candidates","id":1,"summary":true,"limit":10,"offset":5})),
        json!({"candidates":[{"file":"/a.md","candidates":["note"]}], "aborted_at_load": false}),
    )]);
    let mut client = Client::start_with_timeout(&socket, &[], TIMEOUT);
    let result = client.call_result(
        2,
        "tb_candidates",
        json!({"summary":true,"limit":10,"offset":5}),
    );
    assert_ne!(result["isError"], true, "{result}");
    assert_eq!(payload(&result)["candidates"].as_array().unwrap().len(), 1);
    drop(client);
    handle.join().unwrap();
    drop(temp);
}

#[test]
fn semantic_tokens_and_resolve_member_resolve_the_member_root() {
    for (tool, verb, key, answer) in [
        (
            "tb_semantic_tokens",
            "semantic_tokens",
            "semantic_tokens",
            json!({"semantic_tokens":[{"kind":"anchor","text":"Details"}]}),
        ),
        (
            "tb_resolve_member",
            "resolve_member",
            "resolve_member",
            json!({"resolve_member":{"repo":"fixture","root":ROOT,"editable":true,"local":true,"role":"entry"}}),
        ),
    ] {
        let (temp, socket, handle) = serve(vec![
            (
                Some(json!({"read":"members","id":1,"repo":"fixture"})),
                json!({"members":[{"repo":"fixture","editable":true,"root":ROOT}]}),
            ),
            (
                Some(json!({"read":verb,"id":1,"path":"/tmp/fixture/notes/a.md"})),
                answer.clone(),
            ),
        ]);
        let mut client = Client::start_with_timeout(&socket, &[], TIMEOUT);
        let result = client.call_result(2, tool, json!({"repo":"fixture","path":"notes/a.md"}));
        assert_ne!(result["isError"], true, "{tool}: {result}");
        let payload = payload(&result);
        assert!(payload.get(key).is_some(), "{tool}: {payload}");
        drop(client);
        handle.join().unwrap();
        drop(temp);
    }
}

#[test]
fn typed_classifies_an_instance_and_falls_back_to_resolve_target() {
    // An instance: one read, no fallback.
    let (temp, socket, handle) = serve(vec![
        (
            Some(json!({"read":"members","id":1,"repo":"fixture"})),
            json!({"members":[{"repo":"fixture","editable":true,"root":ROOT}]}),
        ),
        (
            Some(json!({"read":"instance","id":1,"path":"/tmp/fixture/notes/a.md"})),
            json!({"instance":{"resolved":true,"claim":["note"],"closure":["note"]}}),
        ),
    ]);
    let mut client = Client::start_with_timeout(&socket, &[], TIMEOUT);
    let result = client.call_result(2, "tb_typed", json!({"repo":"fixture","path":"notes/a.md"}));
    assert_ne!(result["isError"], true, "{result}");
    let classified = payload(&result);
    assert_eq!(classified["typed"], true);
    assert_eq!(classified["kind"], "instance");
    assert_eq!(classified["claim"], json!(["note"]));
    guard(handle, temp);

    // A plain note: `instance` is null, then the local resolve form names the kind.
    let (temp, socket, handle) = serve(vec![
        (
            Some(json!({"read":"members","id":1,"repo":"fixture"})),
            json!({"members":[{"repo":"fixture","editable":true,"root":ROOT}]}),
        ),
        (
            Some(json!({"read":"instance","id":1,"path":"/tmp/fixture/notes/plain.md"})),
            json!({"instance":null}),
        ),
        (
            Some(
                json!({"read":"resolve_target","id":1,"target":"","origin":"/tmp/fixture/notes/plain.md"}),
            ),
            json!({"resolve_target":{"path":"/tmp/fixture/notes/plain.md","kind":"unclassified"}}),
        ),
    ]);
    let mut client = Client::start_with_timeout(&socket, &[], TIMEOUT);
    let result = client.call_result(
        2,
        "tb_typed",
        json!({"repo":"fixture","path":"notes/plain.md"}),
    );
    assert_ne!(result["isError"], true, "{result}");
    let classified = payload(&result);
    assert_eq!(classified["typed"], false);
    assert_eq!(classified["kind"], "unclassified");
    guard(handle, temp);
}

#[test]
fn resolved_and_children_read_their_own_verbs() {
    let (temp, socket, handle) = serve(vec![
        (
            Some(json!({"read":"members","id":1,"repo":"fixture"})),
            json!({"members":[{"repo":"fixture","editable":true,"root":ROOT}]}),
        ),
        (
            Some(json!({"read":"instance","id":1,"path":"/tmp/fixture/notes/a.md"})),
            json!({"instance":{"resolved":true,"claim":["note"]}}),
        ),
    ]);
    let mut client = Client::start_with_timeout(&socket, &[], TIMEOUT);
    let result = client.call_result(
        2,
        "tb_resolved",
        json!({"repo":"fixture","path":"notes/a.md"}),
    );
    assert_ne!(result["isError"], true, "{result}");
    assert_eq!(payload(&result)["instance"]["claim"], json!(["note"]));
    guard(handle, temp);

    let (temp, socket, handle) = serve(vec![
        (
            Some(json!({"read":"members","id":1,"repo":"fixture"})),
            json!({"members":[{"repo":"fixture","editable":true,"root":ROOT}]}),
        ),
        (
            Some(json!({"read":"dir_entries","id":1,"dir":"/tmp/fixture/notes"})),
            json!({"dir_entries":[{"path":"/tmp/fixture/notes/a.md","name":"a.md","kind":"file"}]}),
        ),
    ]);
    let mut client = Client::start_with_timeout(&socket, &[], TIMEOUT);
    let result = client.call_result(2, "tb_children", json!({"repo":"fixture","path":"notes"}));
    assert_ne!(result["isError"], true, "{result}");
    assert_eq!(payload(&result)["dir_entries"].as_array().unwrap().len(), 1);
    guard(handle, temp);
}

#[test]
fn follow_resolves_a_target_and_peeks_the_line() {
    let (temp, socket, handle) = serve(vec![
        (
            Some(json!({"read":"members","id":1,"repo":"fixture"})),
            json!({"members":[{"repo":"fixture","editable":true,"root":ROOT}]}),
        ),
        (
            Some(
                json!({"read":"resolve_target","id":1,"target":"notes/b","origin":"/tmp/fixture/notes/a.md"}),
            ),
            json!({"resolve_target":{"path":"/tmp/fixture/notes/b.md","kind":"instance","hash":"h","source":null}}),
        ),
    ]);
    let mut client = Client::start_with_timeout(&socket, &[], TIMEOUT);
    let result = client.call_result(
        2,
        "tb_follow",
        json!({"repo":"fixture","origin_path":"notes/a.md","target":"notes/b"}),
    );
    assert_ne!(result["isError"], true, "{result}");
    let payload = payload(&result)["follow"].clone();
    assert_eq!(payload["resolved"], true);
    assert_eq!(payload["path"], "/tmp/fixture/notes/b.md");
    guard(handle, temp);
}

#[test]
fn follow_asks_the_fragment_verb_for_a_fragment() {
    for (target, verb, extra, answer) in [
        (
            "notes/b^blk",
            "resolve_block_id",
            json!({"block_id":"blk"}),
            json!({"resolve_block_id":{"file_path":"/tmp/fixture/notes/b.md","kind":"marker","span":{"start":0,"end":4,"line_col":{"start":{"line":2,"col":1},"end":{"col":5,"line":2}}},"type_claim":""}}),
        ),
        (
            "notes/b#Head",
            "resolve_anchor",
            json!({"anchor":"Head"}),
            json!({"resolve_anchor":{"file_path":"/tmp/fixture/notes/b.md","span":{"start":0,"end":6,"line_col":{"start":{"line":1,"col":1},"end":{"col":6,"line":1}}}}}),
        ),
    ] {
        let mut expected =
            json!({"read":verb,"id":1,"target":"notes/b","origin":"/tmp/fixture/notes/a.md"});
        for (k, v) in extra.as_object().unwrap() {
            expected[k] = v.clone();
        }
        let (temp, socket, handle) = serve(vec![
            (
                Some(json!({"read":"members","id":1,"repo":"fixture"})),
                json!({"members":[{"repo":"fixture","editable":true,"root":ROOT}]}),
            ),
            (Some(expected), answer),
            (
                Some(json!({"read":"content","id":1,"path":"/tmp/fixture/notes/b.md"})),
                json!({"content":{"text":"first\nsecond\nthird","hash":"h","commit":null}}),
            ),
        ]);
        let mut client = Client::start_with_timeout(&socket, &[], TIMEOUT);
        let result = client.call_result(
            2,
            "tb_follow",
            json!({"repo":"fixture","origin_path":"notes/a.md","target":target}),
        );
        assert_ne!(result["isError"], true, "{target}: {result}");
        let payload = payload(&result)["follow"].clone();
        assert_eq!(payload["resolved"], true, "{target}: {payload}");
        assert!(payload["snippet"].is_string(), "{target}: {payload}");
        guard(handle, temp);
    }
}

#[test]
fn follow_skips_the_peek_when_disabled() {
    let (temp, socket, handle) = serve(vec![
        (
            Some(json!({"read":"members","id":1,"repo":"fixture"})),
            json!({"members":[{"repo":"fixture","editable":true,"root":ROOT}]}),
        ),
        (
            Some(
                json!({"read":"resolve_target","id":1,"target":"notes/b","origin":"/tmp/fixture/notes/a.md"}),
            ),
            json!({"resolve_target":null}),
        ),
    ]);
    let mut client = Client::start_with_timeout(&socket, &[], TIMEOUT);
    let result = client.call_result(
        2,
        "tb_follow",
        json!({"repo":"fixture","origin_path":"notes/a.md","target":"notes/b","peek":false}),
    );
    let payload = payload(&result)["follow"].clone();
    assert_eq!(payload["resolved"], false);
    guard(handle, temp);
}

fn guard(handle: thread::JoinHandle<()>, temp: tempfile::TempDir) {
    handle.join().unwrap();
    drop(temp);
}
