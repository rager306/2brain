//! The `search` read: ranked lexical search over the catalogued note text.
//!
//! Each test below pins one reason a piece of the wiring exists: a term that
//! occurs in one note finds that note, `exact` answers a punctuation token as a
//! conjunction, a body rewrite changes the ranking on the next read (the
//! incremental index patch), and the last page reports `next_offset: null`
//! rather than a sentinel. The capture test at the end pins the served shape.

#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use tb_engine::{Client, ConfigSource, Engine, ServeHandle, serve};

struct Harness {
    _dir: tempfile::TempDir,
    _sock_dir: tempfile::TempDir,
    /// Kept so a test can drive the rebuild after writing a file, which is how
    /// the incremental fast path is stepped deterministically.
    engine: Engine,
    _server: ServeHandle,
    client: Client,
}

fn harness(root: &Path) -> Harness {
    let dir_keep = tempfile::tempdir().unwrap();
    let sock_dir = tempfile::tempdir().unwrap();
    let socket = sock_dir.path().join("s");
    let engine = Engine::new(root, ConfigSource::Empty);
    engine.rebuild();
    let server = serve(engine.handle(), &socket).expect("serve");
    let client = Client::connect(&socket).expect("connect");
    Harness {
        _dir: dir_keep,
        _sock_dir: sock_dir,
        engine,
        _server: server,
        client,
    }
}

fn write(root: &Path, name: &str, content: &str) {
    fs::write(root.join(name), content).unwrap();
}

/// The corpus every test below shares. The terms are disjoint, so one test's
/// notes cannot enter another test's answer:
/// - `zarquon` occurs once, in `alpha.md`: the rare-term read.
/// - `error` and `fix` occur together only in `codes.md`: the punctuation
///   conjunction.
/// - `gamma` ranks `beta.md` above `alpha.md` until `alpha.md` is rewritten: the
///   incremental patch.
/// - `paging` occurs in five identical notes: paging and the tie order.
/// - `zeta` / `eta` / `theta` split three ways: the three-term default.
fn seeded() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap();
    crate::seed_repo(&root);
    write(
        &root,
        "alpha.md",
        "---\ntitle: alpha\n---\n\nthe zarquon protocol. gamma appears once among these \
         filler words: one two three four five six seven eight nine ten eleven twelve \
         thirteen fourteen fifteen sixteen.\n",
    );
    write(&root, "beta.md", "---\ntitle: beta\n---\n\ngamma\n");
    write(
        &root,
        "codes.md",
        "---\ntitle: codes\n---\n\nthe error->fix atom\n",
    );
    write(
        &root,
        "partial-fix.md",
        "---\ntitle: partial fix\n---\n\nfix the wording\n",
    );
    write(
        &root,
        "partial-error.md",
        "---\ntitle: partial error\n---\n\nan error worth naming\n",
    );
    // Five identical bodies, so the five scores tie exactly and the answer's
    // order is the documented tie rule: ascending document id, which is the
    // catalogue's path order.
    for name in ["page-a", "page-b", "page-c", "page-d", "page-e"] {
        write(
            &root,
            &format!("{name}.md"),
            &format!("---\ntitle: {name}\n---\n\nthe paging record\n"),
        );
    }
    write(&root, "both.md", "---\ntitle: both\n---\n\nzeta eta\n");
    write(
        &root,
        "only-zeta.md",
        "---\ntitle: only zeta\n---\n\nzeta\n",
    );
    write(
        &root,
        "only-theta.md",
        "---\ntitle: only theta\n---\n\ntheta\n",
    );
    (dir, root)
}

/// Run one `search` query and return its payload.
fn search(client: &mut Client, args: Value) -> Value {
    let mut request = args;
    request["read"] = json!("search");
    let resp = client.query(&request).unwrap();
    assert_eq!(resp["ready"], true, "{resp}");
    resp["result"]["search"].clone()
}

/// The page's paths, in answer order.
fn paths(search: &Value) -> Vec<String> {
    search["matches"]
        .as_array()
        .expect("matches is an array")
        .iter()
        .map(|m| m["path"].as_str().expect("path is a string").to_string())
        .collect()
}

/// The file name of each ranked path, so an assertion names the note rather than
/// the temp directory it happens to live under.
fn names(search: &Value) -> Vec<String> {
    paths(search)
        .iter()
        .map(|path| {
            Path::new(path)
                .file_name()
                .expect("a path has a file name")
                .to_string_lossy()
                .into_owned()
        })
        .collect()
}

// (a) A term that occurs in exactly one note finds that note, with the totals a
// caller needs to trust the answer: one match, nothing to page, not exact.
#[test]
fn search_returns_the_note_holding_a_rare_term() {
    let (_dir, root) = seeded();
    let mut h = harness(&root);
    let search = search(&mut h.client, json!({ "query": "zarquon" }));
    assert_eq!(search["total"], 1, "{search}");
    assert_eq!(search["offset"], 0);
    assert_eq!(search["next_offset"], Value::Null);
    assert_eq!(search["exact"], false);
    assert_eq!(names(&search), vec!["alpha.md".to_string()], "{search}");
    let hit = &search["matches"][0];
    assert!(
        hit["score"].as_f64().expect("a score") > 0.0,
        "a match is scored positively: {hit}"
    );
    assert_eq!(hit["matched_terms"], 1);
}

// (b) `exact` answers a punctuation token as the conjunction of its terms, so
// `error->fix` reaches the one note that holds both and neither note that holds
// one. No punctuation is compared, and no position is needed.
#[test]
fn an_exact_search_for_a_punctuation_token_returns_only_the_document_holding_it() {
    let (_dir, root) = seeded();
    let mut h = harness(&root);
    let search = search(
        &mut h.client,
        json!({ "query": "error->fix", "exact": true }),
    );
    assert_eq!(search["exact"], true);
    assert_eq!(search["total"], 1, "{search}");
    assert_eq!(names(&search), vec!["codes.md".to_string()], "{search}");
    assert_eq!(search["matches"][0]["matched_terms"], 2, "{search}");
}

// (c) The incremental patch proof. Two notes hold `gamma`; the short one ranks
// first. Rewriting the long one's body so it repeats the term five times flips
// the order on the next read — which can only happen if the edit reached the
// held index, since no file is read on the query path.
#[test]
fn a_body_rewrite_changes_the_ranking_on_the_next_read() {
    let (_dir, root) = seeded();
    let mut h = harness(&root);

    let before = search(&mut h.client, json!({ "query": "gamma" }));
    assert_eq!(before["total"], 2, "{before}");
    assert_eq!(
        names(&before),
        vec!["beta.md".to_string(), "alpha.md".to_string()],
        "the short body ranks first: {before}"
    );

    // A plain content edit of a catalogued path, exactly what the watcher hands
    // `classify_dirty`: it takes the incremental fast path, which patches the
    // index at the changed document's own id.
    write(
        &root,
        "alpha.md",
        "---\ntitle: alpha\n---\n\ngamma gamma gamma gamma gamma\n",
    );
    h.engine.rebuild();

    let after = search(&mut h.client, json!({ "query": "gamma" }));
    assert_eq!(after["total"], 2, "{after}");
    assert_eq!(
        names(&after),
        vec!["alpha.md".to_string(), "beta.md".to_string()],
        "the rewritten body now ranks first: {after}"
    );
    assert!(
        after["matches"][0]["score"].as_f64().expect("a score")
            > before["matches"][1]["score"].as_f64().expect("a score"),
        "the rewritten note outranks its own old score"
    );
}

// (d) Paging. `total` counts every match in scope, and `next_offset` is `null` at
// the end rather than a sentinel past it: a caller that loops until it sees null
// terminates, and one that trusts a count does not ask for a page that is not
// there.
#[test]
fn the_last_page_reports_a_null_next_offset() {
    let (_dir, root) = seeded();
    let mut h = harness(&root);

    let first = search(&mut h.client, json!({ "query": "paging", "limit": 2 }));
    assert_eq!(first["total"], 5, "{first}");
    assert_eq!(first["offset"], 0);
    assert_eq!(first["next_offset"], 2, "{first}");
    assert_eq!(
        names(&first),
        vec!["page-a.md".to_string(), "page-b.md".to_string()],
        "identical scores page in path order: {first}"
    );

    let second = search(
        &mut h.client,
        json!({ "query": "paging", "limit": 2, "offset": 2 }),
    );
    assert_eq!(second["total"], 5);
    assert_eq!(second["next_offset"], 4, "{second}");

    let last = search(
        &mut h.client,
        json!({ "query": "paging", "limit": 2, "offset": 4 }),
    );
    assert_eq!(last["total"], 5);
    assert_eq!(last["next_offset"], Value::Null, "{last}");
    assert_eq!(names(&last), vec!["page-e.md".to_string()], "{last}");

    // Past the end is an empty page and still no next offset: the total is the
    // only thing that says the caller already saw everything.
    let past = search(
        &mut h.client,
        json!({ "query": "paging", "limit": 2, "offset": 6 }),
    );
    assert_eq!(past["total"], 5);
    assert_eq!(past["next_offset"], Value::Null, "{past}");
    assert!(paths(&past).is_empty(), "{past}");
}

// The default minimum should match. Three distinct terms require two of them, not
// one: a plain OR answers a third of the vault, which is why the default is not
// "any term".
#[test]
fn three_distinct_terms_need_two_of_them_by_default() {
    let (_dir, root) = seeded();
    let mut h = harness(&root);
    let search = search(&mut h.client, json!({ "query": "zeta eta theta" }));
    assert_eq!(search["total"], 1, "{search}");
    assert_eq!(names(&search), vec!["both.md".to_string()], "{search}");
    assert_eq!(search["matches"][0]["matched_terms"], 2);
}

// The golden capture: the served shape and its values against a fixed corpus,
// checked on every run rather than only structurally. It is regenerable in
// process (`AU_BLESS_SEARCH_CAPTURE=1`), which is why this one capture is
// fixture-derived: a `search` score is a property of corpus statistics, so a
// capture that can only be taken from the living vault cannot be compared
// automatically at all.
#[test]
fn the_search_capture_matches_the_engine() {
    let (_dir, root) = seeded();
    let mut h = harness(&root);
    let resp = h
        .client
        .query(&json!({ "read": "search", "query": "zarquon" }))
        .unwrap();
    let actual = normalize(&resp["result"], root.to_str().expect("a UTF-8 temp path"));

    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests-golden/engine/au_search.json");
    if std::env::var_os("AU_BLESS_SEARCH_CAPTURE").is_some() {
        fs::write(&path, serde_json::to_string_pretty(&actual).unwrap() + "\n").unwrap();
        return;
    }
    let text =
        fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    let golden: Value = serde_json::from_str(&text).expect("the capture parses");
    assert_same(&actual, &golden, "search");
}

/// The probe's normalization, in Rust: the workspace root becomes `$VAULT` and
/// the rebuild `version` is dropped, so a capture diffs only when the engine
/// changed. Keys are compared as values, not as text.
fn normalize(value: &Value, root: &str) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .filter(|(key, _)| key.as_str() != "version")
                .map(|(key, value)| (key.clone(), normalize(value, root)))
                .collect(),
        ),
        Value::Array(items) => {
            Value::Array(items.iter().map(|item| normalize(item, root)).collect())
        }
        Value::String(text) => Value::String(text.replace(root, "$VAULT")),
        other => other.clone(),
    }
}

/// Compare a capture with the engine's answer, allowing a relative tolerance on
/// the scores: the same corpus yields the same score on every run, but a libm
/// difference across platforms must not fail a contract test. Everything else
/// must match exactly.
fn assert_same(actual: &Value, golden: &Value, at: &str) {
    match (actual, golden) {
        (Value::Number(a), Value::Number(g)) if a.is_f64() || g.is_f64() => {
            let (a, g) = (a.as_f64().expect("a float"), g.as_f64().expect("a float"));
            assert!(
                (a - g).abs() <= 1e-9 * g.abs().max(1.0),
                "{at}: score {a} drifted from {g}"
            );
        }
        (Value::Object(a), Value::Object(g)) => {
            assert_eq!(a.len(), g.len(), "{at}: key sets differ");
            for (key, value) in a {
                let other = g
                    .get(key)
                    .unwrap_or_else(|| panic!("{at}: `{key}` absent from the capture"));
                assert_same(value, other, &format!("{at}.{key}"));
            }
        }
        (Value::Array(a), Value::Array(g)) => {
            assert_eq!(a.len(), g.len(), "{at}: lengths differ");
            for (index, (value, other)) in a.iter().zip(g).enumerate() {
                assert_same(value, other, &format!("{at}[{index}]"));
            }
        }
        _ => assert_eq!(actual, golden, "{at}"),
    }
}
