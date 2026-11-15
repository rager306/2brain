#![cfg(unix)]

//! Structural rules that are cheaper to assert than to rediscover: the leaf modules stay
//! leaves, the module dependency set is explicit, and `lib.rs` does not grow back into a
//! god module. These are fitness functions in the sense of `docs/architecture-plan.md`
//! P0.2 — they exist so a refactor has a floor, not to describe the ideal.
//!
//! Not modelled here: `wire.rs` reads `crate::SCHEMA_VERSION`, a symbol dependency that
//! the transport port (P1.1) removes. It is not a module edge, so the rules below let it
//! through; P1.1's own acceptance covers it.

use std::{collections::BTreeSet, fs, path::Path, time::Duration};

use serde_json::json;

mod support;

use support::Client;

/// Modules that must never depend on a sibling: an infrastructure leaf that reaches back
/// into the crate is where cycles are born.
const LEAVES: [&str; 4] = ["bounded_stdio", "capability", "policy", "schemas"];

/// Every module dependency that is allowed today. Adding one is a decision, so it needs a
/// line here and a reason in the message.
const ALLOWED: [(&str, &str); 7] = [
    ("lib", "bounded_stdio"),
    ("lib", "capability"),
    ("lib", "ledger"),
    ("lib", "policy"),
    ("lib", "resources"),
    ("lib", "schemas"),
    ("lib", "wire"),
];

/// Production lines of `src/lib.rs`, excluding the inline test module. A ratchet: lower it
/// when a port moves out, never raise it to make room.
const LIB_RS_PRODUCTION_BUDGET: usize = 4612;

fn src() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

fn modules() -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(src())
        .unwrap()
        .filter_map(|entry| {
            let path = entry.unwrap().path();
            let stem = path.file_stem()?.to_str()?.to_owned();
            (path.extension()? == "rs" && stem != "main").then_some(stem)
        })
        .collect();
    names.sort();
    names
}

/// Module edges, read from the source rather than from the compiler. Any of `policy::`,
/// `crate::policy::`, `super::policy::` counts: all three are the same dependency.
fn edges(known: &[String]) -> BTreeSet<(String, String)> {
    let mut found = BTreeSet::new();
    for from in known {
        let text = fs::read_to_string(src().join(format!("{from}.rs"))).unwrap();
        for to in known {
            if to != from && text.contains(&format!("{to}::")) {
                found.insert((from.clone(), to.clone()));
            }
        }
    }
    found
}

#[test]
fn leaf_modules_do_not_depend_on_a_sibling() {
    let known = modules();
    let found = edges(&known);
    for (from, to) in &found {
        if LEAVES.contains(&from.as_str()) {
            panic!(
                "{from} depends on {to}: a leaf module reaching into the crate is how \
                 cycles start; put the shared value in the leaf instead"
            );
        }
    }
}

#[test]
fn the_module_dependency_set_is_explicit() {
    let known = modules();
    let found: BTreeSet<(String, String)> = edges(&known);
    let allowed: BTreeSet<(String, String)> = ALLOWED
        .iter()
        .map(|(from, to)| ((*from).to_owned(), (*to).to_owned()))
        .collect();
    let unexpected: Vec<_> = found.difference(&allowed).cloned().collect();
    assert!(
        unexpected.is_empty(),
        "new module dependencies {unexpected:?}; add them to ALLOWED with a reason"
    );
    let stale: Vec<_> = allowed.difference(&found).cloned().collect();
    assert!(
        stale.is_empty(),
        "ALLOWED names dependencies that no longer exist {stale:?}; remove them"
    );
}

#[test]
fn lib_rs_does_not_grow() {
    let text = fs::read_to_string(src().join("lib.rs")).unwrap();
    let production = text
        .lines()
        .position(|line| line == "mod tests {")
        .unwrap_or_else(|| text.lines().count());
    assert!(
        production <= LIB_RS_PRODUCTION_BUDGET,
        "src/lib.rs has {production} production lines, budget {LIB_RS_PRODUCTION_BUDGET}: \
         move a responsibility out rather than raising the budget"
    );
}

#[test]
fn every_advertised_tool_declares_an_output_schema() {
    let mut client = Client::start_with_timeout(
        Path::new("/nonexistent.sock"),
        &[
            "--allow-mutations",
            "--allow-composition",
            "--allow-workspace-shape",
        ],
        Duration::from_secs(10),
    );
    let reply = client.request(json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}));
    let tools = reply["result"]["tools"].as_array().unwrap();
    assert_eq!(
        tools.len(),
        51,
        "the catalogue changed size: {}",
        tools.len()
    );
    for tool in tools {
        let name = tool["name"].as_str().unwrap();
        assert!(
            tool.get("outputSchema").is_some(),
            "{name} advertises no output schema, so a validating client cannot check it"
        );
    }
}
