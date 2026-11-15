#![cfg(unix)]

//! Validate every captured payload against its declared schema, at every depth.
//!
//! `schema_contract.rs` checks key sets and `required`. This checks TYPES recursively,
//! because that is where both schema bugs lived: `commits` was declared an array while
//! the engine sends an object, and `hash` was declared a string while a delete sends
//! null. A key-set check sees neither.
//!
//! The fixtures are captured from a real engine (`scripts/capture-mutation-fixtures.py`
//! for mutations, the ignored real-engine test for reads), so a failure here means the
//! adapter promises a shape it does not deliver — which a validating client turns into
//! "Invalid structured content" after the mutation already landed.

use std::{collections::BTreeMap, path::Path};

use serde_json::Value;

mod support;

use support::Client;

fn fixtures(dir: &str) -> Vec<(String, Value)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join(dir);
    let mut out: Vec<(String, Value)> = Vec::new();
    for entry in
        std::fs::read_dir(&root).unwrap_or_else(|error| panic!("{}: {error}", root.display()))
    {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        // The fixture is either the payload itself (mutations) or {tool, payload} (reads).
        let stored: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let (name, payload) = match (stored.get("tool"), stored.get("payload")) {
            (Some(tool), Some(payload)) => (tool.as_str().unwrap().to_owned(), payload.clone()),
            _ => (
                path.file_stem().unwrap().to_string_lossy().into_owned(),
                stored,
            ),
        };
        out.push((name, payload));
    }
    out
}

/// The JSON type name of a value, with `integer` separated from `number` because JSON
/// Schema distinguishes them and a declared `integer` must not accept `1.5`.
fn kind(value: &Value) -> &'static str {
    match value {
        Value::Bool(_) => "boolean",
        Value::Number(number) if number.is_i64() || number.is_u64() => "integer",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
        Value::Null => "null",
    }
}

fn walk(value: &Value, schema: &Value, path: &str, problems: &mut Vec<String>) {
    if schema.get("oneOf").is_some() || schema.get("anyOf").is_some() {
        return; // a union of shapes needs a real validator; out of scope here
    }
    if let Some(declared) = schema.get("type") {
        let declared_types: Vec<&str> = match declared {
            Value::String(kind) => vec![kind.as_str()],
            Value::Array(kinds) => kinds.iter().filter_map(Value::as_str).collect(),
            _ => Vec::new(),
        };
        let actual = kind(value);
        let accepted = declared_types.contains(&actual)
            || (actual == "integer" && declared_types.contains(&"number"));
        if !declared_types.is_empty() && !accepted {
            problems.push(format!(
                "{path}: declared [{}] but holds {actual}",
                declared_types.join(", ")
            ));
            return;
        }
    }
    if let (Value::Object(object), Some(properties)) = (value, schema.get("properties")) {
        for (key, item) in object {
            match properties.get(key) {
                Some(declared) => walk(item, declared, &format!("{path}.{key}"), problems),
                None if schema.get("additionalProperties") == Some(&Value::Bool(false)) => {
                    problems.push(format!("{path}.{key}: not declared"));
                }
                None => {}
            }
        }
    }
    if let (Value::Array(items), Some(items_schema)) = (value, schema.get("items")) {
        for (index, item) in items.iter().take(20).enumerate() {
            walk(item, items_schema, &format!("{path}[{index}]"), problems);
        }
    }
}

#[test]
fn every_captured_payload_matches_its_declared_types() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    // Mutations are only advertised with grants, and their payloads are half of what
    // this test checks: a read-only launch would silently skip them.
    let mut client = Client::start_with(
        &socket,
        &[
            "--allow-mutations",
            "--allow-composition",
            "--allow-workspace-shape",
        ],
    );
    let listed = client.request(serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}
    }));
    let mut schemas: BTreeMap<String, Value> = BTreeMap::new();
    for tool in listed["result"]["tools"].as_array().unwrap() {
        schemas.insert(
            tool["name"].as_str().unwrap().to_owned(),
            tool["outputSchema"].clone(),
        );
    }
    assert!(!schemas.is_empty(), "no tool declares an output schema");

    let mut checked = 0;
    let mut problems: Vec<String> = Vec::new();
    for (name, payload) in fixtures("tests/fixtures/synthetic-reads")
        .into_iter()
        .chain(fixtures("tests/fixtures/mutations"))
    {
        let Some(schema) = schemas.get(&name) else {
            problems.push(format!("{name}: no such advertised tool"));
            continue;
        };
        walk(&payload, schema, &name, &mut problems);
        checked += 1;
    }
    assert!(checked >= 32, "only {checked} fixtures were checked");
    assert!(problems.is_empty(), "type drift:\n{}", problems.join("\n"));
}
