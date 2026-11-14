#![cfg(unix)]

//! The graph and addressability reads: `frontmatter`, `anchors`, `block_ids`,
//! `references`, `neighborhood`. Each must send the exact wire request and pass
//! the engine payload through.

use std::{os::unix::net::UnixListener, thread, time::Duration};

use serde_json::{Value, json};

mod support;

use support::{Client, payload, read_request, write_response_frame};

const TIMEOUT: Duration = Duration::from_secs(10);
const ROOT: &str = "/tmp/fixture";

/// Serve `members` first (every path-addressed read resolves the root), then the
/// expected read, asserting the exact wire request for the second.
fn serve(
    expected: Value,
    answer: Value,
) -> (
    tempfile::TempDir,
    std::path::PathBuf,
    thread::JoinHandle<()>,
) {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream.set_read_timeout(Some(TIMEOUT)).unwrap();
        let members = read_request(&mut stream);
        assert_eq!(
            members,
            json!({"read":"members","id":1,"repo":"fixture"}),
            "members first"
        );
        write_response_frame(
            &mut stream,
            9,
            json!({"members":[{"repo":"fixture","editable":true,"root":ROOT}]}),
        );
        let (mut stream, _) = listener.accept().unwrap();
        stream.set_read_timeout(Some(TIMEOUT)).unwrap();
        assert_eq!(read_request(&mut stream), expected, "wire request");
        write_response_frame(&mut stream, 9, answer);
    });
    (temp, socket, handle)
}

#[test]
fn frontmatter_sends_the_absolute_path_and_passes_the_map() {
    let answer = json!({"frontmatter": {"type":"note","title":"x"}});
    let (temp, socket, handle) = serve(
        json!({"read":"frontmatter","id":1,"path":"/tmp/fixture/notes/a.md"}),
        answer.clone(),
    );
    let mut client = Client::start_with_timeout(&socket, &[], TIMEOUT);
    let result = client.call_result(
        2,
        "au_frontmatter",
        json!({"repo":"fixture","path":"notes/a.md"}),
    );
    assert_ne!(result["isError"], true, "{result}");
    assert_eq!(payload(&result), answer);
    drop(client);
    handle.join().unwrap();
    drop(temp);
}

#[test]
fn anchors_and_block_ids_address_the_target() {
    for (tool, verb, answer) in [
        (
            "au_anchors",
            "anchors",
            json!({"anchors":[{"text":"Head","level":1,"span":{"start":0,"end":6}}]}),
        ),
        (
            "au_block_ids",
            "block_ids",
            json!({"block_ids":[{"id":"intro","kind":"marker","type_claim":"","span":{"start":9,"end":15}}]}),
        ),
    ] {
        let (temp, socket, handle) = serve(
            json!({"read":verb,"id":1,"target":"","origin":"/tmp/fixture/notes/a.md"}),
            answer.clone(),
        );
        let mut client = Client::start_with_timeout(&socket, &[], TIMEOUT);
        let result = client.call_result(2, tool, json!({"repo":"fixture","path":"notes/a.md"}));
        assert_ne!(result["isError"], true, "{tool}: {result}");
        assert_eq!(payload(&result), answer, "{tool}");
        drop(client);
        handle.join().unwrap();
        drop(temp);
    }
}

#[test]
fn references_reads_both_directions_and_keeps_the_two_keys() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let out = json!({"references_out":[{"target":"[[b]]","resolved":"/tmp/fixture/b.md","kind":"navigational"}]});
    let inb = json!({"references_in":[{"target":"[[a]]","resolved":"/tmp/fixture/a.md","kind":"navigational"}]});
    let expected_out = out.clone();
    let expected_in = inb.clone();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream.set_read_timeout(Some(TIMEOUT)).unwrap();
        assert_eq!(read_request(&mut stream)["read"], "members");
        write_response_frame(
            &mut stream,
            9,
            json!({"members":[{"repo":"fixture","editable":true,"root":ROOT}]}),
        );
        let (mut stream, _) = listener.accept().unwrap();
        assert_eq!(
            read_request(&mut stream),
            json!({"read":"references_out","id":1,"path":"/tmp/fixture/notes/a.md"})
        );
        write_response_frame(&mut stream, 9, expected_out);
        let (mut stream, _) = listener.accept().unwrap();
        assert_eq!(
            read_request(&mut stream),
            json!({"read":"references_in","id":1,"path":"/tmp/fixture/notes/a.md"})
        );
        write_response_frame(&mut stream, 9, expected_in);
    });
    let mut client = Client::start_with_timeout(&socket, &[], TIMEOUT);
    let result = client.call_result(
        2,
        "au_references",
        json!({"repo":"fixture","path":"notes/a.md"}),
    );
    assert_ne!(result["isError"], true, "{result}");
    let payload = payload(&result);
    assert_eq!(
        payload,
        json!({"references_out": out["references_out"], "references_in": inb["references_in"]})
    );
    drop(client);
    handle.join().unwrap();
    drop(temp);
}

#[test]
fn references_direction_out_reads_once() {
    let (temp, socket, handle) = serve(
        json!({"read":"references_out","id":1,"path":"/tmp/fixture/notes/a.md"}),
        json!({"references_out":[]}),
    );
    let mut client = Client::start_with_timeout(&socket, &[], TIMEOUT);
    let result = client.call_result(
        2,
        "au_references",
        json!({"repo":"fixture","path":"notes/a.md","direction":"out"}),
    );
    assert_eq!(payload(&result), json!({"references_out": []}));
    drop(client);
    handle.join().unwrap();
    drop(temp);
}

#[test]
fn neighborhood_forwards_the_walk_arguments() {
    let (temp, socket, handle) = serve(
        json!({"read":"neighborhood","id":1,"path":"/tmp/fixture/notes/a.md",
               "direction":"both","depth":2,"kinds":["navigational"],"scope":"own","max_nodes":50}),
        json!({"neighborhood":{"nodes":[],"edges":[],"truncated":false}}),
    );
    let mut client = Client::start_with_timeout(&socket, &[], TIMEOUT);
    let result = client.call_result(
        2,
        "au_neighborhood",
        json!({"repo":"fixture","path":"notes/a.md","direction":"both","depth":2,
               "kinds":["navigational"],"scope":"own","max_nodes":50}),
    );
    assert_ne!(result["isError"], true, "{result}");
    let payload = payload(&result);
    assert_eq!(payload["neighborhood"]["truncated"], false);
    drop(client);
    handle.join().unwrap();
    drop(temp);
}

#[test]
fn the_new_reads_reject_paths_before_connecting() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut client = Client::start_with_timeout(&socket, &[], TIMEOUT);
    for (index, tool) in [
        "au_frontmatter",
        "au_anchors",
        "au_block_ids",
        "au_references",
        "au_neighborhood",
    ]
    .into_iter()
    .enumerate()
    {
        for bad in ["/etc/passwd", "../x.md"] {
            let result = client.call_result(
                index as u32 + 10,
                tool,
                json!({"repo":"fixture","path":bad}),
            );
            assert_eq!(result["isError"], true, "{tool} {bad}: {result}");
        }
        let empty =
            client.call_result(index as u32 + 20, tool, json!({"repo":"fixture","path":""}));
        assert_eq!(empty["isError"], true, "{tool}: {empty}");
    }
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}
