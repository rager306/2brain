#![cfg(unix)]

//! Progressive disclosure: the server advertises one line per tool in the
//! concise profile, and full definitions come from `au_search_tools` and
//! `au_describe_tool` when a caller asks for them.

use std::{
    io::{BufRead, BufReader, Write},
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver},
    thread,
    time::Duration,
};

use serde_json::{Value, json};

const TIMEOUT: Duration = Duration::from_secs(10);

struct Client {
    child: Child,
    input: ChildStdin,
    responses: Receiver<Value>,
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Client {
    fn start(extra: &[&str]) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_au-mcp-rs"))
            .arg("/nonexistent.sock")
            .args(extra)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, responses) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if tx
                    .send(serde_json::from_str(&line).expect("stdout JSON"))
                    .is_err()
                {
                    break;
                }
            }
        });
        let mut client = Self {
            child,
            input,
            responses,
        };
        let init = client.request(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"discovery-test","version":"1"}}}));
        assert!(init.get("result").is_some(), "{init}");
        client.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
        client
    }

    fn send(&mut self, value: Value) {
        writeln!(self.input, "{value}").unwrap();
        self.input.flush().unwrap();
    }

    fn request(&mut self, value: Value) -> Value {
        self.send(value);
        self.responses
            .recv_timeout(TIMEOUT)
            .expect("MCP response timeout")
    }

    fn tools(&mut self) -> Vec<Value> {
        let reply = self.request(json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}));
        reply["result"]["tools"].as_array().unwrap().clone()
    }

    fn call(&mut self, id: u32, name: &str, arguments: Value) -> Value {
        let reply = self.request(json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":name,"arguments":arguments}}));
        reply["result"].clone()
    }
}

fn payload(result: &Value) -> Value {
    serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap()
}

#[test]
fn the_full_profile_keeps_every_definition() {
    let mut client = Client::start(&[]);
    let tools = client.tools();
    assert!(tools.len() >= 18, "{}", tools.len());
    for tool in &tools {
        assert!(tool.get("outputSchema").is_some(), "{tool}");
        let description = tool["description"].as_str().unwrap();
        assert!(!description.is_empty(), "{tool}");
    }
}

#[test]
fn the_concise_profile_shrinks_each_definition_but_keeps_the_catalogue() {
    let mut full = Client::start(&[]);
    let full_tools = full.tools();
    let mut concise = Client::start(&["--tools", "concise"]);
    let concise_tools = concise.tools();

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
fn search_finds_a_tool_by_task_words_and_detail_selects_the_depth() {
    let mut client = Client::start(&[]);

    let found = payload(&client.call(3, "au_search_tools", json!({"query":"read file content"})));
    let names: Vec<&str> = found["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert_eq!(names.first(), Some(&"au_content"), "{names:?}");
    assert!(
        found["tools"][0].get("inputSchema").is_none(),
        "summary depth"
    );

    let names_only = payload(&client.call(
        4,
        "au_search_tools",
        json!({"query":"types","detail":"name","limit":3}),
    ));
    assert!(names_only["tools"].as_array().unwrap().len() <= 3);
    assert!(names_only["tools"][0].get("description").is_none());

    let schema = payload(&client.call(
        5,
        "au_search_tools",
        json!({"query":"content","detail":"schema","limit":1}),
    ));
    assert!(
        schema["tools"][0]["inputSchema"]["properties"]["path"].is_object(),
        "{schema}"
    );

    let empty = client.call(6, "au_search_tools", json!({"query":""}));
    assert_eq!(empty["isError"], true, "{empty}");
}

#[test]
fn describe_returns_the_full_definition_and_refuses_what_is_not_visible() {
    let mut client = Client::start(&[]);
    let described = payload(&client.call(3, "au_describe_tool", json!({"name":"au_content"})));
    assert_eq!(described["name"], "au_content");
    assert_eq!(
        described["inputSchema"]["required"],
        json!(["repo", "path"])
    );
    assert!(described.get("outputSchema").is_some());

    let unknown = client.call(4, "au_describe_tool", json!({"name":"au_nope"}));
    assert_eq!(unknown["isError"], true, "{unknown}");
    assert!(
        unknown["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("au_search_tools"),
        "{unknown}"
    );

    // A read-only launch must not leak a mutation definition through discovery.
    let hidden = client.call(5, "au_describe_tool", json!({"name":"au_write_file"}));
    assert_eq!(hidden["isError"], true, "{hidden}");
    let found = payload(&client.call(6, "au_search_tools", json!({"query":"write file"})));
    assert!(
        !found["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool["name"] == "au_write_file"),
        "{found}"
    );
}
