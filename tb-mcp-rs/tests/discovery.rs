#![cfg(unix)]

//! Progressive disclosure: the server advertises one line per tool by default
//! (the concise profile), and full definitions come from `tb_search_tools` and
//! `tb_describe_tool` when a caller asks for them. `--tools full` opts back in
//! to every definition up front.

use std::{path::Path, time::Duration};

use serde_json::{Value, json};

mod support;

use support::{Client, payload};

const TIMEOUT: Duration = Duration::from_secs(10);

fn connect(extra: &[&str]) -> Client {
    Client::start_with_timeout(Path::new("/nonexistent.sock"), extra, TIMEOUT)
}

fn tools(client: &mut Client) -> Vec<Value> {
    let reply = client.request(json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}));
    reply["result"]["tools"].as_array().unwrap().clone()
}

#[test]
fn the_full_profile_keeps_every_definition() {
    let mut client = connect(&["--tools", "full"]);
    let tools = tools(&mut client);
    assert!(tools.len() >= 18, "{}", tools.len());
    for tool in &tools {
        assert!(tool.get("outputSchema").is_some(), "{tool}");
        let description = tool["description"].as_str().unwrap();
        assert!(!description.is_empty(), "{tool}");
    }
}

#[test]
fn the_concise_profile_shrinks_each_definition_but_keeps_the_catalogue() {
    let mut full = connect(&["--tools", "full"]);
    let full_tools = tools(&mut full);
    let mut concise = connect(&["--tools", "concise"]);
    let concise_tools = tools(&mut concise);

    assert_eq!(
        concise_tools.len(),
        full_tools.len(),
        "the profile must not hide tools, only shorten them"
    );
    for tool in &concise_tools {
        let name = tool["name"].as_str().unwrap();
        let description = tool["description"].as_str().unwrap();
        assert!(
            description.chars().count() <= 141,
            "{name} keeps a long description: {description}"
        );
        // Everything deeper is one call away, including the output schema.
        assert!(tool.get("outputSchema").is_none(), "{name}");
        assert_eq!(tool["inputSchema"]["type"], "object", "{name}");
    }
    let concise_size = serde_json::to_string(&concise_tools).unwrap().len();
    let full_size = serde_json::to_string(&full_tools).unwrap().len();
    assert!(
        concise_size * 2 < full_size,
        "concise {concise_size} vs full {full_size}"
    );
}

#[test]
fn the_default_profile_is_concise() {
    // The default surface is one line per tool with no output schema; `--tools
    // full` is the opt-in. Pinned here so a future edit cannot silently widen
    // the default back to the heavy surface.
    let mut default = connect(&[]);
    let default_tools = tools(&mut default);
    let mut concise = connect(&["--tools", "concise"]);
    let concise_tools = tools(&mut concise);
    assert_eq!(
        serde_json::to_string(&default_tools).unwrap(),
        serde_json::to_string(&concise_tools).unwrap(),
        "the launch default must stay the concise profile"
    );
}

#[test]
fn describe_returns_the_full_definition_under_the_concise_default() {
    // The concise surface withholds detail up front; `tb_describe_tool` must
    // hand ALL of it back: the long description and both schemas, byte-equal
    // to what the full profile advertises.
    let mut full = connect(&["--tools", "full"]);
    let described_full =
        payload(&full.call_result(3, "tb_describe_tool", json!({"name":"tb_search"})));
    let mut concise = connect(&["--tools", "concise"]);
    let described_concise =
        payload(&concise.call_result(3, "tb_describe_tool", json!({"name":"tb_search"})));
    assert_eq!(described_full, described_concise);
    let definition = &described_concise["tool"];
    let description = definition["description"].as_str().unwrap();
    assert!(
        description.chars().count() > 141,
        "describe must return more than the concise one-liner: {description}"
    );
    assert!(definition.get("outputSchema").is_some());
    assert_eq!(definition["inputSchema"]["required"], json!(["query"]));
    assert!(
        definition["inputSchema"]["properties"]["query"]
            .get("description")
            .is_some(),
        "describe returns the parameter descriptions the concise surface strips"
    );
}

#[test]
fn search_finds_a_tool_by_task_words_and_detail_selects_the_depth() {
    let mut client = connect(&[]);

    let found =
        payload(&client.call_result(3, "tb_search_tools", json!({"query":"read file content"})));
    let names: Vec<&str> = found["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert_eq!(names.first(), Some(&"tb_content"), "{names:?}");
    assert!(
        found["tools"][0].get("inputSchema").is_none(),
        "summary depth"
    );

    let names_only = payload(&client.call_result(
        4,
        "tb_search_tools",
        json!({"query":"types","detail":"name","limit":3}),
    ));
    assert!(names_only["tools"].as_array().unwrap().len() <= 3);
    assert!(names_only["tools"][0].get("description").is_none());

    let schema = payload(&client.call_result(
        5,
        "tb_search_tools",
        json!({"query":"content","detail":"schema","limit":1}),
    ));
    assert!(
        schema["tools"][0]["inputSchema"]["properties"]["path"].is_object(),
        "{schema}"
    );

    let empty = client.call_result(6, "tb_search_tools", json!({"query":""}));
    assert_eq!(empty["isError"], true, "{empty}");
}

#[test]
fn describe_returns_the_full_definition_and_refuses_what_is_not_visible() {
    let mut client = connect(&[]);
    let described =
        payload(&client.call_result(3, "tb_describe_tool", json!({"name":"tb_content"})));
    // The definition sits under `tool`, the one key the declared `outputSchema`
    // allows. A bare definition is rejected by any client that validates
    // `structuredContent`, which is how this shape was found.
    assert_eq!(
        described.as_object().unwrap().keys().collect::<Vec<_>>(),
        vec!["tool"]
    );
    let definition = &described["tool"];
    assert_eq!(definition["name"], "tb_content");
    assert_eq!(
        definition["inputSchema"]["required"],
        json!(["repo", "path"])
    );
    assert!(definition.get("outputSchema").is_some());

    let unknown = client.call_result(4, "tb_describe_tool", json!({"name":"tb_nope"}));
    assert_eq!(unknown["isError"], true, "{unknown}");
    assert!(
        unknown["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("tb_search_tools"),
        "{unknown}"
    );

    // A read-only launch must not leak a mutation definition through discovery.
    let hidden = client.call_result(5, "tb_describe_tool", json!({"name":"tb_write_file"}));
    assert_eq!(hidden["isError"], true, "{hidden}");
    let found = payload(&client.call_result(6, "tb_search_tools", json!({"query":"write file"})));
    assert!(
        !found["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool["name"] == "tb_write_file"),
        "{found}"
    );
}
