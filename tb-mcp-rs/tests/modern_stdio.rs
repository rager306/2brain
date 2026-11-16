use std::{
    io::{Read, Write},
    os::unix::net::UnixListener,
    thread,
};

use serde_json::{Value, json};

mod support;

use support::Client;

const VERSION: &str = "2026-07-28";

fn request(id: u32, method: &str, params: Value) -> Value {
    let mut params = params.as_object().unwrap().clone();
    params.insert(
        "_meta".into(),
        json!({
            "io.modelcontextprotocol/protocolVersion": VERSION,
            "io.modelcontextprotocol/clientCapabilities": {},
            "io.modelcontextprotocol/clientInfo": {"name": "modern-test", "version": "1"}
        }),
    );
    json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
}

fn exchange(client: &mut Client, request: Value) -> Value {
    let response = client.request(request.clone());
    assert_eq!(response["id"], request["id"]);
    response
}

#[test]
fn modern_first_ping_obeys_version_and_removed_method_rules() {
    let temp = tempfile::tempdir().unwrap();
    let mut client = Client::spawn(&temp.path().join("unused.sock"));
    let mut unknown_ping = request(10, "ping", json!({}));
    unknown_ping["params"]["_meta"]["io.modelcontextprotocol/protocolVersion"] =
        json!("2099-01-01");
    let response = exchange(&mut client, unknown_ping);
    assert_eq!(response["error"]["code"], -32022);
    let response = exchange(&mut client, request(11, "ping", json!({})));
    assert_eq!(response["error"]["code"], -32601);
}

// The production server currently advertises only tested legacy versions.
// Enable this test when the server enables and validates 2026-07-28.
#[test]
fn modern_discovery_and_direct_requests_without_initialize() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let daemon = thread::spawn(move || {
        for (verb, payload) in [
            ("overview", json!({"members":[]})),
            ("top_level_dirs", json!([])),
        ] {
            let (mut stream, _) = listener.accept().unwrap();
            let mut len = [0; 4];
            stream.read_exact(&mut len).unwrap();
            let mut bytes = vec![0; u32::from_be_bytes(len) as usize];
            stream.read_exact(&mut bytes).unwrap();
            assert_eq!(
                serde_json::from_slice::<Value>(&bytes).unwrap(),
                json!({"read":verb,"id":1})
            );
            let response = serde_json::to_vec(&json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":7,"result":{verb:payload}})).unwrap();
            stream
                .write_all(&(response.len() as u32).to_be_bytes())
                .unwrap();
            stream.write_all(&response).unwrap();
        }
    });

    let mut discover_client = Client::spawn(&socket);
    let discover = exchange(
        &mut discover_client,
        request(1, "server/discover", json!({})),
    );
    assert_eq!(discover["result"]["resultType"], "complete");
    assert!(
        discover["result"]["supportedVersions"]
            .as_array()
            .unwrap()
            .contains(&json!(VERSION))
    );
    assert!(discover["result"]["capabilities"]["tools"].is_object());
    assert_eq!(
        discover["result"]["_meta"]["io.modelcontextprotocol/serverInfo"]["name"],
        // The product name, not the crate name: clients see `2brain`.
        "2brain"
    );

    // A fresh process receives tools/list as its first message, without discovery or initialize.
    let mut client = Client::spawn(&socket);
    let list = exchange(&mut client, request(2, "tools/list", json!({})));
    assert_eq!(list["result"]["resultType"], "complete");
    let tools = list["result"]["tools"].as_array().unwrap();
    for name in ["tb_overview", "tb_top_level_dirs"] {
        let tool = tools.iter().find(|tool| tool["name"] == name).unwrap();
        assert_eq!(tool["inputSchema"]["additionalProperties"], false);
        assert_eq!(tool["annotations"]["readOnlyHint"], true);
    }
    for (id, name, payload) in [
        (3, "tb_overview", json!({"overview":{"members":[]}})),
        (4, "tb_top_level_dirs", json!({"top_level_dirs":[]})),
    ] {
        let response = exchange(
            &mut client,
            request(id, "tools/call", json!({"name":name,"arguments":{}})),
        );
        assert_eq!(response["result"]["resultType"], "complete");
        assert_ne!(response["result"]["isError"], true);
        assert_eq!(
            serde_json::from_str::<Value>(
                response["result"]["content"][0]["text"].as_str().unwrap()
            )
            .unwrap(),
            payload
        );
    }
    let denied = exchange(
        &mut client,
        request(5, "tools/call", json!({"name":"shutdown","arguments":{}})),
    );
    assert!(denied.get("error").is_some() || denied["result"]["isError"] == true);
    let invalid = exchange(
        &mut client,
        request(
            6,
            "tools/call",
            json!({"name":"tb_overview","arguments":{"extra":true}}),
        ),
    );
    assert!(invalid.get("error").is_some() || invalid["result"]["isError"] == true);
    let mut unsupported = request(7, "tools/list", json!({}));
    unsupported["params"]["_meta"]["io.modelcontextprotocol/protocolVersion"] = json!("2099-01-01");
    let response = exchange(&mut client, unsupported);
    assert_eq!(response["error"]["code"], -32022);
    assert_eq!(response["error"]["data"]["requested"], "2099-01-01");
    assert!(
        response["error"]["data"]["supported"]
            .as_array()
            .unwrap()
            .contains(&json!(VERSION))
    );
    daemon.join().unwrap();
}

/// A ping without a declared version keeps the legacy behaviour: the
/// specification allows it before initialization, so it must still answer.
#[test]
fn legacy_ping_before_initialize_still_answers() {
    let temp = tempfile::tempdir().unwrap();
    let mut client = Client::spawn(&temp.path().join("unused.sock"));
    // A bare ping: no `_meta`, so no version is declared.
    let response = exchange(
        &mut client,
        json!({"jsonrpc":"2.0","id":12,"method":"ping","params":{}}),
    );
    assert_eq!(response["result"], json!({}), "{response}");
}

/// A supported pre-2026 version keeps the legacy ping as well.
#[test]
fn a_legacy_version_ping_before_initialize_still_answers() {
    let temp = tempfile::tempdir().unwrap();
    let mut client = Client::spawn(&temp.path().join("unused.sock"));
    let mut ping = request(13, "ping", json!({}));
    ping["params"]["_meta"]["io.modelcontextprotocol/protocolVersion"] = json!("2025-06-18");
    let response = exchange(&mut client, ping);
    assert_eq!(response["result"], json!({}), "{response}");
}
