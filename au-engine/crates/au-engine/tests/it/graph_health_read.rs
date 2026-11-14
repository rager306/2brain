//! The `graph_health` read: cycles, dead ends, sources, broken links, and
//! centrality joined in one answer.

#![cfg(unix)]

use std::fs;
use std::path::PathBuf;

use au_engine::{Client, ConfigSource, Engine, ServeHandle, serve};
use serde_json::{Value, json};

struct Harness {
    _dir: tempfile::TempDir,
    _sock_dir: tempfile::TempDir,
    _server: ServeHandle,
    client: Client,
}

fn harness(root: &std::path::Path) -> Harness {
    let dir_keep = tempfile::tempdir().unwrap();
    let sock_dir = tempfile::tempdir().unwrap();
    let socket = sock_dir.path().join("s");
    let engine = Engine::new(root, ConfigSource::Empty);
    engine.rebuild();
    let server = serve(engine.handle(), &socket).expect("serve");
    let client = Client::connect(&socket).expect("connect");
    Harness {
        _dir: dir_keep,
        _sock_dir: sock_dir,
        _server: server,
        client,
    }
}

fn write(root: &std::path::Path, name: &str, body: &str) {
    fs::create_dir_all(root.join("notes")).unwrap();
    fs::write(root.join("notes").join(name), body).unwrap();
}

/// Two notes pointing at each other, one dead end, one island, one broken link.
fn seeded() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap();
    crate::seed_repo(&root);
    write(&root, "a.md", "---\ntitle: a\n---\n\nsee [[b]]\n");
    write(&root, "b.md", "---\ntitle: b\n---\n\nsee [[a]]\n");
    write(&root, "c.md", "---\ntitle: c\n---\n\nno links out\n");
    write(&root, "island.md", "---\ntitle: island\n---\n\nlonely\n");
    write(&root, "d.md", "---\ntitle: d\n---\n\nsee [[c]]\n");
    // A dangling link is only diagnosed on a file that claims a type: an
    // untyped note is not an instance, so its body is not validated.
    fs::create_dir_all(root.join("type")).unwrap();
    fs::write(
        root.join("type/note.type.yaml"),
        "fields:\n  title: String\n",
    )
    .unwrap();
    write(
        &root,
        "broken.md",
        "---\ntype: note\ntitle: broken\n---\n\nsee [[nowhere-at-all]]\n",
    );
    (dir, root)
}

fn health(client: &mut Client, args: Value) -> Value {
    let resp = client.query(&json!({ "read": "graph_health" })).unwrap();
    assert_eq!(resp["ready"], true, "{resp}");
    resp["result"]["graph_health"].clone()
}

#[test]
fn graph_health_reports_the_cycle_and_the_problems() {
    let (_dir, root) = seeded();
    let mut h = harness(&root);
    let _ = harness;
    let view = health(&mut h.client, json!({}));
    let counts = &view["counts"];
    assert!(counts["nodes"].as_u64().unwrap() >= 5, "{counts}");
    assert!(counts["edges"].as_u64().unwrap() >= 3, "{counts}");
    let cycles = view["cycles"].as_array().unwrap();
    assert_eq!(cycles.len(), 1, "{cycles:?}");
    assert_eq!(cycles[0]["length"], 2);
    let cycle_nodes: Vec<&str> = cycles[0]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(cycle_nodes[0].ends_with("a.md"), "{cycle_nodes:?}");
    assert!(cycle_nodes[1].ends_with("b.md"), "{cycle_nodes:?}");
    assert_eq!(view["counts"]["nodes_in_cycles"], 2);
    let islands: Vec<&str> = view["isolated"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["path"].as_str().unwrap())
        .collect();
    assert!(
        islands.iter().any(|p| p.ends_with("island.md")),
        "{islands:?}"
    );
    let dead: Vec<&str> = view["dead_ends"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["path"].as_str().unwrap())
        .collect();
    assert!(dead.iter().any(|p| p.ends_with("c.md")), "{dead:?}");
    assert!(
        !dead.iter().any(|p| p.ends_with("island.md")),
        "an island is not a dead end: it has no inbound edge either"
    );
    let broken = view["broken_links"].as_array().unwrap();
    assert_eq!(broken.len(), 1, "{broken:?}");
    assert!(broken[0]["path"].as_str().unwrap().ends_with("broken.md"));
    // A prose wiki-link that resolves to nothing reports as navigational; the
    // typed-slot sibling reports as reference-target-missing. Both count.
    assert_eq!(broken[0]["code"], "navigational-target-not-found");
    assert!(broken[0]["line"].is_number());
    // a and b are the crossroads: both sit in the cycle.
    let central = view["central"].as_array().unwrap();
    assert!(central[0]["total"].as_u64().unwrap() >= 2, "{central:?}");
    assert_eq!(view["truncated"], false);
}

#[test]
fn graph_health_caps_every_list_but_counts_them_all() {
    let (_dir, root) = seeded();
    let mut h = harness(&root);
    let resp = h
        .client
        .query(&json!({ "read": "graph_health", "limit": 1 }))
        .unwrap();
    let view = &resp["result"]["graph_health"];
    assert_eq!(view["counts"]["nodes_in_cycles"], 2);
    assert_eq!(view["counts"]["broken_links"], 1);
    assert_eq!(view["central"].as_array().unwrap().len(), 1);
    assert_eq!(view["truncated"], true, "a list was cut: {view}");
    let unknown = h
        .client
        .query(&json!({ "read": "graph_health", "depth": 2 }))
        .unwrap();
    assert_eq!(unknown["type"], "error", "{unknown}");
}
