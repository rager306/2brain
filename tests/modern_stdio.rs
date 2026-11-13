use std::{
    io::{BufRead, BufReader, Read, Write},
    os::unix::net::UnixListener,
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver},
    thread,
    time::Duration,
};

use serde_json::{Value, json};

const VERSION: &str = "2026-07-28";

struct Client {
    child: Child,
    input: ChildStdin,
    responses: Receiver<Value>,
}

impl Client {
    fn start(socket: &std::path::Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_au-mcp-rs"))
            .arg(socket)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (sender, responses) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                let value = serde_json::from_str(&line).expect("stdout must contain JSON only");
                if sender.send(value).is_err() {
                    break;
                }
            }
        });
        Self {
            child,
            input,
            responses,
        }
    }

    fn request(&mut self, request: Value) -> Value {
        writeln!(self.input, "{request}").unwrap();
        self.input.flush().unwrap();
        let response = self
            .responses
            .recv_timeout(Duration::from_secs(5))
            .expect("server did not respond");
        assert_eq!(response["id"], request["id"]);
        response
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

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

#[test]
#[ignore = "modern protocol not enabled yet"]
fn modern_first_ping_obeys_version_and_removed_method_rules() {
    let temp = tempfile::tempdir().unwrap();
    let mut client = Client::start(&temp.path().join("unused.sock"));
    let mut unknown_ping = request(10, "ping", json!({}));
    unknown_ping["params"]["_meta"]["io.modelcontextprotocol/protocolVersion"] =
        json!("2099-01-01");
    let response = client.request(unknown_ping);
    assert_eq!(response["error"]["code"], -32022);
    let response = client.request(request(11, "ping", json!({})));
    assert_eq!(response["error"]["code"], -32601);
}

// The production server currently advertises only tested legacy versions.
// Enable this test when the server enables and validates 2026-07-28.
#[test]
#[ignore = "modern protocol not enabled yet"]
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

    let mut discover_client = Client::start(&socket);
    let discover = discover_client.request(request(1, "server/discover", json!({})));
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
        env!("CARGO_PKG_NAME")
    );

    // A fresh process receives tools/list as its first message, without discovery or initialize.
    let mut client = Client::start(&socket);
    let list = client.request(request(2, "tools/list", json!({})));
    assert_eq!(list["result"]["resultType"], "complete");
    let tools = list["result"]["tools"].as_array().unwrap();
    for name in ["au_overview", "au_top_level_dirs"] {
        let tool = tools.iter().find(|tool| tool["name"] == name).unwrap();
        assert_eq!(tool["inputSchema"]["additionalProperties"], false);
        assert_eq!(tool["annotations"]["readOnlyHint"], true);
    }
    for (id, name, payload) in [
        (3, "au_overview", json!({"overview":{"members":[]}})),
        (4, "au_top_level_dirs", json!({"top_level_dirs":[]})),
    ] {
        let response = client.request(request(
            id,
            "tools/call",
            json!({"name":name,"arguments":{}}),
        ));
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
    let denied = client.request(request(
        5,
        "tools/call",
        json!({"name":"shutdown","arguments":{}}),
    ));
    assert!(denied.get("error").is_some() || denied["result"]["isError"] == true);
    let invalid = client.request(request(
        6,
        "tools/call",
        json!({"name":"au_overview","arguments":{"extra":true}}),
    ));
    assert!(invalid.get("error").is_some() || invalid["result"]["isError"] == true);
    let mut unsupported = request(7, "tools/list", json!({}));
    unsupported["params"]["_meta"]["io.modelcontextprotocol/protocolVersion"] = json!("2099-01-01");
    let response = client.request(unsupported);
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
