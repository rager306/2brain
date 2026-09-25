//! The daemon-less member resolution, `au_engine::members::resolve_members`: each
//! declared member's tier, path, and outcome code, answered from the build's own
//! composition, and agreeing with what a full build mounts.

use std::fs;
use std::path::{Path, PathBuf};

use au_diagnostics::Diagnostic;
use au_engine::members::{resolve_members, MemberResolution, MemberResolutionEntry};
use au_engine::{
    build_reusing, BuildError, DiagSource, ParseLayer, RegistryLocation, RepoName, UserRegistry,
};
use au_parser::RealFileSystem;

fn repo(root: &Path, yaml: &str) {
    fs::create_dir_all(root.join(".arsumbris")).unwrap();
    fs::write(root.join(".arsumbris/repo.yaml"), yaml).unwrap();
}

/// A workspace touching every tier:
/// - `ws`, the entry, declaring deps `cached` (locked, in the cache) and `peer`
///   (absent), and locking a transitive `ghost` the cache does not hold.
/// - `sib`, a co-present sibling beside the entry.
/// - `reg`, located only through the registry.
/// - `gone`, declared and absent.
/// - `off`, declared and disabled.
struct Fixture {
    _dir: tempfile::TempDir,
    top: PathBuf,
    entry: PathBuf,
    cache: PathBuf,
    registry: UserRegistry,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let top = fs::canonicalize(dir.path()).unwrap();
    let entry = top.join("ws");
    repo(
        &entry,
        "name: ws\ndeps:\n  - name: cached\n  - name: peer\n",
    );
    fs::write(
        entry.join(".arsumbris/workspace.yaml"),
        "edit:\n  - ws\ndiscover:\n  - sib\n  - reg\n  - gone\n  - off\ndisabled:\n  - off\n",
    )
    .unwrap();
    fs::write(
        entry.join(".arsumbris/repo.lock"),
        "packages:\n  - name: cached\n    remote: git@example.com:o/cached.git\n    sha: aaaa1111\n  - name: ghost\n    remote: git@example.com:o/ghost.git\n    sha: bbbb2222\n",
    )
    .unwrap();
    repo(&top.join("sib"), "name: sib\n");
    let reg_root = top.join("elsewhere").join("reg-checkout");
    repo(&reg_root, "name: reg\n");
    let cache = top.join("cache");
    repo(&cache.join("aaaa1111"), "name: cached\n");
    let mut registry = UserRegistry::new();
    registry.insert(
        RepoName("reg".into()),
        RegistryLocation {
            remote: None,
            path: reg_root,
        },
    );
    Fixture {
        _dir: dir,
        top,
        entry,
        cache,
        registry,
    }
}

fn resolve(f: &Fixture) -> MemberResolution {
    resolve_members(&f.entry, &RealFileSystem, &f.registry, Some(&f.cache)).unwrap()
}

fn member<'a>(r: &'a MemberResolution, name: &str) -> &'a MemberResolutionEntry {
    r.members
        .iter()
        .find(|m| m.name == name)
        .unwrap_or_else(|| panic!("no member '{name}' in {:?}", r.members))
}

#[test]
fn each_member_reports_the_tier_that_located_it() {
    let f = fixture();
    let r = resolve(&f);
    let tier = |n: &str| member(&r, n).tier;
    assert_eq!(tier("ws"), "entry");
    assert_eq!(tier("sib"), "sibling");
    assert_eq!(tier("reg"), "registry");
    assert_eq!(tier("cached"), "cache");
    assert_eq!(tier("gone"), "unmounted");
    assert_eq!(tier("peer"), "unmounted");
    assert_eq!(tier("off"), "disabled");
    assert_eq!(
        member(&r, "sib").path.as_deref(),
        Some(f.top.join("sib").as_path())
    );
    assert_eq!(member(&r, "gone").path, None);
}

#[test]
fn roles_and_outcome_codes_follow_the_build() {
    let f = fixture();
    let r = resolve(&f);
    let m = |n: &str| (member(&r, n).role, member(&r, n).code.clone());
    assert_eq!(m("ws"), ("entry", None));
    assert_eq!(m("sib"), ("discover", None));
    assert_eq!(m("cached"), ("dep", None));
    assert_eq!(
        m("gone"),
        ("discover", Some("discover-member-unmounted".to_string()))
    );
    assert_eq!(m("peer"), ("dep", Some("peer-unmounted".to_string())));
    assert_eq!(m("off"), ("discover", None));
}

#[test]
fn a_locked_dependency_missing_from_the_cache_marks_the_answer_incomplete() {
    let f = fixture();
    let r = resolve(&f);
    assert!(!r.complete);
    assert_eq!(r.incomplete.len(), 1, "{:?}", r.incomplete);
    assert_eq!(r.incomplete[0].code.as_str(), "dependency-cache-miss");
    assert!(r.incomplete[0].message.contains("ghost"));
    // The unfetched transitive dependency is not a member.
    assert!(r.members.iter().all(|m| m.name != "ghost"));
}

#[test]
fn a_fully_located_workspace_is_complete() {
    let f = fixture();
    fs::write(
        f.entry.join(".arsumbris/repo.lock"),
        "packages:\n  - name: cached\n    remote: git@example.com:o/cached.git\n    sha: aaaa1111\n",
    )
    .unwrap();
    let r = resolve(&f);
    assert!(r.complete, "{:?}", r.incomplete);
    assert!(r.incomplete.is_empty());
}

/// The point of the read: it cannot disagree with a daemon's build. Every member
/// it reports located is a repo the build mounts at the same root, every member it
/// reports unmounted is absent from the build, its `diagnostics` equal the build's
/// knowledge-base-level ones (the README obligations aside, which it does not
/// run), and every member-outcome diagnostic the build emits names a member row
/// carrying that code.
#[test]
fn agrees_with_what_a_full_build_mounts() {
    let f = fixture();
    let r = resolve(&f);
    let kb = build_reusing(
        &f.entry,
        &RealFileSystem,
        &f.registry,
        &ParseLayer::default(),
        &std::collections::BTreeMap::new(),
        Some(&f.cache),
    )
    .unwrap();

    // The daemon's `members` read reports the same tier for every member it lists.
    let view = au_engine::wire::introspect_members(&kb, &f.entry, Some(&f.cache));
    for row in &view.members {
        let resolved = r.members.iter().find(|m| m.name == row.repo);
        assert_eq!(
            row.tier,
            resolved.map(|m| m.tier),
            "tier of {} on the members read",
            row.repo
        );
    }

    for m in &r.members {
        let mounted = kb.repos.by_name(&m.name);
        match m.tier {
            "unmounted" | "disabled" => assert!(mounted.is_none(), "{} mounted", m.name),
            _ => assert_eq!(
                mounted.map(|repo| repo.root.as_path()),
                m.path.as_deref(),
                "{} root",
                m.name
            ),
        }
    }

    // Both directions, as a multiset over the whole diagnostic.
    const README_CODES: [&str; 3] = [
        "repo-missing-readme",
        "readme-type-undeclared",
        "readme-misplaced",
    ];
    let key = |d: &Diagnostic| format!("{d:?}");
    let mut built: Vec<String> = kb
        .diagnostics_by_source
        .get(&DiagSource::KnowledgeBase)
        .into_iter()
        .flatten()
        .filter(|d| !README_CODES.contains(&d.code.as_str()))
        .map(key)
        .collect();
    let mut read: Vec<String> = r.diagnostics.iter().map(key).collect();
    built.sort();
    read.sort();
    assert!(!read.is_empty(), "the fixture raises member diagnostics");
    assert_eq!(read, built);

    // Every member-outcome diagnostic has its member row, carrying its code.
    const OUTCOME_CODES: [&str; 6] = [
        "edit-member-unmounted",
        "discover-member-unmounted",
        "peer-unmounted",
        "edit-member-read-only",
        "workspace-member-unwalkable",
        "member-at-reserved-root",
    ];
    let mut outcomes = 0;
    for d in kb.diagnostics() {
        if !OUTCOME_CODES.contains(&d.code.as_str()) {
            continue;
        }
        outcomes += 1;
        assert!(
            r.members
                .iter()
                .any(|m| m.code.as_deref() == Some(d.code.as_str())
                    && d.message.contains(&format!("'{}'", m.name))),
            "no member row for {}: {}",
            d.code.as_str(),
            d.message
        );
    }
    assert!(
        outcomes > 0,
        "the fixture raises member-outcome diagnostics"
    );
}

#[test]
fn a_non_repo_entry_is_refused_like_the_build() {
    let dir = tempfile::tempdir().unwrap();
    let err = resolve_members(dir.path(), &RealFileSystem, &UserRegistry::new(), None).unwrap_err();
    assert!(matches!(err, BuildError::EntryNotARepo { .. }), "{err:?}");
}
