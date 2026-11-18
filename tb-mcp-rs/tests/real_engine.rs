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

/// The directory a private engine's sockets live in, found rather than spelled.
///
/// Which name the state root carries is the engine's decision (`$2BRAIN_HOME`, an existing
/// `.2brain`, an existing `.arsumbris`, else `.2brain`), so a test that joins a literal name
/// asserts that decision instead of the behaviour it means to test: it passes while the rule
/// holds and fails when the rule changes and the engine is right. This walks `$HOME` for any
/// `<root>/au-engine/run` and returns the first, which is the same directory either way.
fn state_run_dir(home: &Path) -> PathBuf {
    let deadline = Instant::now() + STARTUP_TIMEOUT;
    loop {
        if let Ok(entries) = fs::read_dir(home) {
            for entry in entries.flatten() {
                let run = entry.path().join("au-engine").join("run");
                if run.is_dir() {
                    return run;
                }
            }
        }
        assert!(
            Instant::now() < deadline,
            "the engine created no au-engine/run directory under {}",
            home.display()
        );
        thread::sleep(Duration::from_millis(50));
    }
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

/// The MCP metadata version must be a version the engine actually served.
///
/// The engine's `version` advances asynchronously after a mutation, so the
/// direct read taken just before the MCP call can legitimately observe an older
/// one. Accept the pair only when the engine reaches the reported version.
fn assert_engine_version(socket: &Path, reply: &Value, direct: &Value) {
    let metadata_version = reply["result"]["_meta"]["io.2brain/engineVersion"]
        .as_str()
        .expect("MCP response must carry a string engine version")
        .to_owned();
    metadata_version
        .parse::<u64>()
        .unwrap_or_else(|_| panic!("engine version must be decimal: {reply}"));
    if direct["version"].as_u64().map(|v| v.to_string()) == Some(metadata_version.clone()) {
        return;
    }
    let deadline = Instant::now() + RESPONSE_TIMEOUT;
    loop {
        let observed = engine_read(socket, "overview")
            .expect("read overview while waiting for the engine version");
        if observed["version"].as_u64().map(|v| v.to_string()) == Some(metadata_version.clone()) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "engine never reached the version the MCP response reported: {metadata_version}, reply {reply}"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

/// Build the disposable fixture every real-engine test serves: two members with
/// a dependency, typed notes, an invalid link, and a clean git work tree.
/// Make `dir` a clean single-commit git work tree, which mutations require.
fn commit_work_tree(dir: &Path) {
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
            .current_dir(dir)
            .status()
            .expect("run git");
        assert!(status.success(), "git {args:?} in {}", dir.display());
    };
    git(&["init", "-q"]);
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "fixture"]);
}

fn build_fixture(temp: &Path) -> (PathBuf, PathBuf) {
    let home = temp.join("home");
    let repo = temp.join("fixture");
    let base = temp.join("base");
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
        "---\ntype: note\nlink: \"[[b]]\"\n---\n\n## Details\n\nA marker for the addressability reads. ^detail\n",
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

    // The engine commits every mutation, so each served member must be a clean
    // git work tree. The dependency gets its own repository so its git root is
    // its own directory rather than whatever encloses the temporary parent.
    commit_work_tree(&base);
    commit_work_tree(&repo);
    (home, repo)
}

// Build the engine separately: RUSTUP_HOME=/root/.rustup cargo +1.99.0 build --manifest-path ../tb-engine/Cargo.toml -p au-cli --bin au
// Then run: RUSTUP_HOME=/root/.rustup AU_ENGINE_BIN=../tb-engine/target/debug/au cargo +1.99.0 test --locked --test real_engine -- --ignored
/// The read fixtures replay every advertised read through the MCP layer against
/// a real engine on a disposable repository. Paths are recorded relative to the
/// fixture root as `$FIXTURE`, so the corpus does not carry machine paths.
fn fixture_cases() -> [(&'static str, Value); 32] {
    [
        ("tb_overview", json!({})),
        ("tb_top_level_dirs", json!({})),
        ("tb_type", json!({"name":"note"})),
        ("tb_type_tree", json!({})),
        ("tb_types", json!({})),
        ("tb_hubs", json!({"limit":20})),
        ("tb_diagnostics", json!({"limit":50})),
        ("tb_subtypes", json!({"base":"alpha"})),
        ("tb_members", json!({})),
        ("tb_imports", json!({"scope":"all"})),
        ("tb_files", json!({"repo":"fixture","limit":50})),
        ("tb_content", json!({"repo":"fixture","path":"notes/a.md"})),
        ("tb_instances_of", json!({"type_name":"note"})),
        (
            "tb_validate_value",
            json!({"type_name":"note","value":{"link":"[[b]]"}}),
        ),
        (
            "tb_preview_mutation",
            json!({"repo":"fixture","path":"notes/a.md","op":"write_file","content":"---\ntype: note\n---\n"}),
        ),
        (
            "tb_reconcile",
            json!({"repo":"fixture","path":"notes/ghost.md","expect_absent":true}),
        ),
        (
            "tb_frontmatter",
            json!({"repo":"fixture","path":"notes/a.md"}),
        ),
        ("tb_anchors", json!({"repo":"fixture","path":"notes/a.md"})),
        ("tb_graph_health", json!({"repo":"fixture","limit":10})),
        (
            "tb_rollback_preview",
            json!({"repo":"fixture","commit":"HEAD"}),
        ),
        (
            "tb_grep",
            json!({"repo":"fixture","query":"addressability","limit":10,"offset":0}),
        ),
        (
            "tb_block_ids",
            json!({"repo":"fixture","path":"notes/a.md"}),
        ),
        (
            "tb_references",
            json!({"repo":"fixture","path":"notes/a.md"}),
        ),
        (
            "tb_neighborhood",
            json!({"repo":"fixture","path":"notes/a.md","depth":2,"kinds":["navigational"]}),
        ),
        ("tb_instances", json!({"limit":3,"offset":0})),
        ("tb_candidates", json!({"summary":true,"limit":5})),
        (
            "tb_semantic_tokens",
            json!({"repo":"fixture","path":"notes/a.md"}),
        ),
        (
            "tb_resolve_member",
            json!({"repo":"fixture","path":"notes/a.md"}),
        ),
        (
            "tb_follow",
            json!({"repo":"fixture","origin_path":"notes/a.md","target":"a#Details"}),
        ),
        ("tb_children", json!({"repo":"fixture","path":"notes"})),
        ("tb_typed", json!({"repo":"fixture","path":"notes/a.md"})),
        ("tb_resolved", json!({"repo":"fixture","path":"notes/a.md"})),
    ]
}

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/synthetic-reads")
}

/// Replace the disposable root with `$FIXTURE` so a fixture is machine-neutral.
fn normalize(root: &Path, value: &Value) -> Value {
    let text = value
        .to_string()
        .replace(&root.display().to_string(), "$FIXTURE");
    let value: Value = serde_json::from_str(&text).expect("normalized fixture stays JSON");
    normalize_commits(&value)
}

/// A git object name changes with every capture, so it is recorded as `$COMMIT`.
/// Engine content hashes are 16 characters, so a 40-character lowercase hex
/// string in these payloads is a commit and nothing else.
fn normalize_commits(value: &Value) -> Value {
    const HEX: &[u8] = b"0123456789abcdef";
    match value {
        Value::String(text) if text.len() == 40 && text.bytes().all(|byte| HEX.contains(&byte)) => {
            json!("$COMMIT")
        }
        Value::Array(items) => Value::Array(items.iter().map(normalize_commits).collect()),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, item)| (key.clone(), normalize_commits(item)))
                .collect(),
        ),
        other => other.clone(),
    }
}

#[test]
#[ignore = "requires AU_ENGINE_BIN; captures or verifies the synthetic read fixtures"]
fn synthetic_read_fixtures_match_the_engine() {
    let engine = std::env::var_os("AU_ENGINE_BIN").expect("set AU_ENGINE_BIN to the au binary");
    let capture = std::env::var_os("CAPTURE_FIXTURES").is_some();
    let temp = tempfile::Builder::new()
        .prefix("au-fixtures-")
        .tempdir_in("/tmp")
        .unwrap();
    let (home, _repo) = build_fixture(temp.path());

    let mut daemon = OwnedChild(
        Command::new(engine)
            .args(["daemon", "start"])
            .arg(&_repo)
            .env("HOME", &home)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("start private engine"),
    );
    let socket = wait_ready(&state_run_dir(&home), &mut daemon.0);
    let mut mcp = OwnedChild(
        Command::new(env!("CARGO_BIN_EXE_2brain"))
            .arg("mcp")
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
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"fixture-capture","version":"1"}}}),
    );
    let init = receive(&rx);
    assert!(init.get("result").is_some(), "{init}");
    send(
        &mut input,
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
    );

    let dir = fixture_dir();
    std::fs::create_dir_all(&dir).unwrap();
    for (index, (tool, arguments)) in fixture_cases().into_iter().enumerate() {
        send(
            &mut input,
            json!({"jsonrpc":"2.0","id":100 + index,"method":"tools/call","params":{"name":tool,"arguments":arguments}}),
        );
        let reply = receive(&rx);
        assert_ne!(reply["result"]["isError"], true, "{tool}: {reply}");
        let payload: Value = serde_json::from_str(
            reply["result"]["content"][0]["text"]
                .as_str()
                .unwrap_or_else(|| panic!("{tool}: no text content in {reply}")),
        )
        .unwrap_or_else(|error| panic!("{tool}: content is not JSON: {error}"));
        let recorded = json!({
            "tool": tool,
            "arguments": arguments,
            "payload": normalize(temp.path(), &payload),
        });
        let path = dir.join(format!("{tool}.json"));
        if capture {
            let mut text = serde_json::to_string_pretty(&recorded).unwrap();
            text.push('\n');
            std::fs::write(&path, text).unwrap();
        } else {
            let stored: Value = serde_json::from_str(
                &std::fs::read_to_string(&path)
                    .unwrap_or_else(|error| panic!("reading {}: {error}", path.display())),
            )
            .unwrap();
            assert_eq!(
                recorded, stored,
                "{tool} no longer matches the recorded fixture; re-capture with CAPTURE_FIXTURES=1 if the change is intentional"
            );
        }
    }
    drop(mcp);
    drop(daemon);
    reader.join().unwrap();
}

/// A workspace with two editable members under ONE git tree: `base` owns the
/// schema and defines `widget`, `app` peers `base` and holds an instance claiming
/// `widget::base`. One enclosing tree is what a structural refactor requires, and
/// it is also why a member grant is the only thing standing between the cascade
/// and `app`.
fn build_two_member_workspace(temp: &Path) -> (PathBuf, PathBuf) {
    let home = temp.join("home");
    let entry = temp.join("entry");
    fs::create_dir_all(home.join(".arsumbris")).unwrap();
    let write = |rel: &str, content: &str| {
        let path = entry.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    };
    write(
        ".arsumbris/repo.yaml",
        "name: v\ntype: au.engine.repo::au-engine\n",
    );
    write(
        ".arsumbris/workspace.yaml",
        "edit:\n  - v\n  - base\n  - app\ntype: au.engine.workspace::au-engine\n",
    );
    write(
        "base/.arsumbris/repo.yaml",
        "name: base\ntype: au.engine.repo::au-engine\n",
    );
    write("base/type/widget.type.yaml", "fields:\n  label: String\n");
    write(
        "app/.arsumbris/repo.yaml",
        "name: app\ntype: au.engine.repo::au-engine\ndeps:\n  - name: base\n",
    );
    write("app/holder.md", "---\ntype: widget::base\nlabel: x\n---\n");
    commit_work_tree(&entry);
    (home, entry)
}

/// A `2brain mcp` process over an existing socket, with the handshake done.
struct Mcp {
    _child: OwnedChild,
    input: std::process::ChildStdin,
    output: mpsc::Receiver<String>,
}

impl Mcp {
    fn start(socket: &Path, home: &Path, extra: &[&str]) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_2brain"));
        child
            .arg("mcp")
            .arg(socket)
            .args(extra)
            .env("HOME", home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = OwnedChild(child.spawn().expect("start MCP server"));
        let mut input = child.0.stdin.take().unwrap();
        let stdout = child.0.stdout.take().unwrap();
        let (tx, output) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                if tx.send(line.expect("read MCP stdout")).is_err() {
                    break;
                }
            }
        });
        send(
            &mut input,
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"rename-type-grant","version":"1"}}}),
        );
        let init = receive(&output);
        assert!(init.get("result").is_some(), "{init}");
        send(
            &mut input,
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        );
        Self {
            _child: child,
            input,
            output,
        }
    }

    /// Call one tool and return the raw reply.
    fn call(&mut self, id: u64, tool: &str, arguments: Value) -> Value {
        send(
            &mut self.input,
            json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":tool,"arguments":arguments}}),
        );
        receive(&self.output)
    }
}

/// The text payload of a tool reply, or the error text when `isError`.
fn reply_text(reply: &Value) -> String {
    reply["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("no text content in {reply}"))
        .to_owned()
}

#[test]
#[ignore = "requires AU_ENGINE_BIN pointing to a separately built au binary"]
fn rename_type_refuses_a_cascade_outside_the_member_grant() {
    let engine = std::env::var_os("AU_ENGINE_BIN").expect("set AU_ENGINE_BIN to the au binary");
    let temp = tempfile::Builder::new()
        .prefix("au-rename-type-grant-")
        .tempdir_in("/tmp")
        .unwrap();
    let (home, entry) = build_two_member_workspace(temp.path());
    let mut daemon = OwnedChild(
        Command::new(engine)
            .args(["daemon", "start"])
            .arg(&entry)
            .env("HOME", &home)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("start private engine"),
    );
    let socket = wait_ready(&state_run_dir(&home), &mut daemon.0);

    let def = entry.join("base/type/widget.type.yaml");
    let note = entry.join("app/holder.md");
    let def_before = fs::read(&def).unwrap();
    let note_before = fs::read(&note).unwrap();

    // Only `base` is allowed: the definition's member, not the referrer's.
    let policy = temp.path().join("policy-base-only.json");
    fs::write(&policy, r#"{"allow_mutations":true,"members":["base"]}"#).unwrap();
    {
        let mut mcp = Mcp::start(
            &socket,
            &home,
            &[
                "--allow-mutations",
                "--policy",
                policy.to_str().unwrap(),
                "--principal",
                "tester",
            ],
        );
        let type_reply = mcp.call(2, "tb_type", json!({"name":"widget::base"}));
        assert_ne!(type_reply["result"]["isError"], true, "{type_reply}");
        let definition: Value = serde_json::from_str(&reply_text(&type_reply)).unwrap();
        let hash = definition["type"]["hash"]
            .as_str()
            .expect("type hash")
            .to_owned();
        assert_eq!(definition["type"]["repo"], "base", "{definition}");

        let reply = mcp.call(
            3,
            "tb_rename_type",
            json!({"old_name":"widget::base","new_name":"gadget","expected_hash":hash}),
        );
        assert_eq!(
            reply["result"]["isError"], true,
            "the cascade leaves the grant, so the call must refuse: {reply}"
        );
        let message = reply_text(&reply);
        assert!(
            message.starts_with("policy/member_grant: "),
            "the refusal keeps the stage vocabulary: {message}"
        );
        assert!(
            message.contains("'app'") && message.contains("holder.md"),
            "it names the member outside the grant and a path there: {message}"
        );
        assert!(
            message.contains("'base'"),
            "it says the defining member is allowed: {message}"
        );
    }

    // The engine was never asked to write: both files are byte-identical.
    assert_eq!(fs::read(&def).unwrap(), def_before, "the def moved");
    assert_eq!(
        fs::read(&note).unwrap(),
        note_before,
        "the referrer changed"
    );
    assert!(
        !entry.join("base/type/gadget.type.yaml").exists(),
        "the rename landed despite the refusal"
    );

    // The same rename, with both members allowed, applies and carries the claim.
    let both = temp.path().join("policy-both.json");
    fs::write(
        &both,
        r#"{"allow_mutations":true,"members":["base","app"]}"#,
    )
    .unwrap();
    {
        let mut mcp = Mcp::start(
            &socket,
            &home,
            &[
                "--allow-mutations",
                "--policy",
                both.to_str().unwrap(),
                "--principal",
                "tester",
            ],
        );
        let type_reply = mcp.call(2, "tb_type", json!({"name":"widget::base"}));
        let definition: Value = serde_json::from_str(&reply_text(&type_reply)).unwrap();
        let hash = definition["type"]["hash"]
            .as_str()
            .expect("type hash")
            .to_owned();
        let reply = mcp.call(
            3,
            "tb_rename_type",
            json!({"old_name":"widget::base","new_name":"gadget","expected_hash":hash}),
        );
        assert_ne!(
            reply["result"]["isError"], true,
            "a fully granted cascade applies: {reply}"
        );
    }
    assert!(
        entry.join("base/type/gadget.type.yaml").exists(),
        "the def file moved to its new name"
    );
    assert!(!def.exists(), "the old def path is gone");
    assert_eq!(
        fs::read_to_string(&note).unwrap(),
        "---\ntype: gadget::base\nlabel: x\n---\n",
        "the referrer's claim followed the rename"
    );
}

#[test]
#[ignore = "requires AU_ENGINE_BIN pointing to a separately built au binary"]
fn mcp_reads_match_real_engine_on_temporary_repo() {
    let engine = std::env::var_os("AU_ENGINE_BIN").expect("set AU_ENGINE_BIN to the au binary");
    let temp = tempfile::Builder::new()
        .prefix("au-e2e-")
        .tempdir_in("/tmp")
        .unwrap();
    let (home, repo) = build_fixture(temp.path());

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
    let socket = wait_ready(&state_run_dir(&home), &mut daemon.0);
    let mut mcp = OwnedChild(
        Command::new(env!("CARGO_BIN_EXE_2brain"))
            .arg("mcp")
            .arg(&socket)
            .arg("--allow-mutations")
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
            json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":format!("tb_{verb}"),"arguments":{}}}),
        );
        let reply = receive(&rx);
        assert_ne!(reply["result"]["isError"], true, "{reply}");
        assert_engine_version(&socket, &reply, &direct);
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
            json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"tb_type","arguments":arguments}}),
        );
        let reply = receive(&rx);
        assert_ne!(reply["result"]["isError"], true, "{reply}");
        assert_engine_version(&socket, &reply, &direct);
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
            json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"tb_types","arguments":arguments}}),
        );
        let reply = receive(&rx);
        assert_ne!(reply["result"]["isError"], true, "{reply}");
        assert_engine_version(&socket, &reply, &direct);
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
            json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"tb_hubs","arguments":arguments}}),
        );
        let reply = receive(&rx);
        assert_ne!(reply["result"]["isError"], true, "{reply}");
        assert_engine_version(&socket, &reply, &direct);
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
            json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"tb_hubs","arguments":arguments}}),
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
            json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"tb_content","arguments":{"repo":"fixture","path":relative}}}),
        );
        let reply = receive(&rx);
        assert_ne!(reply["result"]["isError"], true, "{reply}");
        assert_engine_version(&socket, &reply, &direct);
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
        json!({"jsonrpc":"2.0","id":92,"method":"tools/call","params":{"name":"tb_content","arguments":{"repo":"fixture","path":"../base/asset.pdf"}}}),
    );
    let denied = receive(&rx);
    assert_eq!(denied["result"]["isError"], true, "{denied}");

    // Mutation round trip against the real engine: read, guarded write, re-read.
    send(
        &mut input,
        json!({"jsonrpc":"2.0","id":93,"method":"tools/call","params":{"name":"tb_content","arguments":{"repo":"fixture","path":"notes/a.md"}}}),
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
        json!({"jsonrpc":"2.0","id":96,"method":"tools/call","params":{"name":"tb_preview_mutation","arguments":{"repo":"fixture","path":"notes/a.md","op":"write_file","content":new_text}}}),
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
        json!({"jsonrpc":"2.0","id":94,"method":"tools/call","params":{"name":"tb_write_file","arguments":{"repo":"fixture","path":"notes/a.md","content":new_text,"expected_hash":hash}}}),
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
        json!({"jsonrpc":"2.0","id":95,"method":"tools/call","params":{"name":"tb_write_file","arguments":{"repo":"fixture","path":"notes/a.md","content":new_text,"expected_hash":hash}}}),
    );
    let replay = receive(&rx);
    assert_eq!(replay["result"]["isError"], true, "{replay}");
    assert!(
        replay["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("tb_content"),
        "{replay}"
    );
    // The write advanced the engine version; let the rebuild settle so later
    // response versions are comparable again.
    wait_stable_version(&socket);

    // Edit and delete complete the mutation surface against a real engine.
    let payload_of = |reply: &Value| -> Value {
        serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap()
    };
    let read_hash = |input: &mut std::process::ChildStdin, id: u32| -> String {
        send(
            input,
            json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"tb_content","arguments":{"repo":"fixture","path":"notes/a.md"}}}),
        );
        let reply = receive(&rx);
        assert_ne!(reply["result"]["isError"], true, "{reply}");
        payload_of(&reply)["content"]["hash"]
            .as_str()
            .expect("content hash")
            .to_owned()
    };

    let after_write = read_hash(&mut input, 97);
    send(
        &mut input,
        json!({"jsonrpc":"2.0","id":98,"method":"tools/call","params":{"name":"tb_edit_file","arguments":{"repo":"fixture","path":"notes/a.md","old_string":"rewritten by the MCP test","new_string":"edited by the MCP test","expected_hash":after_write}}}),
    );
    let edited = receive(&rx);
    assert_ne!(edited["result"]["isError"], true, "{edited}");
    wait_stable_version(&socket);
    let on_disk = engine_read_request(
        &socket,
        json!({"read":"content","id":1,"path": repo.join("notes/a.md").to_str().unwrap()}),
    )
    .expect("read edited content");
    let text = on_disk["result"]["content"]["text"].as_str().unwrap();
    assert!(text.contains("edited by the MCP test"), "{text}");
    assert!(!text.contains("rewritten by the MCP test"), "{text}");

    // Reconcile reports an applied write when the current hash is the expected one.
    let before_delete = read_hash(&mut input, 99);
    send(
        &mut input,
        json!({"jsonrpc":"2.0","id":103,"method":"tools/call","params":{"name":"tb_reconcile","arguments":{"repo":"fixture","path":"notes/a.md","expected_hash":before_delete}}}),
    );
    let reconciled = receive(&rx);
    assert_ne!(reconciled["result"]["isError"], true, "{reconciled}");
    assert_eq!(payload_of(&reconciled)["outcome"], "applied");

    // An applied reconcile means the interrupted write produced the current
    // state, so the caller's earlier read is stale: the guard is gone and a
    // mutation needs a fresh read rather than silently reusing a stale hash.
    send(
        &mut input,
        json!({"jsonrpc":"2.0","id":104,"method":"tools/call","params":{"name":"tb_delete_file","arguments":{"repo":"fixture","path":"notes/a.md","expected_hash":before_delete}}}),
    );
    let stale = receive(&rx);
    assert_eq!(stale["result"]["isError"], true, "{stale}");
    assert!(
        stale["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .starts_with("policy/prior_read:"),
        "{stale}"
    );

    let before_delete = read_hash(&mut input, 106);
    send(
        &mut input,
        json!({"jsonrpc":"2.0","id":104,"method":"tools/call","params":{"name":"tb_delete_file","arguments":{"repo":"fixture","path":"notes/a.md","expected_hash":before_delete}}}),
    );
    let deleted = receive(&rx);
    assert_ne!(deleted["result"]["isError"], true, "{deleted}");
    wait_stable_version(&socket);
    let gone = engine_read_request(
        &socket,
        json!({"read":"content","id":1,"path": repo.join("notes/a.md").to_str().unwrap()}),
    )
    .expect("read deleted content");
    assert!(gone["result"]["content"].is_null(), "{gone}");

    // An absent file is what an interrupted delete was meant to produce.
    send(
        &mut input,
        json!({"jsonrpc":"2.0","id":105,"method":"tools/call","params":{"name":"tb_reconcile","arguments":{"repo":"fixture","path":"notes/a.md","expect_absent":true}}}),
    );
    let absent = receive(&rx);
    assert_ne!(absent["result"]["isError"], true, "{absent}");
    assert_eq!(payload_of(&absent)["outcome"], "applied");

    for (id, arguments, request) in [
        (
            100,
            json!({"type_name":"note"}),
            json!({"read":"instances_of","id":1,"type":"note","instance":false,"body":false}),
        ),
        (
            101,
            json!({"type_name":"note","origins":["file"]}),
            json!({"read":"instances_of","id":1,"type":"note","instance":false,"body":false,"origins":["file"]}),
        ),
    ] {
        let direct =
            engine_read_request(&socket, request).expect("read instances_of from private engine");
        assert_eq!(direct["ready"], true, "{direct}");
        send(
            &mut input,
            json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"tb_instances_of","arguments":arguments}}),
        );
        let reply = receive(&rx);
        assert_ne!(reply["result"]["isError"], true, "{reply}");
        assert_engine_version(&socket, &reply, &direct);
        let payload: Value =
            serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(
            payload, direct["result"],
            "MCP instances_of differs from engine"
        );
        assert!(
            payload["instances_of"]
                .as_array()
                .is_some_and(|m| !m.is_empty()),
            "{payload}"
        );
    }

    let validate_arguments = json!({"type_name":"note","value":{"link":"[[b]]"}});
    let validate_request =
        json!({"read":"validate_value","id":1,"type_name":"note","value":{"link":"[[b]]"}});
    let direct =
        engine_read_request(&socket, validate_request).expect("validate value on private engine");
    assert_eq!(direct["ready"], true, "{direct}");
    send(
        &mut input,
        json!({"jsonrpc":"2.0","id":102,"method":"tools/call","params":{"name":"tb_validate_value","arguments":validate_arguments}}),
    );
    let reply = receive(&rx);
    assert_ne!(reply["result"]["isError"], true, "{reply}");
    assert_engine_version(&socket, &reply, &direct);
    let payload: Value =
        serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(
        payload, direct["result"],
        "MCP validate_value differs from engine"
    );
    let verdict = &payload["validate_value"][0];
    assert_eq!(verdict["identity"]["name"], "note", "{payload}");
    assert!(verdict["identity"]["repo"].is_string(), "{payload}");

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
            json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"tb_files","arguments":arguments}}),
        );
        let reply = receive(&rx);
        assert_ne!(reply["result"]["isError"], true, "{reply}");
        assert_engine_version(&socket, &reply, &direct);
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
            json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"tb_imports","arguments":arguments}}),
        );
        let reply = receive(&rx);
        assert_ne!(reply["result"]["isError"], true, "{reply}");
        assert_engine_version(&socket, &reply, &direct);
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
            json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"tb_members","arguments":args}}),
        );
        let reply = receive(&rx);
        assert_ne!(reply["result"]["isError"], true, "{reply}");
        assert_engine_version(&socket, &reply, &direct);
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
            json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"tb_subtypes","arguments":{"base":base,"scope":scope}}}),
        );
        let reply = receive(&rx);
        assert_ne!(reply["result"]["isError"], true, "{reply}");
        assert_engine_version(&socket, &reply, &direct);
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
            json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"tb_diagnostics","arguments":arguments}}),
        );
        let reply = receive(&rx);
        assert_ne!(reply["result"]["isError"], true, "{reply}");
        assert_engine_version(&socket, &reply, &direct);
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
