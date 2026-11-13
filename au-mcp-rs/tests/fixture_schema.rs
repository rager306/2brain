#![cfg(unix)]

//! Keep the synthetic read corpus honest without an engine: every committed
//! fixture must parse, name an advertised read-only tool, and carry a non-empty
//! payload under the key that read documents.

use std::{collections::BTreeSet, path::Path};

use serde_json::Value;

fn fixture_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/synthetic-reads")
}

/// The payload key each fixture must carry, so an empty answer cannot pass as a
/// captured one.
const PAYLOAD_KEYS: [(&str, &str); 16] = [
    ("au_overview", "overview"),
    ("au_top_level_dirs", "top_level_dirs"),
    ("au_type", "type"),
    ("au_type_tree", "type_tree"),
    ("au_types", "types"),
    ("au_hubs", "hubs"),
    ("au_diagnostics", "diagnostics"),
    ("au_subtypes", "base"),
    ("au_members", "members"),
    ("au_imports", "imports"),
    ("au_files", "files"),
    ("au_content", "content"),
    ("au_instances_of", "instances_of"),
    ("au_validate_value", "validate_value"),
    ("au_preview_mutation", "preview_mutation"),
    ("au_reconcile", "outcome"),
];

#[test]
fn every_fixture_parses_and_names_a_read_only_tool() {
    let dir = fixture_dir();
    let mut seen = BTreeSet::new();
    for entry in std::fs::read_dir(&dir).unwrap_or_else(|error| {
        panic!(
            "reading {}: {error}; the corpus is captured by \
             `CAPTURE_FIXTURES=1 cargo test --test real_engine -- --ignored`",
            dir.display()
        )
    }) {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        let name = path.file_stem().unwrap().to_str().unwrap().to_owned();
        assert!(
            seen.insert(name.clone()),
            "duplicate fixture for {name}: {}",
            path.display()
        );
        let stored: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap())
            .unwrap_or_else(|error| panic!("parsing {}: {error}", path.display()));
        assert_eq!(
            stored["tool"],
            name,
            "{} names another tool",
            path.display()
        );
        assert!(
            stored["arguments"].is_object(),
            "{} has no recorded arguments",
            path.display()
        );
        let payload = &stored["payload"];
        assert!(
            payload.is_object(),
            "{} payload is not an object",
            path.display()
        );
        // Multi-key payloads are the reads whose result is not one collection:
        // `subtypes` carries its base name and `reconcile` its outcome triple.
        assert!(
            payload.as_object().unwrap().len() <= 3,
            "{} carries an unexpected payload shape: {payload}",
            path.display()
        );
        let text = payload.to_string();
        assert!(
            !text.contains("/tmp/"),
            "{} leaks an absolute temporary path",
            path.display()
        );
    }
    for (tool, _) in PAYLOAD_KEYS {
        assert!(seen.contains(tool), "{tool} has no captured fixture");
    }
    assert_eq!(seen.len(), PAYLOAD_KEYS.len(), "{seen:?}");
}

#[test]
fn every_fixture_payload_key_is_documented() {
    for (tool, key) in PAYLOAD_KEYS {
        let path = fixture_dir().join(format!("{tool}.json"));
        let stored: Value = serde_json::from_str(
            &std::fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("reading {}: {error}", path.display())),
        )
        .unwrap();
        assert!(
            stored["payload"].get(key).is_some(),
            "{tool} must carry {key}: {}",
            stored["payload"]
        );
        assert!(
            !stored["payload"][key].is_null(),
            "{tool} captured a null {key}"
        );
    }
}
