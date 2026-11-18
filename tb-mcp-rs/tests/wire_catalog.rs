//! Keep three catalogs from drifting apart: the verbs `WIRE.md` documents, the
//! adapter's read tools with the engine verb each one asks for, and the fixture
//! corpus. The tool→verb half is read from `src/wire.rs`'s `READ_VERBS`, the one place
//! the adapter states which verb a tool reads; the handler text supplies only the
//! mutations, which stay request literals.

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

/// Tools that send no engine request of their own.
#[rustfmt::skip]
const ADAPTER_ONLY: [(&str, &str); 3] = [
    ("tb_search_tools",   "reads the adapter's own tool inventory"),
    ("tb_describe_tool",  "reads the adapter's own tool inventory"),
    ("tb_watch",          "waits on the `changes` subscription, not a read"),
];

/// Reads `WIRE.md` documents that no adapter tool exposes: pinned decisions, not
/// oversights. A new engine read fails the test below until this list names it,
/// and an entry that starts being exposed fails it too.
#[rustfmt::skip]
const NOT_EXPOSED: [(&str, &str); 17] = [
    ("candidate_counts",  "count companion; `tb_candidates` pages instead"),
    ("commit_meta",       "git trailers; the git surface is `tb_rollback_preview`"),
    ("config",            "one consumer config file outside the knowledge base"),
    ("device_config",     "device-global files under `$HOME`: host state"),
    ("diagnostic_counts", "count companion; `tb_diagnostics` pages instead"),
    ("file_history",      "git history of one path: host `git`, no consumer"),
    ("graph_shape",       "scalar summary; `tb_overview` mirrors it for the entry"),
    ("ignores",           "`.auignore` rules: host configuration, like its writer"),
    ("instance_counts",   "count companion; `tb_instances` counts every site"),
    ("lifecycle",         "daemon state, host-only; the frame's `ready` probes it"),
    ("link_graph",        "unbounded visualizer payload; walks are `tb_neighborhood`"),
    ("pins",              "`@commit` pin inventory; no adapter consumer"),
    ("recent_commits",    "workspace git log, like `file_history`"),
    ("type_batch",        "batch detail; `tb_types` plus `tb_type` browse and drill"),
    ("type_closure",      "closure walk; the `type` def carries `effective_fields`"),
    ("type_counts",       "count companion; `tb_types` pages instead"),
    ("type_graph",        "unbounded schema graph; `tb_type_tree` serves the tree"),
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

/// Every quoted string in `text`, in order.
fn strings(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut at = 0;
    while let Some(offset) = text[at..].find('"') {
        let open = at + offset;
        let Some(value) = quoted(text, open) else {
            break;
        };
        at = open + value.len() + 2;
        found.push(value);
    }
    found
}

/// The tool→verb table in `src/wire.rs`: the one place a read tool's engine verb is
/// stated. Each row is `("<tool>", &["<verb>", …])`, however the formatter wraps it; a
/// composed tool lists every verb it builds its answer from.
fn verb_table() -> BTreeMap<String, BTreeSet<String>> {
    let source = read(&manifest_dir().join("src/wire.rs"));
    let start = source
        .find("pub(crate) const READ_VERBS")
        .expect("the verb table");
    let block = &source[start..];
    let block = &block[block.find("= &[").expect("the table's body")..];
    let block = &block[..block.find("\n];").expect("the table's end")];
    let mut table = BTreeMap::new();
    // A row is the only thing in the table that opens a parenthesis, so splitting on one
    // leaves each row's strings: its tool, then its verbs.
    for row in block.split('(').skip(1) {
        let mut names = strings(row).into_iter();
        let tool = names.next().expect("a row names its tool");
        table.insert(tool, names.collect());
    }
    table
}

/// Every registered tool: one `#[tool(...)]` attribute per handler, with the verbs the
/// table says it reads and the mutation literals it builds.
fn tools() -> Vec<Tool> {
    let source = read(&manifest_dir().join("src/lib.rs"));
    let table = verb_table();
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
                reads: table.get(&name).cloned().unwrap_or_default(),
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

/// Mutations `WIRE.md` documents that no adapter tool sends: pinned decisions, not
/// oversights. The read side has carried such a list since this file was written; the
/// mutation side did not, so a documented mutation could appear or vanish without a
/// failing test. A new engine mutation fails the test below until this list names it, and
/// an entry that becomes exposed fails it too.
#[rustfmt::skip]
const NOT_EXPOSED_MUTATIONS: [(&str, &str); 8] = [
    ("append_record",        "inline-record frontmatter stamp; `tb_promote`/`tb_inline` move a record, neither edits its fields"),
    ("delete_dir",           "directory-level; `tb_delete_file` removes one file and `tb_batch` composes files"),
    ("edit_record",          "inline-record frontmatter stamp, the sibling of `append_record`"),
    ("move_dir",             "directory-level; `tb_rename` moves one file and rewrites its inbound references"),
    ("register",             "`workspaces.yaml` is user-authored; members are declared through the workspace-shape tools"),
    ("set_config",           "one consumer config file outside the knowledge base: host state, like the `config` read"),
    ("set_ignores",          "`.auignore` rules: host configuration, like its writer"),
    ("set_workspace_member_disabled", "the `disabled:` overlay; `tb_set_workspace_member_role` moves a member between lists and nothing disables one"),
];

#[test]
fn every_documented_mutation_has_an_adapter_tool_or_a_reason() {
    let catalog = catalog();
    let exposed: BTreeSet<String> = tools().into_iter().flat_map(|tool| tool.mutates).collect();
    let not_exposed: BTreeMap<&str, &str> = NOT_EXPOSED_MUTATIONS.into_iter().collect();
    for verb in &catalog.mutations {
        if !exposed.contains(verb) && !not_exposed.contains_key(verb.as_str()) {
            panic!("WIRE.md documents the mutation {verb:?} and no adapter tool sends it");
        }
    }
    for verb in not_exposed.keys() {
        assert!(
            catalog.mutations.contains(*verb),
            "NOT_EXPOSED_MUTATIONS lists {verb:?}, undocumented"
        );
        assert!(
            !exposed.contains(*verb),
            "NOT_EXPOSED_MUTATIONS lists {verb:?}, now exposed: drop it"
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
