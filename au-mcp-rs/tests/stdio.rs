use std::{
    io::{BufRead, BufReader, Read, Write},
    os::unix::net::UnixListener,
    process::{Child, Command, Stdio},
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, Instant},
};

use serde_json::{Value, json};

const TIMEOUT: Duration = Duration::from_secs(10);

struct OwnedChild(Child);

impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn supported_protocols_list_only_read_tools_and_call_fake_engine() {
    for version in ["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"] {
        exercise_protocol(version, version);
    }
}

#[test]
fn unsupported_protocol_negotiates_a_supported_version() {
    exercise_protocol("1999-01-01", "2025-11-25");
}

fn exercise_protocol(requested_protocol: &str, expected_protocol: &str) {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let daemon = thread::spawn(move || {
        for (verb, result) in [
            ("overview", json!({"overview":{"members":[]}})),
            (
                "type_tree",
                json!({"type_tree":{"roots":["A"],"nodes":[{"name":"A"}],"extra":true},"other":1}),
            ),
        ] {
            let deadline = Instant::now() + TIMEOUT;
            let (mut stream, _) = loop {
                match listener.accept() {
                    Ok(connection) => break connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "fake daemon accept timed out");
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("fake daemon accept failed: {error}"),
                }
            };
            stream.set_read_timeout(Some(TIMEOUT)).unwrap();
            stream.set_write_timeout(Some(TIMEOUT)).unwrap();
            let mut len = [0; 4];
            stream.read_exact(&mut len).unwrap();
            let mut request = vec![0; u32::from_be_bytes(len) as usize];
            stream.read_exact(&mut request).unwrap();
            assert_eq!(
                serde_json::from_slice::<Value>(&request).unwrap(),
                json!({"read":verb,"id":1})
            );
            let response = serde_json::to_vec(&json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":7,"result":result})).unwrap();
            stream
                .write_all(&(response.len() as u32).to_be_bytes())
                .unwrap();
            for chunk in response.chunks(3) {
                stream.write_all(chunk).unwrap();
            }
        }
    });
    let mut child = OwnedChild(
        Command::new(env!("CARGO_BIN_EXE_au-mcp-rs"))
            .arg(socket)
            .arg("--allow-mutations")
            .arg("--allow-composition")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let mut input = child.0.stdin.take().unwrap();
    let stdout = child.0.stdout.take().unwrap();
    let (tx, mut output) = mpsc::channel();
    let reader = thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    fn send(input: &mut impl Write, value: Value) {
        writeln!(input, "{value}").unwrap();
        input.flush().unwrap();
    }
    fn receive(output: &mut Receiver<std::io::Result<String>>) -> Value {
        let line = output
            .recv_timeout(TIMEOUT)
            .expect("server response timed out or stdout closed")
            .expect("failed to read server stdout");
        serde_json::from_str(&line).expect("stdout must contain only MCP JSON")
    }
    send(
        &mut input,
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":requested_protocol,"capabilities":{},"clientInfo":{"name":"protocol-test","version":"1"}}}),
    );
    let initialized = receive(&mut output);
    assert_eq!(
        initialized["result"]["protocolVersion"], expected_protocol,
        "requested protocol {requested_protocol}"
    );
    send(
        &mut input,
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
    );
    send(
        &mut input,
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}),
    );
    let tools = receive(&mut output);
    let tools = tools["result"]["tools"].as_array().unwrap();
    let names: Vec<_> = tools.iter().map(|v| v["name"].as_str().unwrap()).collect();
    assert_eq!(
        names,
        [
            "au_add_dep",
            "au_anchors",
            "au_assign_block_id",
            "au_block_ids",
            "au_candidates",
            "au_children",
            "au_content",
            "au_delete_file",
            "au_describe_tool",
            "au_diagnostics",
            "au_edit_file",
            "au_files",
            "au_follow",
            "au_frontmatter",
            "au_hubs",
            "au_imports",
            "au_inline",
            "au_instances",
            "au_instances_of",
            "au_members",
            "au_neighborhood",
            "au_overview",
            "au_preview_mutation",
            "au_promote",
            "au_reconcile",
            "au_references",
            "au_remove_dep",
            "au_rename",
            "au_rename_block_id",
            "au_rename_type",
            "au_resolve_member",
            "au_resolved",
            "au_search_tools",
            "au_semantic_tokens",
            "au_subtypes",
            "au_top_level_dirs",
            "au_type",
            "au_type_tree",
            "au_typed",
            "au_types",
            "au_validate_value",
            "au_write_file"
        ]
    );
    // An agent needs to know what the server is for before choosing a tool.
    let instructions = initialized["result"]["instructions"]
        .as_str()
        .expect("initialize must carry server instructions");
    assert!(
        instructions.contains("au_overview") && instructions.contains("outcome_unknown"),
        "{instructions}"
    );
    // Every tool states its output shape, so a client can parse the answer
    // without guessing the envelope key.
    for tool in tools {
        let name = tool["name"].as_str().unwrap();
        let schema = tool
            .get("outputSchema")
            .unwrap_or_else(|| panic!("{name} declares no outputSchema"));
        assert_eq!(schema["type"], "object", "{name}");
        // Most reads require exactly their own envelope key; `au_references`
        // returns either or both directions, so it declares no required key.
        if let Some(required) = schema["required"].as_array() {
            for key in required {
                let key = key.as_str().unwrap();
                assert!(
                    schema["properties"].get(key).is_some(),
                    "{name} requires a key it does not declare"
                );
            }
        }
    }

    let diagnostics = tools
        .iter()
        .find(|tool| tool["name"] == "au_diagnostics")
        .unwrap();
    let schema = &diagnostics["inputSchema"]["properties"];
    assert_eq!(schema["repo"]["minLength"], 1);
    assert_eq!(schema["code"]["minLength"], 1);
    assert_eq!(schema["limit"]["minimum"], 1);
    assert_eq!(schema["limit"]["maximum"], 500);
    assert!(schema.get("path").is_none());
    assert!(schema.get("path_prefix").is_none());
    let content = tools
        .iter()
        .find(|tool| tool["name"] == "au_content")
        .unwrap();
    assert_eq!(content["inputSchema"]["required"], json!(["repo", "path"]));
    let files = tools
        .iter()
        .find(|tool| tool["name"] == "au_files")
        .unwrap();
    let files_schema = &files["inputSchema"]["properties"];
    assert_eq!(files_schema["repo"]["minLength"], 1);
    assert_eq!(files_schema["limit"]["minimum"], 1);
    assert_eq!(files_schema["limit"]["maximum"], 500);
    assert!(files_schema.get("path").is_none());
    let imports = tools
        .iter()
        .find(|tool| tool["name"] == "au_imports")
        .unwrap();
    let properties = imports["inputSchema"]["properties"].as_object().unwrap();
    assert_eq!(properties.len(), 1);
    assert!(properties.contains_key("scope"));
    let members = tools
        .iter()
        .find(|tool| tool["name"] == "au_members")
        .unwrap();
    assert_eq!(members["inputSchema"]["properties"]["repo"]["minLength"], 1);
    assert!(members["inputSchema"]["properties"].get("scope").is_none());
    let subtypes = tools
        .iter()
        .find(|tool| tool["name"] == "au_subtypes")
        .unwrap();
    assert_eq!(subtypes["inputSchema"]["required"], json!(["base"]));
    assert_eq!(
        subtypes["inputSchema"]["properties"]["base"]["minLength"],
        1
    );
    let tree_tool = tools
        .iter()
        .find(|tool| tool["name"] == "au_type_tree")
        .unwrap();
    let description = tree_tool["description"].as_str().unwrap();
    assert!(description.contains("whole workspace"));
    assert!(description.contains("dependencies"));
    assert!(description.contains("unpaged"));
    assert_eq!(tree_tool["annotations"]["destructiveHint"], false);
    assert_eq!(tree_tool["annotations"]["idempotentHint"], true);
    let type_schema = &tools.iter().find(|tool| tool["name"] == "au_type").unwrap()["inputSchema"];
    assert_eq!(type_schema["required"], json!(["name"]));
    assert_eq!(type_schema["properties"]["name"]["type"], "string");
    assert!(type_schema["properties"].get("repo").is_some());
    for tool in tools {
        let read_only = tool["annotations"]["readOnlyHint"] == true;
        if matches!(
            tool["name"].as_str().unwrap(),
            "au_write_file"
                | "au_edit_file"
                | "au_delete_file"
                | "au_assign_block_id"
                | "au_rename_block_id"
                | "au_rename"
                | "au_rename_type"
                | "au_add_dep"
                | "au_remove_dep"
                | "au_inline"
                | "au_promote"
        ) {
            assert!(!read_only, "the write tool must not claim to be read-only");
            assert_eq!(tool["annotations"]["destructiveHint"], true);
        } else {
            assert!(read_only, "{} must stay read-only", tool["name"]);
        }
    }
    assert!(
        tools
            .iter()
            .all(|tool| tool["inputSchema"]["additionalProperties"] == false)
    );
    for (id, name) in [
        (5, "au_overview"),
        (6, "au_top_level_dirs"),
        (9, "au_type_tree"),
    ] {
        send(
            &mut input,
            json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":name,"arguments":{"extra":true}}}),
        );
        let result = receive(&mut output);
        assert_eq!(result["id"], id);
        assert_eq!(result["result"]["isError"], true);
    }
    for (id, name, arguments, code) in [
        (7, "au_overview", json!("invalid"), -32601),
        (8, "unknown_tool", json!({}), -32602),
    ] {
        send(
            &mut input,
            json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":name,"arguments":arguments}}),
        );
        let result = receive(&mut output);
        assert_eq!(result["id"], id);
        assert_eq!(result["error"]["code"], code);
    }
    send(
        &mut input,
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"au_overview","arguments":{}}}),
    );
    let result = receive(&mut output);
    assert_eq!(
        serde_json::from_str::<Value>(result["result"]["content"][0]["text"].as_str().unwrap())
            .unwrap(),
        json!({"overview":{"members":[]}})
    );
    send(
        &mut input,
        json!({"jsonrpc":"2.0","id":10,"method":"tools/call","params":{"name":"au_type_tree","arguments":{}}}),
    );
    let tree = receive(&mut output);
    assert_eq!(
        serde_json::from_str::<Value>(tree["result"]["content"][0]["text"].as_str().unwrap())
            .unwrap(),
        json!({"type_tree":{"roots":["A"],"nodes":[{"name":"A"}],"extra":true},"other":1})
    );
    send(
        &mut input,
        json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"shutdown","arguments":{}}}),
    );
    let denied = receive(&mut output);
    assert!(denied.get("error").is_some() || denied["result"]["isError"] == true);
    drop(input);
    let deadline = Instant::now() + TIMEOUT;
    let status = loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            break status;
        }
        assert!(Instant::now() < deadline, "server did not exit");
        thread::sleep(Duration::from_millis(10));
    };
    assert!(status.success());
    reader.join().unwrap();
    daemon.join().unwrap();
}
