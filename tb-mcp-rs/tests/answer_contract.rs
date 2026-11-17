#![cfg(unix)]

//! The answer contract as MCP prompts and workflow resources: the three workflows
//! substitute their argument, the two workflow resources answer without the engine,
//! `initialize` advertises exactly what the server serves, and a client that never
//! asks for any of it sees a `tools/list` byte-identical to the pinned fixture.

use std::{
    io::{ErrorKind, Write},
    os::unix::net::UnixListener,
    process::{Command, Stdio},
};

use serde_json::{Value, json};

mod support;

use support::Client;

/// A dead socket plus its non-blocking listener: every answer in this file must
/// come from the adapter itself, so each test ends by asserting no engine frame
/// ever arrived.
fn dead_engine() -> (tempfile::TempDir, UnixListener) {
    let temp = tempfile::tempdir().unwrap();
    let listener = UnixListener::bind(temp.path().join("engine.sock")).unwrap();
    listener.set_nonblocking(true).unwrap();
    (temp, listener)
}

fn nothing_reached_the_engine(listener: &UnixListener) {
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        ErrorKind::WouldBlock,
        "these answers must be served without the engine"
    );
}

fn request(client: &mut Client, id: u32, method: &str, params: Value) -> Value {
    client.request(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
}

#[test]
fn initialize_advertises_exactly_the_capabilities_the_server_serves() {
    let (_temp, listener) = dead_engine();
    let mut client = Client::spawn(&_temp.path().join("engine.sock"));
    let init = client.initialize();
    let capabilities = init["result"]["capabilities"].as_object().unwrap();
    let names: Vec<&String> = capabilities.keys().collect();
    assert_eq!(
        names,
        ["prompts", "resources", "tools"],
        "a capability claimed but not served, or one served but hidden: {init}"
    );
    // The prompt catalogue is static for the process's lifetime, so no
    // listChanged is claimed and no notification is ever owed.
    assert!(
        !capabilities["prompts"]["listChanged"]
            .as_bool()
            .unwrap_or(false),
        "{init}"
    );
    drop(client);
    nothing_reached_the_engine(&listener);
}

#[test]
fn prompts_list_names_the_three_workflows_with_one_required_argument_each() {
    let (_temp, listener) = dead_engine();
    let mut client = Client::start(&_temp.path().join("engine.sock"));
    let reply = request(&mut client, 2, "prompts/list", json!({}));
    let prompts = reply["result"]["prompts"].as_array().unwrap();
    assert_eq!(
        prompts
            .iter()
            .map(|p| p["name"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["recall", "intake", "handoff"],
        "{reply}"
    );
    for prompt in prompts {
        assert!(
            !prompt["description"].as_str().unwrap().is_empty(),
            "{prompt}"
        );
        let arguments = prompt["arguments"].as_array().unwrap();
        assert_eq!(arguments.len(), 1, "{prompt}");
        assert_eq!(arguments[0]["required"], json!(true), "{prompt}");
    }
    drop(client);
    nothing_reached_the_engine(&listener);
}

#[test]
fn prompts_get_substitutes_the_argument_and_names_the_tools_that_do_the_work() {
    let (_temp, listener) = dead_engine();
    let mut client = Client::start(&_temp.path().join("engine.sock"));
    let cases = [
        (
            "recall",
            "question",
            "why does the intake family route through hubs",
            "tb_search",
        ),
        (
            "intake",
            "record",
            "2026-11-17: the gate checks freshness before editability",
            "tb_diagnostics",
        ),
        (
            "handoff",
            "state",
            "mid-refactor: the search floor moved, cascade unverified",
            "tb_reconcile",
        ),
    ];
    for (index, (name, argument, sample, grounded_tool)) in cases.into_iter().enumerate() {
        let reply = request(
            &mut client,
            10 + index as u32,
            "prompts/get",
            json!({"name": name, "arguments": {(argument): sample}}),
        );
        let messages = reply["result"]["messages"].as_array().unwrap();
        assert!(!messages.is_empty(), "{reply}");
        assert_eq!(messages[0]["role"], "user", "{reply}");
        let text = messages[0]["content"]["text"].as_str().unwrap();
        assert!(text.contains(sample), "{name}: {text}");
        assert!(
            !text.contains(&format!("{{{argument}}}")),
            "a placeholder survived substitution: {text}"
        );
        assert!(
            text.contains(grounded_tool),
            "{name} must name real tools: {text}"
        );
    }
    drop(client);
    nothing_reached_the_engine(&listener);
}

#[test]
fn a_missing_argument_and_an_unknown_prompt_are_refusals_that_name_the_way_out() {
    let (_temp, listener) = dead_engine();
    let mut client = Client::start(&_temp.path().join("engine.sock"));
    let reply = request(&mut client, 2, "prompts/get", json!({"name": "recall"}));
    let message = reply["error"]["message"].as_str().unwrap();
    assert!(message.contains("question"), "{reply}");
    let reply = request(&mut client, 3, "prompts/get", json!({"name": "fortune"}));
    let message = reply["error"]["message"].as_str().unwrap();
    assert!(message.contains("prompts/list"), "{reply}");
    drop(client);
    nothing_reached_the_engine(&listener);
}

#[test]
fn resources_read_serves_the_workflow_texts_without_the_engine() {
    let (_temp, listener) = dead_engine();
    let mut client = Client::start(&_temp.path().join("engine.sock"));
    let answer_shape = request(
        &mut client,
        2,
        "resources/read",
        json!({"uri": "2brain://workflow/answer-shape"}),
    );
    let contents = answer_shape["result"]["contents"].as_array().unwrap();
    assert_eq!(contents[0]["mimeType"], "text/markdown", "{answer_shape}");
    let text = contents[0]["text"].as_str().unwrap();
    for part in [
        "Direct answer",
        "Backing, cited",
        "Tensions",
        "Sources consulted",
        "Honesty",
    ] {
        assert!(text.contains(part), "answer-shape lost {part}: {text}");
    }
    // The legacy scheme stays accepted on read, the same one-release window the
    // member resources document.
    let legacy = request(
        &mut client,
        3,
        "resources/read",
        json!({"uri": "arsumbris://workflow/answer-shape"}),
    );
    assert_eq!(
        legacy["result"]["contents"][0]["text"], contents[0]["text"],
        "{legacy}"
    );

    let write_gates = request(
        &mut client,
        4,
        "resources/read",
        json!({"uri": "2brain://workflow/write-gates"}),
    );
    let text = write_gates["result"]["contents"][0]["text"]
        .as_str()
        .unwrap();
    for part in [
        "policy/<stage>",
        "prior_read",
        "expected_hash",
        "member_grant",
        "absence",
        "tb_reconcile",
    ] {
        assert!(text.contains(part), "write-gates lost {part}: {text}");
    }
    drop(client);
    nothing_reached_the_engine(&listener);
}

#[test]
fn an_unknown_workflow_uri_is_refused_without_reaching_the_engine() {
    let (_temp, listener) = dead_engine();
    let mut client = Client::start(&_temp.path().join("engine.sock"));
    for (id, uri) in [
        (2, "2brain://workflow/nothing-by-this-name"),
        (3, "2brain://workflow/"),
    ] {
        let reply = request(&mut client, id, "resources/read", json!({"uri": uri}));
        assert!(reply["error"].is_object(), "{reply}");
    }
    drop(client);
    nothing_reached_the_engine(&listener);
}

#[test]
fn resources_list_carries_the_workflows_ahead_of_the_catalogue() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let daemon = std::thread::spawn(move || {
        let (mut files, _) = listener.accept().unwrap();
        assert_eq!(
            support::read_request(&mut files),
            json!({"read":"files","id":1,"limit":100,"offset":0})
        );
        support::write_frame(
            &mut files,
            json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":4,
                "result":{"files":[
                    {"path":"/tmp/ws/notes/a.md","stem":"a","repo":"fixture","kind":"note"},
                    {"path":"/tmp/ws/notes/b c.md","stem":"b c","repo":"fixture","kind":"note"}]}}),
        );
        let (mut members, _) = listener.accept().unwrap();
        assert_eq!(
            support::read_request(&mut members),
            json!({"read":"members","id":1})
        );
        support::write_frame(&mut members, support::members_frame("/tmp/ws", 3, true));
    });
    let mut client = Client::start(&socket);
    let reply = request(&mut client, 2, "resources/list", json!({}));
    let resources = reply["result"]["resources"].as_array().unwrap();
    assert_eq!(resources.len(), 4, "{reply}");
    assert_eq!(resources[0]["uri"], "2brain://workflow/answer-shape");
    assert_eq!(resources[1]["uri"], "2brain://workflow/write-gates");
    assert_eq!(resources[2]["uri"], "2brain://fixture/notes/a.md");
    assert_eq!(
        resources[3]["uri"], "2brain://fixture/notes/b%20c.md",
        "the catalogue's own URI shape must not change behind the workflow head"
    );
    assert!(reply["result"].get("nextCursor").is_none(), "{reply}");
    daemon.join().unwrap();
}

#[test]
fn a_full_catalogue_page_still_hands_back_a_cursor() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let files: Vec<Value> = (0..100)
        .map(|index| {
            json!({"path": format!("/tmp/ws/n{index}.md"), "stem": format!("n{index}"), "repo": "fixture"})
        })
        .collect();
    let daemon = std::thread::spawn(move || {
        let (mut files_stream, _) = listener.accept().unwrap();
        assert_eq!(
            support::read_request(&mut files_stream),
            json!({"read":"files","id":1,"limit":100,"offset":0})
        );
        support::write_frame(
            &mut files_stream,
            json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":4,
                "result":{"files": files}}),
        );
        let (mut members, _) = listener.accept().unwrap();
        assert_eq!(
            support::read_request(&mut members),
            json!({"read":"members","id":1})
        );
        support::write_frame(&mut members, support::members_frame("/tmp/ws", 3, true));
    });
    let mut client = Client::start(&socket);
    let reply = request(&mut client, 2, "resources/list", json!({}));
    let resources = reply["result"]["resources"].as_array().unwrap();
    assert_eq!(
        resources.len(),
        102,
        "two workflow entries plus a full page"
    );
    assert_eq!(reply["result"]["nextCursor"], json!("100"), "{reply}");
    daemon.join().unwrap();
}

/// The raw-line pin: `tools/list` on a launch that never asks for prompts or
/// workflow resources is byte-identical to the pre-answer-contract surface.
///
/// When the tool surface changes ON PURPOSE, regenerate the fixture by driving
/// the built binary over stdio (initialize, then tools/list with id 2) and
/// replacing `tests/fixtures/tools-list-concise.json` with the whole reply line,
/// and record the reason in the change's commit message - the same discipline as
/// the engine's golden wire contract.
#[test]
fn tools_list_bytes_are_pinned() {
    let expected = include_str!("fixtures/tools-list-concise.json");
    let temp = tempfile::tempdir().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_2brain"))
        .arg("mcp")
        .arg(temp.path().join("engine.sock"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    writeln!(
        stdin,
        "{}",
        json!({"jsonrpc":"2.0","id":1,"method":"initialize",
            "params":{"protocolVersion":"2025-06-18","capabilities":{},
            "clientInfo":{"name":"byte-pin","version":"1"}}})
    )
    .unwrap();
    writeln!(
        stdin,
        "{}",
        json!({"jsonrpc":"2.0","method":"notifications/initialized"})
    )
    .unwrap();
    writeln!(
        stdin,
        "{}",
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}})
    )
    .unwrap();
    stdin.flush().unwrap();
    drop(stdin);
    let stdout = std::io::BufRead::lines(std::io::BufReader::new(child.stdout.take().unwrap()));
    let mut tools_line = None;
    for line in stdout {
        let line = line.unwrap();
        if line.contains("\"tools\"") && line.contains("\"id\":2") {
            tools_line = Some(line);
        }
    }
    child.wait().unwrap();
    let tools_line = tools_line.expect("tools/list reply on stdout");
    assert_eq!(
        tools_line, *expected,
        "tools/list bytes drifted; if the drift is intentional, regenerate \
         tests/fixtures/tools-list-concise.json and say so in the commit message"
    );
}
