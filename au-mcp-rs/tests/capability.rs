#![cfg(unix)]

//! The capability axis: one class per advertised tool, filterable at discovery, and
//! never a way around a grant. The table itself is asserted in the crate's own tests;
//! these checks go through the wire a client actually uses.

use std::{path::Path, time::Duration};

use serde_json::{Value, json};

mod support;

use support::Client;

const TIMEOUT: Duration = Duration::from_secs(10);

fn connect(extra: &[&str]) -> Client {
    Client::start_with_timeout(Path::new("/nonexistent.sock"), extra, TIMEOUT)
}

fn search(client: &mut Client, arguments: Value) -> Vec<String> {
    let result = client.call_result(2, "au_search_tools", arguments);
    assert_ne!(result["isError"], true, "{result}");
    result["structuredContent"]["tools"]
        .as_array()
        .unwrap_or_else(|| panic!("no tools array: {result}"))
        .iter()
        .map(|tool| tool["name"].as_str().unwrap().to_owned())
        .collect()
}

fn advertised(client: &mut Client) -> Vec<String> {
    let reply = client.request(json!({"jsonrpc":"2.0","id":3,"method":"tools/list","params":{}}));
    reply["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap().to_owned())
        .collect()
}

/// A read-only launch must not surface a mutation through the filter. Absence from
/// `tools/list` is the policy; the filter is a second door and has to be closed too.
#[test]
fn a_read_only_launch_never_returns_a_mutating_tool() {
    let mut client = connect(&[]);
    let names = advertised(&mut client);
    assert!(!names.contains(&"au_write_file".to_owned()), "{names:?}");

    for query in ["write", "file", "mutation", "batch", "rename", "workspace"] {
        let found = search(
            &mut client,
            json!({"query": query, "capability": "mutate", "limit": 20}),
        );
        assert!(found.is_empty(), "{query}: {found:?}");
    }
}

#[test]
fn the_mutation_filter_returns_exactly_the_advertised_mutations() {
    let mut client = connect(&[
        "--allow-mutations",
        "--allow-composition",
        "--allow-workspace-shape",
    ]);
    let advertised = advertised(&mut client);
    let found = search(
        &mut client,
        json!({"query": "au", "capability": "mutate", "limit": 20}),
    );
    assert_eq!(found.len(), 16, "{found:?}");
    for name in &found {
        assert!(advertised.contains(name), "{name} is not advertised");
    }
    assert!(
        found.iter().all(|name| !name.contains("instances")),
        "{found:?}"
    );
}

/// The four classes must partition the catalogue: the fully enumerable ones add up,
/// and the remainder is read. `limit` is capped at 20, so the read class is checked by
/// arithmetic on the catalogue rather than by paging it.
#[test]
fn the_classes_partition_the_advertised_catalogue() {
    let mut client = connect(&[
        "--allow-mutations",
        "--allow-composition",
        "--allow-workspace-shape",
    ]);
    let advertised = advertised(&mut client);
    assert_eq!(advertised.len(), 52, "{advertised:?}");

    let mut classified: Vec<String> = Vec::new();
    for (capability, expected) in [("mutate", 16), ("discover", 2), ("watch", 1)] {
        let found = search(
            &mut client,
            json!({"query": "au", "capability": capability, "limit": 20}),
        );
        assert_eq!(found.len(), expected, "{capability}: {found:?}");
        classified.extend(found);
    }
    classified.sort();
    classified.dedup();
    assert_eq!(classified.len(), 19, "{classified:?}");
    for name in &classified {
        assert!(advertised.contains(name), "{name} is not advertised");
    }
    let reads: Vec<String> = advertised
        .iter()
        .filter(|name| !classified.contains(name))
        .cloned()
        .collect();
    assert_eq!(reads.len(), 33, "{reads:?}");

    let page = search(
        &mut client,
        json!({"query": "au", "capability": "read", "limit": 20}),
    );
    assert_eq!(page.len(), 20, "{page:?}");
    for name in &page {
        assert!(reads.contains(name), "{name} is not a read");
    }
}

/// An unknown class is a clean argument error that names the accepted values, not a
/// silent empty page.
#[test]
fn an_unknown_capability_is_named_in_the_error() {
    let mut client = connect(&[]);
    let result = client.call_result(
        2,
        "au_search_tools",
        json!({"query": "instances", "capability": "reading"}),
    );
    let text = result["error"]["message"]
        .as_str()
        .or_else(|| result["content"][0]["text"].as_str())
        .unwrap_or_default();
    for wanted in ["read", "mutate", "discover", "watch"] {
        assert!(text.contains(wanted), "{wanted} missing from: {text}");
    }
    assert!(
        text.contains("reading"),
        "the rejected value is not named: {text}"
    );
}
