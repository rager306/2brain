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
//!
//! Paging is the third shape of the same class: the adapter adds `next_offset` to a
//! paged answer and sets it to null when the page ends the sequence, so the declaration
//! has to accept null. A capture of a first page cannot show that, which is why the
//! paging tests below drive the tool to its last page over a fake engine instead of
//! reading a fixture.

use std::{
    collections::BTreeMap,
    os::unix::net::{UnixListener, UnixStream},
    path::Path,
    thread,
    time::{Duration, Instant},
};

use serde_json::{Value, json};

mod support;

use support::Client;

/// How long a fake engine waits for the adapter, so a scenario that never reaches
/// the engine fails instead of hanging the suite.
const ENGINE_WAIT: Duration = Duration::from_secs(10);

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

/// The output schemas a launch advertises, by tool name: the contract a strict client
/// validates each answer against.
fn declared_schemas(client: &mut Client) -> BTreeMap<String, Value> {
    let listed =
        client.request(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}}));
    let mut schemas: BTreeMap<String, Value> = BTreeMap::new();
    for tool in listed["result"]["tools"].as_array().unwrap() {
        schemas.insert(
            tool["name"].as_str().unwrap().to_owned(),
            tool["outputSchema"].clone(),
        );
    }
    assert!(!schemas.is_empty(), "no tool declares an output schema");
    schemas
}

/// Validates one answer against its tool's declaration, at every depth.
#[track_caller]
fn assert_declares(tool: &str, answer: &Value, schemas: &BTreeMap<String, Value>) {
    let schema = schemas
        .get(tool)
        .unwrap_or_else(|| panic!("{tool}: no such advertised tool"));
    let mut problems = Vec::new();
    walk(answer, schema, tool, &mut problems);
    assert!(
        problems.is_empty(),
        "{tool} answers a shape it does not declare:\n{}",
        problems.join("\n")
    );
}

/// The captured answer for one read, so a scenario pages a real engine answer
/// instead of a hand-written one.
fn captured(tool: &str) -> Value {
    fixtures("tests/fixtures/synthetic-reads")
        .into_iter()
        .find(|(name, _)| name == tool)
        .unwrap_or_else(|| panic!("no captured fixture for {tool}"))
        .1
}

/// Accepts the engine connection the next read makes, failing rather than hanging
/// when the adapter never connects.
fn accept(listener: &UnixListener) -> UnixStream {
    let deadline = Instant::now() + ENGINE_WAIT;
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                stream.set_read_timeout(Some(ENGINE_WAIT)).unwrap();
                stream.set_write_timeout(Some(ENGINE_WAIT)).unwrap();
                return stream;
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(
                    Instant::now() < deadline,
                    "the adapter never connected to the fake engine"
                );
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("engine accept failed: {error}"),
        }
    }
}

/// Calls `tool` against a fake engine that answers `verb` with `answer`, and returns
/// the answer and the schemas the same launch declares.
#[track_caller]
fn call_against_engine(
    tool: &str,
    verb: &str,
    arguments: Value,
    answer: Value,
) -> (Value, BTreeMap<String, Value>) {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let verb = verb.to_owned();
    let handled = thread::spawn(move || {
        let mut stream = accept(&listener);
        let request = support::read_request(&mut stream);
        assert_eq!(request["read"], json!(verb), "wire verb");
        support::write_response_frame(&mut stream, 9, answer);
    });

    let mut client = Client::start_with_timeout(&socket, &[], ENGINE_WAIT);
    let schemas = declared_schemas(&mut client);
    let result = client.call_result(2, tool, arguments);
    assert_ne!(result["isError"], true, "{tool}: {result}");
    drop(client);
    handled.join().unwrap();

    let structured = result["structuredContent"].clone();
    assert!(
        !structured.is_null(),
        "{tool}: the answer carries no structuredContent"
    );
    (structured, schemas)
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
    let schemas = declared_schemas(&mut client);

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

/// The page that ends a sequence answers `next_offset: null`, which a strict client
/// rejects unless the declaration says so. Found on the live server on 2026-11-15:
/// `tb_instances_of {"type_name":"moc","limit":200}` came back to a client as
/// "None is not of type 'integer'" — the read itself had succeeded.
#[test]
fn a_page_that_ends_the_sequence_declares_its_null_offset() {
    let records = captured("tb_instances_of")["instances_of"].clone();
    let total = records.as_array().unwrap().len();
    let (answer, schemas) = call_against_engine(
        "tb_instances_of",
        "instances_of",
        // Exactly the whole set: one page that is also the last page.
        json!({"type_name": "note", "limit": total}),
        json!({"instances_of": records}),
    );
    assert_eq!(answer["total"], json!(total), "{answer}");
    assert_eq!(answer["offset"], json!(0), "{answer}");
    assert_eq!(
        answer["next_offset"],
        Value::Null,
        "this page must end the sequence: {answer}"
    );
    assert_declares("tb_instances_of", &answer, &schemas);
}

/// Every read the adapter pages itself (`page_array` in `src/lib.rs`) must satisfy its
/// declaration partway through the sequence, where `next_offset` is a number, and on
/// the page that ends it, where `next_offset` is null. Reverting either union to
/// `integer` fails this test.
#[test]
fn every_adapter_paged_read_validates_on_its_last_page() {
    // (tool, wire verb, key the records sit under, the arguments that select the read)
    let paged = [
        (
            "tb_instances_of",
            "instances_of",
            "instances_of",
            json!({"type_name": "note"}),
        ),
        ("tb_instances", "instances", "instances", json!({})),
    ];
    for (tool, verb, key, base) in paged {
        let records = captured(tool)[key].clone();
        let total = records.as_array().unwrap().len();
        assert!(
            total >= 2,
            "{tool}: a capture of {total} record(s) cannot page"
        );
        let mut body = serde_json::Map::new();
        body.insert(key.to_owned(), records);
        let body = Value::Object(body);

        // Partway: one record withheld, so `next_offset` is a number.
        let mut arguments = base.clone();
        arguments["limit"] = json!(total - 1);
        arguments["offset"] = json!(0);
        let (answer, schemas) = call_against_engine(tool, verb, arguments, body.clone());
        assert_eq!(
            answer["next_offset"],
            json!(total - 1),
            "{tool} partway: {answer}"
        );
        assert_declares(tool, &answer, &schemas);

        // The last page: the sequence ends here, so `next_offset` is null.
        let mut arguments = base;
        arguments["limit"] = json!(1);
        arguments["offset"] = json!(total - 1);
        let (answer, schemas) = call_against_engine(tool, verb, arguments, body);
        assert_eq!(
            answer["next_offset"],
            Value::Null,
            "{tool} last page: {answer}"
        );
        assert_declares(tool, &answer, &schemas);
    }
}
