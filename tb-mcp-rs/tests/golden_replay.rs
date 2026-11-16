#![cfg(unix)]

//! Replay the production-captured payloads from `tb-engine/tests-golden`
//! through the MCP layer.
//!
//! The fixtures are tool payloads captured on the live stack before any fork
//! change. This test does not re-capture them: it proves the adapter passes the
//! same data through unchanged, so a silent normalisation (re-sorting, dropping
//! fields, renaming keys) fails here instead of at a client.

use std::{
    os::unix::net::UnixListener,
    path::{Path, PathBuf},
    thread,
    time::Duration,
};

use serde_json::{Value, json};

mod support;

use support::{Client, read_request, write_frame};

const TIMEOUT: Duration = Duration::from_secs(10);

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../tb-engine/tests-golden")
        .canonicalize()
        .expect("the golden fixtures must exist next to au-mcp-rs")
}

fn golden(name: &str) -> Value {
    let path = golden_dir().join(name);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("reading {}: {error}", path.display()));
    serde_json::from_str(&text)
        .unwrap_or_else(|error| panic!("parsing {}: {error}", path.display()))
}

fn run_case(tool: &str, request: Value, result_payload: Value, expected: Value) {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let daemon = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream.set_read_timeout(Some(TIMEOUT)).unwrap();
        assert_eq!(read_request(&mut stream), request);
        write_frame(
            &mut stream,
            json!({"type":"response","schema_version":29,"id":1,"ready":true,"version":42,
                "result": result_payload}),
        );
    });
    let mut client = Client::start_with_timeout(&socket, &[], TIMEOUT);
    let reply = client.call(2, tool, json!({}));
    assert_ne!(reply["result"]["isError"], true, "{reply}");
    let payload: Value =
        serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(payload, expected, "{tool} altered the captured payload");
    assert_eq!(reply["result"]["_meta"]["io.2brain/engineVersion"], "42");
    daemon.join().unwrap();
}

#[test]
fn the_adapter_passes_captured_production_payloads_through_unchanged() {
    let overview = golden("tb_overview.json");
    run_case(
        "tb_overview",
        json!({"read":"overview","id":1}),
        json!({"overview": overview.clone()}),
        json!({"overview": overview}),
    );

    let hubs = golden("tb_hubs.json");
    run_case(
        "tb_hubs",
        json!({"read":"hubs","id":1,"limit":20,"offset":0}),
        json!({"hubs": hubs.clone()}),
        json!({"hubs": hubs}),
    );

    // The diagnostics fixture is the bridge's envelope; the engine serves the
    // same array under the read's own key.
    let diagnostics = golden("tb_diagnostics.json");
    let array = diagnostics["diagnostics"].clone();
    assert!(array.is_array(), "{diagnostics}");
    run_case(
        "tb_diagnostics",
        json!({"read":"diagnostics","id":1,"limit":50,"offset":0}),
        json!({"diagnostics": array.clone()}),
        json!({"diagnostics": array}),
    );
}
