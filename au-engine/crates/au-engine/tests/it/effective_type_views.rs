//! Effective fields and effective meta on the type views over the socket: the
//! closure's fields each tagged with the def that declares them (a divergent
//! field once per origin), and the meta blocks that survive subtype shadowing,
//! each def read in its own repo.

#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};

use au_engine::{serve, Client, ConfigSource, Engine, ServeHandle};
use serde_json::{json, Value};

/// `base` owns `arity-meta` and an abstract `frame-container` carrying `center?`
/// and an `arity-meta` block. `app` imports `base`:
/// - `sandwich` extends `frame-container::base`, the inherited field and meta.
/// - `broken` (`meta: []`) and `other` both extend `root`, which carries a
///   block; `leaf1` extends `[broken, other]`.
/// - `pa` and `pb` carry different blocks; `leaf2` extends `[pa, pb]`.
/// - `da` and `db` declare `t` with different shapes; `dleaf` extends both.
/// - `note` in both repos, identical fields, different `arity-meta` values.
fn fixture() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap();
    crate::seed_workspace(&root, &["base", "app"]);
    let w = |rel: &str, content: &str| {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, content).unwrap();
    };
    w("base/.arsumbris/repo.yaml", "name: base\n");
    w(
        "base/type/arity-meta.type.yaml",
        "extends: au.engine.meta::au-engine\nfields:\n  arity: Number\n",
    );
    w(
        "base/type/frame-container.type.yaml",
        "abstract: true\nfields:\n  center?: String\nmeta:\n  - type: arity-meta\n    arity: 1\n",
    );
    w(
        "base/type/note.type.yaml",
        "fields:\n  title: String\nmeta:\n  - type: arity-meta\n    arity: 7\n",
    );
    w(
        "app/.arsumbris/repo.yaml",
        "name: app\ndeps:\n  - name: base\n",
    );
    let block = |n: u32| format!("meta:\n  - type: arity-meta::base\n    arity: {n}\n");
    w(
        "app/type/sandwich.type.yaml",
        "extends: frame-container::base\nfields:\n  top?: String\n",
    );
    w(
        "app/type/root.type.yaml",
        &format!("fields: {{}}\n{}", block(3)),
    );
    w("app/type/broken.type.yaml", "extends: root\nmeta: []\n");
    w("app/type/other.type.yaml", "extends: root\n");
    w(
        "app/type/leaf1.type.yaml",
        "extends:\n  - broken\n  - other\n",
    );
    w(
        "app/type/pa.type.yaml",
        &format!("fields: {{}}\n{}", block(1)),
    );
    w(
        "app/type/pb.type.yaml",
        &format!("fields: {{}}\n{}", block(2)),
    );
    w("app/type/leaf2.type.yaml", "extends:\n  - pa\n  - pb\n");
    w("app/type/da.type.yaml", "fields:\n  t?: String\n");
    w("app/type/db.type.yaml", "fields:\n  t?: Number\n");
    w("app/type/dleaf.type.yaml", "extends:\n  - da\n  - db\n");
    w(
        "app/type/note.type.yaml",
        &format!("fields:\n  title: String\n{}", block(9)),
    );
    (dir, root)
}

struct Harness {
    _dir: tempfile::TempDir,
    _sock_dir: tempfile::TempDir,
    _engine: Engine,
    _server: ServeHandle,
    client: Client,
}

fn started() -> Harness {
    started_with(|_| {})
}

/// The fixture, with `extra` writing further files before the engine starts.
fn started_with(extra: impl FnOnce(&Path)) -> Harness {
    let (dir, root) = fixture();
    extra(&root);
    let sock_dir = tempfile::tempdir().unwrap();
    let socket = sock_dir.path().join("s");
    let engine = Engine::new(&root, ConfigSource::Empty);
    let server = serve(engine.handle(), &socket).expect("serve");
    engine.rebuild();
    let client = Client::connect(&socket).expect("connect");
    Harness {
        _dir: dir,
        _sock_dir: sock_dir,
        _engine: engine,
        _server: server,
        client,
    }
}

impl Harness {
    fn read(&mut self, q: Value) -> Value {
        let resp = self.client.query(&q).unwrap();
        assert_eq!(resp["ready"], true, "{resp}");
        resp["result"].clone()
    }

    fn ty(&mut self, name: &str) -> Value {
        self.read(json!({ "read": "type", "name": name }))["type"].clone()
    }
}

/// `(field name, origin name, origin repo, divergent)` per effective field.
fn fields(def: &Value) -> Vec<(String, String, String, bool)> {
    def["effective_fields"]
        .as_array()
        .unwrap_or_else(|| panic!("no effective_fields: {def}"))
        .iter()
        .map(|f| {
            (
                f["name"].as_str().unwrap().to_string(),
                f["origin"]["name"].as_str().unwrap().to_string(),
                f["origin"]["repo"].as_str().unwrap().to_string(),
                f["divergent"].as_bool().unwrap(),
            )
        })
        .collect()
}

fn t(name: &str, origin: &str, repo: &str, divergent: bool) -> (String, String, String, bool) {
    (name.into(), origin.into(), repo.into(), divergent)
}

/// The `arity` values of the one effective `arity-meta::base` entry, with each
/// block's declaring def.
fn arity(def: &Value) -> Vec<(i64, String, String)> {
    let metas = def["effective_meta"].as_array().unwrap();
    let Some(entry) = metas.iter().find(|m| {
        m["meta_type"]["name"] == "arity-meta" && m["meta_type"]["type_owners"] == json!(["base"])
    }) else {
        return Vec::new();
    };
    entry["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| {
            let value = b["body"]
                .as_array()
                .unwrap()
                .iter()
                .find(|f| f["name"] == "arity")
                .unwrap()["value"]
                .as_i64()
                .unwrap();
            (
                value,
                b["from"]["name"].as_str().unwrap().to_string(),
                b["from"]["repo"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

#[test]
fn an_inherited_field_and_meta_surface_across_the_repo_boundary() {
    let mut h = started();
    let sandwich = h.ty("sandwich");
    assert_eq!(
        fields(&sandwich),
        vec![
            t("center", "frame-container", "base", false),
            t("top", "sandwich", "app", false)
        ]
    );
    assert_eq!(
        arity(&sandwich),
        vec![(1, "frame-container".into(), "base".into())]
    );
    // The block's source is in the DECLARING def's file, the peer's.
    let src = sandwich["effective_meta"][0]["blocks"][0]["source"]["file"]
        .as_str()
        .unwrap();
    assert!(
        src.ends_with("base/type/frame-container.type.yaml"),
        "{src}"
    );
    // Own `fields` / `meta_blocks` stay the def's own.
    assert_eq!(sandwich["fields"].as_array().unwrap().len(), 1);
    assert_eq!(sandwich["meta_blocks"], Value::Null);
}

#[test]
fn a_break_shadows_its_ancestor_on_every_path() {
    let mut h = started();
    assert!(arity(&h.ty("leaf1")).is_empty(), "{}", h.ty("leaf1"));
    assert!(
        arity(&h.ty("other")).len() == 1,
        "other still inherits root's block"
    );
}

#[test]
fn unrelated_parents_surface_both_blocks_and_the_conflict_fires() {
    let mut h = started();
    assert_eq!(
        arity(&h.ty("leaf2")),
        vec![
            (1, "pa".into(), "app".into()),
            (2, "pb".into(), "app".into())
        ]
    );
    let diags = h.read(json!({ "read": "diagnostics", "code": "meta-inheritance-conflict" }));
    let files: Vec<String> = diags["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["span"]["file"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(files.len(), 1, "{diags}");
    assert!(
        Path::new(&files[0]).ends_with("app/type/leaf2.type.yaml"),
        "{files:?}"
    );
}

#[test]
fn a_divergent_field_is_one_entry_per_origin_on_the_views_and_type_closure() {
    let mut h = started();
    let want = vec![t("t", "da", "app", true), t("t", "db", "app", true)];
    assert_eq!(fields(&h.ty("dleaf")), want);
    let dleaf = h.ty("dleaf");
    let shapes: Vec<&str> = dleaf["effective_fields"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["shape"].as_str().unwrap())
        .collect();
    assert_eq!(shapes, vec!["String", "Number"]);

    let closure = h.read(json!({ "read": "type_closure", "name": "dleaf" }));
    let entry = &closure["type_closure"][0];
    let got: Vec<(String, String, bool)> = entry["fields"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| {
            (
                f["name"].as_str().unwrap().to_string(),
                f["origin"]["name"].as_str().unwrap().to_string(),
                f["divergent"].as_bool().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        got,
        vec![
            ("t".into(), "da".into(), true),
            ("t".into(), "db".into(), true)
        ]
    );
}

#[test]
fn in_sync_twins_each_surface_their_own_meta() {
    let mut h = started();
    let own = h.ty("note::app");
    let peer = h.ty("note::base");
    assert_eq!(own["hash"], peer["hash"], "the twins are one identity");
    assert_eq!(arity(&own), vec![(9, "note".into(), "app".into())]);
    assert_eq!(arity(&peer), vec![(7, "note".into(), "base".into())]);
}

#[test]
fn every_full_mode_view_carries_them_and_summary_does_not() {
    let mut h = started();
    let full = h.read(json!({ "read": "types", "repo": "app" }));
    for def in full["types"].as_array().unwrap() {
        assert!(def.get("effective_fields").is_some(), "types: {def}");
        assert!(def.get("effective_meta").is_some(), "types: {def}");
    }
    let batch = h.read(json!({ "read": "type_batch", "names": ["sandwich"] }));
    assert_eq!(fields(&batch["type_batch"][0]).len(), 2, "{batch}");
    let subs = h.read(json!({ "read": "subtypes", "base": "frame-container::base" }));
    let sandwich = subs["subtypes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == "sandwich")
        .unwrap_or_else(|| panic!("{subs}"))
        .clone();
    assert_eq!(
        arity(&sandwich),
        vec![(1, "frame-container".into(), "base".into())]
    );

    let summary = h.read(json!({ "read": "types", "repo": "app", "summary": true }));
    for def in summary["types"].as_array().unwrap() {
        assert!(def.get("effective_fields").is_none(), "summary: {def}");
        assert!(def.get("effective_meta").is_none(), "summary: {def}");
    }
}

#[test]
fn an_in_sync_meta_type_lists_every_owner() {
    // `app` defines its own `arity-meta`, byte-identical to base's: ONE meta
    // identity both repos own.
    let mut h = started_with(|root| {
        fs::write(
            root.join("app/type/arity-meta.type.yaml"),
            "extends: au.engine.meta::au-engine\nfields:\n  arity: Number\n",
        )
        .unwrap();
    });
    let sandwich = h.ty("sandwich");
    let metas = sandwich["effective_meta"].as_array().unwrap();
    assert_eq!(metas.len(), 1, "{sandwich}");
    assert_eq!(metas[0]["meta_type"]["name"], "arity-meta");
    assert_eq!(metas[0]["meta_type"]["type_owners"], json!(["app", "base"]));
    assert_eq!(metas[0]["meta_type"]["hash"].as_str().unwrap().len(), 16);
}
