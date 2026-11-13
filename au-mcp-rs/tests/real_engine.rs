#![cfg(unix)]

use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use serde_json::{Value, json};

const STARTUP_TIMEOUT: Duration = Duration::from_secs(30);
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(10);

struct OwnedChild(Child);

impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn engine_read(socket: &Path, verb: &str) -> std::io::Result<Value> {
    engine_read_request(socket, json!({"read": verb, "id": 1}))
}

fn engine_read_request(socket: &Path, request: Value) -> std::io::Result<Value> {
    let mut stream = UnixStream::connect(socket)?;
    stream.set_read_timeout(Some(RESPONSE_TIMEOUT))?;
    stream.set_write_timeout(Some(RESPONSE_TIMEOUT))?;
    let request = request.to_string();
    stream.write_all(&(request.len() as u32).to_be_bytes())?;
    stream.write_all(request.as_bytes())?;
    let mut len = [0; 4];
    stream.read_exact(&mut len)?;
    let mut response = vec![0; u32::from_be_bytes(len) as usize];
    stream.read_exact(&mut response)?;
    serde_json::from_slice(&response).map_err(std::io::Error::other)
}

fn wait_ready(run: &Path, daemon: &mut Child) -> PathBuf {
    let deadline = Instant::now() + STARTUP_TIMEOUT;
    loop {
        if let Ok(entries) = fs::read_dir(run) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|ext| ext == "sock")
                    && engine_read(&path, "lifecycle")
                        .is_ok_and(|response| response["ready"] == true)
                {
                    return path;
                }
            }
        }
        assert!(
            daemon.try_wait().unwrap().is_none(),
            "daemon exited before ready"
        );
        assert!(Instant::now() < deadline, "daemon did not become ready");
        thread::sleep(Duration::from_millis(50));
    }
}

/// Wait until consecutive reads report the same engine version, so a
/// mutation's rebuild has settled before responses are compared.
fn wait_stable_version(socket: &Path) -> u64 {
    let deadline = Instant::now() + RESPONSE_TIMEOUT;
    let mut last = None;
    loop {
        let response = engine_read(socket, "overview").expect("read overview");
        let version = response["version"].as_u64().expect("engine version");
        if last == Some(version) {
            return version;
        }
        last = Some(version);
        assert!(Instant::now() < deadline, "engine version did not settle");
        thread::sleep(Duration::from_millis(25));
    }
}

fn send(input: &mut impl Write, request: Value) {
    writeln!(input, "{request}").unwrap();
    input.flush().unwrap();
}

fn receive(output: &mpsc::Receiver<String>) -> Value {
    let line = output
        .recv_timeout(RESPONSE_TIMEOUT)
        .expect("MCP response timed out or stdout closed");
    serde_json::from_str(&line).expect("MCP stdout must contain JSON only")
}

fn assert_engine_version(reply: &Value, direct: &Value) {
    let version = direct["version"]
        .as_u64()
        .expect("ready engine response must carry a numeric version");
    let metadata_version = reply["result"]["_meta"]["io.arsumbris/engineVersion"]
        .as_str()
        .expect("MCP response must carry a string engine version");
    // The fixture is static, so both reads observe the same engine version.
    assert_eq!(metadata_version, version.to_string(), "{reply}");
}

// Build the engine separately: RUSTUP_HOME=/root/.rustup cargo +1.99.0 build --manifest-path ../au-engine/Cargo.toml -p au-cli --bin au
// Then run: RUSTUP_HOME=/root/.rustup AU_ENGINE_BIN=../au-engine/target/debug/au cargo +1.99.0 test --locked --test real_engine -- --ignored
#[test]
#[ignore = "requires AU_ENGINE_BIN pointing to a separately built au binary"]
fn mcp_reads_match_real_engine_on_temporary_repo() {
    let engine = std::env::var_os("AU_ENGINE_BIN").expect("set AU_ENGINE_BIN to the au binary");
    let temp = tempfile::Builder::new()
        .prefix("au-e2e-")
        .tempdir_in("/tmp")
        .unwrap();
    let home = temp.path().join("home");
    let repo = temp.path().join("fixture");
    let base = temp.path().join("base");
    fs::create_dir_all(base.join(".arsumbris")).unwrap();
    fs::create_dir_all(base.join("type")).unwrap();
    fs::write(base.join(".arsumbris/repo.yaml"), "name: base\n").unwrap();
    fs::write(
        base.join("type/peer.type.yaml"),
        "fields:\n  title: String\n",
    )
    .unwrap();
    fs::write(base.join("asset.pdf"), b"synthetic asset").unwrap();
    fs::create_dir_all(home.join(".arsumbris")).unwrap();
    fs::create_dir_all(repo.join(".arsumbris")).unwrap();
    fs::create_dir_all(repo.join("type")).unwrap();
    fs::create_dir_all(repo.join("notes/order")).unwrap();
    fs::write(repo.join("notes/order/inner.md"), "synthetic note\n").unwrap();
    fs::write(repo.join("notes/order z.md"), "synthetic note\n").unwrap();
    fs::write(
        repo.join(".arsumbris/repo.yaml"),
        "name: fixture\ndeps:\n  - name: base\n",
    )
    .unwrap();
    fs::write(
        repo.join("type/peer-child.type.yaml"),
        "extends: peer::base\nfields:\n  label: String\n",
    )
    .unwrap();
    fs::write(
        repo.join("type/note.type.yaml"),
        "fields:\n  link?: file*\n",
    )
    .unwrap();
    fs::write(
        repo.join("type/alpha.type.yaml"),
        "fields:\n  title: String\n",
    )
    .unwrap();
    fs::write(
        repo.join("type/alpha.child.type.yaml"),
        "extends: alpha\nfields:\n  label: String\n",
    )
    .unwrap();
    fs::write(
        repo.join("type/zeta.type.yaml"),
        "fields:\n  title: String\n",
    )
    .unwrap();
    fs::write(
        repo.join("notes/a.md"),
        "---\ntype: note\nlink: \"[[b]]\"\n---\n",
    )
    .unwrap();
    fs::write(repo.join("notes/b.md"), "---\ntype: note\n---\n").unwrap();
    fs::write(
        repo.join("notes/c.md"),
        "---\ntype: note\nlink: \"[[b]]\"\n---\n",
    )
    .unwrap();
    fs::write(
        repo.join("notes/d.md"),
        "---\ntype: note\nlink: \"[[a]]\"\n---\n",
    )
    .unwrap();

    fs::write(
        repo.join("notes/invalid.md"),
        "---\ntype: note\nlink: \"[[missing-target]]\"\n---\n",
    )
    .unwrap();

    // The engine commits every mutation, so the fixture must be a clean git
    // work tree before it is served.
    let git = |args: &[&str]| {
        let status = Command::new("git")
            .args([
                "-c",
                "user.name=au-mcp-rs",
                "-c",
                "user.email=test@example.invalid",
            ])
            .args(["-c", "commit.gpgsign=false"])
            .args(args)
            .current_dir(&repo)
            .status()
            .expect("run git");
        assert!(status.success(), "git {args:?} failed");
    };
    git(&["init", "-q"]);
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "fixture"]);

    let mut daemon = OwnedChild(
        Command::new(engine)
            .args(["daemon", "start"])
            .arg(&repo)
            .env("HOME", &home)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("start private engine"),
    );
    let socket = wait_ready(&home.join(".arsumbris/au-engine/run"), &mut daemon.0);
    let mut mcp = OwnedChild(
        Command::new(env!("CARGO_BIN_EXE_au-mcp-rs"))
            .arg(&socket)
            .env("HOME", &home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("start MCP server"),
    );
    let mut input = mcp.0.stdin.take().unwrap();
    let stdout = mcp.0.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    let reader = thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if tx.send(line.expect("read MCP stdout")).is_err() {
                break;
            }
        }
    });
    send(
        &mut input,
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"real-engine-test","version":"1"}}}),
    );
    let init = receive(&rx);
    assert_eq!(init["result"]["protocolVersion"], "2024-11-05", "{init}");
    send(
        &mut input,
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
    );

    for (id, verb) in [(2, "overview"), (3, "top_level_dirs"), (9, "type_tree")] {
        let direct = engine_read(&socket, verb).expect("read private engine");
        assert_eq!(direct["ready"], true, "{direct}");
        send(
            &mut input,
            json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":format!("au_{verb}"),"arguments":{}}}),
        );
        let reply = receive(&rx);
        assert_ne!(reply["result"]["isError"], true, "{reply}");
        assert_engine_version(&reply, &direct);
        let text = reply["result"]["content"][0]["text"]
            .as_str()
            .expect("MCP text response");
        let payload: Value = serde_json::from_str(text).expect("engine JSON payload");
        assert_eq!(
            payload, direct["result"],
            "MCP {verb} differs from real engine"
        );
        if verb == "top_level_dirs" {
            assert!(
                payload["top_level_dirs"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|dir| dir["name"] == "notes" && dir["repo"] == "fixture"),
                "fixture directory missing: {payload}"
            );
        }
        if verb == "type_tree" {
            let tree = &payload["type_tree"];
            assert!(
                tree["roots"].as_array().unwrap().contains(&json!("note")),
                "fixture root missing: {payload}"
            );
            let node = tree["nodes"]
                .as_array()
                .unwrap()
                .iter()
                .find(|node| node["name"] == "note" && node["repo"] == "fixture")
                .expect("fixture type node missing");
            assert!(node["hash"].as_str().is_some_and(|hash| !hash.is_empty()));
            assert_eq!(node["parents"], json!([]));
            assert_eq!(node["children"], json!([]));
        }
    }
    for (id, arguments, engine_name, found) in [
        (4, json!({"name": "note"}), "note", true),
        (5, json!({"name": "missing"}), "missing", false),
        (6, json!({"name": "note::fixture"}), "note::fixture", true),
        (
            7,
            json!({"name": "note", "repo": "fixture"}),
            "note::fixture",
            true,
        ),
        (
            8,
            json!({"name": "note", "repo": "missing"}),
            "note::missing",
            false,
        ),
    ] {
        let direct = engine_read_request(
            &socket,
            json!({"read": "type", "name": engine_name, "id": 1}),
        )
        .expect("read type from private engine");
        assert_eq!(direct["ready"], true, "{direct}");
        send(
            &mut input,
            json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"au_type","arguments":arguments}}),
        );
        let reply = receive(&rx);
        assert_ne!(reply["result"]["isError"], true, "{reply}");
        assert_engine_version(&reply, &direct);
        let text = reply["result"]["content"][0]["text"]
            .as_str()
            .expect("MCP text response");
        let payload: Value = serde_json::from_str(text).expect("engine JSON payload");
        assert_eq!(
            payload, direct["result"],
            "MCP type differs for {engine_name}"
        );
        if found {
            assert_eq!(payload["type"]["name"], "note", "{payload}");
        } else {
            assert!(payload["type"].is_null(), "{payload}");
        }
    }
    let mut types_read = |id: u64, arguments: Value, request: Value| {
        let direct = engine_read_request(&socket, request).expect("read types from private engine");
        assert_eq!(direct["ready"], true, "{direct}");
        send(
            &mut input,
            json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"au_types","arguments":arguments}}),
        );
        let reply = receive(&rx);
        assert_ne!(reply["result"]["isError"], true, "{reply}");
        assert_engine_version(&reply, &direct);
        let text = reply["result"]["content"][0]["text"]
            .as_str()
            .expect("MCP text response");
        let payload: Value = serde_json::from_str(text).expect("engine JSON payload");
        assert_eq!(payload, direct["result"], "MCP types differs from engine");
        payload
    };

    let default = types_read(
        10,
        json!({}),
        json!({"read":"types","id":1,"summary":true,"limit":50,"offset":0}),
    );
    assert!(default["types"].is_array(), "{default}");

    let summary = types_read(
        11,
        json!({"repo":"fixture", "limit":500}),
        json!({"read":"types","id":1,"repo":"fixture","summary":true,"limit":500,"offset":0}),
    );
    let entries = summary["types"].as_array().expect("fixture types array");
    let names: Vec<&str> = entries
        .iter()
        .map(|entry| entry["name"].as_str().expect("type name"))
        .collect();
    assert!(names.windows(2).all(|pair| pair[0] <= pair[1]), "{names:?}");
    for name in ["alpha", "note", "zeta"] {
        assert!(names.contains(&name), "missing {name}: {summary}");
    }
    let summary_note = entries
        .iter()
        .find(|entry| entry["name"] == "note")
        .unwrap();
    assert!(summary_note.get("fields").is_none(), "{summary_note}");

    let full = types_read(
        12,
        json!({"repo":"fixture", "summary":false, "limit":500}),
        json!({"read":"types","id":1,"repo":"fixture","summary":false,"limit":500,"offset":0}),
    );
    let full_entries = full["types"].as_array().expect("full types array");
    let full_names: Vec<&str> = full_entries
        .iter()
        .map(|entry| entry["name"].as_str().expect("type name"))
        .collect();
    assert_eq!(names, full_names);
    let full_note = full_entries
        .iter()
        .find(|entry| entry["name"] == "note")
        .unwrap();
    assert!(full_note.get("fields").is_some(), "{full_note}");

    let unknown = types_read(
        13,
        json!({"repo":"missing"}),
        json!({"read":"types","id":1,"repo":"missing","summary":true,"limit":50,"offset":0}),
    );
    assert!(unknown["types"].is_null(), "{unknown}");

    for (id, offset) in [(14, 0), (15, 1), (16, entries.len())] {
        let page = types_read(
            id,
            json!({"repo":"fixture", "limit":1, "offset":offset}),
            json!({"read":"types","id":1,"repo":"fixture","summary":true,"limit":1,"offset":offset}),
        );
        let expected = &entries[offset..(offset + 1).min(entries.len())];
        assert_eq!(page["types"].as_array().unwrap(), expected, "{page}");
    }
    let mut hubs_read = |id: u64, arguments: Value, request: Value| {
        let direct = engine_read_request(&socket, request).expect("read hubs from private engine");
        assert_eq!(direct["ready"], true, "{direct}");
        send(
            &mut input,
            json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"au_hubs","arguments":arguments}}),
        );
        let reply = receive(&rx);
        assert_ne!(reply["result"]["isError"], true, "{reply}");
        assert_engine_version(&reply, &direct);
        let text = reply["result"]["content"][0]["text"]
            .as_str()
            .expect("MCP text response");
        let payload: Value = serde_json::from_str(text).expect("engine JSON payload");
        assert_eq!(payload, direct["result"], "MCP hubs differs from engine");
        payload
    };

    let hubs = hubs_read(
        20,
        json!({}),
        json!({"read":"hubs","id":1,"limit":20,"offset":0}),
    );
    let entries = hubs["hubs"].as_array().expect("hubs array");
    assert!(entries.len() >= 2, "fixture should have two hubs: {hubs}");
    assert!(
        entries[0]["path"].as_str().unwrap().ends_with("notes/b.md"),
        "{hubs}"
    );
    assert_eq!(entries[0]["refs_structural"], 2, "{hubs}");
    let overview = engine_read(&socket, "overview").expect("read private overview");
    assert_eq!(overview["ready"], true, "{overview}");
    assert_eq!(
        hubs["hubs"], overview["result"]["overview"]["hubs"],
        "default overview hubs differ"
    );

    for (id, scope) in [(21, "own"), (22, "all")] {
        hubs_read(
            id,
            json!({"scope":scope}),
            json!({"read":"hubs","id":1,"scope":scope,"limit":20,"offset":0}),
        );
    }
    let scoped = hubs_read(
        23,
        json!({"repo":"fixture", "scope":"all", "limit":20, "offset":0}),
        json!({"read":"hubs","id":1,"repo":"fixture","scope":"all","limit":20,"offset":0}),
    );
    let scoped_entries = scoped["hubs"].as_array().expect("scoped hubs array");
    assert!(scoped_entries.len() >= 2, "{scoped}");
    for (id, offset) in [(24, 0), (25, 1), (26, scoped_entries.len())] {
        let page = hubs_read(
            id,
            json!({"repo":"fixture", "scope":"all", "limit":1, "offset":offset}),
            json!({"read":"hubs","id":1,"repo":"fixture","scope":"all","limit":1,"offset":offset}),
        );
        let expected = &scoped_entries[offset..(offset + 1).min(scoped_entries.len())];
        assert_eq!(page["hubs"].as_array().unwrap(), expected, "{page}");
    }
    let unknown = hubs_read(
        31,
        json!({"repo":"missing"}),
        json!({"read":"hubs","id":1,"repo":"missing","limit":20,"offset":0}),
    );
    assert_eq!(unknown["hubs"], json!([]), "{unknown}");
    for (id, arguments) in [
        (27, json!({"scope":"invalid"})),
        (28, json!({"limit":-1})),
        (29, json!({"offset":"1"})),
        (30, json!({"unexpected":true})),
    ] {
        send(
            &mut input,
            json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"au_hubs","arguments":arguments}}),
        );
        let reply = receive(&rx);
        assert!(
            reply.get("error").is_some() || reply["result"]["isError"] == true,
            "{reply}"
        );
    }
    for (id, relative) in [(90, "notes/a.md"), (91, "notes/missing.md")] {
        let direct = engine_read_request(
            &socket,
            json!({"read":"content","id":1,"path": repo.join(relative).to_str().unwrap()}),
        )
        .expect("read content from private engine");
        assert_eq!(direct["ready"], true, "{direct}");
        send(
            &mut input,
            json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"au_content","arguments":{"repo":"fixture","path":relative}}}),
        );
        let reply = receive(&rx);
        assert_ne!(reply["result"]["isError"], true, "{reply}");
        assert_engine_version(&reply, &direct);
        let payload: Value =
            serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(payload, direct["result"], "MCP content differs from engine");
        if id == 90 {
            let content = &payload["content"];
            assert!(content["text"].as_str().unwrap().contains("link"));
            assert!(
                content["hash"].as_str().is_some_and(|h| !h.is_empty()),
                "{payload}"
            );
        } else {
            assert!(payload["content"].is_null(), "{payload}");
        }
    }
    send(
        &mut input,
        json!({"jsonrpc":"2.0","id":92,"method":"tools/call","params":{"name":"au_content","arguments":{"repo":"fixture","path":"../base/asset.pdf"}}}),
    );
    let denied = receive(&rx);
    assert_eq!(denied["result"]["isError"], true, "{denied}");

    // Mutation round trip against the real engine: read, guarded write, re-read.
    send(
        &mut input,
        json!({"jsonrpc":"2.0","id":93,"method":"tools/call","params":{"name":"au_content","arguments":{"repo":"fixture","path":"notes/a.md"}}}),
    );
    let read = receive(&rx);
    assert_ne!(read["result"]["isError"], true, "{read}");
    let read_payload: Value =
        serde_json::from_str(read["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    let hash = read_payload["content"]["hash"]
        .as_str()
        .expect("content hash")
        .to_owned();
    let new_text = "---\ntype: note\nlink: \"[[b]]\"\n---\n\nrewritten by the MCP test\n";
    // Preview first: it must report a product and change nothing.
    send(
        &mut input,
        json!({"jsonrpc":"2.0","id":96,"method":"tools/call","params":{"name":"au_preview_mutation","arguments":{"repo":"fixture","path":"notes/a.md","op":"write_file","content":new_text}}}),
    );
    let preview = receive(&rx);
    assert_ne!(preview["result"]["isError"], true, "{preview}");
    let preview_payload: Value =
        serde_json::from_str(preview["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(
        preview_payload["preview_mutation"]["target"]["path"],
        repo.join("notes/a.md").to_str().unwrap()
    );
    let unchanged = engine_read_request(
        &socket,
        json!({"read":"content","id":1,"path": repo.join("notes/a.md").to_str().unwrap()}),
    )
    .expect("read content after preview");
    assert!(
        unchanged["result"]["content"]["text"]
            .as_str()
            .unwrap()
            .contains("link"),
        "preview must not write: {unchanged}"
    );
    send(
        &mut input,
        json!({"jsonrpc":"2.0","id":94,"method":"tools/call","params":{"name":"au_write_file","arguments":{"repo":"fixture","path":"notes/a.md","content":new_text,"expected_hash":hash}}}),
    );
    let written = receive(&rx);
    assert_ne!(written["result"]["isError"], true, "{written}");
    let written_payload: Value =
        serde_json::from_str(written["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(
        written_payload["path"],
        repo.join("notes/a.md").to_str().unwrap()
    );
    assert!(
        written_payload["hash"]
            .as_str()
            .is_some_and(|h| !h.is_empty()),
        "{written_payload}"
    );
    let direct = engine_read_request(
        &socket,
        json!({"read":"content","id":1,"path": repo.join("notes/a.md").to_str().unwrap()}),
    )
    .expect("read written content from private engine");
    assert_eq!(direct["result"]["content"]["text"], new_text);
    assert_eq!(
        direct["result"]["content"]["hash"], written_payload["hash"],
        "the engine's hash must match the mutation result"
    );
    // The recorded read was discarded, so the same write cannot be replayed.
    send(
        &mut input,
        json!({"jsonrpc":"2.0","id":95,"method":"tools/call","params":{"name":"au_write_file","arguments":{"repo":"fixture","path":"notes/a.md","content":new_text,"expected_hash":hash}}}),
    );
    let replay = receive(&rx);
    assert_eq!(replay["result"]["isError"], true, "{replay}");
    assert!(
        replay["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("au_content"),
        "{replay}"
    );
    // The write advanced the engine version; let the rebuild settle so later
    // response versions are comparable again.
    wait_stable_version(&socket);

    for (id, arguments, request) in [
        (
            80,
            json!({}),
            json!({"read":"files","id":1,"limit":50,"offset":0}),
        ),
        (
            81,
            json!({"repo":"base","scope":"all","limit":500}),
            json!({"read":"files","id":1,"repo":"base","scope":"all","limit":500,"offset":0}),
        ),
        (
            82,
            json!({"repo":"base","scope":"own","limit":500}),
            json!({"read":"files","id":1,"repo":"base","scope":"own","limit":500,"offset":0}),
        ),
        (
            83,
            json!({"repo":"fixture","limit":1,"offset":1}),
            json!({"read":"files","id":1,"repo":"fixture","limit":1,"offset":1}),
        ),
    ] {
        let direct = engine_read_request(&socket, request).expect("read files from private engine");
        assert_eq!(direct["ready"], true, "{direct}");
        send(
            &mut input,
            json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"au_files","arguments":arguments}}),
        );
        let reply = receive(&rx);
        assert_ne!(reply["result"]["isError"], true, "{reply}");
        assert_engine_version(&reply, &direct);
        let payload: Value =
            serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(payload, direct["result"], "MCP files differs from engine");
        let files = payload["files"].as_array().unwrap();
        if id == 80 {
            let nested = files
                .iter()
                .position(|f| {
                    f["path"]
                        .as_str()
                        .is_some_and(|p| p.ends_with("notes/order/inner.md"))
                })
                .expect("nested file");
            let sibling = files
                .iter()
                .position(|f| {
                    f["path"]
                        .as_str()
                        .is_some_and(|p| p.ends_with("notes/order z.md"))
                })
                .expect("sibling file");
            assert!(nested < sibling, "component ordering: {payload}");
        }
        if id == 81 {
            assert!(
                files.iter().any(
                    |f| f["path"].as_str().is_some_and(|p| p.ends_with("asset.pdf"))
                        && f["kind"] == "asset"
                ),
                "{payload}"
            );
        }
        if id == 82 {
            assert!(files.is_empty(), "{payload}");
        }
        if id == 83 {
            assert_eq!(files.len(), 1, "{payload}");
        }
    }
    for (id, scope) in [(70, None), (71, Some("all")), (72, Some("own"))] {
        let mut request = json!({"read":"imports","id":1});
        let mut arguments = json!({});
        if let Some(scope) = scope {
            request["scope"] = json!(scope);
            arguments["scope"] = json!(scope);
        }
        let direct =
            engine_read_request(&socket, request).expect("read imports from private engine");
        assert_eq!(direct["ready"], true, "{direct}");
        send(
            &mut input,
            json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"au_imports","arguments":arguments}}),
        );
        let reply = receive(&rx);
        assert_ne!(reply["result"]["isError"], true, "{reply}");
        assert_engine_version(&reply, &direct);
        let payload: Value =
            serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(payload, direct["result"], "MCP imports differs from engine");
        let imports = payload["imports"].as_array().unwrap();
        assert!(
            imports.iter().any(|entry| entry["importer"] == "fixture"
                && entry["owner"] == "base"
                && entry["name"] == "peer"),
            "{payload}"
        );
        if scope == Some("own") {
            assert!(
                imports.iter().all(|entry| entry["owner"] != "au-engine"),
                "{payload}"
            );
        }
    }
    for (id, repo) in [(60, None), (61, Some("fixture")), (62, Some("missing"))] {
        let mut request = json!({"read":"members","id":1});
        let mut args = json!({});
        if let Some(repo) = repo {
            request["repo"] = json!(repo);
            args["repo"] = json!(repo);
        }
        let direct =
            engine_read_request(&socket, request).expect("read members from private engine");
        assert_eq!(direct["ready"], true, "{direct}");
        send(
            &mut input,
            json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"au_members","arguments":args}}),
        );
        let reply = receive(&rx);
        assert_ne!(reply["result"]["isError"], true, "{reply}");
        assert_engine_version(&reply, &direct);
        let payload: Value =
            serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(payload, direct["result"], "MCP members differs from engine");
        let members = payload["members"].as_array().unwrap();
        if repo == Some("missing") {
            assert!(members.is_empty(), "{payload}");
        } else {
            assert!(
                members
                    .iter()
                    .any(|m| m["repo"] == "fixture" && m["role"] == "entry"),
                "{payload}"
            );
        }
    }
    for (id, base, scope, found) in [
        (50, "alpha", "all", true),
        (51, "alpha::fixture", "own", true),
        (52, "unknown", "all", false),
    ] {
        let direct = engine_read_request(
            &socket,
            json!({"read":"subtypes","id":1,"base":base,"scope":scope}),
        )
        .expect("read subtypes from private engine");
        send(
            &mut input,
            json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"au_subtypes","arguments":{"base":base,"scope":scope}}}),
        );
        let reply = receive(&rx);
        assert_ne!(reply["result"]["isError"], true, "{reply}");
        assert_engine_version(&reply, &direct);
        let payload: Value =
            serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(
            payload, direct["result"],
            "MCP subtypes differs from engine"
        );
        let entries = payload["subtypes"].as_array().unwrap();
        assert_eq!(
            entries.iter().any(|item| item["name"] == "alpha.child"),
            found,
            "{payload}"
        );
    }
    let mut diagnostics_read = |id: u64, arguments: Value, request: Value| {
        let direct =
            engine_read_request(&socket, request).expect("read diagnostics from private engine");
        assert_eq!(direct["ready"], true, "{direct}");
        send(
            &mut input,
            json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"au_diagnostics","arguments":arguments}}),
        );
        let reply = receive(&rx);
        assert_ne!(reply["result"]["isError"], true, "{reply}");
        assert_engine_version(&reply, &direct);
        let payload: Value =
            serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(
            payload, direct["result"],
            "MCP diagnostics differs from engine"
        );
        payload
    };
    let diagnostics = diagnostics_read(
        40,
        json!({}),
        json!({"read":"diagnostics","id":1,"limit":50,"offset":0}),
    );
    let entries = diagnostics["diagnostics"]
        .as_array()
        .expect("diagnostics array");
    assert!(
        !entries.is_empty(),
        "fixture must produce diagnostics: {diagnostics}"
    );
    let first = &entries[0];
    let code = first["code"].as_str().unwrap();
    let severity = first["severity"].as_str().unwrap();
    let filtered = diagnostics_read(
        41,
        json!({"repo":"fixture","scope":"own","severity":severity,"code":code,"limit":1}),
        json!({"read":"diagnostics","id":1,"repo":"fixture","scope":"own","severity":severity,"code":code,"limit":1,"offset":0}),
    );
    assert_eq!(
        filtered["diagnostics"].as_array().unwrap().len(),
        1,
        "{filtered}"
    );
    diagnostics_read(
        42,
        json!({"repo":"missing","limit":1,"offset":1}),
        json!({"read":"diagnostics","id":1,"repo":"missing","limit":1,"offset":1}),
    );
    drop(input);
    let deadline = Instant::now() + RESPONSE_TIMEOUT;
    let status = loop {
        if let Some(status) = mcp.0.try_wait().expect("wait for MCP server") {
            break status;
        }
        assert!(Instant::now() < deadline, "MCP server did not exit");
        thread::sleep(Duration::from_millis(50));
    };
    assert!(status.success(), "MCP server exited with {status}");
    reader.join().expect("MCP reader thread");
}
