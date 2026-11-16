#![cfg(unix)]

//! Structural rules that are cheaper to assert than to rediscover: the leaf modules stay
//! leaves, the module dependency set is explicit, and `lib.rs` does not grow back into a
//! god module. These are fitness functions in the sense of `docs/architecture-plan.md`
//! P0.2 — they exist so a refactor has a floor, not to describe the ideal.
//!
//! `wire.rs` no longer reads `crate::SCHEMA_VERSION` (the transport port, P1.1, moved it
//! into `transport.rs`), so every module edge that exists is declared below. The test does
//! not detect a cycle itself; it fails when an edge nobody declared appears.

use std::{collections::BTreeSet, fs, path::Path, time::Duration};

use serde_json::json;

mod support;

use support::Client;

/// Modules that must never depend on a sibling: an infrastructure leaf that reaches back
/// into the crate is where cycles are born.
const LEAVES: [&str; 4] = ["bounded_stdio", "capability", "policy", "schemas"];

/// Every module dependency that is allowed today. Adding one is a decision, so it needs a
/// line here and a reason in the message.
const ALLOWED: [(&str, &str); 13] = [
    ("cli", "transport"),
    ("gate", "policy"),
    ("lib", "bounded_stdio"),
    ("lib", "capability"),
    ("lib", "describe"), // one-line descriptions: the cut every default surface carries
    ("lib", "gate"),
    ("lib", "ledger"),
    ("lib", "policy"),
    ("lib", "resources"),
    ("lib", "schemas"),
    ("lib", "transport"),
    ("lib", "wire"),
    ("wire", "transport"),
];

/// Production lines of `src/lib.rs`, excluding the inline test module. A ratchet: lower it
/// when a port moves out, never raise it to make room.
// 4424 after the gate moved out of lib.rs, plus 81 for one new tool (`tb_search`,
// 2026-11-15) — the ratchet guards a refactor from re-growing the file; a feature pays
// its own lines once and the number is recorded here with its reason.
// 4505 after `tb_search`, plus 25 on 2026-11-16: `guarded_mutation` now takes the tool name so the audit
// ledger records `tb_write_file` instead of the literal `mutation` — provenance the ledger existed for.
// 4501 on 2026-11-16: the read tools' engine verbs moved into `wire.rs`'s `READ_VERBS`, so a read
// call site names a tool instead of also repeating the verb, and `file_read` lost an unused key.
// 4501 after `init` and `doctor` moved into `src/cli.rs`: the module is public for the binary's
// dispatcher, which costs one line here and no logic — the budget did not move.
// 4501 after the verb table, plus 10 on 2026-11-16: `tb_graph_health` gained the `verbs` argument that reaches
// the engine's per-verb counters — a diagnostic no client could reach before the engine exposed it.
// 4511, plus 20 on 2026-11-16: `tb_create_file` sends the engine's `create_file` verb, and that verb's
// absence conflict is the create gate refusing a moment later, so `finish_mutation` maps it back to
// `policy/absence`. The mapping is one guarded arm; its words, and the predicate that reads `detail.reason`,
// live in `policy.rs` and `wire.rs` rather than here.
// 4531, plus 33 on 2026-11-16: `tb_rename_type` previews the cascade before mutating and refuses when any
// member it rewrites is outside the grant. The decision is a guarded match and the audit line that names the
// offending member; the engine read, the `affected` parsing and the refusal text live in `wire.rs` and
// `policy.rs`. The alternative — keeping the check out of lib.rs — would mean the mutation tool no longer
// decides what it may write, which is the defect this closes.
// 4564 before the one-line description cut moved out to `describe.rs`; the port took 9 lines with it and
// the ceiling follows DOWN, which is the only direction the ratchet is allowed to move on a refactor.
const LIB_RS_PRODUCTION_BUDGET: usize = 4556;

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

/// Module edges, read from the source rather than from the compiler.
///
/// Inside a module a dependency must be rooted at `crate::`, `self::`, or `super::`,
/// because a bare segment can belong to a foreign path: `rmcp::transport::` reads
/// exactly like a sibling module named `transport`. At the crate root a bare `x::`
/// is how the declared modules are named, so it counts there. The three rooted
/// spellings and the root's bare one all name the same edge.
fn edges(known: &[String]) -> BTreeSet<(String, String)> {
    let mut found = BTreeSet::new();
    for from in known {
        let text = fs::read_to_string(src().join(format!("{from}.rs"))).unwrap();
        for to in known {
            if to == from {
                continue;
            }
            let rooted = ["crate", "self", "super"]
                .iter()
                .any(|root| text.contains(&format!("{root}::{to}::")));
            let local = from == "lib" && text.contains(&format!("{to}::"));
            if rooted || local {
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
    // Output schemas are a full-profile property: the concise default withholds
    // them by design (one line per tool; `tb_describe_tool` serves the rest).
    // This test pins the catalogue property under the opt-in heavy surface.
    let mut client = Client::start_with_timeout(
        Path::new("/nonexistent.sock"),
        &[
            "--tools",
            "full",
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
        52,
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
