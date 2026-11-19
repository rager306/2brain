//! The `search` read: ranked lexical search over the catalogued note text.
//!
//! Each test below pins one reason a piece of the wiring exists: a term that
//! occurs in one note finds that note, `exact` answers a punctuation token as a
//! conjunction, a body rewrite changes the ranking on the next read (the
//! incremental index patch), and the last page reports `next_offset: null`
//! rather than a sentinel. The source-identity tests pin the URL lookup: a
//! pasted `source_url` finds its notes, an equivalent form finds the same
//! notes, an unknown URL falls through to the plain ranking. The capture test
//! at the end pins the served shape.

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
/// Three distinct terms need two of them by default — unless the one a document holds is rare, which is the
/// escape `006ac0b` added. Both halves are asserted here, because the escape is what makes the second half
/// non-obvious: `theta` occurs in two of the fixture's thirteen notes, so its idf clears `RARE_IDF` and a
/// one-term document is admitted; `zeta` sits in the same two notes but the query below pairs it with two
/// COMMON terms, and there the floor bites.
#[test]
fn three_distinct_terms_need_two_of_them_unless_the_one_held_is_rare() {
    let (_dir, root) = seeded();
    let mut h = harness(&root);

    // The escape: `only-theta.md` holds one term of three and is kept anyway.
    let rare = search(&mut h.client, json!({ "query": "zeta eta theta" }));
    assert_eq!(rare["total"], 2, "{rare}");
    assert_eq!(
        names(&rare),
        vec!["both.md".to_string(), "only-theta.md".to_string()],
        "{rare}"
    );
    assert_eq!(rare["matches"][0]["matched_terms"], 2);

    // The floor: two of three, with every term common enough that no escape applies.
    let floored = search(&mut h.client, json!({ "query": "paging record zeta" }));
    assert_eq!(floored["total"], 5, "{floored}");
    assert_eq!(
        names(&floored),
        vec![
            "page-a.md".to_string(),
            "page-b.md".to_string(),
            "page-c.md".to_string(),
            "page-d.md".to_string(),
            "page-e.md".to_string()
        ],
        "{floored}"
    );
    assert_eq!(floored["matches"][0]["matched_terms"], 2);
}

// (e) Every match carries a short excerpt of the note's BODY around the queried
// term, and the answer states the width it was cut to. The window is what lets a
// caller judge a hit without a second `content` read (`docs/research/context-pack.md`);
// it is body text only, so a note whose `description:` matched still gets its
// opening paragraph rather than YAML.
#[test]
fn a_match_carries_a_body_excerpt_around_the_queried_term() {
    let (_dir, root) = seeded();
    let mut h = harness(&root);
    let answered = search(&mut h.client, json!({ "query": "zarquon" }));
    assert_eq!(answered["excerpt_chars"], 240, "{answered}");
    let excerpt = answered["matches"][0]["excerpt"]
        .as_str()
        .expect("a match carries an excerpt");
    assert!(excerpt.contains("zarquon"), "{excerpt}");
    assert!(
        !excerpt.contains("title:"),
        "an excerpt is body text, never frontmatter: {excerpt}"
    );

    // A term that occurs only in the `description:` still finds the note, and the
    // excerpt is then the body's opening: the query terms are not in the body to
    // open on.
    write(
        &root,
        "described.md",
        "---\ntitle: described\ndescription: a quasar vocabulary note\n---\n\n\
         The opening paragraph says nothing about the query.\n",
    );
    h.engine.rebuild();
    let described = search(&mut h.client, json!({ "query": "quasar" }));
    assert_eq!(
        names(&described),
        vec!["described.md".to_string()],
        "{described}"
    );
    let excerpt = described["matches"][0]["excerpt"]
        .as_str()
        .expect("a match carries an excerpt");
    assert!(
        excerpt.starts_with("The opening paragraph"),
        "a description-only hit quotes the body's opening: {excerpt}"
    );

    // The excerpt follows the body, not a copy of it: it is cut from the parse the
    // catalogue holds, so an edit that reaches the catalogue reaches the excerpt on
    // the next read.
    write(
        &root,
        "described.md",
        "---\ntitle: described\ndescription: a quasar vocabulary note\n---\n\n\
         A rewritten body now opens the note.\n",
    );
    h.engine.rebuild();
    let rewritten = search(&mut h.client, json!({ "query": "quasar" }));
    let excerpt = rewritten["matches"][0]["excerpt"]
        .as_str()
        .expect("a match carries an excerpt");
    assert!(
        excerpt.starts_with("A rewritten body"),
        "the excerpt is cut from the held parse, so it follows the edit: {excerpt}"
    );
}

// The source-identity lookup: a query that is itself a URL is answered by the
// notes whose `source_url:` names it, ahead of anything the ranking can say
// about the words inside that URL. The corpus is its own, so these URL fields
// cannot enter the shared corpus's answers above.
fn seeded_with_source() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap();
    crate::seed_repo(&root);
    write(
        &root,
        "cited.md",
        "---\ntitle: cited\nsource_url: \"https://arxiv.org/abs/2602.20021\"\n---\n\n\
         an echo attack on peer messages mentions arxiv 2602 20021 in prose too\n",
    );
    // The same identity in a different form: the scheme, `www.`, the
    // abs/pdf choice, a query string and a fragment all normalize away.
    write(
        &root,
        "cited-echo.md",
        "---\ntitle: cited echo\nsource_url: \"http://www.arxiv.org/pdf/2602.20021?utm_source=x#conclusion\"\n---\n\n\
         a second extraction of the same paper\n",
    );
    // Prose that holds the URL's words without holding its identity: it can
    // only ever be ranked, never looked up.
    write(
        &root,
        "prose-only.md",
        "---\ntitle: prose only\n---\n\n\
         arxiv 2602 20021 as plain words plus example org not held, no source_url field\n",
    );
    (dir, root)
}

// (source-lookup) The lookup answers with the carrying notes, unranked and
// marked; the ranking follows, with the looked-up paths deduplicated — so
// `cited.md`, whose prose holds the URL's words, appears once, not twice.
#[test]
fn a_pasted_source_url_finds_the_notes_that_carry_it() {
    let (_dir, root) = seeded_with_source();
    let mut h = harness(&root);
    let search = search(
        &mut h.client,
        json!({ "query": "https://arxiv.org/abs/2602.20021" }),
    );
    // Catalogue path order: `cited-echo.md` sorts before `cited.md`.
    assert_eq!(
        names(&search),
        vec![
            "cited-echo.md".to_string(),
            "cited.md".to_string(),
            "prose-only.md".to_string(),
        ],
        "{search}"
    );
    // Three answers: two from the lookup, one from the ranking. Without the
    // dedupe the looked-up note would also be ranked and `total` would be 4.
    assert_eq!(search["total"], 3, "{search}");
    for looked_up in &search["matches"].as_array().unwrap()[..2] {
        assert_eq!(
            looked_up["matched_source"]["identity"], "arxiv:2602.20021",
            "{search}"
        );
    }
    // Each hit echoes the URL ITS OWN note authored.
    assert_eq!(
        search["matches"][0]["matched_source"]["source_url"],
        "http://www.arxiv.org/pdf/2602.20021?utm_source=x#conclusion",
        "{search}"
    );
    assert_eq!(
        search["matches"][1]["matched_source"]["source_url"], "https://arxiv.org/abs/2602.20021",
        "{search}"
    );
    // No ranking happened for a lookup hit: the existing fields sit at their
    // no-match values, and the proof the hit is exact is `matched_source`.
    let looked_up = &search["matches"][0];
    assert_eq!(looked_up["score"], 0.0);
    assert_eq!(looked_up["matched_terms"], 0);
    assert_eq!(looked_up["weak"], false);
    let ranked = &search["matches"][2];
    assert!(
        ranked["score"].as_f64().expect("a ranked score") > 0.0,
        "{search}"
    );
    assert_eq!(ranked["matched_terms"], 4, "{search}");
    assert!(ranked.get("matched_source").is_none(), "{search}");
}

// (source-lookup) Identity, not spelling: whatever the caller pasted, if it
// normalizes to the same source, it finds the same notes — and the answer
// echoes each note's OWN spelling of the URL.
#[test]
fn an_equivalent_url_form_finds_the_same_notes() {
    let (_dir, root) = seeded_with_source();
    let mut h = harness(&root);
    let search = search(
        &mut h.client,
        json!({ "query": "HTTP://WWW.arxiv.ORG/pdf/2602.20021?utm_source=x#conclusion" }),
    );
    assert_eq!(
        names(&search)[..2],
        ["cited-echo.md".to_string(), "cited.md".to_string()],
        "{search}"
    );
    for looked_up in &search["matches"].as_array().unwrap()[..2] {
        assert_eq!(looked_up["matched_source"]["identity"], "arxiv:2602.20021");
    }
    // Each note's authored form, not the query's, is what comes back.
    assert_eq!(
        search["matches"][0]["matched_source"]["source_url"],
        "http://www.arxiv.org/pdf/2602.20021?utm_source=x#conclusion",
        "{search}"
    );
    assert_eq!(
        search["matches"][1]["matched_source"]["source_url"], "https://arxiv.org/abs/2602.20021",
        "{search}"
    );
}

// (source-lookup) A URL the base holds no `source_url` for must not return
// worse results than the ranking it interrupted: the answer is exactly the
// plain ranking of the URL's own terms, with no lookup marker anywhere.
#[test]
fn an_unknown_url_falls_through_to_the_plain_ranking() {
    let (_dir, root) = seeded_with_source();
    let mut h = harness(&root);
    let url = search(
        &mut h.client,
        json!({ "query": "https://example.org/not-held" }),
    );
    let words = search(&mut h.client, json!({ "query": "example org not held" }));
    // Same terms, same ranking: the URL shape added nothing and took nothing.
    assert_eq!(url["total"], words["total"], "{url} vs {words}");
    assert_eq!(names(&url), names(&words), "{url} vs {words}");
    for m in url["matches"].as_array().unwrap() {
        assert!(m.get("matched_source").is_none(), "{m}");
    }
}

// (source-lookup) The looked-up hits are ranks like any ranks: paging crosses
// the lookup/ranked boundary on the same `offset` / `next_offset` rules.
#[test]
fn a_source_page_pages_like_any_page() {
    let (_dir, root) = seeded_with_source();
    let mut h = harness(&root);
    let first = search(
        &mut h.client,
        json!({ "query": "https://arxiv.org/abs/2602.20021", "limit": 1 }),
    );
    assert_eq!(names(&first), vec!["cited-echo.md".to_string()], "{first}");
    assert_eq!(first["next_offset"], 1, "{first}");
    let second = search(
        &mut h.client,
        json!({ "query": "https://arxiv.org/abs/2602.20021", "limit": 2, "offset": 1 }),
    );
    assert_eq!(
        names(&second),
        vec!["cited.md".to_string(), "prose-only.md".to_string()],
        "{second}"
    );
    assert_eq!(second["offset"], 1);
    assert_eq!(second["next_offset"], Value::Null, "{second}");
}

// The golden capture: the served shape and its values against a fixed corpus,
// checked on every run rather than only structurally. It is regenerable in
// process (`TB_BLESS_SEARCH_CAPTURE=1`), which is why this one capture is
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
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests-golden/engine/tb_search.json");
    if std::env::var_os("TB_BLESS_SEARCH_CAPTURE").is_some() {
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

/// A dependency mounted beside the workspace must not answer for it.
///
/// `scope` defaulted to `all`, so a term that occurs only in a `discover` member came back as if it were the
/// user's own knowledge: measured on the live vault, a question the vault could not answer was answered by a
/// dependency repository's README (`docs/research/task-set.md`). The default is now `own`; `all` is something a
/// caller asks for.
#[test]
fn a_default_search_answers_from_the_own_workspace_not_a_mounted_dependency() {
    let dir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap();
    crate::seed_repo(&root);
    fs::create_dir_all(root.join("dep/.2brain")).unwrap();
    fs::write(root.join("dep/.2brain/repo.yaml"), "name: dep\n").unwrap();
    crate::seed_readme(&root.join("dep"));
    fs::write(
        root.join(".2brain/workspace.yaml"),
        "edit:\n  - v\ndiscover:\n  - dep\ntype: tb.engine.workspace::tb-engine\n",
    )
    .unwrap();
    write(
        &root,
        "own.md",
        "---\ntitle: own\n---\n\nthe zarquon protocol\n",
    );
    write(
        &root,
        "dep/borrowed.md",
        "---\ntitle: borrowed\n---\n\nthe zarquon protocol, borrowed\n",
    );
    let mut h = harness(&root);

    let default = search(&mut h.client, json!({"query": "zarquon"}));
    assert_eq!(
        names(&default),
        vec!["own.md"],
        "the default scope answers from the workspace, not a mounted dependency: {default}"
    );

    let all = search(&mut h.client, json!({"query": "zarquon", "scope": "all"}));
    assert_eq!(
        paths(&all).len(),
        2,
        "`scope: all` is the escape hatch and sees both: {all}"
    );

    let explicit_own = search(&mut h.client, json!({"query": "zarquon", "scope": "own"}));
    assert_eq!(names(&explicit_own), vec!["own.md"], "{explicit_own}");
}
