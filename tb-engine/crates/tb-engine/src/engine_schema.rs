//! The hardwired `tb.engine.*` schema type-defs.
//!
//! Every engine-schema file is a typed instance of one of these compiled-in
//! defs, owned by a reserved `au-engine` repo identity, never authored in a
//! knowledge base: the in-repo `repo.yaml`, the per-user `repos.yaml` / `workspaces.yaml`,
//! the in-repo `.arsumbris/workspace.yaml`, and the `.arsumbris/` locks. The
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
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use tb_core::{Instance, TypeClaim, TypeDef, TypeName, TypeNameClaim};
use tb_diagnostics::{ByteRange, Diagnostic, DiagnosticCode, Severity, Span, SuggestedFix};

use crate::parse::{FileParse, parse_file};
use crate::repo::{Repo, RepoName};

/// The reserved repo identity owning every hardwired engine-schema def.
///
/// A knowledge base's same-named def coexists as a DISTINCT `(name, hash)` identity, they
/// compose like any cross-repo pair, `::au-engine` scopes the engine's. This is
/// the engine's slice of the layered-ownership pattern (host owns `au.host.*`,
/// harness owns `au.mcp.*`).
pub const BUILTIN_ENGINE_REPO: &str = "tb-engine";

/// The nominal meta marker type-name, `au.engine.meta`. A type is legal in a
/// `meta:` position only if its resolved closure mixes this in (as the qualified
/// `au.engine.meta::au-engine`). Injected into au-core's validator so it stays
/// domain-pure. See
/// [[spec - meta type marker - the meta position admits only types that mix in the engine meta base]].
pub const ENGINE_META_TYPE: &str = "tb.engine.meta";

/// A knowledge base type-def in the reserved `au.engine.*` namespace whose name the
/// engine does NOT hardwire. The namespace is reserved forward: a future engine
/// version may hardwire this name, and the knowledge base def would then live-shadow it.
/// Advisory (`hint`), never blocks; the knowledge base def keeps its own identity and
/// resolves normally. See
/// [[spec - cross-repo resolution - in-repo identity and deps over a per-user repo registry]]
/// "Engine files are typed".
pub const ENGINE_NAME_FORWARD_RESERVED: DiagnosticCode =
    DiagnosticCode::from_static("engine-name-forward-reserved");

/// A knowledge base type-def named exactly like a hardwired `au.engine.*` type. The two
/// COEXIST as distinct `(name, hash)` identities, `::au-engine` scopes the
/// engine's; the code only NOTES the engine owns the name, it never ignores the
/// knowledge base's def. Advisory (`warning`), never blocks. See
/// [[spec - cross-repo resolution - in-repo identity and deps over a per-user repo registry]]
/// "Engine files are typed".
pub const ENGINE_NAME_LIVE_SHADOW: DiagnosticCode =
    DiagnosticCode::from_static("engine-name-live-shadow");

/// A knowledge base type-def that shadows a hardwired `au.engine.*` type AND has DIVERGED
/// from it: its `(name, closure-hash)` identity differs from the engine's copy.
/// Advisory `drift`, the same tier as a drifted pinned reference, never blocks.
/// The base coexistence is `engine-name-live-shadow`; this escalates a diverged
/// shadow so a consumer can rank it above an in-sync one (which fires no drift).
pub const ENGINE_NAME_SHADOW_DRIFT: DiagnosticCode =
    DiagnosticCode::from_static("engine-name-shadow-drift");

/// An engine-schema file carries no written `type:` key. Its kind still assigns
/// the FLOOR type, so the file stays fully correct and the engine reads its data
/// regardless. Advisory `drift`, a high-attention nudge to self-describe: the
/// engine's own writers emit the qualified `type: tb.engine.X::au-engine`, so an
/// absent one reads as a hand-authored or legacy file. Never blocks. See
/// [[spec - engine-schema file claims - the kind assigns a floor, a written type self-describes and mixes in more]].
pub const ENGINE_SCHEMA_TYPE_UNWRITTEN: DiagnosticCode =
    DiagnosticCode::from_static("engine-schema-type-unwritten");

/// An engine-schema file writes a `type:` whose resolved closure OMITS the
/// kind's floor type (e.g. a `repo.yaml` written `type: tb.engine.workspace::au-engine`).
/// The written claim names the wrong kind, so the file does not self-describe as
/// what it structurally is. An `error`; the engine still reads its data from the
/// kind-assigned floor (the resolution path is claim-independent), so it is
/// recoverable, never a lost-data case. A written claim that INCLUDES the floor
/// and mixes in more is fine. See
/// [[spec - engine-schema file claims - the kind assigns a floor, a written type self-describes and mixes in more]].
pub const ENGINE_SCHEMA_TYPE_FLOOR_OMITTED: DiagnosticCode =
    DiagnosticCode::from_static("engine-schema-type-floor-omitted");

/// The engine-schema type namespace in its CANONICAL spelling: the one `DEFS`
/// names and every writer emits. A def named `tb.engine` or under `tb.engine.`
/// sits in the engine's reserved namespace, see [`schema_type_matches`].
///
/// Release 2 flipped this from `au.engine` (tier C step 7). Both spellings are
/// still ACCEPTED — [`LEGACY_ENGINE_NAMESPACE`] is what a release-1 tree holds —
/// so the migration window stays open and no on-disk byte changes meaning.
pub(crate) const ENGINE_NAMESPACE: &str = "tb.engine";

/// The same namespace in its release-1 spelling, accepted on every read and
/// rewritten forward by [`canonical_schema_claim`]. Release 2 never writes it.
pub(crate) const LEGACY_ENGINE_NAMESPACE: &str = "au.engine";

/// The namespace a migration writes: the canonical one, since release 2 flipped
/// it. Named separately from [`ENGINE_NAMESPACE`] so the migration reads as a
/// statement about direction rather than an alias.
pub(crate) const NEXT_ENGINE_NAMESPACE: &str = ENGINE_NAMESPACE;

/// The part of a namespaced engine-schema name after its root: `""` for a bare
/// root, `".repo"` for `au.engine.repo` AND `tb.engine.repo`. `None` for a name
/// in neither spelling. The dot guard keeps an unrelated name like
/// `au.engineering` out of the namespace.
pub(crate) fn schema_leaf(name: &str) -> Option<&str> {
    [ENGINE_NAMESPACE, LEGACY_ENGINE_NAMESPACE]
        .into_iter()
        .find_map(|root| {
            let rest = name.strip_prefix(root)?;
            (rest.is_empty() || rest.starts_with('.')).then_some(rest)
        })
}

/// Whether a type name sits in the engine-reserved namespace, in either
/// spelling: the exact root (`au.engine`, `tb.engine`) or any dotted descendant
/// (`au.engine.<leaf>`, `tb.engine.<leaf>`).
pub(crate) fn schema_type_matches(name: &str) -> bool {
    schema_leaf(name).is_some()
}

/// The canonical (release-1) spelling of a hardwired engine-schema name: the
/// `DEFS` entry the name denotes in either spelling (`tb.engine.repo` becomes
/// `au.engine.repo`). A name that is not one of the hardwired defs, including a
/// knowledge base's own `au.engine.custom` or `tb.engine.custom`, is returned
/// unchanged, so it keeps its own identity.
pub(crate) fn canonical_schema_name(name: &str) -> &str {
    if let Some((canonical, _)) = DEFS.iter().copied().find(|(n, _)| *n == name) {
        return canonical;
    }
    let Some(leaf) = schema_leaf(name) else {
        return name;
    };
    DEFS.iter()
        .map(|(canonical, _)| *canonical)
        .find(|canonical| schema_leaf(canonical) == Some(leaf))
        .unwrap_or(name)
}

/// Whether a name is one of the hardwired engine-schema defs, in either
/// spelling.
pub(crate) fn is_hardwired_engine_name(name: &str) -> bool {
    DEFS.iter()
        .any(|(canonical, _)| *canonical == canonical_schema_name(name))
}

/// The builtin engine repo's identity in its release-1 spelling, accepted on
/// every read and rewritten forward by [`canonical_schema_claim`].
pub(crate) const LEGACY_BUILTIN_ENGINE_REPO: &str = "au-engine";

/// The identity a migration writes: the canonical one, since release 2 flipped
/// it.
pub(crate) const NEXT_BUILTIN_ENGINE_REPO: &str = BUILTIN_ENGINE_REPO;

/// Every spelling of the builtin engine repo's identity, the canonical one
/// first. A `::repo` qualifier naming any of these names the builtin.
pub(crate) const BUILTIN_ENGINE_REPO_SPELLINGS: &[&str] =
    &[BUILTIN_ENGINE_REPO, LEGACY_BUILTIN_ENGINE_REPO];

/// Whether a repo name is the builtin engine repo's identity, in either
/// spelling (`au-engine` in release 1, `tb-engine` from release 2).
///
/// Exact-match, the way [`crate::repo::RepoMap::by_name`] compares repo names: a
/// repo name is a case-sensitive identity, so a case variant is a different
/// repo. A caller that must also fold case (the device config path guard, for a
/// case-insensitive filesystem) compares against [`BUILTIN_ENGINE_REPO_SPELLINGS`]
/// itself.
pub(crate) fn is_builtin_repo(name: &str) -> bool {
    BUILTIN_ENGINE_REPO_SPELLINGS.contains(&name)
}

/// Canonicalize a type name a READ argument carries: `"au.engine.repo::au-engine"`
/// becomes `"tb.engine.repo::tb-engine"`.
///
/// A read names a type the same way a file claims one, so a release-1 client keeps
/// working after the flip: without this, `instances_of au.engine.repo::au-engine`
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
/// `au.engine.custom` or `tb.engine.custom` keeps its own identity.
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

/// A written claim that still spells the release-1 namespace or repo.
///
/// Release 2 flipped the canonical spelling, and the legacy one stays ACCEPTED so
/// a release-1 tree keeps reading. Accepted is not the same as current: a file
/// that writes `au.engine.repo::au-engine` is rewritten in memory to the canonical
/// identity, and the author cannot tell from any read that the bytes on disk are
/// stale. This note makes the window observable, which is what lets it be closed:
/// a migration pass reads the vault, takes every file this fires on, and rewrites
/// exactly those.
///
/// Advisory (`hint`), never blocks. The fix names the canonical spelling so an
/// editor can apply it without consulting this module.
pub const ENGINE_SCHEMA_LEGACY_SPELLING: DiagnosticCode =
    DiagnosticCode::from_static("engine-schema-legacy-spelling");

/// The legacy-spelling note for one written claim, or `None` when the claim is
/// already canonical.
pub(crate) fn legacy_spelling_diag(source: &Path, claim: &TypeNameClaim) -> Option<Diagnostic> {
    let name_is_legacy = claim
        .name
        .0
        .strip_prefix(LEGACY_ENGINE_NAMESPACE)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('.'));
    let repo_is_legacy = claim.repo.as_deref() == Some(LEGACY_BUILTIN_ENGINE_REPO);
    if !name_is_legacy && !repo_is_legacy {
        return None;
    }
    let canonical = canonical_query_name(&claim.authored());
    Some(Diagnostic {
        code: ENGINE_SCHEMA_LEGACY_SPELLING,
        severity: Severity::Hint,
        span: Span::new(source.to_path_buf(), claim.span),
        message: format!(
            "engine-schema claim spells the release-1 form '{}'; it reads as '{canonical}', so the \
             data is correct and the bytes are stale",
            claim.authored()
        ),
        related: vec![],
        fix: Some(SuggestedFix::advisory(format!(
            "write '{canonical}' instead; `au migrate identity` applies this for a whole tree"
        ))),
    })
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

/// Canonicalize the `type:` claim of a parsed instance, bare or list, returning
/// one advisory per claim that was written in the release-1 spelling.
///
/// The note is what makes the migration window observable: the claim already reads
/// correctly, so without it a stale file is indistinguishable from a current one
/// in every read.
pub(crate) fn canonical_schema_instance(inst: &mut Instance) -> Vec<Diagnostic> {
    let source = inst.source_path.clone();
    let mut notes = Vec::new();
    match &mut inst.type_claim {
        TypeClaim::Bare(claim) => {
            if let Some(note) = legacy_spelling_diag(&source, claim) {
                notes.push(note);
            }
            *claim = canonical_schema_claim(claim.clone());
        }
        TypeClaim::List { items, .. } => {
            for claim in items.iter() {
                if let Some(note) = legacy_spelling_diag(&source, claim) {
                    notes.push(note);
                }
            }
            canonicalize_claims(items);
        }
    }
    notes
}

fn canonicalize_claims(claims: &mut [TypeNameClaim]) {
    for claim in claims {
        *claim = canonical_schema_claim(claim.clone());
    }
}

/// The coexistence note for a REPO-authored type-def whose name sits in the
/// reserved `au.engine.*` namespace, or `None` for an ordinary name.
///
/// The engine reserves the namespace by CONVENTION, not annihilation: the knowledge base
/// def keeps its own `(name, hash)` identity and resolves like any other, so the
/// note is advisory. A hardwired name is `engine-name-live-shadow` (both
/// coexist, `::au-engine` scopes the engine's); any other reserved name is
/// `engine-name-forward-reserved` (the engine may claim it later). The builtin's
/// own defs never reach this: they are seeded straight into the `au-engine`
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
pub(crate) const BUILTIN_ROOT: &str = "<au-engine-builtin>";

/// The hardwired defs as `(type-name, source)`. The source is the schema:
/// seven file types, the record element types their list fields reference, and
/// the `au.engine.meta` marker base. Every field is a primitive, a `String[]`, or
/// a `T[]` of one of these element records, so the whole set is expressible with
/// no maps. The `au.engine.readme` file type is body-only, a self-declared
/// markdown node with a section template and no fields. The marker base is a
/// fieldless abstract tag, see
/// [[spec - meta type marker - the meta position admits only types that mix in the engine meta base]].
const DEFS: &[(&str, &str)] = &[
    // --- File types ----------------------------------------------------------
    (
        "tb.engine.repo",
        "\
#: A repo's committed identity and dependency set, `.arsumbris/repo.yaml`.
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
#: The per-user registry, `~/.arsumbris/au-engine/config/repos.yaml`, name to remote+path.
fields:
  repos?: tb.engine.registry-entry[]
",
    ),
    (
        "tb.engine.workspaces",
        "\
#: The per-user workspaces index, `~/.arsumbris/au-engine/config/workspaces.yaml`.
fields:
  workspaces?: tb.engine.workspace-entry[]
",
    ),
    (
        "tb.engine.workspace",
        "\
#: The optional workspace composition, `.arsumbris/workspace.yaml`, edit + discover members. `disabled:` overlays the role lists, a member declared but intentionally not mounted.
fields:
  edit?: String[]
  discover?: String[]
  disabled?: String[]
",
    ),
    (
        "tb.engine.repo-lock",
        "\
#: The per-repo dependency lock, `.arsumbris/repo.lock`, engine-written.
fields:
  packages?: tb.engine.locked-package[]
",
    ),
    (
        "tb.engine.workspace-lock",
        "\
#: The per-workspace discover lock, `.arsumbris/workspace.lock`, engine-written.
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

/// The hardwired `au.engine.*` type-defs, parsed once through the real parse
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

/// The hardwired `au.engine.*` type-defs, cloned from the memoized parse.
pub(crate) fn builtin_engine_defs() -> Vec<TypeDef> {
    BUILTIN_DEFS.clone()
}

/// The compiled-in `au-engine` repo: a fixed identity with no on-disk root,
/// present in every built knowledge base so `::au-engine` resolves as a universal peer
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
    /// `au.engine.meta` marker base.
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
        // The whole set feeds one graph, as it does under the `au-engine` repo
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
    fn schema_namespace_membership_accepts_both_spellings() {
        for name in [
            "au.engine",
            "tb.engine.repo",
            "tb.engine.custom-thing",
            "tb.engine",
            "tb.engine.repo",
            "tb.engine.custom-thing",
        ] {
            assert!(schema_type_matches(name), "'{name}' sits in the namespace");
        }
        // The dot guard keeps a merely-prefixed name out, in both spellings.
        assert!(!schema_type_matches("au.engineering"));
        assert!(!schema_type_matches("tb.engineering"));
        assert!(!schema_type_matches("engine"));
        assert!(!schema_type_matches("note"));
    }

    #[test]
    fn hardwired_name_membership_accepts_both_spellings() {
        assert!(is_hardwired_engine_name("au.engine.repo"));
        assert!(is_hardwired_engine_name("au.engine.locked-package"));
        assert!(is_hardwired_engine_name("tb.engine.repo"));
        assert!(is_hardwired_engine_name("tb.engine.locked-package"));
        // In the namespace, but not one of the hardwired defs.
        assert!(!is_hardwired_engine_name("au.engine.widget"));
        assert!(!is_hardwired_engine_name("tb.engine.widget"));
        assert!(!is_hardwired_engine_name("note"));
    }

    #[test]
    fn the_builtin_repo_identity_accepts_both_spellings() {
        assert!(is_builtin_repo(BUILTIN_ENGINE_REPO));
        assert!(is_builtin_repo("au-engine"));
        assert!(is_builtin_repo("tb-engine"));
        // Exact identity: a case variant names a different repo.
        assert!(!is_builtin_repo("Au-Engine"));
        assert!(!is_builtin_repo("myrepo"));
        assert!(!is_builtin_repo(""));
    }

    fn claim(raw: &str) -> TypeNameClaim {
        TypeNameClaim::parse(raw, ByteRange::new(0, 0))
    }

    #[test]
    fn a_read_argument_is_canonicalized_so_a_release_one_client_still_answers() {
        assert_eq!(
            canonical_query_name("au.engine.repo::au-engine"),
            "tb.engine.repo::tb-engine"
        );
        assert_eq!(canonical_query_name("au.engine.repo"), "tb.engine.repo");
        // A knowledge base's own type is untouched, in either spelling.
        assert_eq!(canonical_query_name("note"), "note");
        assert_eq!(
            canonical_query_name("tb.engine.custom::myrepo"),
            "tb.engine.custom::myrepo"
        );
        // A non-builtin repo qualifier survives while the name is canonicalized.
        assert_eq!(
            canonical_query_name("au.engine.repo::myrepo"),
            "tb.engine.repo::myrepo"
        );
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
    fn a_legacy_spelled_claim_lands_on_the_builtin() {
        // A release-1 file keeps reading: its claim is respelled forward, which is
        // why the migration window stays open.
        assert_eq!(
            canonical_schema_claim(claim("au.engine.repo::au-engine")),
            claim("tb.engine.repo::tb-engine")
        );
        // The legacy spelling of the builtin's own identity, on both sides.
        assert_eq!(
            canonical_schema_claim(claim("au.engine.repo::tb-engine")),
            claim("tb.engine.repo::tb-engine")
        );
        // An unqualified hardwired name loses only its spelling, not its form.
        let bare = canonical_schema_claim(claim("au.engine.repo"));
        assert_eq!(bare, claim("tb.engine.repo"));
        assert_eq!(bare.repo, None);
        // A non-hardwired name under the builtin's other spelling still points at
        // the builtin repo: the qualifier is respelled, the name is kept.
        assert_eq!(
            canonical_schema_claim(claim("note::au-engine")),
            claim("note::tb-engine")
        );
        // The authored span survives, so a diagnostic still lands on the bytes.
        let authored = claim("au.engine.repo::au-engine");
        assert_eq!(canonical_schema_claim(authored.clone()).span, authored.span);
    }

    #[test]
    fn a_knowledge_base_type_keeps_its_own_identity() {
        // In the reserved namespace but not a hardwired def, and under a
        // knowledge base's own peer: nothing to canonicalize.
        for raw in [
            "tb.engine.custom",
            "au.engine.custom",
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
    fn the_type_def_wrapper_canonicalizes_the_name_and_every_claim() {
        let path = PathBuf::from("type/tb.engine.repo.type.yaml");
        let src = b"extends: au.engine.meta::au-engine\nfields:\n  x: String\n";
        match parse_file(&path, src) {
            FileParse::TypeDef {
                type_def: Some(td), ..
            } => {
                assert_eq!(
                    td.name.0, "tb.engine.repo",
                    "the def name is the canonical one"
                );
                assert_eq!(
                    td.parents[0].name.0, "tb.engine.meta",
                    "a legacy-spelled ancestor is respelled forward"
                );
                assert_eq!(td.parents[0].repo.as_deref(), Some(BUILTIN_ENGINE_REPO));
            }
            other => panic!("expected a type-def, got {other:?}"),
        }
    }

    #[test]
    fn the_instance_wrapper_canonicalizes_the_type_claim() {
        let path = PathBuf::from("repo.yaml");
        let src = b"type: au.engine.repo::au-engine\nname: r\n";
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
                assert_eq!(
                    claims,
                    vec!["tb.engine.repo::tb-engine".to_string()],
                    "a legacy-spelled claim reads forward"
                );
                // The claim is CORRECT, and the bytes are stale: exactly one
                // advisory says so, and its fix names the canonical spelling.
                assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
                assert_eq!(diagnostics[0].code, ENGINE_SCHEMA_LEGACY_SPELLING);
                assert_eq!(diagnostics[0].severity, Severity::Hint);
                assert!(
                    diagnostics[0]
                        .fix
                        .as_ref()
                        .is_some_and(|f| f.description.contains("tb.engine.repo::tb-engine")),
                    "the fix names the canonical spelling: {diagnostics:?}"
                );
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
        let td = td_named("au.engine.repo");
        let d = engine_name_reservation_diag(&td).expect("a reserved name fires");
        assert_eq!(d.code, ENGINE_NAME_LIVE_SHADOW);
        assert_eq!(d.severity, Severity::Warning);
    }

    #[test]
    fn a_reserved_but_unhardwired_name_is_forward_reserved_hint() {
        let td = td_named("au.engine.widget");
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
