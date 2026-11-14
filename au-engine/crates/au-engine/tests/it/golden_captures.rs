//! The golden captures in `tests-golden/engine/` are checked here, not compared.
//!
//! Comparing them needs a running engine over a frozen copy of the production
//! vault, so that check lives outside the default suite (see
//! `tests-golden/README.md`). What CAN be checked without an engine is that the
//! captures are still usable as evidence: each file parses, names a read the
//! engine actually serves, carries that read's key with the right JSON kind, and
//! has already been normalized (no absolute workspace path, no rebuild counter).
//! A capture that stops satisfying this is worse than a missing one, because a
//! reviewer would take it as proof.

#![cfg(unix)]

use std::fs;
use std::path::PathBuf;

use serde_json::Value;

/// The captures, with the read verb each one pins and whether that read's payload
/// is a JSON array: `hubs` and `diagnostics` answer with lists, `overview` and
/// `graph_health` with objects.
const CAPTURES: &[(&str, &str, bool)] = &[
    ("au_overview.json", "overview", false),
    ("au_hubs.json", "hubs", true),
    ("au_diagnostics.json", "diagnostics", true),
    ("au_graph_health.json", "graph_health", false),
];

fn capture_dir() -> PathBuf {
    // crates/au-engine -> the workspace root that holds tests-golden/.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests-golden/engine")
}

fn load(name: &str) -> Value {
    let path = capture_dir().join(name);
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

#[test]
fn every_capture_names_a_read_and_carries_its_payload() {
    for (file, verb, array) in CAPTURES {
        let capture = load(file);
        let payload = capture
            .get(*verb)
            .unwrap_or_else(|| panic!("{file}: no `{verb}` key; top level is {capture}"));
        assert!(
            payload.is_array() == *array,
            "{file}: `{verb}` should be {}",
            if *array { "an array" } else { "an object" }
        );
    }
}

#[test]
fn captures_are_normalized() {
    for (file, _, _) in CAPTURES {
        let text = fs::read_to_string(capture_dir().join(file)).expect("read");
        assert!(
            !text.contains("/root/vault"),
            "{file}: carries an absolute workspace path; re-capture through scripts/engine_probe.py"
        );
        assert!(
            !text.contains("\"version\""),
            "{file}: carries the rebuild counter; the probe drops it"
        );
    }
}

#[test]
fn the_capture_set_is_complete() {
    let dir = capture_dir();
    let mut found: Vec<String> = fs::read_dir(&dir)
        .unwrap_or_else(|error| panic!("{}: {error}", dir.display()))
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".json"))
        .collect();
    found.sort();
    let mut expected: Vec<String> = CAPTURES.iter().map(|(file, _, _)| file.to_string()).collect();
    expected.sort();
    assert_eq!(
        found, expected,
        "the capture set changed: record the new file in CAPTURES or delete it"
    );
}
