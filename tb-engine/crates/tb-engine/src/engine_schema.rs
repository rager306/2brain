//! The hardwired `tb.engine.*` schema type-defs.
//!
//! Every engine-schema file is a typed instance of one of these compiled-in
//! defs, owned by a reserved `tb-engine` repo identity, never authored in a
//! knowledge base: the in-repo `repo.yaml`, the per-user `repos.yaml` / `workspaces.yaml`,
//! the in-repo `.2brain/workspace.yaml`, and the `.2brain/` locks. The
//! type-def IS the schema, self-documenting and `(name, hash)`-comparable like
//! any cross-repo type. See
//! [[spec - cross-repo resolution - in-repo identity and deps over a per-user repo registry]]
//! "Engine files are typed" and
//! [[spec - engine-schema files - hardwired-schema files are first-class substrate nodes]].
//!
//! The defs are authored as ordinary `.type.yaml` source and parsed through the
//! real `parse_file` path, so their `Shape` ASTs, spans, and canonical hashes
//! come out exactly as a repo-authored def's would. That keeps them
//! wire-serializable (the shape-AST read) and drift-comparable against a
//! consumer's mirror, both without a rework.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::LazyLock;

use tb_core::{Instance, TypeClaim, TypeDef, TypeName, TypeNameClaim};
use tb_diagnostics::{ByteRange, Diagnostic, DiagnosticCode, Severity, Span};

use crate::parse::{FileParse, parse_file};
use crate::repo::{Repo, RepoName};

/// The reserved repo identity owning every hardwired engine-schema def.
///
/// A knowledge base's same-named def coexists as a DISTINCT `(name, hash)` identity, they
/// compose like any cross-repo pair, `::tb-engine` scopes the engine's. This is
/// the engine's slice of the layered-ownership pattern (host owns `au.host.*`,
/// harness owns `au.mcp.*`).
pub const BUILTIN_ENGINE_REPO: &str = "tb-engine";

/// The nominal meta marker type-name, `tb.engine.meta`. A type is legal in a
/// `meta:` position only if its resolved closure mixes this in (as the qualified
/// `tb.engine.meta::tb-engine`). Injected into tb-core's validator so it stays
/// domain-pure. See
/// [[spec - meta type marker - the meta position admits only types that mix in the engine meta base]].
pub const ENGINE_META_TYPE: &str = "tb.engine.meta";

/// A knowledge base type-def in the reserved `tb.engine.*` namespace whose name the
/// engine does NOT hardwire. The namespace is reserved forward: a future engine
/// version may hardwire this name, and the knowledge base def would then live-shadow it.
/// Advisory (`hint`), never blocks; the knowledge base def keeps its own identity and
/// resolves normally. See
/// [[spec - cross-repo resolution - in-repo identity and deps over a per-user repo registry]]
/// "Engine files are typed".
pub const ENGINE_NAME_FORWARD_RESERVED: DiagnosticCode =
    DiagnosticCode::from_static("engine-name-forward-reserved");

/// A knowledge base type-def named exactly like a hardwired `tb.engine.*` type. The two
/// COEXIST as distinct `(name, hash)` identities, `::tb-engine` scopes the
/// engine's; the code only NOTES the engine owns the name, it never ignores the
/// knowledge base's def. Advisory (`warning`), never blocks. See
/// [[spec - cross-repo resolution - in-repo identity and deps over a per-user repo registry]]
/// "Engine files are typed".
pub const ENGINE_NAME_LIVE_SHADOW: DiagnosticCode =
    DiagnosticCode::from_static("engine-name-live-shadow");

/// A knowledge base type-def that shadows a hardwired `tb.engine.*` type AND has DIVERGED
/// from it: its `(name, closure-hash)` identity differs from the engine's copy.
/// Advisory `drift`, the same tier as a drifted pinned reference, never blocks.
/// The base coexistence is `engine-name-live-shadow`; this escalates a diverged
/// shadow so a consumer can rank it above an in-sync one (which fires no drift).
pub const ENGINE_NAME_SHADOW_DRIFT: DiagnosticCode =
    DiagnosticCode::from_static("engine-name-shadow-drift");

/// An engine-schema file carries no written `type:` key. Its kind still assigns
/// the FLOOR type, so the file stays fully correct and the engine reads its data
/// regardless. Advisory `drift`, a high-attention nudge to self-describe: the
/// engine's own writers emit the qualified `type: tb.engine.X::tb-engine`, so an
/// absent one reads as a hand-authored or legacy file. Never blocks. See
/// [[spec - engine-schema file claims - the kind assigns a floor, a written type self-describes and mixes in more]].
pub const ENGINE_SCHEMA_TYPE_UNWRITTEN: DiagnosticCode =
    DiagnosticCode::from_static("engine-schema-type-unwritten");

/// An engine-schema file writes a `type:` whose resolved closure OMITS the
/// kind's floor type (e.g. a `repo.yaml` written `type: tb.engine.workspace::tb-engine`).
/// The written claim names the wrong kind, so the file does not self-describe as
/// what it structurally is. An `error`; the engine still reads its data from the
/// kind-assigned floor (the resolution path is claim-independent), so it is
/// recoverable, never a lost-data case. A written claim that INCLUDES the floor
/// and mixes in more is fine. See
/// [[spec - engine-schema file claims - the kind assigns a floor, a written type self-describes and mixes in more]].
pub const ENGINE_SCHEMA_TYPE_FLOOR_OMITTED: DiagnosticCode =
    DiagnosticCode::from_static("engine-schema-type-floor-omitted");

/// The engine-schema type namespace: the one `DEFS` names and every writer
/// emits. A def named `tb.engine` or under `tb.engine.` sits in the engine's
/// reserved namespace, see [`schema_type_matches`].
pub(crate) const ENGINE_NAMESPACE: &str = "tb.engine";

/// The part of a namespaced engine-schema name after its root: `""` for a bare
/// root, `".repo"` for `tb.engine.repo`. `None` for any other name. The dot
/// guard keeps an unrelated name like `tb.engineering` out of the namespace.
pub(crate) fn schema_leaf(name: &str) -> Option<&str> {
    let rest = name.strip_prefix(ENGINE_NAMESPACE)?;
    (rest.is_empty() || rest.starts_with('.')).then_some(rest)
}

/// Whether a type name sits in the engine-reserved namespace: the exact root
/// (`tb.engine`) or any dotted descendant (`tb.engine.<leaf>`).
pub(crate) fn schema_type_matches(name: &str) -> bool {
    schema_leaf(name).is_some()
}

/// The hardwired engine-schema name a written name denotes.
///
/// One spelling exists, so a name is already canonical and is returned
/// unchanged; the function stays so a caller reads "this is the identity now"
/// rather than assuming a rewrite happened.
pub(crate) fn canonical_schema_name(name: &str) -> &str {
    name
}

/// Whether a name is one of the hardwired engine-schema defs, in either
/// spelling.
pub(crate) fn is_hardwired_engine_name(name: &str) -> bool {
    DEFS.iter()
        .any(|(canonical, _)| *canonical == canonical_schema_name(name))
}

/// Every spelling of the builtin engine repo's identity, the canonical one
/// first. A `::repo` qualifier naming any of these names the builtin.
pub(crate) const BUILTIN_ENGINE_REPO_SPELLINGS: &[&str] = &[BUILTIN_ENGINE_REPO];

/// Whether a repo name is the builtin engine repo's identity, in either
/// spelling (`tb-engine` in release 1, `tb-engine` from release 2).
///
/// Exact-match, the way [`crate::repo::RepoMap::by_name`] compares repo names: a
/// repo name is a case-sensitive identity, so a case variant is a different
/// repo. A caller that must also fold case (the device config path guard, for a
/// case-insensitive filesystem) compares against [`BUILTIN_ENGINE_REPO_SPELLINGS`]
/// itself.
pub(crate) fn is_builtin_repo(name: &str) -> bool {
    BUILTIN_ENGINE_REPO_SPELLINGS.contains(&name)
}

/// Canonicalize a type name a READ argument carries: `"tb.engine.repo::tb-engine"`
/// becomes `"tb.engine.repo::tb-engine"`.
///
/// A read names a type the same way a file claims one, so a release-1 client keeps
/// working after the flip: without this, `instances_of tb.engine.repo::tb-engine`
/// answers nothing while the same instances are served under the new spelling. A
/// name outside the engine schema is returned unchanged, so a knowledge base's own
/// types are untouched.
pub(crate) fn canonical_query_name(raw: &str) -> String {
    let (name, repo) = match raw.split_once("::") {
        Some((name, repo)) => (name, Some(repo)),
        None => (raw, None),
    };
    let canonical = canonical_schema_name(name);
    match repo {
        Some(repo) if is_builtin_repo(repo) => format!("{canonical}::{BUILTIN_ENGINE_REPO}"),
        Some(repo) => format!("{canonical}::{repo}"),
        None => canonical.to_string(),
    }
}

/// Canonicalize ONE engine-schema claim a file yields: an instance `type:`, a
/// type-def `extends` / `sealed` / required-meta claim.
///
/// Rewrites only what can only mean the builtin's schema: a claim qualified by
/// the builtin repo (in either spelling) has that qualifier respelled to
/// [`BUILTIN_ENGINE_REPO`], and a hardwired name (in either spelling) is
/// respelled to its canonical form — unqualified, or under a non-builtin peer
/// qualifier. Everything else is untouched, so a knowledge base's own
/// `tb.engine.custom` or `tb.engine.custom` keeps its own identity.
///
/// A SPELLING normalisation, not a resolution: an unqualified claim still
/// resolves in the authoring repo, so a bare `tb.engine.repo` in a repo that
/// defines no such type stays `unknown-type-claim`.
///
/// Release 1 keeps the legacy spelling canonical, so this is the identity for
/// every claim written today and it makes the future spelling resolve. Release 2
/// flips [`BUILTIN_ENGINE_REPO`] and renames `DEFS`, and the same function then
/// maps the legacy spelling forward. See `docs/research/tier-c-seams.md`, "The
/// seam the inventory missed".
pub(crate) fn canonical_schema_claim(claim: TypeNameClaim) -> TypeNameClaim {
    let canonical = canonical_schema_name(&claim.name.0);
    if let Some(repo) = claim.repo.as_deref() {
        if !is_builtin_repo(repo) {
            return claim;
        }
        return TypeNameClaim {
            name: TypeName(canonical.to_string()),
            repo: Some(BUILTIN_ENGINE_REPO.to_string()),
            span: claim.span,
        };
    }
    if canonical == claim.name.0 {
        return claim;
    }
    TypeNameClaim {
        name: TypeName(canonical.to_string()),
        ..claim
    }
}

/// Canonicalize the claims a parsed type-def yields: its own path-derived name
/// and every claim it carries. Called by the tb-engine parse wrapper, never by
/// tb-core, which stays domain-pure and knows no engine-schema names.
pub(crate) fn canonical_schema_type_def(td: &mut TypeDef) {
    td.name = TypeName(canonical_schema_name(&td.name.0).to_string());
    canonicalize_claims(&mut td.parents);
    canonicalize_claims(&mut td.sealed);
    canonicalize_claims(&mut td.required_meta);
}

/// Canonicalize the `type:` claim of a parsed instance, bare or list.
///
/// One spelling exists, so this rewrites a builtin-repo qualifier to its
/// canonical identity and leaves every other claim alone; it returns no notes
/// because there is no legacy spelling left to point at.
pub(crate) fn canonical_schema_instance(inst: &mut Instance) -> Vec<Diagnostic> {
    match &mut inst.type_claim {
        TypeClaim::Bare(claim) => *claim = canonical_schema_claim(claim.clone()),
        TypeClaim::List { items, .. } => canonicalize_claims(items),
    }
    Vec::new()
}

fn canonicalize_claims(claims: &mut [TypeNameClaim]) {
    for claim in claims {
        *claim = canonical_schema_claim(claim.clone());
    }
}

/// The coexistence note for a REPO-authored type-def whose name sits in the
/// reserved `tb.engine.*` namespace, or `None` for an ordinary name.
///
/// The engine reserves the namespace by CONVENTION, not annihilation: the knowledge base
/// def keeps its own `(name, hash)` identity and resolves like any other, so the
/// note is advisory. A hardwired name is `engine-name-live-shadow` (both
/// coexist, `::tb-engine` scopes the engine's); any other reserved name is
/// `engine-name-forward-reserved` (the engine may claim it later). The builtin's
/// own defs never reach this: they are seeded straight into the `tb-engine`
/// repo, never walked as knowledge base files.
pub(crate) fn engine_name_reservation_diag(td: &TypeDef) -> Option<Diagnostic> {
    let name = td.name.0.as_str();
    if !schema_type_matches(name) {
        return None;
    }
    let span = Span::new(td.source_path.clone(), td.source_span);
    // A hardwired name is canonicalized by the parse wrapper, so `canonical` is
    // the spelling the engine itself uses; naming it keeps the note readable
    // under either spelling a hand-built `TypeDef` may carry.
    let canonical = canonical_schema_name(name);
    let diag = if is_hardwired_engine_name(name) {
        Diagnostic {
            code: ENGINE_NAME_LIVE_SHADOW,
            severity: Severity::Warning,
            span,
            message: format!(
                "type-def '{canonical}' shares a name with a hardwired engine type; both coexist \
                 as distinct identities, '{canonical}::{BUILTIN_ENGINE_REPO}' names the engine's"
            ),
            related: vec![],
            fix: None,
        }
    } else {
        Diagnostic {
            code: ENGINE_NAME_FORWARD_RESERVED,
            severity: Severity::Hint,
            span,
            message: format!(
                "type-def '{name}' is in the engine-reserved 'tb.engine.*' namespace, which the \
                 engine may hardwire in a future version"
            ),
            related: vec![],
            fix: None,
        }
    };
    Some(diag)
}

/// The drift note for a hardwired-name shadow whose closure-hash has DIVERGED
/// from the engine's copy. The caller compares the two `(name, closure-hash)`
/// identities post-graph-build and calls this only on a divergence; an in-sync
/// shadow (equal hashes) is silent, its coexistence already noted by
/// `engine-name-live-shadow`.
pub(crate) fn engine_name_shadow_drift_diag(
    name: &str,
    file: PathBuf,
    span: ByteRange,
) -> Diagnostic {
    Diagnostic {
        code: ENGINE_NAME_SHADOW_DRIFT,
        severity: Severity::Drift,
        span: Span::new(file, span),
        message: format!(
            "type-def '{name}' shadows a hardwired engine type and has DIVERGED from it; the \
             engine's copy is '{name}::{}'",
            BUILTIN_ENGINE_REPO
        ),
        related: vec![],
        fix: None,
    }
}

/// A sentinel root for the builtin repo, NOT an on-disk path.
///
/// No walked file sits under it, so `RepoMap::repo_of` never routes a real file
/// to the builtin repo, and `RepoMap::root()` skips it via the `builtin` flag.
/// The builtin's defs are seeded straight into its graph, never read from disk.
pub(crate) const BUILTIN_ROOT: &str = "<tb-engine-builtin>";

/// The hardwired defs as `(type-name, source)`. The source is the schema:
/// seven file types, the record element types their list fields reference, and
/// the `tb.engine.meta` marker base. Every field is a primitive, a `String[]`, or
/// a `T[]` of one of these element records, so the whole set is expressible with
/// no maps. The `tb.engine.readme` file type is body-only, a self-declared
/// markdown node with a section template and no fields. The marker base is a
/// fieldless abstract tag, see
/// [[spec - meta type marker - the meta position admits only types that mix in the engine meta base]].
const DEFS: &[(&str, &str)] = &[
    // --- File types ----------------------------------------------------------
    (
        "tb.engine.repo",
        "\
#: A repo's committed identity and dependency set, `.2brain/repo.yaml`.
fields:
  name: String
  remote?: String
  description?: String
  deps?: tb.engine.dep[]
",
    ),
    (
        "tb.engine.repos",
        "\
#: The per-user registry, `<state dir>/engine/config/repos.yaml` (see `repo::state_dir_in`), name to remote+path.
fields:
  repos?: tb.engine.registry-entry[]
",
    ),
    (
        "tb.engine.workspaces",
        "\
#: The per-user workspaces index, `<state dir>/engine/config/workspaces.yaml` (see `repo::state_dir_in`).
fields:
  workspaces?: tb.engine.workspace-entry[]
",
    ),
    (
        "tb.engine.workspace",
        "\
#: The optional workspace composition, `.2brain/workspace.yaml`, edit + discover members. `disabled:` overlays the role lists, a member declared but intentionally not mounted.
fields:
  edit?: String[]
  discover?: String[]
  disabled?: String[]
",
    ),
    (
        "tb.engine.repo-lock",
        "\
#: The per-repo dependency lock, `<identity dir>/repo.lock`, engine-written.
fields:
  packages?: tb.engine.locked-package[]
",
    ),
    (
        "tb.engine.workspace-lock",
        "\
#: The per-workspace discover lock, `<identity dir>/workspace.lock`, engine-written.
fields:
  packages?: tb.engine.locked-package[]
",
    ),
    (
        "tb.engine.readme",
        "\
#: A repo's self-description at its root, `README.md`. Self-declared, its
#: sections say what the repo is, how to use it, and how to extend it.
fields:
  #: A one-line digest of the whole README, about one sentence per section:
  #: what the repo is, how to use it, how to extend it. E.g. \"Engine over a
  #: typed file graph. Use it via the daemon. Build on top by defining your
  #: own types.\"
  tldr: String
body:
  - section: Repo Overview
    guidance: \"The engine-owned self-description block. Keep the repo's own top-level sections outside it.\"
    body:
      - section: What this is
        guidance: \"What this repo is and the role it plays. Name the domain and what it holds, not its history.\"
      - section: How to use this
        guidance: \"How a human or agent consumes this repo. The entry points, the main commands or reads, and where to start.\"
      - section: How to extend this
        guidance: \"How a downstream repo builds on this one's types. Which types it exposes for a consumer to import via `::repo`, subtype, or claim as an instance identity, and the conventions for doing so.\"
",
    ),
    // --- Element records -----------------------------------------------------
    (
        "tb.engine.dep",
        "\
#: One dependency entry in a repo's `deps`.
fields:
  name: String
  remote?: String
  ref?: String
",
    ),
    (
        "tb.engine.registry-entry",
        "\
#: One entry in the per-user registry: a name's remote and local path.
fields:
  name: String
  remote?: String
  path: String
",
    ),
    (
        "tb.engine.workspace-entry",
        "\
#: One entry in the workspaces index: a workspace file and an optional alias.
fields:
  path: String
  name?: String
",
    ),
    (
        "tb.engine.locked-package",
        "\
#: One pinned dependency in a repo lock: a name, its remote, and the sha.
fields:
  name: String
  remote: String
  sha: String
",
    ),
    // --- Marker bases --------------------------------------------------------
    (
        "tb.engine.meta",
        "\
#: The nominal meta marker. A type-def is legal in a `meta:` position only if
#: its resolved closure mixes this in, via `tb.engine.meta::tb-engine`. Abstract,
#: never claimed directly, a block names a concrete subtype that carries it.
abstract: true
fields: {}
",
    ),
];

/// The hardwired `tb.engine.*` type-defs, parsed once through the real parse
/// path. A parse error or a missing def is a programming error in a compiled-in
/// source string, so it panics loudly rather than silently dropping a def.
static BUILTIN_DEFS: LazyLock<Vec<TypeDef>> = LazyLock::new(|| {
    DEFS.iter()
        .map(|(name, src)| {
            let path = PathBuf::from(BUILTIN_ROOT).join(format!("{name}.type.yaml"));
            match parse_file(&path, src.as_bytes()) {
                FileParse::TypeDef {
                    type_def: Some(td),
                    diagnostics,
                    ..
                } => {
                    assert!(
                        diagnostics.iter().all(|d| d.severity != Severity::Error),
                        "hardwired engine schema '{name}' has parse errors: {diagnostics:?}"
                    );
                    assert_eq!(
                        td.name.0, *name,
                        "hardwired engine schema path-derived name mismatch"
                    );
                    td
                }
                other => panic!(
                    "hardwired engine schema '{name}' did not parse to a type-def: {other:?}"
                ),
            }
        })
        .collect()
});

/// The hardwired `tb.engine.*` type-defs, cloned from the memoized parse.
pub(crate) fn builtin_engine_defs() -> Vec<TypeDef> {
    BUILTIN_DEFS.clone()
}

/// The compiled-in `tb-engine` repo: a fixed identity with no on-disk root,
/// present in every built knowledge base so `::tb-engine` resolves as a universal peer
/// (the resolution gate finds it by name, no `deps` declaration needed).
pub(crate) fn builtin_engine_repo() -> Repo {
    Repo {
        root: PathBuf::from(BUILTIN_ROOT),
        name: RepoName(BUILTIN_ENGINE_REPO.to_string()),
        declared: true,
        builtin: true,
        description: None,
        remote: None,
        deps: Vec::new(),
        peer_paths: BTreeMap::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tb_core::{
        build_graph, run_body_typing_checks, run_graph_structure_checks, run_inheritance_checks,
    };

    /// Exactly the finalized set: seven file types, four element records, and the
    /// `tb.engine.meta` marker base.
    const EXPECTED: &[&str] = &[
        "tb.engine.repo",
        "tb.engine.repos",
        "tb.engine.workspaces",
        "tb.engine.workspace",
        "tb.engine.repo-lock",
        "tb.engine.workspace-lock",
        "tb.engine.readme",
        "tb.engine.dep",
        "tb.engine.registry-entry",
        "tb.engine.workspace-entry",
        "tb.engine.locked-package",
        "tb.engine.meta",
    ];

    /// A user namespace that merely starts with the legacy prefix is NOT a legacy claim: it is the
    /// knowledge base's own type, canonicalization leaves it alone, and the migration never touches
    /// it. Nagging about it is a false alarm (review finding #24).
    #[test]
    fn the_hardwired_set_is_exactly_the_finalized_leaf_names() {
        let defs = builtin_engine_defs();
        let mut names: Vec<&str> = defs.iter().map(|d| d.name.0.as_str()).collect();
        names.sort_unstable();
        let mut expected: Vec<&str> = EXPECTED.to_vec();
        expected.sort_unstable();
        assert_eq!(names, expected);
    }

    #[test]
    fn the_hardwired_defs_build_a_clean_graph_with_identities() {
        // The whole set feeds one graph, as it does under the `tb-engine` repo
        // at build time. Every element ref resolves within the set, so the load
        // checks must be silent — a hardwired schema is authoritative.
        let build = build_graph(builtin_engine_defs());
        let mut diags = build.diagnostics;
        diags.extend(run_graph_structure_checks(&build.graph));
        diags.extend(run_inheritance_checks(&build.graph));
        diags.extend(run_body_typing_checks(&build.graph));
        diags.extend(tb_core::run_location_checks(&build.graph, None, None));
        assert!(
            diags.iter().all(|d| d.severity != Severity::Error),
            "hardwired engine schema graph has errors: {diags:?}"
        );
        // Identity is `(name, closure-hash)`; every def gets a closure id, so the
        // set is drift-comparable against a consumer mirror (Action 6).
        for name in EXPECTED {
            assert!(
                build
                    .graph
                    .closure_id(&tb_core::TypeName(name.to_string()))
                    .is_some(),
                "no closure id computed for hardwired '{name}'"
            );
        }
    }

    #[test]
    fn the_builtin_repo_is_flagged_and_named() {
        let repo = builtin_engine_repo();
        assert!(repo.builtin);
        assert_eq!(repo.name.as_str(), BUILTIN_ENGINE_REPO);
        assert!(repo.deps.is_empty());
    }

    #[test]
    fn schema_namespace_membership_is_the_one_namespace() {
        for name in ["tb.engine", "tb.engine.repo", "tb.engine.custom-thing"] {
            assert!(schema_type_matches(name), "'{name}' sits in the namespace");
        }
        // The retired spelling is not the namespace any more.
        assert!(!schema_type_matches(concat!("au", ".engine", ".repo")));
        // The dot guard keeps a merely-prefixed name out.
        assert!(!schema_type_matches("tb.engineering"));
        assert!(!schema_type_matches("engine"));
        assert!(!schema_type_matches("note"));
    }

    #[test]
    fn hardwired_name_membership_is_the_one_spelling() {
        assert!(is_hardwired_engine_name("tb.engine.repo"));
        assert!(is_hardwired_engine_name("tb.engine.locked-package"));
        // In the namespace, but not one of the hardwired defs.
        assert!(!is_hardwired_engine_name("tb.engine.widget"));
        assert!(!is_hardwired_engine_name("note"));
        assert!(!is_hardwired_engine_name(concat!("au", ".engine", ".repo")));
    }

    #[test]
    fn the_builtin_repo_identity_accepts_both_spellings() {
        assert!(is_builtin_repo(BUILTIN_ENGINE_REPO));
        assert!(is_builtin_repo("tb-engine"));
        assert!(
            !is_builtin_repo(concat!("au-", "engine")),
            "the old repo name is not builtin"
        );
        assert!(is_builtin_repo("tb-engine"));
        assert!(
            !is_builtin_repo(concat!("au-", "engine")),
            "the old repo name is not builtin"
        );
        // Exact identity: a case variant names a different repo.
        assert!(!is_builtin_repo("Au-Engine"));
        assert!(!is_builtin_repo("myrepo"));
        assert!(!is_builtin_repo(""));
    }

    fn claim(raw: &str) -> TypeNameClaim {
        TypeNameClaim::parse(raw, ByteRange::new(0, 0))
    }

    #[test]
    fn a_read_argument_names_the_one_spelling() {
        assert_eq!(
            canonical_query_name("tb.engine.repo::tb-engine"),
            "tb.engine.repo::tb-engine"
        );
        assert_eq!(canonical_query_name("note"), "note");
        assert_eq!(
            canonical_query_name("tb.engine.custom::myrepo"),
            "tb.engine.custom::myrepo"
        );
        // The retired spelling is not rewritten: there is nothing to rewrite.
        let old = concat!("au", ".engine.repo", "::au-", "engine");
        assert_eq!(canonical_query_name(old), old);
    }

    #[test]
    fn a_canonical_spelled_claim_is_already_canonical() {
        // Release 2: the new spelling is canonical, so the seam is the identity
        // for every claim written today.
        for raw in [
            "tb.engine.repo::tb-engine",
            "tb.engine.repo",
            "note::myrepo",
            "note",
        ] {
            let c = claim(raw);
            assert_eq!(
                canonical_schema_claim(c.clone()),
                c,
                "'{raw}' is already canonical"
            );
        }
    }

    #[test]
    fn a_claim_on_the_builtin_keeps_its_identity_and_span() {
        let authored = claim("tb.engine.repo::tb-engine");
        let canonical = canonical_schema_claim(authored.clone());
        assert_eq!(canonical, authored, "the one spelling is already canonical");
        assert_eq!(canonical.span, authored.span, "the span lands on the bytes");
    }

    #[test]
    fn a_knowledge_base_type_keeps_its_own_identity() {
        // In the reserved namespace but not a hardwired def, and under a
        // knowledge base's own peer: nothing to canonicalize.
        for raw in [
            "tb.engine.custom",
            concat!("au", ".engine", ".custom"),
            "tb.engine.custom::tb-engine",
            "tb.engine.custom::myrepo",
        ] {
            let c = claim(raw);
            assert_eq!(
                canonical_schema_claim(c.clone()),
                c,
                "'{raw}' keeps its own identity"
            );
        }
    }

    #[test]
    fn the_type_def_wrapper_keeps_the_name_and_its_claim() {
        let path = PathBuf::from("type/tb.engine.repo.type.yaml");
        let src = b"extends: tb.engine.meta::tb-engine\nfields:\n  x: String\n";
        match parse_file(&path, src) {
            FileParse::TypeDef {
                type_def: Some(td), ..
            } => {
                assert_eq!(td.name.0, "tb.engine.repo");
                assert_eq!(td.parents[0].name.0, "tb.engine.meta");
                assert_eq!(td.parents[0].repo.as_deref(), Some(BUILTIN_ENGINE_REPO));
            }
            other => panic!("expected a type-def, got {other:?}"),
        }
    }

    #[test]
    fn the_instance_wrapper_keeps_the_claim_and_emits_no_note() {
        let path = PathBuf::from("repo.yaml");
        let src = b"type: tb.engine.repo::tb-engine\nname: r\n";
        match crate::parse::parse_engine_schema_instance(
            &path,
            src,
            "tb.engine.repo",
            Some(BUILTIN_ENGINE_REPO),
        ) {
            FileParse::Instance {
                instance: Some(inst),
                diagnostics,
                ..
            } => {
                let claims: Vec<String> = inst
                    .type_claim
                    .iter()
                    .map(TypeNameClaim::authored)
                    .collect();
                assert_eq!(claims, vec!["tb.engine.repo::tb-engine".to_string()]);
                assert!(diagnostics.is_empty(), "{diagnostics:?}");
            }
            other => panic!("expected an instance, got {other:?}"),
        }
    }

    #[test]
    fn a_canonical_engine_schema_file_earns_no_spelling_note() {
        let path = PathBuf::from("repo.yaml");
        let src = b"type: tb.engine.repo::tb-engine\nname: r\n";
        match crate::parse::parse_engine_schema_instance(
            &path,
            src,
            "tb.engine.repo",
            Some(BUILTIN_ENGINE_REPO),
        ) {
            FileParse::Instance { diagnostics, .. } => {
                assert!(diagnostics.is_empty(), "{diagnostics:?}");
            }
            other => panic!("expected an instance, got {other:?}"),
        }
    }

    fn td_named(name: &str) -> TypeDef {
        let path = PathBuf::from(format!("{name}.type.yaml"));
        match parse_file(&path, b"fields:\n  x: String\n") {
            FileParse::TypeDef {
                type_def: Some(td), ..
            } => td,
            other => panic!("'{name}' did not parse to a type-def: {other:?}"),
        }
    }

    #[test]
    fn a_hardwired_name_in_the_repo_is_live_shadow_warning() {
        let td = td_named("tb.engine.repo");
        let d = engine_name_reservation_diag(&td).expect("a reserved name fires");
        assert_eq!(d.code, ENGINE_NAME_LIVE_SHADOW);
        assert_eq!(d.severity, Severity::Warning);
    }

    #[test]
    fn a_reserved_but_unhardwired_name_is_forward_reserved_hint() {
        let td = td_named("tb.engine.widget");
        let d = engine_name_reservation_diag(&td).expect("a reserved name fires");
        assert_eq!(d.code, ENGINE_NAME_FORWARD_RESERVED);
        assert_eq!(d.severity, Severity::Hint);
    }

    #[test]
    fn an_ordinary_name_fires_no_reservation_note() {
        let td = td_named("note");
        assert!(engine_name_reservation_diag(&td).is_none());
    }
}
