#![cfg(unix)]

//! MCP resources: the catalogue as addressable URIs, and reading one back.

use std::{os::unix::net::UnixListener, thread};

use serde_json::json;

mod support;

use support::{Client, members_frame, read_request, write_frame};

#[test]
fn initialize_advertises_resources_without_subscription() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    UnixListener::bind(&socket).unwrap();
    let mut client = Client::spawn(&socket);
    let init = client.initialize();
    let capabilities = &init["result"]["capabilities"];
    assert!(capabilities["tools"].is_object(), "{init}");
    assert!(capabilities["resources"].is_object(), "{init}");
    // Absent and false mean the same thing on the wire; what must never happen
    // is claiming a subscription this server does not serve.
    assert!(
        !capabilities["resources"]["subscribe"]
            .as_bool()
            .unwrap_or(false)
    );
    drop(client);
}

#[test]
fn list_resources_returns_member_scoped_uris() {
    let temp = tempfile::tempdir().unwrap();
    let root = "/tmp/ws";
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let daemon = thread::spawn(move || {
        let (mut files, _) = listener.accept().unwrap();
        assert_eq!(
            read_request(&mut files),
            json!({"read":"files","id":1,"limit":100,"offset":0})
        );
        write_frame(
            &mut files,
            json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":4,
                "result":{"files":[
                    {"path":"/tmp/ws/notes/a.md","stem":"a","repo":"fixture","kind":"note"},
                    {"path":"/tmp/ws/notes/b c.md","stem":"b c","repo":"fixture","kind":"note"}]}}),
        );
        let (mut members, _) = listener.accept().unwrap();
        assert_eq!(read_request(&mut members), json!({"read":"members","id":1}));
        write_frame(&mut members, members_frame(root, 3, true));
    });
    let mut client = Client::start(&socket);
    let reply =
        client.request(json!({"jsonrpc":"2.0","id":2,"method":"resources/list","params":{}}));
    let resources = reply["result"]["resources"].as_array().unwrap();
    // Two static workflow resources sit ahead of the catalogue on the first page;
    // the member-scoped shape below them is what this test pins.
    assert_eq!(resources.len(), 4, "{reply}");
    assert_eq!(resources[0]["uri"], "2brain://workflow/answer-shape");
    assert_eq!(resources[1]["uri"], "2brain://workflow/write-gates");
    assert_eq!(resources[2]["uri"], "2brain://fixture/notes/a.md");
    assert_eq!(resources[2]["mimeType"], "text/markdown");
    assert_eq!(
        resources[3]["uri"], "2brain://fixture/notes/b%20c.md",
        "a space in a file name must not break the URI"
    );
    daemon.join().unwrap();
}

#[test]
fn read_resource_returns_the_engine_text() {
    let temp = tempfile::tempdir().unwrap();
    let root = "/tmp/ws";
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let daemon = thread::spawn(move || {
        let (mut members, _) = listener.accept().unwrap();
        assert_eq!(
            read_request(&mut members),
            json!({"read":"members","id":1,"repo":"fixture"})
        );
        write_frame(&mut members, members_frame(root, 3, true));
        let (mut content, _) = listener.accept().unwrap();
        assert_eq!(
            read_request(&mut content),
            json!({"read":"content","id":1,"path":"/tmp/ws/notes/a.md"})
        );
        write_frame(
            &mut content,
            json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":5,
                "result":{"content":{"text":"# a\n","hash":"h1","commit":null}}}),
        );
    });
    let mut client = Client::start(&socket);
    let reply = client.request(json!({
        "jsonrpc":"2.0","id":2,"method":"resources/read",
        "params":{"uri":"arsumbris://fixture/notes/a.md"}}));
    let contents = reply["result"]["contents"].as_array().unwrap();
    assert_eq!(contents[0]["text"], "# a\n", "{reply}");
    assert_eq!(contents[0]["mimeType"], "text/markdown");
    daemon.join().unwrap();
}

#[test]
fn a_foreign_uri_is_refused_before_the_engine() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut client = Client::start(&socket);
    let reply = client.request(json!({
        "jsonrpc":"2.0","id":2,"method":"resources/read",
        "params":{"uri":"file:///etc/passwd"}}));
    assert!(reply["error"].is_object(), "{reply}");
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}
