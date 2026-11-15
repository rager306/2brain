//! Keep three catalogs from drifting apart: the verbs `WIRE.md` documents, the
//! adapter's read tools with the engine verb each one sends, and the fixture
//! corpus. The adapter half is derived from `src/lib.rs`, not hand-written: each
//! `#[tool(...)]` handler names its engine read in a `json!` literal, forwards a
//! verb to `file_read` / `target_read`, or names it in a helper. Two tables cover
//! the indirections a text scan cannot see, and both are kept honest below.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use serde_json::Value;

/// One registered tool and the engine requests its handler builds.
struct Tool {
    name: String,
    reads: BTreeSet<String>,
    mutates: BTreeSet<String>,
}

/// Handlers whose engine read verb sits in a helper, not in the handler.
const HELPER_VERBS: [(&str, &str); 2] = [
    ("self.content_with_timeout", "content"),
    ("read_content(", "content"),
];

/// A handler that picks its read verb at runtime, so no request literal names it.
const DYNAMIC_VERBS: [(&str, &[&str]); 1] = [(
    "au_follow",
    &["resolve_target", "resolve_anchor", "resolve_block_id"],
)];

/// Tools that send no engine request of their own.
#[rustfmt::skip]
const ADAPTER_ONLY: [(&str, &str); 3] = [
    ("au_search_tools",   "reads the adapter's own tool inventory"),
    ("au_describe_tool",  "reads the adapter's own tool inventory"),
    ("au_watch",          "waits on the `changes` subscription, not a read"),
];

/// Reads `WIRE.md` documents that no adapter tool exposes: pinned decisions, not
/// oversights. A new engine read fails the test below until this list names it,
/// and an entry that starts being exposed fails it too.
#[rustfmt::skip]
const NOT_EXPOSED: [(&str, &str); 17] = [
    ("candidate_counts",  "count companion; `au_candidates` pages instead"),
    ("commit_meta",       "git trailers; the git surface is `au_rollback_preview`"),
    ("config",            "one consumer config file outside the knowledge base"),
    ("device_config",     "device-global files under `$HOME`: host state"),
    ("diagnostic_counts", "count companion; `au_diagnostics` pages instead"),
    ("file_history",      "git history of one path: host `git`, no consumer"),
    ("graph_shape",       "scalar summary; `au_overview` mirrors it for the entry"),
    ("ignores",           "`.auignore` rules: host configuration, like its writer"),
    ("instance_counts",   "count companion; `au_instances` counts every site"),
    ("lifecycle",         "daemon state, host-only; the frame's `ready` probes it"),
    ("link_graph",        "unbounded visualizer payload; walks are `au_neighborhood`"),
    ("pins",              "`@commit` pin inventory; no adapter consumer"),
    ("recent_commits",    "workspace git log, like `file_history`"),
    ("type_batch",        "batch detail; `au_types` plus `au_type` browse and drill"),
    ("type_closure",      "closure walk; the `type` def carries `effective_fields`"),
    ("type_counts",       "count companion; `au_types` pages instead"),
    ("type_graph",        "unbounded schema graph; `au_type_tree` serves the tree"),
];

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

/// The first non-whitespace index at or after `at`.
fn skip_space(text: &str, mut at: usize) -> usize {
    while matches!(text.as_bytes().get(at), Some(b' ' | b'\t' | b'\n' | b'\r')) {
        at += 1;
    }
    at
}

/// The quoted string starting at `at`, when `at` is an opening quote.
fn quoted(text: &str, at: usize) -> Option<String> {
    let rest = text.get(at..)?.strip_prefix('"')?;
    Some(rest.split('"').next()?.to_owned())
}

/// Every `"<key>": "<verb>"` literal in a handler. A request whose verb is a
/// variable (`json!({"read": verb, ...})`) is skipped on purpose.
fn request_verbs(block: &str, key: &str) -> BTreeSet<String> {
    let needle = format!("\"{key}\"");
    let mut verbs = BTreeSet::new();
    let mut at = 0;
    while let Some(found) = block[at..].find(&needle) {
        at += found + needle.len();
        let colon = skip_space(block, at);
        if block[colon..].starts_with(':')
            && let Some(verb) = quoted(block, skip_space(block, colon + 1))
        {
            verbs.insert(verb);
        }
    }
    verbs
}

/// The engine reads one handler sends, in all the shapes `src/lib.rs` uses.
fn read_verbs(name: &str, block: &str) -> BTreeSet<String> {
    let mut verbs = request_verbs(block, "read");
    // `self.file_read(args, "<verb>", ...)`: the first literal after the call.
    for marker in ["self.file_read(", "self.target_read("] {
        let args = block
            .find(marker)
            .map(|start| skip_space(block, start + marker.len()));
        if let Some(verb) = args
            .and_then(|args| block[args..].find('"').map(|offset| args + offset))
            .and_then(|open| quoted(block, open))
        {
            verbs.insert(verb);
        }
    }
    for (marker, verb) in HELPER_VERBS {
        if block.contains(marker) {
            verbs.insert(verb.to_owned());
        }
    }
    for (tool, extra) in DYNAMIC_VERBS {
        if tool == name {
            verbs.extend(extra.iter().map(|verb| (*verb).to_owned()));
        }
    }
    verbs
}

/// Every registered tool: one `#[tool(...)]` attribute per handler.
fn tools() -> Vec<Tool> {
    let source = read(&manifest_dir().join("src/lib.rs"));
    let starts: Vec<usize> = source.match_indices("#[tool(").map(|(at, _)| at).collect();
    let last = source.find("#[tool_handler(").unwrap_or(source.len());
    starts
        .iter()
        .enumerate()
        .map(|(index, &start)| {
            let stop = starts.get(index + 1).copied().unwrap_or(last).min(last);
            let block = &source[start..stop];
            let name = block
                .split_once("async fn ")
                .and_then(|(_, rest)| {
                    rest.split(|c: char| !(c.is_alphanumeric() || c == '_'))
                        .next()
                })
                .unwrap_or_default()
                .to_owned();
            Tool {
                reads: read_verbs(&name, block),
                mutates: request_verbs(block, "mutate"),
                name,
            }
        })
        .collect()
}

/// The verbs `WIRE.md` documents, split by catalog.
struct Catalog {
    reads: BTreeSet<String>,
    mutations: BTreeSet<String>,
    resolve: bool,
}

/// `WIRE.md` documents a verb as a `**verb.**` line under `## The read catalog`
/// or `## The mutation catalog`, and `resolve` as a heading only. The bold form
/// is regular: a lowercase verb, underscore included.
fn catalog() -> Catalog {
    let text = read(&manifest_dir().join("../tb-engine/crates/tb-engine/WIRE.md"));
    let mut catalog = Catalog {
        reads: BTreeSet::new(),
        mutations: BTreeSet::new(),
        resolve: text.contains("## The resolve verb"),
    };
    let mut section = "";
    for line in text.lines() {
        let line = line.trim();
        if let Some(heading) = line.strip_prefix("## ") {
            section = heading;
            continue;
        }
        let verb = line
            .strip_prefix("**")
            .and_then(|rest| rest.strip_suffix(".**"));
        let verb = verb.filter(|verb| verb.chars().all(|c| c.is_ascii_lowercase() || c == '_'));
        match (section, verb) {
            ("The read catalog", Some(verb)) => catalog.reads.insert(verb.to_owned()),
            ("The mutation catalog", Some(verb)) => catalog.mutations.insert(verb.to_owned()),
            _ => false,
        };
    }
    catalog
}

/// One captured fixture: its file name, the tool it called, its payload keys.
fn fixtures() -> Vec<(String, String, Vec<String>)> {
    let dir = manifest_dir().join("tests/fixtures/synthetic-reads");
    let entries =
        std::fs::read_dir(&dir).unwrap_or_else(|error| panic!("{}: {error}", dir.display()));
    let mut fixtures = Vec::new();
    for path in entries.flatten().map(|entry| entry.path()) {
        if path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        let stored: Value = serde_json::from_str(&read(&path)).unwrap();
        let keys = stored["payload"]
            .as_object()
            .map(|payload| payload.keys().cloned().collect())
            .unwrap_or_default();
        let file = path.file_name().unwrap().to_string_lossy().into_owned();
        let tool = stored["tool"].as_str().unwrap_or_default().to_owned();
        fixtures.push((file, tool, keys));
    }
    fixtures
}

#[test]
fn every_adapter_verb_is_documented() {
    let catalog = catalog();
    for tool in tools() {
        for verb in &tool.reads {
            let msg = format!(
                "{} sends the read {verb:?}, undocumented in WIRE.md",
                tool.name
            );
            assert!(
                catalog.reads.contains(verb) || (verb == "resolve" && catalog.resolve),
                "{msg}"
            );
        }
        for verb in &tool.mutates {
            let msg = format!(
                "{} sends the mutation {verb:?}, undocumented in WIRE.md",
                tool.name
            );
            assert!(catalog.mutations.contains(verb), "{msg}");
        }
    }
}

#[test]
fn every_documented_read_has_an_adapter_tool() {
    let catalog = catalog();
    let exposed: BTreeSet<String> = tools().into_iter().flat_map(|tool| tool.reads).collect();
    let not_exposed: BTreeMap<&str, &str> = NOT_EXPOSED.into_iter().collect();
    for verb in &catalog.reads {
        if !exposed.contains(verb) && !not_exposed.contains_key(verb.as_str()) {
            panic!("WIRE.md documents the read {verb:?} and no adapter tool sends it");
        }
    }
    for verb in not_exposed.keys() {
        assert!(
            catalog.reads.contains(*verb),
            "NOT_EXPOSED lists {verb:?}, undocumented"
        );
        assert!(
            !exposed.contains(*verb),
            "NOT_EXPOSED lists {verb:?}, now exposed: drop it"
        );
    }
}

#[test]
fn every_fixture_names_an_exposed_read() {
    let catalog = catalog();
    let mapping: BTreeMap<String, BTreeSet<String>> = tools()
        .into_iter()
        .map(|tool| (tool.name, tool.reads))
        .collect();
    for (file, tool, keys) in fixtures() {
        let Some(verbs) = mapping.get(&tool) else {
            panic!("{file} records {tool}, which maps to no engine read");
        };
        // A payload key that names a documented read is that read's answer, so
        // the tool must send it; derived payloads (`follow`, `typed`, the
        // reconcile outcome) name no verb and are exempt.
        for key in keys.iter().filter(|key| catalog.reads.contains(*key)) {
            assert!(
                verbs.contains(key),
                "{file} records {tool} with a {key:?} payload it never sends"
            );
        }
    }
}

#[test]
fn every_registered_tool_is_classified() {
    let (tools, fixtures) = (tools(), fixtures());
    let (mut read, mut mutation) = (0, 0);
    let mut bare = BTreeSet::new();
    for tool in &tools {
        if !tool.mutates.is_empty() {
            mutation += 1;
        } else if tool.reads.is_empty() {
            bare.insert(tool.name.clone());
        } else {
            read += 1;
        }
    }
    let allowed: BTreeSet<String> = ADAPTER_ONLY
        .iter()
        .map(|(name, _)| (*name).to_owned())
        .collect();
    let msg = format!(
        "inventory: {} tools ({read} read, {mutation} mutation, {} adapter-only), {} fixtures",
        tools.len(),
        allowed.len(),
        fixtures.len()
    );
    assert_eq!(
        read + mutation + bare.len(),
        tools.len(),
        "{msg}; every tool must be classified exactly once"
    );
    assert_eq!(bare, allowed, "{msg}");
}
