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
    // The assertions below check output schemas and descriptions, so the launch
    // asks for the full profile explicitly rather than leaning on the default.
    let mut child = OwnedChild(
        Command::new(env!("CARGO_BIN_EXE_2brain"))
            .arg("mcp")
            .arg(socket)
            .arg("--tools")
            .arg("full")
            .arg("--allow-mutations")
            .arg("--allow-composition")
            .arg("--allow-workspace-shape")
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
            "tb_add_dep",
            "tb_add_workspace_member",
            "tb_anchors",
            "tb_assign_block_id",
            "tb_batch",
            "tb_block_ids",
            "tb_candidates",
            "tb_children",
            "tb_content",
            "tb_create_file",
            "tb_delete_file",
            "tb_describe_tool",
            "tb_diagnostics",
            "tb_edit_file",
            "tb_files",
            "tb_follow",
            "tb_frontmatter",
            "tb_graph_health",
            "tb_grep",
            "tb_hubs",
            "tb_imports",
            "tb_inline",
            "tb_instances",
            "tb_instances_of",
            "tb_members",
            "tb_neighborhood",
            "tb_overview",
            "tb_preview_mutation",
            "tb_promote",
            "tb_reconcile",
            "tb_references",
            "tb_remove_dep",
            "tb_remove_workspace_member",
            "tb_rename",
            "tb_rename_block_id",
            "tb_rename_type",
            "tb_resolve_member",
            "tb_resolved",
            "tb_rollback_preview",
            "tb_search",
            "tb_search_tools",
            "tb_semantic_tokens",
            "tb_set_workspace_member_role",
            "tb_subtypes",
            "tb_top_level_dirs",
            "tb_type",
            "tb_type_tree",
            "tb_typed",
            "tb_types",
            "tb_validate_value",
            "tb_watch",
            "tb_write_file"
        ]
    );
    // An agent needs to know what the server is for before choosing a tool.
    let instructions = initialized["result"]["instructions"]
        .as_str()
        .expect("initialize must carry server instructions");
    assert!(
        instructions.contains("tb_overview") && instructions.contains("outcome_unknown"),
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
        // Most reads require exactly their own envelope key; `tb_references`
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
        .find(|tool| tool["name"] == "tb_diagnostics")
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
        .find(|tool| tool["name"] == "tb_content")
        .unwrap();
    assert_eq!(content["inputSchema"]["required"], json!(["repo", "path"]));
    let files = tools
        .iter()
        .find(|tool| tool["name"] == "tb_files")
        .unwrap();
    let files_schema = &files["inputSchema"]["properties"];
    assert_eq!(files_schema["repo"]["minLength"], 1);
    assert_eq!(files_schema["limit"]["minimum"], 1);
    assert_eq!(files_schema["limit"]["maximum"], 500);
    assert!(files_schema.get("path").is_none());
    let imports = tools
        .iter()
        .find(|tool| tool["name"] == "tb_imports")
        .unwrap();
    let properties = imports["inputSchema"]["properties"].as_object().unwrap();
    assert_eq!(properties.len(), 1);
    assert!(properties.contains_key("scope"));
    let members = tools
        .iter()
        .find(|tool| tool["name"] == "tb_members")
        .unwrap();
    assert_eq!(members["inputSchema"]["properties"]["repo"]["minLength"], 1);
    assert!(members["inputSchema"]["properties"].get("scope").is_none());
    let subtypes = tools
        .iter()
        .find(|tool| tool["name"] == "tb_subtypes")
        .unwrap();
    assert_eq!(subtypes["inputSchema"]["required"], json!(["base"]));
    assert_eq!(
        subtypes["inputSchema"]["properties"]["base"]["minLength"],
        1
    );
    let tree_tool = tools
        .iter()
        .find(|tool| tool["name"] == "tb_type_tree")
        .unwrap();
    let description = tree_tool["description"].as_str().unwrap();
    assert!(description.contains("whole workspace"));
    assert!(description.contains("dependencies"));
    assert!(description.contains("unpaged"));
    assert_eq!(tree_tool["annotations"]["destructiveHint"], false);
    assert_eq!(tree_tool["annotations"]["idempotentHint"], true);
    let type_schema = &tools.iter().find(|tool| tool["name"] == "tb_type").unwrap()["inputSchema"];
    assert_eq!(type_schema["required"], json!(["name"]));
    assert_eq!(type_schema["properties"]["name"]["type"], "string");
    assert!(type_schema["properties"].get("repo").is_some());
    for tool in tools {
        let read_only = tool["annotations"]["readOnlyHint"] == true;
        if matches!(
            tool["name"].as_str().unwrap(),
            "tb_write_file"
                | "tb_edit_file"
                | "tb_delete_file"
                | "tb_assign_block_id"
                | "tb_rename_block_id"
                | "tb_rename"
                | "tb_rename_type"
                | "tb_add_dep"
                | "tb_remove_dep"
                | "tb_inline"
                | "tb_promote"
                | "tb_batch"
                | "tb_create_file"
                | "tb_add_workspace_member"
                | "tb_remove_workspace_member"
                | "tb_set_workspace_member_role"
        ) {
            assert!(!read_only, "the write tool must not claim to be read-only");
            // Creating a file replaces nothing, so its hint must say so; every
            // other write can destroy what was there.
            let destructive = tool["name"] != "tb_create_file";
            assert_eq!(tool["annotations"]["destructiveHint"], destructive);
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
        (5, "tb_overview"),
        (6, "tb_top_level_dirs"),
        (9, "tb_type_tree"),
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
        (7, "tb_overview", json!("invalid"), -32601),
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
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"tb_overview","arguments":{}}}),
    );
    let result = receive(&mut output);
    assert_eq!(
        serde_json::from_str::<Value>(result["result"]["content"][0]["text"].as_str().unwrap())
            .unwrap(),
        json!({"overview":{"members":[]}})
    );
    send(
        &mut input,
        json!({"jsonrpc":"2.0","id":10,"method":"tools/call","params":{"name":"tb_type_tree","arguments":{}}}),
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
