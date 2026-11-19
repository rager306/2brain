#![cfg(unix)]

//! `docs/tools.md` is how a human finds a tool; the router is what a client is
//! offered. Drift between the two is silent: a tool with no row is a surface the
//! document cannot describe, and a row naming no tool sends a reader to a call
//! that fails. Both directions are compared here, so the next one fails a test
//! instead of a reader.

use std::{collections::BTreeSet, fs, path::Path, time::Duration};

use serde_json::json;

mod support;

use support::Client;

const TIMEOUT: Duration = Duration::from_secs(10);

/// Every tool the router advertises: all three grants, so the mutations are in
/// the catalogue too. A read-only launch shows only the subset without grants,
/// and a document missing a mutation row is exactly the drift this catches.
fn advertised() -> BTreeSet<String> {
    let mut client = Client::start_with_timeout(
        Path::new("/nonexistent.sock"),
        &[
            "--allow-mutations",
            "--allow-composition",
            "--allow-workspace-shape",
        ],
        TIMEOUT,
    );
    let reply = client.request(json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}));
    reply["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap().to_owned())
        .collect()
}

/// The tool names a document's rows carry, with the line each row starts on.
///
/// Only the first cell is read: the cells after it are prose about when to use
/// the tool, and they name other tools on purpose. A row may cover two tools
/// (`tb_add_dep` / `tb_remove_dep`), so the cell is scanned rather than matched
/// as one name.
fn rows(text: &str) -> Vec<(usize, BTreeSet<String>)> {
    text.lines()
        .enumerate()
        .filter_map(|(index, line)| {
            if !line.starts_with("| `tb_") {
                return None;
            }
            let first_cell = line.split('|').nth(1)?;
            let names: BTreeSet<String> = first_cell
                .split('`')
                .skip(1)
                .step_by(2)
                .filter(|span| span.starts_with("tb_"))
                .map(str::to_owned)
                .collect();
            Some((index + 1, names))
        })
        .collect()
}

/// The one figure the document states for the whole surface: `**tools** (52)`.
fn stated_total(text: &str) -> usize {
    let tail = text
        .split_once("**tools** (")
        .unwrap_or_else(|| panic!("docs/tools.md no longer states the tool count"))
        .1;
    let digits: String = tail.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().expect("the stated tool count is a number")
}

#[test]
fn every_tool_has_a_row_and_every_row_names_a_tool() {
    let doc = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/tools.md"))
        .expect("docs/tools.md ships with the crate");
    let advertised = advertised();
    let documented = rows(&doc);

    let mut named: BTreeSet<String> = BTreeSet::new();
    let mut repeated: Vec<String> = Vec::new();
    for (line, names) in &documented {
        for name in names {
            if !named.insert(name.clone()) {
                repeated.push(format!("{name} (line {line})"));
            }
        }
    }

    let missing: Vec<&String> = advertised.difference(&named).collect();
    assert!(
        missing.is_empty(),
        "the router advertises {missing:?} and docs/tools.md has no row for them"
    );
    let unknown: Vec<&String> = named.difference(&advertised).collect();
    assert!(
        unknown.is_empty(),
        "docs/tools.md has a row for {unknown:?}, which no launch advertises"
    );
    assert!(
        repeated.is_empty(),
        "these tools are named by more than one row: {repeated:?}"
    );
    assert_eq!(
        stated_total(&doc),
        advertised.len(),
        "docs/tools.md states a total that the router does not advertise"
    );
}

/// The verification scripts grant and then grep for tool names in the agent's transcript. Every one
/// of them used `mcp__2brain__au_*` long after the adapter renamed its tools to `tb_*`, so the grant
/// covered nothing and each check looked for a call that could never appear — a script that always
/// fails, or a `grep -q` that quietly never matches. This is the regression guard: no script may
/// name a tool the router does not advertise.
#[test]
fn every_tool_a_script_names_is_one_the_router_advertises() {
    let scripts = Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts");
    let mut offenders: Vec<String> = Vec::new();
    let mut stack = vec![scripts];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().is_some_and(|name| name == "__pycache__") {
                    continue;
                }
                stack.push(path);
                continue;
            }
            let Ok(text) = fs::read_to_string(&path) else {
                continue;
            };
            for (index, line) in text.lines().enumerate() {
                let mut rest = line;
                while let Some(at) = rest.find("mcp__2brain__") {
                    let tail = &rest[at + "mcp__2brain__".len()..];
                    let name: String = tail
                        .chars()
                        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                        .collect();
                    if !name.is_empty() && !advertised().contains(&name) {
                        offenders.push(format!("{}:{} {name}", path.display(), index + 1));
                    }
                    rest = &tail[name.len()..];
                }
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "these scripts name an MCP tool the router does not advertise: {offenders:?}"
    );
}
