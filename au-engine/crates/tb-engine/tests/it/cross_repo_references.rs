//! Cross-boundary `[[name::repo]]` resolution: a qualified link resolves into
//! the named repo when present, and otherwise diagnoses why — the target is
//! missing in a present peer, the peer is declared but unmounted, or the repo
//! is unknown.

#![cfg(unix)]

use std::fs;
use std::path::Path;

use serde_json::json;
use tb_engine::{Client, ConfigSource, Engine, ServeHandle, build, serve};
use tb_parser::RealFileSystem;

fn write(root: &Path, rel: &str, content: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

/// `base` (present) holds `recovery.md`. `app` declares `base` and `finance`
/// as peers; `finance` is not present in the tree. `app/doc.md` links into
/// each, plus an unknown repo.
fn fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap();
    crate::seed_workspace(&root, &["base", "app"]);
    write(&root, "base/.arsumbris/repo.yaml", "name: base\n");
    write(
        &root,
        "app/.arsumbris/repo.yaml",
        "name: app\ndeps:\n  - name: base\n  - name: finance\n",
    );
    write(&root, "base/recovery.md", "Recovery notes.\n");
    write(
        &root,
        "app/doc.md",
        "Links:\n- [[recovery::base]] resolves\n- [[gone::base]] target missing\n\
         - [[recovery::finance]] declared but absent\n- [[recovery::ghost]] unknown repo\n",
    );
    dir
}

#[test]
fn cross_repo_references_resolve_or_diagnose() {
    let dir = fixture();
    let root = fs::canonicalize(dir.path()).unwrap();
    let kb = build(&root, &RealFileSystem).expect("build");

    let doc = root.join("app/doc.md");
    let codes: Vec<(String, String)> = kb
        .diagnostics()
        .filter(|d| d.span.file == doc)
        .map(|d| (d.code.as_str().to_string(), format!("{:?}", d.severity)))
        .collect();

    // `[[recovery::base]]` resolves (base present, recovery exists): no diag.
    // `[[gone::base]]`: present peer, target absent → target-missing (warning,
    // a dangling typed ref is open-world growth).
    assert!(
        codes
            .iter()
            .any(|(c, s)| c == "reference-target-missing" && s == "Warning"),
        "gone::base is target-missing, got {codes:?}"
    );
    // `[[recovery::finance]]`: declared peer, not present → unavailable (warn).
    assert!(
        codes
            .iter()
            .any(|(c, s)| c == "reference-repo-unavailable" && s == "Warning"),
        "recovery::finance is repo-unavailable, got {codes:?}"
    );
    // `[[recovery::ghost]]`: undeclared repo → unknown (error).
    assert!(
        codes
            .iter()
            .any(|(c, s)| c == "reference-repo-unknown" && s == "Error"),
        "recovery::ghost is repo-unknown, got {codes:?}"
    );
    // Exactly three diagnostics on this file: the resolved link emits none.
    assert_eq!(
        codes.len(),
        3,
        "the resolved link emits no diagnostic, got {codes:?}"
    );
}

#[test]
fn a_pinned_cross_repo_reference_is_not_existence_checked() {
    // A named cross-repo PIN `[[gone::base@sha]]` is inert: it resolves against
    // its commit's tree, not `base`'s live state. So the unpinned `[[gone::base]]`
    // fires `reference-target-missing` (base present, target absent) while the
    // pinned form fires NOTHING — the live counterpart being gone is expected.
    let dir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap();
    crate::seed_workspace(&root, &["base", "app"]);
    write(&root, "base/.arsumbris/repo.yaml", "name: base\n");
    write(&root, "base/recovery.md", "Recovery notes.\n");
    write(
        &root,
        "app/.arsumbris/repo.yaml",
        "name: app\ndeps:\n  - name: base\n",
    );
    write(
        &root,
        "app/doc.md",
        "Pinned to a gone target: [[gone::base@a1b2c3d]]\n",
    );

    let kb = build(&root, &RealFileSystem).expect("build");
    let doc = root.join("app/doc.md");
    let codes: Vec<String> = kb
        .diagnostics()
        .filter(|d| d.span.file == doc)
        .map(|d| d.code.as_str().to_string())
        .collect();
    assert!(
        codes.is_empty(),
        "a pinned cross-repo reference is inert, no existence check, got {codes:?}"
    );
}

#[test]
fn a_qualified_escaping_path_is_impossible_not_merely_missing() {
    // The third resolution surface. An impossible address must not sit in the
    // bucket that means "not written yet" — that conflation is the whole reason
    // `reference-path-escapes-repo` exists, and the unqualified surfaces already
    // draw the distinction. A consumer matching on the code must not see it
    // appear and disappear with the spelling.
    let dir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap();
    crate::seed_workspace(&root, &["base", "app"]);
    write(&root, "base/.arsumbris/repo.yaml", "name: base\n");
    write(&root, "base/recovery.md", "Recovery notes.\n");
    write(
        &root,
        "app/.arsumbris/repo.yaml",
        "name: app\ndeps:\n  - name: base\n",
    );
    write(
        &root,
        "app/doc.md",
        "Escaping: [[../base/recovery.md::base]]\nMerely absent: [[nope::base]]\n",
    );

    let kb = build(&root, &RealFileSystem).expect("build");
    let doc = root.join("app/doc.md");
    let codes: Vec<(String, String)> = kb
        .diagnostics()
        .filter(|d| d.span.file == doc)
        .map(|d| (d.code.as_str().to_string(), format!("{:?}", d.severity)))
        .collect();

    assert!(
        codes
            .iter()
            .any(|(c, s)| c == "reference-path-escapes-repo" && s == "Warning"),
        "the escaping qualified path is still bucketed as missing, got {codes:?}"
    );
    // Beside it, so the distinction is visible rather than a blanket reclassify.
    assert!(
        codes
            .iter()
            .any(|(c, s)| c == "reference-target-missing" && s == "Warning"),
        "the merely-absent target lost its own code, got {codes:?}"
    );
    // The fix is what makes the diagnostic actionable, and it is the shared one.
    let fix = kb
        .diagnostics()
        .find(|d| d.code.as_str() == "reference-path-escapes-repo")
        .and_then(|d| d.fix.clone())
        .expect("the escaping path carries no fix");
    assert_eq!(fix.description, tb_references::ESCAPES_REPO_FIX);
}

#[test]
fn a_resolving_repo_link_forms_a_cross_repo_backlink_edge() {
    let dir = fixture();
    let root = fs::canonicalize(dir.path()).unwrap();
    let kb = build(&root, &RealFileSystem).expect("build");

    // `app/doc.md`'s `[[recovery::base]]` is an inbound edge on base/recovery.md.
    let recovery = root.join("base/recovery.md");
    let sources: Vec<_> = kb
        .backlinks(&recovery)
        .iter()
        .map(|b| b.source.clone())
        .collect();
    assert!(
        sources.contains(&root.join("app/doc.md")),
        "the ::repo link forms a cross-repo backlink edge, got {sources:?}"
    );
}

struct Harness {
    _sock_dir: tempfile::TempDir,
    _engine: Engine,
    _server: ServeHandle,
    client: Client,
}

fn started(root: &Path) -> Harness {
    let sock_dir = tempfile::tempdir().unwrap();
    let socket = sock_dir.path().join("s");
    let engine = Engine::new(root, ConfigSource::Empty);
    let server = serve(engine.handle(), &socket).expect("serve");
    engine.rebuild();
    let client = Client::connect(&socket).expect("connect");
    Harness {
        _sock_dir: sock_dir,
        _engine: engine,
        _server: server,
        client,
    }
}

#[test]
fn backlinks_carries_the_source_repo_for_a_cross_repo_inbound_edge() {
    let dir = fixture();
    let root = fs::canonicalize(dir.path()).unwrap();
    let mut h = started(&root);

    // `app/doc.md`'s `[[recovery::base]]` is an inbound edge on `base/recovery.md`
    // from a different repo, so the backlink carries the source repo `app`.
    let resp = h
        .client
        .query(&json!({ "read": "references_in", "path": "base/recovery.md" }))
        .unwrap();
    let edges = resp["result"]["references_in"].as_array().unwrap();
    let from_app = edges
        .iter()
        .find(|e| e["source"].as_str().unwrap().ends_with("app/doc.md"))
        .expect("the cross-repo inbound edge from app is present");
    assert_eq!(
        from_app["repo"], "app",
        "a cross-repo inbound edge carries its source repo, not just an absolute path"
    );
}

#[test]
fn references_out_resolves_a_repo_qualified_link() {
    let dir = fixture();
    let root = fs::canonicalize(dir.path()).unwrap();
    let mut h = started(&root);

    let resp = h
        .client
        .query(&json!({ "read": "references_out", "path": "app/doc.md" }))
        .unwrap();
    let refs = resp["result"]["references_out"].as_array().unwrap();

    let qualified = refs
        .iter()
        .find(|r| r["target"] == "recovery" && r["repo"] == "base")
        .expect("the recovery::base link is reported with its repo qualifier");
    assert!(
        qualified["resolved"]
            .as_str()
            .unwrap()
            .ends_with("base/recovery.md"),
        "recovery::base resolves into base, got {:?}",
        qualified["resolved"]
    );

    // The unavailable one carries its repo but does not resolve.
    let unavailable = refs
        .iter()
        .find(|r| r["target"] == "recovery" && r["repo"] == "finance")
        .expect("the finance link is reported");
    assert!(unavailable["resolved"].is_null(), "finance is not present");
}

#[test]
fn resolve_target_honors_repo_qualifier_and_origin_scope() {
    let dir = fixture();
    let root = fs::canonicalize(dir.path()).unwrap();
    let mut h = started(&root);

    // A `::repo`-qualified target resolves against the named repo with no
    // origin — the host's cross-repo go-to-def case, which used to return null.
    let resp = h
        .client
        .query(&json!({ "read": "resolve_target", "target": "recovery::base" }))
        .unwrap();
    assert!(
        resp["result"]["resolve_target"]["path"]
            .as_str()
            .unwrap()
            .ends_with("base/recovery.md"),
        "recovery::base resolves into base, got {:?}",
        resp["result"]["resolve_target"]
    );

    // An unqualified target is scoped to its origin's repo. From a file in
    // `app`, bare `recovery` (which lives in `base`) does not resolve repo-local.
    let resp = h
        .client
        .query(&json!({
            "read": "resolve_target",
            "target": "recovery",
            "origin": "app/doc.md",
        }))
        .unwrap();
    assert!(
        resp["result"]["resolve_target"].is_null(),
        "bare recovery is not repo-local to app, got {:?}",
        resp["result"]["resolve_target"]
    );

    // From an origin in `base`, the same bare target resolves repo-local — the
    // member-local navigation case.
    let resp = h
        .client
        .query(&json!({
            "read": "resolve_target",
            "target": "recovery",
            "origin": "base/recovery.md",
        }))
        .unwrap();
    assert!(
        resp["result"]["resolve_target"]["path"]
            .as_str()
            .unwrap()
            .ends_with("base/recovery.md"),
        "bare recovery resolves repo-local from a base origin, got {:?}",
        resp["result"]["resolve_target"]
    );

    // An origin-scoped `::repo` target into a declared-but-unmounted peer is null.
    let resp = h
        .client
        .query(&json!({
            "read": "resolve_target",
            "target": "recovery::finance",
            "origin": "app/doc.md",
        }))
        .unwrap();
    assert!(
        resp["result"]["resolve_target"].is_null(),
        "finance is declared but unmounted, got {:?}",
        resp["result"]["resolve_target"]
    );
}

#[test]
fn semantic_tokens_style_a_repo_link_by_cross_repo_resolution() {
    let dir = fixture();
    let root = fs::canonicalize(dir.path()).unwrap();
    let mut h = started(&root);

    let resp = h
        .client
        .query(&json!({ "read": "semantic_tokens", "path": "app/doc.md" }))
        .unwrap();
    let tokens = resp["result"]["semantic_tokens"].as_array().unwrap();

    // The resolvable `recovery::base` link is wikilink-resolved, into base, with
    // its repo qualifier — not broken (the repo-local-only bug).
    let resolved = tokens
        .iter()
        .find(|t| t["kind"] == "wikilink-resolved" && t["repo"] == "base")
        .expect("recovery::base is a resolved cross-repo token");
    assert_eq!(resolved["target"], "recovery");
    assert!(
        resolved["resolved"]
            .as_str()
            .unwrap()
            .ends_with("base/recovery.md")
    );

    // The unknown-repo link is broken, carrying its repo.
    assert!(
        tokens
            .iter()
            .any(|t| t["kind"] == "wikilink-broken" && t["repo"] == "ghost"),
        "missing::ghost is a broken cross-repo token, got {tokens:?}"
    );
}

#[test]
fn a_cross_repo_type_claim_token_carries_its_repo() {
    // A `type: note::base` claim's semantic token must carry the `::repo`, so a
    // consumer can tell a peer type from an own one (finding 2.6).
    let dir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap();
    crate::seed_workspace(&root, &["base", "app"]);
    write(&root, "base/.arsumbris/repo.yaml", "name: base\n");
    write(
        &root,
        "base/type/note.type.yaml",
        "fields:\n  title: String\n",
    );
    write(
        &root,
        "app/.arsumbris/repo.yaml",
        "name: app\ndeps:\n  - name: base\n",
    );
    write(&root, "app/n.md", "---\ntype: note::base\ntitle: hi\n---\n");
    let mut h = started(&root);
    let resp = h
        .client
        .query(&json!({ "read": "semantic_tokens", "path": "app/n.md" }))
        .unwrap();
    let tokens = resp["result"]["semantic_tokens"].as_array().unwrap();
    assert!(
        tokens
            .iter()
            .any(|t| t["kind"] == "type-claim" && t["name"] == "note::base"),
        "the instance claim token must carry ::repo, got {tokens:?}"
    );
}

/// The `type-claim` tokens of one file, via the daemon.
fn claim_tokens(h: &mut Harness, path: &str) -> Vec<serde_json::Value> {
    let resp = h
        .client
        .query(&json!({ "read": "semantic_tokens", "path": path }))
        .unwrap();
    resp["result"]["semantic_tokens"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|t| t["kind"] == "type-claim")
        .cloned()
        .collect()
}

#[test]
fn a_type_claim_token_carries_the_identity_it_resolves_to() {
    // `app` and `base` each own a DIVERGED `note`, so the two identities share a
    // name and differ by hash. Every claim position must carry the identity the
    // claim resolves to, so a consumer joins it onto an identity-keyed read.
    let dir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap();
    crate::seed_workspace(&root, &["base", "app"]);
    write(&root, "base/.arsumbris/repo.yaml", "name: base\n");
    write(
        &root,
        "base/type/note.type.yaml",
        "fields:\n  title: String\n",
    );
    write(
        &root,
        "app/.arsumbris/repo.yaml",
        "name: app\ndeps:\n  - name: base\n",
    );
    write(
        &root,
        "app/type/note.type.yaml",
        "fields:\n  body: String\n",
    );
    write(&root, "app/type/card.type.yaml", "extends: note::base\n");
    write(
        &root,
        "app/type/holder.type.yaml",
        "fields:\n  item?: note::base\n  items?: note::base[]\n",
    );
    // `base` imports nothing, so it resolves in its own graph (no fold).
    write(&root, "base/local.md", "---\ntype: note\ntitle: t\n---\n");
    write(&root, "app/own.md", "---\ntype: note\nbody: b\n---\n");
    write(
        &root,
        "app/peer.md",
        "---\ntype: note::base\ntitle: t\n---\n",
    );
    write(
        &root,
        "app/selfq.md",
        "---\ntype: note::app\nbody: b\n---\n",
    );
    write(&root, "app/ghost.md", "---\ntype: ghost\n---\n");
    write(
        &root,
        "app/holder.md",
        "---\ntype: holder\nitem:\n  type: note::base\n  title: inline\nitems:\n---\n\
         ```[:items]\ntype: note::base\ntitle: fenced\n```\n",
    );
    let mut h = started(&root);

    let resolved = |t: &serde_json::Value| -> (String, String, String) {
        let r = &t["resolved"];
        (
            r["name"].as_str().unwrap().to_string(),
            r["repo"].as_str().unwrap().to_string(),
            r["hash"].as_str().unwrap().to_string(),
        )
    };
    let single = |h: &mut Harness, path: &str| {
        let toks = claim_tokens(h, path);
        assert_eq!(toks.len(), 1, "{path}: {toks:?}");
        toks[0].clone()
    };

    // A bare claim resolves in its own repo, a qualified one in the peer.
    let (own_name, own_repo, own_hash) = resolved(&single(&mut h, "app/own.md"));
    let (peer_name, peer_repo, peer_hash) = resolved(&single(&mut h, "app/peer.md"));
    assert_eq!((own_name.as_str(), own_repo.as_str()), ("note", "app"));
    assert_eq!((peer_name.as_str(), peer_repo.as_str()), ("note", "base"));
    assert_ne!(
        own_hash, peer_hash,
        "diverged same-named types are two identities"
    );

    // The owner's own bare claim, resolved in its own graph with no fold, is the
    // identity the importer reaches through `note::base`.
    let local = resolved(&single(&mut h, "base/local.md"));
    assert_eq!(
        local,
        (peer_name.clone(), peer_repo.clone(), peer_hash.clone())
    );

    // A self-qualifier is the own type.
    let selfq = single(&mut h, "app/selfq.md");
    assert_eq!(selfq["name"], "note::app", "the authored form stays");
    assert_eq!(
        resolved(&selfq),
        (own_name.clone(), own_repo, own_hash.clone())
    );

    // An unresolved claim omits `resolved` rather than guessing.
    let ghost = single(&mut h, "app/ghost.md");
    assert!(ghost.get("resolved").is_none(), "{ghost:?}");

    // A type-def parent claim resolves like an instance claim.
    let parent = single(&mut h, "app/type/card.type.yaml");
    assert_eq!(parent["name"], "note::base");
    assert_eq!(resolved(&parent).2, peer_hash);

    // Inline-record and typed-fence claims resolve too.
    let nested: Vec<(String, String, String)> = claim_tokens(&mut h, "app/holder.md")
        .iter()
        .filter(|t| t["name"] == "note::base")
        .map(resolved)
        .collect();
    assert_eq!(
        nested.len(),
        2,
        "the inline record and the fence: {nested:?}"
    );
    for r in &nested {
        assert_eq!((r.1.as_str(), r.2.as_str()), ("base", peer_hash.as_str()));
    }

    // The token's `(name, hash)` joins onto the identity-keyed instance_counts
    // row, the join its doc prescribes.
    let resp = h
        .client
        .query(&json!({ "read": "instance_counts", "origins": ["file"] }))
        .unwrap();
    let rows = resp["result"]["instance_counts"]["by_type"]
        .as_array()
        .unwrap();
    let row = |hash: &str| {
        rows.iter()
            .find(|r| r["name"] == "note" && r["hash"] == hash)
            .unwrap_or_else(|| panic!("no note row for {hash}: {rows:?}"))
    };
    assert_eq!(row(&own_hash)["type_owners"], json!(["app"]));
    assert_eq!(row(&peer_hash)["type_owners"], json!(["base"]));
}

#[test]
fn an_in_sync_type_claim_token_names_the_repo_the_claim_names() {
    // `app` and `base` define a byte-identical `note`, ONE identity that folds to
    // one node. The owner must follow the claim (bare is `app`'s, `::base` is
    // `base`'s), never the shared node's processing-order-dependent origin.
    let dir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap();
    crate::seed_workspace(&root, &["base", "app"]);
    write(&root, "base/.arsumbris/repo.yaml", "name: base\n");
    write(
        &root,
        "base/type/note.type.yaml",
        "fields:\n  title: String\n",
    );
    write(
        &root,
        "app/.arsumbris/repo.yaml",
        "name: app\ndeps:\n  - name: base\n",
    );
    write(
        &root,
        "app/type/note.type.yaml",
        "fields:\n  title: String\n",
    );
    write(&root, "app/own.md", "---\ntype: note\ntitle: t\n---\n");
    write(
        &root,
        "app/peer.md",
        "---\ntype: note::base\ntitle: t\n---\n",
    );
    write(
        &root,
        "app/type/card.type.yaml",
        "extends:\n  - note\n  - note::base\n",
    );
    let mut h = started(&root);

    let identity =
        |t: &serde_json::Value| (t["resolved"]["repo"].clone(), t["resolved"]["hash"].clone());
    let own = claim_tokens(&mut h, "app/own.md");
    let peer = claim_tokens(&mut h, "app/peer.md");
    assert_eq!(own[0]["resolved"]["repo"], "app", "{own:?}");
    assert_eq!(peer[0]["resolved"]["repo"], "base", "{peer:?}");
    assert_eq!(
        own[0]["resolved"]["hash"], peer[0]["resolved"]["hash"],
        "an in-sync type is one identity"
    );

    // Both claims in ONE file, so they meet the same folded node.
    let parents = claim_tokens(&mut h, "app/type/card.type.yaml");
    let by_name = |n: &str| identity(parents.iter().find(|t| t["name"] == n).unwrap());
    assert_eq!(by_name("note").0, "app", "{parents:?}");
    assert_eq!(by_name("note::base").0, "base", "{parents:?}");
    assert_eq!(by_name("note").1, by_name("note::base").1);
}
