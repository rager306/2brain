//! IPC over a Unix domain socket: framed JSON, reads and live subscriptions.
//!
//! One endpoint per entry point, at the path [`socket_path`] derives, so the
//! daemon and any consumer reach it from the entry path alone. Each frame is a
//! length-prefixed JSON object. A request names either a `read` or a
//! `subscribe`; every outbound frame is tagged by a `type`, so a consumer
//! demultiplexes responses and events on the one connection. The full contract
//! is `WIRE.md`.
//!
//! The read catalog is closed, a per-capability set from [[design - engine
//! shape]]: diagnostics (whole-knowledge-base, or scoped by path / path-prefix /
//! severity / code); the type graph (all type-defs, one by
//! name); instances of a type by closure membership; the knowledge-base-wide
//! per-instance introspection and the implicit-identity candidate scan; one
//! instance's resolved view with its full value layer; references (a file's
//! outgoing links,
//! backlinks, resolve a target, resolve a block-id); the knowledge base (a directory's
//! children, a file's frontmatter, a file's content, the top-level graphs);
//! the engine's own type-system reference (the embedded atomic specs);
//! and a ready probe surfacing the Deriving-to-Ready transition. A read's
//! `response` frame carries a schema version, the readiness, the observed
//! version, and the result. Reads serve the held analysis; content is the one
//! read that returns source-form text from the working tree.
//!
//! The subscription catalog is likewise closed: lifecycle, types, files,
//! changes, the parametric diagnostics channel, the drawable link_graph and
//! type_graph, and recent_commits, scoped by the same filters as their reads. A
//! `subscribe` is answered by
//! an `ack`, then an `initial_value` when the channel has one, then
//! `change_event` frames. Most channels wake on the engine's version signal;
//! recent_commits is the exception, a git-state projection woken by an
//! on-demand reflog watcher (off the version signal), see [`run_recent_commits`].
//! Subscriptions live with the connection and die when it closes. The
//! connection runs a writer task draining a frame channel, so a response and an
//! event never interleave a partial frame, and one task per subscription. Each
//! data channel diffs its own projection across rebuilds and carries the delta
//! in its change events' scope hint: changed files, changed paths, changed
//! type names, files whose diagnostics changed.
//!
//! Alongside both is one control verb, `shutdown`, the lifecycle signal `au
//! daemon stop` sends; a foreground server parks on
//! [`ServeHandle::wait_for_shutdown`] until it arrives.
//!
//! The serve layer is async on a private tokio runtime; the analysis core stays
//! sync, its held-state lock taken briefly inside a handler and never across an
//! await. Windows named-pipe transport is deferred; this is Unix-only. The wire
//! shape (length-prefixed JSON) is transport-agnostic and would carry over.

use std::collections::BTreeMap;
use std::io::{ErrorKind, Read as IoRead, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tb_diagnostics::{Diagnostic, Severity};
use tb_grammar::{Shape, ShapeSpanRole};
use tb_parser::{BodyEvent, scan_body};
use tb_references::{extract_field_marker, parse_wikilink, parse_wikilink_inner};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::runtime::Runtime;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinSet;
use tracing::Instrument;

use crate::engine::{EngineHandle, Read, RefState};
use crate::gitwriter::{CommitMessage, CommitSha, GitWriter, MutationId, ShellGit};
use crate::mutate::MutationReject;
use crate::parse::{FileParse, ShapeLeafSpan, served_file_kind};
use crate::wire;
use crate::{KnowledgeBase, RefSurface};

/// The wire schema version stamped on every frame. v29: an `inline_record` value
/// (in `effective_values`) replaces its raw-mapping `value` with `fields`, each
/// nested field resolved through the value model exactly like a top-level field
/// and recursively to any depth, so a tuple / brand / reference inside a nested
/// record reads resolved, not as a raw string. BREAKING (a type-change of an
/// existing field). v24: the write verbs' `stamp`
/// rider (a single stamp) is replaced by `stamps`, a LIST of stamps applied in
/// order into the write's one commit, so many stampers fold into one mutation. A
/// length-1 list is exactly the former single stamp. v22: `references_out` gains a
/// new `kind` value `commit-referent`, for a commit-only reference
/// (`[[::@sha]]` / `[[::repo@sha]]`) that names a commit rather than a file. Its
/// `resolved` is null and `commit` is set, but it is NOT dangling. v18: a type view
/// (`TypeIntrospection`) gains three fields for the abstract / required-meta
/// batch: `abstract` (bool, the `abstract: true` marker), `required_meta`
/// (String[], the type's own `required:` obligations, authored forms), and
/// `unmet_required_meta` (String[], the required-meta names a non-abstract type
/// does not satisfy, empty when satisfied / exempt / no resolution graph). v17
/// (prior). v16: the `members` and
/// `resolve_member` reads drop the `primary` boolean and gain a `local` boolean
/// (a live working tree vs a read-only cache snapshot, the LOCATION axis) and a
/// `role` string (`entry` / `edit` / `discover` / `dep`); `editable` is now the
/// role axis (an editable authoring surface), no longer location-derived. A
/// consumer hides consumed members with `!editable` and scopes physical writes
/// with `editable && local`. v15: the hardwired
/// `au.engine.workspace` def is reshaped from a single `primary: String[+]`
/// selection to two optional lists, `edit?: String[]` and `discover?: String[]`,
/// and a new `au.engine.workspace-lock` def (`packages?`, parallel to
/// `au.engine.repo-lock`) pins the workspace's discover closure; a consumer
/// reads these on `type_system_reference` / `instances_of`. v14: (prior). v13:
/// `instances_of` returns
/// ALL instances of a type tagged by `origin` (file / nested inline record /
/// type-def meta), not only file-level; each match gains `origin`, a byte
/// `span`, and an origin-specific `locator`; the request accepts an `origins`
/// include-set filter (absent = all, a change from the old file-only default).
/// v12: a new `device_config`
/// read exposes the per-user device-global engine-schema files (`repos.yaml`,
/// `workspaces.yaml`), each file's path, content, and field-shape diagnostics
/// against its hardwired `au.engine.*` def. v11: the workspace-wide type
/// reads (`types`, `type_tree`, `type_counts`, `subtypes`, `list_imports`, and
/// the repo-scoped `types` / `type_counts`) accept an optional `scope: all | own`
/// argument (default `all`, unchanged); `own` hides every dependency and the
/// `au.engine.*` builtin, surfacing only the user's own primary, editable repos.
/// v10: the `resolve` frame
/// replaces its single `commit` / `commit_error` scalars with per-repo
/// `commits: { [repo]: sha }` / `commit_errors: { [repo]: message }` maps, one
/// entry per editable repo whose own `.arsumbris/repo.lock` committed, since the
/// dependency lock moved from a per-workspace file to a per-repo one. v9: the
/// `members` read
/// replaces each member's `provenance` string with two booleans, a derived
/// `editable` (a local working tree vs a read-only cache snapshot) and a
/// `primary` (a named primary vs a computed transitive dependency). v4: a
/// request may carry an `id`, echoed on its `response` / `ack` / `error` reply
/// so clients settle by id; `error` frames also carry a `for` discriminator. v3:
/// `resolve_target` returns `{ path, kind }` instead of the bare path string.
///
/// Public so a consumer (e.g. the `daemon status` surface) can compare a running
/// daemon's reported `schema_version` against the version this binary compiles,
/// and flag a stale process speaking an old wire.
pub const SCHEMA_VERSION: u32 = 29;

/// The largest frame payload accepted off the wire. A client-supplied length
/// prefix above this is a protocol violation, not a real request: the reader
/// rejects it before allocating, so a stray or hostile length cannot drive an
/// unbounded allocation. Generous for any genuine read, subscribe, or mutate.
const MAX_FRAME: usize = 16 << 20;

/// The serialized outbound frames a connection's producers hand to its writer
/// task. The writer drains this, framing one at a time, so responses and events
/// never interleave a partial frame on the socket.
type FrameTx = mpsc::Sender<Vec<u8>>;

/// The per-entry socket path, `<state dir>/au-engine/run/<hash>.sock` — `~/.2brain` on a
/// migrated or fresh home, `~/.arsumbris` on one that has not been migrated.
///
/// One endpoint per entry point, derivable by both the daemon and a consumer
/// from the entry path alone, no configuration. `entry` is the absolute path the
/// engine was pointed at, a folder-repo directory, so each entry point gets its
/// own socket and many workspaces in one folder never collide.
///
/// The socket lives OUTSIDE the knowledge base, so a deep knowledge base path can no longer overrun
/// the Unix-socket `sun_path` limit (the SUN_LEN failure). The `<hash>` is the
/// FNV-1a of the entry path's bytes, the engine's own [`crate::ContentHash`], so
/// the file name is short and fixed-width and a consumer in any language derives
/// the same path by hashing the same bytes.
///
/// The caller passes a canonicalized absolute path (the CLI's `canonical_root`,
/// a consumer's realpath), so the daemon and a client hash an identical string.
pub fn socket_path(entry: &Path) -> PathBuf {
    sockets_dir().join(socket_file_name(entry))
}

/// The socket file name for an entry, `<hash>.sock`.
///
/// A pure deterministic function of the entry path's bytes, no env and no IO:
/// the FNV-1a hash (the engine's [`crate::ContentHash`]) rendered as fixed-width
/// hex. Exposed so a consumer, or a test with an injected home, derives the same
/// name the daemon binds under. Distinct entry paths hash to distinct names, so
/// each workspace file in a shared folder gets its own socket.
pub fn socket_file_name(entry: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let hash = crate::ContentHash::of(entry.as_os_str().as_bytes());
    format!("{:016x}.sock", hash.0)
}

/// The directory holding per-entry daemon sockets, `<state dir>/au-engine/run`.
///
/// Read from RAW `$HOME`, not the canonicalized [`crate::device_root`], because
/// this path is a wire contract mirrored byte-for-byte by the SDK, which reads
/// `process.env.HOME` unresolved: both sides must agree, so both read `$HOME` as
/// given. Falls back to the system temp dir when `$HOME` is unset (a headless run,
/// unreachable on the daemon-start path, which requires `$HOME`), so a socket path
/// always exists for `stop` / `status`.
fn sockets_dir() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    crate::repo::state_dir_in(&home)
        .join("au-engine")
        .join("run")
}

#[cfg(test)]
mod wire_doc_tests {
    use super::*;

    /// `WIRE.md` is hand-maintained with no generator, so its version summary can
    /// lag a [`SCHEMA_VERSION`] bump silently. This asserts the doc's `currently
    /// \`N\`` line matches the const, turning that drift into a caught build error.
    #[test]
    fn wire_md_summary_matches_schema_version() {
        let wire = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/WIRE.md"));
        // The single "current version" summary line, e.g. "currently `29`.".
        let marker = "the wire version, an integer, currently `";
        let start = wire
            .find(marker)
            .expect("WIRE.md must carry the wire-version summary line");
        let rest = &wire[start + marker.len()..];
        let end = rest
            .find('`')
            .expect("the version number must be closed by a backtick");
        let documented: u32 = rest[..end]
            .parse()
            .expect("the documented wire version must be an integer");
        assert_eq!(
            documented, SCHEMA_VERSION,
            "WIRE.md version summary (`{documented}`) is stale, bump it to \
             SCHEMA_VERSION (`{SCHEMA_VERSION}`)"
        );
    }
}

#[cfg(test)]
mod request_deser_tests {
    use super::*;

    /// The nested internal tags compose: `read` selects the preview verb, then
    /// `op` selects the mutation shape, with its args flat alongside.
    #[test]
    fn preview_mutation_request_deserializes_each_op() {
        let write: Request = serde_json::from_value(serde_json::json!({
            "read": "preview_mutation",
            "op": "write_file",
            "path": "a.md",
            "content": "x",
        }))
        .unwrap();
        match write {
            Request::PreviewMutation(PreviewMutationArgs::WriteFile {
                path,
                content,
                stamps,
                ensure_mixins,
                ensure_mixins_strict,
            }) => {
                assert_eq!(path, "a.md");
                assert_eq!(content, "x");
                assert!(stamps.is_empty());
                // Absent riders default as on the mutation: none, strict.
                assert!(ensure_mixins.is_empty());
                assert!(ensure_mixins_strict);
            }
            other => panic!("expected a write op, got {other:?}"),
        }

        let edit: Request = serde_json::from_value(serde_json::json!({
            "read": "preview_mutation",
            "op": "edit_file",
            "path": "a.md",
            "old_string": "x",
            "new_string": "y",
            "replace_all": true,
        }))
        .unwrap();
        assert!(matches!(
            edit,
            Request::PreviewMutation(PreviewMutationArgs::EditFile {
                replace_all: true,
                ..
            })
        ));

        let delete: Request = serde_json::from_value(serde_json::json!({
            "read": "preview_mutation",
            "op": "delete_file",
            "path": "a.md",
        }))
        .unwrap();
        assert!(matches!(
            delete,
            Request::PreviewMutation(PreviewMutationArgs::DeleteFile { .. })
        ));

        let rename: Request = serde_json::from_value(serde_json::json!({
            "read": "preview_mutation",
            "op": "rename",
            "path": "a.md",
            "to": "sub/a.md",
            "ensure_mixins": ["m::peer"],
            "ensure_mixins_strict": false,
        }))
        .unwrap();
        match rename {
            Request::PreviewMutation(PreviewMutationArgs::Rename {
                path,
                to,
                stamps,
                ensure_mixins,
                ensure_mixins_strict,
            }) => {
                assert_eq!((path.as_str(), to.as_str()), ("a.md", "sub/a.md"));
                assert!(stamps.is_empty());
                assert_eq!(ensure_mixins, vec!["m::peer".to_string()]);
                assert!(!ensure_mixins_strict);
            }
            other => panic!("expected a rename op, got {other:?}"),
        }

        // A type rename is addressed by name, not path: no `path`, and the two
        // names are the whole request.
        let claim: Request = serde_json::from_value(serde_json::json!({
            "read": "preview_mutation",
            "op": "rename_type",
            "old_name": "note::base",
            "new_name": "memo",
        }))
        .unwrap();
        match claim {
            Request::PreviewMutation(PreviewMutationArgs::RenameType { old_name, new_name }) => {
                assert_eq!(
                    (old_name.as_str(), new_name.as_str()),
                    ("note::base", "memo")
                );
            }
            other => panic!("expected a rename_type op, got {other:?}"),
        }
    }
}

#[cfg(test)]
mod socket_path_tests {
    use super::*;

    /// The file name is a fixed-width `<16-hex>.sock`, no matter how deep the
    /// entry path — the SUN_LEN fix: a deep knowledge base path no longer feeds the
    /// socket path, a constant-length hash does.
    #[test]
    fn file_name_is_short_and_fixed_width_regardless_of_entry_depth() {
        let shallow = socket_file_name(Path::new("/a"));
        let deep = socket_file_name(Path::new(
            "/Users/someone/very/deeply/nested/repo/tree/that/would/overrun/sun_path/workspace",
        ));
        assert_eq!(shallow.len(), 21, "16 hex + '.sock'"); // 16 + 5
        assert_eq!(deep.len(), 21, "the deep path hashes to the same width");
        assert!(shallow.ends_with(".sock"));
        assert!(deep.ends_with(".sock"));
    }

    /// Deterministic: the same entry always derives the same socket.
    #[test]
    fn file_name_is_deterministic() {
        let a = socket_file_name(Path::new("/repo/kb"));
        let b = socket_file_name(Path::new("/repo/kb"));
        assert_eq!(a, b);
    }

    /// Distinct entries derive distinct sockets, so a directory and a manifest
    /// file under it (and two manifest files in one folder) never collide.
    #[test]
    fn distinct_entries_derive_distinct_sockets() {
        let dir = socket_file_name(Path::new("/repo/kb"));
        let manifest_a = socket_file_name(Path::new("/repo/kb/a/.arsumbris/workspace.yaml"));
        let manifest_b = socket_file_name(Path::new("/repo/kb/b/.arsumbris/workspace.yaml"));
        assert_ne!(dir, manifest_a);
        assert_ne!(manifest_a, manifest_b);
    }

    /// The socket lives outside the knowledge base, under the device state dir's
    /// `au-engine/run/`, so it never sits in a watched member tree. Which spelling that
    /// directory has is [`crate::repo::state_dir_in`]'s decision, not this test's.
    #[test]
    fn socket_is_under_state_run() {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let expected = crate::repo::state_dir_in(&home)
            .join("au-engine")
            .join("run")
            .join(socket_file_name(Path::new("/repo/kb")));
        assert_eq!(socket_path(Path::new("/repo/kb")), expected);
    }
}

/// A request over the socket, tagged by `read`.
///
/// Most variants are reads of the held analysis. `Shutdown` is the one control
/// verb: it is not a read of knowledge base state but the daemon-lifecycle signal `au
/// daemon stop` sends, asking the process to exit. It rides the same socket so
/// stopping needs no PID file.
#[derive(Debug, Deserialize)]
#[serde(tag = "read", rename_all = "snake_case")]
enum Request {
    /// Knowledge-base diagnostics, optionally scoped by [`DiagnosticsFilterArgs`],
    /// paged by `limit` / `offset`.
    Diagnostics(DiagnosticsFilterArgs),
    /// Counts over the filtered diagnostics set: total, by severity, by code.
    /// The shape of the problem without materializing every entry.
    DiagnosticCounts(DiagnosticsFilterArgs),
    /// Type-defs in the wire shape, each annotated with its repo. Absent `repo`
    /// is the workspace-wide read: every type-def across all members, deduped by
    /// identity to owner repos. `repo` scopes to one member's whole graph (its
    /// own defs). An unknown repo → null. Paged by `limit` / `offset`; projected
    /// to a lightweight summary by `summary`. See [`TypesArgs`].
    Types(TypesArgs),
    /// Counts over the `types` set for the same `repo` scope: a total plus a
    /// by-repo histogram. The shape of the vocabulary without materializing
    /// every def, the type dual of `diagnostic_counts`. An unknown repo → null.
    TypeCounts(TypeCountsArgs),
    /// The cross-repo type tree: the workspace's owner-deduped type-defs as a
    /// parent/child adjacency forest, owner-annotated per node. `scope: own`
    /// keeps only the user's own repos' types (see [`TypeTreeArgs`]).
    TypeTree(TypeTreeArgs),
    /// One type-def by `name`, annotated with its `repo` and identity `hash`;
    /// null when absent. Named by the authored form — bare (`foo`, the owner
    /// across the workspace) or `::repo`-qualified (`foo::repo`, the identity
    /// that repo holds) — with no separate `repo` key.
    ///
    /// The batch form is its own verb, [`Request::TypeBatch`]. One verb
    /// answering two cardinalities under one payload key would be the envelope
    /// rule's only ambiguous case, so it is two verbs and each is mono-shaped.
    Type(TypeArgs),
    /// Several type-defs by `names`: an array of results, ONE per requested
    /// name, in request order, each the single-`name` result or null for an
    /// unresolved name. The detail-on-demand dual of the `types` summary, so a
    /// consumer drilling into several summary entries pays one round trip.
    TypeBatch(TypeBatchArgs),
    /// Instances whose effective closure contains the named type, across all
    /// origins (file / nested inline record / type-def meta). `origins` narrows
    /// the site kinds returned; absent means all.
    InstancesOf(InstancesOfArgs),
    /// The workspace's discovered cross-repo import set: one record per
    /// (importing repo, imported type identity), the fold-axis `::repo` types
    /// each repo authors. `scope: own` keeps only imports made by the user's own
    /// repos and drops the automatic `au.engine.*` builtin fold (see
    /// [`ImportsArgs`]).
    Imports(ImportsArgs),
    /// Literal text search over the catalogued files, optionally scoped by repo
    /// or to the user's own repos. Answers one match per line, with a 1-based
    /// line and character column, paged by `limit` / `offset`. Additive, no
    /// `schema_version` bump. See [`GrepArgs`] for the scan bounds.
    Grep(GrepArgs),
    /// Ranked lexical search over the catalogued note text: BM25 over each note's
    /// body and its `description:`, answered from the held index so no file is
    /// read. One match per document, best score first, paged by `limit` /
    /// `offset`. The ranking dual of `grep`, which stays the literal byte scan.
    /// Additive, no `schema_version` bump. See [`SearchArgs`].
    Search(SearchArgs),
    /// Every type-def across the workspace whose closure includes `base`, each
    /// as its owning repo's def with the owner repo. The type-level
    /// dual of `instances_of`. The base itself is excluded. `scope: own` keeps
    /// only subtypes owned by the user's own repos (see [`SubtypesArgs`]).
    Subtypes(SubtypesArgs),
    /// Knowledge-base-wide per-instance introspection: every resolved instance's
    /// closure, effective shape, collisions, and full value layer.
    Instances,
    /// Knowledge-base-wide implicit-identity candidate scan, grouped by file. Paged by
    /// `limit` / `offset` (over files); projected to a lightweight summary
    /// (file + candidate type names) by `summary`. See [`CandidatesArgs`].
    Candidates(CandidatesArgs),
    /// Counts over the candidate scan: totals plus a by-type file histogram
    /// ("N untyped files could claim type X"). The candidates dual of
    /// `diagnostic_counts`, over the full scan (no paging). Takes no args.
    CandidateCounts(CandidateCountsArgs),
    /// Per-type instance counts: a total plus a per-identity histogram, so a
    /// consumer renders a vocabulary-with-counts overview in one round-trip. The
    /// instances dual of `type_counts`, scoped by the same `repo` / `scope` axis
    /// applied to the SITE'S repo. Closure-inclusive: a count equals the length
    /// of the matching `instances_of` drill-in over the same origins. An
    /// `origins?` filter (file / nested / meta) mirrors `instances_of`; absent
    /// counts all three. An unknown repo → null.
    InstanceCounts(InstanceCountsArgs),
    /// One instance's resolved view, with the full value layer. Null when the
    /// path is not a parsed instance.
    ///
    /// Named `instance` (singular) against the knowledge-base-wide `instances`, the same
    /// singular/plural pair as `type` / `types`. The former name, `resolved`,
    /// was a past participle in a catalog of nouns, and collided with this
    /// view's own `resolved` boolean field.
    Instance { path: String },
    /// Inbound reference edges to a file. Named against `references_out`, so
    /// the one axis reads as one pair; formerly `backlinks`.
    ReferencesIn { path: String },
    /// Outgoing references from a file's body, in source order.
    ReferencesOut { path: String },
    /// Reverse-by-target inert-pin lookup: every commit-pinned reference naming
    /// `target` whose source file contains an instance of `source_type`. A fold
    /// over the retained OUTBOUND pins, not a backlink — an inert pin forms no
    /// inbound edge. Both fields required; `source_type` scopes the source-file
    /// set to `instances_of(source_type)`, keeping the fold off the whole graph.
    /// No time awareness: a since-reused name returns EVERY pin naming it, the
    /// consumer windows. See [[spec - pinned references - a recorded resolved
    /// edge with an immutable past and an on-demand forward trace]].
    Pins { target: String, source_type: String },
    /// Per-commit metadata for a set of commits, member-aware, off the build
    /// path. Each `commits[]` entry names a commit and optionally the member
    /// (`repo`) whose store to resolve it in, defaulting to the entry repo. The
    /// join partner for `pins`: a consumer collects the distinct
    /// `{ commit, repo? }` set and calls this once. Positional, one record per
    /// input in order; an absent commit is `available: false`. See
    /// [[spec - engine-mediated git reads - member-aware commit metadata and file history over the object store]].
    CommitMeta { commits: Vec<CommitRef> },
    /// A file's commit stream, member-aware, off the build path. `path` is the
    /// file (relative to the entry repo, or to `repo`'s member root when given).
    /// Each row carries the commit, timestamp, author, subject, and a `status` /
    /// `from` that are git's OWN rename similarity heuristic, best-effort, not
    /// authoritative. A non-git member or an unknown path yields an empty stream.
    /// See [[spec - engine-mediated git reads - member-aware commit metadata and file history over the object store]].
    FileHistory {
        path: String,
        #[serde(default)]
        repo: Option<String>,
    },
    /// A bounded, newest-first commit stream merged across the workspace's
    /// working trees, member-aware, off the build path. `members` filters to a
    /// subset by member name (each mapped to its owning tree and deduped),
    /// default all trees. `limit` (count) and / or `since` (a git time window)
    /// bound it; with neither, a default cap applies. Each row is keyed by its
    /// working-tree `tree` root plus the `members` in it, and carries
    /// `author{name,email}`, `timestamp`, `subject`, `changed_files`, and
    /// `trailers`. See [[spec - recent-commits activity stream - a member-aware
    /// bounded commit stream with a reflog-watched append-only subscription]].
    RecentCommits {
        #[serde(default)]
        members: Option<Vec<String>>,
        #[serde(default)]
        limit: Option<usize>,
        #[serde(default)]
        since: Option<String>,
    },
    /// A bounded N-hop reference-graph walk from `path`: the reachable subgraph
    /// of nodes plus the edges traversed. Direction and depth are arguments. See
    /// [`NeighborhoodArgs`] and [[spec - neighborhood read - a bounded n-hop
    /// reference walk returning a subgraph of files and addressable blocks]].
    Neighborhood(NeighborhoodArgs),
    /// Resolve a wikilink target to a file path; null when unresolved.
    /// `target` carries the wikilink fragment grammar, so an embedded `::repo`
    /// qualifier is honored. `origin` scopes resolution to the file the link
    /// appears in, the way `references_out` resolves a body link; absent
    /// `origin` resolves a `::repo` target against the named repo and a bare
    /// target scopelessly across every repo index.
    ResolveTarget {
        target: String,
        #[serde(default)]
        origin: Option<String>,
    },
    /// Resolve a `^block-id` inside a target file to its typed block; null
    /// when the target or block doesn't resolve. `origin` scopes the target's
    /// resolution as in `resolve_target`.
    ResolveBlockId {
        target: String,
        block_id: String,
        #[serde(default)]
        origin: Option<String>,
    },
    /// Resolve a `#head` anchor inside a target file to its heading; null
    /// when the target or anchor doesn't resolve. Matching per
    /// [[type reference::au-type-system]]: case-insensitive exact heading text, first
    /// match in document order. `origin` scopes the target's resolution as in
    /// `resolve_target`.
    ResolveAnchor {
        target: String,
        anchor: String,
        #[serde(default)]
        origin: Option<String>,
    },
    /// Every heading in a target file, the listing dual of `resolve_anchor`:
    /// what a `#anchor` fragment can address, rather than whether one address
    /// resolves. Null when the target doesn't resolve; an empty array for a
    /// resolved file carrying no headings. `origin` scopes the target's
    /// resolution as in `resolve_target`.
    Anchors {
        target: String,
        #[serde(default)]
        origin: Option<String>,
    },
    /// Every addressable id in a target file, the listing dual of
    /// `resolve_block_id`: what a `^block_id` fragment can address, rather
    /// than whether one address resolves. Every occurrence is listed,
    /// duplicates included. Null when the target doesn't resolve; an empty
    /// array for a resolved file carrying none. `origin` scopes the target's
    /// resolution as in `resolve_target`.
    BlockIds {
        target: String,
        #[serde(default)]
        origin: Option<String>,
    },
    /// The resolvable file set, what a `[[` wikilink target can address: every
    /// catalogued file, assets included, with the stem a bare `[[name]]`
    /// resolves by. The listing dual of `resolve_target`, and the read dual of
    /// the `files` subscription channel. See [`FilesArgs`].
    Files(FilesArgs),
    /// Direct entries (files and subdirectories) of a directory. Named
    /// `dir_entries`, not `children`, because `type_tree.nodes[].children`
    /// already means graph children in this same API; formerly `children`.
    DirEntries { dir: String },
    /// A file's parsed frontmatter as a JSON map; null for non-instances.
    Frontmatter { path: String },
    /// A file's source text, content hash, and pin anchor,
    /// `{ content, hash, commit }`; null when unreadable. `hash` is the
    /// guard-usable `expected_hash`; `commit` is HEAD of the owning repo (null
    /// when not a git working tree), the pin anchor for the returned bytes.
    Content { path: String },
    /// A file's typed highlight tokens, an ordered `(range, kind)` stream;
    /// null when the path is not a readable markdown file.
    SemanticTokens { path: String },
    /// Validate a transient JSON value against a named type-def, returning the
    /// diagnostics a file with that frontmatter would get. The value never
    /// touches disk; spans index a synthesized document, not the value.
    ///
    /// MULTI-FIT: one verdict per mounted identity the name denotes. An unknown
    /// name yields ONE null-identity verdict carrying `unknown-type-claim`, the
    /// verdict a file gets, so a miss fails closed.
    ValidateValue {
        type_name: String,
        value: serde_json::Value,
        /// Scope to one repo's identity; absent = every mounted repo owning the
        /// name. A `::repo` in `type_name` wins over this arg.
        #[serde(default)]
        repo: Option<String>,
    },
    /// The workspace's declared members: each member's name, absolute root, and
    /// whether it is scattered outside the workspace root or a subdir of it.
    /// Optional `repo` narrows to that one member, so `members({repo})` mirrors
    /// `overview({repo}).members`.
    Members(MembersArgs),
    /// A member's file-scope rules: the editable `.auignore` patterns plus the
    /// non-editable default excludes and hard floor, for one member (`repo`) or
    /// every member. `resolve` adds the boundary-level effect (pruned dirs,
    /// excluded files) at a bounded extra walk, reported at boundaries not
    /// contents. An unknown `repo` yields an empty member list. See
    /// [[spec - scope management surface - an ignores read and a set_ignores config mutation]].
    Ignores {
        #[serde(default)]
        repo: Option<String>,
        #[serde(default)]
        resolve: bool,
    },
    /// Which declared member owns a path: the member's name and absolute root;
    /// null when the path lies under no declared member.
    ResolveMember { path: String },
    /// The per-user device-global engine-schema files (`repos.yaml`,
    /// `workspaces.yaml`): each file's resolved path, content, and field-shape
    /// diagnostics against its hardwired `au.engine.*` def. These sit outside
    /// every knowledge base, so the diagnostics land here, not on the `diagnostics` read.
    /// An entry is null when its path cannot be resolved (no per-user config
    /// directory). See
    /// [[spec - engine-schema files - hardwired-schema files are first-class substrate nodes]].
    DeviceConfig,
    /// One CONSUMER config file under the scoped-config channel,
    /// `<scope>/<consumer>/config/<file>`: its resolved path, content, and
    /// field-shape diagnostics against a stamped `type`. Out-of-band by the verb,
    /// never a walked node, the shape `device_config` returns. Machine scope
    /// resolves under `~/.arsumbris/`; repo scope under a declared member's
    /// `.arsumbris/`. An unresolved `type` is stored-as-is with a
    /// `config-type-unresolved` advisory. See
    /// [[spec - scoped config channel - a config read and set_config mutation over scope, consumer, file, type]].
    Config(ConfigArgs),
    /// The top-level directories of every mounted member, each tagged with its
    /// owning repo. `repo` selects one member, absent spans all; `scope`
    /// filters by class (`own` keeps the user's editable repos). Formerly
    /// `top_level_graphs`, which was entry-scoped and misnamed — these are
    /// folders, and nothing about the typed graph is consulted.
    TopLevelDirs(TopLevelDirsArgs),
    /// The up-front orientation map: workspace shape (members, top-level
    /// graphs), vocabulary shape (type counts), health shape (diagnostic
    /// counts), and the hub-node ranking over the typed reference graph. One
    /// cheap read a consumer loads first, then drills through the other reads.
    /// See [[spec - orientation overview read - a computed up-front map with a
    /// hub ranking over the typed reference graph]].
    Overview(OverviewArgs),
    /// The resolved ancestor closure and effective field set of a type
    /// identity. A bare `name` conflates, so it answers one closure PER
    /// matching identity; `repo` (or a `::repo` in the name) scopes to one.
    /// See [[spec - cross-repo identity on the wire - a name conflates, a
    /// qualifier scopes to identity, every result carries owner and hash]].
    TypeClosure(TypeClosureArgs),
    /// The most-referenced files over the typed reference graph, the "start
    /// here" signal. Promoted out of `overview`, which carried the ranking as
    /// an unmirrored field: every other `overview` field is reproducible by one
    /// call to its own read, and this one had no read to call.
    Hubs(HubsArgs),
    /// The scalar whole-graph summary folded from the catalog + backlink index:
    /// connected-component count, orphan counts, degree histograms, and density.
    /// The "your knowledge base is 14 disconnected components" signal, a fact
    /// you QUERY, never a diagnostic that nags. `repo` / `scope` like `overview`,
    /// mirrored by `overview.graph_shape`. See [[spec - whole-graph reads - a
    /// scalar shape summary and a full node-edge payload folded from the
    /// backlink index]].
    GraphShape(GraphShapeArgs),
    /// What reverting one commit would touch, without touching anything: the
    /// paths the commit changed, whether each still holds its bytes, and whether
    /// a later commit rewrote it. Additive, no `schema_version` bump.
    RollbackPreview(RollbackPreviewArgs),
    /// The graph's problems joined in one answer: directed cycles (SCCs with more
    /// than one node, plus self-loops), dead ends (no outbound edge), sources (no
    /// inbound edge), broken links (reference diagnostics), and the most central
    /// nodes by total degree. `repo` / `scope` like `graph_shape`; every list is
    /// capped by `limit` while `counts` covers the whole in-scope graph.
    /// Additive, no `schema_version` bump.
    GraphHealth(GraphHealthArgs),
    /// The full whole-graph node+edge payload a force-directed visualization
    /// lays out: every content file as a node (isolated ones included), every
    /// resolved reference as an edge. `repo` / `scope` like `graph_shape`. The
    /// un-ranked, un-truncated sibling of `hubs`, and the global-unseeded
    /// complement of `neighborhood`. See [[spec - whole-graph reads - a scalar
    /// shape summary and a full node-edge payload folded from the backlink
    /// index]].
    LinkGraph(LinkGraphArgs),
    /// The type graph as a node+edge payload, the type-side sibling of
    /// `link_graph`: type-def nodes with `subtype` / `field-type` edges by
    /// default, `instance-of` (adding claiming-instance nodes) and `meta` edges
    /// opt-in via `edges`. `repo` / `scope` like `link_graph`. Drawn beside
    /// `link_graph` and merged by path. See [[spec - whole-graph reads - a
    /// scalar shape summary and a full node-edge payload folded from the
    /// backlink index]].
    TypeGraph(TypeGraphArgs),
    /// Simulate a deterministic mutation over an overlay of the current snapshot
    /// and report its product, the would-be file's type identities and
    /// diagnostics plus the per-file blast radius, WITHOUT writing disk or
    /// committing. The pre-tool gate surface: a mediator previews a pending write
    /// to know its product before it lands. `op` selects the mutation to
    /// simulate; its arguments ride alongside. Structural refusals and a
    /// non-mountable path fold into the result's `reject`, so this is always a
    /// read, never a mutation. See [[spec - mutation preview read - simulate a
    /// write over an overlay and report its product without committing]].
    PreviewMutation(PreviewMutationArgs),
    /// The engine and ref lifecycle states. Named `lifecycle`, not `ready`,
    /// because "ready" meant three things at once: the frame's can-I-answer
    /// flag, this read, and a state VALUE. The frame keeps the flag.
    Lifecycle,
    /// Control verb: ask the daemon to shut down. Acked, then the serving
    /// process unblocks and exits, removing the socket.
    Shutdown,
}

/// The `scope` value on the wire: `all` (every mounted member) or `own` (the
/// user's own primary, editable repos only — every dependency and the
/// `au.engine.*` builtin hidden). This is the value a caller STATES. What a read
/// answers when the caller states nothing is that read's [`ScopeDefault`],
/// declared per read in [`ScopedArgs::DEFAULT`] and documented, with its reason,
/// in `SCOPE_TABLE` below.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Scope {
    All,
    Own,
}

/// The one `scope` argument type on the serving surface. Every read that takes
/// `scope` takes this one.
///
/// Only ABSENCE is interpretable: a stated value always means what it says, and
/// [`ScopeArg::resolve`] is the single place it becomes the concrete
/// [`wire::TypeScope`] the views apply. So there is one spelling of "the user's
/// own repos", one resolver, and one table that says what each read does when the
/// caller is silent — rather than a read's default living in its handler, its
/// args struct, and its documentation.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
struct ScopeArg(Option<Scope>);

impl<'de> Deserialize<'de> for ScopeArg {
    /// The two wire values, or none. The absent state is not a third spelling a
    /// caller can send, so it has no name on the wire.
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(ScopeArg(Option::<Scope>::deserialize(deserializer)?))
    }
}

impl ScopeArg {
    /// THE resolver: the concrete filter a read runs at, from the caller's
    /// `scope` (or nothing), the read's [`ScopeDefault`], and whether that
    /// default moves on a location pin.
    ///
    /// An explicit value always wins and always means what it says, so a pin
    /// never overrides it. The pin is consulted only by
    /// [`ScopeDefault::OwnUnlessPinned`].
    fn resolve(self, default: ScopeDefault, pinned: bool) -> wire::TypeScope {
        let own_only = match (self.0, default) {
            (Some(Scope::All), _) => false,
            (Some(Scope::Own), _) => true,
            (None, ScopeDefault::All) => false,
            (None, ScopeDefault::Own) => true,
            (None, ScopeDefault::OwnUnlessPinned) => !pinned,
        };
        wire::TypeScope::new(own_only)
    }
}

/// What a read answers when the caller states no `scope`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ScopeDefault {
    /// `all`: every mounted member, pin or no pin. A VOCABULARY read.
    All,
    /// `own`: the caller's own editable members, pin or no pin. A
    /// QUESTION-ANSWERING read.
    Own,
    /// `own` when unpinned, `all` once a location pin names what is wanted. An
    /// ACTIONABILITY read.
    OwnUnlessPinned,
}

/// One `scope` argument together with its read's declaration: the absent
/// meaning, and whether a location pin moves it.
///
/// [`Self::scope`] is the one call site of [`ScopeArg::resolve`], so no handler
/// restates the rule; the declaration here is what the scope table below
/// documents, and a test holds the two together.
trait ScopedArgs {
    /// What this read answers when the caller states no `scope`. MUST agree with
    /// the scope table below: `scope_defaults_are_the_table` asserts it for
    /// every request.
    const DEFAULT: ScopeDefault;

    /// The `scope` the caller sent.
    fn scope_arg(&self) -> ScopeArg;

    /// Whether a LOCATION PIN — a named `repo`, or on `diagnostics` /
    /// `diagnostic_counts` a `path` / `path_prefix` — moves this read's default.
    ///
    /// Only [`ScopeDefault::OwnUnlessPinned`] has such an axis, so only those
    /// args structs implement it. A fixed default leaves the `false` default
    /// even when a `repo` is named: the pin narrows WHICH member, which is not
    /// the same thing as moving the scope.
    fn pin_moves_default(&self) -> bool {
        false
    }

    /// The concrete filter this read runs at.
    fn scope(&self) -> wire::TypeScope {
        self.scope_arg()
            .resolve(Self::DEFAULT, self.pin_moves_default())
    }
}

/// One row of the scope table: a read, what its `scope` defaults to, and why.
#[cfg(test)]
#[derive(Debug)]
struct ScopeRow {
    /// The wire verb, as [`Request::verb`] returns it and `WIRE.md` spells it.
    verb: &'static str,
    /// What the read answers when the caller states no `scope`.
    default: ScopeDefault,
    /// The reason, in one sentence.
    reason: &'static str,
}

/// THE scope-default table: every read that carries a `scope` argument, the
/// meaning of its absence, and the reason. Compiled with the tests, which is
/// where its consumers are: `scope_defaults_are_the_table` fails if a read's own
/// declaration ([`ScopedArgs::DEFAULT`]) and this table ever disagree.
///
/// The default follows the KIND of question the read answers, not its shape:
///
/// - ACTIONABILITY (`OwnUnlessPinned`): "what should I act on". A read-only
///   dependency's contribution is noise; a pinned location is the caller
///   already naming what they mean, so answering that with an empty list because
///   of an invisible default is the failure this guards against.
/// - VOCABULARY (`All`): "what exists to author against". You write
///   `book::library`, so hiding peer types by default would make completion,
///   hover, and the type tree look broken.
/// - QUESTION (`Own`): "what does MY knowledge say". A dependency repository's
///   README answering a search is the "searched the wrong base" failure. A
///   `repo` narrows WHICH member to search, not whether to search outside the
///   caller's own content, so it does not move this default.
#[cfg(test)]
const SCOPE_TABLE: &[ScopeRow] = &[
    ScopeRow {
        verb: "diagnostics",
        default: ScopeDefault::OwnUnlessPinned,
        reason: "a WORKLIST, not an inventory: an error inside a read-only dependency is not \
                 actionable, and a pinned repo/file/subtree is the caller naming what they want.",
    },
    ScopeRow {
        verb: "diagnostic_counts",
        default: ScopeDefault::OwnUnlessPinned,
        reason: "the same worklist as `diagnostics`, counted: same default, same pins.",
    },
    ScopeRow {
        verb: "types",
        default: ScopeDefault::All,
        reason: "vocabulary to author AGAINST: `book::library` names a peer type, so hiding peers \
                 would make completion and hover look broken.",
    },
    ScopeRow {
        verb: "type_counts",
        default: ScopeDefault::All,
        reason: "the count dual of `types`, so it spans the same vocabulary.",
    },
    ScopeRow {
        verb: "type_tree",
        default: ScopeDefault::All,
        reason: "the vocabulary as a parent/child forest; a hidden peer is a missing branch.",
    },
    ScopeRow {
        verb: "imports",
        default: ScopeDefault::All,
        reason: "the workspace's import set; `own` drops the builtin fold, so `all` is the \
                 complete picture.",
    },
    ScopeRow {
        verb: "grep",
        default: ScopeDefault::Own,
        reason: "a literal scan answers where the caller's own text says something; a dependency \
                 mounted beside it is not part of the answer. `repo` narrows WHICH member, so it \
                 does not move this default.",
    },
    ScopeRow {
        verb: "search",
        default: ScopeDefault::Own,
        reason: "a search is a question about the caller's own knowledge; a dependency README \
                 answering it is the 'searched the wrong base' failure. `repo` narrows WHICH \
                 member, so it does not move this default.",
    },
    ScopeRow {
        verb: "subtypes",
        default: ScopeDefault::All,
        reason: "the subtype tree of one base: a peer's subtype is still something to author \
                 against.",
    },
    ScopeRow {
        verb: "instance_counts",
        default: ScopeDefault::All,
        reason: "instance SITES per identity across the workspace, the dual of `instances_of`, \
                 which is workspace-wide.",
    },
    ScopeRow {
        verb: "neighborhood",
        default: ScopeDefault::All,
        reason: "`path` is a location pin, so the walk starts where the caller pointed; a \
                 dependency target is reported at the boundary rather than expanded.",
    },
    ScopeRow {
        verb: "files",
        default: ScopeDefault::All,
        reason: "a wikilink into a dependency is legal, so the resolvable target set is what \
                 exists to link against.",
    },
    ScopeRow {
        verb: "top_level_dirs",
        default: ScopeDefault::OwnUnlessPinned,
        reason: "orientation: the directories the caller can act in, and `all` once a `repo` \
                 names the member.",
    },
    ScopeRow {
        verb: "overview",
        default: ScopeDefault::OwnUnlessPinned,
        reason: "orientation is first-person ('where am I'), and `all` once a `repo` names the \
                 member; its echoed `repo` / `scope` let a consumer drill down without \
                 re-deriving the default.",
    },
    ScopeRow {
        verb: "hubs",
        default: ScopeDefault::OwnUnlessPinned,
        reason: "ranked by inbound references, which are other notes to work FROM, so the own \
                 workspace is the answer and a `repo` pin is explicit.",
    },
    ScopeRow {
        verb: "graph_shape",
        default: ScopeDefault::OwnUnlessPinned,
        reason: "the shape of the graph being worked in, `all` once a `repo` names it.",
    },
    ScopeRow {
        verb: "graph_health",
        default: ScopeDefault::OwnUnlessPinned,
        reason: "the same in-scope node set as `graph_shape`, so the same default and pins.",
    },
    ScopeRow {
        verb: "link_graph",
        default: ScopeDefault::OwnUnlessPinned,
        reason: "the graph to LAY OUT beside the content being edited; `all` once a `repo` \
                 names the member.",
    },
    ScopeRow {
        verb: "type_graph",
        default: ScopeDefault::OwnUnlessPinned,
        reason: "the schema drawn beside `link_graph` over the same scope axis, so the same \
                 default and pins.",
    },
];

impl Request {
    /// The wire verb this request names: the `read` tag's value, exactly as
    /// `WIRE.md` spells it in the read catalog (and as `shutdown` in the control
    /// verb section).
    ///
    /// A 52-arm match, written out rather than derived from the variant name, for
    /// two reasons.
    ///
    /// - Totality is the point: the compiler refuses a variant without an arm, so
    ///   there is no way to add a read that the counters cannot name. A derived
    ///   name would compile and would silently disagree with the wire the first
    ///   time a variant and its verb diverged.
    /// - The dispatch in [`dispatch`] destructures its argument, so the verb is
    ///   known only inside each arm. This map is what lets the serve loop time and
    ///   COUNT a request without restructuring that dispatch into 52 call sites —
    ///   the same "one source for the verb list" the adapter's `READ_VERBS` gave
    ///   its tools.
    ///
    /// A test (`verb_tests`) pins all three: every arm's literal is in
    /// [`crate::verb_metrics::VERBS`], `VERBS` equals this document's census, and
    /// neither list carries a duplicate.
    fn verb(&self) -> &'static str {
        match self {
            Request::Diagnostics(_) => "diagnostics",
            Request::DiagnosticCounts(_) => "diagnostic_counts",
            Request::Types(_) => "types",
            Request::TypeCounts(_) => "type_counts",
            Request::TypeTree { .. } => "type_tree",
            Request::Type(_) => "type",
            Request::TypeBatch(_) => "type_batch",
            Request::InstancesOf(_) => "instances_of",
            Request::Imports { .. } => "imports",
            Request::Grep(_) => "grep",
            Request::Search(_) => "search",
            Request::Subtypes { .. } => "subtypes",
            Request::Instances => "instances",
            Request::Candidates(_) => "candidates",
            Request::CandidateCounts(_) => "candidate_counts",
            Request::InstanceCounts(_) => "instance_counts",
            Request::Instance { .. } => "instance",
            Request::ReferencesIn { .. } => "references_in",
            Request::ReferencesOut { .. } => "references_out",
            Request::Pins { .. } => "pins",
            Request::CommitMeta { .. } => "commit_meta",
            Request::FileHistory { .. } => "file_history",
            Request::RecentCommits { .. } => "recent_commits",
            Request::Neighborhood(_) => "neighborhood",
            Request::ResolveTarget { .. } => "resolve_target",
            Request::ResolveBlockId { .. } => "resolve_block_id",
            Request::ResolveAnchor { .. } => "resolve_anchor",
            Request::Anchors { .. } => "anchors",
            Request::BlockIds { .. } => "block_ids",
            Request::Files(_) => "files",
            Request::DirEntries { .. } => "dir_entries",
            Request::Frontmatter { .. } => "frontmatter",
            Request::Content { .. } => "content",
            Request::SemanticTokens { .. } => "semantic_tokens",
            Request::ValidateValue { .. } => "validate_value",
            Request::Members(_) => "members",
            Request::Ignores { .. } => "ignores",
            Request::ResolveMember { .. } => "resolve_member",
            Request::DeviceConfig => "device_config",
            Request::Config(_) => "config",
            Request::TopLevelDirs(_) => "top_level_dirs",
            Request::Overview(_) => "overview",
            Request::TypeClosure(_) => "type_closure",
            Request::Hubs(_) => "hubs",
            Request::GraphShape(_) => "graph_shape",
            Request::RollbackPreview(_) => "rollback_preview",
            Request::GraphHealth(_) => "graph_health",
            Request::LinkGraph(_) => "link_graph",
            Request::TypeGraph(_) => "type_graph",
            Request::PreviewMutation(_) => "preview_mutation",
            Request::Lifecycle => "lifecycle",
            Request::Shutdown => "shutdown",
        }
    }
}

/// The op a `preview_mutation` simulates, tagged by `op`. Mirrors the v1
/// mutation catalog's `write_file` / `edit_file` / `delete_file`, reusing their
/// argument shapes so a consumer previews the exact call it would then mutate
/// with. Deterministic ops only; non-deterministic ones (`bash`, codegen) have
/// no meaningful product.
#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum PreviewMutationArgs {
    WriteFile {
        path: String,
        content: String,
        #[serde(default)]
        stamps: Vec<Stamp>,
        #[serde(default)]
        ensure_mixins: Vec<String>,
        #[serde(default = "default_true")]
        ensure_mixins_strict: bool,
    },
    EditFile {
        path: String,
        old_string: String,
        new_string: String,
        #[serde(default)]
        replace_all: bool,
        #[serde(default)]
        stamps: Vec<Stamp>,
        #[serde(default)]
        ensure_mixins: Vec<String>,
        #[serde(default = "default_true")]
        ensure_mixins_strict: bool,
    },
    DeleteFile {
        path: String,
    },
    /// A file move or rename, the `rename` mutation's `path` / `to`.
    Rename {
        path: String,
        to: String,
        #[serde(default)]
        stamps: Vec<Stamp>,
        #[serde(default)]
        ensure_mixins: Vec<String>,
        #[serde(default = "default_true")]
        ensure_mixins_strict: bool,
    },
    /// A folder move or rename, the `move_dir` mutation's `path` / `to`.
    MoveDir {
        path: String,
        to: String,
    },
    /// A folder delete, the `delete_dir` mutation's `path`.
    DeleteDir {
        path: String,
    },
    /// A type-def rename, the `rename_type` mutation's `old_name` / `new_name`.
    /// No `path`: the def file is derived from the type graph, not named.
    RenameType {
        old_name: String,
        new_name: String,
    },
}

impl PreviewMutationArgs {
    /// Split into the request shape the handler serves: a single-file op (the
    /// target path, the engine-side op, the stamp riders), or a multi-file op,
    /// planned by its own handler.
    fn into_request(self) -> PreviewRequest {
        let (path, op, riders) = match self {
            PreviewMutationArgs::WriteFile {
                path,
                content,
                stamps,
                ensure_mixins,
                ensure_mixins_strict,
            } => (
                path,
                crate::mutate::PreviewOp::Write { content },
                preview_riders(stamps, ensure_mixins, ensure_mixins_strict),
            ),
            PreviewMutationArgs::EditFile {
                path,
                old_string,
                new_string,
                replace_all,
                stamps,
                ensure_mixins,
                ensure_mixins_strict,
            } => (
                path,
                crate::mutate::PreviewOp::Edit {
                    old_string,
                    new_string,
                    replace_all,
                },
                preview_riders(stamps, ensure_mixins, ensure_mixins_strict),
            ),
            PreviewMutationArgs::DeleteFile { path } => (
                path,
                crate::mutate::PreviewOp::Delete,
                crate::engine::PreviewRiders::default(),
            ),
            PreviewMutationArgs::Rename {
                path,
                to,
                stamps,
                ensure_mixins,
                ensure_mixins_strict,
            } => {
                return PreviewRequest::Rename {
                    path,
                    to,
                    riders: preview_riders(stamps, ensure_mixins, ensure_mixins_strict),
                };
            }
            PreviewMutationArgs::MoveDir { path, to } => {
                return PreviewRequest::MoveDir { path, to };
            }
            PreviewMutationArgs::DeleteDir { path } => return PreviewRequest::DeleteDir { path },
            PreviewMutationArgs::RenameType { old_name, new_name } => {
                return PreviewRequest::RenameType { old_name, new_name };
            }
        };
        PreviewRequest::File { path, op, riders }
    }
}

/// A write's wire riders as the preview's engine-side riders.
fn preview_riders(
    stamps: Vec<Stamp>,
    mixins: Vec<String>,
    strict: bool,
) -> crate::engine::PreviewRiders {
    crate::engine::PreviewRiders {
        stamps: to_preview_stamps(stamps),
        mixins,
        strict,
    }
}

/// A `preview_mutation` request, split by how the preview is planned.
enum PreviewRequest {
    File {
        path: String,
        op: crate::mutate::PreviewOp,
        riders: crate::engine::PreviewRiders,
    },
    Rename {
        path: String,
        to: String,
        riders: crate::engine::PreviewRiders,
    },
    MoveDir {
        path: String,
        to: String,
    },
    DeleteDir {
        path: String,
    },
    RenameType {
        old_name: String,
        new_name: String,
    },
}

/// A write stamp list as the preview's engine-side riders.
fn to_preview_stamps(ss: Vec<Stamp>) -> Vec<crate::mutate::PreviewStamp> {
    ss.into_iter()
        .map(|s| crate::mutate::PreviewStamp {
            field: s.field,
            record: s.record,
            match_on: s.match_on,
        })
        .collect()
}

/// A mutation request, tagged by `mutate` — the wire's third verb, per
/// [[spec - mutation channel v1 - a closed primitive catalog through one
/// mediated path]]. The catalog is closed; unknown primitives and unknown args
/// are error frames.
#[derive(Debug, Deserialize)]
#[serde(tag = "mutate", rename_all = "snake_case")]
enum Mutation {
    /// Full-content write, parents created. `expected_hash` is the optional
    /// read-before-write guard.
    WriteFile(WriteFileArgs),
    /// Full-content write whose precondition is the target's ABSENCE, checked
    /// inside the serialized mutation as part of the write itself. An existing
    /// path rejects instead of being overwritten, so two consumers creating one
    /// path produce one success and one conflict rather than a lost update.
    CreateFile(CreateFileArgs),
    /// Exact string replacement; the match is its own precondition.
    EditFile(EditFileArgs),
    /// Remove a file. `expected_hash` is the optional read-before-write guard.
    DeleteFile(DeleteFileArgs),
    /// Several file-level ops in one commit; see [`BatchArgs`]. Additive.
    Batch(BatchArgs),
    /// Engine-assigned `^:` id on the addressable entity at a byte offset;
    /// returns the id and the `[[target^id]]` ref.
    AssignBlockId(AssignBlockIdArgs),
    /// Move a file to a new path, rewriting every inbound reference to follow,
    /// as one saga. Same-repo only; refuses type-def files (that is `rename_type`).
    Rename(RenameArgs),
    /// Move a folder to a new path, every catalogued file under it plus its
    /// directory skeleton, rewriting every path-addressed inbound reference to
    /// follow, as one saga. Same-repo only; refuses a folder holding anything the
    /// engine does not catalogue.
    MoveDir(MoveDirArgs),
    /// Delete a folder, every catalogued file under it plus its directory
    /// skeleton, as one saga. Raw: no referrer is rewritten, the references the
    /// delete strands are reported.
    DeleteDir(DeleteDirArgs),
    /// Extract the inline `^:id` record at a byte offset into its own file,
    /// leaving `[[newFile]]` in its place and rewriting every `[[host^id]]`
    /// referrer to `[[newFile]]`, as one saga.
    Promote(PromoteArgs),
    /// Fold a `[[file]]`-referenced file into a host referrer as a `^:id`
    /// record, rewrite the other referrers to it, and delete the file, as one
    /// saga. The dual of promote.
    Inline(InlineArgs),
    /// Rename an inline `^:id` record's block-id in its host, rewriting every
    /// `[[host^id]]` referrer to the new id, as one saga. The block-id sibling of
    /// `rename` (which moves a file); rides the same rewrite core.
    RenameBlockId(RenameBlockIdArgs),
    /// Rename a type-def. Moves its file (the name derives from the filename)
    /// and cascades both reference surfaces atomically: the type-name
    /// references in the owning repo (`type:` claims, `sealed:` branches, slot
    /// shapes, qualified keys `field{name}`, body `use:`, meta `type:`), and the wikilinks
    /// to the def file across the mounted set. The type-vocabulary sibling of
    /// `rename`; what `rename` refuses for a type-def file.
    RenameType(RenameTypeArgs),
    /// Replace a member's `.auignore` scope rules — a CONFIG-sort mutation, not a
    /// graph mutation. It edits which content enters the graph, so it MAY orphan
    /// references (they become advisory diagnostics); the strict no-dangling
    /// contract is deliberately relaxed for it. The one sanctioned write under
    /// `.arsumbris/`, and only to `<root>/.arsumbris/.auignore`. Validates the
    /// patterns up front (a malformed pattern rejects, nothing written), commits
    /// per mutation, and re-scopes. See
    /// [[spec - scope management surface - an ignores read and a set_ignores config mutation]].
    SetIgnores(SetIgnoresArgs),
    /// Register (write or update) one `{ name, remote, path }` entry in the
    /// per-user registry (`~/.arsumbris/au-engine/config/repos.yaml`) — a CONFIG-sort
    /// mutation over a DEVICE-GLOBAL file, so NO git commit (the file is outside
    /// every repo), unlike `set_ignores`. The consumer-driven bootstrap for an
    /// unmounted peer: a folder picker supplies the path, `register` records it,
    /// the rebuild mounts it. Refuses (a reject frame) when the name's identity
    /// disagrees (`dependency-identity-conflict`). See
    /// [[spec - cross-repo resolution - in-repo identity and deps over a per-user repo registry]].
    Register(RegisterArgs),
    /// Write one CONSUMER config file under the scoped-config channel,
    /// `<scope>/<consumer>/config/<file>` — a CONFIG-sort mutation, the write dual
    /// of the `config` read. A sanctioned bypass of the `.arsumbris/` write-guard,
    /// like `set_ignores`: path-safety (and the reserved `au-engine` owner segment)
    /// gate the two caller-controlled segments, the path is constructed directly.
    /// Repo scope commits per mutation through the saga; machine scope writes a
    /// device-global file with NO commit. The write injects `type: <declared>` so
    /// the file self-describes, and an `expected_hash` is a compare-and-set. See
    /// [[spec - scoped config channel - a config read and set_config mutation over scope, consumer, file, type]].
    SetConfig(SetConfigArgs),
    /// Patch a nested typed record's fields in place, keyed by the `instances_of`
    /// `field_path` locator. `type` re-types the record; other keys replace a
    /// scalar field or insert an absent one. A byte-splice, so comments survive.
    /// See [[spec - nested record edits - patch a record and append to a sequence by byte-splice, comments preserved]].
    EditRecord(EditRecordArgs),
    /// Append one element to a sequence field, keyed by the `field_path` locator.
    /// A byte-splice after the last element (or a seed into an empty `[]`), so
    /// existing elements and their comments stay byte-identical.
    AppendRecord(AppendRecordArgs),
    /// Add a `{ name, remote?, ref? }` entry to a repo's `deps:` — a
    /// COMPOSITION-sort mutation authoring `<member>/.arsumbris/repo.yaml`. A
    /// guard-bypass like `set_config`, but the file is a first-class node, so the
    /// change re-resolves the mounted set and rebuilds. Does NOT fetch: the new
    /// peer stays `peer-unmounted` until `resolve`. A duplicate name rejects. See
    /// [[spec - composition authorship - governed list verbs author workspace members and repo deps, rebuilding on change]].
    AddDep(AddDepArgs),
    /// Remove a dep entry from a repo's `deps:` by name — the composition-sort
    /// dual of `add_dep`. An absent name rejects.
    RemoveDep(RemoveDepArgs),
    /// Add a member to a workspace's `edit:` or `discover:` — a COMPOSITION-sort
    /// mutation authoring `<member>/.arsumbris/workspace.yaml`, created
    /// self-complete when absent. A member already in either list rejects. See
    /// [[spec - composition authorship - governed list verbs author workspace members and repo deps, rebuilding on change]].
    AddWorkspaceMember(AddWorkspaceMemberArgs),
    /// Remove a member from every one of `edit:` / `discover:` holding it.
    RemoveWorkspaceMember(RemoveWorkspaceMemberArgs),
    /// Move a member between `edit:` and `discover:` in one commit.
    SetWorkspaceMemberRole(SetWorkspaceMemberRoleArgs),
    /// Switch a declared member off or on through the workspace's `disabled:`
    /// overlay, one commit. Disabling a name not declared in `edit:` /
    /// `discover:`, or the containing repo, rejects.
    SetWorkspaceMemberDisabled(SetWorkspaceMemberDisabledArgs),
}

/// One `field_path` segment on the wire: a field name (string) or a list index
/// (number). Untagged, so `["phases", 0]` deserializes directly. The
/// deserialize dual of [`wire::PathSeg`]; index is tried first so a number never
/// reads as the string form.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum PathSegArg {
    Index(usize),
    Field(String),
}

impl From<PathSegArg> for tb_core::PathSegment {
    fn from(seg: PathSegArg) -> Self {
        match seg {
            PathSegArg::Index(i) => tb_core::PathSegment::Index(i),
            PathSegArg::Field(f) => tb_core::PathSegment::Field(f),
        }
    }
}

/// The validation stance for a nested-record mutation. `advise` (default) lands
/// the write and surfaces diagnostics; `reject` refuses when the change raises
/// the file's validation-error count, writing nothing.
#[derive(Debug, Deserialize, Default, PartialEq, Eq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum OnInvalid {
    #[default]
    Advise,
    Reject,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EditRecordArgs {
    path: String,
    /// The `instances_of` locator to the record; empty addresses the file instance.
    field_path: Vec<PathSegArg>,
    /// A shallow map of field name to value. `type` re-types; `^` is rejected.
    patch: serde_json::Map<String, serde_json::Value>,
    expected_hash: Option<String>,
    #[serde(default)]
    on_invalid: OnInvalid,
    #[serde(default)]
    stamps: Vec<Stamp>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AppendRecordArgs {
    path: String,
    /// The `instances_of` locator to the SEQUENCE-valued field.
    field_path: Vec<PathSegArg>,
    /// The element to append, a record or a scalar.
    value: serde_json::Value,
    expected_hash: Option<String>,
    #[serde(default)]
    on_invalid: OnInvalid,
    #[serde(default)]
    stamps: Vec<Stamp>,
}

/// One entry of a write verb's `stamps` list: idempotently ensure a caller-supplied
/// record into a named frontmatter list-field of the file the write touches,
/// folded into the write's own commit. A verb carries a LIST, applied in order,
/// each entry the same idempotent ensure. The engine treats it as OPAQUE. See
/// [[spec - stamp injection - a write rider idempotently ensures a frontmatter record folded into the write's commit]].
/// Every stamp targets the verb's primary file only; the spec's `path` (another
/// file in the write set) is deferred.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Stamp {
    /// The frontmatter sequence key the record is ensured in.
    field: String,
    /// The record to ensure, an arbitrary structured value.
    record: serde_json::Value,
    /// Optional dedup predicate: path-within-element to value pairs. When some
    /// existing element carries every pair the stamp is a no-op; absent, the
    /// record is always appended. v1 keys are top-level element slots (a field,
    /// or `type` for the element's claim).
    match_on: Option<serde_json::Map<String, serde_json::Value>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WriteFileArgs {
    path: String,
    content: String,
    expected_hash: Option<String>,
    #[serde(default)]
    stamps: Vec<Stamp>,
    /// Mixins to ensure on the written file's `type:` claim, folded into this
    /// write's commit ([[spec - ensure-mixin write directive - a governed write ensures a type-claim mixin idempotently, folded into the write's own commit]]).
    #[serde(default)]
    ensure_mixins: Vec<String>,
    #[serde(default = "default_true")]
    ensure_mixins_strict: bool,
    /// Caller attribution trailers folded into this write's commit.
    #[serde(default)]
    attribution: Vec<TrailerInput>,
    /// ADR-0002: refuse before writing when the state this write would produce carries
    /// an ERROR-severity diagnostic. Default `false`, which is the advisory contract
    /// every draft relies on.
    #[serde(default)]
    require_clean: bool,
}

/// Args for `create_file`, the absence-preconditioned sibling of
/// [`WriteFileArgs`]: the same full content and the same riders, and no
/// `expected_hash` — there is no prior text to compare against, and the absence
/// is the engine's precondition, not the caller's. An `expected_hash` sent here
/// is an unknown field, refused rather than silently ignored.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateFileArgs {
    path: String,
    content: String,
    #[serde(default)]
    stamps: Vec<Stamp>,
    #[serde(default)]
    ensure_mixins: Vec<String>,
    #[serde(default = "default_true")]
    ensure_mixins_strict: bool,
    /// Caller attribution trailers folded into this create's commit.
    #[serde(default)]
    attribution: Vec<TrailerInput>,
    /// ADR-0002: refuse before creating when the state this create would produce
    /// carries an ERROR-severity diagnostic. Default `false`.
    #[serde(default)]
    require_clean: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EditFileArgs {
    path: String,
    old_string: String,
    new_string: String,
    #[serde(default)]
    replace_all: bool,
    #[serde(default)]
    stamps: Vec<Stamp>,
    #[serde(default)]
    ensure_mixins: Vec<String>,
    #[serde(default = "default_true")]
    ensure_mixins_strict: bool,
    /// Caller attribution trailers folded into this edit's commit.
    #[serde(default)]
    attribution: Vec<TrailerInput>,
    /// ADR-0002: refuse before writing when the state this edit would produce
    /// carries an ERROR-severity diagnostic. Default `false`.
    #[serde(default)]
    require_clean: bool,
}

/// One operation inside a batch. The tag names the same file-level verbs the
/// single mutations serve, so a batch is a composition of writes, edits, and
/// deletes, never a new primitive.
// Each tag is pinned to the single verb it composes, so a variant rename cannot
// silently change the wire.
#[derive(Debug, Deserialize)]
#[serde(tag = "op", deny_unknown_fields)]
enum BatchOp {
    #[serde(rename = "write_file")]
    Write {
        path: String,
        content: String,
        /// Read-before-write guard, per op.
        #[serde(default)]
        expected_hash: Option<String>,
    },
    #[serde(rename = "edit_file")]
    Edit {
        path: String,
        old_string: String,
        new_string: String,
        /// The exact-match guard is the file's own: absent hash, as in the
        /// single `edit_file`.
        #[serde(default)]
        replace_all: bool,
    },
    #[serde(rename = "delete_file")]
    Delete {
        path: String,
        #[serde(default)]
        expected_hash: Option<String>,
    },
}

/// Args for the `batch` mutation: several file-level ops committed once.
///
/// The point is atomicity, not speed: N single mutations are N chances to lose a
/// reply and land half a change. A batch applies every op or none, commits each
/// touched member once, and compensates the whole set on any rejection.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BatchArgs {
    ops: Vec<BatchOp>,
    /// ADR-0002 for a whole batch: refuse before writing when the state the set would
    /// produce carries an ERROR-severity diagnostic, naming the op that would introduce it.
    /// Default `false`, the advisory contract every draft relies on.
    #[serde(default)]
    require_clean: bool,
}

/// The largest batch a caller may send. Beyond this the caller wants a script,
/// not one atomic commit, and a single huge saga's intent marker stops being a
/// readable record of one change.
const MAX_BATCH_OPS: usize = 50;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeleteFileArgs {
    path: String,
    /// Optional read-before-write guard: the content hash the caller last read.
    /// A mismatch rejects without deleting; absent deletes regardless.
    expected_hash: Option<String>,
    /// Caller attribution trailers folded into this delete's commit.
    #[serde(default)]
    attribution: Vec<TrailerInput>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AssignBlockIdArgs {
    path: String,
    /// A byte offset inside the file, e.g. from a span the wire served.
    /// The engine resolves the enclosing addressable entity.
    at: usize,
    #[serde(default)]
    stamps: Vec<Stamp>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RenameTypeArgs {
    old_name: String,
    new_name: String,
    #[serde(default)]
    stamps: Vec<Stamp>,
    /// The members the caller's grant covers, checked against the cascade this
    /// mutation plans, under the write lock. Empty means unrestricted: a direct
    /// caller with no policy, which is the pre-existing contract.
    #[serde(default)]
    allowed_members: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RegisterArgs {
    /// The repo name to register. Must equal the declared `name` in the target's
    /// `.arsumbris/repo.yaml` (checked before writing).
    name: String,
    /// The local path where the repo sits on this machine, absolute.
    path: String,
    /// The repo's git remote, an optional recorded convenience. A disagreement
    /// with an existing entry's remote is a `dependency-identity-conflict`.
    remote: Option<String>,
}

/// The scope of a scoped-config `(scope, consumer, file, type)` request.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ConfigScope {
    /// `~/.arsumbris/<consumer>/config/<file>`, the personal device tier. The
    /// type resolves in the served entry's graph.
    Machine,
    /// `<repo>/.arsumbris/<consumer>/config/<file>`, the committed repo tier. The
    /// type resolves in the owning member's graph.
    Repo,
}

/// The `config` read arguments. `type` is stamped onto the file's value (not read
/// from a `type:` key) and field-shape validated in the scope's graph.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigArgs {
    scope: ConfigScope,
    /// The owner segment, a namespacing convention (`host-app`, `agent-tools`). Path-safe
    /// and not the engine's own `au-engine`.
    consumer: String,
    /// The config filename under `<consumer>/config/`.
    file: String,
    #[serde(rename = "type")]
    type_name: String,
    /// Repo scope: the member root whose `.arsumbris/` holds the file, a declared
    /// member root (as `members` surfaces). Absent defaults to the served entry.
    /// Ignored at machine scope.
    #[serde(default)]
    root: Option<String>,
}

/// The `set_config` mutation arguments. Exactly one of `content` (the whole file)
/// or `edit` (one keyed record splice) is given. `type` is the DECLARED floor:
/// injected into the content when the file does not already self-describe with its
/// own `type:`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SetConfigArgs {
    scope: ConfigScope,
    consumer: String,
    file: String,
    #[serde(rename = "type")]
    type_name: String,
    /// Repo scope: the member root whose `.arsumbris/` holds the file (a declared
    /// member root). Absent defaults to the served entry. Ignored at machine scope.
    #[serde(default)]
    root: Option<String>,
    /// The whole file content to write. Exactly one of `content` / `edit`.
    #[serde(default)]
    content: Option<String>,
    /// A single keyed-record splice over the existing file (comments and siblings
    /// preserved). Exactly one of `content` / `edit`.
    #[serde(default)]
    edit: Option<ConfigEditArgs>,
    /// Optional compare-and-set against the current file's content hash. A
    /// mismatch (or an absent file) rejects, nothing written.
    #[serde(default)]
    expected_hash: Option<String>,
}

/// The `set_config` `edit` sub-shape: patch one keyed record in place, reusing the
/// byte-splice core `edit_record` uses. There is no `on_invalid` gate: a config
/// write is advisory (never refused on a field-shape verdict), and the splice's
/// own structural self-check still rejects a change that would break the YAML, so
/// the file is never left broken.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigEditArgs {
    /// The locator to the record; empty addresses the file's top-level record.
    field_path: Vec<PathSegArg>,
    /// A shallow map of field name to scalar value. `type` re-types a record that
    /// has an explicit claim; `^` is rejected.
    patch: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SetIgnoresArgs {
    /// The member root whose `.auignore` to write. Must name a declared member
    /// root exactly (the `ignores` read surfaces it).
    root: String,
    /// The full replacement pattern list, one per line. An empty list removes the
    /// file, reverting the member to the default excludes.
    patterns: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AddDepArgs {
    /// The member root whose `repo.yaml` to author. Absent means the served entry.
    /// Must name a declared, editable member root; a consumed member rejects.
    #[serde(default)]
    root: Option<String>,
    /// The dependency's name, the `deps[].name` and the duplicate-detection key.
    name: String,
    /// The dependency's git remote, optional.
    #[serde(default)]
    remote: Option<String>,
    /// The dependency's ref (branch / tag), optional.
    #[serde(default, rename = "ref")]
    ref_field: Option<String>,
    /// Optional compare-and-set against the current `repo.yaml` content hash.
    #[serde(default)]
    expected_hash: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RemoveDepArgs {
    /// The member root whose `repo.yaml` to author. Absent means the served entry.
    #[serde(default)]
    root: Option<String>,
    /// The name of the dep to remove; an absent name rejects.
    name: String,
    /// Optional compare-and-set against the current `repo.yaml` content hash.
    #[serde(default)]
    expected_hash: Option<String>,
}

/// A workspace member role on the wire, `edit` or `discover`.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum WorkspaceRole {
    Edit,
    Discover,
}

impl WorkspaceRole {
    fn other(self) -> WorkspaceRole {
        match self {
            WorkspaceRole::Edit => WorkspaceRole::Discover,
            WorkspaceRole::Discover => WorkspaceRole::Edit,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AddWorkspaceMemberArgs {
    /// The member root whose `workspace.yaml` to author. Absent means the served
    /// entry. The file is created (seeded self-complete) when absent.
    #[serde(default)]
    root: Option<String>,
    /// The member name to add to the workspace composition.
    name: String,
    /// Which list to add it to, `edit` or `discover`.
    role: WorkspaceRole,
    /// Optional compare-and-set against the current `workspace.yaml` content hash.
    #[serde(default)]
    expected_hash: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RemoveWorkspaceMemberArgs {
    #[serde(default)]
    root: Option<String>,
    /// The member name to remove from every list holding it; absent rejects.
    name: String,
    #[serde(default)]
    expected_hash: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SetWorkspaceMemberRoleArgs {
    #[serde(default)]
    root: Option<String>,
    /// The member to re-role; must already be a member, absent rejects.
    name: String,
    /// The target role to move it to, `edit` or `discover`.
    role: WorkspaceRole,
    #[serde(default)]
    expected_hash: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SetWorkspaceMemberDisabledArgs {
    #[serde(default)]
    root: Option<String>,
    /// The member to switch off or on.
    name: String,
    /// `true` adds it to `disabled:`, `false` removes it.
    disabled: bool,
    #[serde(default)]
    expected_hash: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RenameArgs {
    path: String,
    /// The new path. v1 must resolve to the same repo as `path`.
    to: String,
    /// ADR-0003 for a cross-member cascade: the members the caller's policy grants. Sent only
    /// when the policy has an allowlist; every member the saga would write must be in it.
    #[serde(default)]
    allowed_members: Vec<String>,
    /// Optional stamps, folded into the RENAMED file (`to`) after the move, so a
    /// `{ type: file-change.rename, from }` record shares the rename's commit.
    #[serde(default)]
    stamps: Vec<Stamp>,
    /// Mixins to ensure on the renamed file (`to`), folded into the rename's commit.
    #[serde(default)]
    ensure_mixins: Vec<String>,
    #[serde(default = "default_true")]
    ensure_mixins_strict: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MoveDirArgs {
    /// The folder to move.
    path: String,
    /// The folder's new path, free, in the same repo as `path`.
    to: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeleteDirArgs {
    /// The folder to delete.
    path: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PromoteArgs {
    path: String,
    /// A byte offset inside the inline record to extract; the engine resolves the
    /// innermost enclosing record, mirroring `assign_block_id`'s locator. The
    /// locator for a record with no `^:` id. Exactly one of `at` / `block_id`.
    at: Option<usize>,
    /// The record's `^:` id; the stable locator for a record that already carries
    /// one (the referenced case always does). Exactly one of `at` / `block_id`.
    block_id: Option<String>,
    /// The new file's path. v1 must resolve to the same repo as `path`.
    to: String,
    /// Optional stamps, folded into the newly-extracted file (`to`).
    #[serde(default)]
    stamps: Vec<Stamp>,
    /// ADR-0003: the members the caller's policy grants. Sent only when the policy has an
    /// allowlist; every member the saga writes must be in it.
    #[serde(default)]
    allowed_members: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct InlineArgs {
    /// The file to fold in and delete.
    path: String,
    /// The host referrer that will hold the inlined `^:id` record. Must resolve
    /// to the same repo as `path`, and must reference `path`.
    into: String,
    /// A byte offset in `into` landing on the `[[path]]` reference to host the
    /// record. Required when `into` references `path` more than once; the sole
    /// reference is used when omitted.
    at: Option<usize>,
    /// Optional stamps, folded into the host file (`into`) the content is pulled into.
    #[serde(default)]
    stamps: Vec<Stamp>,
    /// ADR-0003: the members the caller's policy grants. Sent only when the policy has an
    /// allowlist; every member the saga writes must be in it.
    #[serde(default)]
    allowed_members: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RenameBlockIdArgs {
    /// The host file carrying the inline `^:id` record.
    path: String,
    /// The record's current `^:` block-id.
    block_id: String,
    /// The new block-id. Must match the block-id grammar and be free in `path`.
    to_block_id: String,
    /// Optional stamps, folded into the host file (`path`).
    #[serde(default)]
    stamps: Vec<Stamp>,
    /// ADR-0003: the members the caller's policy grants. Sent only when the policy has an
    /// allowlist; every member the saga writes must be in it.
    #[serde(default)]
    allowed_members: Vec<String>,
}

/// A subscription request, tagged by `subscribe`, symmetric with [`Request`].
///
/// The argless variants and the parametric `diagnostics` channel together
/// are the closed v1 catalog. The channel name on the wire follows the catalog,
/// not the Rust identifier, see [`Subscription::channel`].
#[derive(Debug, Deserialize)]
#[serde(tag = "subscribe", rename_all = "snake_case")]
enum Subscription {
    /// The ref's Deriving-to-Ready lifecycle. Renamed with the read.
    Lifecycle,
    /// The type-def introspection stream. Matches the `types` read.
    Types,
    /// The set of catalogued files.
    Files,
    /// Every rebuild.
    Changes,
    /// Diagnostics: whole-knowledge-base when no filter is given, scoped by
    /// [`DiagnosticsFilterArgs`] otherwise. Multiplexed per connection.
    Diagnostics(DiagnosticsFilterArgs),
    /// The whole-graph node+edge payload as the initial value, then a node/edge
    /// DELTA per rebuild, so an interactive layout patches in place rather than
    /// re-shipping the graph. Scoped by [`LinkGraphArgs`] (`repo` / `scope`),
    /// the same args as the `link_graph` read. See [[spec - whole-graph reads -
    /// a scalar shape summary and a full node-edge payload folded from the
    /// backlink index]].
    LinkGraph(LinkGraphArgs),
    /// The type graph as a node+edge payload as the initial value, then a node/
    /// edge DELTA per rebuild, so a layout patches in place. Scoped by
    /// [`TypeGraphArgs`] (`repo` / `scope` / `edges`), the same args as the
    /// `type_graph` read. The type-side sibling of the `link_graph`
    /// subscription. See [[spec - whole-graph reads - a scalar shape summary and
    /// a full node-edge payload folded from the backlink index]].
    TypeGraph(TypeGraphArgs),
    /// The seed page of `recent_commits` as the initial value, then a
    /// `commits-appeared` change event carrying each newly-appeared commit. An
    /// APPEND-ONLY stream over a reflog watcher (off the version signal), the
    /// live half of the git ACTIVITY view. Scoped by the same `members` /
    /// `limit` / `since` as the read. See [[spec - recent-commits activity stream
    /// - a member-aware bounded commit stream with a reflog-watched append-only
    ///   subscription]].
    RecentCommits(RecentCommitsSubArgs),
}

impl Subscription {
    /// The channel name as it appears on the wire, echoed in the ack.
    fn channel(&self) -> &'static str {
        match self {
            Subscription::Lifecycle => "lifecycle",
            Subscription::Types => "types",
            Subscription::Files => "files",
            Subscription::Changes => "changes",
            Subscription::Diagnostics(_) => "diagnostics",
            Subscription::LinkGraph(_) => "link_graph",
            Subscription::TypeGraph(_) => "type_graph",
            Subscription::RecentCommits(_) => "recent_commits",
        }
    }
}

/// Args for the `overview` read.
///
/// `scope` defaults to **`own`**, deliberately unlike the four ENUMERATION
/// reads that default to `all`. Orientation is inherently first-person: the
/// question is "where am I", not "what is loaded". The mounted set is the
/// transitive dependency closure, so a workspace-wide default would bury the
/// user's own content under infrastructure they did not write, and the default
/// is the decision — an arg nobody passes changes nothing.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct OverviewArgs {
    /// Absent spans every mounted member; present selects that one.
    #[serde(default)]
    repo: Option<String>,
    /// The `scope` argument; absent takes this read's default, stated in the
    /// scope table beside [`Request`].
    #[serde(default)]
    scope: ScopeArg,
}

// The ONE deliberate gap the defaults leave: `overview.type_counts` is
// own-scoped while `type_counts({})` is all-scoped, because `overview` is the
// first-person map. `overview` therefore echoes its resolved `repo` / `scope`,
// so a consumer drilling down passes them verbatim instead of re-deriving a
// default it cannot see. Every other default is the read's own row in the scope
// table beside [`Request`], and `repo: "base", scope: "own"` legitimately
// yields nothing: the caller stated both filters.

/// Args for the `members` read. Optional `repo` narrows to one member, so
/// `overview({repo}).members` is reproducible by one `members` call — the mirror
/// property, closed for this field. `members` honors `repo` but NOT `scope`: it
/// is the TOPOLOGY field, and `members.editable` is how a consumer sees which
/// members `own` selected.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct MembersArgs {
    #[serde(default)]
    repo: Option<String>,
}

/// One commit in a `commit_meta` request: the commit-ish, and optionally the
/// member (`repo`) whose store to resolve it in. `repo` absent defaults to the
/// entry repo, so a single-repo consumer omits it. Per-commit, not per-batch,
/// because a folded pin set spans members.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CommitRef {
    commit: String,
    #[serde(default)]
    repo: Option<String>,
}

/// One caller-supplied attribution trailer on a mutation, `{ key, value }`. The
/// engine folds it into the commit verbatim and never interprets it, so a
/// `session` / `span` stays a caller concept. Validated against the reserved
/// engine keys before it is written.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TrailerInput {
    key: String,
    value: String,
}

/// Args for the `top_level_dirs` read. A dedicated strict struct, not an inline
/// enum variant, so a typo'd arg is an `error` frame rather than a silently
/// different-scoped answer — a scope typo must not quietly change the result.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct TopLevelDirsArgs {
    #[serde(default)]
    repo: Option<String>,
    /// The `scope` argument; absent takes this read's default, stated in the
    /// scope table beside [`Request`].
    #[serde(default)]
    scope: ScopeArg,
}

/// Args for the `instances_of` read. A dedicated strict struct so an unknown
/// arg — a mistyped `body` / `instance` flag especially — is an `error` frame,
/// not a silently-ignored key that answers with the fact absent.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct InstancesOfArgs {
    #[serde(rename = "type")]
    type_name: String,
    #[serde(default)]
    origins: Option<Vec<wire::Origin>>,
    /// Splice each match's resolved view (the `instance` read's payload) onto
    /// its record, keyed by the match's FILE. Default false is the base record.
    /// Named for the fact it attaches, NOT `resolve`: `resolve` means the
    /// boundary-effect walk on `ignores`, and a consumer's own `resolve?` toggle
    /// misled them, so the value-layer fact is `instance`.
    #[serde(default)]
    instance: bool,
    /// Splice each match's markdown BODY (the prose after frontmatter) onto its
    /// record, keyed by the match's FILE. Default false. The one fact that reads
    /// disk, so it splices off the executor.
    #[serde(default)]
    body: bool,
}

/// Args for the `type_closure` read.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TypeClosureArgs {
    /// The type name, bare (`foo`, conflating across mounted repos) or
    /// qualified (`foo::repo`, that one identity).
    name: String,
    /// Scope to one repo's identity. Absent = any-mounted, the multi-fit
    /// answer. A `::repo` in `name` says the same thing and wins.
    #[serde(default)]
    repo: Option<String>,
}

/// Args for the `hubs` read: the same `repo` / `scope` pair as `overview`,
/// plus paging.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct HubsArgs {
    #[serde(default)]
    repo: Option<String>,
    /// The `scope` argument; absent takes this read's default, stated in the
    /// scope table beside [`Request`].
    #[serde(default)]
    scope: ScopeArg,
    /// Page size. Absent is the engine's `HUB_LIMIT`, the same bound
    /// `overview.hubs` carries, so the field mirrors an argless call.
    #[serde(default)]
    limit: Option<usize>,
    /// Page start, absent is 0.
    #[serde(default)]
    offset: Option<usize>,
}

/// Args for the `graph_shape` read: the same `repo` / `scope` pair as
/// `overview`, plus `orphan_paths`. The scalar whole-graph summary always
/// reports orphan COUNTS; `orphan_paths` opts into the unbounded path list, off
/// by default so a fragmented corpus's thousands of orphans stay behind a flag.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct GraphShapeArgs {
    #[serde(default)]
    repo: Option<String>,
    /// The `scope` argument; absent takes this read's default, stated in the
    /// scope table beside [`Request`].
    #[serde(default)]
    scope: ScopeArg,
    /// Include the orphan file paths, each tagged `no_inbound` or `isolated`.
    /// Default false is counts only.
    #[serde(default)]
    orphan_paths: bool,
}

/// Args for the `link_graph` read: the same `repo` / `scope` pair as
/// `graph_shape`. Richer server-side filters (edge kinds, type filters, path
/// prefix) are a deferred follow-on, so the strict struct rejects them loudly
/// today rather than answering a differently-filtered graph.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct LinkGraphArgs {
    #[serde(default)]
    repo: Option<String>,
    /// The `scope` argument; absent takes this read's default, stated in the
    /// scope table beside [`Request`].
    #[serde(default)]
    scope: ScopeArg,
}

/// Args for the `type_graph` read: the same `repo` / `scope` pair as
/// `link_graph`, plus `edges`, the edge-class selection. Absent `edges` is the
/// type-to-type backbone (`subtype` + `field-type`); `instance-of` and `meta`
/// are opt-in. An unknown edge class fails deserialization (an error frame).
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct TypeGraphArgs {
    #[serde(default)]
    repo: Option<String>,
    /// The `scope` argument; absent takes this read's default, stated in the
    /// scope table beside [`Request`].
    #[serde(default)]
    scope: ScopeArg,
    /// The edge classes to include; absent is `subtype` + `field-type`.
    #[serde(default)]
    edges: Option<Vec<EdgeClass>>,
}

/// The walk direction wire value. An unknown value fails deserialization, so a
/// typo is a malformed-read error frame, never a silent default.
#[derive(Debug, Default, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum WireDirection {
    #[default]
    Out,
    In,
    Both,
}

impl From<WireDirection> for crate::neighborhood::Direction {
    fn from(d: WireDirection) -> Self {
        match d {
            WireDirection::Out => crate::neighborhood::Direction::Out,
            WireDirection::In => crate::neighborhood::Direction::In,
            WireDirection::Both => crate::neighborhood::Direction::Both,
        }
    }
}

/// Args for the `neighborhood` read. `path` is required; the rest default. An
/// unknown arg is a malformed-read error frame (`deny_unknown_fields`), and an
/// unknown `direction` value likewise. The `kinds`-required-past-depth-1 rule is
/// cross-field, so it is checked in the handler, not here.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct NeighborhoodArgs {
    /// The seed file, always a file node.
    path: String,
    /// `out` (default) / `in` / `both`.
    #[serde(default)]
    direction: WireDirection,
    /// Maximum hop count; default 1 (the seed plus its direct targets).
    #[serde(default = "default_depth")]
    depth: usize,
    /// The edge-kind include set, any of `navigational` / `contributing` /
    /// `field`. Absent means all kinds at depth 1; REQUIRED past depth 1.
    #[serde(default)]
    kinds: Option<Vec<String>>,
    /// The `scope` argument; absent takes this read's default (`all`, because a
    /// `path` is already a location pin). `own` prunes at the repo boundary: a
    /// dependency target is reported but not expanded. See the scope table
    /// beside [`Request`].
    #[serde(default)]
    scope: ScopeArg,
    /// The node cap; absent is the engine's [`NEIGHBORHOOD_MAX_NODES`]. The seed
    /// counts, so `max_nodes` 1 returns the seed alone.
    #[serde(default)]
    max_nodes: Option<usize>,
    /// Splice each node's `content`: a file node's whole-file
    /// `{ text, hash, commit }` from the working tree, a block node's span slice
    /// (`{ text, hash: null, commit: null }`, a block is not a git object).
    #[serde(default)]
    content: bool,
    /// Splice each FILE node's markdown `body` (the prose after frontmatter).
    #[serde(default)]
    body: bool,
    /// Splice each FILE node's resolved `instance` view; null for a plain note.
    #[serde(default)]
    instance: bool,
}

fn default_depth() -> usize {
    1
}

/// Args for the `type_tree` read. Accepts only `scope`, which is absent by
/// default: unlike the dedicated strict structs below, this keeps the leniency
/// its inline variant had, so an unknown key is still ignored.
#[derive(Debug, Default, Deserialize)]
struct TypeTreeArgs {
    #[serde(default)]
    scope: ScopeArg,
}

/// Args for the `imports` read: `scope`, absent by default.
#[derive(Debug, Default, Deserialize)]
struct ImportsArgs {
    #[serde(default)]
    scope: ScopeArg,
}

/// Args for the `subtypes` read: `base`, bare (`foo`) or `::repo`-qualified
/// (`foo::repo`), plus `scope`, absent by default.
#[derive(Debug, Deserialize)]
struct SubtypesArgs {
    base: String,
    #[serde(default)]
    scope: ScopeArg,
}

/// Args for the `types` read. All optional. Unknown args are rejected loudly
/// rather than silently ignored, matching the diagnostics filters, per the
/// silent-drops-are-worse-than-rejection principle.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct TypesArgs {
    /// Absent = the workspace-wide owner-deduped read. Present = scope to one
    /// member's whole graph (its own defs). An unknown repo → null.
    #[serde(default)]
    repo: Option<String>,
    /// Project each entry to the lightweight summary (`repo` / `name` / `hash`
    /// / `parents` / `sealed` / `doc`) instead of the full def.
    /// Default false = full detail. Detail-on-demand is the single `type` read.
    #[serde(default)]
    summary: bool,
    /// The `scope` argument; absent takes this read's default, stated in the
    /// scope table beside [`Request`].
    #[serde(default)]
    scope: ScopeArg,
    /// Page size: at most this many entries after `offset`, over the name-sorted
    /// set. Absent returns the whole set.
    limit: Option<usize>,
    /// Page start: skip this many entries. Absent is 0.
    offset: Option<usize>,
}

/// Args for the `files` read. All optional, unknown args rejected loudly.
///
/// `scope` defaults to `all`, NOT the `own` an actionability read takes: a
/// wikilink into a dependency is legal (only a TYPE crossing gates on a
/// declared dep), so the resolvable target set is what exists to link against.
/// Defaulting to `own` would hide targets the validator accepts.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FilesArgs {
    /// Absent = every mounted member. Present = only this member's files.
    #[serde(default)]
    repo: Option<String>,
    /// The `scope` argument; absent takes this read's default (`all`), stated in
    /// the scope table beside [`Request`].
    #[serde(default)]
    scope: ScopeArg,
    /// Page size: at most this many entries after `offset`, over the
    /// path-sorted set. Absent returns the whole catalogue.
    limit: Option<usize>,
    /// Page start: skip this many entries. Absent is 0.
    offset: Option<usize>,
}

/// Args for the `rollback_preview` read: what reverting one commit would touch.
///
/// The engine already records what each mutation committed; this read answers the
/// question a caller asks BEFORE undoing anything: which paths the revert would
/// write, whether each still holds the commit's own bytes, and whether a later
/// commit has rewritten it. It never writes.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RollbackPreviewArgs {
    /// The member whose working tree holds the commit.
    repo: String,
    /// The commit to preview reverting.
    commit: String,
}

/// Args for the `graph_health` read: the graph's problems in one answer.
///
/// `graph_shape` counts and `diagnostics` reports, but an agent asking "what is
/// wrong with this graph" would have to join them by hand: cycles are in neither,
/// dead ends are a degree histogram read, and broken links are buried among
/// unrelated diagnostics. This read composes the three over the same in-scope
/// node set, so the join happens once, in the engine, where the graph lives.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct GraphHealthArgs {
    #[serde(default)]
    repo: Option<String>,
    /// The `scope` argument; absent takes this read's default, stated in the
    /// scope table beside [`Request`] (the same row shape as `graph_shape`).
    #[serde(default)]
    scope: ScopeArg,
    /// Cap per list (cycles, isolated, dead ends, sources, broken links,
    /// central). Default 20, range 1..=200. `counts` always covers the whole
    /// in-scope graph.
    limit: Option<usize>,
    /// Add the per-verb request counters, a `verbs` object BESIDE the payload.
    /// Absent (or false) leaves the key out entirely, so a client that does not
    /// ask sees byte-identical results to the read before this argument existed.
    #[serde(default)]
    verbs: bool,
}

/// Args for the `grep` read: a literal text search over the catalogued files.
///
/// Not a regex engine and not a semantic search. It answers the one question a
/// read surface over a knowledge base must answer without the caller walking the
/// catalogue: "which notes contain this text". The engine holds the catalogue
/// and the member scopes, so the alternative — N `content` reads guessed by the
/// caller — is both slower and less correct.
///
/// Every file is read from the working tree, which is the materialised ref, the
/// same sanctioned exception the `content` read takes. Files above
/// [`GREP_MAX_FILE_BYTES`] and files holding a NUL byte in their first
/// [`GREP_SNIFF_BYTES`] are skipped and counted, never silently matched.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct GrepArgs {
    /// The literal text to find. Empty is rejected: it would match every line.
    query: String,
    /// Absent = every mounted member. Present = only this member's files.
    #[serde(default)]
    repo: Option<String>,
    /// The `scope` argument; absent takes this read's default (`own`), stated
    /// with its reason in the scope table beside [`Request`]. A `repo` narrows
    /// WHICH member to scan, not whether to scan outside the caller's own
    /// content, so it does not move the default.
    #[serde(default)]
    scope: ScopeArg,
    /// Case-sensitive matching. Default false: prose search is case-insensitive
    /// far more often than not, and the exact-case caller can ask.
    #[serde(default)]
    case_sensitive: bool,
    /// Match page size after `offset`. Default [`GREP_DEFAULT_LIMIT`], capped at
    /// [`GREP_MAX_LIMIT`].
    limit: Option<usize>,
    /// Match page start. Absent is 0.
    offset: Option<usize>,
}

/// Args for the `search` read: BM25 over the catalogued note text.
///
/// The ranking dual of [`GrepArgs`]. `grep` answers "which file contains this
/// text" by reading every file; this answers "which note is most about this
/// text" from the index the build already holds, so it ranks and reads no disk.
/// A note is a document when it has a body: its body is field 0 (weight 1.0) and
/// its `description:` frontmatter is field 1 (weight 3.0).
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchArgs {
    /// The words to look for. Matched by term, so `error->fix` is the conjunction
    /// of `error` and `fix`. Empty is rejected: it carries no term to rank.
    query: String,
    /// Absent = every mounted member. Present = only this member's documents.
    #[serde(default)]
    repo: Option<String>,
    /// The `scope` argument; absent takes this read's default (`own`), stated
    /// with its reason in the scope table beside [`Request`]. `all` widens to
    /// every mounted member, including dependencies, and is something a caller
    /// asks for rather than receives. A `repo` narrows WHICH member to search,
    /// so it does not move the default either.
    #[serde(default)]
    scope: ScopeArg,
    /// Require every distinct query term instead of the `min_should_match`
    /// default. A conjunction, not a phrase: the index stores no positions.
    #[serde(default)]
    exact: bool,
    /// How many distinct query terms a document must hold. Absent uses
    /// [`crate::search::default_min_should_match`]: all of them for one or two
    /// terms, two for three or more. `exact` overrides it.
    #[serde(default)]
    min_should_match: Option<usize>,
    /// Match page size after `offset`. Default [`SEARCH_DEFAULT_LIMIT`], capped at
    /// [`SEARCH_MAX_LIMIT`].
    limit: Option<usize>,
    /// Match page start. Absent is 0.
    offset: Option<usize>,
}

/// Args for the `type_counts` read: the same `repo` scope as `types`, but over
/// the FULL set, so no `summary` / `limit` / `offset`. Those are rejected as
/// unknown args (meaningless for a count), per silent-drops-are-worse.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct TypeCountsArgs {
    #[serde(default)]
    repo: Option<String>,
    /// The `scope` argument; absent takes this read's default, stated in the
    /// scope table beside [`Request`].
    #[serde(default)]
    scope: ScopeArg,
}

/// Args for the `instance_counts` read: the same `repo` / `scope` pair as
/// `type_counts`, plus an `origins?` filter mirroring `instances_of`, so the
/// count read is a true dual of the list read. Absent `origins` counts ALL
/// origins (file + nested + meta), identical to `instances_of`'s no-arg set, so
/// the two duals default the same way. A present list narrows the tally, so
/// `origins:['file']` yields the file-only count that matches a file-only
/// `instances_of` drill-in. Unknown args are rejected loudly, matching the other
/// reads.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct InstanceCountsArgs {
    #[serde(default)]
    repo: Option<String>,
    /// The `scope` argument; absent takes this read's default, stated in the
    /// scope table beside [`Request`].
    #[serde(default)]
    scope: ScopeArg,
    /// `file` / `nested` / `meta`, same semantics as `instances_of`. Absent =
    /// all three.
    #[serde(default)]
    origins: Option<Vec<wire::Origin>>,
}

/// Args for the `type` read. `deny_unknown_fields` keeps the documented
/// strictness: an unknown arg is an `error` frame, never a silent result. A
/// missing `name` fails to deserialize, and a stray `names` is rejected as
/// unknown here — the two selectors are separate verbs now, so the former
/// both-present / neither-present validation is structural rather than a
/// `TryFrom`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TypeArgs {
    /// Bare (`foo`) or `::repo`-qualified (`foo::repo`), the authored form.
    name: String,
}

/// Args for the `type_batch` read, the batch dual of [`TypeArgs`]. Same
/// strictness.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TypeBatchArgs {
    /// Each bare or `::repo`-qualified; results come back order-matched.
    names: Vec<String>,
}

/// Args for the `candidates` read. All optional. The scan is knowledge-base-wide, so
/// there is no `repo` scope. Unknown args are rejected loudly, matching the
/// other read args.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct CandidatesArgs {
    /// Project each file to the lightweight summary (file + candidate type
    /// names) instead of full per-candidate detail. Default false = full.
    #[serde(default)]
    summary: bool,
    /// Page size: at most this many files after `offset`, in the catalog's
    /// sorted order. Absent returns every scanned file.
    limit: Option<usize>,
    /// Page start: skip this many files. Absent is 0.
    offset: Option<usize>,
}

/// Args for the `candidate_counts` read. It takes none (the scan is knowledge-base-wide
/// and counts are over the full set), so any arg is rejected loudly, matching
/// the other reads' strictness.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct CandidateCountsArgs {}

/// The diagnostics filters, shared by the `diagnostics` read and channel.
/// All optional, all compose (AND). Unknown args are rejected loudly rather
/// than silently ignored — a typo'd filter must not return the unfiltered
/// set, per the silent-drops-are-worse-than-rejection principle.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct DiagnosticsFilterArgs {
    /// Keep diagnostics in exactly this file, repo-relative or absolute.
    path: Option<String>,
    /// Keep diagnostics in files under this directory path, matched on
    /// whole path components.
    path_prefix: Option<String>,
    /// Keep diagnostics of exactly this severity.
    severity: Option<Severity>,
    /// Keep diagnostics carrying exactly this code.
    code: Option<String>,
    /// Page size for the `diagnostics` read: at most this many entries, after
    /// `offset`. Absent returns the whole filtered set. Read-only: the
    /// subscription channel and `diagnostic_counts` operate over the full set
    /// and ignore paging.
    limit: Option<usize>,
    /// Page start for the `diagnostics` read: skip this many filtered entries
    /// before the page. Absent is 0.
    offset: Option<usize>,
    /// Keep diagnostics owned by exactly this member. Absent spans every
    /// mounted member, the `repo` meaning every read shares.
    repo: Option<String>,
    /// Keep diagnostics owned by the user's own repos (`own`, the DEFAULT) or
    /// by every mounted member (`all`). Orthogonal to `repo`, which selects
    /// WHICH member.
    ///
    /// Defaults to `own` because diagnostics are a WORKLIST, not an inventory:
    /// a validation error inside a read-only dependency is not something the
    /// user can act on, so it is noise in the default answer. The
    /// workspace-health codes (`peer-unmounted`, `edit-member-unmounted`,
    /// `dependency-cache-miss`) are anchored at the DECLARING repo's manifest,
    /// so they stay visible under `own` — nothing actionable is hidden.
    ///
    /// Defaults to `all` when ANY location pin is present — `repo`, `path`, or
    /// `path_prefix` — because the caller already named what they are looking
    /// at. See [`Self::resolve`] and the scope table beside [`Request`].
    #[serde(default)]
    scope: ScopeArg,
}

impl DiagnosticsFilterArgs {
    /// Resolve against the knowledge base root: path args absolutize the way every
    /// other read's path arg does.
    fn resolve(self, root: &Path) -> DiagnosticsScope {
        // A LOCATION PIN — a named repo, file, or subtree — is an explicit
        // narrowing, so it defaults `scope` to `all`. A `path` naming one file
        // is at least as explicit as a `repo`: asking for the file the user is
        // looking at must not empty out just because that file lives in a
        // read-only dependency (own-scoped, `owner_matches` would drop it). Only
        // the UNPINNED "give me my problems" case keeps the `own` worklist
        // default. The rule itself lives in `ScopedArgs::scope`.
        let scope = self.scope();
        DiagnosticsScope {
            path: self.path.map(|p| abs_arg(root, &p)),
            path_prefix: self.path_prefix.map(|p| abs_arg(root, &p)),
            severity: self.severity,
            code: self.code,
            scope,
            repo: self.repo,
        }
    }
}

/// The `scope` declarations, one per args struct that carries the argument. Each
/// `DEFAULT` is the read's row in the scope table beside [`Request`]; the table
/// test holds the two together, and [`ScopedArgs::scope`] is what the handlers
/// call, so a default is stated once and resolved once.
impl ScopedArgs for DiagnosticsFilterArgs {
    const DEFAULT: ScopeDefault = ScopeDefault::OwnUnlessPinned;
    fn scope_arg(&self) -> ScopeArg {
        self.scope
    }
    /// A file or subtree names what the caller is looking at as explicitly as a
    /// `repo` does, so all three pins default the worklist to `all`.
    fn pin_moves_default(&self) -> bool {
        self.repo.is_some() || self.path.is_some() || self.path_prefix.is_some()
    }
}

impl ScopedArgs for TypesArgs {
    const DEFAULT: ScopeDefault = ScopeDefault::All;
    fn scope_arg(&self) -> ScopeArg {
        self.scope
    }
}

impl ScopedArgs for TypeCountsArgs {
    const DEFAULT: ScopeDefault = ScopeDefault::All;
    fn scope_arg(&self) -> ScopeArg {
        self.scope
    }
}

impl ScopedArgs for TypeTreeArgs {
    const DEFAULT: ScopeDefault = ScopeDefault::All;
    fn scope_arg(&self) -> ScopeArg {
        self.scope
    }
}

impl ScopedArgs for ImportsArgs {
    const DEFAULT: ScopeDefault = ScopeDefault::All;
    fn scope_arg(&self) -> ScopeArg {
        self.scope
    }
}

impl ScopedArgs for SubtypesArgs {
    const DEFAULT: ScopeDefault = ScopeDefault::All;
    fn scope_arg(&self) -> ScopeArg {
        self.scope
    }
}

impl ScopedArgs for InstanceCountsArgs {
    const DEFAULT: ScopeDefault = ScopeDefault::All;
    fn scope_arg(&self) -> ScopeArg {
        self.scope
    }
}

impl ScopedArgs for FilesArgs {
    const DEFAULT: ScopeDefault = ScopeDefault::All;
    fn scope_arg(&self) -> ScopeArg {
        self.scope
    }
}

impl ScopedArgs for NeighborhoodArgs {
    const DEFAULT: ScopeDefault = ScopeDefault::All;
    fn scope_arg(&self) -> ScopeArg {
        self.scope
    }
}

impl ScopedArgs for OverviewArgs {
    const DEFAULT: ScopeDefault = ScopeDefault::OwnUnlessPinned;
    fn scope_arg(&self) -> ScopeArg {
        self.scope
    }
    fn pin_moves_default(&self) -> bool {
        self.repo.is_some()
    }
}

impl ScopedArgs for TopLevelDirsArgs {
    const DEFAULT: ScopeDefault = ScopeDefault::OwnUnlessPinned;
    fn scope_arg(&self) -> ScopeArg {
        self.scope
    }
    fn pin_moves_default(&self) -> bool {
        self.repo.is_some()
    }
}

impl ScopedArgs for HubsArgs {
    const DEFAULT: ScopeDefault = ScopeDefault::OwnUnlessPinned;
    fn scope_arg(&self) -> ScopeArg {
        self.scope
    }
    fn pin_moves_default(&self) -> bool {
        self.repo.is_some()
    }
}

impl ScopedArgs for GraphShapeArgs {
    const DEFAULT: ScopeDefault = ScopeDefault::OwnUnlessPinned;
    fn scope_arg(&self) -> ScopeArg {
        self.scope
    }
    fn pin_moves_default(&self) -> bool {
        self.repo.is_some()
    }
}

impl ScopedArgs for GraphHealthArgs {
    const DEFAULT: ScopeDefault = ScopeDefault::OwnUnlessPinned;
    fn scope_arg(&self) -> ScopeArg {
        self.scope
    }
    fn pin_moves_default(&self) -> bool {
        self.repo.is_some()
    }
}

impl ScopedArgs for LinkGraphArgs {
    const DEFAULT: ScopeDefault = ScopeDefault::OwnUnlessPinned;
    fn scope_arg(&self) -> ScopeArg {
        self.scope
    }
    fn pin_moves_default(&self) -> bool {
        self.repo.is_some()
    }
}

impl ScopedArgs for TypeGraphArgs {
    const DEFAULT: ScopeDefault = ScopeDefault::OwnUnlessPinned;
    fn scope_arg(&self) -> ScopeArg {
        self.scope
    }
    fn pin_moves_default(&self) -> bool {
        self.repo.is_some()
    }
}

impl ScopedArgs for GrepArgs {
    const DEFAULT: ScopeDefault = ScopeDefault::Own;
    fn scope_arg(&self) -> ScopeArg {
        self.scope
    }
}

impl ScopedArgs for SearchArgs {
    const DEFAULT: ScopeDefault = ScopeDefault::Own;
    fn scope_arg(&self) -> ScopeArg {
        self.scope
    }
}

/// The resolved diagnostics scope, the one filter predicate behind the
/// `diagnostics` read and channel.
struct DiagnosticsScope {
    path: Option<PathBuf>,
    path_prefix: Option<PathBuf>,
    severity: Option<Severity>,
    code: Option<String>,
    repo: Option<String>,
    scope: wire::TypeScope,
}

impl Default for DiagnosticsScope {
    fn default() -> Self {
        Self {
            path: None,
            path_prefix: None,
            severity: None,
            code: None,
            repo: None,
            scope: wire::TypeScope::all(),
        }
    }
}

impl DiagnosticsScope {
    /// Whether the repo axis is engaged at all, so the common unfiltered case
    /// skips the per-diagnostic owner resolution entirely.
    fn filters_by_repo(&self) -> bool {
        self.repo.is_some() || self.scope.own_only()
    }

    /// Whether a diagnostic's owning member passes the `repo` / `scope` pair.
    ///
    /// Ownership is the member owning the diagnostic's `span.file`, resolved by
    /// `repo_of`, which picks the DEEPEST containing repo — so a diagnostic
    /// inside a nested member attributes to that member, not to its container.
    ///
    /// An UNOWNED diagnostic is believed impossible: every knowledge-base-stream
    /// diagnostic is anchored either at a file the walk catalogued or at a
    /// mounted member's manifest (`peer-unmounted`, the likeliest candidate,
    /// anchors at the DECLARING repo's `repo.yaml`), and the device-global
    /// config diagnostics ride inside the `device_config` read rather than this
    /// stream. So this is a defensive branch, not a documented behaviour to
    /// code against. It asserts in debug rather than silently dropping: if some
    /// future emitter does produce one, we find out instead of quietly
    /// filtering it out of a scoped answer.
    fn owner_matches(&self, v: &KnowledgeBase, d: &Diagnostic) -> bool {
        if !self.filters_by_repo() {
            return true;
        }
        let Some(owner) = v.repos.repo_of(&d.span.file) else {
            debug_assert!(
                false,
                "diagnostic {} at {} belongs to no mounted member; scoped \
                 diagnostics would silently drop it",
                d.code.as_str(),
                d.span.file.display()
            );
            return false;
        };
        if let Some(want) = self.repo.as_deref() {
            if owner.name.as_str() != want {
                return false;
            }
        }
        self.scope.includes_repo(v, owner)
    }

    fn matches(&self, v: &KnowledgeBase, d: &Diagnostic) -> bool {
        self.path.as_deref().is_none_or(|p| d.span.file == p)
            && self
                .path_prefix
                .as_deref()
                .is_none_or(|p| d.span.file.starts_with(p))
            && self.severity.is_none_or(|s| d.severity == s)
            && self.code.as_deref().is_none_or(|c| d.code.as_str() == c)
            && self.owner_matches(v, d)
    }
}

/// The diagnostics in a scope, over the served stream.
///
/// A `path` or `path_prefix` scope narrows to a contiguous key range first, so
/// a per-file or per-directory read is O(log n + k), not a whole-stream scan.
/// The full `matches` predicate still runs over the narrowed set, so any
/// `severity` / `code` filter rides on top with identical semantics. A scope
/// with no path narrowing (unfiltered, or severity/code only) legitimately
/// spans the whole stream.
fn scoped_diagnostics<'a>(
    v: &'a KnowledgeBase,
    scope: &'a DiagnosticsScope,
) -> Box<dyn Iterator<Item = &'a Diagnostic> + 'a> {
    if let Some(path) = scope.path.as_deref() {
        Box::new(
            v.diagnostics_for_file(path)
                .filter(move |d| scope.matches(v, d)),
        )
    } else if let Some(prefix) = scope.path_prefix.as_deref() {
        Box::new(
            v.diagnostics_under(prefix)
                .filter(move |d| scope.matches(v, d)),
        )
    } else {
        Box::new(v.diagnostics().filter(move |d| scope.matches(v, d)))
    }
}

/// The `diagnostic_counts` read result: the filtered set's total, plus
/// breakdowns by severity and by code, each name-sorted (BTreeMap).
#[derive(Debug, Serialize)]
struct DiagnosticCountsView {
    total: usize,
    by_severity: std::collections::BTreeMap<String, usize>,
    by_code: std::collections::BTreeMap<String, usize>,
}

/// Count the scoped diagnostics into totals by severity and by code, the
/// `diagnostic_counts` read's body. Shared with the `overview` read, which
/// counts the whole knowledge base (an empty scope).
fn diagnostic_counts_view(v: &KnowledgeBase, scope: &DiagnosticsScope) -> DiagnosticCountsView {
    let mut total = 0usize;
    let mut by_severity: std::collections::BTreeMap<String, usize> =
        std::collections::BTreeMap::new();
    let mut by_code: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for d in scoped_diagnostics(v, scope) {
        total += 1;
        *by_severity
            .entry(severity_str(d.severity).to_string())
            .or_default() += 1;
        *by_code.entry(d.code.as_str().to_string()).or_default() += 1;
    }
    DiagnosticCountsView {
        total,
        by_severity,
        by_code,
    }
}

/// The wire string for a severity, the same lowercase form `Severity`
/// serializes to and the `severity` filter accepts.
fn severity_str(s: Severity) -> &'static str {
    match s {
        Severity::Error => "error",
        Severity::Drift => "drift",
        Severity::Warning => "warning",
        Severity::Hint => "hint",
    }
}

/// A `response` frame: the reply to a `read`. `ready` is false while the ref is
/// Deriving; `version` and `result` are present only when ready.
#[derive(Debug, Serialize)]
struct Response {
    #[serde(rename = "type")]
    kind: &'static str,
    schema_version: u32,
    /// The request's `id`, echoed so the client settles by id. Omitted when the
    /// request carried none. Stamped on the way out, after the handler builds
    /// the response.
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<serde_json::Value>,
    ready: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<serde_json::Value>,
    /// The human-facing reason on an `error`-kind response. `None` on a normal
    /// response. Present when a read's ARGUMENTS are semantically invalid past
    /// what serde can reject (a cross-field rule like "kinds required past depth
    /// 1"), matching the `error` field a malformed-frame error carries.
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

impl Response {
    fn not_ready() -> Self {
        Response {
            kind: "response",
            schema_version: SCHEMA_VERSION,
            id: None,
            ready: false,
            version: None,
            result: None,
            error: None,
        }
    }

    fn ready(version: u64, result: serde_json::Value) -> Self {
        Response {
            kind: "response",
            schema_version: SCHEMA_VERSION,
            id: None,
            ready: true,
            version: Some(version),
            result: Some(result),
            error: None,
        }
    }

    /// An `error`-kind response for a semantically-invalid read argument, the
    /// handler-level analogue of the parse-layer `error_frame`. A cross-field
    /// rule (`kinds` required past depth 1) that serde cannot express is
    /// checked in the handler and rejected here rather than answered.
    fn invalid(message: impl Into<String>) -> Self {
        Response {
            kind: "error",
            schema_version: SCHEMA_VERSION,
            id: None,
            ready: false,
            version: None,
            result: None,
            error: Some(message.into()),
        }
    }

    /// Stamp the request's `id` onto this response so the client settles by id.
    fn with_id(mut self, id: Option<serde_json::Value>) -> Self {
        self.id = id;
        self
    }
}

/// Knowledge-base-wide per-instance introspection. `entries` is empty when the build
/// aborted at load; `aborted_at_load` distinguishes that from a knowledge base with no
/// instances. Per-file diagnostics ride the `diagnostics` read, not duplicated
/// here.
#[derive(Debug, Serialize)]
struct InstancesView {
    count: usize,
    aborted_at_load: bool,
    /// The payload, keyed by the read's own name. `count` and
    /// `aborted_at_load` beside it are envelope metadata ABOUT the payload.
    instances: Vec<wire::InstanceIntrospection>,
}

/// Knowledge-base-wide implicit-identity candidate scan, grouped by file. `aborted_at_load`
/// distinguishes a skipped scan (broken vocabulary) from an empty one. `files`
/// is full detail or the lightweight summary, paged by [`CandidatesArgs`].
#[derive(Debug, Serialize)]
struct CandidatesView {
    aborted_at_load: bool,
    /// The payload, keyed by the read's own name; `aborted_at_load` beside it
    /// is envelope metadata.
    candidates: wire::WireCandidateFiles,
}

/// One instance's resolved view, coherent at one version. `resolved` is false
/// when the claim doesn't resolve (no effective shape); `closure` is then
/// empty. The value layer (`effective_values`, `section_presence`,
/// `body_events`) carries full contribution provenance, serving `ProvenancePort`
/// from the same coherent read; it is present even for an unresolved claim,
/// since frontmatter and body contributions don't need the graph.
#[derive(Debug, Serialize)]
struct ResolvedView {
    resolved: bool,
    claim: Vec<String>,
    closure: Vec<String>,
    candidates: Vec<CandidateDto>,
    effective_values: Vec<wire::FieldValuesEntry>,
    section_presence: Option<Vec<wire::SectionPresenceEntry>>,
    body_events: Option<Vec<wire::BodyEventIntrospection>>,
    /// Addressable inline records ([[type block-id::au-type-system]]); omitted when none.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    record_block_ids: Vec<wire::RecordBlockIdIntrospection>,
    diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Serialize)]
struct CandidateDto {
    type_name: String,
    /// RFC 6901 JSON pointer to the scope inside the file (`""` = top level).
    inline_path: String,
}

#[derive(Debug, Serialize)]
struct BacklinkDto {
    source: String,
    /// The source's repo, present only when the inbound edge crosses a repo
    /// boundary (the source is in a different repo than the target). Omitted for
    /// a repo-local edge. Saves the consumer a `resolve_member` round-trip on the
    /// absolute `source` path to recover cross-repo provenance.
    #[serde(skip_serializing_if = "Option::is_none")]
    repo: Option<String>,
    slot: Option<String>,
    surface: String,
    /// The inbound edge kind, `navigational` / `contributing` / `field`. Derived
    /// from `surface` and `slot`, so it never disagrees with them. `field` is a
    /// frontmatter-surface edge; the typed/untyped split `references_out` makes
    /// (`field-reference` / `field-string-wikilink` / `unknown`) is deferred
    /// inbound, a later refinement of `field`. See [`inbound_kind`].
    kind: &'static str,
    span_start: usize,
    span_end: usize,
    /// Line/column rendering of the span, in the `source` file.
    #[serde(skip_serializing_if = "Option::is_none")]
    line_col: Option<tb_diagnostics::LineColRange>,
    /// The TARGET block-id fragment with its mode: `{ id, referent }`. `referent`
    /// is `true` for a `^^id` block-referent, `false` for a bare `^id` anchor.
    block_id: Option<tb_references::BlockId>,
    /// The `^:` id of the source record the reference lives in, when the
    /// edge originates inside an inline record — renders as
    /// `[[<source>^<source_block_id>]]`. Omitted otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    source_block_id: Option<String>,
}

/// A node reference in a `neighborhood` edge: the file, plus the `^^`-addressed
/// block id when the endpoint is a block node.
#[derive(Debug, Serialize)]
struct NodeRefDto {
    path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    block_id: Option<String>,
}

/// One reached node in the `neighborhood` subgraph.
#[derive(Debug, Serialize)]
struct NeighborhoodNodeDto {
    path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    block_id: Option<String>,
    depth: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    repo: Option<String>,
    file_kind: &'static str,
    /// Byte length a consumer costs a `content` fetch by: a file node's whole-file
    /// length, a block node's span length. `null` for an unread asset or a block
    /// whose `^^` id did not resolve.
    bytes: Option<usize>,
    /// Byte length a consumer costs a `body` fetch by: a file node's prose-body
    /// length, a block node's span length. `null` for a pure-YAML / type-def
    /// file, or a block whose id did not resolve.
    body_bytes: Option<usize>,
}

/// One traversed edge in the `neighborhood` subgraph, stored natural-direction.
/// The `references_out` fields, populated from whichever direction discovered
/// the edge: an inbound-discovered edge carries no `repo` / `commit` / `anchor`
/// (the backlink index dropped the link).
#[derive(Debug, Serialize)]
struct NeighborhoodEdgeDto {
    from: NodeRefDto,
    /// `null` for a dangling edge (an outbound link resolving to nothing), or a
    /// scope- / budget-excluded target that is not in `nodes`.
    to: Option<NodeRefDto>,
    kind: &'static str,
    surface: &'static str,
    span_start: usize,
    span_end: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    line_col: Option<tb_diagnostics::LineColRange>,
    #[serde(skip_serializing_if = "Option::is_none")]
    field: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_block_id: Option<String>,
    /// The block-id fragment on the link, `{ id, referent }`; absent when none.
    #[serde(skip_serializing_if = "Option::is_none")]
    block_id: Option<tb_references::BlockId>,
    /// The `::repo` qualifier, present only on an outbound-discovered crossing.
    #[serde(skip_serializing_if = "Option::is_none")]
    repo: Option<String>,
    /// The `@commit` pin, present only on an outbound-discovered pinned link.
    #[serde(skip_serializing_if = "Option::is_none")]
    commit: Option<String>,
    /// The `#anchor` fragment, present only on an outbound-discovered link.
    #[serde(skip_serializing_if = "Option::is_none")]
    anchor: Option<String>,
}

/// A node cut by `max_nodes`, named so a consumer can surface or re-fetch it.
#[derive(Debug, Serialize)]
struct DroppedNodeDto {
    path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    block_id: Option<String>,
    depth: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    repo: Option<String>,
}

/// The `neighborhood` read payload: the reachable subgraph plus the truncation
/// report.
#[derive(Debug, Serialize)]
struct NeighborhoodDto {
    nodes: Vec<NeighborhoodNodeDto>,
    edges: Vec<NeighborhoodEdgeDto>,
    truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    truncated_at_depth: Option<usize>,
    dropped: Vec<DroppedNodeDto>,
}

/// One outgoing body wikilink: the bare target, the resolved file path (null
/// when it doesn't resolve), the source span (bytes plus line/column), and
/// the fragment parts. Serves `LinkGraphPort.getOutgoing`.
#[derive(Debug, Serialize)]
struct OutgoingRefDto {
    target: String,
    resolved: Option<String>,
    span: wire::SpanRange,
    /// The `::repo` qualifier when the link crosses a repo boundary; absent for
    /// an unqualified link. `resolved` then points into that repo.
    #[serde(skip_serializing_if = "Option::is_none")]
    repo: Option<String>,
    /// The `@commit` pin, the commit-ish a pinned reference resolves against;
    /// absent for an unpinned link. Binds to `::repo`. The resolved-edge mirror
    /// of the `body_events.wikilink.parsed` `commit`, so the outgoing-edge set
    /// carries which edges are pinned without a per-file body scan.
    ///
    /// A commit-bearing edge is an INERT PIN: it is a snapshot into an immutable
    /// past, so `resolved` is null (a pin does not attach to a live file) and the
    /// edge is NEVER dangling, whatever its `kind`. A consumer MUST NOT read a
    /// pin's null `resolved` as a broken link — the `commit` presence is the
    /// signal. This holds for the named-target pin (`[[file::@sha]]`, an ordinary
    /// `kind` here) as well as the empty-target `commit-referent` kind.
    #[serde(skip_serializing_if = "Option::is_none")]
    commit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    anchor: Option<String>,
    /// The block-id fragment with its mode: `{ id, referent }`. `referent` is
    /// `true` for a `^^id` block-referent (the block's value fills the slot),
    /// `false` for a bare `^id` navigational anchor (the file is the referent).
    #[serde(skip_serializing_if = "Option::is_none")]
    block_id: Option<tb_references::BlockId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    field: Option<String>,
    /// The `^:` id of the enclosing inline record this edge originates from,
    /// when it does. Mirrors the inbound edge's field of the same name, so the
    /// two directions describe an edge the same way.
    #[serde(skip_serializing_if = "Option::is_none")]
    source_block_id: Option<String>,
    /// Which surface the edge sits on: `frontmatter` or `body`.
    surface: &'static str,
    /// What KIND of edge this is. Load-bearing, not cosmetic: the kind decides
    /// how a consumer should act on the edge and how severe a break is.
    ///
    /// The "dangling" reading below is OVERRIDDEN when `commit` is set: a
    /// commit-bearing edge is an inert pin, so its null `resolved` is expected,
    /// not a break, whatever this kind says. Check `commit` before treating any
    /// null `resolved` as dangling.
    ///
    /// - `field-reference` — a typed reference field value in frontmatter, a
    ///   slot that admits one (`myType*` / `file*` / `any*` / a reference
    ///   branch of a compound). The structural edge; validated and
    ///   closure-checked, and a dangling one is an error.
    /// - `field-string-wikilink` — a `[[...]]` in a frontmatter value whose
    ///   slot does NOT admit a reference, or one embedded in a longer string.
    ///   An intended-but-untyped pointer; navigational only.
    /// - `contributing` — a body `[[target:field]]`, which is both a link and a
    ///   data contribution. Following it says where a field value came from.
    /// - `navigational` — a body prose `[[...]]` with no attribution. A hint;
    ///   a dangling one is a warning.
    /// - `commit-referent` — a commit-only reference (`[[::@sha]]` /
    ///   `[[::repo@sha]]`) that names a COMMIT, not a file. `resolved` is null and
    ///   `commit` is set; the commit stays readable because git history is
    ///   append-only, and the edge is NEVER dangling, so a consumer must not treat
    ///   its null `resolved` as a broken link. Surface-independent, settled by the
    ///   link's own syntax.
    /// - `unknown` — a WHOLE-VALUE frontmatter wikilink on a TYPED instance
    ///   whose slot could not be resolved, so the engine cannot say which of the
    ///   first two it is.
    ///
    /// The frontmatter split is decided by the SLOT, never by the value's
    /// syntax — the same rule the value layer applies.
    ///
    /// `unknown` exists because the alternative is a LIE. Where a typed
    /// instance's effective shape is absent (its `type:` claim does not resolve,
    /// or its repo's vocabulary aborted) this used to report
    /// `field-string-wikilink`, which is documented above as a positive
    /// assertion that the slot does not admit a reference. Saying "I cannot
    /// tell" is the honest answer, and it is NARROW: it needs a `type:` intent
    /// the engine could not honor. A body edge is settled by the link's syntax,
    /// an embedded frontmatter link at the value level, and a plain NOTE is
    /// definitively untyped (no slot admits a reference, so `field-string-
    /// wikilink`) — none of those can be `unknown`.
    kind: &'static str,
}

/// One inert-pin record the `pins` reverse read returns: a commit-pinned
/// reference naming `target`, and where it sits. The inbound direction an
/// inert pin drops, recovered by folding the retained OUTBOUND pins, so every
/// field is source-side. Never a live edge, so there is no `resolved` — the
/// pin names a coordinate into an immutable past, not a current file.
#[derive(Debug, Serialize)]
struct PinRecordDto {
    /// The file holding the pin.
    source: String,
    /// The `^:` id of the enclosing inline record the pin sits in, when it does
    /// — a nested-record site (au-provenance's spans nest inside a ledger file).
    /// Absent for a top-level frontmatter or body pin.
    #[serde(skip_serializing_if = "Option::is_none")]
    source_block_id: Option<String>,
    /// The pin's byte span in `source`, plus derived line/column.
    span: wire::SpanRange,
    /// The field the pin fills, the innermost key for a nested value; a body
    /// `:field` attribution; or absent for an untyped prose pin. A consumer
    /// filters by field (au-provenance keeps `read`) without a second read.
    #[serde(skip_serializing_if = "Option::is_none")]
    slot: Option<String>,
    /// Which surface the pin sits on: `frontmatter` or `body`.
    surface: &'static str,
    /// The pinned target NAME, exactly as the pin recorded it. Not live-
    /// resolved: the read matches the recorded coordinate, never the graph.
    target: String,
    /// The `::repo` scope when the pin crosses a repo boundary; absent otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    repo: Option<String>,
    /// The pinned commit, always present — a pin is a commit-bearing reference.
    commit: String,
    /// The `^block-id` / `^^block-id` fragment on the pin, `{ id, referent }`;
    /// absent when the pin names no interior block.
    #[serde(skip_serializing_if = "Option::is_none")]
    block_id: Option<tb_references::BlockId>,
}

/// A resolved `[[target^block-id]]` reference: the file carrying the typed
/// block, its `type:` claim, and the fenced block's byte span. Serves
/// `LinkGraphPort.resolveBlockId`.
#[derive(Debug, Serialize)]
struct ResolvedBlockDto {
    file_path: String,
    /// Empty for a navigational `marker` and for a claim-less record.
    type_claim: Vec<String>,
    span: wire::SpanRange,
    /// Which addressable entity the id reached ([[type block-id::au-type-system]]):
    /// `record` (inline `^:`), `typed_block` (`[:field]` fence), or
    /// `marker` (bare `^id` marker or untyped fence id) — navigational,
    /// never a typed-reference target.
    kind: &'static str,
}

/// A resolved `#head` anchor: the heading line. Navigational only,
/// anchors are never type-bearing ([[type reference::au-type-system]]).
#[derive(Debug, Serialize)]
struct ResolvedAnchorDto {
    file_path: String,
    span: wire::SpanRange,
}

/// One addressable heading in a file: what a `#anchor` fragment matches,
/// plus where it sits.
///
/// `file_path` is absent by design — it is the read's argument, so repeating
/// it per entry says nothing. What remains is `resolve_anchor`'s payload
/// (`span`) plus the key that selects it (`text`), so a consumer that lists
/// and then picks needs no second round trip.
#[derive(Debug, Serialize)]
struct AnchorDto {
    /// Exactly what a `#anchor` fragment matches: the heading text with its
    /// trailing `^id` marker excluded (the body scanner strips the marker
    /// before parsing the heading, so the text never carries one). Serving
    /// the matched form means a consumer inserts it verbatim and the link
    /// resolves.
    text: String,
    /// Heading depth, 1..=6. Carried because the scan already holds it, so
    /// an outline consumer needs no second read.
    level: u8,
    /// The heading LINE, the same span `resolve_anchor` returns.
    span: wire::SpanRange,
}

/// One addressable id in a file: what a `^block_id` fragment can reach.
///
/// `file_path` is absent for the same reason it is on [`AnchorDto`]: it is the
/// argument. What remains is `resolve_block_id`'s payload (`kind`,
/// `type_claim`, `span`) plus the key that selects it (`id`).
#[derive(Debug, Serialize)]
struct BlockIdDto {
    /// The bare id, without its `^` sigil, as `[[file^id]]` spells it.
    id: String,
    /// Which addressable entity carries it, `resolve_block_id`'s vocabulary
    /// verbatim ([[type block-id::au-type-system]]): `record` (inline `^:`), `typed_block`
    /// (`[:field]` fence), or `marker` (bare `^id` marker or untyped fence
    /// id).
    ///
    /// Typedness rides HERE, never a separate flag: a `^^` block-referent
    /// demands a typed value, satisfied by `record` and `typed_block` and not
    /// by `marker`. So a consumer completing `^^` filters on this field.
    kind: &'static str,
    /// The effective claim, explicit or slot-pinned. Empty for a `marker` and
    /// for a claim-less record.
    type_claim: Vec<String>,
    /// The addressed entity's span: the record's `^:` value, the whole fence,
    /// or the marker line. The same span `resolve_block_id` returns.
    span: wire::SpanRange,
}

/// One catalogued file: what a `[[` wikilink target can resolve to.
#[derive(Debug, Serialize)]
struct FileDto {
    /// The absolute path, the same address space every other read serves.
    path: String,
    /// The basename minus ONE extension, what a bare `[[name]]` resolves by
    /// ([[type reference::au-type-system]]): `x.session.yaml` stems to `x.session`. Served so
    /// no consumer re-derives the strip-one-extension rule, and the
    /// extensionless-match rule with it. Not unique: two files sharing a stem
    /// are an ambiguous bare target, which resolution reports and this listing
    /// does not.
    stem: String,
    /// The member owning the file; null when the path belongs to none.
    repo: Option<String>,
    /// `instance` / `type-def` / `note` / `asset`, the shared vocabulary. See
    /// [`crate::parse::served_file_kind`].
    kind: &'static str,
}

/// One semantic token: a source `range` plus its AU-semantic role. The
/// `kind`-specific fields flatten beside `range`, so a token is
/// `{ range, kind, ... }`. Spec [[spec - semantic tokens]].
#[derive(Debug, Serialize)]
struct SemanticToken {
    range: wire::SpanRange,
    #[serde(flatten)]
    kind: SemanticTokenKind,
}

/// The AU-semantic role of a [`SemanticToken`], a tagged union on `kind`.
/// The body-surface kinds (wikilink, block-id, anchor) serve notes and
/// typed instances alike; the value-layer kinds (field-value, type-claim,
/// typed-block) join in as the read grows.
#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
enum SemanticTokenKind {
    /// A body or frontmatter wikilink whose target resolves to a file.
    WikilinkResolved {
        target: String,
        /// The `::repo` qualifier when the link crosses a repo boundary.
        #[serde(skip_serializing_if = "Option::is_none")]
        repo: Option<String>,
        resolved: String,
    },
    /// A wikilink whose target does not resolve.
    WikilinkBroken {
        target: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        repo: Option<String>,
    },
    /// A commit-pinned wikilink: a named pin (`[[file::@sha]]`) or the empty-target
    /// commit-referent (`[[::@sha]]` / `[[::repo@sha]]`). An inert coordinate into an
    /// immutable past, never re-resolved live and never dangling, so it is neither
    /// resolved nor broken. This ONE kind covers every inert pin (`is_inert_pin`),
    /// which is NOT symmetric with `references_out`: there only the empty-target
    /// commit-referent gets the `commit-referent` edge kind, while a named pin keeps
    /// its ordinary kind (`navigational` / `field-reference` / ...) and carries its
    /// inertness on the orthogonal `commit` / null-`resolved` fields instead.
    WikilinkPinned {
        target: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        repo: Option<String>,
        commit: String,
    },
    /// A typed scalar value, colored by its resolved type. `value_type` is
    /// the leaf's shape, a list element carrying the element shape. The type
    /// is the declared shape when the engine knows it (top-level, slot-
    /// pinned, or list-element), else inferred from the YAML literal for an
    /// extra or untyped-record field, always a bare primitive. The two are
    /// wire-identical by design; the declared-vs-extra distinction is a
    /// schema question, on the diagnostics / `type` reads.
    FieldValue {
        field: String,
        value_type: wire::WireShape,
    },
    /// A `type:` reference to a type-def, one per claimed name. Serves an
    /// instance identity claim, an inline-record or typed-fence claim, a
    /// type-def parent claim, and a `meta:` sub-region's `type:` alike.
    TypeClaim {
        /// The authored form, bare or `::repo`-qualified.
        name: String,
        /// The identity the claim resolves to, `(name, owning repo, closure
        /// hash)`, the same `TypeId` validation resolves it to. Absent when the
        /// claim does not resolve (an unknown type, an unmounted peer), so a
        /// consumer omits rather than guesses.
        #[serde(skip_serializing_if = "Option::is_none")]
        resolved: Option<wire::ClosureIdentity>,
    },
    /// An inline `[:field]` typed fence, spanning the whole fence. A
    /// container: its inner scalars emit their own `field-value` tokens.
    TypedBlock { field: String },
    /// An addressable id: a bare `^id` marker, a trailing fence id, or a
    /// `^:` inline-record id.
    BlockId { id: String },
    /// A heading, the target a `#anchor` fragment resolves to.
    Anchor { text: String },
    /// A type-def field's declared shape, a container over the whole shape
    /// expression. `value_type` is the parsed shape; null when the shape
    /// failed to parse. Encloses the type-ref / shape-builtin / enum-member
    /// leaves.
    FieldShape {
        field: String,
        value_type: Option<wire::WireShape>,
    },
    /// A navigable type-def name in a type-def's structure: a field-shape name
    /// (record / reference / inline base, compound operand, def-ref bound) or a
    /// `sealed:` branch.
    TypeRef { name: String },
    /// A built-in shape keyword: a primitive, `file`, `any`, or `type`.
    ShapeBuiltin { name: String },
    /// An inline enum literal in a field shape.
    EnumMember { value: String },
}

/// One direct child of a directory. Serves `RepoFilesPort.listChildren` and,
/// at the root, `TopLevelGraphsPort`.
#[derive(Debug, Serialize)]
struct DirEntryDto {
    path: String,
    name: String,
    kind: &'static str,
}

/// One top-level directory of one MOUNTED MEMBER, tagged with the member that
/// owns it. The repo tag is what makes a multi-member result readable, and it
/// lets a consumer regroup by member without a second read.
#[derive(Debug, Serialize)]
struct TopLevelDirDto {
    repo: String,
    name: String,
    path: String,
    /// The declared name of the mounted member ROOTED at this directory, when
    /// there is one; `None` for an ordinary content directory.
    ///
    /// A member may sit physically inside another repo (legal, though the usual
    /// topology places members elsewhere). Its folder then genuinely IS a
    /// directory of the containing repo, so it is reported for BOTH: once as
    /// the container's own top-level directory, and again as the member's own
    /// dirs under the member's tag. The overlap is honest, not double-counting
    /// to be filtered away.
    ///
    /// This field is what makes it legible. A consumer could derive it by
    /// joining `path` against every `members[].root`, but that pushes a
    /// path-equality comparison (canonicalization, symlinks, separators) onto
    /// every consumer for a fact known for free here. The member NAME rather
    /// than a bare flag, so the loop closes without a second read; a consumer
    /// wanting `root` / `role` / `editable` calls `members` once and joins by
    /// name, the same shape `instances_of.member` takes.
    ///
    /// Non-`None` only for a DECLARED member: the content walk is bounded at an
    /// undeclared nested repo's marker, so such a directory holds no catalogued
    /// file and never appears here at all.
    member: Option<String>,
}

/// A hub node in the `overview` read: a referenced file ranked by its inbound
/// reference count over the typed reference graph. `refs_structural` counts the
/// inbound edges filling a typed slot, `refs_navigational` the prose links; the
/// split is the neutral fact a consumer re-ranks by, there is no opaque score.
/// `kind` is the file's own role, `repo` its owner.
#[derive(Debug, Serialize)]
struct Hub {
    path: String,
    repo: Option<String>,
    kind: &'static str,
    refs_structural: usize,
    refs_navigational: usize,
    refs_total: usize,
}

/// The `graph_shape` read result: the scalar whole-graph summary. Raw facts, no
/// thresholds — the paper's numbers come from a 22-node corpus and are not
/// evidence, so the consumer applies its own policy.
#[derive(Debug, Serialize)]
struct GraphShapeView {
    /// The RESOLVED `repo` / `scope` this map was computed at, echoed back, so a
    /// consumer holds the scope it drilled at. Same rationale as `overview`.
    repo: Option<String>,
    scope: &'static str,
    /// Files in scope (the catalog node universe), and resolved edges in scope
    /// (both endpoints in scope). `edge_count` splits by the `slot.is_some()`
    /// partition `hubs` uses, so density per basis is derivable.
    node_count: usize,
    edge_count: usize,
    edges_structural: usize,
    edges_navigational: usize,
    /// Weakly-connected-component count over the undirected combined graph, the
    /// headline island count, and the node count of the biggest component (which
    /// distinguishes "14 tiny islands" from "one web plus 13 strays").
    components: usize,
    largest_component: usize,
    orphans: OrphansView,
    degree: DegreeView,
    /// `edge_count / node_count`, a convenience over the raw counts above.
    density: f64,
}

/// The two orphan definitions. `no_inbound` (nothing points at it) is the useful
/// one; `isolated` (no inbound AND no outbound, a singleton component) is the
/// strict subset. `paths` is present only when the read's `orphan_paths` is set.
#[derive(Debug, Serialize)]
struct OrphansView {
    no_inbound: usize,
    isolated: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    paths: Option<Vec<OrphanPath>>,
}

/// One orphan file, tagged so `no_inbound`-but-not-`isolated` is distinguishable
/// from `isolated` without a second pass.
#[derive(Debug, Serialize)]
struct OrphanPath {
    path: String,
    kind: &'static str,
}

/// The in- and out-degree distributions, each a histogram. The distribution
/// SHAPE is the signal, a few high-degree hubs over a long low-degree tail
/// versus a flat spread; `hubs` already serves the per-node top of it.
#[derive(Debug, Serialize)]
struct DegreeView {
    #[serde(rename = "in")]
    inbound: Vec<DegreeBucket>,
    out: Vec<DegreeBucket>,
}

/// `count` nodes carry exactly `degree` edges on this axis. Sorted by `degree`.
#[derive(Debug, Serialize, PartialEq)]
struct DegreeBucket {
    degree: usize,
    count: usize,
}

/// The `link_graph` read result: the full node+edge payload a whole-graph
/// visualization lays out. Nodes and edges are sorted for a deterministic
/// payload (and a stable diff for the `link_graph` subscription).
#[derive(Debug, Serialize)]
struct LinkGraphView {
    /// The RESOLVED `repo` / `scope`, echoed back, as `graph_shape` and
    /// `overview` do.
    repo: Option<String>,
    scope: &'static str,
    nodes: Vec<GraphNodeView>,
    edges: Vec<GraphEdgeView>,
}

/// A graph node: one content file. Carries the same hub counts `hubs` computes,
/// so the layout sizes a node for free. `refs_navigational` is
/// `refs_total - refs_structural`. Counts are over the IN-SCOPE inbound edges
/// (the induced subgraph), so they match the edge list in the same payload.
/// Isolated files (no edges) are present — a graph view must draw floating
/// orphans.
#[derive(Debug, Clone, Serialize, PartialEq)]
struct GraphNodeView {
    path: String,
    repo: Option<String>,
    kind: &'static str,
    refs_structural: usize,
    refs_total: usize,
}

/// A resolved reference edge, source to target. `kind` is the coarse edge kind
/// (`field` / `contributing` / `navigational`), `surface` the referrer surface
/// (`frontmatter` / `body`), the same vocabulary `references_in` serves. Only
/// resolved edges exist here; a dangling link is a diagnostic, not an edge.
#[derive(Debug, Clone, Serialize, PartialEq)]
struct GraphEdgeView {
    from: String,
    to: String,
    kind: &'static str,
    surface: &'static str,
}

/// A shutdown rendezvous: a `watch` channel flipped to `true` once a shutdown
/// request arrives. Connection tasks hold senders; the foreground waiter holds
/// a receiver. `watch` retains the latest value, so a shutdown that lands
/// before the waiter parks is not lost.
type ShutdownTx = watch::Sender<bool>;

/// A running server. Owns the tokio runtime driving the accept loop and every
/// connection task. Dropping it removes the socket file and shuts the runtime
/// down; the accept loop ends when the listener is dropped with it.
pub struct ServeHandle {
    socket: PathBuf,
    /// `Option` so `Drop` can `take` it and shut it down in the background.
    runtime: Option<Runtime>,
    /// Signals the accept loop and connection tasks to stop. Held so the watch
    /// channel stays open even if the accept loop ends, and flipped on `Drop`.
    shutdown_tx: ShutdownTx,
    shutdown_rx: watch::Receiver<bool>,
}

impl Drop for ServeHandle {
    fn drop(&mut self) {
        // Tell the accept loop and live connections to stop, then shut the
        // runtime down WITHOUT blocking this thread. Dropping a multi-thread
        // `Runtime` directly blocks on in-flight `spawn_blocking` tasks, which
        // is the teardown hang under the parallel test harness;
        // `shutdown_background` never blocks.
        let _ = self.shutdown_tx.send(true);
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
        let _ = std::fs::remove_file(&self.socket);
    }
}

impl ServeHandle {
    /// Block until a `Shutdown` request arrives over the socket.
    ///
    /// The daemon's foreground loop parks here; `au daemon stop` connecting and
    /// sending the shutdown verb wakes it. Returns once asked to stop, or once
    /// the channel closes; dropping the handle then removes the socket.
    pub fn wait_for_shutdown(&self) {
        let mut rx = self.shutdown_rx.clone();
        if let Some(runtime) = self.runtime.as_ref() {
            runtime.block_on(async move {
                let _ = rx.wait_for(|&v| v).await;
            });
        }
    }
}

/// Bind a Unix socket at `socket` and serve reads against `handle` until asked
/// to shut down. A private tokio runtime drives the accept loop and one task
/// per connection.
///
/// The socket is created `0o600`: the wire is unauthenticated, so the trust
/// boundary is the owning user. This is a single-user surface; a multi-user
/// deployment would need real caller authentication, not just file mode.
pub fn serve(handle: EngineHandle, socket: impl Into<PathBuf>) -> std::io::Result<ServeHandle> {
    let socket = socket.into();

    // A daemon serves one local socket. Its async layer only multiplexes I/O,
    // the heavy fs / git / validation work runs on `spawn_blocking`. So a small
    // fixed worker count is right, and the default one-worker-per-core would
    // only explode the thread count (badly so under the test harness, which
    // runs many in-process daemons on parallel threads). `max_blocking_threads`
    // is the scaling knob for throughput under many repos or many users.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .max_blocking_threads(512)
        .enable_all()
        .build()?;
    // The bind is the mutual-exclusion primitive: the OS lets exactly one
    // process bind a given path, so a live daemon's socket is never clobbered.
    // A pre-existing path fails with `AddrInUse`. Distinguish a live daemon
    // (connect succeeds) from a stale socket left by an ungraceful exit
    // (connect refused); only a confirmed-stale socket is removed and the bind
    // retried. The earlier code removed the path unconditionally, which would
    // unlink a live peer's socket on any probe false-negative.
    //
    // Residual: two concurrent starts both finding the SAME stale socket can
    // still race the reclaim. Closing that window fully needs an OS file lock
    // held for the daemon's lifetime (a dependency or MSRV 1.89's
    // `File::try_lock`); it is out of scope for the single-user surface.
    let bind = |rt: &Runtime| rt.block_on(async { tokio::net::UnixListener::bind(&socket) });
    let listener = match bind(&runtime) {
        Ok(l) => l,
        Err(e) if e.kind() == ErrorKind::AddrInUse => {
            if UnixStream::connect(&socket).is_ok() {
                // A live daemon owns the socket; leave it be.
                return Err(e);
            }
            // Stale socket from an ungraceful exit: reclaim it.
            std::fs::remove_file(&socket)?;
            bind(&runtime)?
        }
        Err(e) => return Err(e),
    };
    // Restrict the socket to the owning user: the wire is unauthenticated.
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))?;

    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let accept_tx = shutdown_tx.clone();
    runtime.spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let handle = handle.clone();
            let shutdown_tx = accept_tx.clone();
            tokio::spawn(async move {
                let _ = serve_connection(handle, stream, shutdown_tx).await;
            });
        }
    });
    // The accept loop is not signalled directly; `ServeHandle::Drop` shuts the
    // runtime down in the background, which cancels this task and every
    // connection task. That is what stops a `wait_for_shutdown` daemon once the
    // handle drops, and what makes test teardown non-blocking.

    Ok(ServeHandle {
        socket,
        runtime: Some(runtime),
        shutdown_tx,
        shutdown_rx,
    })
}

/// Serve one connection: a read loop, a writer task, and a session of
/// per-subscription tasks.
///
/// The stream is split. A writer task owns the write half and drains the frame
/// channel, so a response and a change event never interleave a partial frame.
/// The read loop owns the read half: it answers reads inline, and a `subscribe`
/// spawns a subscription task into the session. When the read loop ends, the
/// session is aborted and the frame channel closes, ending the writer.
async fn serve_connection(
    handle: EngineHandle,
    stream: tokio::net::UnixStream,
    shutdown_tx: ShutdownTx,
) -> std::io::Result<()> {
    let (mut read_half, mut write_half) = stream.into_split();

    let (frame_tx, mut frame_rx) = mpsc::channel::<Vec<u8>>(64);
    let writer = tokio::spawn(async move {
        while let Some(bytes) = frame_rx.recv().await {
            if write_frame_async(&mut write_half, &bytes).await.is_err() {
                break;
            }
        }
    });

    // The session: one task per active subscription, all aborted when the
    // connection ends. Each is assigned a per-connection id in its ack.
    let mut session: JoinSet<()> = JoinSet::new();
    let mut next_subscription_id: u64 = 0;
    let mut shutdown_after_flush = false;

    while let Some(frame) = read_frame_async(&mut read_half).await? {
        let mut value: serde_json::Value = match serde_json::from_slice(&frame) {
            Ok(v) => v,
            Err(e) => {
                // The bytes are not JSON, so there is no id to echo.
                send_frame(
                    &frame_tx,
                    error_frame(format!("malformed request: {e}"), None, "unknown"),
                )
                .await;
                continue;
            }
        };

        // The optional client-supplied correlation id, echoed on the reply so
        // the client settles by id rather than by arrival order. Removed before
        // the verb payload is deserialized: the arg structs deny unknown fields,
        // so a lingering `id` sibling would be rejected as a bad argument.
        let id = value.as_object_mut().and_then(|obj| obj.remove("id"));

        if value.get("read").is_some() {
            // The verb string for the trace span, captured before `value` is
            // consumed. A no-op when tracing is off.
            let verb = value
                .get("read")
                .and_then(|v| v.as_str())
                .unwrap_or("read")
                .to_string();
            match serde_json::from_value::<Request>(value) {
                Ok(req) => {
                    // A `shutdown` read stops the daemon, but only AFTER this
                    // reply is flushed: signalling here would race the writer
                    // (the daemon drops its runtime and kills the frame before
                    // it reaches the client). Record the intent and close the
                    // read loop so the writer drains first.
                    let is_shutdown = matches!(req, Request::Shutdown);
                    let response = handle_request(&handle, req)
                        .instrument(tracing::info_span!("read", verb = %verb))
                        .await
                        .with_id(id);
                    let bytes = serde_json::to_vec(&response).expect("response serializes");
                    if frame_tx.send(bytes).await.is_err() {
                        break;
                    }
                    if is_shutdown {
                        shutdown_after_flush = true;
                        break;
                    }
                }
                Err(e) => {
                    send_frame(
                        &frame_tx,
                        error_frame(format!("malformed read: {e}"), id, "read"),
                    )
                    .await;
                }
            }
        } else if value.get("subscribe").is_some() {
            match serde_json::from_value::<Subscription>(value) {
                Ok(sub) => {
                    let sub_id = next_subscription_id;
                    next_subscription_id += 1;
                    let handle = handle.clone();
                    let frame_tx = frame_tx.clone();
                    session.spawn(async move {
                        run_subscription(handle, frame_tx, sub_id, id, sub).await;
                    });
                }
                Err(e) => {
                    send_frame(
                        &frame_tx,
                        error_frame(format!("malformed subscribe: {e}"), id, "subscribe"),
                    )
                    .await;
                }
            }
        } else if value.get("mutate").is_some() {
            let verb = value
                .get("mutate")
                .and_then(|v| v.as_str())
                .unwrap_or("mutate")
                .to_string();
            match serde_json::from_value::<Mutation>(value) {
                Ok(m) => {
                    let mut frame = handle_mutation(&handle, m)
                        .instrument(tracing::info_span!("mutation", verb = %verb))
                        .await;
                    stamp_frame_id(&mut frame, &id);
                    if !send_frame(&frame_tx, frame).await {
                        break;
                    }
                }
                Err(e) => {
                    send_frame(
                        &frame_tx,
                        error_frame(format!("malformed mutate: {e}"), id, "mutate"),
                    )
                    .await;
                }
            }
        } else if value.get("resolve").is_some() {
            let mut frame = handle_resolve(&handle).await;
            stamp_frame_id(&mut frame, &id);
            if !send_frame(&frame_tx, frame).await {
                break;
            }
        } else {
            send_frame(
                &frame_tx,
                error_frame(
                    "request names neither a read, a subscribe, a mutate, nor a resolve"
                        .to_string(),
                    id,
                    "unknown",
                ),
            )
            .await;
        }
    }

    // Connection closed: abort the subscriptions, drop the frame sender so the
    // writer drains and ends.
    session.shutdown().await;
    drop(frame_tx);
    let _ = writer.await;
    if shutdown_after_flush {
        let _ = shutdown_tx.send(true);
    }
    Ok(())
}

/// Serialize a frame and hand it to the writer. Returns false when the channel
/// is closed (the connection went away), the producer task's cue to end.
async fn send_frame(frame_tx: &FrameTx, frame: serde_json::Value) -> bool {
    let bytes = serde_json::to_vec(&frame).expect("frame serializes");
    frame_tx.send(bytes).await.is_ok()
}

/// Stamp a request's `id` onto a value-built reply frame (mutation responses),
/// so the client settles it by id. A no-op when the request carried none.
fn stamp_frame_id(frame: &mut serde_json::Value, id: &Option<serde_json::Value>) {
    if let (Some(id), Some(obj)) = (id, frame.as_object_mut()) {
        obj.insert("id".to_string(), id.clone());
    }
}

/// An `error` frame: a request frame the daemon could not handle. `for_kind`
/// names the request shape (`read` | `subscribe` | `mutate` | `resolve` |
/// `unknown`) so a client can route the error even without an id; `id` echoes
/// the request's correlation id when it carried one.
fn error_frame(
    message: String,
    id: Option<serde_json::Value>,
    for_kind: &str,
) -> serde_json::Value {
    let mut frame = serde_json::json!({
        "type": "error",
        "schema_version": SCHEMA_VERSION,
        "error": message,
        "for": for_kind,
    });
    stamp_frame_id(&mut frame, &id);
    frame
}

/// An `ack` frame: the immediate reply to a `subscribe`. Echoes the request's
/// `id`, so the client ties this subscription to its request before the
/// per-subscription frames (which carry `subscription_id`) arrive.
fn ack_frame(
    subscription_id: u64,
    channel: &str,
    request_id: &Option<serde_json::Value>,
) -> serde_json::Value {
    let mut frame = serde_json::json!({
        "type": "ack",
        "schema_version": SCHEMA_VERSION,
        "subscription_id": subscription_id,
        "channel": channel,
        "accepted": true,
    });
    stamp_frame_id(&mut frame, request_id);
    frame
}

/// An `initial_value` frame: a channel's current state, delivered once after
/// the ack.
fn initial_value_frame(
    subscription_id: u64,
    at_version: u64,
    result: serde_json::Value,
) -> serde_json::Value {
    serde_json::json!({
        "type": "initial_value",
        "schema_version": SCHEMA_VERSION,
        "subscription_id": subscription_id,
        "at_version": at_version,
        "result": result,
    })
}

/// A `change_event` frame with the coarse whole-knowledge-base scope hint. Only the
/// `ready` channel still carries it; the data channels compute precise hints.
fn change_event_frame(subscription_id: u64, kind: &str, at_version: u64) -> serde_json::Value {
    serde_json::json!({
        "type": "change_event",
        "schema_version": SCHEMA_VERSION,
        "subscription_id": subscription_id,
        "kind": kind,
        "at_version": at_version,
        "scope_hint": { "scope": "knowledge-base" },
    })
}

/// The standard reject for a write whose owning member is consumed (`discover` /
/// `dep`): the engine may not author it. Shared by every write surface that
/// resolves an owning member — the path-arg gate below, `rename_type`'s
/// graph-derived def, and the config channel — so the check is one expression, not
/// a per-surface reinvention.
fn not_editable_reject(
    name: &crate::repo::RepoName,
    role: crate::repo::MemberRole,
) -> MutationReject {
    MutationReject::new(format!(
        "member '{}' is mounted as '{}', not editable — list it in workspace.yaml 'edit:' to author it",
        name.as_str(),
        role.as_str(),
    ))
}

/// Resolve a mutation path against the held knowledge base's members, then gate it
/// on the owning member's EDITABILITY. A write reaches any declared member the same
/// way a read does, but only an EDITABLE member (the entry or an `edit` member) may
/// be authored through the channel; a consumed member (`discover` / `dep`) is
/// rejected. This gate is what keeps a write off an unwatched, regenerable cache
/// snapshot (a silent-wrong-result), the write path's half of the read-side
/// role-derived editability.
///
/// Both mutation surfaces route through here, so preview reports the identical reject
/// the real write would.
fn resolve_editable_target(
    kb: &KnowledgeBase,
    root: &Path,
    path: &str,
) -> Result<std::path::PathBuf, MutationReject> {
    let target = crate::mutate::resolve_repo_path(&kb.repos, root, path)?;
    // The owning member is guaranteed by `resolve_repo_path` (a path under no member
    // is already the escape reject), so `repo_of` is `Some` here.
    let member = kb
        .repos
        .repo_of(&target)
        .expect("resolve_repo_path guarantees an owning member");
    let role = crate::wire::member_role(kb, &member.name);
    if !role.editable() {
        return Err(not_editable_reject(&member.name, role));
    }
    Ok(target)
}

/// A file rename planned over the held knowledge base: the repo the move stays
/// in, the repo-relative paths each referrer's spelling is re-rendered between,
/// and the inbound referrers, each source with the byte spans of its references
/// and the content hash the spans were read against.
struct RenamePlan {
    repo_root: PathBuf,
    old_rel: PathBuf,
    new_rel: PathBuf,
    referrers: BTreeMap<PathBuf, Vec<tb_diagnostics::ByteRange>>,
    referrer_hashes: BTreeMap<PathBuf, Option<crate::ir::ContentHash>>,
}

/// Plan a rename of `from` to `to`, or the refusal message. Shared by the
/// committing `rename` and its preview, so the two refuse, and gather, alike.
///
/// Refuses a type-def on either side: a type-def's name derives from its
/// filename, so moving one renames the type and strands its claims, which are
/// not wikilinks. That is `rename_type`'s job. Refuses a cross-repo move (v1
/// moves within one repo).
///
/// The referrers come from the backlink index, which locates; the source on
/// disk is the truth, re-read at write time. A self-reference is included, so
/// the moved file points at its new name.
fn plan_rename(v: &KnowledgeBase, from: &Path, to: &Path) -> Result<RenamePlan, String> {
    if tb_parser::classify_by_path(from) == Some(tb_parser::FileKind::TypeDef)
        || tb_parser::classify_by_path(to) == Some(tb_parser::FileKind::TypeDef)
    {
        return Err(
            "rename does not move type-def files — a type-def's name derives from its \
                    filename; use rename_type"
                .to_string(),
        );
    }
    let (rf, rt) = match (v.repos.repo_of(from), v.repos.repo_of(to)) {
        (Some(rf), Some(rt)) => (rf, rt),
        _ => return Err("path or destination escapes the workspace".to_string()),
    };
    if rf.root != rt.root {
        return Err(
            "cross-repo rename is not supported yet — path and to must be in the same repo"
                .to_string(),
        );
    }
    crate::dirop::check_destination_ancestors(&rf.root, to)?;
    let old_rel = from.strip_prefix(&rf.root).unwrap_or(from).to_path_buf();
    let new_rel = to.strip_prefix(&rf.root).unwrap_or(to).to_path_buf();
    let mut referrers: BTreeMap<PathBuf, Vec<tb_diagnostics::ByteRange>> = BTreeMap::new();
    let mut referrer_hashes: BTreeMap<PathBuf, Option<crate::ir::ContentHash>> = BTreeMap::new();
    for bl in v.backlinks(from) {
        referrers
            .entry(bl.source.clone())
            .or_default()
            .push(bl.span);
        referrer_hashes
            .entry(bl.source.clone())
            .or_insert_with(|| v.catalog.get(&bl.source).and_then(|e| e.hash));
    }
    Ok(RenamePlan {
        repo_root: rf.root.clone(),
        old_rel,
        new_rel,
        referrers,
        referrer_hashes,
    })
}

/// Resolve a mutation path against the held knowledge base's members. A write reaches any
/// declared member, scattered or subdir, the same way a read does. `Err` carries a
/// ready frame the caller returns as-is: a reject for an escaping/guarded/non-editable
/// path, or not-ready if the ref is mid-build.
fn resolve_mutation_target(
    handle: &EngineHandle,
    path: &str,
) -> Result<std::path::PathBuf, serde_json::Value> {
    let root = handle.root().to_path_buf();
    match handle
        .read(move |v| resolve_editable_target(v, &root, path))
        .value()
    {
        Some(Ok(target)) => Ok(target),
        Some(Err(reject)) => Err(mutation_reject_frame(reject)),
        None => Err(serde_json::to_value(Response::not_ready()).expect("response serializes")),
    }
}

/// One member of a mutation's saga: a git working tree the mutation touches,
/// with the tree-relative paths it writes there.
///
/// The member is the TREE, not the repo. Git commits per working tree and one
/// mutation makes one commit per tree, so several daemon-owned repos inside one
/// tree coalesce into a single member holding the union of their paths.
struct SagaMember {
    /// The working tree this member's paths commit into. For a non-git member
    /// (no enclosing tree anywhere) it is the owning repo root, which never
    /// commits.
    tree_root: PathBuf,
    /// The daemon-owned repos whose touched paths this tree holds, name-sorted.
    /// Several for a monorepo, one for a repo that is its own working tree.
    repo_names: Vec<crate::repo::RepoName>,
    /// Paths relative to `tree_root`, the basis the commit, the clean check, and
    /// the path-restore all take.
    paths: Vec<PathBuf>,
    /// Whether the member has a git working tree. A non-git member is written but
    /// never committed, the transition fallback until it is materialized under
    /// git.
    git: bool,
}

/// A mutation's per-repo plan, applied as one local saga.
///
/// The member set is the daemon-owned repos the mutation touches, each resolved
/// from a touched path by `repo_of` (the deepest owning repo). Every commit of
/// the saga carries the shared `mutation_id`. Single-repo is the N=1 case of the
/// same structure, not a separate path.
struct SagaPlan {
    mutation_id: String,
    members: Vec<SagaMember>,
    /// Absolute paths in the member set that the mutation will NOT write.
    ///
    /// A referrer the rewrite leaves byte-identical has nothing written to it:
    /// an all-commit-pinned one, or a bare `[[name]]` one of a name-keeping file
    /// move. It stays in the member set, because it still needs RECOMPUTING: a
    /// frozen pin stops resolving live once its target moves.
    ///
    /// Every git-facing step reads [`SagaPlan::written_paths`], which drops these.
    /// - the clean-at-HEAD guard, so a file the mutation never touches cannot
    ///   reject it.
    /// - the commit pathspec, so a human's uncommitted edit to it is never
    ///   committed under the mutation's `Mutation-Id`.
    /// - the compensating restore and the intent marker, so a rollback never
    ///   resets it to HEAD and discards that edit.
    ///
    /// Empty for every mutation that writes everything it touches.
    not_written: std::collections::BTreeSet<PathBuf>,
    /// Absolute paths in the member set that git ignores and has never tracked,
    /// written OUTSIDE git.
    ///
    /// Such a path has no HEAD to be clean against, and git refuses to stage
    /// it, so a commit naming one fails the whole mutation. [`SagaPlan::written_paths`]
    /// drops these, so they leave every git step: the clean guard, the no-op
    /// probe, the commit pathspec, the compensating restore, and the intent
    /// marker's member paths. Still written, still recomputed. See
    /// [[spec - git write path - commit-per-mutation as a local saga over the workspace's materialized repos#A git-ignored untracked path is written outside git]].
    outside_git: std::collections::BTreeSet<PathBuf>,
    /// A caller-supplied opaque attribution payload, folded into every commit
    /// this saga makes as trailers beside `Mutation-Id`. Validated against the
    /// reserved keys at the handler boundary. Empty for a mutation carrying no
    /// attribution (and for every refactor today). See
    /// [[spec - git write path - commit-per-mutation as a local saga over the workspace's materialized repos]].
    attribution: Vec<crate::gitwriter::CommitTrailer>,
    /// The directories the mutation creates or must keep, undone on rollback.
    /// Empty for a mutation that touches no directory of its own.
    dirs: DirJournal,
    /// Why git could not classify the touched paths, when it could not.
    ///
    /// The saga refuses before opening, see [`saga_precondition_reject`]. A
    /// path git cannot classify is one it cannot stage or restore either, e.g.
    /// a path beyond a symlink, which `git status` still reports clean. The
    /// clean guard and the no-op probe would pass it, and a rollback would
    /// delete what it cannot restore.
    unclassified: Option<String>,
}

/// The directories a saga acts on, journaled so a rollback puts the directory
/// skeleton back as well as the files.
///
/// Git tracks files, never directories. Restoring the files brings back every
/// directory that holds one, but not an empty directory the op removed, and it
/// never removes an empty directory the op created. Worse, removing a created
/// file prunes every parent it leaves empty, so a directory that existed, empty,
/// before the op would vanish with it. The journal names both sides, planned
/// before the saga opens, so the undo is right however far the write got.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct DirJournal {
    /// Absolute directories the op creates. Undo removes each one left empty,
    /// deepest first.
    created: Vec<PathBuf>,
    /// Absolute directories that must exist after an undo: each one the op
    /// removes, and each existing one a created file's pruning could take.
    kept: Vec<PathBuf>,
}

impl SagaPlan {
    /// The paths of `m` this mutation writes THROUGH GIT, relative to its tree
    /// root: its touched paths minus [`SagaPlan::not_written`] and
    /// [`SagaPlan::outside_git`]. The one path set the clean guard, the commit,
    /// the compensating restore, and the intent marker take.
    ///
    /// Empty when the member holds only paths the mutation does not write. A
    /// caller skips such a member outright: an empty pathspec is NOT an empty
    /// scope to git, `git status -- ` and `git commit -- ` with no path cover the
    /// whole tree.
    fn written_paths(&self, m: &SagaMember) -> Vec<PathBuf> {
        m.paths
            .iter()
            .filter(|p| {
                let abs = m.tree_root.join(p);
                !self.not_written.contains(&abs) && !self.outside_git.contains(&abs)
            })
            .cloned()
            .collect()
    }
}

/// Run a mutation's pre-saga planning off the tokio thread. The saga plan
/// spawns `git check-ignore`, and a refactor's not-written set reads every
/// referrer; the write that follows already runs in its own blocking task.
async fn off_thread<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    tokio::task::spawn_blocking(f)
        .await
        .expect("planning task never panics")
}

/// A reference-rewriting refactor's saga plan, off the tokio thread: the
/// not-written set over `referrers` (reads each one), the consumed-member
/// reject over the rest, then [`resolve_saga_plan`] over `touched` carrying
/// the not-written set. Shared by `promote`, `inline`, and the block-id rename.
async fn refactor_saga_plan(
    handle: &EngineHandle,
    touched: Vec<PathBuf>,
    referrers: BTreeMap<PathBuf, Vec<tb_diagnostics::ByteRange>>,
) -> Result<SagaPlan, crate::mutate::MutationReject> {
    let h = handle.clone();
    off_thread(move || {
        let not_written = referrers_not_written(&referrers);
        if let Some(reject) =
            reject_if_consumed_referrers(&h, &written_referrers(&referrers, &not_written))
        {
            return Err(reject);
        }
        Ok(resolve_saga_plan_writing(&h, &touched, not_written))
    })
    .await
}

/// [`resolve_saga_plan`] off the tokio thread, for a handler with nothing else
/// to plan.
async fn saga_plan_off_thread(handle: &EngineHandle, targets: Vec<PathBuf>) -> SagaPlan {
    let h = handle.clone();
    off_thread(move || resolve_saga_plan(&h, &targets)).await
}

/// Resolve a mutation's member set from the absolute paths it touches.
///
/// Two stages, answering two different questions.
/// - SCOPE, which paths are in play: each path's owning repo (`repo_of`, the
///   deepest owning repo). A path with no owning repo cannot occur for a
///   resolved mutation target and is dropped.
/// - GROUPING, how those paths become members: each repo's enclosing git working
///   tree. Several repos in one tree COALESCE into one member holding the union
///   of their paths, so one mutation makes one commit per tree.
///
/// A repo with no enclosing tree keys on its own root and stays non-git: written,
/// never committed.
///
/// Members are tree-root-ordered and every name list is sorted, so the commit
/// trailer's member list is deterministic.
fn resolve_saga_plan(handle: &EngineHandle, targets: &[PathBuf]) -> SagaPlan {
    resolve_saga_plan_writing(handle, targets, std::collections::BTreeSet::new())
}

/// [`resolve_saga_plan`] for a mutation that leaves some touched paths
/// byte-identical, `not_written`, absolute. They stay members, so the rebuild
/// recomputes them, but no gate asks git about them: a path the op never
/// stages, commits, or restores cannot make it refuse.
fn resolve_saga_plan_writing(
    handle: &EngineHandle,
    targets: &[PathBuf],
    not_written: std::collections::BTreeSet<PathBuf>,
) -> SagaPlan {
    // Stage one: absolute touched paths per owning repo.
    let by_repo = handle
        .read(|v| {
            let mut by_repo: BTreeMap<PathBuf, (crate::repo::RepoName, Vec<PathBuf>)> =
                BTreeMap::new();
            for t in targets {
                if let Some(repo) = v.repos.repo_of(t) {
                    by_repo
                        .entry(repo.root.clone())
                        .or_insert_with(|| (repo.name.clone(), Vec::new()))
                        .1
                        .push(t.clone());
                }
            }
            by_repo
        })
        .value()
        .unwrap_or_default();

    let members = coalesce_by_working_tree(by_repo);
    let (outside_git, unclassified) = match classify_outside_git(&ShellGit, &members, &not_written)
    {
        Ok(set) => (set, None),
        Err(why) => (std::collections::BTreeSet::new(), Some(why)),
    };
    SagaPlan {
        mutation_id: crate::mutate::generate_mutation_id(),
        members,
        not_written,
        outside_git,
        // Set by a handler that carries a caller attribution payload.
        attribution: Vec::new(),
        dirs: DirJournal::default(),
        unclassified,
    }
}

/// The absolute paths across `members` that git ignores and has never tracked,
/// the set [`SagaPlan::outside_git`] carries.
///
/// One `check-ignore` per git member over the touched paths it writes, both
/// sides of a move classified independently. A `not_written` path is never
/// staged, committed, or restored, so it is not asked about. A non-git member has no ignore rules to
/// consult, its paths are already written without committing.
///
/// A classification that fails is an error, never a guess either way.
/// - guessing "outside git" would drop the path from the clean guard unchecked.
/// - guessing "git-recorded" trusts a gate git passes silently: a path beyond a
///   symlink fails `check-ignore`, reads clean to `git status`, then fails
///   `git add`, and a rollback deletes what it cannot restore.
///
/// So the caller refuses the mutation, see [`SagaPlan::unclassified`].
fn classify_outside_git(
    git: &impl GitWriter,
    members: &[SagaMember],
    not_written: &std::collections::BTreeSet<PathBuf>,
) -> Result<std::collections::BTreeSet<PathBuf>, String> {
    let mut out = std::collections::BTreeSet::new();
    for m in members.iter().filter(|m| m.git) {
        let written: Vec<PathBuf> = m
            .paths
            .iter()
            .filter(|p| !not_written.contains(&m.tree_root.join(p)))
            .cloned()
            .collect();
        if written.is_empty() {
            continue;
        }
        let ignored = git
            .ignored_untracked(&m.tree_root, &written)
            .map_err(|e| e.message)?;
        out.extend(ignored.into_iter().map(|p| m.tree_root.join(p)));
    }
    Ok(out)
}

/// The referrers a refactor will NOT write: those whose every reference is
/// commit-pinned, so the rewrite is byte-identical.
///
/// Answers from [`crate::rename::would_rewrite`], the same predicate `ref_edits`
/// skips on, so the planner and the rewriter cannot disagree about which files
/// the mutation writes.
///
/// A referrer whose content cannot be read here is treated as WRITABLE, so it
/// stays gated. The failure direction matters: guessing "writable" costs a
/// conservative rejection, guessing "exempt" would let the mutation write over
/// uncommitted work unchecked.
fn referrers_not_written(
    referrers: &BTreeMap<PathBuf, Vec<tb_diagnostics::ByteRange>>,
) -> std::collections::BTreeSet<PathBuf> {
    referrers
        .iter()
        .filter(|(path, spans)| match std::fs::read_to_string(path) {
            Ok(content) => !crate::rename::would_rewrite(&content, spans),
            Err(_) => false,
        })
        .map(|(path, _)| path.clone())
        .collect()
}

/// The referrers a file MOVE will NOT write: those it leaves byte-identical.
///
/// Answers from [`crate::rename::move_rewrite`], the same predicate the
/// committing write and the rename preview use. That is wider than
/// [`referrers_not_written`]: a move keeping the file's name re-emits a bare
/// `[[name]]` identically, so a referrer holding only bare-name links is not
/// written either, not only an all-pinned one.
///
/// Conservative in the failure direction, each case stays WRITABLE, so it stays
/// gated:
/// - an unreadable referrer.
/// - a referrer whose bytes drifted from the held hash, its spans are unreliable.
/// - a rewrite error, the write will reject with the message.
///
/// The moved file itself is never not-written, whatever its self-references do:
/// the move deletes it at `moved`, so it must ride the commit.
fn move_referrers_not_written(
    referrers: &BTreeMap<PathBuf, Vec<tb_diagnostics::ByteRange>>,
    held: &BTreeMap<PathBuf, Option<crate::ir::ContentHash>>,
    moved: &Path,
    old_rel: &Path,
    new_rel: &Path,
) -> std::collections::BTreeSet<PathBuf> {
    referrers
        .iter()
        .filter(|(path, _)| path.as_path() != moved)
        .filter(|(path, spans)| {
            let Ok(content) = std::fs::read_to_string(path) else {
                return false;
            };
            let hash = crate::ir::ContentHash::of(content.as_bytes());
            held.get(*path).copied().flatten() == Some(hash)
                && matches!(
                    crate::rename::move_rewrite(&content, spans, old_rel, new_rel),
                    Ok(None)
                )
        })
        .map(|(path, _)| path.clone())
        .collect()
}

/// The referrer paths a refactor WILL rewrite: its whole referrer set minus the
/// commit-pinned, byte-identical ones. The write-set the editability gate below
/// classifies, so a pinned referrer in a consumed member never blocks.
fn written_referrers(
    referrers: &BTreeMap<PathBuf, Vec<tb_diagnostics::ByteRange>>,
    not_written: &std::collections::BTreeSet<PathBuf>,
) -> std::collections::BTreeSet<PathBuf> {
    referrers
        .keys()
        .filter(|p| !not_written.contains(*p))
        .cloned()
        .collect()
}

/// The written referrers that live in a consumed (non-editable) member, grouped
/// by owning member. A refactor rewrites referrers to keep the mounted-set graph
/// consistent, but it may not author a member it does not own, so any such
/// referrer blocks the whole refactor.
fn consumed_written_referrers(
    v: &KnowledgeBase,
    written: &std::collections::BTreeSet<PathBuf>,
) -> BTreeMap<crate::repo::RepoName, Vec<PathBuf>> {
    let mut blocking: BTreeMap<crate::repo::RepoName, Vec<PathBuf>> = BTreeMap::new();
    for path in written {
        if let Some(repo) = v.repos.repo_of(path) {
            if !crate::wire::member_role(v, &repo.name).editable() {
                blocking
                    .entry(repo.name.clone())
                    .or_default()
                    .push(path.clone());
            }
        }
    }
    for paths in blocking.values_mut() {
        paths.sort();
    }
    blocking
}

/// Reject a refactor when any referrer it would rewrite lives in a consumed
/// member. The `detail` carries the blocking `(member -> files)` list so a
/// consumer renders the choice: promote the member to `edit`, or (the tracked
/// future opt-in) skip it and leave an advisory dangling reference.
fn reject_if_consumed_referrers(
    handle: &EngineHandle,
    written: &std::collections::BTreeSet<PathBuf>,
) -> Option<crate::mutate::MutationReject> {
    let blocking = handle
        .read(|v| consumed_written_referrers(v, written))
        .value()
        .unwrap_or_default();
    if blocking.is_empty() {
        return None;
    }
    let members: Vec<String> = blocking.keys().map(|n| n.as_str().to_string()).collect();
    let mut detail = serde_json::Map::new();
    for (member, paths) in &blocking {
        detail.insert(
            member.as_str().to_string(),
            serde_json::Value::from(
                paths
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>(),
            ),
        );
    }
    Some(crate::mutate::MutationReject {
        message: format!(
            "cannot rewrite references in consumed member(s) {} — a refactor may not author a \
             member it does not own; list them in workspace.yaml 'edit:' to rewrite their references",
            members.join(", "),
        ),
        detail: Some(serde_json::json!({ "blocking_consumed_referrers": detail })),
    })
}

/// Group per-repo absolute paths into saga members, one per git working tree.
///
/// The grouping half of [`resolve_saga_plan`], split out because it is the whole
/// correctness claim and is testable without an engine: several repos inside one
/// tree must come back as ONE member, or a mutation lands N commits sharing a
/// `Mutation-Id` and compensation can only reach the newest.
fn coalesce_by_working_tree(
    by_repo: BTreeMap<PathBuf, (crate::repo::RepoName, Vec<PathBuf>)>,
) -> Vec<SagaMember> {
    type Acc = (
        std::collections::BTreeSet<crate::repo::RepoName>,
        Vec<PathBuf>,
        bool,
    );
    let mut by_tree: BTreeMap<PathBuf, Acc> = BTreeMap::new();
    for (repo_root, (name, abs_paths)) in by_repo {
        let (root, git) = match crate::gitwriter::working_tree_of(&repo_root) {
            Some(tree) => (tree, true),
            None => (repo_root, false),
        };
        let entry = by_tree
            .entry(root.clone())
            .or_insert_with(|| (std::collections::BTreeSet::new(), Vec::new(), git));
        entry.0.insert(name);
        for abs in abs_paths {
            // Rebase onto the tree, the basis the commit takes. `repo_of` put the
            // path inside the repo and the tree encloses the repo, so this holds.
            if let Ok(rel) = abs.strip_prefix(&root) {
                entry.1.push(rel.to_path_buf());
            }
        }
    }
    by_tree
        .into_iter()
        .map(|(tree_root, (names, paths, git))| SagaMember {
            tree_root,
            repo_names: names.into_iter().collect(),
            paths,
            git,
        })
        .collect()
}

/// The absolute paths a saga touches across every member, the dirty set the
/// single post-settle rebuild reads. Spans all members, git and non-git alike: a
/// non-git member's written file is still re-read into the IR. One rebuild over
/// this whole set bumps the version once, so a multi-repo mutation is observed as
/// one change after the saga settles, never per-repo mid-saga.
fn saga_dirty_set(plan: &SagaPlan) -> std::collections::BTreeSet<PathBuf> {
    plan.members
        .iter()
        .flat_map(|m| m.paths.iter().map(|p| m.tree_root.join(p)))
        .collect()
}

/// The type-def named `name`, the rename target.
fn find_owned_type_def<'a>(
    v: &'a crate::ir::KnowledgeBase,
    name: &tb_core::TypeName,
    in_repo: Option<&str>,
) -> Option<&'a tb_core::TypeDef> {
    for repo in v.repos.repos() {
        // A `::repo`-qualified target selects ONE owner's identity; without it,
        // the first owner across the mounted set wins (the historical behaviour).
        if in_repo.is_some_and(|want| repo.name.as_str() != want) {
            continue;
        }
        if let Some(td) = v.graphs.of(&repo.name).get(name) {
            return Some(td);
        }
    }
    None
}

/// The path a type-def file takes when its type is renamed to `new_name`. The
/// type-name derives from the filename's stem before its known suffix, so the
/// rename swaps that stem and keeps the suffix and directory. The inverse of
/// [`tb_core::type_name_from_path`].
fn renamed_type_def_path(from: &Path, new_name: &tb_core::TypeName) -> Option<PathBuf> {
    let file_name = from.file_name()?.to_str()?;
    let suffix = [".type.yaml", ".type.yml", ".yaml", ".yml"]
        .into_iter()
        .find(|s| file_name.ends_with(s))?;
    Some(from.with_file_name(format!("{}{suffix}", new_name.as_str())))
}

/// Everything `rename_type` gathers under the read lock, before the saga writes.
/// Carries the def's file relocation and every referrer to rewrite. The
/// committing saga and its preview both start from this one plan, so the two
/// cannot disagree about which files the cascade touches.
#[derive(Clone)]
struct RenameTypeGather {
    /// The def's current path.
    from: PathBuf,
    /// The def's new path.
    to: PathBuf,
    /// The repo root the def's file move belongs to, for the `Moved:` record.
    move_repo_root: PathBuf,
    /// The def's repo-relative paths, for the wikilink-surface rewrite.
    old_rel: PathBuf,
    new_rel: PathBuf,
    /// The old and new names, the same in every reject message and commit.
    old_name: tb_core::TypeName,
    new_name: tb_core::TypeName,
    /// Type-name-reference edits per referrer (surface 1), each tagged with what
    /// the reference is.
    type_edits: BTreeMap<PathBuf, Vec<crate::typerefs::TypeRefEdit>>,
    /// Wikilink-to-def spans per referrer (surface 2).
    link_spans: BTreeMap<PathBuf, Vec<tb_diagnostics::ByteRange>>,
    /// Each referrer's held content hash, the write-time re-read guard.
    referrer_hashes: BTreeMap<PathBuf, Option<crate::ir::ContentHash>>,
    /// Every referrer path to rewrite, deduped across both surfaces.
    referrers: Vec<PathBuf>,
    /// The member owning each path the cascade may write, the def and every
    /// referrer, so an affected-path report names a member without a second read.
    repos: BTreeMap<PathBuf, crate::repo::RepoName>,
}

impl RenameTypeGather {
    /// The paths the saga touches: the def's own file (`from` plus the
    /// not-yet-existing `to`), and each referrer. Deduped, path-sorted.
    fn touched(&self) -> Vec<PathBuf> {
        let mut set: std::collections::BTreeSet<PathBuf> = std::collections::BTreeSet::new();
        set.insert(self.from.clone());
        set.insert(self.to.clone());
        set.extend(self.referrers.iter().cloned());
        set.into_iter().collect()
    }

    /// Split the referrers into those the cascade writes and those it leaves
    /// byte-identical, remembering which of them re-spell a wikilink.
    ///
    /// `rename_type` writes a referrer for either of two reasons, so being left
    /// unwritten needs both to be absent: a type-name CLAIM edit rewrites the
    /// file even when every wikilink in it is frozen. Unreadable reads as
    /// WRITTEN, so it stays gated: guessing exempt would write over uncommitted
    /// work unchecked.
    fn written_and_exempt(&self) -> RenameTypeWrites {
        let mut writes = RenameTypeWrites::default();
        for path in &self.referrers {
            let claim_edit = self.type_edits.get(path).is_some_and(|e| !e.is_empty());
            let link_edit = self.link_spans.get(path).is_some_and(|spans| {
                read_referrer_checked(path, self.referrer_hashes.get(path).copied().flatten())
                    .map(|c| crate::rename::would_rewrite(&c, spans))
                    .unwrap_or(true)
            });
            if link_edit {
                writes.links.insert(path.clone());
            }
            if claim_edit || link_edit {
                writes.written.insert(path.clone());
            } else {
                writes.exempt.insert(path.clone());
            }
        }
        writes.exempt.retain(|p| !writes.written.contains(p));
        writes
    }

    /// How a written path changes: the def's move, plus the tagged type-name
    /// edits and the wikilink re-spellings there. A self-referencing def reports
    /// both its move and its own edits.
    fn changes_at(&self, path: &Path, writes: &RenameTypeWrites) -> Vec<&'static str> {
        let mut changes: Vec<&'static str> = Vec::new();
        if path == self.from {
            changes.push("definition_move");
        }
        if let Some(edits) = self.type_edits.get(path) {
            changes.extend(edits.iter().map(|edit| edit.kind.as_str()));
        }
        if writes.links.contains(path) {
            changes.push("wikilink");
        }
        changes.sort_unstable();
        changes.dedup();
        changes
    }
}

/// The split of a `rename_type`'s referrers: what the cascade writes, what it
/// leaves byte-identical, and which referrers re-spell a wikilink.
#[derive(Default)]
struct RenameTypeWrites {
    written: std::collections::BTreeSet<PathBuf>,
    exempt: std::collections::BTreeSet<PathBuf>,
    /// Referrers whose wikilink to the def actually re-spells, so a frozen one
    /// is not reported as a link change.
    links: std::collections::BTreeSet<PathBuf>,
}

/// Plan a `rename_type` over the held knowledge base: the owned def file, the
/// new path, and both reference surfaces. Surface 1 (type-name references) is
/// repo-aware and its edits derive from each referrer's parse (claim spans, raw
/// shapes). Surface 2 (wikilinks to the def file) is the backlink index,
/// mounted-set scope, cross-repo referrers included. Each referrer's content
/// hash is held so the write-time re-read is guarded.
///
/// Shared by the committing mutation and its preview, so neither the refusals
/// nor the touched set can drift. The old name may be `base::repo` to select
/// WHICH owner's identity to rename when two mounted repos own the name; bare
/// picks the first owner. The new name is always the owner's own, so it stays
/// bare.
fn plan_rename_type(
    v: &crate::ir::KnowledgeBase,
    old_name_arg: &str,
    new_name_arg: &str,
) -> Result<RenameTypeGather, String> {
    let (old_base, old_repo) = match old_name_arg.split_once("::") {
        Some((base, repo)) => (base.to_string(), Some(repo.to_string())),
        None => (old_name_arg.to_string(), None),
    };
    let old_name = tb_core::TypeName(old_base);
    let new_name = tb_core::TypeName(new_name_arg.to_string());
    let def =
        find_owned_type_def(v, &old_name, old_repo.as_deref()).ok_or_else(|| match &old_repo {
            Some(r) => format!(
                "no type-def named '{}' owned by '{}' to rename",
                old_name.as_str(),
                r
            ),
            None => format!("no type-def named '{}' to rename", old_name.as_str()),
        })?;
    let from = def.source_path.clone();
    let repo = v
        .repos
        .repo_of(&from)
        .ok_or_else(|| "the type-def's repo is not mounted".to_string())?;
    let repo_name = repo.name.clone();
    let repo_root = repo.root.clone();
    // The def file is a PRIMARY write derived from the graph, not a path
    // arg, so it bypasses `resolve_editable_target`. Gate it here: a
    // type-def owned by a consumed (`discover` / `dep`) member is not the
    // engine's to author.
    let role = crate::wire::member_role(v, &repo_name);
    if !role.editable() {
        return Err(format!(
            "type-def '{}' is owned by '{}', mounted as '{}', not editable — \
             list it in workspace.yaml 'edit:' to rename its types",
            old_name.as_str(),
            repo_name.as_str(),
            role.as_str(),
        ));
    }
    if !tb_core::is_valid_type_name(new_name.as_str()) {
        return Err(format!("'{}' is not a valid type name", new_name.as_str()));
    }
    if v.graphs.of(&repo_name).get(&new_name).is_some() {
        return Err(format!(
            "type-def '{}' already exists — pick a free name",
            new_name.as_str()
        ));
    }
    let to = renamed_type_def_path(&from, &new_name)
        .ok_or_else(|| format!("could not derive the new path for '{}'", new_name.as_str()))?;
    let old_rel = from.strip_prefix(&repo_root).unwrap_or(&from).to_path_buf();
    let new_rel = to.strip_prefix(&repo_root).unwrap_or(&to).to_path_buf();

    let mut type_edits: BTreeMap<PathBuf, Vec<crate::typerefs::TypeRefEdit>> = BTreeMap::new();
    let mut referrer_hashes: BTreeMap<PathBuf, Option<crate::ir::ContentHash>> = BTreeMap::new();
    let mut repos: BTreeMap<PathBuf, crate::repo::RepoName> = BTreeMap::new();
    repos.insert(from.clone(), repo_name.clone());
    for path in v.type_referrers(&repo_name, &old_name) {
        if let Some(entry) = v.catalog.get(&path) {
            // Each referrer rewrites against its OWN repo: a bare `foo`
            // in the def's repo becomes bare `new`, a mounted
            // `foo::<def repo>` in a peer becomes `new::<def repo>`,
            // qualifier-preserving.
            let file_repo = crate::refnames::source_repo(&v.repos, &path);
            let edits = crate::typerefs::type_ref_edits(
                &entry.parse,
                file_repo.as_str(),
                &old_name,
                repo_name.as_str(),
                &new_name,
            );
            if !edits.is_empty() {
                referrer_hashes.entry(path.clone()).or_insert(entry.hash);
                note_repo(&mut repos, v, &path);
                type_edits.insert(path, edits);
            }
        }
    }

    let mut link_spans: BTreeMap<PathBuf, Vec<tb_diagnostics::ByteRange>> = BTreeMap::new();
    for bl in v.backlinks(&from) {
        link_spans
            .entry(bl.source.clone())
            .or_default()
            .push(bl.span);
        referrer_hashes
            .entry(bl.source.clone())
            .or_insert_with(|| v.catalog.get(&bl.source).and_then(|e| e.hash));
        note_repo(&mut repos, v, &bl.source);
    }

    // Every referrer to rewrite, deduped across both surfaces.
    let referrers: Vec<PathBuf> = type_edits
        .keys()
        .chain(link_spans.keys())
        .cloned()
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();

    Ok(RenameTypeGather {
        from,
        to,
        move_repo_root: repo_root,
        old_rel,
        new_rel,
        old_name,
        new_name,
        type_edits,
        link_spans,
        referrer_hashes,
        referrers,
        repos,
    })
}

/// Record the member owning `path`, so an affected-path report names it.
fn note_repo(
    repos: &mut BTreeMap<PathBuf, crate::repo::RepoName>,
    v: &crate::ir::KnowledgeBase,
    path: &Path,
) {
    if let Some(repo) = v.repos.repo_of(path) {
        repos.insert(path.to_path_buf(), repo.name.clone());
    }
}

/// The moves a saga's commits record, keyed by repo root, so each member's
/// commit carries only its own `Moved:` trailers. A rename's referrers can span
/// repos, so the move belongs only to the repo where the file actually moved.
/// Empty for a non-move mutation.
type MovesByTree = std::collections::BTreeMap<PathBuf, Vec<crate::gitwriter::MovedRecord>>;

/// The `Moved:` record for a file move, keyed for the commit that will carry it.
///
/// Keyed by the WORKING TREE holding the repo, with the paths rebased onto that
/// tree. That is the commit's own basis: `settle_saga` looks these up by the
/// member's `tree_root`, and a repo nested in a larger tree commits into the
/// tree. Keyed by the repo root instead, a nested repo's rename would find no
/// entry and silently carry no trailer, breaking the forward trace without
/// failing anything.
///
/// The trace reader (`moved_trailers_since`) is not yet surfaced, so this basis
/// is established before it has a consumer rather than migrated after.
fn moves_for(repo_root: &Path, old_rel: &Path, new_rel: &Path) -> MovesByTree {
    let tree =
        crate::gitwriter::working_tree_of(repo_root).unwrap_or_else(|| repo_root.to_path_buf());
    let onto_tree = |rel: &Path| {
        repo_root
            .join(rel)
            .strip_prefix(&tree)
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|_| rel.to_path_buf())
    };
    let record = crate::gitwriter::MovedRecord {
        from: onto_tree(old_rel),
        to: onto_tree(new_rel),
    };
    let mut moves = MovesByTree::new();
    moves.insert(tree, vec![record]);
    moves
}

/// What a settled saga reports: the commits it made and the paths it wrote
/// outside any commit.
#[derive(Debug, Default)]
struct SagaCommits {
    /// One commit per committing member, each paired with the repo that
    /// committed it. Name-sorted by member. Empty when nothing committed (every
    /// member off-git, or a no-op that wrote nothing). The mutate result
    /// projects this as `commits: { [repo]: sha }`, and derives the `commit`
    /// scalar from the response file's own repo.
    commits: Vec<(crate::repo::RepoName, CommitSha)>,
    /// The paths written outside git, absolute and path-sorted, see
    /// [`saga_untracked_files`]. The mutate result's `untracked_files`.
    untracked_files: Vec<PathBuf>,
}

impl SagaCommits {
    fn iter(&self) -> std::slice::Iter<'_, (crate::repo::RepoName, CommitSha)> {
        self.commits.iter()
    }

    fn is_empty(&self) -> bool {
        self.commits.is_empty()
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.commits.len()
    }
}

/// The paths a saga writes outside git: the ignored, untracked paths it
/// writes, absolute and path-sorted. A path it only recomputes is not listed.
/// Shared by the committing saga and the preview, so the two report one set.
fn saga_untracked_files(plan: &SagaPlan) -> Vec<PathBuf> {
    plan.outside_git
        .difference(&plan.not_written)
        .cloned()
        .collect()
}

/// `apply_saga` for a mutation that records no moves, the common case.
///
/// `require_git` rejects the whole saga up front if any member is non-git: a
/// structural refactor rewrites references across its members and compensates
/// git-only, so a non-git member could not be rolled back. A simple single-file
/// write passes `false`, its write-without-commit fallback is intended.
fn apply_saga<T>(
    git: &impl GitWriter,
    marker_dir: &Path,
    plan: &SagaPlan,
    summary: String,
    require_git: bool,
    write: impl FnOnce() -> Result<T, MutationReject>,
) -> Result<(T, SagaCommits), MutationReject> {
    apply_saga_with_moves(
        git,
        marker_dir,
        plan,
        summary,
        &MovesByTree::new(),
        require_git,
        write,
    )
}

/// The members a saga would write that the caller's grant does not cover, as a refusal.
///
/// ADR-0003 said the grant must be checked against the plan actually executed rather than a
/// preview. `rename_type` does that for its own cascade; this is the same rule for any saga
/// whose args carry `allowed_members`, and it reads the PLAN, so a referrer in another member
/// cannot be rewritten outside the grant. An empty list is the absence of a grant, not a grant
/// of nothing: a launch without a policy sends nothing and is unchanged.
fn grant_refusal(
    plan: &SagaPlan,
    allowed: &[String],
    verb: &str,
) -> Option<crate::mutate::MutationReject> {
    if allowed.is_empty() {
        return None;
    }
    let mut outside: Vec<String> = Vec::new();
    for member in &plan.members {
        for repo in &member.repo_names {
            if !allowed.iter().any(|name| name == repo.as_str())
                && !outside.iter().any(|seen| seen == repo.as_str())
            {
                outside.push(repo.as_str().to_string());
            }
        }
    }
    if outside.is_empty() {
        return None;
    }
    outside.sort();
    Some(crate::mutate::MutationReject::new(format!(
        "policy/member_grant: {verb} rewrites {} member(s) outside the grant: {}",
        outside.len(),
        outside.join(", ")
    )))
}

/// Every precondition a saga checks before it opens, `None` when all hold.
///
/// - git classified every touched path, see [`SagaPlan::unclassified`].
/// - with `require_git`, a git working tree covers every member, see
///   [`require_git_reject`].
///
/// The one gate [`apply_saga_with_moves`] runs, and every preview runs it with
/// its op's `require_git`, so a preview refuses where the mutation would.
fn saga_precondition_reject(plan: &SagaPlan, require_git: bool) -> Option<MutationReject> {
    if let Some(why) = &plan.unclassified {
        return Some(MutationReject::new(format!(
            "git cannot classify a path this mutation writes, so it could neither stage nor roll it back ({why}). Refusing. A path under a symlinked directory is the usual cause"
        )));
    }
    if require_git {
        return require_git_reject(plan);
    }
    None
}

/// The require-git precondition, `None` when it holds.
///
/// A structural refactor rewrites references across its member set and a folder
/// op acts file by file, and both compensate git-only (revert-by-checkout), so a
/// member written mid-saga with no working tree to revert could not be rolled
/// back. An all-or-nothing reject before the saga opens, never a half-apply.
///
/// A member nested inside a larger working tree IS covered by it, so this
/// refuses only a member with no enclosing tree anywhere, not every repo lacking
/// a `.git` of its own. A member holding only paths the op leaves byte-identical
/// is never written, so it has nothing to roll back and never refuses. Shared by
/// the saga and the previews of the ops it binds, so a preview refuses where the
/// op would.
fn require_git_reject(plan: &SagaPlan) -> Option<MutationReject> {
    let non_git: Vec<&str> = plan
        .members
        .iter()
        .filter(|m| !m.git && !plan.written_paths(m).is_empty())
        .flat_map(|m| m.repo_names.iter().map(|n| n.as_str()))
        .collect();
    (!non_git.is_empty()).then(|| {
        MutationReject::new(format!(
            "mutation spans a member no git working tree covers ({}): compensation is git-only, so a mid-saga failure could not be rolled back. Refusing. Put it under a working tree, its own root or any ancestor",
            non_git.join(", ")
        ))
    })
}

/// Apply a mutation as one local saga over its member set: the cross-repo
/// clean-at-HEAD precondition, the durable intent marker, the write, then
/// commit-each-correlated, with git as the compensation on failure. Returns the
/// write's value and the commits made, one per committing member. Single-repo is
/// the N=1 case, a non-git member writes without committing.
///
/// `git` is the swappable mechanism; production passes `&ShellGit`. The port is
/// injected so the saga's compensation logic is testable against a writer that
/// fails a chosen member's commit. `marker_dir` is [`saga_marker_dir`] (the launch
/// entry's `.arsumbris/au-engine/run/`), a single per-daemon location, where the intent marker opens and
/// closes the saga across all its members. `moves` records each member's renames
/// as `Moved:` trailers for the pinned-reference forward trace.
fn apply_saga_with_moves<T>(
    git: &impl GitWriter,
    marker_dir: &Path,
    plan: &SagaPlan,
    summary: String,
    moves: &MovesByTree,
    require_git: bool,
    write: impl FnOnce() -> Result<T, MutationReject>,
) -> Result<(T, SagaCommits), MutationReject> {
    if let Some(reject) = saga_precondition_reject(plan, require_git) {
        return Err(reject);
    }

    // Only git working trees commit; a non-git member is written but not committed.
    let git_members: Vec<&SagaMember> = plan.members.iter().filter(|m| m.git).collect();
    // The complete member set every commit carries, so any one commit names all
    // committing repos. One member can cover several repos (a monorepo tree), so
    // this is the union across members.
    //
    // Sorted by NAME, not left in member order. Members are tree-ordered, which
    // is an implementation detail of the grouping and would put the trailer's
    // names in an order no reader can predict. This trailer is read by a human in
    // `git log` and parsed by crash recovery, so it is name-sorted for both.
    let mut member_names: Vec<crate::repo::RepoName> = git_members
        .iter()
        .flat_map(|m| m.repo_names.iter().cloned())
        .collect();
    member_names.sort();

    // Cross-repo clean precondition: every touched path in every member clean at
    // HEAD, an all-or-nothing early reject before the saga opens, before any
    // tree is touched.
    for m in &git_members {
        // Only the paths the mutation will actually write. A referrer the
        // refactor leaves byte-identical cannot have its work clobbered, so
        // gating on it would let a file the mutation never touches reject it.
        let checked = plan.written_paths(m);
        if checked.is_empty() {
            continue;
        }
        let clean = git
            .is_clean(&m.tree_root, &checked)
            .map_err(|e| MutationReject::new(format!("git precondition failed: {}", e.message)))?;
        if !clean {
            return Err(MutationReject::new(
                "path has uncommitted changes — the engine is the writer; commit or discard them first",
            ));
        }
    }

    // Park every outside-git path's prior state BEFORE the marker names it, so
    // the marker never points at a half-written copy.
    let preimages = park_preimages(marker_dir, plan).map_err(|e| {
        drop_preimages(marker_dir, &plan.mutation_id);
        MutationReject::new(format!(
            "could not open the saga: parking a path outside git failed: {e}"
        ))
    })?;

    // Open the saga: write the intent marker before any working tree is touched.
    let marker = write_intent_marker(marker_dir, plan, &preimages).map_err(|e| {
        drop_preimages(marker_dir, &plan.mutation_id);
        MutationReject::new(format!(
            "could not open the saga: intent marker write failed: {e}"
        ))
    })?;

    // The body settles to commit-all or compensate-all, then the marker closes.
    let outcome = settle_saga(
        git,
        &git_members,
        &member_names,
        plan,
        &summary,
        moves,
        marker_dir,
        &preimages,
        write,
    );
    // The marker first, then the parked copies: a crash between the two leaves
    // orphaned copies the next startup sweeps, never a marker whose copies are
    // gone.
    remove_intent_marker(&marker);
    drop_preimages(marker_dir, &plan.mutation_id);
    outcome
}

/// The settling body of a saga: apply every write, commit each member correlated
/// by the shared `Mutation-Id`, and compensate every member on any failure, a
/// write failure or a commit failure alike. Always settles before returning, so
/// the caller can close the intent marker.
// Nine inputs are the saga's own state, passed once by its only caller; a struct
// would restate that list at the one call site instead of naming the arguments.
#[allow(clippy::too_many_arguments)]
fn settle_saga<T>(
    git: &impl GitWriter,
    git_members: &[&SagaMember],
    member_names: &[crate::repo::RepoName],
    plan: &SagaPlan,
    summary: &str,
    moves: &MovesByTree,
    marker_dir: &Path,
    preimages: &[Preimage],
    write: impl FnOnce() -> Result<T, MutationReject>,
) -> Result<(T, SagaCommits), MutationReject> {
    let mutation_id = MutationId(plan.mutation_id.clone());

    // Apply every write across all members' working trees. A partial write
    // compensates every member, then rejects.
    let value = match write() {
        Ok(v) => v,
        Err(reject) => {
            let mut errors = compensate(git, &[], git_members, plan, &mutation_id);
            errors.extend(restore_preimages(preimages));
            errors.extend(undo_dirs(&plan.dirs));
            return Err(note_incomplete_rollback(
                marker_dir,
                &plan.mutation_id,
                errors,
                reject,
            ));
        }
    };

    // Commit each member, correlated by the shared Mutation-Id.
    let mut commits = SagaCommits {
        commits: Vec::new(),
        untracked_files: saga_untracked_files(plan),
    };
    let mut committed: Vec<&SagaMember> = Vec::new();
    for m in git_members {
        // A no-op write leaves the member's paths clean at HEAD: no diff to
        // commit. Skip it rather than letting `git commit` fail on an empty diff,
        // the spec's "a no-op write makes no commit". An is_clean error is not a
        // clean signal, so fall through and let the commit decide.
        let written = plan.written_paths(m);
        if written.is_empty() || git.is_clean(&m.tree_root, &written).unwrap_or(false) {
            continue;
        }
        let message = CommitMessage {
            summary: summary.to_string(),
            mutation_id: mutation_id.clone(),
            members: member_names.to_vec(),
            moves: committed_moves(moves, plan, &m.tree_root),
            reverts: None,
            attribution: plan.attribution.clone(),
        };
        match git.commit(&m.tree_root, &written, &message) {
            Ok(sha) => {
                // The wire serves `commits: { [repo]: sha }`, so every repo this
                // tree covers names the one commit that carries its change. A
                // coalesced member yields several entries sharing a sha.
                for name in &m.repo_names {
                    commits.commits.push((name.clone(), sha.clone()));
                }
                committed.push(*m);
            }
            Err(e) => {
                // Compensate: revert those already committed by Mutation-Id, then
                // restore every member's paths to HEAD. A compensation error is
                // recorded durably, never swallowed.
                let mut errors = compensate(git, &committed, git_members, plan, &mutation_id);
                errors.extend(restore_preimages(preimages));
                errors.extend(undo_dirs(&plan.dirs));
                let reject = MutationReject::new(format!("commit failed: {}", e.message));
                return Err(note_incomplete_rollback(
                    marker_dir,
                    &plan.mutation_id,
                    errors,
                    reject,
                ));
            }
        }
    }
    Ok((value, commits))
}

/// The marker line tag for a path written outside git. Never an absolute path,
/// so it cannot collide with a member line's tree root.
const PREIMAGE_LINE: &str = "preimage";

/// The marker line tag for a directory the saga creates, see [`DirJournal`].
const DIR_CREATED_LINE: &str = "dir-created";

/// The marker line tag for a directory a rollback must leave in place.
const DIR_KEPT_LINE: &str = "dir-kept";

/// Put a saga's directory skeleton back, after its files are restored, returning
/// the directories it could not. Removes each created directory left empty,
/// deepest first, then ensures every kept one exists. Both halves are idempotent,
/// so a partial write, a repeat after a crash mid-rollback, and a directory the
/// write never reached all land the same skeleton.
///
/// A created directory still holding something after the files are restored
/// holds content the saga did not write, so it is left in place and reported,
/// never removed.
fn undo_dirs(journal: &DirJournal) -> Vec<(PathBuf, String)> {
    let mut errors = Vec::new();
    let mut created = journal.created.clone();
    created.sort();
    // Reverse path order visits every child before its parent.
    for dir in created.iter().rev() {
        match std::fs::remove_dir(dir) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => errors.push((
                dir.clone(),
                format!("removing a directory the saga created failed: {e}"),
            )),
        }
    }
    for dir in &journal.kept {
        if let Err(e) = std::fs::create_dir_all(dir) {
            errors.push((
                dir.clone(),
                format!("recreating a directory the saga removed failed: {e}"),
            ));
        }
    }
    errors
}

/// A marker directory line back into the journal it belongs to, `false` for any
/// other line.
fn parse_dir_line(line: &str, journal: &mut DirJournal) -> bool {
    let Some((tag, path)) = line.split_once('\t') else {
        return false;
    };
    match tag {
        DIR_CREATED_LINE => journal.created.push(PathBuf::from(path)),
        DIR_KEPT_LINE => journal.kept.push(PathBuf::from(path)),
        _ => return false,
    }
    true
}

/// One path the saga writes outside git, and how to put it back.
///
/// Git cannot roll such a path back, it has no HEAD, so the saga parks its
/// prior state before the write: a copy of its bytes when it exists, nothing
/// when it does not. One rule covers every op without knowing which ran, a
/// move parks the source and records the destination absent, an overwrite or a
/// delete parks the file, a creation records it absent. See
/// [[spec - git write path - commit-per-mutation as a local saga over the workspace's materialized repos#A git-ignored untracked path is written outside git]].
#[derive(Debug, Clone, PartialEq, Eq)]
struct Preimage {
    /// The working tree the path sits in.
    tree_root: PathBuf,
    /// The path, relative to `tree_root`.
    path: PathBuf,
    /// The parked copy of its prior bytes, `None` when it did not exist.
    parked: Option<PathBuf>,
}

/// The directory a saga parks its outside-git copies in, beside the marker.
fn preimage_dir(marker_dir: &Path, mutation_id: &str) -> PathBuf {
    marker_dir.join(format!("preimage.{mutation_id}"))
}

/// Park the prior state of every path the saga writes outside git, before any
/// is touched. A path the saga does not write is skipped, restoring it would
/// discard a human's edit. Each copy is fsynced, so a crash after the marker
/// names it finds the whole prior bytes.
fn park_preimages(marker_dir: &Path, plan: &SagaPlan) -> std::io::Result<Vec<Preimage>> {
    let dir = preimage_dir(marker_dir, &plan.mutation_id);
    let mut out = Vec::new();
    for m in plan.members.iter().filter(|m| m.git) {
        for p in &m.paths {
            let abs = m.tree_root.join(p);
            if !plan.outside_git.contains(&abs) || plan.not_written.contains(&abs) {
                continue;
            }
            let parked = if abs.is_file() {
                crate::ignored_dir::ensure_git_ignored_dir(marker_dir)?;
                std::fs::create_dir_all(&dir)?;
                let copy = dir.join(out.len().to_string());
                std::fs::copy(&abs, &copy)?;
                std::fs::File::open(&copy)?.sync_all()?;
                Some(copy)
            } else {
                None
            };
            out.push(Preimage {
                tree_root: m.tree_root.clone(),
                path: p.clone(),
                parked,
            });
        }
    }
    Ok(out)
}

/// Put every outside-git path back to its parked state, returning the paths it
/// could not restore. A parked copy is copied back, never moved, so restoring
/// twice (a crash mid-rollback, then recovery) lands the same bytes. A path
/// that did not exist is removed, with the empty directories its creation left.
fn restore_preimages(preimages: &[Preimage]) -> Vec<(PathBuf, String)> {
    let mut errors = Vec::new();
    for pi in preimages {
        let abs = pi.tree_root.join(&pi.path);
        let restored = match &pi.parked {
            Some(parked) => abs
                .parent()
                .map_or(Ok(()), std::fs::create_dir_all)
                .and_then(|()| std::fs::copy(parked, &abs).map(|_| ())),
            None => match std::fs::remove_file(&abs) {
                Ok(()) => {
                    crate::gitwriter::prune_empty_parents(&pi.tree_root, &pi.path);
                    Ok(())
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e),
            },
        };
        if let Err(e) = restored {
            errors.push((abs, format!("restoring the path outside git failed: {e}")));
        }
    }
    errors
}

/// Drop a saga's parked copies, once nothing can need them.
fn drop_preimages(marker_dir: &Path, mutation_id: &str) {
    let _ = std::fs::remove_dir_all(preimage_dir(marker_dir, mutation_id));
}

/// Drop every saga's parked copies, the startup sweep after recovery.
fn sweep_preimages(marker_dir: &Path) {
    let Ok(entries) = std::fs::read_dir(marker_dir) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.file_name().to_string_lossy().starts_with("preimage.") {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// A marker `preimage` line back into its [`Preimage`], `None` for any other
/// line.
fn parse_preimage_line(line: &str) -> Option<Preimage> {
    let mut fields = line.split('\t');
    if fields.next()? != PREIMAGE_LINE {
        return None;
    }
    let tree_root = PathBuf::from(fields.next()?);
    let path = PathBuf::from(fields.next()?);
    let parked = fields.next().filter(|s| !s.is_empty()).map(PathBuf::from);
    Some(Preimage {
        tree_root,
        path,
        parked,
    })
}

/// The `Moved:` records a member's commit carries: its tree's moves, minus any
/// with either side outside git. A source git never tracked has no committed
/// history for a forward trace to follow from, and a destination git never
/// records has none to follow to, so either record would chain through a path
/// history does not hold.
fn committed_moves(
    moves: &MovesByTree,
    plan: &SagaPlan,
    tree_root: &Path,
) -> Vec<crate::gitwriter::MovedRecord> {
    moves
        .get(tree_root)
        .map(|records| {
            records
                .iter()
                .filter(|r| {
                    !plan.outside_git.contains(&tree_root.join(&r.from))
                        && !plan.outside_git.contains(&tree_root.join(&r.to))
                })
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

/// The saga intent marker's file name under [`saga_marker_dir`]. The write
/// pipeline is serialized (one mutation at a time, see
/// `EngineHandle::write_lock`), so at most one saga is ever in flight and a
/// single fixed marker file is enough.
const INTENT_MARKER_NAME: &str = "saga.intent";

/// The engine's saga-recovery directory under the launch entry root,
/// `<root>/<identity dir>/au-engine/run/`. Holds the `saga.intent` marker and the
/// `saga.failed.<id>` sentinels: ephemeral run-state, owner-namespaced under the
/// engine, see
/// [[spec - 2brain layout - a reserved multi-tenant device root, owner-namespaced with a category sublayer]].
///
/// The identity directory is resolved by the WRITE rule, not spelled: a repository keeps the
/// spelling it already has, and a repository that has neither is new, so it gets the current
/// name. Spelling the legacy name here created a second identity directory inside a new
/// repository — a `.arsumbris/au-engine/` beside its `.2brain/repo.yaml` — which the engine's
/// own reserved-root and migration checks then have to treat as a foreign body.
fn saga_marker_dir(root: &Path) -> PathBuf {
    tb_parser::identity_dir_for_write_with(root, |dir| dir.exists())
        .join("au-engine")
        .join("run")
}

#[cfg(test)]
mod grant_refusal_tests {
    use super::*;

    fn plan_with(members: &[&str]) -> SagaPlan {
        SagaPlan {
            mutation_id: "m".to_string(),
            members: members
                .iter()
                .map(|name| SagaMember {
                    tree_root: PathBuf::from(format!("/ws/{name}")),
                    repo_names: vec![crate::repo::RepoName((*name).to_string())],
                    paths: vec![PathBuf::from(format!("/ws/{name}/note.md"))],
                    git: true,
                })
                .collect(),
            not_written: Default::default(),
            outside_git: Default::default(),
            unclassified: None,
            dirs: Default::default(),
            attribution: Vec::new(),
        }
    }

    /// No list is the ABSENCE of a grant, not a grant of nothing: a policy-free launch sends
    /// nothing and every saga stays permitted.
    #[test]
    fn an_absent_grant_refuses_nothing() {
        assert!(grant_refusal(&plan_with(&["ra", "rb"]), &[], "rename").is_none());
    }

    /// A grant that covers every member of the plan permits it.
    #[test]
    fn a_grant_covering_the_plan_permits_it() {
        let allowed = vec!["ra".to_string(), "rb".to_string()];
        assert!(grant_refusal(&plan_with(&["ra", "rb"]), &allowed, "rename").is_none());
    }

    /// A member outside the grant refuses, and every such member is named once.
    #[test]
    fn a_member_outside_the_grant_refuses_and_is_named() {
        let allowed = vec!["ra".to_string()];
        let reject = grant_refusal(&plan_with(&["ra", "rb", "rc"]), &allowed, "rename")
            .expect("a member outside the grant must refuse");
        assert!(
            reject.message.contains("policy/member_grant"),
            "{}",
            reject.message
        );
        assert!(reject.message.contains("rb"), "{}", reject.message);
        assert!(reject.message.contains("rc"), "{}", reject.message);
        assert!(!reject.message.contains("ra,"), "{}", reject.message);
    }
}

#[cfg(test)]
mod saga_marker_dir_tests {
    use super::*;

    /// A new repository writes the current identity directory, so its run-state belongs there.
    /// Spelling the legacy name would leave a `.arsumbris/au-engine/` inside a repository whose
    /// only identity directory is `.2brain`.
    #[test]
    fn a_new_repository_keeps_its_run_state_in_the_new_identity_directory() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join(tb_parser::IDENTITY_DIR)).unwrap();
        assert_eq!(
            saga_marker_dir(root),
            root.join(tb_parser::IDENTITY_DIR)
                .join("au-engine")
                .join("run")
        );
    }

    /// A repository that already has the legacy spelling keeps it: the write rule follows what
    /// exists rather than migrating a repository as a side effect of a mutation.
    #[test]
    fn a_legacy_repository_keeps_its_run_state_where_it_already_is() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join(tb_parser::LEGACY_IDENTITY_DIR)).unwrap();
        assert_eq!(
            saga_marker_dir(root),
            root.join(tb_parser::LEGACY_IDENTITY_DIR)
                .join("au-engine")
                .join("run")
        );
    }

    /// A root with neither spelling is new, so the current name wins.
    #[test]
    fn a_root_with_neither_spelling_gets_the_current_name() {
        let tmp = tempfile::TempDir::new().unwrap();
        assert_eq!(
            saga_marker_dir(tmp.path()),
            tmp.path()
                .join(tb_parser::CREATE_IDENTITY_DIR)
                .join("au-engine")
                .join("run")
        );
    }
}

/// The saga intent marker present for `root` under either identity spelling, when a mutation is
/// in flight or a crashed one is awaiting recovery: the marker opens before a write and closes
/// on settle.
///
/// Read-only, so a tool about to touch a repository's files (the tier-C identity migration) can
/// refuse rather than race the engine's recovery. The legacy directory is checked first because
/// this release still spells it that way ([`saga_marker_dir`]); the new name answers for a
/// repository that has already been migrated.
pub fn in_flight_saga_marker(root: &Path) -> Option<PathBuf> {
    [
        saga_marker_dir(root),
        root.join(tb_parser::IDENTITY_DIR)
            .join("au-engine")
            .join("run"),
    ]
    .into_iter()
    .map(|dir| dir.join(INTENT_MARKER_NAME))
    .find(|marker| marker.is_file())
}

/// Open a saga by writing its durable intent marker, fsynced. Returns the marker
/// path so the caller closes it on settle.
///
/// Format: a `mutation-id=<id>` line, then one line per committing member, the
/// member's absolute WORKING-TREE root, then the tree-relative paths it writes,
/// all tab-separated. Then one `preimage` line per path written outside git,
/// its tree root, its tree-relative path, and its parked copy (empty when the
/// path did not exist), see [`Preimage`]. Then one `dir-created` or `dir-kept`
/// line per journaled directory, absolute, see [`DirJournal`]. One line per tree, so a member covering several repos of a
/// monorepo records once, matching the one commit it makes. The per-member paths
/// are what crash recovery path-restores a written-but-not-committed member by;
/// the tree root makes recovery self-contained, no rebuilt IR needed. It records
/// intent, not the pending contents, so it can roll a crashed saga back but not
/// complete it.
///
/// Engine-internal, under `marker_dir` ([`saga_marker_dir`], the launch entry's
/// `.arsumbris/au-engine/run/`). That root is the entry repo's
/// directory, an ancestor of every co-present member but not of a scattered member
/// that sits elsewhere. Either way it is a single
/// per-daemon location, so one marker spans the whole cross-repo saga, never one
/// per member. It is known at launch, before the IR builds, which is what crash
/// recovery needs.
///
/// It closes the window the commit trailers cannot: a crash after writing the
/// trees but before the first commit leaves dirty trees and zero trailers,
/// indistinguishable from an unmediated edit without the marker. Crash recovery
/// reads it back to compensate an interrupted saga.
fn write_intent_marker(
    marker_dir: &Path,
    plan: &SagaPlan,
    preimages: &[Preimage],
) -> std::io::Result<PathBuf> {
    use std::io::Write;
    crate::ignored_dir::ensure_git_ignored_dir(marker_dir)?;
    let mut body = format!("mutation-id={}\n", plan.mutation_id);
    for m in plan.members.iter().filter(|m| m.git) {
        let written = plan.written_paths(m);
        if written.is_empty() {
            continue;
        }
        body.push_str(&m.tree_root.to_string_lossy());
        for p in &written {
            body.push('\t');
            body.push_str(&p.to_string_lossy());
        }
        body.push('\n');
    }
    for pi in preimages {
        body.push_str(PREIMAGE_LINE);
        body.push('\t');
        body.push_str(&pi.tree_root.to_string_lossy());
        body.push('\t');
        body.push_str(&pi.path.to_string_lossy());
        body.push('\t');
        if let Some(parked) = &pi.parked {
            body.push_str(&parked.to_string_lossy());
        }
        body.push('\n');
    }
    for (tag, dirs) in [
        (DIR_CREATED_LINE, &plan.dirs.created),
        (DIR_KEPT_LINE, &plan.dirs.kept),
    ] {
        for dir in dirs {
            body.push_str(tag);
            body.push('\t');
            body.push_str(&dir.to_string_lossy());
            body.push('\n');
        }
    }
    let path = marker_dir.join(INTENT_MARKER_NAME);
    let mut file = std::fs::File::create(&path)?;
    file.write_all(body.as_bytes())?;
    file.sync_all()?;
    // fsync the directory too, so the marker's existence survives a crash.
    if let Ok(dir) = std::fs::File::open(marker_dir) {
        let _ = dir.sync_all();
    }
    Ok(path)
}

/// A crashed saga rolled back at startup: the mutation it recovered, and which
/// member repos were reverted (they had committed) versus restored (they wrote
/// but did not commit).
#[derive(Debug)]
pub struct SagaRecovery {
    pub mutation_id: String,
    pub reverted: Vec<PathBuf>,
    pub restored: Vec<PathBuf>,
}

/// An un-rolled-back saga, surfaced from a `saga.failed.<id>` sentinel. It
/// persists until an operator resolves it, so it is re-surfaced at every start.
#[derive(Debug)]
pub struct SagaFailure {
    pub mutation_id: String,
    /// The sentinel's body: one line per member that could not be rolled back.
    pub detail: String,
}

/// What startup recovery found and did.
#[derive(Debug)]
pub struct SagaRecoveryReport {
    /// The intent marker that was processed, if one was open.
    pub recovered: Option<SagaRecovery>,
    /// Outstanding failure sentinels, this run's and any prior run's.
    pub failures: Vec<SagaFailure>,
}

/// Roll an interrupted saga back before serving, then report any outstanding
/// un-rolled-back sagas.
///
/// Reads the intent marker under [`saga_marker_dir`]. Each member it names is
/// compensated by the same rule the live saga uses: a member carrying the
/// mutation's commit is reverted by `Mutation-Id`, a member that wrote but did
/// not commit has its recorded paths restored to HEAD. The intent marker is then
/// removed. A member that cannot be rolled back (a revert conflict after a human
/// committed over the engine, or a classify error) is recorded in a durable
/// `saga.failed.<id>` sentinel, never silently dropped.
///
/// Recovery does not auto-retry a failure: a failed revert is most plausibly a
/// conflict needing human resolution, so the engine's job is to keep it visible,
/// not to loop. Every `saga.failed.<id>` sentinel is re-surfaced here at each
/// start until an operator removes it.
///
/// The intent marker is the only thing that distinguishes a crash-before-first-
/// commit (dirty trees, zero trailers) from an unmediated edit, so recovery never
/// guesses from the working tree alone.
pub fn recover_crashed_saga(root: &Path) -> SagaRecoveryReport {
    let marker_dir = saga_marker_dir(root);
    let recovered = recover_intent_marker(&marker_dir);
    // Every parked copy is spent once the marker is processed, and one with no
    // marker is an orphan of a crash between closing the marker and dropping it.
    sweep_preimages(&marker_dir);
    let failures = scan_failure_sentinels(&marker_dir);
    SagaRecoveryReport {
        recovered,
        failures,
    }
}

/// Process the open intent marker, if any. Compensates each member, records any
/// member it could not roll back in a `saga.failed.<id>` sentinel, then removes
/// the intent marker.
fn recover_intent_marker(marker_dir: &Path) -> Option<SagaRecovery> {
    let marker_path = marker_dir.join(INTENT_MARKER_NAME);
    let content = std::fs::read_to_string(&marker_path).ok()?;
    let mut lines = content.lines();
    let mutation_id = lines.next()?.strip_prefix("mutation-id=")?.to_string();
    let id = MutationId(mutation_id.clone());

    let mut reverted = Vec::new();
    let mut restored = Vec::new();
    let mut errors: Vec<(PathBuf, String)> = Vec::new();
    let mut preimages = Vec::new();
    let mut dirs = DirJournal::default();
    for line in lines {
        if let Some(pi) = parse_preimage_line(line) {
            preimages.push(pi);
            continue;
        }
        if parse_dir_line(line, &mut dirs) {
            continue;
        }
        let mut fields = line.split('\t');
        let Some(root_str) = fields.next().filter(|s| !s.is_empty()) else {
            continue;
        };
        let tree_root = PathBuf::from(root_str);
        let paths: Vec<PathBuf> = fields.map(PathBuf::from).collect();
        // One line per TREE, so one classification covers every repo that tree
        // holds: the coalesced member made one commit, and this reverts that one.
        match ShellGit.committed(&tree_root, &id) {
            Ok(true) => match ShellGit.revert_commit(&tree_root, &id) {
                Ok(()) => reverted.push(tree_root),
                Err(e) => errors.push((tree_root, format!("revert failed: {}", e.message))),
            },
            Ok(false) => match ShellGit.restore_paths(&tree_root, &paths) {
                Ok(()) => restored.push(tree_root),
                Err(e) => errors.push((tree_root, format!("restore failed: {}", e.message))),
            },
            Err(e) => errors.push((tree_root, format!("classify failed: {}", e.message))),
        }
    }
    errors.extend(restore_preimages(&preimages));
    errors.extend(undo_dirs(&dirs));
    if !errors.is_empty() {
        write_failure_sentinel(marker_dir, &mutation_id, &errors);
    }
    let _ = std::fs::remove_file(&marker_path);
    Some(SagaRecovery {
        mutation_id,
        reverted,
        restored,
    })
}

/// Read every `saga.failed.<id>` sentinel under the marker directory, name-sorted
/// for a deterministic report. They are not removed, an operator clears them
/// after manual repair.
fn scan_failure_sentinels(marker_dir: &Path) -> Vec<SagaFailure> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(marker_dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Some(id) = name.strip_prefix("saga.failed.") {
            let detail = std::fs::read_to_string(entry.path()).unwrap_or_default();
            out.push(SagaFailure {
                mutation_id: id.to_string(),
                detail,
            });
        }
    }
    out.sort_by(|a, b| a.mutation_id.cmp(&b.mutation_id));
    out
}

/// Close a saga by removing its intent marker, on settle (commit-all or
/// compensate-all). Best-effort: a stale marker is detected and handled by crash
/// recovery, never a correctness hazard.
fn remove_intent_marker(path: &Path) {
    let _ = std::fs::remove_file(path);
}

/// The failure sentinel's file name, one per un-rolled-back saga, keyed by
/// `Mutation-Id`. Distinct from `saga.intent` and from another saga's failure,
/// so the next saga's marker never clobbers it.
fn failure_sentinel_name(mutation_id: &str) -> String {
    format!("saga.failed.{mutation_id}")
}

/// Record an incomplete rollback durably, so a dangling saga is never invisible.
/// One line per member that could not be rolled back, the repo root and why.
/// Surfaced at every startup, removed only by an operator after manual repair.
/// Best-effort: a sentinel write that fails cannot itself be compensated.
fn write_failure_sentinel(marker_dir: &Path, mutation_id: &str, errors: &[(PathBuf, String)]) {
    use std::io::Write;
    if crate::ignored_dir::ensure_git_ignored_dir(marker_dir).is_err() {
        return;
    }
    let mut body = format!("mutation-id={mutation_id}\n");
    for (root, err) in errors {
        body.push_str(&format!("{}\t{}\n", root.to_string_lossy(), err));
    }
    let path = marker_dir.join(failure_sentinel_name(mutation_id));
    if let Ok(mut file) = std::fs::File::create(&path) {
        let _ = file.write_all(body.as_bytes());
        let _ = file.sync_all();
    }
}

/// On a saga whose rollback could not complete, write the failure sentinel and
/// tag the reject so the consumer learns the rollback was incomplete. No errors
/// is the clean case, the reject passes through unchanged.
fn note_incomplete_rollback(
    marker_dir: &Path,
    mutation_id: &str,
    errors: Vec<(PathBuf, String)>,
    mut reject: MutationReject,
) -> MutationReject {
    if errors.is_empty() {
        return reject;
    }
    write_failure_sentinel(marker_dir, mutation_id, &errors);
    reject.message = format!(
        "{}; the rollback was incomplete, {} member(s) recorded in {}",
        reject.message,
        errors.len(),
        failure_sentinel_name(mutation_id)
    );
    reject
}

/// Roll a failed saga back, returning the per-member errors it could not
/// compensate. Each already-committed member is reverted by its `Mutation-Id`
/// (idempotent, race-free against a concurrent human commit), then every member's
/// written paths ([`SagaPlan::written_paths`]) are restored to HEAD. Restoring a committed-then-reverted member
/// is a no-op, so passing all members is safe and removes the need to track which
/// were written-with-changes (a no-op write skips its commit but stays in the
/// member list).
///
/// A compensation error cannot itself be compensated, so the errors are returned
/// for the caller to record durably, never swallowed — a member that could not
/// be rolled back must not become invisible.
fn compensate(
    git: &impl GitWriter,
    committed: &[&SagaMember],
    members: &[&SagaMember],
    plan: &SagaPlan,
    mutation_id: &MutationId,
) -> Vec<(PathBuf, String)> {
    let mut errors = Vec::new();
    for m in committed {
        if let Err(e) = git.revert_commit(&m.tree_root, mutation_id) {
            errors.push((m.tree_root.clone(), format!("revert failed: {}", e.message)));
        }
    }
    for m in members {
        // Only what the mutation wrote. A path it never wrote may hold a human's
        // uncommitted edit, and restoring it to HEAD would discard that work.
        let written = plan.written_paths(m);
        if written.is_empty() {
            continue;
        }
        if let Err(e) = git.restore_paths(&m.tree_root, &written) {
            errors.push((
                m.tree_root.clone(),
                format!("restore failed: {}", e.message),
            ));
        }
    }
    errors
}

/// Fold a list of write stamps into a just-written file, in order, returning the
/// hash the response should carry: the stamped file's new hash when the last
/// non-no-op stamp changed it, else the primary write's hash. Each stamp re-reads
/// the just-written bytes, so a same-field append sees the growing list. Runs
/// INSIDE the write closure, so every stamp shares the write's commit
/// ([[spec - stamp injection - a write rider idempotently ensures a frontmatter record folded into the write's commit]]).
fn fold_write_stamps(
    target: &Path,
    stamps: &[Stamp],
    primary: crate::ir::ContentHash,
) -> Result<crate::ir::ContentHash, crate::mutate::MutationReject> {
    let mut hash = primary;
    for s in stamps {
        if let Some(h) =
            crate::mutate::fold_stamp(target, &s.field, &s.record, s.match_on.as_ref())?
        {
            hash = h;
        }
    }
    Ok(hash)
}

/// The serde default for `ensure_mixins_strict`: strict is the default, so an
/// un-appliable mixin rejects the whole write unless the caller opts out.
fn default_true() -> bool {
    true
}

/// Fold a write's `ensure_mixins` into the just-written file, AFTER the stamps (so
/// a mixin validates against the stamped content), returning the response hash
/// and each entry's outcome. Runs INSIDE the write closure, so an applied mixin
/// shares the write's commit. The decision is [`crate::ensure_mixin::fold`], the
/// same fold `preview_mutation` runs. Under STRICT an un-appliable mixin
/// propagates a `MutationReject`, so the saga compensates and nothing lands;
/// under LENIENT it is a reported skip and the write proceeds.
fn fold_write_mixins(
    handle: &EngineHandle,
    target: &Path,
    mixins: &[String],
    strict: bool,
    primary: crate::ir::ContentHash,
) -> Result<(crate::ir::ContentHash, Vec<wire::MixinOutcome>), crate::mutate::MutationReject> {
    if mixins.is_empty() {
        return Ok((primary, Vec::new()));
    }
    let content = std::fs::read_to_string(target).map_err(|e| {
        crate::mutate::MutationReject::new(format!(
            "ensure_mixin could not read {} to fold into: {e}",
            target.display()
        ))
    })?;
    let (folded, outcomes) = match handle
        .read(|kb| crate::ensure_mixin::fold(kb, target, content.clone(), mixins, strict))
    {
        Read::Ready { value, .. } => value?,
        _ => {
            return Err(crate::mutate::MutationReject::new(
                "engine is not ready to resolve an ensure_mixin",
            ));
        }
    };
    if folded == content {
        return Ok((primary, outcomes));
    }
    std::fs::write(target, &folded).map_err(|e| {
        crate::mutate::MutationReject::new(format!(
            "ensure_mixin could not write {}: {e}",
            target.display()
        ))
    })?;
    Ok((crate::ir::ContentHash::of(folded.as_bytes()), outcomes))
}

/// Whether a `stamps` / `ensure_mixins` rider may ride this write's target, and
/// the reject reason when it may not. A rider splices an INSTANCE's frontmatter:
/// a `type:`-first record for a stamp, a mixin on the identity claim. A type-def
/// cannot carry that:
/// - a type-def (`*.type.yaml` / `*.type.yml`): its `type:` is a
///   PARENT claim, not an instance identity, and a stamp's `---` wrap would strand
///   the def's body past a document separator. `rename` already rejects a type-def
///   target; the frontmatter verbs do so here.
///
/// A plain instance file returns `None` and the rider rides it.
fn rider_target_reject(write_path: &Path) -> Option<crate::mutate::MutationReject> {
    if tb_parser::classify_by_path(write_path) == Some(tb_parser::FileKind::TypeDef) {
        return Some(crate::mutate::MutationReject::new(
            "a stamp or ensure_mixin targets an instance's frontmatter, not a type-def; a \
             type-def's `type:` is a parent claim, not an instance identity",
        ));
    }
    None
}

/// Project mixin outcomes into a mutation response's `extra` map, under
/// `ensure_mixins`, omitted when the write carried none.
fn mixin_outcomes_extra(
    outcomes: Vec<wire::MixinOutcome>,
) -> serde_json::Map<String, serde_json::Value> {
    let mut extra = serde_json::Map::new();
    if !outcomes.is_empty() {
        extra.insert(
            "ensure_mixins".into(),
            serde_json::to_value(outcomes).expect("mixin outcomes serialize"),
        );
    }
    extra
}

/// ADR-0002: the refusal when the state after this write would carry an ERROR-severity
/// diagnostic, or `None` when it would be clean.
///
/// The check is the preview build the `preview_mutation` verb already performs, so it
/// costs a build only when a caller asks for it. `handle_mutation` holds the write
/// pipeline's lock for its whole body, so the preview and the write that follows
/// observe one held state — the two-call race the ADR refuses in the adapter cannot
/// happen here. A preview that rejects for its own reason (an absent target, a stamp on
/// a non-list field) leaves the verdict to the write, which refuses with that same
/// reason: one failure, one answer.
async fn clean_check_refusal(
    handle: &EngineHandle,
    target: &Path,
    op: crate::mutate::PreviewOp,
    riders: crate::engine::PreviewRiders,
) -> Option<serde_json::Value> {
    let handle = handle.clone();
    let input = crate::engine::PreviewInput::File {
        target: target.to_path_buf(),
        op,
        riders,
    };
    let read = tokio::task::spawn_blocking(move || {
        handle.preview(input, |outcome| match outcome {
            crate::engine::PreviewOutcome::Built { preview, .. } => preview
                .diagnostics()
                .filter(|diagnostic| diagnostic.severity == Severity::Error)
                .map(|diagnostic| {
                    format!(
                        "{} in {}",
                        diagnostic.code.as_str(),
                        diagnostic.span.file.display()
                    )
                })
                .collect::<Vec<String>>(),
            crate::engine::PreviewOutcome::Reject(_) => Vec::new(),
        })
    })
    .await
    .expect("the clean check never panics");
    // A state that is not ready is the write path's answer to give, not this check's.
    let Read::Ready { value: errors, .. } = read else {
        return None;
    };
    if errors.is_empty() {
        return None;
    }
    Some(mutation_reject_frame(crate::mutate::MutationReject::new(
        format!(
            "policy/diagnostics: this mutation would introduce {} error(s); fix them or write \
             without `require_clean`: {}",
            errors.len(),
            errors.join(", ")
        ),
    )))
}

/// The change set a batch would produce, composed in the caller's order through the same
/// pure edit transform the committing op uses ([`crate::mutate::compute_edit`]), so the
/// preview and the write cannot disagree about what an op yields.
///
/// The net effect is what matters: two ops on one path collapse to the last one, and an
/// edit reads the running content an earlier op would have left, not the bytes on disk.
fn batch_change_set(
    ops: &[BatchOp],
    resolved: &[PathBuf],
) -> Result<crate::engine::ChangeSet, crate::mutate::MutationReject> {
    use crate::overlay::Override;
    use std::collections::BTreeMap;
    let mut touched: BTreeMap<PathBuf, Option<Vec<u8>>> = BTreeMap::new();
    for (op, target) in ops.iter().zip(resolved) {
        match op {
            BatchOp::Write { content, .. } => {
                touched.insert(target.clone(), Some(content.clone().into_bytes()));
            }
            BatchOp::Edit {
                old_string,
                new_string,
                replace_all,
                ..
            } => {
                let base = match touched.get(target) {
                    Some(Some(bytes)) => String::from_utf8(bytes.clone()).map_err(|_| {
                        MutationReject::new(format!(
                            "{} is not UTF-8 after an earlier op",
                            target.display()
                        ))
                    })?,
                    Some(None) => {
                        return Err(MutationReject::new(format!(
                            "{} is deleted earlier in the batch and cannot be edited after",
                            target.display()
                        )));
                    }
                    None => std::fs::read_to_string(target).map_err(|e| {
                        MutationReject::new(format!("cannot read {}: {e}", target.display()))
                    })?,
                };
                let updated =
                    crate::mutate::compute_edit(&base, old_string, new_string, *replace_all)?;
                touched.insert(target.clone(), Some(updated.into_bytes()));
            }
            BatchOp::Delete { .. } => {
                touched.insert(target.clone(), None);
            }
        }
    }
    let mut overrides = Vec::with_capacity(touched.len());
    let mut gone = Vec::new();
    for (path, state) in touched {
        match state {
            Some(content) => overrides.push(Override::Replace { path, content }),
            None => {
                gone.push(path.clone());
                overrides.push(Override::Remove { path });
            }
        }
    }
    Ok(crate::engine::ChangeSet {
        target: resolved[0].clone(),
        overrides,
        also_dirty: resolved.to_vec(),
        gone,
    })
}

/// [`clean_check_refusal`] for a batch: preview the composed set, attribute every ERROR
/// diagnostic to the last op that touched its file, and refuse with that attribution.
/// A path no op touched (a referrer, say) is named as `elsewhere` rather than blamed on
/// an innocent op.
async fn clean_check_batch(
    handle: &EngineHandle,
    set: crate::engine::ChangeSet,
    ops: &[BatchOp],
    resolved: &[PathBuf],
) -> Option<serde_json::Value> {
    let handle = handle.clone();
    let read = tokio::task::spawn_blocking(move || {
        handle.preview(
            crate::engine::PreviewInput::Set(set),
            |outcome| match outcome {
                crate::engine::PreviewOutcome::Built { preview, .. } => preview
                    .diagnostics()
                    .filter(|diagnostic| diagnostic.severity == Severity::Error)
                    .map(|diagnostic| {
                        (
                            diagnostic.span.file.clone(),
                            diagnostic.code.as_str().to_string(),
                        )
                    })
                    .collect::<Vec<(PathBuf, String)>>(),
                crate::engine::PreviewOutcome::Reject(_) => Vec::new(),
            },
        )
    })
    .await
    .expect("the clean check never panics");
    let Read::Ready { value: errors, .. } = read else {
        return None;
    };
    if errors.is_empty() {
        return None;
    }
    let blamed = errors
        .iter()
        .map(|(file, code)| {
            let last = resolved.iter().rposition(|target| target == file);
            match last {
                Some(index) => {
                    let verb = match &ops[index] {
                        BatchOp::Write { .. } => "write_file",
                        BatchOp::Edit { .. } => "edit_file",
                        BatchOp::Delete { .. } => "delete_file",
                    };
                    format!(
                        "batch op {} ({verb} {}): {code} in {}",
                        index + 1,
                        file.display(),
                        file.display()
                    )
                }
                None => format!("elsewhere in the tree: {code} in {}", file.display()),
            }
        })
        .collect::<Vec<String>>();
    Some(mutation_reject_frame(crate::mutate::MutationReject::new(
        format!(
            "policy/diagnostics: this batch would introduce {} error(s); fix them or write \
             without `require_clean`: {}",
            errors.len(),
            blamed.join(", ")
        ),
    )))
}

/// Run one mutation: resolve and guard the path, execute the write, drive
/// the synchronous rebuild, then answer with fresh state — the post-mutation
/// version, the file's catalog hash, and its diagnostics. A rejection is an
/// `error` frame and writes nothing; diagnostics never reject, they ride
/// the response ([[spec - mutation channel v1 - a closed primitive catalog
/// through one mediated path]]).
async fn handle_mutation(handle: &EngineHandle, m: Mutation) -> serde_json::Value {
    // Serialize the write pipeline: one mutation at a time across every
    // connection. Held across the whole saga and rebuild, this enforces the
    // single-threaded model the intent marker and crash recovery rely on. Reads
    // and subscriptions never take it.
    let _write = handle.write_lock().lock().await;
    // Mutating while Deriving would race the initial build; the consumer
    // waits for ready like it does for reads.
    if handle.version().is_none() {
        return serde_json::to_value(Response::not_ready()).expect("response serializes");
    }
    match m {
        Mutation::WriteFile(args) => {
            let write_path = match resolve_mutation_target(handle, &args.path) {
                Ok(t) => t,
                Err(frame) => return frame,
            };
            let response_path = write_path.clone();
            // A stamp or a mixin rides an instance file's frontmatter; a
            // type-def target would be corrupted. Reject early, before any write.
            if !args.stamps.is_empty() || !args.ensure_mixins.is_empty() {
                if let Some(reject) = rider_target_reject(&write_path) {
                    return mutation_reject_frame(reject);
                }
            }
            let attribution = match resolve_attribution(&args.attribution) {
                Ok(a) => a,
                Err(frame) => return frame,
            };
            if args.require_clean
                && let Some(frame) = clean_check_refusal(
                    handle,
                    &write_path,
                    crate::mutate::PreviewOp::Write {
                        content: args.content.clone(),
                    },
                    preview_riders(
                        args.stamps.clone(),
                        args.ensure_mixins.clone(),
                        args.ensure_mixins_strict,
                    ),
                )
                .await
            {
                return frame;
            }
            let mut plan = saga_plan_off_thread(handle, vec![write_path.clone()]).await;
            plan.attribution = attribution;
            let marker_dir = saga_marker_dir(handle.root());
            let summary = format!("write_file {}", args.path);
            // Disk write, commit-or-compensate, then full rebuild, off the workers.
            let blocking_handle = handle.clone();
            let blocking_path = write_path.clone();
            let outcome = tokio::task::spawn_blocking(move || {
                let ((written, mixins), commits) =
                    apply_saga(&ShellGit, &marker_dir, &plan, summary, false, || {
                        let written = crate::mutate::write_file(
                            &blocking_path,
                            &args.content,
                            args.expected_hash.as_deref(),
                        )?;
                        // Fold the stamps then the mixins into the just-written file,
                        // so they share this commit. The final hash (if a rider
                        // changed the file) is the response hash, so `reflected`
                        // compares the riddled bytes.
                        let written = fold_write_stamps(&blocking_path, &args.stamps, written)?;
                        fold_write_mixins(
                            &blocking_handle,
                            &blocking_path,
                            &args.ensure_mixins,
                            args.ensure_mixins_strict,
                            written,
                        )
                    })?;
                blocking_handle.rebuild_paths(saga_dirty_set(&plan));
                Ok::<_, crate::mutate::MutationReject>((written, mixins, commits))
            })
            .await
            .expect("mutation task never panics");
            match outcome {
                Err(reject) => mutation_reject_frame(reject),
                Ok((written, mixins, commits)) => mutation_response_extra(
                    handle,
                    &response_path,
                    Some(written),
                    commits,
                    mixin_outcomes_extra(mixins),
                ),
            }
        }
        Mutation::CreateFile(args) => {
            let write_path = match resolve_mutation_target(handle, &args.path) {
                Ok(t) => t,
                Err(frame) => return frame,
            };
            let response_path = write_path.clone();
            // A stamp or a mixin rides an instance file's frontmatter; a
            // type-def target would be corrupted. Reject early, before any write.
            if !args.stamps.is_empty() || !args.ensure_mixins.is_empty() {
                if let Some(reject) = rider_target_reject(&write_path) {
                    return mutation_reject_frame(reject);
                }
            }
            let attribution = match resolve_attribution(&args.attribution) {
                Ok(a) => a,
                Err(frame) => return frame,
            };
            if args.require_clean
                && let Some(frame) = clean_check_refusal(
                    handle,
                    &write_path,
                    crate::mutate::PreviewOp::Write {
                        content: args.content.clone(),
                    },
                    preview_riders(
                        args.stamps.clone(),
                        args.ensure_mixins.clone(),
                        args.ensure_mixins_strict,
                    ),
                )
                .await
            {
                return frame;
            }
            let mut plan = saga_plan_off_thread(handle, vec![write_path.clone()]).await;
            plan.attribution = attribution;
            let marker_dir = saga_marker_dir(handle.root());
            let summary = format!("create_file {}", args.path);
            // Disk write, commit-or-compensate, then full rebuild, off the workers.
            // The absence check is `create_file`'s own exclusive create, INSIDE the
            // closure, so it runs where the write is serialized and no second
            // request can slip between deciding the path is free and taking it.
            // A conflict is a saga that wrote nothing, so it compensates to a no-op.
            let blocking_handle = handle.clone();
            let blocking_path = write_path.clone();
            let outcome = tokio::task::spawn_blocking(move || {
                let ((written, mixins), commits) =
                    apply_saga(&ShellGit, &marker_dir, &plan, summary, false, || {
                        let written = crate::mutate::create_file(&blocking_path, &args.content)?;
                        let written = fold_write_stamps(&blocking_path, &args.stamps, written)?;
                        fold_write_mixins(
                            &blocking_handle,
                            &blocking_path,
                            &args.ensure_mixins,
                            args.ensure_mixins_strict,
                            written,
                        )
                    })?;
                blocking_handle.rebuild_paths(saga_dirty_set(&plan));
                Ok::<_, crate::mutate::MutationReject>((written, mixins, commits))
            })
            .await
            .expect("mutation task never panics");
            match outcome {
                Err(reject) => mutation_reject_frame(reject),
                Ok((written, mixins, commits)) => mutation_response_extra(
                    handle,
                    &response_path,
                    Some(written),
                    commits,
                    mixin_outcomes_extra(mixins),
                ),
            }
        }
        Mutation::EditFile(args) => {
            let write_path = match resolve_mutation_target(handle, &args.path) {
                Ok(t) => t,
                Err(frame) => return frame,
            };
            let response_path = write_path.clone();
            if !args.stamps.is_empty() || !args.ensure_mixins.is_empty() {
                if let Some(reject) = rider_target_reject(&write_path) {
                    return mutation_reject_frame(reject);
                }
            }
            let attribution = match resolve_attribution(&args.attribution) {
                Ok(a) => a,
                Err(frame) => return frame,
            };
            if args.require_clean
                && let Some(frame) = clean_check_refusal(
                    handle,
                    &write_path,
                    crate::mutate::PreviewOp::Edit {
                        old_string: args.old_string.clone(),
                        new_string: args.new_string.clone(),
                        replace_all: args.replace_all,
                    },
                    preview_riders(
                        args.stamps.clone(),
                        args.ensure_mixins.clone(),
                        args.ensure_mixins_strict,
                    ),
                )
                .await
            {
                return frame;
            }
            let mut plan = saga_plan_off_thread(handle, vec![write_path.clone()]).await;
            plan.attribution = attribution;
            let marker_dir = saga_marker_dir(handle.root());
            let summary = format!("edit_file {}", args.path);
            let blocking_handle = handle.clone();
            let blocking_path = write_path.clone();
            let outcome = tokio::task::spawn_blocking(move || {
                let ((written, mixins), commits) =
                    apply_saga(&ShellGit, &marker_dir, &plan, summary, false, || {
                        let written = crate::mutate::edit_file(
                            &blocking_path,
                            &args.old_string,
                            &args.new_string,
                            args.replace_all,
                        )?;
                        let written = fold_write_stamps(&blocking_path, &args.stamps, written)?;
                        fold_write_mixins(
                            &blocking_handle,
                            &blocking_path,
                            &args.ensure_mixins,
                            args.ensure_mixins_strict,
                            written,
                        )
                    })?;
                blocking_handle.rebuild_paths(saga_dirty_set(&plan));
                Ok::<_, crate::mutate::MutationReject>((written, mixins, commits))
            })
            .await
            .expect("mutation task never panics");
            match outcome {
                Err(reject) => mutation_reject_frame(reject),
                Ok((written, mixins, commits)) => mutation_response_extra(
                    handle,
                    &response_path,
                    Some(written),
                    commits,
                    mixin_outcomes_extra(mixins),
                ),
            }
        }
        Mutation::DeleteFile(args) => {
            let write_path = match resolve_mutation_target(handle, &args.path) {
                Ok(t) => t,
                Err(frame) => return frame,
            };
            let response_path = write_path.clone();
            let attribution = match resolve_attribution(&args.attribution) {
                Ok(a) => a,
                Err(frame) => return frame,
            };
            let mut plan = saga_plan_off_thread(handle, vec![write_path.clone()]).await;
            plan.attribution = attribution;
            let marker_dir = saga_marker_dir(handle.root());
            let summary = format!("delete_file {}", args.path);
            let blocking_handle = handle.clone();
            let blocking_path = write_path.clone();
            let outcome = tokio::task::spawn_blocking(move || {
                // The last-live commit: HEAD immediately BEFORE the delete is the
                // parent of the deletion commit, the last commit where the file
                // still existed. The clean-at-HEAD precondition guarantees the
                // file was present there, and the serialized write lock keeps
                // this HEAD the deletion commit's parent. Captured before the
                // saga runs, since the commit advances HEAD. au-provenance's
                // delete tombstone pins this so the file's last content reads
                // back, see [[spec - git write path - commit-per-mutation as a
                // local saga over the workspace's materialized repos]].
                let last_live = crate::gitwriter::head_commit_fast(&blocking_path);
                let ((), commits) =
                    apply_saga(&ShellGit, &marker_dir, &plan, summary, false, || {
                        guard_unchanged_before_touch(&blocking_path, None)?;
                        crate::mutate::delete_file(&blocking_path, args.expected_hash.as_deref())
                    })?;
                blocking_handle.rebuild_paths(saga_dirty_set(&plan));
                Ok::<_, crate::mutate::MutationReject>((commits, last_live))
            })
            .await
            .expect("mutation task never panics");
            match outcome {
                Err(reject) => mutation_reject_frame(reject),
                // No content landed, so no written hash to lag; the response
                // carries the now-absent target's fresh (empty) state.
                Ok((commits, last_live)) => {
                    // Surface the last-live commit only when the delete actually
                    // committed (on-git): off-git there is no deletion commit and
                    // no last-live to pin. The deletion commit itself stays in
                    // `commits` / `commit` for attribution.
                    let mut extra = serde_json::Map::new();
                    if !commits.is_empty() {
                        if let Some(sha) = last_live {
                            extra.insert(
                                "last_live_commit".to_string(),
                                serde_json::Value::String(sha),
                            );
                        }
                    }
                    mutation_response_extra(handle, &response_path, None, commits, extra)
                }
            }
        }
        Mutation::Batch(args) => batch_mutation(handle, args).await,
        Mutation::AssignBlockId(args) => assign_block_id(handle, args).await,
        Mutation::EditRecord(args) => handle_edit_record(handle, args).await,
        Mutation::AppendRecord(args) => handle_append_record(handle, args).await,
        Mutation::Rename(args) => {
            let from = match resolve_mutation_target(handle, &args.path) {
                Ok(t) => t,
                Err(frame) => return frame,
            };
            let to = match resolve_mutation_target(handle, &args.to) {
                Ok(t) => t,
                Err(frame) => return frame,
            };
            let gathered = handle.read(|v| plan_rename(v, &from, &to));
            let RenamePlan {
                repo_root: move_repo_root,
                old_rel,
                new_rel,
                referrers,
                referrer_hashes,
            } = match gathered {
                crate::Read::Ready { value: Ok(g), .. } => g,
                crate::Read::Ready {
                    value: Err(msg), ..
                } => {
                    return mutation_reject_frame(crate::mutate::MutationReject::new(msg));
                }
                crate::Read::NotReady => {
                    return serde_json::to_value(Response::not_ready())
                        .expect("response serializes");
                }
            };
            // The mutation touches `from`, `to`, and every referrer. `from` is a
            // referrer too when the file references itself; it is already in the
            // touched set via the move, so it is not added twice.
            let mut touched = vec![from.clone(), to.clone()];
            touched.extend(referrers.keys().filter(|p| **p != from).cloned());
            // A move that keeps the name leaves a bare `[[name]]` referrer
            // byte-identical, so it is not written: not clean-gated, not
            // consumed-member-gated, not committed. Still recomputed.
            let planned = {
                let h = handle.clone();
                let (referrers, referrer_hashes) = (referrers.clone(), referrer_hashes.clone());
                let (from, old_rel, new_rel) = (from.clone(), old_rel.clone(), new_rel.clone());
                off_thread(move || {
                    let not_written = move_referrers_not_written(
                        &referrers,
                        &referrer_hashes,
                        &from,
                        &old_rel,
                        &new_rel,
                    );
                    // A referrer this rename would rewrite that lives in a consumed
                    // member rejects the whole refactor.
                    if let Some(reject) = reject_if_consumed_referrers(
                        &h,
                        &written_referrers(&referrers, &not_written),
                    ) {
                        return Err(reject);
                    }
                    Ok(resolve_saga_plan_writing(&h, &touched, not_written))
                })
                .await
            };
            let mut plan = match planned {
                Ok(plan) => plan,
                Err(reject) => return mutation_reject_frame(reject),
            };
            if let Some(reject) = grant_refusal(&plan, &args.allowed_members, "rename") {
                return mutation_reject_frame(reject);
            }
            let not_written = plan.not_written.clone();
            // The rollback removes the created destination and prunes each parent
            // it leaves empty, so a parent that existed, empty, would vanish.
            plan.dirs.kept = to
                .ancestors()
                .skip(1)
                .find(|d| d.exists())
                .map(Path::to_path_buf)
                .into_iter()
                .collect();
            let marker_dir = saga_marker_dir(handle.root());
            let summary = format!("rename {} -> {}", args.path, args.to);
            let blocking_handle = handle.clone();
            let blocking_from = from.clone();
            let blocking_to = to.clone();
            let stamps = args.stamps;
            let ensure_mixins = args.ensure_mixins;
            let strict = args.ensure_mixins_strict;
            let outcome = tokio::task::spawn_blocking(move || {
                // The move belongs to the tree where the file actually moved, so
                // only that member's commit carries the `Moved:` trailer; a
                // referrer in another tree only had links rewritten.
                let moves = moves_for(&move_repo_root, &old_rel, &new_rel);
                let ((written, mixins), commits) = apply_saga_with_moves(
                    &ShellGit,
                    &marker_dir,
                    &plan,
                    summary,
                    &moves,
                    true,
                    || {
                        // Rewrite every referrer first, then move the file. The
                        // moved file's own rewrite, when it self-references, is
                        // the baseline its pre-move re-check compares against.
                        let mut self_written: Option<String> = None;
                        for (path, spans) in &referrers {
                            let content = read_referrer_checked(
                                path,
                                referrer_hashes.get(path).copied().flatten(),
                            )?;
                            // A byte-identical rewrite is not written: a bare
                            // `[[name]]` on a name-keeping move, or an all-pinned
                            // referrer. Writing it anyway costs a disk write and
                            // wakes the watcher for a file that did not change.
                            let Some(rewritten) =
                                crate::rename::move_rewrite(&content, spans, &old_rel, &new_rel)?
                            else {
                                continue;
                            };
                            // The plan left this path out of the clean guard and
                            // the commit. Writing it now would land an
                            // uncommitted change, so refuse instead.
                            if not_written.contains(path) {
                                return Err(crate::mutate::MutationReject::new(format!(
                                    "referrer {} changed since the rename was planned — retry",
                                    path.display()
                                )));
                            }
                            std::fs::write(path, &rewritten).map_err(|e| {
                                crate::mutate::MutationReject::new(format!(
                                    "cannot write referrer {}: {e}",
                                    path.display()
                                ))
                            })?;
                            if *path == blocking_from {
                                self_written = Some(rewritten);
                            }
                        }
                        guard_unchanged_before_touch(&blocking_from, self_written.as_deref())?;
                        let written = crate::mutate::rename_file(&blocking_from, &blocking_to)?;
                        // Fold the stamps then the mixins into the RENAMED file,
                        // after the move, so they ride the rename's commit.
                        let written = fold_write_stamps(&blocking_to, &stamps, written)?;
                        fold_write_mixins(
                            &blocking_handle,
                            &blocking_to,
                            &ensure_mixins,
                            strict,
                            written,
                        )
                    },
                )?;
                blocking_handle.rebuild_paths(saga_dirty_set(&plan));
                Ok::<_, crate::mutate::MutationReject>((written, mixins, commits))
            })
            .await
            .expect("mutation task never panics");
            match outcome {
                Err(reject) => mutation_reject_frame(reject),
                Ok((written, mixins, commits)) => mutation_response_extra(
                    handle,
                    &to,
                    Some(written),
                    commits,
                    mixin_outcomes_extra(mixins),
                ),
            }
        }
        Mutation::MoveDir(args) => move_dir(handle, args).await,
        Mutation::DeleteDir(args) => delete_dir(handle, args).await,
        Mutation::RenameType(args) => {
            // A rename_type cascades across the def file and every referrer, so it
            // has NO single target file for a stamp. It rejects any stamp here; when
            // the multi-file `stamp.path` (naming one file in the write set) lands,
            // rename_type gains stamping with an explicit target.
            if !args.stamps.is_empty() {
                return mutation_reject_frame(crate::mutate::MutationReject::new(
                    "rename_type has no single target file for a stamp — it rewrites the def and \
                     every referrer; stamping a specific file needs the deferred multi-file \
                     stamp target",
                ));
            }
            let gathered = handle.read(|v| plan_rename_type(v, &args.old_name, &args.new_name));
            let gather = match gathered {
                crate::Read::Ready { value: Ok(g), .. } => g,
                crate::Read::Ready {
                    value: Err(msg), ..
                } => {
                    return mutation_reject_frame(crate::mutate::MutationReject::new(msg));
                }
                crate::Read::NotReady => {
                    return serde_json::to_value(Response::not_ready())
                        .expect("response serializes");
                }
            };
            // ADR-0003: the grant is checked against the cascade THIS mutation plans, under the
            // write lock, instead of against a preview taken by an earlier request. A referrer
            // added between the two was rewritten outside the caller's grant.
            if !args.allowed_members.is_empty() {
                let mut outside: Vec<(String, String)> = Vec::new();
                // The def's destination is the same member as its source and has no `repos`
                // entry of its own (the file does not exist yet), so the check walks the
                // paths the cascade writes: the def and every referrer.
                let written = std::iter::once(&gather.from).chain(gather.referrers.iter());
                for path in written {
                    let member = gather.repos.get(path);
                    match member {
                        Some(member)
                            if args
                                .allowed_members
                                .iter()
                                .any(|allowed| allowed == member.as_str()) => {}
                        Some(member) => {
                            outside.push((member.as_str().to_string(), path.display().to_string()))
                        }
                        // A path with no owning member cannot be shown to be inside the
                        // grant, so it is outside it: this is the fail-closed direction.
                        None => outside.push((String::new(), path.display().to_string())),
                    }
                }
                if !outside.is_empty() {
                    outside.sort();
                    let named: Vec<String> = outside
                        .iter()
                        .map(|(member, path)| {
                            if member.is_empty() {
                                format!("an undeclared member holding {path}")
                            } else {
                                format!("member {member} ({path})")
                            }
                        })
                        .collect();
                    return mutation_reject_frame(crate::mutate::MutationReject::new(format!(
                        "policy/member_grant: rename_type {} -> {} rewrites {} path(s) outside the \
                         grant: {}",
                        args.old_name,
                        args.new_name,
                        named.len(),
                        named.join(", ")
                    )));
                }
            }
            let touched = gather.touched();
            let planned = {
                let h = handle.clone();
                let gather = gather.clone();
                off_thread(move || {
                    let writes = gather.written_and_exempt();
                    // A referrer this rename_type would rewrite (a claim edit or a
                    // live wikilink) that lives in a consumed member rejects the
                    // whole refactor. `written` is referrers only; the def
                    // file's own editability is gated earlier.
                    if let Some(reject) = reject_if_consumed_referrers(&h, &writes.written) {
                        return Err(reject);
                    }
                    Ok(resolve_saga_plan_writing(&h, &touched, writes.exempt))
                })
                .await
            };
            let plan = match planned {
                Ok(plan) => plan,
                Err(reject) => return mutation_reject_frame(reject),
            };
            let RenameTypeGather {
                from,
                to,
                move_repo_root,
                old_rel,
                new_rel,
                old_name,
                new_name,
                type_edits,
                link_spans,
                referrer_hashes,
                referrers,
                ..
            } = gather;
            let marker_dir = saga_marker_dir(handle.root());
            let summary = format!("rename_type {} -> {}", old_name.as_str(), new_name.as_str());
            // A rename records the def file's move for the pinned-reference
            // forward trace, scoped to the def's repo.
            let moves = moves_for(&move_repo_root, &old_rel, &new_rel);
            let blocking_handle = handle.clone();
            let blocking_from = from.clone();
            let blocking_to = to.clone();
            let outcome = tokio::task::spawn_blocking(move || {
                let (written, commits) = apply_saga_with_moves(
                    &ShellGit,
                    &marker_dir,
                    &plan,
                    summary,
                    &moves,
                    true,
                    || {
                        // Rewrite every referrer (both surfaces merged) first, then
                        // relocate the def. A self-referencing def is rewritten under
                        // its old name, then the file move carries it; that rewrite
                        // is the baseline its pre-move re-check compares against.
                        let mut self_written: Option<String> = None;
                        for path in &referrers {
                            let wrote = rewrite_referrer(
                                path,
                                referrer_hashes.get(path).copied().flatten(),
                                type_edits.get(path).map(Vec::as_slice),
                                link_spans.get(path).map(Vec::as_slice),
                                &old_rel,
                                &new_rel,
                            )?;
                            if *path == blocking_from && wrote.is_some() {
                                self_written = wrote;
                            }
                        }
                        guard_unchanged_before_touch(&blocking_from, self_written.as_deref())?;
                        crate::mutate::rename_file(&blocking_from, &blocking_to)
                    },
                )?;
                blocking_handle.rebuild_paths(saga_dirty_set(&plan));
                Ok::<_, crate::mutate::MutationReject>((written, commits))
            })
            .await
            .expect("mutation task never panics");
            match outcome {
                Err(reject) => mutation_reject_frame(reject),
                Ok((written, commits)) => mutation_response(handle, &to, Some(written), commits),
            }
        }
        Mutation::Promote(args) => {
            let from = match resolve_mutation_target(handle, &args.path) {
                Ok(t) => t,
                Err(frame) => return frame,
            };
            let to = match resolve_mutation_target(handle, &args.to) {
                Ok(t) => t,
                Err(frame) => return frame,
            };
            if tb_parser::classify_by_path(&to) == Some(tb_parser::FileKind::TypeDef) {
                return mutation_reject_frame(crate::mutate::MutationReject::new(
                    "promote creates an instance file, not a type-def — `to` must not be a \
                     type-def path",
                ));
            }
            // The `to` extension is the format selector and must be one the engine
            // writes an instance into: `.md` (frontmatter-fenced, empty body) or
            // `.yaml`/`.yml` (bare YAML). Anything else (no extension, `.txt`) is a
            // rejected request, not a silent default.
            let to_ext = to
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if !matches!(to_ext.as_str(), "md" | "markdown" | "yaml" | "yml") {
                return mutation_reject_frame(crate::mutate::MutationReject::new(format!(
                    "promote target must be a .md, .markdown, .yaml, or .yml file, got {:?}",
                    args.to
                )));
            }
            // The record locator: a byte offset, or a `^:` id. Exactly one — the
            // offset addresses a record with no id, the id is the stable handle
            // for one that has it (the referenced case always does).
            enum Locator {
                At(usize),
                Block(String),
            }
            let locator = match (args.at, &args.block_id) {
                (Some(_), Some(_)) => {
                    return mutation_reject_frame(crate::mutate::MutationReject::new(
                        "pass exactly one of `at` or `block_id`, not both",
                    ));
                }
                (None, None) => {
                    return mutation_reject_frame(crate::mutate::MutationReject::new(
                        "pass exactly one of `at` (a byte offset) or `block_id`",
                    ));
                }
                (Some(at), None) => Locator::At(at),
                (None, Some(id)) => Locator::Block(id.clone()),
            };
            // Over the held knowledge base: the same-repo guard, the record's whole span,
            // the host's content hash (read-before-write), and the inbound
            // referrers filtered to this record's block-id. A referrer inside the
            // record's own span is excluded — it travels with the extracted
            // content, it is not rewritten in place.
            struct Located {
                record_span: tb_diagnostics::ByteRange,
                host_hash: Option<crate::ir::ContentHash>,
                new_stem: String,
                /// The promoted record's own `^:` id, if it has one. The single id
                /// that collapses to a whole-file `[[newFile]]` reference; every
                /// other travelling (nested) id keeps its id under the new file.
                promoted_id: Option<String>,
                referrers: BTreeMap<PathBuf, Vec<tb_diagnostics::ByteRange>>,
                /// The held content hash of each referrer, for the read-before-write
                /// drift check when the saga re-reads it.
                referrer_hashes: BTreeMap<PathBuf, Option<crate::ir::ContentHash>>,
                /// References inside the record to any block-id that travels with
                /// it (its own id or a nested one), host-absolute. Redirected onto
                /// the new file during extraction so they do not dangle once the
                /// record moves.
                inner_self_pointers: Vec<tb_diagnostics::ByteRange>,
            }
            let gathered = handle.read(|v| {
                let (rf, rt) = match (v.repos.repo_of(&from), v.repos.repo_of(&to)) {
                    (Some(rf), Some(rt)) => (rf, rt),
                    _ => return Err("path or destination escapes the workspace".to_string()),
                };
                if rf.root != rt.root {
                    return Err(
                        "promote into another repo is not supported — path and to must be in the \
                         same repo"
                            .to_string(),
                    );
                }
                let entry = v
                    .catalog
                    .get(&from)
                    .ok_or_else(|| "the host file is not in the knowledge base".to_string())?;
                let inst = match entry.parse.as_ref() {
                    FileParse::Instance {
                        instance: Some(inst),
                        ..
                    } => inst,
                    _ => {
                        return Err("the host file is not a typed instance — nothing to promote"
                            .to_string())
                    }
                };
                let hit = match &locator {
                    Locator::At(at) => record_at(&inst.fields, *at).ok_or_else(|| {
                        "no inline record encloses the offset — pass an offset inside a `^:` record"
                            .to_string()
                    })?,
                    Locator::Block(id) => record_by_block_id(&inst.fields, id)
                        .ok_or_else(|| format!("no inline record carries the block-id `{id}`"))?,
                };
                let record_span = hit.span;
                // The host slot must admit a reference: promote replaces the
                // record with a `[[newFile]]` reference, which a bare inline-only
                // slot (`T`, `any`, a non-`&` compound) cannot hold. Only `&`
                // admits both forms. (A `*` slot holds no inline record, so the
                // locator never finds one there; only the bare case reaches here.)
                if crate::resolution_build::record_slot_admits_reference_at_of_kb(
                    v,
                    &from,
                    inst,
                    record_span,
                ) == Some(false)
                {
                    return Err(
                        "promote out of an inline-only slot: the slot holds an inline record but \
                         cannot hold the `[[newFile]]` reference promote writes — only an \
                         inline-or-reference (`&`) slot can be promoted"
                            .to_string(),
                    );
                }
                // A record is a subtree: the unit that travels to the new file is
                // its whole block-id set — its own `^:` id (if any) plus every
                // nested `^:` id declared within its span. Referrers to any of
                // them must follow, or they dangle at the host the record left.
                let travelling: std::collections::BTreeSet<String> =
                    crate::resolution_build::record_targets_of_kb(v, &from, inst)
                        .into_iter()
                        .filter(|(_, t)| {
                            t.span.start >= record_span.start && t.span.end <= record_span.end
                        })
                        .map(|(id, _)| id)
                        .collect();
                // The host's inbound edges to any travelling id. Empty for a
                // record with no addressable id and no referrers — the
                // referrerless case, cross-repo-safe by construction. An edge
                // inside the record's own span is a self-reference: it travels
                // with the extracted content, so it is redirected there rather
                // than rewritten in place.
                let mut referrers: BTreeMap<PathBuf, Vec<tb_diagnostics::ByteRange>> =
                    BTreeMap::new();
                let mut referrer_hashes: BTreeMap<PathBuf, Option<crate::ir::ContentHash>> =
                    BTreeMap::new();
                let mut inner_self_pointers: Vec<tb_diagnostics::ByteRange> = Vec::new();
                // A bare `^id` referrer that fills a slot is navigational — the
                // HOST file is its value, `^id` a jump anchor. promote's rewrite
                // follows the extracted record, which would silently change that
                // value from the host to the record, so it is rejected. The author
                // must first make it `^^id` (to follow the record) or drop the
                // anchor. A `^^id` referrer and a slot-less prose bare `^id` rewrite
                // cleanly.
                let mut ambiguous: Option<(PathBuf, String, String)> = None;
                for bl in v.backlinks(&from) {
                    let Some(block) = bl.block_id.as_ref() else {
                        continue;
                    };
                    if !travelling.contains(block.id.as_str()) {
                        continue;
                    }
                    let inside_record = bl.source == from
                        && bl.span.start >= record_span.start
                        && bl.span.end <= record_span.end;
                    if inside_record {
                        inner_self_pointers.push(bl.span);
                    } else {
                        if ambiguous.is_none() && !block.referent {
                            if let Some(slot) = &bl.slot {
                                ambiguous =
                                    Some((bl.source.clone(), slot.clone(), block.id.clone()));
                            }
                        }
                        referrers
                            .entry(bl.source.clone())
                            .or_default()
                            .push(bl.span);
                        referrer_hashes
                            .entry(bl.source.clone())
                            .or_insert_with(|| v.catalog.get(&bl.source).and_then(|e| e.hash));
                    }
                }
                if let Some((src, slot, id)) = ambiguous {
                    return Err(format!(
                        "promote would silently change a reference's value: {} fills slot `{slot}` \
                         with a bare `^{id}` (navigational — the host file is the value, `^{id}` a \
                         jump anchor). Change it to `^^{id}` to follow the promoted record, or drop \
                         the `^{id}` anchor, then retry",
                        src.display()
                    ));
                }
                let new_stem = to
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                Ok(Located {
                    record_span,
                    host_hash: entry.hash,
                    new_stem,
                    promoted_id: hit.existing.clone(),
                    referrers,
                    referrer_hashes,
                    inner_self_pointers,
                })
            });
            let located = match gathered {
                crate::Read::Ready { value: Ok(g), .. } => g,
                crate::Read::Ready {
                    value: Err(msg), ..
                } => {
                    return mutation_reject_frame(crate::mutate::MutationReject::new(msg));
                }
                crate::Read::NotReady => {
                    return serde_json::to_value(Response::not_ready())
                        .expect("response serializes");
                }
            };
            // Every touched file: the host, the new file, and each referrer (the
            // host is already present when it is also a self-referrer).
            let mut touched = vec![from.clone(), to.clone()];
            touched.extend(located.referrers.keys().filter(|p| **p != from).cloned());
            // A referrer this promote would rewrite that lives in a consumed member
            // rejects the whole refactor.
            let plan = match refactor_saga_plan(handle, touched, located.referrers.clone()).await {
                Ok(plan) => plan,
                Err(reject) => return mutation_reject_frame(reject),
            };
            if let Some(reject) = grant_refusal(&plan, &args.allowed_members, "promote") {
                return mutation_reject_frame(reject);
            }
            let marker_dir = saga_marker_dir(handle.root());
            let summary = format!("promote {} -> {}", args.path, args.to);
            let blocking_handle = handle.clone();
            let blocking_from = from.clone();
            let blocking_to = to.clone();
            let stamps = args.stamps;
            let Located {
                record_span,
                host_hash,
                new_stem,
                promoted_id,
                referrers,
                referrer_hashes,
                inner_self_pointers,
            } = located;
            let outcome = tokio::task::spawn_blocking(move || {
                // A travelling reference repoints at the new file, carrying its
                // `:field` attribution and `::repo` qualifier. The promoted
                // record's own id collapses to a whole-file `[[newFile]]` (block-id
                // and anchor drop); a nested id that travelled keeps its id under
                // the new file (`[[newFile^nestedId]]`).
                let transform = |w: &tb_references::WikilinkRef| {
                    let mut nw = w.clone();
                    nw.target = new_stem.clone();
                    nw.anchor = None;
                    if w.block_id_str() == promoted_id.as_deref() {
                        nw.block_id = None;
                    }
                    nw
                };
                let (written, commits) =
                    apply_saga(&ShellGit, &marker_dir, &plan, summary, true, || {
                        if blocking_to.exists() {
                            return Err(crate::mutate::MutationReject::new(format!(
                                "destination already exists: {}",
                                blocking_to.display()
                            )));
                        }
                        let content = std::fs::read_to_string(&blocking_from).map_err(|e| {
                            crate::mutate::MutationReject::new(format!(
                                "cannot read {}: {e}",
                                blocking_from.display()
                            ))
                        })?;
                        // The span came from the held parse; a file that drifted on
                        // disk makes it unreliable. Read-before-write, structurally.
                        if host_hash != Some(crate::ir::ContentHash::of(content.as_bytes())) {
                            return Err(crate::mutate::MutationReject::new(
                            "the file changed since the engine last read it — wait for the rebuild \
                             and retry",
                        ));
                        }
                        // The record's self-references, redirected to the new file so
                        // they do not dangle once the record moves out of the host.
                        let inner_edits =
                            crate::rename::ref_edits(&content, &inner_self_pointers, &transform)?;
                        let raw = crate::promote::extract_record_file(
                            &content,
                            record_span,
                            &inner_edits,
                        )?;
                        // A `.md` instance needs frontmatter fences around the YAML;
                        // a `.yaml`/`.yml` instance is the bare YAML.
                        let is_md = matches!(
                            blocking_to.extension().and_then(|e| e.to_str()),
                            Some("md") | Some("markdown")
                        );
                        let file_content = if is_md {
                            format!("---\n{raw}---\n")
                        } else {
                            raw
                        };
                        // The host's own edits — the record replacement plus any
                        // self-referrer rewrites — applied as one last-first pass so
                        // they never shift each other, whatever their file order.
                        //
                        // The record's value span runs through the field's
                        // terminating newline. The reference must replace only the
                        // record's bytes and LEAVE that newline, or the following
                        // line (the closing `---` fence, or the next key) glues onto
                        // the reference and the host stops parsing. `extract_record_file`
                        // above keeps the full `record_span`, so the extracted file
                        // still terminates cleanly; only the in-host replacement trims.
                        let mut ref_end = record_span.end;
                        while ref_end > record_span.start
                            && matches!(content.as_bytes().get(ref_end - 1), Some(b'\n' | b'\r'))
                        {
                            ref_end -= 1;
                        }
                        let ref_span = tb_diagnostics::ByteRange::new(record_span.start, ref_end);
                        let ref_text = format!("\"[[{new_stem}]]\"");
                        let mut host_edits = vec![(ref_span, ref_text)];
                        if let Some(spans) = referrers.get(&blocking_from) {
                            host_edits
                                .extend(crate::rename::ref_edits(&content, spans, &transform)?);
                        }
                        let host_after = crate::rename::apply_edits(&content, host_edits)?;
                        // Rewrite every other referrer.
                        for (source, spans) in &referrers {
                            if *source == blocking_from {
                                continue;
                            }
                            let rc = read_referrer_checked(
                                source,
                                referrer_hashes.get(source).copied().flatten(),
                            )?;
                            let rewritten = crate::rename::rewrite_refs(&rc, spans, &transform)?;
                            // A referrer whose every reference is commit-pinned
                            // yields no edits, so the rewrite is byte-identical.
                            // Writing it anyway costs a disk write and wakes the
                            // watcher for a file that did not change.
                            if rewritten != rc {
                                std::fs::write(source, rewritten).map_err(|e| {
                                    crate::mutate::MutationReject::new(format!(
                                        "cannot write referrer {}: {e}",
                                        source.display()
                                    ))
                                })?;
                            }
                        }
                        // The new file, then the host pointing at it.
                        let written = crate::mutate::write_file(&blocking_to, &file_content, None)?;
                        std::fs::write(&blocking_from, host_after).map_err(|e| {
                            crate::mutate::MutationReject::new(format!(
                                "cannot write host {}: {e}",
                                blocking_from.display()
                            ))
                        })?;
                        // Fold the stamp into the newly-extracted file, this commit.
                        fold_write_stamps(&blocking_to, &stamps, written)
                    })?;
                blocking_handle.rebuild_paths(saga_dirty_set(&plan));
                Ok::<_, crate::mutate::MutationReject>((written, commits))
            })
            .await
            .expect("mutation task never panics");
            match outcome {
                Err(reject) => mutation_reject_frame(reject),
                Ok((written, commits)) => mutation_response(handle, &to, Some(written), commits),
            }
        }
        Mutation::Inline(args) => {
            let file = match resolve_mutation_target(handle, &args.path) {
                Ok(t) => t,
                Err(frame) => return frame,
            };
            let into = match resolve_mutation_target(handle, &args.into) {
                Ok(t) => t,
                Err(frame) => return frame,
            };
            if file == into {
                return mutation_reject_frame(crate::mutate::MutationReject::new(
                    "inline needs a file and a distinct host — `path` and `into` are the same file",
                ));
            }
            // Over the held knowledge base: the same-repo guard, the file's body-emptiness
            // (a record has no body), the chosen reference value in `into`, the
            // host's other references (-> local `[[^id]]`), and the cross-file
            // referrers (-> `[[host^id]]`). Plus the read-before-write hashes.
            struct Located {
                value_span: tb_diagnostics::ByteRange,
                into_other: Vec<tb_diagnostics::ByteRange>,
                cross: BTreeMap<PathBuf, Vec<tb_diagnostics::ByteRange>>,
                /// The held content hash of each cross-file referrer, for the
                /// read-before-write drift check when the saga re-reads it.
                cross_hashes: BTreeMap<PathBuf, Option<crate::ir::ContentHash>>,
                into_hash: Option<crate::ir::ContentHash>,
                file_hash: Option<crate::ir::ContentHash>,
                file_is_md: bool,
                into_stem: String,
                /// The combined file-local id namespace of `file` and `into`. Fresh
                /// ids (the new record id and any collision renames) are drawn clear
                /// of it.
                namespace: std::collections::BTreeSet<String>,
                /// Each id `file`'s subtree shares with `into`, plus its `^:`
                /// declaration span in `file` (file-absolute). Renamed to a fresh id
                /// during the fold so the merged file has no duplicate id.
                colliding: Vec<(String, tb_diagnostics::ByteRange)>,
                /// `file`'s references to its own blocks (file-absolute spans),
                /// rebased onto the host as the folded content travels.
                file_self: Vec<tb_diagnostics::ByteRange>,
                /// Referrers that cannot be re-pointed to a block-id, one
                /// human-readable reason each — a `file*` whole-file slot or a
                /// `[[file#head]]` anchor. A non-empty list is a rejected request.
                offenders: Vec<String>,
            }
            let gathered = handle.read(|v| {
                let (rf, ri) = match (v.repos.repo_of(&file), v.repos.repo_of(&into)) {
                    (Some(a), Some(b)) => (a, b),
                    _ => return Err("path or into escapes the workspace".to_string()),
                };
                if rf.root != ri.root {
                    return Err(
                        "inline across repos is not supported — path and into must be in the same \
                         repo"
                            .to_string(),
                    );
                }
                let file_entry = v
                    .catalog
                    .get(&file)
                    .ok_or_else(|| "the file to inline is not in the knowledge base".to_string())?;
                let (file_is_md, has_body, file_inst) = match file_entry.parse.as_ref() {
                    FileParse::Instance {
                        is_markdown,
                        body,
                        instance: Some(inst),
                        ..
                    } => (*is_markdown, !body.trim().is_empty(), inst),
                    _ => return Err("the file to inline is not a typed instance".to_string()),
                };
                if has_body {
                    return Err(
                        "the file to inline has a non-empty body — a record has no body; strip the \
                         body first, or keep it as its own file"
                            .to_string(),
                    );
                }
                let into_entry = v
                    .catalog
                    .get(&into)
                    .ok_or_else(|| "the host `into` is not in the knowledge base".to_string())?;
                let into_inst = match into_entry.parse.as_ref() {
                    FileParse::Instance {
                        instance: Some(inst),
                        ..
                    } => inst,
                    _ => return Err("the host `into` is not a typed instance".to_string()),
                };
                // Block-ids are unique within a file. Folding `file`'s subtree into
                // `into` moves every `^:` id `file` declares into `into`'s
                // namespace; an id `into` already declares (inline record or body
                // marker) collides. Rather than reject, the travelling id is
                // renamed to a fresh id during the fold and every reference to it
                // follows — the `colliding` set carries each colliding id and its
                // `^:` declaration span (file-absolute) so the saga can rewrite it.
                // `namespace` is the combined id space the fresh ids are drawn
                // clear of.
                let file_targets =
                    crate::resolution_build::record_targets_of_kb(v, &file, file_inst);
                let file_ids = host_block_ids(file_entry.parse.as_ref(), &file_targets);
                let into_ids = host_block_ids(
                    into_entry.parse.as_ref(),
                    &crate::resolution_build::record_targets_of_kb(v, &into, into_inst),
                );
                let colliding: Vec<(String, tb_diagnostics::ByteRange)> = file_ids
                    .intersection(&into_ids)
                    .map(|id| {
                        (
                            id.clone(),
                            file_targets
                                .get(id)
                                .expect("a colliding id is one of file's record ids")
                                .span,
                        )
                    })
                    .collect();
                let namespace: std::collections::BTreeSet<String> =
                    file_ids.union(&into_ids).cloned().collect();
                // `file`'s references to its own blocks travel with the folded
                // content; rebased onto the host (local `[[^id]]`, the renamed id
                // when it collided) so they do not dangle once `file` is deleted.
                // Routed out of `cross` here so the saga rewrites them in the folded
                // text, not on the about-to-be-deleted file.
                let mut into_refs: Vec<tb_diagnostics::ByteRange> = Vec::new();
                let mut cross: BTreeMap<PathBuf, Vec<tb_diagnostics::ByteRange>> = BTreeMap::new();
                let mut cross_hashes: BTreeMap<PathBuf, Option<crate::ir::ContentHash>> =
                    BTreeMap::new();
                let mut file_self: Vec<tb_diagnostics::ByteRange> = Vec::new();
                for bl in v.backlinks(&file) {
                    if bl.source == file {
                        file_self.push(bl.span);
                    } else if bl.source == into {
                        into_refs.push(bl.span);
                    } else {
                        cross.entry(bl.source.clone()).or_default().push(bl.span);
                        cross_hashes
                            .entry(bl.source.clone())
                            .or_insert_with(|| v.catalog.get(&bl.source).and_then(|e| e.hash));
                    }
                }
                if into_refs.is_empty() {
                    return Err(
                        "`into` does not reference `path` — it is not a referrer".to_string()
                    );
                }
                into_refs.sort_by_key(|s| s.start);
                let chosen = match args.at {
                    Some(at) => into_refs
                        .iter()
                        .copied()
                        .find(|s| at >= s.start && at < s.end)
                        .ok_or_else(|| "`at` does not land on a reference to `path`".to_string())?,
                    None => {
                        if into_refs.len() > 1 {
                            return Err(
                                "`into` references `path` more than once — pass `at` to choose \
                                 which reference hosts the record"
                                    .to_string(),
                            );
                        }
                        into_refs[0]
                    }
                };
                let value_span =
                    value_span_at(&into_inst.fields, chosen.start).ok_or_else(|| {
                        "could not locate the reference value to fold into".to_string()
                    })?;
                // Host-slot guard: the chosen reference's slot must admit the
                // inlined record. Inline folds `file`'s content into `into` as a
                // `^:id` record; a reference-only (`*`) slot holds the `[[file]]`
                // reference now but cannot hold the record. Only `&`
                // (inline-or-reference) admits both directions — the dual of
                // promote's bare-slot guard. An undeclared (extra) slot imposes no
                // constraint.
                if let Some(shape) =
                    crate::resolution_build::slot_shape_at_of_kb(v, &into, into_inst, value_span)
                {
                    if !tb_core::shape_is_inline_or_reference(&shape) {
                        return Err(format!(
                            "inline into a reference-only slot (`{shape}`): it holds the `[[…]]` \
                             reference but cannot hold the inlined record — only an \
                             inline-or-reference (`&`) slot can be inlined into"
                        ));
                    }
                }
                let into_other: Vec<tb_diagnostics::ByteRange> =
                    into_refs.iter().copied().filter(|s| *s != chosen).collect();
                let into_stem = into
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                // Referrer guard: every other referrer becomes `[[into^id]]` (or
                // local `[[^id]]` within `into`), addressing the folded block. A
                // referrer that cannot hold a block-id is a rejected request,
                // named in the response `detail`:
                // - a `file*` slot references a whole file; `^block-id` is rejected
                //   on it, see [[type-def shape file::au-type-system]].
                // - a `[[file#head]]` anchor targets a heading in the deleted file.
                let mut offenders: Vec<String> = Vec::new();
                let mut note_offender = |source: &Path, span: tb_diagnostics::ByteRange| {
                    let Some(parse) = v.catalog.get(source).map(|e| e.parse.as_ref()) else {
                        return;
                    };
                    if let FileParse::Instance {
                        instance: Some(inst),
                        ..
                    } = parse
                    {
                        // The backlink span covers the `[[…]]` link; the slot
                        // resolver keys on the enclosing scalar's whole value-span,
                        // so map through `value_span_at` first (as the host-slot
                        // guard does for the chosen reference).
                        let governs = value_span_at(&inst.fields, span.start).and_then(|vspan| {
                            crate::resolution_build::slot_shape_at_of_kb(v, source, inst, vspan)
                        });
                        if matches!(governs, Some(Shape::Reference(name)) if name.as_str() == "file") {
                            offenders.push(format!(
                                "{}: a `file*` slot references a whole file and cannot hold a \
                                 block-id",
                                source.display()
                            ));
                            return;
                        }
                    }
                    if let Some(raw) = wikilink_inner_at(parse, span) {
                        if parse_wikilink_inner(&raw).is_ok_and(|w| w.anchor.is_some()) {
                            offenders.push(format!(
                                "{}: a `[[…#head]]` anchor targets a heading in the deleted file",
                                source.display()
                            ));
                        }
                    }
                };
                for span in &into_other {
                    note_offender(&into, *span);
                }
                for (source, spans) in &cross {
                    for span in spans {
                        note_offender(source, *span);
                    }
                }
                Ok(Located {
                    value_span,
                    into_other,
                    cross,
                    cross_hashes,
                    into_hash: into_entry.hash,
                    file_hash: file_entry.hash,
                    file_is_md,
                    into_stem,
                    namespace,
                    colliding,
                    file_self,
                    offenders,
                })
            });
            let located = match gathered {
                crate::Read::Ready { value: Ok(g), .. } => g,
                crate::Read::Ready {
                    value: Err(msg), ..
                } => {
                    return mutation_reject_frame(crate::mutate::MutationReject::new(msg));
                }
                crate::Read::NotReady => {
                    return serde_json::to_value(Response::not_ready())
                        .expect("response serializes");
                }
            };
            let Located {
                value_span,
                into_other,
                cross,
                cross_hashes,
                into_hash,
                file_hash,
                file_is_md,
                into_stem,
                namespace,
                colliding,
                file_self,
                offenders,
            } = located;
            // A referrer that cannot be re-pointed to a block-id is a rejected
            // request, the offenders named in `detail`. The caller resolves them
            // first, or promote stays the safe direction.
            if !offenders.is_empty() {
                return mutation_reject_frame(crate::mutate::MutationReject {
                    message: "inline cannot re-point every referrer to the inlined block — some \
                              referrers reference the whole file or a heading inside it; resolve \
                              them first, or keep the file as its own node"
                        .to_string(),
                    detail: Some(serde_json::json!({ "offenders": offenders })),
                });
            }
            let mut touched = vec![into.clone(), file.clone()];
            touched.extend(cross.keys().filter(|p| **p != into && **p != file).cloned());
            // A cross-referrer this inline would rewrite that lives in a consumed
            // member rejects the whole refactor.
            let plan = match refactor_saga_plan(handle, touched, cross.clone()).await {
                Ok(plan) => plan,
                Err(reject) => return mutation_reject_frame(reject),
            };
            if let Some(reject) = grant_refusal(&plan, &args.allowed_members, "inline") {
                return mutation_reject_frame(reject);
            }
            let marker_dir = saga_marker_dir(handle.root());
            let summary = format!("inline {} into {}", args.path, args.into);
            let blocking_handle = handle.clone();
            let blocking_into = into.clone();
            let stamps = args.stamps;
            let blocking_file = file.clone();
            let outcome = tokio::task::spawn_blocking(move || {
                let (written, commits) =
                    apply_saga(&ShellGit, &marker_dir, &plan, summary, true, || {
                        let into_content =
                            std::fs::read_to_string(&blocking_into).map_err(|e| {
                                crate::mutate::MutationReject::new(format!(
                                    "cannot read {}: {e}",
                                    blocking_into.display()
                                ))
                            })?;
                        if into_hash != Some(crate::ir::ContentHash::of(into_content.as_bytes())) {
                            return Err(crate::mutate::MutationReject::new(
                            "the host changed since the engine last read it — wait for the rebuild \
                             and retry",
                        ));
                        }
                        let file_content =
                            std::fs::read_to_string(&blocking_file).map_err(|e| {
                                crate::mutate::MutationReject::new(format!(
                                    "cannot read {}: {e}",
                                    blocking_file.display()
                                ))
                            })?;
                        if file_hash != Some(crate::ir::ContentHash::of(file_content.as_bytes())) {
                            return Err(crate::mutate::MutationReject::new(
                            "the file changed since the engine last read it — wait for the rebuild \
                             and retry",
                        ));
                        }
                        // Fresh ids: one per travelling id that collides with the host,
                        // then the new record id — all drawn clear of the combined
                        // namespace and of each other. `colliding` is sorted (a BTreeSet
                        // intersection), so the assignment is deterministic.
                        let mut taken = namespace.clone();
                        let mut rename_map: std::collections::BTreeMap<String, String> =
                            std::collections::BTreeMap::new();
                        let mut file_edits: Vec<(tb_diagnostics::ByteRange, String)> = Vec::new();
                        for (old, decl_span) in &colliding {
                            let fresh = crate::mutate::generate_block_id(|c| taken.contains(c));
                            taken.insert(fresh.clone());
                            file_edits.push((*decl_span, fresh.clone()));
                            rename_map.insert(old.clone(), fresh);
                        }
                        let id = crate::mutate::generate_block_id(|c| taken.contains(c));
                        // A reference's block-id under the new home: a colliding id maps
                        // to its fresh rename, any other nested id keeps its id, a
                        // whole-file reference takes the new record id.
                        let map_bid =
                            |b: &Option<tb_references::BlockId>| -> Option<tb_references::BlockId> {
                                match b {
                                    // A nested id keeps its mode; only the id maps to
                                    // its fresh rename if it collided.
                                    Some(b) => Some(tb_references::BlockId {
                                        id: rename_map
                                            .get(&b.id)
                                            .cloned()
                                            .unwrap_or_else(|| b.id.clone()),
                                        referent: b.referent,
                                    }),
                                    // A whole-file record reference now points at the
                                    // inline record, so it must pull the record's VALUE:
                                    // a block-referent `^^id`, not a navigational `^id`
                                    // (which would resolve to the host file instead).
                                    None => Some(tb_references::BlockId {
                                        id: id.clone(),
                                        referent: true,
                                    }),
                                }
                            };
                        // `file`'s frontmatter is the record body. Rewrite it before
                        // folding: rename the colliding `^:` declarations, and rebase
                        // `file`'s references to its own blocks to the local form so they
                        // resolve in the host once `file` is gone.
                        let self_form = |w: &tb_references::WikilinkRef| {
                            let mut nw = w.clone();
                            nw.target = String::new();
                            nw.block_id = map_bid(&w.block_id);
                            nw.anchor = None;
                            // A local reference has no repo qualifier; a `::`-qualified
                            // self-reference would otherwise serialize to a malformed
                            // empty-target `[[::repo^id]]`.
                            nw.repo = None;
                            nw
                        };
                        file_edits.extend(crate::rename::ref_edits(
                            &file_content,
                            &file_self,
                            &self_form,
                        )?);
                        let file_content = crate::rename::apply_edits(&file_content, file_edits)?;
                        let (frontmatter, has_body) =
                            crate::inline::frontmatter_and_has_body(&file_content, file_is_md)?;
                        if has_body {
                            return Err(crate::mutate::MutationReject::new(
                                "the file to inline has a non-empty body — a record has no body",
                            ));
                        }
                        // The fold, plus the host's other references to the local
                        // `[[^id]]`, applied as one last-first pass.
                        let (fold_span, fold_text) =
                            crate::inline::fold_edit(&into_content, value_span, &frontmatter, &id)?;
                        let local = |w: &tb_references::WikilinkRef| {
                            let mut nw = w.clone();
                            nw.target = String::new();
                            nw.block_id = map_bid(&w.block_id);
                            nw.anchor = None;
                            // A local reference drops the repo qualifier, see `self_form`.
                            nw.repo = None;
                            nw
                        };
                        let mut into_edits = vec![(fold_span, fold_text)];
                        into_edits.extend(crate::rename::ref_edits(
                            &into_content,
                            &into_other,
                            &local,
                        )?);
                        let into_after = crate::rename::apply_edits(&into_content, into_edits)?;
                        // Cross-file referrers become `[[host^id]]`, the repo qualifier
                        // and fragments preserved, the block-id mapped the same way.
                        let cross_form = |w: &tb_references::WikilinkRef| {
                            let mut nw = w.clone();
                            nw.target = into_stem.clone();
                            nw.block_id = map_bid(&w.block_id);
                            nw.anchor = None;
                            nw
                        };
                        for (source, spans) in &cross {
                            let rc = read_referrer_checked(
                                source,
                                cross_hashes.get(source).copied().flatten(),
                            )?;
                            let rewritten = crate::rename::rewrite_refs(&rc, spans, &cross_form)?;
                            // A referrer whose every reference is commit-pinned
                            // yields no edits, so the rewrite is byte-identical.
                            // Writing it anyway costs a disk write and wakes the
                            // watcher for a file that did not change.
                            if rewritten != rc {
                                std::fs::write(source, rewritten).map_err(|e| {
                                    crate::mutate::MutationReject::new(format!(
                                        "cannot write referrer {}: {e}",
                                        source.display()
                                    ))
                                })?;
                            }
                        }
                        // Write the host with the folded record, then delete the file.
                        let written = crate::mutate::write_file(&blocking_into, &into_after, None)?;
                        guard_unchanged_before_touch(&blocking_file, None)?;
                        std::fs::remove_file(&blocking_file).map_err(|e| {
                            crate::mutate::MutationReject::new(format!(
                                "cannot delete {}: {e}",
                                blocking_file.display()
                            ))
                        })?;
                        // Fold the stamp into the host file the content was pulled into.
                        fold_write_stamps(&blocking_into, &stamps, written)
                    })?;
                blocking_handle.rebuild_paths(saga_dirty_set(&plan));
                Ok::<_, crate::mutate::MutationReject>((written, commits))
            })
            .await
            .expect("mutation task never panics");
            match outcome {
                Err(reject) => mutation_reject_frame(reject),
                Ok((written, commits)) => mutation_response(handle, &into, Some(written), commits),
            }
        }
        Mutation::RenameBlockId(args) => {
            let host = match resolve_mutation_target(handle, &args.path) {
                Ok(t) => t,
                Err(frame) => return frame,
            };
            if args.block_id == args.to_block_id {
                return mutation_reject_frame(crate::mutate::MutationReject::new(
                    "`block_id` and `to_block_id` are the same — nothing to rename",
                ));
            }
            if !tb_parser::is_valid_block_id(&args.to_block_id) {
                return mutation_reject_frame(crate::mutate::MutationReject::new(format!(
                    "`to_block_id` {:?} is not a valid block-id — ids are [A-Za-z0-9_-]+",
                    args.to_block_id
                )));
            }
            // Over the held knowledge base: the host's repo membership, the declaration
            // span of `block_id` (the inline record's `^:` value), a check that
            // `to_block_id` is free in the host's file-local id namespace, and the
            // inbound referrers filtered to this block-id (cross-file
            // `[[host^id]]` and host-local `[[^id]]`).
            struct Located {
                decl_span: tb_diagnostics::ByteRange,
                host_hash: Option<crate::ir::ContentHash>,
                referrers: BTreeMap<PathBuf, Vec<tb_diagnostics::ByteRange>>,
                /// The held content hash of each referrer, for the read-before-write
                /// drift check when the saga re-reads it.
                referrer_hashes: BTreeMap<PathBuf, Option<crate::ir::ContentHash>>,
            }
            let gathered = handle.read(|v| {
                if v.repos.repo_of(&host).is_none() {
                    return Err("path escapes the workspace".to_string());
                }
                let entry = v
                    .catalog
                    .get(&host)
                    .ok_or_else(|| "the host file is not in the knowledge base".to_string())?;
                let inst =
                    match entry.parse.as_ref() {
                        FileParse::Instance {
                            instance: Some(inst),
                            ..
                        } => inst,
                        _ => return Err(
                            "the host file is not a typed instance — no inline record to rename"
                                .to_string(),
                        ),
                    };
                let targets = crate::resolution_build::record_targets_of_kb(v, &host, inst);
                let decl_span = targets.get(&args.block_id).map(|t| t.span).ok_or_else(|| {
                    format!(
                        "no inline record carries block-id `{}` in {}; block-id rename covers \
                         inline `^:` records only",
                        args.block_id,
                        host.display()
                    )
                })?;
                if host_block_ids(entry.parse.as_ref(), &targets).contains(&args.to_block_id) {
                    return Err(format!(
                        "block-id `{}` already exists in {} — pick a free id",
                        args.to_block_id,
                        host.display()
                    ));
                }
                let mut referrers: BTreeMap<PathBuf, Vec<tb_diagnostics::ByteRange>> =
                    BTreeMap::new();
                let mut referrer_hashes: BTreeMap<PathBuf, Option<crate::ir::ContentHash>> =
                    BTreeMap::new();
                for bl in v.backlinks(&host) {
                    if bl.block_id.as_ref().map(|b| b.id.as_str()) == Some(args.block_id.as_str()) {
                        referrers
                            .entry(bl.source.clone())
                            .or_default()
                            .push(bl.span);
                        referrer_hashes
                            .entry(bl.source.clone())
                            .or_insert_with(|| v.catalog.get(&bl.source).and_then(|e| e.hash));
                    }
                }
                Ok(Located {
                    decl_span,
                    host_hash: entry.hash,
                    referrers,
                    referrer_hashes,
                })
            });
            let Located {
                decl_span,
                host_hash,
                referrers,
                referrer_hashes,
            } = match gathered {
                crate::Read::Ready { value: Ok(g), .. } => g,
                crate::Read::Ready {
                    value: Err(msg), ..
                } => {
                    return mutation_reject_frame(crate::mutate::MutationReject::new(msg));
                }
                crate::Read::NotReady => {
                    return serde_json::to_value(Response::not_ready())
                        .expect("response serializes");
                }
            };
            let mut touched = vec![host.clone()];
            touched.extend(referrers.keys().filter(|p| **p != host).cloned());
            // A referrer this block-id rename would rewrite that lives in a consumed
            // member rejects the whole refactor.
            let plan = match refactor_saga_plan(handle, touched, referrers.clone()).await {
                Ok(plan) => plan,
                Err(reject) => return mutation_reject_frame(reject),
            };
            if let Some(reject) = grant_refusal(&plan, &args.allowed_members, "rename_block_id") {
                return mutation_reject_frame(reject);
            }
            let marker_dir = saga_marker_dir(handle.root());
            let summary = format!(
                "rename block-id {} -> {} in {}",
                args.block_id, args.to_block_id, args.path
            );
            let blocking_handle = handle.clone();
            let blocking_host = host.clone();
            let stamps = args.stamps;
            let to_block_id = args.to_block_id.clone();
            let outcome = tokio::task::spawn_blocking(move || {
                // A referrer keeps its target, anchor, `:field`, and `::repo`;
                // only the block-id swaps to the new id. A local-form `[[^id]]`
                // has an empty target, so it re-serializes as `[[^newId]]`.
                let transform = |w: &tb_references::WikilinkRef| {
                    let mut nw = w.clone();
                    // Swap the id, preserve the referrer's own mode: a `^^id`
                    // block-referent stays block-referent, a bare `^id` stays
                    // navigational.
                    let referent = w.block_id.as_ref().is_some_and(|b| b.referent);
                    nw.block_id = Some(tb_references::BlockId {
                        id: to_block_id.clone(),
                        referent,
                    });
                    nw
                };
                let (written, commits) =
                    apply_saga(&ShellGit, &marker_dir, &plan, summary, true, || {
                        let content = std::fs::read_to_string(&blocking_host).map_err(|e| {
                            crate::mutate::MutationReject::new(format!(
                                "cannot read {}: {e}",
                                blocking_host.display()
                            ))
                        })?;
                        if host_hash != Some(crate::ir::ContentHash::of(content.as_bytes())) {
                            return Err(crate::mutate::MutationReject::new(
                            "the file changed since the engine last read it — wait for the rebuild \
                             and retry",
                        ));
                        }
                        // The declaration (the `^:` value) plus any host-local
                        // referrers, merged into one last-first pass so neither shifts
                        // the other, whatever their order in the file.
                        let mut host_edits = vec![(decl_span, to_block_id.clone())];
                        if let Some(spans) = referrers.get(&blocking_host) {
                            host_edits
                                .extend(crate::rename::ref_edits(&content, spans, &transform)?);
                        }
                        let host_after = crate::rename::apply_edits(&content, host_edits)?;
                        // Every other referrer, rewritten in place.
                        for (source, spans) in &referrers {
                            if *source == blocking_host {
                                continue;
                            }
                            let rc = read_referrer_checked(
                                source,
                                referrer_hashes.get(source).copied().flatten(),
                            )?;
                            let rewritten = crate::rename::rewrite_refs(&rc, spans, &transform)?;
                            // A referrer whose every reference is commit-pinned
                            // yields no edits, so the rewrite is byte-identical.
                            // Writing it anyway costs a disk write and wakes the
                            // watcher for a file that did not change.
                            if rewritten != rc {
                                std::fs::write(source, rewritten).map_err(|e| {
                                    crate::mutate::MutationReject::new(format!(
                                        "cannot write referrer {}: {e}",
                                        source.display()
                                    ))
                                })?;
                            }
                        }
                        let written = crate::mutate::write_file(&blocking_host, &host_after, None)?;
                        // Fold the stamp into the host file carrying the record.
                        fold_write_stamps(&blocking_host, &stamps, written)
                    })?;
                blocking_handle.rebuild_paths(saga_dirty_set(&plan));
                Ok::<_, crate::mutate::MutationReject>((written, commits))
            })
            .await
            .expect("mutation task never panics");
            match outcome {
                Err(reject) => mutation_reject_frame(reject),
                Ok((written, commits)) => mutation_response(handle, &host, Some(written), commits),
            }
        }
        Mutation::SetIgnores(args) => set_ignores(handle, args).await,
        Mutation::SetConfig(args) => set_config(handle, args).await,
        Mutation::Register(args) => register(handle, args).await,
        Mutation::AddDep(args) => add_dep(handle, args).await,
        Mutation::RemoveDep(args) => remove_dep(handle, args).await,
        Mutation::AddWorkspaceMember(args) => add_workspace_member(handle, args).await,
        Mutation::RemoveWorkspaceMember(args) => remove_workspace_member(handle, args).await,
        Mutation::SetWorkspaceMemberRole(args) => set_workspace_member_role(handle, args).await,
        Mutation::SetWorkspaceMemberDisabled(args) => {
            set_workspace_member_disabled(handle, args).await
        }
    }
}

/// Resolve an EDITABLE member root for a composition write. `root` names a
/// declared member root; absent means the served entry. `Err` carries the exact
/// frame to return: not-ready, the not-editable reject (a consumed member), or a
/// no-such-member reject. The composition-verb prelude, mirroring
/// `set_config_repo`'s member gate.
fn resolve_editable_member_root(
    handle: &EngineHandle,
    root: Option<&str>,
) -> Result<(PathBuf, crate::repo::RepoName), serde_json::Value> {
    let want_root = root.map(|r| member_root_arg(handle.root(), r));
    let resolved = handle.read(move |v| {
        let m = match &want_root {
            Some(p) => v.repos.by_root(p),
            None => v.repos.entry(),
        };
        match m {
            None => Ok(None),
            Some(r) => {
                let role = crate::wire::member_role(v, &r.name);
                if role.editable() {
                    Ok(Some((r.root.clone(), r.name.clone())))
                } else {
                    Err(not_editable_reject(&r.name, role))
                }
            }
        }
    });
    match resolved.value() {
        None => Err(serde_json::to_value(Response::not_ready()).expect("response serializes")),
        Some(Err(reject)) => Err(mutation_reject_frame(reject)),
        Some(Ok(None)) => Err(mutation_reject_frame(crate::mutate::MutationReject::new(
            format!(
                "no declared member root for `{}`",
                root.unwrap_or("<entry>")
            ),
        ))),
        Some(Ok(Some(r))) => Ok(r),
    }
}

/// Run a COMPOSITION-sort write over `<member>/.arsumbris/<file>`: resolve the
/// editable member, splice via `produce`, CAS-write, commit per mutation, then
/// rebuild. A `.arsumbris/` path forces a full re-resolve (`classify_dirty` →
/// `NeedsFull`), so the mounted set / identity change lands. The shared
/// scaffolding for the composition-authorship verbs, the `set_config_repo` shape
/// plus a rebuild (like `write_file`).
///
/// `produce` receives the current file content (`None` when the file is absent, so
/// a create path can seed it) and the containing repo's name (for a `workspace.yaml`
/// seed that must list its own repo in `edit:`). See
/// [[spec - composition authorship - governed list verbs author workspace members and repo deps, rebuilding on change]].
async fn composition_write(
    handle: &EngineHandle,
    root: Option<&str>,
    file: &str,
    summary: String,
    expected_hash: Option<String>,
    produce: impl FnOnce(Option<&str>, &Path, &str) -> Result<String, crate::mutate::MutationReject>
    + Send
    + 'static,
) -> serde_json::Value {
    let (member_root, member_name) = match resolve_editable_member_root(handle, root) {
        Ok(r) => r,
        Err(frame) => return frame,
    };
    let path = crate::repo::identity_dir_for_write(&member_root).join(file);
    let plan = saga_plan_off_thread(handle, vec![path.clone()]).await;
    let marker_dir = saga_marker_dir(handle.root());
    let write_path = path.clone();
    let containing = member_name.0;
    let blocking_handle = handle.clone();
    let outcome = tokio::task::spawn_blocking(move || {
        let (hash, commits) = apply_saga(&ShellGit, &marker_dir, &plan, summary, false, || {
            // Absent is `None`, so a create path seeds a fresh file; a real read
            // error propagates as a reject.
            let current = match std::fs::read_to_string(&write_path) {
                Ok(s) => Some(s),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                Err(e) => {
                    return Err(crate::mutate::MutationReject::new(format!(
                        "cannot read {}: {e}",
                        write_path.display()
                    )));
                }
            };
            let new = produce(current.as_deref(), &write_path, &containing)?;
            crate::mutate::write_file(&write_path, &new, expected_hash.as_deref())
        })?;
        // A `.arsumbris/` composition change re-resolves the whole mounted set.
        blocking_handle.rebuild_paths(saga_dirty_set(&plan));
        Ok::<_, crate::mutate::MutationReject>((hash, commits))
    })
    .await
    .expect("mutation task never panics");
    match outcome {
        Err(reject) => mutation_reject_frame(reject),
        Ok((hash, commits)) => mutation_response(handle, &path, Some(hash), commits),
    }
}

/// The `add_dep` composition mutation: add a `{ name, remote?, ref? }` entry to a
/// repo's `deps:`, authoring `<member>/.arsumbris/repo.yaml`. Does NOT fetch; the
/// new peer stays `peer-unmounted` until `resolve`. A duplicate `name` rejects.
async fn add_dep(handle: &EngineHandle, args: AddDepArgs) -> serde_json::Value {
    let mut dep = serde_json::Map::new();
    dep.insert("name".into(), serde_json::Value::String(args.name.clone()));
    if let Some(remote) = args.remote {
        dep.insert("remote".into(), serde_json::Value::String(remote));
    }
    if let Some(r) = args.ref_field {
        dep.insert("ref".into(), serde_json::Value::String(r));
    }
    let element = serde_json::Value::Object(dep);
    let mut m = serde_json::Map::new();
    m.insert("name".into(), serde_json::Value::String(args.name.clone()));
    let matcher = crate::mutate::ElementMatch::Fields(m);
    composition_write(
        handle,
        args.root.as_deref(),
        "repo.yaml",
        format!("add_dep {}", args.name),
        args.expected_hash,
        move |content, path, _containing| {
            // A repo always has a repo.yaml; None means it vanished mid-write.
            let content = content
                .ok_or_else(|| crate::mutate::MutationReject::new("repo.yaml is missing"))?;
            crate::mutate::splice_append_element(content, path, "deps", &element, &matcher)
        },
    )
    .await
}

/// The `remove_dep` composition mutation: remove a dep entry by name from a
/// repo's `deps:`. An absent name rejects.
async fn remove_dep(handle: &EngineHandle, args: RemoveDepArgs) -> serde_json::Value {
    let mut m = serde_json::Map::new();
    m.insert("name".into(), serde_json::Value::String(args.name));
    let matcher = crate::mutate::ElementMatch::Fields(m);
    composition_write(
        handle,
        args.root.as_deref(),
        "repo.yaml",
        "remove_dep".to_string(),
        args.expected_hash,
        move |content, path, _containing| {
            let content = content
                .ok_or_else(|| crate::mutate::MutationReject::new("repo.yaml is missing"))?;
            crate::mutate::splice_remove_element(content, path, "deps", &matcher)
        },
    )
    .await
}

/// The wire field name for a workspace member role.
fn role_field(role: WorkspaceRole) -> &'static str {
    match role {
        WorkspaceRole::Edit => "edit",
        WorkspaceRole::Discover => "discover",
    }
}

/// The `add_workspace_member` composition mutation: add a member to a workspace's
/// `edit:` or `discover:` list, authoring `<member>/.arsumbris/workspace.yaml`.
/// Creates the file (seeded with the containing repo in `edit:`) when absent. A
/// member already in EITHER list rejects (use `set_workspace_member_role` to
/// change a role). The containing repo cannot be a `discover` member.
async fn add_workspace_member(
    handle: &EngineHandle,
    args: AddWorkspaceMemberArgs,
) -> serde_json::Value {
    let target = role_field(args.role);
    let other = role_field(args.role.other());
    let name = args.name.clone();
    composition_write(
        handle,
        args.root.as_deref(),
        "workspace.yaml",
        format!("add_workspace_member {name}"),
        args.expected_hash,
        move |content, path, containing| {
            if matches!(args.role, WorkspaceRole::Discover) && name == containing {
                return Err(crate::mutate::MutationReject::new(
                    "the containing repo belongs in 'edit', not 'discover'",
                ));
            }
            let element = serde_json::Value::String(name.clone());
            let matcher = crate::mutate::ElementMatch::Scalar(element.clone());
            match content {
                // Create: seed a self-describing, self-complete workspace.yaml
                // (the containing repo in edit:), then add the member to its role.
                None => {
                    let mut seed = format!(
                        "type: tb.engine.workspace::tb-engine\n{}",
                        crate::yaml_render::field(
                            "edit",
                            &serde_json::Value::Array(vec![serde_json::Value::String(
                                containing.to_string()
                            )]),
                            0,
                        )
                    );
                    if name != containing {
                        seed = crate::mutate::splice_append_element(
                            &seed, path, target, &element, &matcher,
                        )?;
                    }
                    Ok(seed)
                }
                Some(content) => {
                    // A member holds one role, so reject if it is already in the
                    // other list; the mover is set_workspace_member_role.
                    if crate::mutate::sequence_has(content, path, other, &matcher)? {
                        return Err(crate::mutate::MutationReject::new(format!(
                            "'{name}' is already a '{other}' member; use set_workspace_member_role to change its role"
                        )));
                    }
                    crate::mutate::splice_append_element(content, path, target, &element, &matcher)
                }
            }
        },
    )
    .await
}

/// The `remove_workspace_member` composition mutation: remove a member from
/// `edit:` / `discover:`, every one holding it. An absent member rejects.
///
/// The containing repo cannot be removed: a `workspace.yaml` lists its own repo in
/// `edit:`, so removing it would author `workspace-omits-containing-repo`. A
/// member also named in the `disabled:` overlay leaves it in the same splice, so
/// the removal never strands a `disabled-member-not-declared` entry.
async fn remove_workspace_member(
    handle: &EngineHandle,
    args: RemoveWorkspaceMemberArgs,
) -> serde_json::Value {
    let name = args.name.clone();
    composition_write(
        handle,
        args.root.as_deref(),
        "workspace.yaml",
        format!("remove_workspace_member {name}"),
        args.expected_hash,
        move |content, path, containing| {
            let content = content.ok_or_else(|| {
                crate::mutate::MutationReject::new("workspace.yaml is absent, nothing to remove")
            })?;
            if name == containing {
                return Err(crate::mutate::MutationReject::new(format!(
                    "'{name}' is the containing repo; a workspace.yaml lists its own repo in 'edit', so it cannot be removed"
                )));
            }
            let matcher =
                crate::mutate::ElementMatch::Scalar(serde_json::Value::String(name.clone()));
            // The name leaves the workspace, so it leaves EVERY list holding it. A
            // hand-authored role conflict (`edit:` and `discover:` both) would
            // otherwise keep the name declared in the other list, and the
            // `disabled:` drop below would silently re-mount it.
            let mut removed = content.to_string();
            let mut found = false;
            for role in ["edit", "discover"] {
                if crate::mutate::sequence_has(&removed, path, role, &matcher)? {
                    removed = crate::mutate::splice_remove_element(&removed, path, role, &matcher)?;
                    found = true;
                }
            }
            if !found {
                return Err(crate::mutate::MutationReject::new(format!(
                    "'{name}' is not a workspace member, nothing to remove"
                )));
            }
            if crate::mutate::sequence_has(&removed, path, "disabled", &matcher)? {
                crate::mutate::splice_remove_element(&removed, path, "disabled", &matcher)
            } else {
                Ok(removed)
            }
        },
    )
    .await
}

/// The `set_workspace_member_role` composition mutation: move a member between
/// `edit:` and `discover:` in one commit. Rejects if the member is absent, already
/// in the target role, or (into `discover`) the containing repo.
async fn set_workspace_member_role(
    handle: &EngineHandle,
    args: SetWorkspaceMemberRoleArgs,
) -> serde_json::Value {
    let target = role_field(args.role);
    let source = role_field(args.role.other());
    let name = args.name.clone();
    composition_write(
        handle,
        args.root.as_deref(),
        "workspace.yaml",
        format!("set_workspace_member_role {name}"),
        args.expected_hash,
        move |content, path, containing| {
            let content = content.ok_or_else(|| {
                crate::mutate::MutationReject::new("workspace.yaml is absent, no member to re-role")
            })?;
            if matches!(args.role, WorkspaceRole::Discover) && name == containing {
                return Err(crate::mutate::MutationReject::new(
                    "the containing repo belongs in 'edit', not 'discover'",
                ));
            }
            let element = serde_json::Value::String(name.clone());
            let matcher = crate::mutate::ElementMatch::Scalar(element.clone());
            crate::mutate::splice_move_element(content, path, source, target, &element, &matcher)
        },
    )
    .await
}

/// The `set_workspace_member_disabled` composition mutation: add a member to, or
/// remove it from, the workspace's `disabled:` overlay, in one commit.
///
/// Disabling needs a DECLARED member, one in `edit:` or `discover:`: the overlay
/// silences a declared member, so an undeclared name would author
/// `disabled-member-not-declared`. The containing repo cannot be disabled, it is
/// the workspace's own home. Re-enabling needs the name present in `disabled:`,
/// and removing the last one leaves `disabled: []`, the list-verb convention. The
/// role lists are never touched, disabling overlays a role, it does not rewrite it.
async fn set_workspace_member_disabled(
    handle: &EngineHandle,
    args: SetWorkspaceMemberDisabledArgs,
) -> serde_json::Value {
    let name = args.name.clone();
    let disabled = args.disabled;
    composition_write(
        handle,
        args.root.as_deref(),
        "workspace.yaml",
        format!(
            "set_workspace_member_disabled {name} {}",
            if disabled { "off" } else { "on" }
        ),
        args.expected_hash,
        move |content, path, containing| {
            let content = content.ok_or_else(|| {
                crate::mutate::MutationReject::new(
                    "workspace.yaml is absent, it declares no member to switch",
                )
            })?;
            let element = serde_json::Value::String(name.clone());
            let matcher = crate::mutate::ElementMatch::Scalar(element.clone());
            if !disabled {
                if !crate::mutate::sequence_has(content, path, "disabled", &matcher)? {
                    return Err(crate::mutate::MutationReject::new(format!(
                        "'{name}' is not disabled, nothing to re-enable"
                    )));
                }
                return crate::mutate::splice_remove_element(content, path, "disabled", &matcher);
            }
            if name == containing {
                return Err(crate::mutate::MutationReject::new(format!(
                    "'{name}' is the containing repo; the workspace's own repo cannot be disabled"
                )));
            }
            let declared = crate::mutate::sequence_has(content, path, "edit", &matcher)?
                || crate::mutate::sequence_has(content, path, "discover", &matcher)?;
            if !declared {
                return Err(crate::mutate::MutationReject::new(format!(
                    "'{name}' is not a declared member; list it in 'edit' or 'discover' before disabling it"
                )));
            }
            if crate::mutate::sequence_has(content, path, "disabled", &matcher)? {
                return Err(crate::mutate::MutationReject::new(format!(
                    "'{name}' is already disabled"
                )));
            }
            crate::mutate::splice_append_element(content, path, "disabled", &element, &matcher)
        },
    )
    .await
}

/// The `register` config mutation: write or update one `{ name, remote, path }`
/// entry in the per-user registry (`repos.yaml`), through the governed channel.
///
/// A DEVICE-GLOBAL write, the file sits outside every repo, so there is NO git
/// commit, unlike `set_ignores`. The registry path comes from the engine's
/// injected device root (a test tempdir) or the real `$HOME/.arsumbris`; absent
/// when `$HOME` is unset, which rejects (nowhere to write). The
/// identity checks live in `register_entry`: a disagreeing remote, or a path
/// whose `repo.yaml` declares another name, rejects as
/// `dependency-identity-conflict`, nothing written. On success the registry
/// changed, so resolution changed: a full rebuild mounts a newly-registered
/// member before the response carries fresh state.
async fn register(handle: &EngineHandle, args: RegisterArgs) -> serde_json::Value {
    // The caller `handle_mutation` already holds the write lock; re-taking it here
    // would deadlock. The registry path is device-scoped, injected for tests.
    let Some(registry_path) = crate::repo::user_registry_path(handle.config()) else {
        return mutation_reject_frame(crate::mutate::MutationReject::new(
            "no device root (set $HOME) to register into".to_string(),
        ));
    };
    let name = crate::repo::RepoName(args.name.clone());
    let path = abs_arg(handle.root(), &args.path);
    let path_str = path.display().to_string();
    let remote = args.remote.clone();

    // The write plus rebuild are blocking (std::fs + a full build), off the
    // runtime workers like the other config mutations.
    let blocking_handle = handle.clone();
    let outcome =
        tokio::task::spawn_blocking(move || -> std::io::Result<crate::repo::RegisterOutcome> {
            let res = crate::repo::register_entry(&registry_path, &name, remote.as_deref(), &path)?;
            if matches!(res, crate::repo::RegisterOutcome::Written) {
                blocking_handle.rebuild();
            }
            Ok(res)
        })
        .await
        .expect("register task never panics");

    match outcome {
        Err(e) => mutation_reject_frame(crate::mutate::MutationReject::new(format!(
            "register write failed: {e}"
        ))),
        Ok(crate::repo::RegisterOutcome::Conflict(msg)) => {
            mutation_reject_frame(crate::mutate::MutationReject::new(msg))
        }
        Ok(crate::repo::RegisterOutcome::Written) => serde_json::json!({
            "type": "registered",
            "schema_version": SCHEMA_VERSION,
            "name": args.name,
            "path": path_str,
            "version": handle.version(),
        }),
    }
}

/// How `set_config` produces the bytes to write: a whole file, or a keyed edit.
/// Built once from the args (exactly one form), then applied against the CURRENT
/// file content inside the write's critical section.
enum ConfigWrite {
    Content(String),
    Edit {
        field_path: Vec<tb_core::PathSegment>,
        patch: serde_json::Map<String, serde_json::Value>,
    },
}

/// The `set_config` mutation: write one consumer config file under the scoped
/// channel, a whole `content` or a keyed `edit`. Path-safety (and the reserved
/// `au-engine` owner segment) gate the two caller-controlled segments up front,
/// independent of scope. Repo scope commits per mutation through the saga; machine
/// scope writes a device-global file with no commit.
async fn set_config(handle: &EngineHandle, args: SetConfigArgs) -> serde_json::Value {
    // The caller `handle_mutation` already holds the write lock and gated on
    // readiness. Path-safety is checked at the verb, before any scope resolution.
    if let Err(e) = crate::repo::check_config_segments(&args.consumer, &args.file) {
        return mutation_reject_frame(crate::mutate::MutationReject::new(e));
    }
    // Exactly one of `content` / `edit`.
    let write = match (args.content, args.edit) {
        (Some(content), None) => ConfigWrite::Content(content),
        (None, Some(edit)) => ConfigWrite::Edit {
            field_path: edit.field_path.into_iter().map(Into::into).collect(),
            patch: edit.patch,
        },
        (Some(_), Some(_)) => {
            return mutation_reject_frame(crate::mutate::MutationReject::new(
                "set_config takes `content` OR `edit`, not both",
            ));
        }
        (None, None) => {
            return mutation_reject_frame(crate::mutate::MutationReject::new(
                "set_config needs `content` or `edit`",
            ));
        }
    };
    let ctx = SetConfigCtx {
        consumer: args.consumer,
        file: args.file,
        type_name: args.type_name,
        root: args.root,
        write,
        expected_hash: args.expected_hash,
    };
    match args.scope {
        ConfigScope::Repo => set_config_repo(handle, ctx).await,
        ConfigScope::Machine => set_config_machine(handle, ctx).await,
    }
}

/// The scope-independent inputs a `set_config` write carries past the args parse.
struct SetConfigCtx {
    consumer: String,
    file: String,
    type_name: String,
    root: Option<String>,
    write: ConfigWrite,
    expected_hash: Option<String>,
}

/// Produce the content `set_config` writes, from the CURRENT on-disk content
/// (`None` when absent). The whole-`content` form injects the declared `type:`
/// when the content does not already self-describe; the `edit` form injects it
/// into the current file first (so the splice parses a typed record and the file
/// self-describes), then splices one keyed record, comments and siblings
/// preserved. The splice's own structural check rejects a change that would break
/// the YAML.
fn produce_config_content(
    current: Option<&str>,
    write: &ConfigWrite,
    type_name: &str,
    path: &Path,
) -> Result<String, crate::mutate::MutationReject> {
    let inject = |c: &str| {
        if crate::parse::content_declares_type(c) {
            c.to_string()
        } else {
            format!("type: {type_name}\n{c}")
        }
    };
    match write {
        ConfigWrite::Content(content) => Ok(inject(content)),
        ConfigWrite::Edit { field_path, patch } => {
            let current = current.ok_or_else(|| {
                crate::mutate::MutationReject::new(
                    "cannot `edit` an absent config file; create it first with `content`",
                )
            })?;
            let based = inject(current);
            crate::mutate::splice_edit_record(&based, path, field_path, patch)
        }
    }
}

/// The machine-scope `set_config` write: a device-global file under
/// `~/.arsumbris/<consumer>/config/<file>`, resolved from the engine's
/// `ConfigSource` (a test tempdir, the real `$HOME/.arsumbris`, or nothing). NO
/// git commit (the file sits outside every repo) and NO rebuild (it is not a
/// graph node), the shape `register` uses. A cross-daemon file lock serializes
/// concurrent writers of the one device-global file, and the produce-write is one
/// critical section under it so a keyed `edit`'s read + splice + write stays
/// atomic. `expected_hash` is a compare-and-set under the lock.
async fn set_config_machine(handle: &EngineHandle, ctx: SetConfigCtx) -> serde_json::Value {
    let Some(base) = handle.config().base() else {
        return mutation_reject_frame(crate::mutate::MutationReject::new(
            "no device root (set $HOME) to write machine-scope config into",
        ));
    };
    let path = match crate::repo::config_path(&base, &ctx.consumer, &ctx.file) {
        Ok(p) => p,
        Err(e) => return mutation_reject_frame(crate::mutate::MutationReject::new(e)),
    };
    let write_path = path.clone();
    let SetConfigCtx {
        type_name,
        write,
        expected_hash,
        ..
    } = ctx;
    let outcome = tokio::task::spawn_blocking(move || {
        crate::repo::write_device_config_with(&write_path, expected_hash.as_deref(), |current| {
            produce_config_content(current, &write, &type_name, &write_path)
        })
    })
    .await
    .expect("mutation task never panics");

    match outcome {
        Err(reject) => mutation_reject_frame(reject),
        // No commit and no catalog entry, so an EMPTY commits set (`commit` is
        // null off-repo) and `written_hash` None (out-of-band, `reflected` true),
        // with the written hash surfaced via `extra`. The uniform mutation frame,
        // minus the commit the repo scope carries.
        Ok(hash) => {
            let mut extra = serde_json::Map::new();
            extra.insert(
                "hash".into(),
                serde_json::Value::String(crate::mutate::hash_hex(hash)),
            );
            mutation_response_extra(handle, &path, None, SagaCommits::default(), extra)
        }
    }
}

/// The repo-scope `set_config` write: construct `<member>/.arsumbris/<consumer>/
/// config/<file>` directly (a guard bypass, like `set_ignores`), inject `type:` so
/// the file self-describes, `expected_hash` compare-and-set, then commit per
/// mutation through the config-mutation saga. The file rides the `.arsumbris`
/// floor, so it is out-of-band: no catalog entry, no re-scope, no rebuild — the
/// graph is unaffected, so the held version does not move.
async fn set_config_repo(handle: &EngineHandle, ctx: SetConfigCtx) -> serde_json::Value {
    // Resolve the member root: an explicit `root` (a declared member), else the
    // served entry. The type is validated only where it resolves; the write itself
    // is store-and-advisory, never refused on a shape verdict.
    let want_root = ctx
        .root
        .as_deref()
        .map(|r| member_root_arg(handle.root(), r));
    // Resolve the owning member AND gate it on editability: repo-scope config is a
    // committed write into the member's `.arsumbris/`, so a consumed (`discover` /
    // `dep`) member is not the engine's to author, exactly like any other write.
    // `Ok(None)` names no member, `Err` is the not-editable reject.
    let resolved = handle.read(move |v| {
        let m = match &want_root {
            Some(p) => v.repos.by_root(p),
            None => v.repos.entry(),
        };
        match m {
            None => Ok(None),
            Some(r) => {
                let role = crate::wire::member_role(v, &r.name);
                if role.editable() {
                    Ok(Some(r.root.clone()))
                } else {
                    Err(not_editable_reject(&r.name, role))
                }
            }
        }
    });
    let member_root = match resolved.value() {
        None => return serde_json::to_value(Response::not_ready()).expect("response serializes"),
        Some(Err(reject)) => return mutation_reject_frame(reject),
        Some(Ok(None)) => {
            return mutation_reject_frame(crate::mutate::MutationReject::new(format!(
                "repo scope needs a declared member root; `{}` names none",
                ctx.root.as_deref().unwrap_or("<entry>")
            )));
        }
        Some(Ok(Some(root))) => root,
    };

    let path = match crate::repo::config_path(
        &crate::repo::identity_dir_for_write(&member_root),
        &ctx.consumer,
        &ctx.file,
    ) {
        Ok(p) => p,
        Err(e) => return mutation_reject_frame(crate::mutate::MutationReject::new(e)),
    };

    let plan = saga_plan_off_thread(handle, vec![path.clone()]).await;
    let marker_dir = saga_marker_dir(handle.root());
    let summary = format!("set_config {}/{}", ctx.consumer, ctx.file);
    let write_path = path.clone();
    let SetConfigCtx {
        type_name,
        write,
        expected_hash,
        ..
    } = ctx;

    let outcome = tokio::task::spawn_blocking(move || {
        // The read + produce + write is inside the saga's write closure (the write
        // lock is held), so a keyed `edit`'s splice sees the current bytes and the
        // `expected_hash` compare-and-set guards them. No rebuild: the file is
        // floored, so no graph state changed.
        apply_saga(&ShellGit, &marker_dir, &plan, summary, false, || {
            let current = std::fs::read_to_string(&write_path).ok();
            let new = produce_config_content(current.as_deref(), &write, &type_name, &write_path)?;
            crate::mutate::write_file(&write_path, &new, expected_hash.as_deref())
        })
    })
    .await
    .expect("mutation task never panics");

    match outcome {
        Err(reject) => mutation_reject_frame(reject),
        Ok((hash, commits)) => {
            // The file is out-of-band (not catalogued), so `written_hash` is None
            // — nothing to lag, `reflected` stays true, like `set_ignores`. The
            // written hash is surfaced explicitly so the consumer's next
            // `expected_hash` compare-and-set has it.
            let mut extra = serde_json::Map::new();
            extra.insert(
                "hash".into(),
                serde_json::Value::String(crate::mutate::hash_hex(hash)),
            );
            mutation_response_extra(handle, &path, None, commits, extra)
        }
    }
}

/// The `set_ignores` config mutation: replace a member's `.auignore` through the
/// governed channel. Distinct from the graph mutations above — it edits the
/// scope config that shapes which content enters the graph, not graph content,
/// so it MAY orphan references (advisory), and the strict no-dangling contract is
/// relaxed for it deliberately.
///
/// The `.arsumbris/` write-guard on the generic verbs stays absolute: this verb
/// does not route through the guarded resolver, it constructs the SINGLE allowed
/// path `<root>/.arsumbris/.auignore` from a validated member root and writes
/// that directly. So the exception is one tightly-scoped path, never a hole in
/// the guard.
///
/// The sequence is the mutation-channel shape: validate the patterns up front (a
/// malformed pattern rejects with the reason, nothing written), write (or, for
/// an empty list, remove) the file, commit per mutation via the saga (a shared
/// `Mutation-Id` trailer, like any governed write), then re-scope — the
/// `.arsumbris/` change forces a full re-walk (already wired) — and respond with
/// fresh state.
async fn set_ignores(handle: &EngineHandle, args: SetIgnoresArgs) -> serde_json::Value {
    // The caller `handle_mutation` already holds the write lock and gated on
    // readiness; re-taking the lock here would deadlock (it is not re-entrant).
    // `root` must name a declared member root (the `ignores` read surfaces it),
    // any spelling that canonicalizes to one. A subdir or a path under no member
    // rejects: the `.auignore` is per-member-root.
    let member_root = member_root_arg(handle.root(), &args.root);
    // Resolve the owning member AND gate on editability: `.auignore` is a committed
    // write into the member's `.arsumbris/`, so a consumed (`discover` / `dep`)
    // member is not the engine's to author. `None` names no member.
    let member = handle
        .read(|v| {
            v.repos
                .by_root(&member_root)
                .map(|r| (r.name.clone(), crate::wire::member_role(v, &r.name)))
        })
        .value()
        .flatten();
    let Some((member_name, role)) = member else {
        return mutation_reject_frame(crate::mutate::MutationReject::new(format!(
            "root must name a declared member root (the `ignores` read surfaces it): {}",
            args.root
        )));
    };
    if !role.editable() {
        return mutation_reject_frame(not_editable_reject(&member_name, role));
    }

    // Validate the patterns before touching disk: build the same `WalkFilter` the
    // build uses; a malformed pattern rejects with the reason, nothing written.
    // This is the win of the governed path over a raw file write, which would
    // degrade the same pattern to the `auignore-load-error` advisory.
    let joined = args.patterns.join("\n");
    if let Err(e) = tb_parser::WalkFilter::with_auignore(&member_root, &joined) {
        return mutation_reject_frame(crate::mutate::MutationReject::new(format!(
            "malformed .auignore pattern, nothing written: {e}"
        )));
    }

    let auignore = crate::repo::identity_dir_for_write(&member_root).join(".auignore");
    let remove = args.patterns.is_empty();
    let plan = saga_plan_off_thread(handle, vec![auignore.clone()]).await;
    let marker_dir = saga_marker_dir(handle.root());
    let summary = format!("set_ignores {}", args.root);

    let blocking_handle = handle.clone();
    let outcome = tokio::task::spawn_blocking(move || {
        let ((), commits) = apply_saga(&ShellGit, &marker_dir, &plan, summary, false, || {
            if remove {
                // Empty list: remove the file, reverting to the default
                // excludes. An already-absent file is a clean no-op (the saga
                // sees no diff and makes no commit), never the reject
                // `delete_file` gives for a missing target.
                guard_unchanged_before_touch(&auignore, None)?;
                match std::fs::remove_file(&auignore) {
                    Ok(()) => Ok(()),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                    Err(e) => Err(crate::mutate::MutationReject::new(format!(
                        "cannot remove {}: {e}",
                        auignore.display()
                    ))),
                }
            } else {
                // One pattern per line, trailing newline, so the file reads
                // back as clean lines.
                let content = format!("{joined}\n");
                crate::mutate::write_file(&auignore, &content, None).map(|_| ())
            }
        })?;
        blocking_handle.rebuild_paths(saga_dirty_set(&plan));
        Ok::<_, crate::mutate::MutationReject>(commits)
    })
    .await
    .expect("mutation task never panics");

    match outcome {
        Err(reject) => mutation_reject_frame(reject),
        // The `.auignore` is out-of-band config, never a catalog entry, so there
        // is no content hash to lag: `reflected` is the delete-style "nothing to
        // lag", and the re-scope ran synchronously in the blocking task before
        // this response, so the held version already reflects the new scope.
        Ok(commits) => {
            let response_path = crate::repo::identity_dir_for_write(&member_root).join(".auignore");
            mutation_response(handle, &response_path, None, commits)
        }
    }
}

/// One dependency member's resolve outcome, success or failure.
struct ResolveMemberSummary {
    name: String,
    sha: Option<String>,
    remote: Option<String>,
    path: Option<String>,
    error: Option<String>,
}

/// The whole resolve's outcome, computed off the runtime workers.
struct ResolveSummary {
    members: Vec<ResolveMemberSummary>,
    /// Each editable repo whose lock committed this resolve, its name to the lock
    /// commit sha. Empty when nothing resolved, no repo is a git tree, or every
    /// lock was unchanged (the idempotent no-op). One `Mutation-Id` correlates the
    /// whole set. Mirrors the mutation frame's `commits: { [repo]: sha }`.
    commits: BTreeMap<String, String>,
    /// Each repo whose lock commit failed, its name to the git error. A set entry
    /// means that repo's lock is on disk but uncommitted, soft-inconsistent,
    /// distinct from an unchanged lock (which is absent, a no-op).
    commit_errors: BTreeMap<String, String>,
    /// One message per package required at conflicting versions across the
    /// closure. A conflicted package resolves to no single version, so it is
    /// excluded from the lock and left unmounted; the engine never picks a winner.
    conflicts: Vec<String>,
}

/// The nearest ancestor directory of `path` that is a git working tree.
///
/// The lock commits into the git tree that physically holds it, not the au-repo
/// the catalog maps the manifest to: the workspace dir is the user's project repo
/// whether or not it declares a `repo.yaml`.
fn enclosing_git_root(path: &Path) -> Option<PathBuf> {
    path.ancestors()
        .skip(1)
        .find(|a| a.join(".git").exists())
        .map(Path::to_path_buf)
}

/// Run a resolve: fetch the served workspace's declared dependency closure into
/// the device cache, write each editable repo's own `repo.lock`, commit each into
/// the git tree that holds it, and rebuild so the mounted knowledge base reflects the result.
///
/// A write-path verb, not a `read`: it writes per-repo engine-schema locks and
/// rebuilds. The resolver is per-repo independent, so a
/// dependency that fails to resolve is reported in `failed` while the rest still
/// lock and mount. Each lock records only what resolved, each snapshot atomic, so
/// it never references a half-fetched package. The per-repo lock commits share ONE
/// `Mutation-Id`, so the whole solve is one auditable, correlated set. A version
/// conflict is reported in the frame's `conflicts`, not in the rebuilt knowledge base: the
/// written locks exclude conflicts, so the rebuild's locate re-detects nothing.
async fn handle_resolve(handle: &EngineHandle) -> serde_json::Value {
    // The resolve write shares the mutation serialization lock, so it never
    // interleaves with a saga or another resolve.
    let _write = handle.write_lock().lock().await;
    if handle.version().is_none() {
        return serde_json::to_value(Response::not_ready()).expect("response serializes");
    }

    let registry = crate::repo::load_user_registry(handle.config(), &tb_parser::RealFileSystem);
    // Resolve the assembled workspace's members (a `workspace.yaml` composition,
    // else the entry repo plus its `deps`), each editable member writing its own
    // `.arsumbris/repo.lock` and the entry its `workspace.lock`.
    let manifest: Option<std::path::PathBuf> =
        crate::build::assembly_roots(handle.entry(), &registry, &tb_parser::RealFileSystem).1;
    let cache_root = match handle.package_cache_root() {
        Some(r) => r.to_path_buf(),
        None => {
            return error_frame(
                "no package cache root; the home directory is unknown".to_string(),
                None,
                "resolve",
            );
        }
    };
    let registry_remote = handle.registry_remote().to_string();
    let root = handle.root().to_path_buf();

    // The fetch, lock write, commit, and rebuild are blocking; keep them off the
    // runtime workers, like a mutation write.
    let blocking_handle = handle.clone();
    let outcome = tokio::task::spawn_blocking(move || -> std::io::Result<ResolveSummary> {
        // The SAME local walk-resolve fixpoint `build` runs, so resolve sees exactly
        // the members `build` reads (a member nested under a declared member
        // included) and fetches / locks their deps. Cache-free; `resolve_and_lock`
        // fetches the closure below.
        let entry_ws_file = match &manifest {
            Some(m) => Some((m.clone(), std::fs::read(m)?)),
            None => None,
        };
        let mut ws = crate::build::local_fixpoint(
            &root,
            &entry_ws_file,
            &registry,
            &tb_parser::RealFileSystem,
        )
        .ws;
        let cache = crate::pkgcache::PackageCache::new(cache_root);
        let (resolutions, conflict_diags, written) =
            crate::pkgcache::resolve_and_lock(&mut ws, &cache, &registry_remote)?;
        let conflicts: Vec<String> = conflict_diags.iter().map(|d| d.message.clone()).collect();

        let members: Vec<ResolveMemberSummary> = resolutions
            .iter()
            .map(|r| {
                let (sha, error) = match &r.outcome {
                    Ok(p) => (Some(p.sha.clone()), None),
                    Err(e) => (None, Some(e.message.clone())),
                };
                ResolveMemberSummary {
                    name: r.name.as_str().to_string(),
                    sha,
                    remote: r.remote.clone(),
                    path: r.path.clone(),
                    error,
                }
            })
            .collect();

        // `resolve_and_lock` wrote one lock per editable repo whose closure
        // changed. Commit EACH into the git tree that holds it, correlated by ONE
        // shared `Mutation-Id`, so a partial failure is one auditable saga. Per
        // repo, three outcomes: a real commit, an idempotent no-op (the lock is
        // unchanged at HEAD), or a genuine git failure (the lock is on disk but
        // uncommitted). Then rebuild over every written lock.
        let id = crate::mutate::generate_mutation_id();
        let mut commits: BTreeMap<String, String> = BTreeMap::new();
        let mut commit_errors: BTreeMap<String, String> = BTreeMap::new();
        for (name, lock_path) in &written {
            if let Some(repo_root) = enclosing_git_root(lock_path) {
                match crate::pkgcache::commit_package_lock(&repo_root, lock_path, name.clone(), &id)
                {
                    Ok(Some(sha)) => {
                        commits.insert(name.as_str().to_string(), sha.0);
                    }
                    Ok(None) => {}
                    Err(e) => {
                        commit_errors.insert(name.as_str().to_string(), e.message);
                    }
                }
            }
        }
        if !written.is_empty() {
            blocking_handle.rebuild_paths(written.iter().map(|(_, p)| p.clone()).collect());
        }

        Ok(ResolveSummary {
            members,
            commits,
            commit_errors,
            conflicts,
        })
    })
    .await
    .expect("resolve task never panics");

    let summary = match outcome {
        Ok(s) => s,
        Err(e) => return error_frame(format!("resolve failed: {e}"), None, "resolve"),
    };

    let resolved: Vec<_> = summary
        .members
        .iter()
        .filter(|m| m.error.is_none())
        .map(|m| {
            serde_json::json!({
                "name": m.name,
                "sha": m.sha,
                "remote": m.remote,
                "path": m.path,
            })
        })
        .collect();
    let failed: Vec<_> = summary
        .members
        .iter()
        .filter(|m| m.error.is_some())
        .map(|m| {
            serde_json::json!({
                "name": m.name,
                "reason": m.error,
                "code": crate::repo::DEPENDENCY_RESOLUTION_FAILED.as_str(),
            })
        })
        .collect();

    serde_json::json!({
        "type": "resolved",
        "schema_version": SCHEMA_VERSION,
        "resolved": resolved,
        "failed": failed,
        "conflicts": summary.conflicts,
        "commits": summary.commits,
        "commit_errors": summary.commit_errors,
        "version": handle.version(),
    })
}

/// `assign_block_id`: the engine assigns a `^:` id to the addressable
/// entity enclosing the byte offset and returns `{ id, ref }`. A YAML
/// inline record gains `^: <id>` as its first key; a markdown block gains
/// a trailing ` ^<id>` marker. Assigning to a record that already carries
/// an id is idempotent — the existing id comes back, nothing is written.
/// The held parse locates the record, so a disk file that drifted from the
/// catalog rejects (the offset would be unreliable).
/// `edit_record`: patch a nested record's fields in place, then run the shared
/// nested-record saga (read, CAS, splice, on_invalid gate, write, commit).
async fn handle_edit_record(handle: &EngineHandle, args: EditRecordArgs) -> serde_json::Value {
    let target = match resolve_mutation_target(handle, &args.path) {
        Ok(t) => t,
        Err(frame) => return frame,
    };
    let field_path: Vec<tb_core::PathSegment> =
        args.field_path.into_iter().map(Into::into).collect();
    let patch = args.patch;
    let splice_target = target.clone();
    nested_record_saga(
        handle,
        target,
        format!("edit_record {}", args.path),
        args.expected_hash,
        args.on_invalid,
        args.stamps,
        move |content| {
            crate::mutate::splice_edit_record(content, &splice_target, &field_path, &patch)
        },
    )
    .await
}

/// `append_record`: add one element to a sequence field, through the shared
/// nested-record saga.
async fn handle_append_record(handle: &EngineHandle, args: AppendRecordArgs) -> serde_json::Value {
    let target = match resolve_mutation_target(handle, &args.path) {
        Ok(t) => t,
        Err(frame) => return frame,
    };
    let field_path: Vec<tb_core::PathSegment> =
        args.field_path.into_iter().map(Into::into).collect();
    let value = args.value;
    let splice_target = target.clone();
    nested_record_saga(
        handle,
        target,
        format!("append_record {}", args.path),
        args.expected_hash,
        args.on_invalid,
        args.stamps,
        move |content| {
            crate::mutate::splice_append_record(content, &splice_target, &field_path, &value)
        },
    )
    .await
}

/// The shared execution core for the nested-record mutations. Under the write
/// lock, in one blocking task: read the current bytes (the CAS input and the
/// splice input), guard `expected_hash`, `splice` to the candidate content, run
/// the `on_invalid` gate, write, commit as a single-file saga, rebuild, respond.
///
/// The splice itself is a pure transform that self-checks structural validity,
/// so a splice that would break the YAML rejects before any write, upholding the
/// never-broken invariant even under `on_invalid: advise`.
async fn nested_record_saga(
    handle: &EngineHandle,
    target: PathBuf,
    summary: String,
    expected_hash: Option<String>,
    on_invalid: OnInvalid,
    stamps: Vec<Stamp>,
    splice: impl FnOnce(&str) -> Result<String, crate::mutate::MutationReject> + Send + 'static,
) -> serde_json::Value {
    let plan = saga_plan_off_thread(handle, vec![target.clone()]).await;
    let marker_dir = saga_marker_dir(handle.root());
    let blocking_handle = handle.clone();
    let write_target = target.clone();
    let outcome = tokio::task::spawn_blocking(move || {
        let gate_handle = blocking_handle.clone();
        let write_path = write_target.clone();
        let (written, commits) =
            apply_saga(&ShellGit, &marker_dir, &plan, summary, false, move || {
                // The current on-disk bytes: the CAS reference and the splice input.
                let current = std::fs::read_to_string(&write_path).map_err(|e| {
                    crate::mutate::MutationReject::new(format!(
                        "cannot read {}: {e}",
                        write_path.display()
                    ))
                })?;
                if let Some(expected) = &expected_hash {
                    let current_hex =
                        crate::mutate::hash_hex(crate::ir::ContentHash::of(current.as_bytes()));
                    if &current_hex != expected {
                        return Err(crate::mutate::MutationReject {
                            message: format!(
                                "expected_hash mismatch: the file changed since it was read \
                                 (current {current_hex})"
                            ),
                            detail: Some(serde_json::json!({ "current_hash": current_hex })),
                        });
                    }
                }
                let candidate = splice(&current)?;
                if on_invalid == OnInvalid::Reject {
                    if let Read::Ready {
                        value: Err(reject), ..
                    } = gate_handle
                        .read(|v| reject_on_new_errors(v, &write_path, &current, &candidate))
                    {
                        return Err(reject);
                    }
                }
                let written = crate::ir::ContentHash::of(candidate.as_bytes());
                std::fs::write(&write_path, &candidate).map_err(|e| {
                    crate::mutate::MutationReject::new(format!(
                        "cannot write {}: {e}",
                        write_path.display()
                    ))
                })?;
                // Fold the stamps into the just-written file, sharing this commit.
                fold_write_stamps(&write_path, &stamps, written)
            })?;
        blocking_handle.rebuild_paths(saga_dirty_set(&plan));
        Ok::<_, crate::mutate::MutationReject>((written, commits))
    })
    .await
    .expect("mutation task never panics");
    match outcome {
        Err(reject) => mutation_reject_frame(reject),
        Ok((written, commits)) => mutation_response(handle, &target, Some(written), commits),
    }
}

/// The `on_invalid: reject` verdict: refuse when the candidate raises the file's
/// validation-ERROR count over the current content's. Both are measured by the
/// same in-memory validation, so the delta is meaningful across the byte shift.
/// A pre-existing error never blocks; only an increase does.
fn reject_on_new_errors(
    kb: &KnowledgeBase,
    path: &Path,
    current: &str,
    candidate: &str,
) -> Result<(), crate::mutate::MutationReject> {
    let before = crate::value_validate::instance_error_count(kb, path, current);
    let after_diags = crate::value_validate::validate_instance_content(kb, path, candidate);
    let after = after_diags
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .count();
    if after > before {
        let codes: Vec<&str> = after_diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .map(|d| d.code.as_str())
            .collect();
        return Err(crate::mutate::MutationReject::new(format!(
            "on_invalid=reject: the edit adds validation errors ({}); nothing written",
            codes.join(", ")
        )));
    }
    Ok(())
}

async fn assign_block_id(handle: &EngineHandle, args: AssignBlockIdArgs) -> serde_json::Value {
    let target = match resolve_mutation_target(handle, &args.path) {
        Ok(t) => t,
        Err(frame) => return frame,
    };

    // Locate the enclosing entity in the held parse, coherent at a version.
    enum Plan {
        Existing(String),
        InsertRecord { record_start: usize },
        MarkdownMarker,
    }
    let located = handle.read(|v| {
        let entry = v.catalog.get(&target)?;
        let plan = match entry.parse.as_ref() {
            FileParse::Instance {
                instance: Some(inst),
                body,
                body_offset,
                is_markdown,
                ..
            } => {
                if let Some(hit) = record_at(&inst.fields, args.at) {
                    match hit.existing {
                        Some(id) => Plan::Existing(id),
                        None => Plan::InsertRecord {
                            record_start: hit.start,
                        },
                    }
                } else if *is_markdown && args.at >= *body_offset {
                    // Idempotent like the record path: a block that already
                    // carries a marker returns its id unwritten.
                    match crate::mutate::existing_markdown_marker(body, args.at - *body_offset) {
                        Some(id) => Plan::Existing(id),
                        None => Plan::MarkdownMarker,
                    }
                } else {
                    return None;
                }
            }
            FileParse::Note {
                body, body_offset, ..
            } => match args
                .at
                .checked_sub(*body_offset)
                .and_then(|rel| crate::mutate::existing_markdown_marker(body, rel))
            {
                Some(id) => Plan::Existing(id),
                None => Plan::MarkdownMarker,
            },
            _ => return None,
        };
        Some((plan, entry.hash))
    });
    let Read::Ready { value: located, .. } = located else {
        return serde_json::to_value(Response::not_ready()).expect("response serializes");
    };
    let Some((plan, catalog_hash)) = located else {
        return mutation_reject_frame(crate::mutate::MutationReject {
            message: "no addressable entity encloses the offset — pass an offset inside an \
                      inline record or a markdown block"
                .to_string(),
            detail: None,
        });
    };

    // Idempotent: an already-addressable record returns its id unwritten — UNLESS
    // a stamp rides along, which still lands its own commit even when the block-id
    // write is a no-op. Without any stamp, no saga, no commit.
    if let Plan::Existing(id) = &plan {
        if args.stamps.is_empty() {
            let mut extra = serde_json::Map::new();
            extra.insert("id".into(), serde_json::Value::String(id.clone()));
            extra.insert(
                "ref".into(),
                serde_json::Value::String(block_ref(handle, &target, id)),
            );
            return mutation_response_extra(handle, &target, None, SagaCommits::default(), extra);
        }
    }

    let blocking_handle = handle.clone();
    let blocking_target = target.clone();
    let at = args.at;
    let stamps = args.stamps;
    let saga = saga_plan_off_thread(handle, vec![target.clone()]).await;
    let marker_dir = saga_marker_dir(handle.root());
    let summary = format!("assign_block_id {}", args.path);
    let outcome = tokio::task::spawn_blocking(move || {
        // assign_block_id adds a fresh `^:id` to ONE record in its host file. The
        // id is new, so there are no `[[host^id]]` referrers to cascade — a
        // single-file edit, not a referrer-rewriting refactor, so it keeps the
        // non-git write-without-commit fallback.
        let ((id, written), commits) =
            apply_saga(&ShellGit, &marker_dir, &saga, summary, false, || {
                let content = std::fs::read_to_string(&blocking_target).map_err(|e| {
                    crate::mutate::MutationReject {
                        message: format!("cannot read {}: {e}", blocking_target.display()),
                        detail: None,
                    }
                })?;
                // The offsets came from the held parse; a file that drifted on disk
                // makes them unreliable. Read-before-write, structurally.
                if catalog_hash != Some(crate::ir::ContentHash::of(content.as_bytes())) {
                    return Err(crate::mutate::MutationReject {
                        message: "the file changed since the engine last read it — wait for the \
                              rebuild and retry with a fresh offset"
                            .to_string(),
                        detail: None,
                    });
                }
                // Write the block-id (except in the idempotent-existing case, where
                // the id is already present and only the stamp lands).
                let write_updated = |updated: String| -> Result<
                    crate::ir::ContentHash,
                    crate::mutate::MutationReject,
                > {
                    let h = crate::ir::ContentHash::of(updated.as_bytes());
                    std::fs::write(&blocking_target, updated).map_err(|e| {
                        crate::mutate::MutationReject {
                            message: format!("cannot write {}: {e}", blocking_target.display()),
                            detail: None,
                        }
                    })?;
                    Ok(h)
                };
                let (id, written) = match plan {
                    Plan::Existing(id) => (id, crate::ir::ContentHash::of(content.as_bytes())),
                    Plan::InsertRecord { record_start } => {
                        let id = crate::mutate::generate_block_id(|c| content.contains(c));
                        let h = write_updated(crate::mutate::insert_record_id(
                            &content,
                            record_start,
                            &id,
                        )?)?;
                        (id, h)
                    }
                    Plan::MarkdownMarker => {
                        let id = crate::mutate::generate_block_id(|c| content.contains(c));
                        let h = write_updated(crate::mutate::insert_markdown_marker(
                            &content, at, &id,
                        )?)?;
                        (id, h)
                    }
                };
                // Fold the stamp into the file, sharing this commit.
                let written = fold_write_stamps(&blocking_target, &stamps, written)?;
                Ok((id, written))
            })?;
        blocking_handle.rebuild_paths(saga_dirty_set(&saga));
        Ok::<_, crate::mutate::MutationReject>((id, written, commits))
    })
    .await
    .expect("mutation task never panics");
    match outcome {
        Ok((id, written, commits)) => {
            let mut extra = serde_json::Map::new();
            extra.insert("id".into(), serde_json::Value::String(id.clone()));
            extra.insert(
                "ref".into(),
                serde_json::Value::String(block_ref(handle, &target, &id)),
            );
            mutation_response_extra(handle, &target, Some(written), commits, extra)
        }
        Err(reject) => mutation_reject_frame(reject),
    }
}

/// The innermost inline record enclosing a byte offset, with its existing
/// `^:` id if it carries one. `start` is the record's first-key offset, the
/// insertion point for a new id; `span` is the record's whole value extent, the
/// slice `promote` lifts out.
struct RecordHit {
    start: usize,
    span: tb_diagnostics::ByteRange,
    existing: Option<String>,
}

fn record_at(fields: &[tb_core::InstanceField], at: usize) -> Option<RecordHit> {
    fn walk(
        value: &tb_core::InstanceValue,
        span: tb_diagnostics::ByteRange,
        at: usize,
        best: &mut Option<RecordHit>,
    ) {
        if at < span.start || at >= span.end {
            return;
        }
        match value {
            tb_core::InstanceValue::Mapping(inline) => {
                *best = Some(RecordHit {
                    start: span.start,
                    span,
                    existing: inline.block_id.as_ref().map(|b| b.id.clone()),
                });
                for f in &inline.fields {
                    walk(&f.value, f.value_span, at, best);
                }
            }
            tb_core::InstanceValue::Sequence(elems) => {
                for e in elems {
                    walk(&e.value, e.span, at, best);
                }
            }
            _ => {}
        }
    }
    let mut best = None;
    for f in fields {
        walk(&f.value, f.value_span, at, &mut best);
    }
    best
}

/// The whole-value span of the innermost scalar value enclosing `point` — the
/// YAML value `inline` replaces with a folded record. A field value or a
/// sequence element, the value that holds the chosen `[[file]]` reference.
fn value_span_at(
    fields: &[tb_core::InstanceField],
    point: usize,
) -> Option<tb_diagnostics::ByteRange> {
    fn walk(
        value: &tb_core::InstanceValue,
        span: tb_diagnostics::ByteRange,
        point: usize,
    ) -> Option<tb_diagnostics::ByteRange> {
        if point < span.start || point >= span.end {
            return None;
        }
        match value {
            tb_core::InstanceValue::String(_) => Some(span),
            tb_core::InstanceValue::Sequence(elems) => {
                elems.iter().find_map(|e| walk(&e.value, e.span, point))
            }
            tb_core::InstanceValue::Mapping(inline) => inline
                .fields
                .iter()
                .find_map(|f| walk(&f.value, f.value_span, point)),
            _ => None,
        }
    }
    fields
        .iter()
        .find_map(|f| walk(&f.value, f.value_span, point))
}

/// Read a referrer slated for an in-place reference rewrite, rejecting if its
/// on-disk content drifted from the hash the engine held when the refactor was
/// planned. The spans being rewritten come from that held parse, so a drift
/// makes them unreliable — a displaced span could land on a different wikilink
/// and be silently mis-rewritten. clean-at-HEAD catches an uncommitted drift;
/// this catches a committed one the rebuild has not yet observed. Shared by every
/// reference-rewriting verb (rename / promote / inline / rename_block_id).
fn read_referrer_checked(
    path: &Path,
    held: Option<crate::ir::ContentHash>,
) -> Result<String, crate::mutate::MutationReject> {
    let content = std::fs::read_to_string(path).map_err(|e| {
        crate::mutate::MutationReject::new(format!("cannot read referrer {}: {e}", path.display()))
    })?;
    if held != Some(crate::ir::ContentHash::of(content.as_bytes())) {
        return Err(crate::mutate::MutationReject::new(format!(
            "referrer {} changed since the engine last read it — wait for the rebuild and retry",
            path.display()
        )));
    }
    Ok(content)
}

/// The write-time re-check for a path the saga is about to MOVE or REMOVE,
/// rejecting if it changed since the upfront clean sweep.
///
/// A referrer is guarded by [`read_referrer_checked`] as it is re-read for its
/// rewrite. A moved or removed file is not re-read, a move relocates whatever
/// bytes are on disk and a delete drops them, so a human edit landing between
/// the sweep and the move would be carried to the new path and committed under
/// the mutation's `Mutation-Id`, or silently deleted. This closes that window at
/// the moment the path is touched.
///
/// The baseline is what the path should hold now:
/// - `engine_wrote: None`, the path is untouched by this mutation so far, so it
///   must still be clean at HEAD. HEAD, not a held hash, because an asset is
///   catalogued unread and carries none.
/// - `engine_wrote: Some(bytes)`, this mutation already rewrote it (a moved file
///   with a self-reference), so it must still hold exactly those bytes.
///
/// A path under no git working tree has no clean-at-HEAD precondition at all,
/// so it passes, as the upfront sweep does.
fn guard_unchanged_before_touch(
    path: &Path,
    engine_wrote: Option<&str>,
) -> Result<(), crate::mutate::MutationReject> {
    let drifted = || {
        crate::mutate::MutationReject::new(format!(
            "{} changed since the mutation was planned — it has uncommitted changes; \
             commit or discard them, then retry",
            path.display()
        ))
    };
    if let Some(expected) = engine_wrote {
        let now = std::fs::read(path).map_err(|e| {
            crate::mutate::MutationReject::new(format!("cannot read {}: {e}", path.display()))
        })?;
        return if now == expected.as_bytes() {
            Ok(())
        } else {
            Err(drifted())
        };
    }
    let Some(tree) = path.parent().and_then(crate::gitwriter::working_tree_of) else {
        return Ok(());
    };
    let rel = path.strip_prefix(&tree).unwrap_or(path).to_path_buf();
    match ShellGit.is_clean(&tree, &[rel]) {
        Ok(true) => Ok(()),
        Ok(false) => Err(drifted()),
        Err(e) => Err(crate::mutate::MutationReject::new(format!(
            "git precondition failed: {}",
            e.message
        ))),
    }
}

/// The write-time re-check for a folder op's files, batched: every path the op
/// is about to move or remove, checked once, immediately before the move or
/// removal phase, rather than one `git status` per file.
///
/// The same baselines as [`guard_unchanged_before_touch`].
/// - a path this mutation already rewrote is byte-compared to those bytes, a
///   read, not a git call.
/// - every other path must still be clean at HEAD: one `is_clean` per working
///   tree over all of them. A path under no working tree passes.
///
/// A dirty tree is re-checked path by path, so the reject names the file that
/// drifted. The window between this check and the last file's move is the
/// phase's own duration, where one-per-file checking paid a git process per
/// file under the write lock.
fn guard_unchanged_before_touch_all(
    paths: &[(&Path, Option<&str>)],
) -> Result<(), crate::mutate::MutationReject> {
    let mut by_tree: BTreeMap<PathBuf, Vec<&Path>> = BTreeMap::new();
    let mut tree_of: BTreeMap<PathBuf, Option<PathBuf>> = BTreeMap::new();
    for (path, engine_wrote) in paths {
        if engine_wrote.is_some() {
            guard_unchanged_before_touch(path, *engine_wrote)?;
            continue;
        }
        let Some(parent) = path.parent() else {
            continue;
        };
        let tree = tree_of
            .entry(parent.to_path_buf())
            .or_insert_with(|| crate::gitwriter::working_tree_of(parent));
        if let Some(tree) = tree {
            by_tree.entry(tree.clone()).or_default().push(path);
        }
    }
    for (tree, paths) in by_tree {
        let rels: Vec<PathBuf> = paths
            .iter()
            .map(|p| p.strip_prefix(&tree).unwrap_or(p).to_path_buf())
            .collect();
        match ShellGit.is_clean(&tree, &rels) {
            Ok(true) => {}
            // Name the drifted file: the per-path check on the failure path only.
            Ok(false) => {
                for p in paths {
                    guard_unchanged_before_touch(p, None)?;
                }
            }
            Err(e) => {
                return Err(crate::mutate::MutationReject::new(format!(
                    "git precondition failed: {}",
                    e.message
                )));
            }
        }
    }
    Ok(())
}

/// What one `rename_type` referrer's bytes would become, or `None` when the
/// rewrite is byte-identical. Merges the precomputed type-name edits with the
/// wikilink edits derived now, and applies them.
///
/// A referrer whose every reference is commit-pinned yields no edits, so the
/// rewrite is byte-identical. Writing it anyway costs a disk write and wakes the
/// watcher for a file that did not change. The `rename_type` case can also reach
/// here with only TYPE-NAME edits, which do change the bytes, so this is a byte
/// comparison rather than an is-the-edit-list-empty test. Shared by the
/// committing saga and its preview, so the two compute one answer.
fn rewrite_referrer_content(
    path: &Path,
    held: Option<crate::ir::ContentHash>,
    type_edits: Option<&[crate::typerefs::TypeRefEdit]>,
    link_spans: Option<&[tb_diagnostics::ByteRange]>,
    old_rel: &Path,
    new_rel: &Path,
) -> Result<Option<String>, crate::mutate::MutationReject> {
    let content = read_referrer_checked(path, held)?;
    let mut edits: Vec<(tb_diagnostics::ByteRange, String)> = type_edits
        .unwrap_or_default()
        .iter()
        .map(|edit| (edit.span, edit.replacement.clone()))
        .collect();
    if let Some(spans) = link_spans {
        edits.extend(crate::rename::link_edits(
            &content, spans, old_rel, new_rel,
        )?);
    }
    let rewritten = crate::rename::apply_edits(&content, edits)?;
    Ok((rewritten != content).then_some(rewritten))
}

/// Rewrite one `rename_type` referrer in place, returning the bytes written, or
/// `None` when the rewrite was byte-identical and nothing was written.
fn rewrite_referrer(
    path: &Path,
    held: Option<crate::ir::ContentHash>,
    type_edits: Option<&[crate::typerefs::TypeRefEdit]>,
    link_spans: Option<&[tb_diagnostics::ByteRange]>,
    old_rel: &Path,
    new_rel: &Path,
) -> Result<Option<String>, crate::mutate::MutationReject> {
    let Some(rewritten) =
        rewrite_referrer_content(path, held, type_edits, link_spans, old_rel, new_rel)?
    else {
        return Ok(None);
    };
    std::fs::write(path, &rewritten).map_err(|e| {
        crate::mutate::MutationReject::new(format!("cannot write referrer {}: {e}", path.display()))
    })?;
    Ok(Some(rewritten))
}

/// Every block-id declared in a host file, across both addressable surfaces:
/// inline-record `^:` ids ([`tb_core::collect_record_targets`], passed in) and
/// body markers (paragraph / heading / own-line markers and a fenced block's
/// trailing id). The file-local id namespace, used to reject a block-id rename
/// whose target id already exists on either surface.
fn host_block_ids(
    parse: &FileParse,
    record_targets: &tb_core::RecordTargets,
) -> std::collections::BTreeSet<String> {
    let mut ids: std::collections::BTreeSet<String> = record_targets.keys().cloned().collect();
    if let Some((body, _)) = parse.markdown_body() {
        for ev in scan_body(body) {
            match ev {
                BodyEvent::BlockIdMarker { id, .. } => {
                    ids.insert(id.to_string());
                }
                BodyEvent::FencedBlock {
                    trailing_block_id: Some(id),
                    ..
                } => {
                    ids.insert(id.to_string());
                }
                _ => {}
            }
        }
    }
    ids
}

/// The inner wikilink text at `span` in a referrer's parse — a frontmatter
/// nav-link or a body wikilink. `None` if no wikilink occupies exactly that
/// span. Recovers the anchor the backlink index does not store, so the inline
/// referrer guard can reject a `[[file#head]]` referrer.
fn wikilink_inner_at(parse: &FileParse, span: tb_diagnostics::ByteRange) -> Option<String> {
    fn from_value(
        value: &tb_core::InstanceValue,
        nav_links: &[tb_core::NavLink],
        span: tb_diagnostics::ByteRange,
    ) -> Option<String> {
        for nl in nav_links {
            if nl.span == span {
                return Some(nl.raw.clone());
            }
        }
        match value {
            tb_core::InstanceValue::Sequence(elems) => elems
                .iter()
                .find_map(|e| from_value(&e.value, &e.nav_links, span)),
            tb_core::InstanceValue::Mapping(inline) => inline
                .fields
                .iter()
                .find_map(|f| from_value(&f.value, &f.nav_links, span)),
            _ => None,
        }
    }
    let fields = match parse {
        FileParse::Instance {
            instance: Some(inst),
            ..
        } => Some(&inst.fields),
        FileParse::Note { fields, .. } => Some(fields),
        _ => None,
    };
    if let Some(fields) = fields {
        if let Some(raw) = fields
            .iter()
            .find_map(|f| from_value(&f.value, &f.nav_links, span))
        {
            return Some(raw);
        }
    }
    if let Some((body, offset)) = parse.markdown_body() {
        for ev in scan_body(body) {
            if let BodyEvent::Wikilink { raw, span: s } = ev {
                if tb_diagnostics::ByteRange::new(s.start + offset, s.end + offset) == span {
                    return Some(raw.to_string());
                }
            }
        }
    }
    None
}

/// The inline record carrying `^: <id>`, by its whole-value span. The stable
/// locator for a record that already has a block-id, complementing `record_at`'s
/// offset locator. First occurrence wins; a `block-id-duplicate` surfaces the
/// collision separately, so at most one record carries the id in a valid file.
fn record_by_block_id(fields: &[tb_core::InstanceField], id: &str) -> Option<RecordHit> {
    fn walk(
        value: &tb_core::InstanceValue,
        span: tb_diagnostics::ByteRange,
        id: &str,
    ) -> Option<RecordHit> {
        match value {
            tb_core::InstanceValue::Mapping(inline) => {
                if inline.block_id.as_ref().is_some_and(|b| b.id == id) {
                    return Some(RecordHit {
                        start: span.start,
                        span,
                        existing: Some(id.to_string()),
                    });
                }
                inline
                    .fields
                    .iter()
                    .find_map(|f| walk(&f.value, f.value_span, id))
            }
            tb_core::InstanceValue::Sequence(elems) => {
                elems.iter().find_map(|e| walk(&e.value, e.span, id))
            }
            _ => None,
        }
    }
    fields.iter().find_map(|f| walk(&f.value, f.value_span, id))
}

/// The `[[target^id]]` ref for an assigned id: the bare stem when it
/// resolves uniquely back to this file, the repo-relative path otherwise.
fn block_ref(handle: &EngineHandle, target: &Path, id: &str) -> String {
    let stem = target
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let unique = handle
        .read(|v| {
            // Repo-local: the bare `[[stem^id]]` ref resolves within the
            // target file's own repo, so uniqueness is judged there.
            v.index_for_path(target)
                .resolve(&stem)
                .ok()
                .map(|p| p == target)
                .unwrap_or(false)
        })
        .value()
        .unwrap_or(false);
    if unique {
        return format!("[[{stem}^{id}]]");
    }
    let rel = target
        .strip_prefix(handle.root())
        .unwrap_or(target)
        .display()
        .to_string();
    format!("[[{rel}^{id}]]")
}

/// The success frame for a mutation with no primitive-specific fields, the
/// common case. See [`mutation_response_extra`].
/// The `preview_mutation` read: resolve the target path, then simulate the op
/// over an overlay and envelope its product. A path that mounts nowhere folds
/// into the result's `reject`, so a preview always answers as a read.
///
/// The simulate step runs on a blocking task: a preview does a scoped recompute
/// (or a whole-knowledge-base build for a type-def), the same shape a rebuild
/// runs, so it stays off the async reactor like a mutation's write does.
/// One trailer line a `commit_meta` record surfaces, `{ key, value }`,
/// uninterpreted. The engine's own (`Mutation-Id`, `Moved:`) and a caller
/// attribution line alike.
#[derive(Debug, Serialize)]
struct CommitTrailerDto {
    key: String,
    value: String,
}

/// One commit's metadata, the `commit_meta` read's per-commit record.
///
/// Positional against the request. An absent commit is `available: false` with
/// the metadata fields omitted, the shape of `pinned-commit-unavailable`.
#[derive(Debug, Serialize)]
struct CommitMetaDto {
    /// The resolved full oid when present, else the requested input verbatim.
    commit: String,
    available: bool,
    /// Committer date, unix seconds, when the commit entered history.
    #[serde(skip_serializing_if = "Option::is_none")]
    timestamp: Option<i64>,
    /// The author, `Name <email>`.
    #[serde(skip_serializing_if = "Option::is_none")]
    author: Option<String>,
    /// The raw commit message, summary and body.
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    /// Every trailer line, uninterpreted. Empty, never absent, so a consumer
    /// always reads an array.
    trailers: Vec<CommitTrailerDto>,
}

impl CommitMetaDto {
    /// An absent commit (unknown repo, non-git member, missing sha, or a git
    /// failure for its store), named rather than dropped.
    fn unavailable(commit: String) -> Self {
        Self {
            commit,
            available: false,
            timestamp: None,
            author: None,
            message: None,
            trailers: Vec::new(),
        }
    }

    fn from_record(r: &crate::gitwriter::CommitMetaRecord) -> Self {
        Self {
            commit: r.commit.clone(),
            available: r.available,
            timestamp: r.timestamp,
            author: r.author.clone(),
            message: r.message.clone(),
            trailers: r
                .trailers
                .iter()
                .map(|t| CommitTrailerDto {
                    key: t.key.clone(),
                    value: t.value.clone(),
                })
                .collect(),
        }
    }
}

/// The `commit_meta` read: resolve each requested commit to its member store,
/// then read metadata off git.
///
/// Two lock/reactor boundaries kept clean. The store resolution runs under the
/// state lock (in-memory, fast). The git reads run on a blocking task, off the
/// async reactor AND off the lock, since each spawns a subprocess. The result
/// is positional against the request, so an absent commit is `available: false`
/// rather than dropped.
async fn commit_meta(handle: &EngineHandle, commits: Vec<CommitRef>) -> Response {
    // A commit-ish is a single line. A newline would add extra `--batch-check`
    // probe lines and misalign the positional result, so reject it loudly rather
    // than silently return skewed records.
    if let Some(bad) = commits.iter().find(|c| c.commit.contains('\n')) {
        return Response::invalid(format!(
            "commit '{}' contains a newline; a commit-ish is a single line",
            bad.commit.escape_default()
        ));
    }
    let entry_root = handle.root().to_path_buf();
    let cache_root = handle.package_cache_root().map(Path::to_path_buf);
    // Under the lock: resolve each request to its git store (the covering
    // working tree). `repo` absent -> the entry repo; a named member -> its
    // covering tree, `None` when untracked (a .git-free snapshot) or unknown,
    // which the git pass renders `available: false`.
    let resolved = handle.read(move |v| {
        let members = wire::introspect_members(v, &entry_root, cache_root.as_deref());
        commits
            .into_iter()
            .map(|c| {
                let store = match &c.repo {
                    None => Some(entry_root.clone()),
                    Some(name) => members
                        .members
                        .iter()
                        .find(|m| &m.repo == name)
                        .and_then(|m| m.git.root.clone())
                        .map(PathBuf::from),
                };
                (c.commit, store)
            })
            .collect::<Vec<_>>()
    });
    let (version, requests) = match resolved {
        Read::NotReady => return Response::not_ready(),
        Read::Ready { version, value } => (version, value),
    };
    let dtos = tokio::task::spawn_blocking(move || commit_meta_git(requests))
        .await
        .expect("commit_meta task never panics");
    enveloped(
        "commit_meta",
        Read::Ready {
            version,
            value: dtos,
        },
    )
}

/// The git half of `commit_meta`, off the reactor. Groups the requests by store
/// so each store pays its two processes once (never one per commit), then
/// scatters the records back into request order.
fn commit_meta_git(requests: Vec<(String, Option<PathBuf>)>) -> Vec<CommitMetaDto> {
    let mut out: Vec<Option<CommitMetaDto>> = (0..requests.len()).map(|_| None).collect();
    let mut by_store: BTreeMap<PathBuf, Vec<usize>> = BTreeMap::new();
    for (i, (_commit, store)) in requests.iter().enumerate() {
        match store {
            Some(root) => by_store.entry(root.clone()).or_default().push(i),
            None => out[i] = Some(CommitMetaDto::unavailable(requests[i].0.clone())),
        }
    }
    for (root, idxs) in by_store {
        let commits: Vec<String> = idxs.iter().map(|&i| requests[i].0.clone()).collect();
        // A git failure for a store degrades to unavailable for its commits, the
        // same as an absent sha, rather than failing the whole read.
        let recs = crate::gitwriter::ShellGit
            .commit_meta(&root, &commits)
            .unwrap_or_default();
        for (k, &i) in idxs.iter().enumerate() {
            out[i] = Some(match recs.get(k) {
                Some(rec) => CommitMetaDto::from_record(rec),
                None => CommitMetaDto::unavailable(requests[i].0.clone()),
            });
        }
    }
    out.into_iter()
        .map(|o| o.expect("every request index is filled"))
        .collect()
}

/// One commit in a file's history, the `file_history` read's per-row record.
///
/// A raw pass-through: `status` and `from` are git's own similarity heuristic,
/// best-effort, NOT authoritative. `message` is the SUBJECT line; the full body
/// is a `commit_meta` join away.
#[derive(Debug, Serialize)]
struct FileHistoryDto {
    commit: String,
    timestamp: i64,
    author: String,
    message: String,
    /// `added` / `modified` / `deleted` / `renamed`. `renamed` is git's
    /// heuristic, not authoritative.
    status: &'static str,
    /// The prior path on a `renamed`, git's heuristic match; omitted otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    from: Option<String>,
}

impl FileHistoryDto {
    fn from_record(r: &crate::gitwriter::FileHistoryRecord) -> Self {
        Self {
            commit: r.commit.clone(),
            timestamp: r.timestamp,
            author: r.author.clone(),
            message: r.message.clone(),
            status: r.status,
            from: r.from.clone(),
        }
    }
}

/// The `file_history` read: resolve the file's absolute path against its member
/// root, then read its commit stream off git.
///
/// Same lock/reactor boundary as `commit_meta`: the path resolution runs under
/// the state lock (in-memory), the git read on a blocking task. Git resolves the
/// covering working tree from the file's own directory, so the routing is
/// member-correct with no tree-relative path math. An unknown `repo`, or a path
/// that is a repo-relative nowhere, yields an empty stream.
async fn file_history(handle: &EngineHandle, path: String, repo: Option<String>) -> Response {
    // `repo` names the root a RELATIVE path resolves against, so an absolute
    // path would make `repo` silently do nothing. Reject the contradiction
    // rather than reading a tree unrelated to the named member.
    if repo.is_some() && Path::new(&path).is_absolute() {
        return Response::invalid(
            "an absolute `path` cannot be combined with `repo`; `repo` names the root a relative `path` resolves against".to_string(),
        );
    }
    let entry_root = handle.root().to_path_buf();
    let cache_root = handle.package_cache_root().map(Path::to_path_buf);
    let resolved = handle.read(move |v| {
        // The base root the path is relative to: the entry, or the named
        // member's own root. An unknown member -> `None` -> empty stream.
        let base = match &repo {
            None => Some(entry_root.clone()),
            Some(name) => {
                let members = wire::introspect_members(v, &entry_root, cache_root.as_deref());
                members
                    .members
                    .iter()
                    .find(|m| &m.repo == name)
                    .map(|m| PathBuf::from(&m.root))
            }
        };
        base.map(|root| {
            let p = Path::new(&path);
            if p.is_absolute() {
                p.to_path_buf()
            } else {
                root.join(p)
            }
        })
    });
    let (version, abs) = match resolved {
        Read::NotReady => return Response::not_ready(),
        Read::Ready { version, value } => (version, value),
    };
    let rows = tokio::task::spawn_blocking(move || file_history_git(abs))
        .await
        .expect("file_history task never panics");
    enveloped(
        "file_history",
        Read::Ready {
            version,
            value: rows,
        },
    )
}

/// The git half of `file_history`, off the reactor. Runs git in the file's own
/// directory so git resolves the covering working tree.
fn file_history_git(abs: Option<PathBuf>) -> Vec<FileHistoryDto> {
    let Some(abs) = abs else {
        return Vec::new();
    };
    let (Some(dir), Some(name)) = (abs.parent(), abs.file_name()) else {
        return Vec::new();
    };
    crate::gitwriter::ShellGit
        .file_history(dir, name)
        .unwrap_or_default()
        .iter()
        .map(FileHistoryDto::from_record)
        .collect()
}

/// The `rollback_preview` read: what a revert of one commit would touch.
///
/// Reads only. The answer is a plan, not an action: it names the paths, what a
/// revert would do to each (restore the parent's bytes, or delete a path the
/// commit added), and whether the path is still safe (clean and untouched since).
/// A caller that wants the revert itself asks the mutation channel, and gets the
/// same saga guarantees as any other write.
async fn rollback_preview(handle: &EngineHandle, repo: String, commit: String) -> Response {
    if repo.is_empty() {
        return Response::invalid("`repo` must not be empty");
    }
    if commit.is_empty() {
        return Response::invalid("`commit` must not be empty");
    }
    let entry_root = handle.root().to_path_buf();
    let cache_root = handle.package_cache_root().map(Path::to_path_buf);
    let wanted = repo.clone();
    let root = handle.read(move |v| {
        let members = wire::introspect_members(v, &entry_root, cache_root.as_deref());
        members
            .members
            .iter()
            .find(|m| m.repo == wanted)
            .map(|m| PathBuf::from(&m.root))
    });
    let (version, root) = match root {
        Read::NotReady => return Response::not_ready(),
        Read::Ready { version, value } => (version, value),
    };
    let Some(root) = root else {
        return Response::invalid(format!("unknown workspace member: {repo}"));
    };
    let report = tokio::task::spawn_blocking(move || rollback_preview_git(&root, &commit))
        .await
        .expect("rollback_preview task never panics");
    match report {
        Ok(report) => enveloped(
            "rollback_preview",
            Read::Ready {
                version,
                value: report,
            },
        ),
        Err(error) => Response::invalid(error),
    }
}

/// What a revert of `commit` would touch, resolved from git alone.
fn rollback_preview_git(root: &Path, commit: &str) -> Result<RollbackPreviewView, String> {
    let git = crate::gitwriter::ShellGit;
    let Some((subject, parents)) = git
        .commit_summary(root, commit)
        .map_err(|e| format!("could not read commit {commit}: {}", e.message))?
    else {
        return Err(format!(
            "commit {commit} is not present in this member's history"
        ));
    };
    let parent = parents.first().cloned();
    let paths = git
        .changed_paths(root, commit)
        .map_err(|e| format!("could not list the paths of {commit}: {}", e.message))?;
    let mut entries = Vec::with_capacity(paths.len());
    let mut safe = true;
    for path in paths {
        // What a revert would do here: put the parent's bytes back, or delete a
        // path this commit introduced.
        let at_parent = match &parent {
            Some(parent) => git.read_at_commit(root, parent, &path).map_err(|e| {
                format!(
                    "could not read {} at {parent}: {}",
                    path.display(),
                    e.message
                )
            })?,
            None => None,
        };
        let action = if at_parent.is_some() {
            "restore"
        } else {
            "delete"
        };
        let dirty = !git
            .is_clean(root, std::slice::from_ref(&path))
            .map_err(|e| format!("could not check {}: {}", path.display(), e.message))?;
        let moved_on = git
            .changed_since(root, commit, &path)
            .map_err(|e| format!("could not check {} history: {}", path.display(), e.message))?;
        let state = if dirty {
            safe = false;
            "dirty"
        } else if moved_on {
            safe = false;
            "changed_since"
        } else {
            "clean"
        };
        entries.push(RollbackPathView {
            path: path.display().to_string(),
            action,
            state,
        });
    }
    Ok(RollbackPreviewView {
        repo: root.to_string_lossy().into_owned(),
        commit: commit.to_owned(),
        subject,
        parent,
        paths: entries,
        safe,
    })
}

/// One path a revert would touch.
#[derive(Debug, Serialize)]
struct RollbackPathView {
    path: String,
    /// What the revert would do: `restore` the parent's bytes, or `delete` a path
    /// the commit introduced.
    action: &'static str,
    /// `clean` (still the commit's own bytes, no later commit touched it),
    /// `dirty` (uncommitted changes on disk), or `changed_since` (a later commit
    /// rewrote it).
    state: &'static str,
}

/// The whole answer: a plan a caller reads before deciding.
#[derive(Debug, Serialize)]
struct RollbackPreviewView {
    repo: String,
    commit: String,
    subject: String,
    /// The parent a revert would diff against; null for a root commit.
    parent: Option<String>,
    paths: Vec<RollbackPathView>,
    /// Whether every path is `clean`, so a revert would apply without a human
    /// deciding anything.
    safe: bool,
}

/// The default `recent_commits` count cap, applied when the request gives
/// NEITHER `limit` nor `since`, so the read always bounds rather than streaming
/// an unbounded merged log.
const DEFAULT_RECENT_COMMITS_LIMIT: usize = 100;

/// The author of a `recent_commits` row, name and email split so a consumer can
/// tell a human commit from an engine one (`2brain <2brain@2brain.local>`).
#[derive(Debug, Serialize)]
struct AuthorDto {
    name: String,
    email: String,
}

/// One changed file in a `recent_commits` row, git's `--name-status -M` per file.
#[derive(Debug, Serialize)]
struct ChangedFileDto {
    path: String,
    /// `added` / `modified` / `deleted` / `renamed`. `renamed` is git's
    /// heuristic, not authoritative.
    status: &'static str,
    /// The prior path on a `renamed`, git's heuristic match; omitted otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    from: Option<String>,
}

/// One commit in the `recent_commits` stream, keyed by its working tree.
#[derive(Debug, Serialize)]
struct RecentCommitDto {
    commit: String,
    /// The owning working-tree root, the lane key.
    tree: String,
    /// The au-repo member names living in that tree.
    members: Vec<String>,
    author: AuthorDto,
    /// Committer date, unix seconds.
    timestamp: i64,
    subject: String,
    changed_files: Vec<ChangedFileDto>,
    /// Every trailer line, uninterpreted. Empty, never absent.
    trailers: Vec<CommitTrailerDto>,
}

impl RecentCommitDto {
    fn from_row(row: crate::gitwriter::RecentCommitRow) -> Self {
        let r = row.record;
        Self {
            commit: r.commit,
            tree: row.tree,
            members: row.members,
            author: AuthorDto {
                name: r.author_name,
                email: r.author_email,
            },
            timestamp: r.timestamp,
            subject: r.subject,
            changed_files: r
                .changed_files
                .into_iter()
                .map(|f| ChangedFileDto {
                    path: f.path,
                    status: f.status,
                    from: f.from,
                })
                .collect(),
            trailers: r
                .trailers
                .into_iter()
                .map(|t| CommitTrailerDto {
                    key: t.key,
                    value: t.value,
                })
                .collect(),
        }
    }
}

/// The `recent_commits` read: resolve the workspace's working trees under the
/// state lock, then read and merge their logs off git.
///
/// Same lock/reactor boundary as `commit_meta`: the member -> tree mapping runs
/// under the state lock (in-memory), the per-tree `git log` runs on a blocking
/// task, off the reactor and off the lock. `members` filters by member name;
/// each maps to its covering working tree (a monorepo's members share one),
/// deduped. `limit` and `since` bound; with neither, a default cap applies.
async fn recent_commits(
    handle: &EngineHandle,
    members: Option<Vec<String>>,
    limit: Option<usize>,
    since: Option<String>,
) -> Response {
    let entry_root = handle.root().to_path_buf();
    let cache_root = handle.package_cache_root().map(Path::to_path_buf);
    // Under the lock: map members to their distinct working trees, the same
    // `git.root` per member `commit_meta` / `file_history` route by.
    let resolved = handle.read(move |v| {
        recent_commit_tree_pairs(v, &entry_root, cache_root.as_deref(), members.as_deref())
    });
    let (version, trees) = match resolved {
        Read::NotReady => return Response::not_ready(),
        Read::Ready { version, value } => (version, value),
    };
    // A time window is a bound on its own, so a default cap applies only when
    // neither `limit` nor `since` is given; the effective cap is both the
    // per-tree `-n` and the merge cut.
    let effective_limit = limit.or_else(|| since.is_none().then_some(DEFAULT_RECENT_COMMITS_LIMIT));
    let rows =
        tokio::task::spawn_blocking(move || recent_commits_git(trees, effective_limit, since))
            .await
            .expect("recent_commits task never panics");
    enveloped(
        "recent_commits",
        Read::Ready {
            version,
            value: rows,
        },
    )
}

/// The git half of `recent_commits`, off the reactor. Runs one `git log` per
/// tree, merges newest-first, cuts to the bound, and maps to the wire rows.
fn recent_commits_git(
    trees: Vec<(String, Vec<String>)>,
    limit: Option<usize>,
    since: Option<String>,
) -> Vec<RecentCommitDto> {
    let per_tree: Vec<(
        String,
        Vec<String>,
        Vec<crate::gitwriter::RecentCommitRecord>,
    )> = trees
        .into_iter()
        .map(|(tree, members)| {
            // A git failure for one tree degrades to an empty contribution, the
            // same as a non-git member, rather than failing the whole read.
            let records = crate::gitwriter::ShellGit
                .recent_commits(Path::new(&tree), limit, since.as_deref())
                .unwrap_or_default();
            (tree, members, records)
        })
        .collect();
    crate::gitwriter::merge_recent_commits(per_tree, limit)
        .into_iter()
        .map(RecentCommitDto::from_row)
        .collect()
}

/// The workspace's working trees (each `(tree root, member names)`), the
/// member -> tree map both the `recent_commits` read and its subscription
/// resolve under the read lock. `members` filters by member name; absent = all.
fn recent_commit_tree_pairs(
    v: &KnowledgeBase,
    entry_root: &Path,
    cache_root: Option<&Path>,
    members: Option<&[String]>,
) -> Vec<(String, Vec<String>)> {
    let view = wire::introspect_members(v, entry_root, cache_root);
    let pairs: Vec<(String, Option<String>)> = view
        .members
        .iter()
        .map(|m| (m.repo.clone(), m.git.root.clone()))
        .collect();
    crate::gitwriter::group_members_by_tree(&pairs, members)
}

/// Args for the `recent_commits` subscription, the same shape as the read.
#[derive(Debug, Deserialize)]
struct RecentCommitsSubArgs {
    #[serde(default)]
    members: Option<Vec<String>>,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    since: Option<String>,
}

/// One tree's held state in a `recent_commits` subscription: its root, member
/// names, and its last-read commit records.
type TreeState = (
    String,
    Vec<String>,
    Vec<crate::gitwriter::RecentCommitRecord>,
);

/// Read each tree's log once, the subscription's seed and per-tree cache.
fn seed_trees(
    trees: Vec<(String, Vec<String>)>,
    cap: Option<usize>,
    since: Option<String>,
) -> Vec<TreeState> {
    trees
        .into_iter()
        .map(|(tree, members)| {
            let records = crate::gitwriter::ShellGit
                .recent_commits(Path::new(&tree), cap, since.as_deref())
                .unwrap_or_default();
            (tree, members, records)
        })
        .collect()
}

/// Re-read one tree's log after its reflog moved.
fn relog_tree(
    tree: String,
    cap: Option<usize>,
    since: Option<String>,
) -> Vec<crate::gitwriter::RecentCommitRecord> {
    crate::gitwriter::ShellGit
        .recent_commits(Path::new(&tree), cap, since.as_deref())
        .unwrap_or_default()
}

/// Every commit oid currently held across the per-tree caches, the set the
/// subscription treats as already accounted for.
///
/// The live diff runs against THIS union, never the bounded seed page: a commit
/// that fell below the seed's global cut is still here, so it is not
/// re-surfaced as newly-appeared on a multi-tree workspace. And because a commit
/// that ages out of every tree's `-n cap` window leaves the union (and can never
/// reappear in a future merge, which only draws from the caches), the set stays
/// bounded by the held records rather than growing per commit forever.
fn held_oids(per_tree: &[TreeState]) -> std::collections::HashSet<String> {
    per_tree
        .iter()
        .flat_map(|(_, _, recs)| recs.iter().map(|r| r.commit.clone()))
        .collect()
}

/// Merge the held per-tree states into the wire rows, newest-first, bounded.
fn merge_states_to_dtos(per_tree: &[TreeState], limit: Option<usize>) -> Vec<RecentCommitDto> {
    let cloned: Vec<_> = per_tree
        .iter()
        .map(|(t, m, r)| (t.clone(), m.clone(), r.clone()))
        .collect();
    crate::gitwriter::merge_recent_commits(cloned, limit)
        .into_iter()
        .map(RecentCommitDto::from_row)
        .collect()
}

/// Disarm a subscription's reflog watchers when its task ends, including on an
/// abort (Drop runs on the dropped future). Disarming a tree that never armed
/// (a non-git tree) is a no-op, so the guard is safe to arm before the watchers.
struct DisarmGuard {
    handle: EngineHandle,
    trees: Vec<PathBuf>,
}

impl Drop for DisarmGuard {
    fn drop(&mut self) {
        self.handle.disarm_git_watch(&self.trees);
    }
}

/// A `change_event` carrying the newly-appeared commits in its scope hint.
/// `at_version` is the held knowledge-base version at emit, a monotonic stamp,
/// NOT a git coherence cursor (the commit is off the version axis).
fn recent_commits_change_event_frame(
    subscription_id: u64,
    at_version: u64,
    commits: &[&RecentCommitDto],
) -> serde_json::Value {
    serde_json::json!({
        "type": "change_event",
        "schema_version": SCHEMA_VERSION,
        "subscription_id": subscription_id,
        "kind": "commits-appeared",
        "at_version": at_version,
        "scope_hint": { "scope": "recent_commits", "commits": commits },
    })
}

/// `recent_commits`: the seed page as the initial value, then a
/// `commits-appeared` change event carrying each newly-appeared commit. An APPEND-ONLY stream: the
/// server emits new commits, the client owns its bounded window. Rewrites are
/// out of scope (the append-only invariant), so no removals.
///
/// Liveness is the reflog watcher, NOT the version signal — a git-state
/// projection. The version signal is used ONCE, to wait for the member set to
/// become known; thereafter only reflog events wake the loop. The watchers are
/// armed before the seed (a commit in the arm-to-seed window is captured, and
/// idempotent by oid) and disarmed when the task ends.
async fn run_recent_commits(
    handle: EngineHandle,
    frame_tx: FrameTx,
    id: u64,
    args: RecentCommitsSubArgs,
) {
    let RecentCommitsSubArgs {
        members,
        limit,
        since,
    } = args;
    // Subscribe to reflog events BEFORE arming, so an event during arm/seed is
    // captured rather than lost.
    let mut git_rx = handle.subscribe_git_events();
    // Wait for readiness so the member -> tree map is known. This is the ONLY
    // use of the version signal here, an initial gate, never a re-log wake.
    let mut ver = handle.subscribe_version();
    while handle.version().is_none() {
        if ver.changed().await.is_err() {
            return;
        }
    }
    let entry_root = handle.root().to_path_buf();
    let cache_root = handle.package_cache_root().map(Path::to_path_buf);
    let members_r = members.clone();
    let trees = match handle.read(move |v| {
        recent_commit_tree_pairs(v, &entry_root, cache_root.as_deref(), members_r.as_deref())
    }) {
        Read::Ready { value, .. } => value,
        Read::NotReady => return, // raced back to Deriving; rare, give up.
    };
    let tree_roots: Vec<PathBuf> = trees.iter().map(|(t, _)| PathBuf::from(t)).collect();

    // Arm the watchers, then guard EXACTLY the subset actually armed, so the
    // teardown decrements only trees this subscription incremented — never a
    // skipped tree, which would decrement another subscription's count for it.
    // A task abort during the arm's `spawn_blocking` is a rare micro-leak (the
    // detached blocking arm finishes with no guard to disarm it); the common
    // path is correct, which the disarm-everything-requested alternative was not.
    let armed = {
        let h = handle.clone();
        let roots = tree_roots.clone();
        tokio::task::spawn_blocking(move || h.arm_git_watch(&roots))
            .await
            .unwrap_or_default()
    };
    let _guard = DisarmGuard {
        handle: handle.clone(),
        trees: armed,
    };

    let effective_limit = limit.or_else(|| since.is_none().then_some(DEFAULT_RECENT_COMMITS_LIMIT));

    // Seed: read every tree's log once, hold it, and emit the merged page.
    let mut per_tree = {
        let trees2 = trees.clone();
        let since2 = since.clone();
        tokio::task::spawn_blocking(move || seed_trees(trees2, effective_limit, since2))
            .await
            .expect("recent_commits seed task never panics")
    };
    let seed = merge_states_to_dtos(&per_tree, effective_limit);
    // The dedup set is EVERYTHING held across the trees, not just the bounded
    // seed page the client received: the live diff runs against the full
    // per-tree union, so a below-the-cut commit is already accounted for and is
    // never re-emitted as newly-appeared. See `held_oids`.
    let mut emitted = held_oids(&per_tree);
    let at = handle.version().unwrap_or(0);
    if !send_frame(
        &frame_tx,
        initial_value_frame(id, at, serde_json::to_value(&seed).expect("rows serialize")),
    )
    .await
    {
        return;
    }

    // Live loop: a reflog event re-logs the tree that moved (or all trees on a
    // lag), re-merges, and emits any commit not seen before. Additions only.
    loop {
        let moved = match git_rx.recv().await {
            Ok(tree) => Some(tree),
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => None, // fall back to all trees.
            Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
        };
        let to_relog: Vec<usize> = match &moved {
            Some(tree) => per_tree
                .iter()
                .position(|(t, _, _)| Path::new(t) == tree.as_path())
                .into_iter()
                .collect(),
            None => (0..per_tree.len()).collect(),
        };
        if to_relog.is_empty() {
            continue; // an event for a tree this subscription does not hold.
        }
        for i in to_relog {
            let tree = per_tree[i].0.clone();
            let since_i = since.clone();
            let records =
                tokio::task::spawn_blocking(move || relog_tree(tree, effective_limit, since_i))
                    .await
                    .expect("recent_commits relog task never panics");
            per_tree[i].2 = records;
        }
        // Merge unbounded for the ongoing stream (each tree is already capped),
        // so every newly-appeared commit is emitted, not just the newest page.
        let merged = merge_states_to_dtos(&per_tree, None);
        let new: Vec<&RecentCommitDto> = merged
            .iter()
            .filter(|d| !emitted.contains(&d.commit))
            .collect();
        // Rebase the dedup set onto everything now held. A commit below the
        // seed's global cut is already in `emitted` (so it is NOT re-surfaced as
        // newly-appeared, the multi-tree leak), and an oid that ages out of every
        // tree's window leaves the set (so it stays bounded).
        let held = held_oids(&per_tree);
        if new.is_empty() {
            emitted = held;
            continue;
        }
        let at = handle.version().unwrap_or(0);
        let frame = recent_commits_change_event_frame(id, at, &new);
        emitted = held;
        if !send_frame(&frame_tx, frame).await {
            return;
        }
    }
}

async fn preview_mutation(handle: &EngineHandle, args: PreviewMutationArgs) -> Response {
    let (path_arg, op, riders) = match args.into_request() {
        PreviewRequest::File { path, op, riders } => (path, op, riders),
        PreviewRequest::Rename { path, to, riders } => {
            return preview_rename(handle, path, to, riders).await;
        }
        PreviewRequest::MoveDir { path, to } => return preview_move_dir(handle, path, to).await,
        PreviewRequest::DeleteDir { path } => return preview_delete_dir(handle, path).await,
        PreviewRequest::RenameType { old_name, new_name } => {
            return preview_rename_type(handle, old_name, new_name).await;
        }
    };
    let root = handle.root().to_path_buf();
    // Resolve the path against the mounted members, with the mutation channel's
    // path rules AND the editability gate. A failure is a refusal, folded into the
    // reject channel, so preview reports the reject the real write would.
    let resolved = handle.read(move |v| resolve_editable_target(v, &root, &path_arg));
    let target = match resolved {
        Read::NotReady => return Response::not_ready(),
        Read::Ready {
            version,
            value: Err(reject),
        } => return rejected_preview(version, reject),
        Read::Ready {
            version,
            value: Ok(target),
        } => {
            // A stamp or a mixin rides an instance's frontmatter, so the write
            // refuses one on a type-def target before anything else; so does its
            // preview.
            if !riders.stamps.is_empty() || !riders.mixins.is_empty() {
                if let Some(reject) = rider_target_reject(&target) {
                    return rejected_preview(version, reject);
                }
            }
            target
        }
    };

    let h = handle.clone();
    let read = tokio::task::spawn_blocking(move || {
        // The saga plan the committing write would build, classified the same
        // way, refusing where the write's saga would.
        let saga = resolve_saga_plan(&h, std::slice::from_ref(&target));
        if let Some(reject) = saga_precondition_reject(&saga, false) {
            return h.read(move |_| preview_reject_view(reject));
        }
        let untracked = saga_untracked_files(&saga);
        let input = crate::engine::PreviewInput::File { target, op, riders };
        h.preview(input, move |outcome| {
            with_extras(
                preview_product_view(outcome),
                PreviewExtras {
                    untracked_files: untracked,
                    ..Default::default()
                },
            )
        })
    })
    .await
    .expect("preview task never panics");
    enveloped("preview_mutation", read)
}

/// Run a multi-file preview off the tokio thread, planning included: `plan`
/// runs over the held knowledge base (a folder op's subtree audit walks the
/// disk), then `build` finishes it (its disk reads, its saga classification)
/// into the change set plus the product's extra fields. Either may refuse.
async fn preview_change_set<P: Send + 'static>(
    handle: &EngineHandle,
    plan: impl FnOnce(&KnowledgeBase) -> Result<P, crate::mutate::MutationReject> + Send + 'static,
    build: impl FnOnce(
        &EngineHandle,
        P,
    ) -> Result<
        (crate::engine::ChangeSet, PreviewExtras),
        crate::mutate::MutationReject,
    > + Send
    + 'static,
) -> Response {
    let h = handle.clone();
    let read = tokio::task::spawn_blocking(move || {
        let (version, planned) = match h.read(plan) {
            Read::NotReady => return Read::NotReady,
            Read::Ready { version, value } => (version, value),
        };
        match planned.and_then(|p| build(&h, p)) {
            Err(reject) => Read::Ready {
                version,
                value: preview_reject_view(reject),
            },
            Ok((set, extras)) => h.preview(crate::engine::PreviewInput::Set(set), move |outcome| {
                with_extras(preview_product_view(outcome), extras)
            }),
        }
    })
    .await
    .expect("preview task never panics");
    enveloped("preview_mutation", read)
}

/// Preview a `rename`: its would-change referrer set, the moved file's verdict
/// at its destination, and the blast radius, without writing or committing.
///
/// Runs the committing rename's own steps up to the write, in its order, so the
/// two cannot disagree: the target resolution and editability gate,
/// [`plan_rename`], the byte-change not-written set, the consumed-member
/// reject, the require-git precondition, and each referrer's rewrite through
/// [`crate::rename::move_rewrite`]. The clean-at-HEAD precondition is a git
/// step about the write moment, not previewed, like every preview's
/// concurrency guard.
async fn preview_rename(
    handle: &EngineHandle,
    path_arg: String,
    to_arg: String,
    riders: crate::engine::PreviewRiders,
) -> Response {
    let root = handle.root().to_path_buf();
    let planned = move |v: &KnowledgeBase| {
        let from = resolve_editable_target(v, &root, &path_arg)?;
        let to = resolve_editable_target(v, &root, &to_arg)?;
        let plan = plan_rename(v, &from, &to).map_err(crate::mutate::MutationReject::new)?;
        Ok((from, to, plan))
    };
    preview_change_set(handle, planned, move |h, (from, to, plan)| {
        let not_written = move_referrers_not_written(
            &plan.referrers,
            &plan.referrer_hashes,
            &from,
            &plan.old_rel,
            &plan.new_rel,
        );
        if let Some(reject) =
            reject_if_consumed_referrers(h, &written_referrers(&plan.referrers, &not_written))
        {
            return Err(reject);
        }
        // The committing rename's saga plan: its touched set, its not-written set.
        let mut touched = vec![from.clone(), to.clone()];
        touched.extend(plan.referrers.keys().filter(|p| **p != from).cloned());
        let saga = resolve_saga_plan_writing(h, &touched, not_written.clone());
        if let Some(reject) = saga_precondition_reject(&saga, true) {
            return Err(reject);
        }
        let (set, rewrites, mixins) =
            plan_move_preview(h, &from, &to, &plan, &not_written, &riders)?;
        Ok((
            set,
            PreviewExtras {
                rewrites: Some(rewrites),
                ensure_mixins: mixins,
                untracked_files: saga_untracked_files(&saga),
                ..Default::default()
            },
        ))
    })
    .await
}

/// Preview a `move_dir`: the referrers it would rewrite, the moved files'
/// verdict under the destination, the blast radius, and what it would move
/// outside git, without writing or committing.
///
/// Shares the committing verb's steps up to the write, in its order: the
/// target resolution, [`crate::dirop::plan_move_dir`] with every folder
/// refusal, the not-written set, the consumed-member reject, the require-git
/// precondition, and [`move_dir_rewrites`]. The clean-at-HEAD precondition is
/// not previewed, as for every op.
async fn preview_move_dir(handle: &EngineHandle, path_arg: String, to_arg: String) -> Response {
    let root = handle.root().to_path_buf();
    let planned = move |v: &KnowledgeBase| {
        let from = resolve_editable_target(v, &root, &path_arg)?;
        let to = resolve_editable_target(v, &root, &to_arg)?;
        crate::dirop::plan_move_dir(v, &from, &to).map_err(crate::mutate::MutationReject::new)
    };
    preview_change_set(handle, planned, move |h, dir_plan| {
        let not_written = dir_plan.not_written();
        let written: std::collections::BTreeSet<PathBuf> = dir_plan
            .referrers
            .keys()
            .filter(|p| !not_written.contains(*p))
            .cloned()
            .collect();
        if let Some(reject) = reject_if_consumed_referrers(h, &written) {
            return Err(reject);
        }
        let saga = resolve_saga_plan_writing(h, &dir_plan.touched(), not_written.clone());
        if let Some(reject) = saga_precondition_reject(&saga, true) {
            return Err(reject);
        }
        let (rewritten, rewrites) = move_dir_rewrites(&dir_plan, &not_written)?;
        let rewritten: BTreeMap<PathBuf, String> = rewritten.into_iter().collect();
        // Each file leaves its old path and lands at its new one, carrying its
        // rewritten bytes when it is a referrer too, else read from the source
        // only when the recompute asks (an asset never is).
        let mut overrides = Vec::new();
        for (old, new) in &dir_plan.files {
            overrides.push(crate::overlay::Override::Remove { path: old.clone() });
            overrides.push(match rewritten.get(old) {
                Some(content) => crate::overlay::Override::Replace {
                    path: new.clone(),
                    content: content.clone().into_bytes(),
                },
                None => crate::overlay::Override::Moved {
                    path: new.clone(),
                    from: old.clone(),
                },
            });
        }
        for (path, content) in &rewritten {
            if !dir_plan.moves(path) {
                overrides.push(crate::overlay::Override::Replace {
                    path: path.clone(),
                    content: content.clone().into_bytes(),
                });
            }
        }
        // A referrer the move leaves byte-identical still recomputes, its edge
        // flips to the new path, the committing verb's rebuild set.
        let also_dirty = dir_plan
            .referrers
            .keys()
            .filter(|p| !rewritten.contains_key(*p) && !dir_plan.moves(p))
            .cloned()
            .collect();
        let extras = PreviewExtras {
            rewrites: Some(rewrites),
            untracked_files: saga_untracked_files(&saga),
            untracked_dirs: Some(dir_plan.untracked_dirs(&saga.outside_git)),
            ..Default::default()
        };
        Ok((
            crate::engine::ChangeSet {
                target: dir_plan.to.clone(),
                overrides,
                also_dirty,
                gone: vec![dir_plan.subtree.dir.clone()],
            },
            extras,
        ))
    })
    .await
}

/// Preview a `rename_type`: every path the cascade rewrites, with the member
/// owning it and what changes there, the moved def's verdict at its new path,
/// and the blast radius, without writing or committing.
///
/// Shares the committing mutation's steps up to the write, in its order: the
/// same [`plan_rename_type`] gather, the same written/exempt split, the
/// consumed-member reject, the require-git precondition, and each referrer's
/// rewrite through [`rewrite_referrer_content`]. The clean-at-HEAD precondition
/// is a git step about the write moment, so it is not previewed, like every op.
async fn preview_rename_type(
    handle: &EngineHandle,
    old_name_arg: String,
    new_name_arg: String,
) -> Response {
    let planned = move |v: &KnowledgeBase| {
        plan_rename_type(v, &old_name_arg, &new_name_arg)
            .map_err(crate::mutate::MutationReject::new)
    };
    preview_change_set(handle, planned, move |h, gather| {
        let writes = gather.written_and_exempt();
        // A referrer this cascade rewrites that lives in a consumed member
        // rejects the whole preview, as it rejects the mutation.
        if let Some(reject) = reject_if_consumed_referrers(h, &writes.written) {
            return Err(reject);
        }
        let saga = resolve_saga_plan_writing(h, &gather.touched(), writes.exempt.clone());
        if let Some(reject) = saga_precondition_reject(&saga, true) {
            return Err(reject);
        }
        let (set, affected) = rename_type_change_set(&gather, &writes)?;
        Ok((
            set,
            PreviewExtras {
                affected: Some(affected),
                untracked_files: saga_untracked_files(&saga),
                ..Default::default()
            },
        ))
    })
    .await
}

/// The would-be change set of a `rename_type`, the committing saga's writes
/// without the disk write: the def moved (carrying its own rewritten bytes when
/// it references itself), each written referrer at its new bytes, and the
/// referrers left byte-identical still recomputed. Alongside it the wire's
/// affected list: every path the cascade rewrites, its member, and what changes.
fn rename_type_change_set(
    gather: &RenameTypeGather,
    writes: &RenameTypeWrites,
) -> Result<(crate::engine::ChangeSet, Vec<wire::AffectedPathView>), crate::mutate::MutationReject>
{
    use crate::overlay::Override;
    let mut overrides = Vec::new();
    let mut also_dirty = Vec::new();
    let mut self_rewrite: Option<String> = None;
    for path in &gather.referrers {
        if !writes.written.contains(path) {
            also_dirty.push(path.clone());
            continue;
        }
        let rewritten = rewrite_referrer_content(
            path,
            gather.referrer_hashes.get(path).copied().flatten(),
            gather.type_edits.get(path).map(Vec::as_slice),
            gather.link_spans.get(path).map(Vec::as_slice),
            &gather.old_rel,
            &gather.new_rel,
        )?;
        match rewritten {
            Some(content) if *path == gather.from => self_rewrite = Some(content),
            Some(content) => overrides.push(Override::Replace {
                path: path.clone(),
                content: content.into_bytes(),
            }),
            // A "written" referrer whose bytes did not change: the committing
            // saga writes nothing either, so it only recomputes.
            None => also_dirty.push(path.clone()),
        }
    }
    // The committing saga's rename_file refuses a taken destination before it
    // touches anything; so does the preview.
    crate::mutate::rename_precheck(&gather.from, &gather.to)?;
    overrides.push(Override::Remove {
        path: gather.from.clone(),
    });
    overrides.push(match self_rewrite {
        Some(text) => Override::Replace {
            path: gather.to.clone(),
            content: text.into_bytes(),
        },
        None => Override::Moved {
            path: gather.to.clone(),
            from: gather.from.clone(),
        },
    });
    let view = |path: &Path| wire::AffectedPathView {
        path: path.display().to_string(),
        repo: gather
            .repos
            .get(path)
            .map(|r| r.as_str().to_string())
            .unwrap_or_default(),
        changes: gather.changes_at(path, writes),
    };
    // The def always moves; every other entry is a referrer the cascade writes.
    let mut affected = vec![view(&gather.from)];
    affected.extend(
        writes
            .written
            .iter()
            .filter(|p| **p != gather.from)
            .map(|p| view(p)),
    );
    affected.sort_by(|a, b| a.path.cmp(&b.path));
    Ok((
        crate::engine::ChangeSet {
            target: gather.to.clone(),
            overrides,
            also_dirty,
            gone: vec![gather.from.clone()],
        },
        affected,
    ))
}

/// Preview a `delete_dir`: the references it would strand, the blast radius,
/// and what it would remove outside git, without writing or committing.
///
/// Shares the committing verb's plan, [`crate::dirop::plan_delete_dir`], with
/// every folder refusal and the stranded report, and its require-git
/// precondition over the same saga plan.
async fn preview_delete_dir(handle: &EngineHandle, path_arg: String) -> Response {
    let root = handle.root().to_path_buf();
    let planned = move |v: &KnowledgeBase| {
        let dir = resolve_editable_target(v, &root, &path_arg)?;
        crate::dirop::plan_delete_dir(v, &dir).map_err(crate::mutate::MutationReject::new)
    };
    preview_change_set(handle, planned, move |h, dir_plan| {
        let saga = resolve_saga_plan(h, &dir_plan.subtree.files);
        if let Some(reject) = saga_precondition_reject(&saga, true) {
            return Err(reject);
        }
        let overrides = dir_plan
            .subtree
            .files
            .iter()
            .map(|f| crate::overlay::Override::Remove { path: f.clone() })
            .collect();
        let extras = PreviewExtras {
            untracked_files: saga_untracked_files(&saga),
            untracked_dirs: Some(dir_plan.untracked_dirs(&saga.outside_git)),
            stranded: Some(dir_plan.stranded),
            ..Default::default()
        };
        Ok((
            crate::engine::ChangeSet {
                target: dir_plan.subtree.dir.clone(),
                overrides,
                also_dirty: Vec::new(),
                gone: vec![dir_plan.subtree.dir],
            },
            extras,
        ))
    })
    .await
}

/// A preview refused before any product, in the read envelope.
fn rejected_preview(version: u64, reject: crate::mutate::MutationReject) -> Response {
    enveloped(
        "preview_mutation",
        Read::Ready {
            version,
            value: preview_reject_view(reject),
        },
    )
}

/// What a preview's product carries beyond its target and blast radius, each
/// the value the committing op's result reports.
#[derive(Default)]
struct PreviewExtras {
    /// A rename's or a folder move's would-change referrers.
    rewrites: Option<Vec<wire::PreviewRewriteView>>,
    /// A folder delete's stranded references.
    stranded: Option<Vec<wire::StrandedView>>,
    /// The paths the op would write outside git, every op.
    untracked_files: Vec<PathBuf>,
    /// A folder op's directories no commit records.
    untracked_dirs: Option<Vec<PathBuf>>,
    /// A rename's `ensure_mixins` outcomes, folded into its change set by the
    /// caller. A single-file op's ride the outcome instead.
    ensure_mixins: Option<Vec<wire::MixinOutcome>>,
    /// A `rename_type`'s affected paths: every file the cascade rewrites, its
    /// member, and what changes there.
    affected: Option<Vec<wire::AffectedPathView>>,
}

/// Attach a preview's extra fields to its product; a reject passes.
fn with_extras(view: wire::PreviewProductView, extras: PreviewExtras) -> wire::PreviewProductView {
    let strings = |ps: &[PathBuf]| ps.iter().map(|p| p.display().to_string()).collect();
    match view {
        wire::PreviewProductView::Product {
            target,
            blast_radius,
            ensure_mixins,
            ..
        } => wire::PreviewProductView::Product {
            target,
            blast_radius,
            rewrites: extras.rewrites,
            stranded: extras.stranded,
            untracked_files: strings(&extras.untracked_files),
            untracked_dirs: extras.untracked_dirs.as_deref().map(strings),
            ensure_mixins: extras.ensure_mixins.or(ensure_mixins),
            affected: extras.affected,
        },
        reject => reject,
    }
}

/// Compute a move's change set, the committing rename's steps up to the write,
/// in its order, minus the disk write and the git steps.
///
/// Returns the engine-side [`crate::engine::ChangeSet`] and the wire's
/// would-change list: only referrers whose bytes change, each with the links it
/// re-spells, the moved file's own self-links reported at its destination. The
/// moved file carries its rewritten bytes, or its riders folded in, else it is
/// read from the source only when the recompute asks, so a moved asset is
/// never read. The mixin outcomes are `None` when the rename carried no mixin.
#[allow(clippy::type_complexity)]
fn plan_move_preview(
    h: &EngineHandle,
    from: &Path,
    to: &Path,
    plan: &RenamePlan,
    not_written: &std::collections::BTreeSet<PathBuf>,
    riders: &crate::engine::PreviewRiders,
) -> Result<
    (
        crate::engine::ChangeSet,
        Vec<wire::PreviewRewriteView>,
        Option<Vec<wire::MixinOutcome>>,
    ),
    crate::mutate::MutationReject,
> {
    use crate::overlay::Override;
    let mut overrides = vec![Override::Remove {
        path: from.to_path_buf(),
    }];
    let mut also_dirty = Vec::new();
    let mut views = Vec::new();
    let mut self_rewrite = None;
    for (path, spans) in &plan.referrers {
        let content =
            read_referrer_checked(path, plan.referrer_hashes.get(path).copied().flatten())?;
        let Some(rewritten) =
            crate::rename::move_rewrite(&content, spans, &plan.old_rel, &plan.new_rel)?
        else {
            if path.as_path() != from {
                also_dirty.push(path.clone());
            }
            continue;
        };
        if not_written.contains(path) {
            return Err(crate::mutate::MutationReject::new(format!(
                "referrer {} changed since the rename was planned — retry",
                path.display()
            )));
        }
        let links =
            crate::rename::move_link_changes(&content, spans, &plan.old_rel, &plan.new_rel)?
                .into_iter()
                .map(|(from, to)| wire::PreviewLinkChangeView { from, to })
                .collect();
        let shown = if path.as_path() == from { to } else { path };
        views.push(wire::PreviewRewriteView {
            path: shown.display().to_string(),
            links,
        });
        if path.as_path() == from {
            self_rewrite = Some(rewritten);
        } else {
            overrides.push(Override::Replace {
                path: path.clone(),
                content: rewritten.into_bytes(),
            });
        }
    }
    crate::mutate::rename_precheck(from, to)?;
    // The riders fold into the moved file, as the committing rename folds them
    // into the renamed file after the move, the mixins decided at the destination
    // against the held knowledge base.
    let mut mixins = None;
    let moved = if riders.stamps.is_empty() && riders.mixins.is_empty() {
        match self_rewrite {
            Some(text) => Override::Replace {
                path: to.to_path_buf(),
                content: text.into_bytes(),
            },
            None => Override::Moved {
                path: to.to_path_buf(),
                from: from.to_path_buf(),
            },
        }
    } else {
        let text = match self_rewrite {
            Some(text) => text,
            None => std::fs::read(from)
                .ok()
                .and_then(|b| String::from_utf8(b).ok())
                .ok_or_else(|| crate::mutate::MutationReject::new("the file is not valid UTF-8"))?,
        };
        let (content, outcomes) = match h.read(|kb| riders.fold(kb, to, text)) {
            Read::Ready { value, .. } => value?,
            _ => {
                return Err(crate::mutate::MutationReject::new(
                    "engine is not ready to resolve an ensure_mixin",
                ));
            }
        };
        mixins = outcomes;
        Override::Replace {
            path: to.to_path_buf(),
            content: content.into_bytes(),
        }
    };
    overrides.push(moved);
    views.sort_by(|a, b| a.path.cmp(&b.path));
    Ok((
        crate::engine::ChangeSet {
            target: to.to_path_buf(),
            overrides,
            also_dirty,
            gone: vec![from.to_path_buf()],
        },
        views,
        mixins,
    ))
}

/// Assemble a preview's wire product from its outcome: the built product, or a
/// structural reject folded into the `reject` channel.
fn preview_product_view(outcome: crate::engine::PreviewOutcome) -> wire::PreviewProductView {
    match outcome {
        crate::engine::PreviewOutcome::Built {
            preview,
            held,
            target,
            gone,
            mixins,
        } => match wire::preview_product_view(preview, held, target, gone) {
            wire::PreviewProductView::Product {
                target,
                blast_radius,
                rewrites,
                stranded,
                untracked_files,
                untracked_dirs,
                ..
            } => wire::PreviewProductView::Product {
                target,
                blast_radius,
                rewrites,
                stranded,
                untracked_files,
                untracked_dirs,
                ensure_mixins: mixins,
                // Set by the caller's extras for a `rename_type` preview.
                affected: None,
            },
            reject => reject,
        },
        crate::engine::PreviewOutcome::Reject(reject) => preview_reject_view(reject),
    }
}

/// A `MutationReject` as a preview product's `reject` field.
fn preview_reject_view(reject: crate::mutate::MutationReject) -> wire::PreviewProductView {
    wire::PreviewProductView::Reject {
        reject: wire::PreviewRejectView {
            message: reject.message,
            detail: reject.detail,
        },
    }
}

fn mutation_response(
    handle: &EngineHandle,
    target: &Path,
    written_hash: Option<crate::ir::ContentHash>,
    commits: SagaCommits,
) -> serde_json::Value {
    mutation_response_extra(
        handle,
        target,
        written_hash,
        commits,
        serde_json::Map::new(),
    )
}

/// `move_dir`: move a folder as one saga, per
/// [[spec - directory mutation verbs - move_dir and delete_dir fold a subtree into one per-file saga]].
///
/// Two phases in a fixed order: every referrer is rewritten at its OLD path,
/// then every file moves, a moved referrer carrying its new bytes. Interleaving
/// per file would find a sibling referrer already moved. Then the directory
/// skeleton follows: each mapped directory is created, empty ones included, and
/// each emptied source directory removed. The saga's directory journal undoes
/// the skeleton on any failure.
async fn move_dir(handle: &EngineHandle, args: MoveDirArgs) -> serde_json::Value {
    let from = match resolve_mutation_target(handle, &args.path) {
        Ok(t) => t,
        Err(frame) => return frame,
    };
    let to = match resolve_mutation_target(handle, &args.to) {
        Ok(t) => t,
        Err(frame) => return frame,
    };
    let marker_dir = saga_marker_dir(handle.root());
    let summary = format!("move_dir {} -> {}", args.path, args.to);
    let blocking_handle = handle.clone();
    let blocking_to = to.clone();
    // Planned off the tokio thread: the subtree audit walks the folder, the
    // not-written set reads every referrer, the saga plan spawns git.
    let outcome = tokio::task::spawn_blocking(move || {
        let handle = blocking_handle;
        let dir_plan = match handle.read(|v| crate::dirop::plan_move_dir(v, &from, &blocking_to)) {
            crate::Read::Ready { value: Ok(p), .. } => p,
            crate::Read::Ready {
                value: Err(msg), ..
            } => {
                return Err(mutation_reject_frame(crate::mutate::MutationReject::new(
                    msg,
                )));
            }
            crate::Read::NotReady => {
                return Err(
                    serde_json::to_value(Response::not_ready()).expect("response serializes")
                );
            }
        };
        let not_written = dir_plan.not_written();
        let written: std::collections::BTreeSet<PathBuf> = dir_plan
            .referrers
            .keys()
            .filter(|p| !not_written.contains(*p))
            .cloned()
            .collect();
        if let Some(reject) = reject_if_consumed_referrers(&handle, &written) {
            return Err(mutation_reject_frame(reject));
        }
        let mut plan = resolve_saga_plan_writing(&handle, &dir_plan.touched(), not_written.clone());
        plan.dirs = DirJournal {
            created: dir_plan.created_dirs(),
            kept: dir_plan.kept_dirs(),
        };
        let untracked_dirs = dir_plan.untracked_dirs(&plan.outside_git);
        let mut moves = MovesByTree::new();
        for (old_rel, new_rel) in dir_plan.moved_rels() {
            for (tree, records) in moves_for(&dir_plan.subtree.repo_root, old_rel, new_rel) {
                moves.entry(tree).or_default().extend(records);
            }
        }
        let (rewrites, commits) =
            apply_saga_with_moves(&ShellGit, &marker_dir, &plan, summary, &moves, true, || {
                move_dir_write(&dir_plan, &not_written)
            })
            .map_err(mutation_reject_frame)?;
        handle.rebuild_paths(saga_dirty_set(&plan));
        Ok((rewrites, commits, untracked_dirs))
    })
    .await
    .expect("mutation task never panics");
    match outcome {
        Err(frame) => frame,
        Ok((rewrites, commits, untracked_dirs)) => {
            let mut extra = serde_json::Map::new();
            extra.insert(
                "rewrites".into(),
                serde_json::to_value(rewrites).expect("rewrites serialize"),
            );
            extra.insert(
                "untracked_dirs".into(),
                serde_json::Value::Array(
                    untracked_dirs
                        .iter()
                        .map(|p| serde_json::Value::String(p.display().to_string()))
                        .collect(),
                ),
            );
            mutation_response_extra(handle, &to, None, commits, extra)
        }
    }
}

/// `delete_dir`: delete a folder as one saga, per
/// [[spec - directory mutation verbs - move_dir and delete_dir fold a subtree into one per-file saga]].
///
/// Raw like `delete_file`: every catalogued file under the folder is removed,
/// no referrer is rewritten, and the references the delete strands are
/// reported. The commit stages the enumerated file list, never the directory,
/// so a file created in the folder after the plan is never touched and its
/// directory stays. Every emptied directory is then removed; the saga's
/// directory journal puts them back on any failure.
///
/// Needs a git working tree, unlike `delete_file`: one file deletes atomically,
/// but a folder removed file by file off-git could not be rolled back from a
/// failure part way.
async fn delete_dir(handle: &EngineHandle, args: DeleteDirArgs) -> serde_json::Value {
    let dir = match resolve_mutation_target(handle, &args.path) {
        Ok(t) => t,
        Err(frame) => return frame,
    };
    let marker_dir = saga_marker_dir(handle.root());
    let summary = format!("delete_dir {}", args.path);
    let blocking_handle = handle.clone();
    let blocking_dir = dir.clone();
    // Planned off the tokio thread: the subtree audit walks the folder, the
    // saga plan spawns git.
    let outcome = tokio::task::spawn_blocking(move || {
        let handle = blocking_handle;
        let dir_plan = match handle.read(|v| crate::dirop::plan_delete_dir(v, &blocking_dir)) {
            crate::Read::Ready { value: Ok(p), .. } => p,
            crate::Read::Ready {
                value: Err(msg), ..
            } => {
                return Err(mutation_reject_frame(crate::mutate::MutationReject::new(
                    msg,
                )));
            }
            crate::Read::NotReady => {
                return Err(
                    serde_json::to_value(Response::not_ready()).expect("response serializes")
                );
            }
        };
        let mut plan = resolve_saga_plan(&handle, &dir_plan.subtree.files);
        plan.dirs = DirJournal {
            created: Vec::new(),
            kept: dir_plan.kept_dirs(),
        };
        let untracked_dirs = dir_plan.untracked_dirs(&plan.outside_git);
        let files = &dir_plan.subtree.files;
        let dirs = &dir_plan.subtree.dirs;
        // The last-live commit, HEAD before the delete: the deletion commit's
        // parent, where every contained file still existed. Clean-at-HEAD
        // guarantees each was there, and the serialized write lock keeps this
        // HEAD the parent. See `delete_file`.
        let last_live = crate::gitwriter::head_commit_fast(&blocking_dir);
        let ((), commits) = apply_saga(&ShellGit, &marker_dir, &plan, summary, true, || {
            delete_dir_write(files, dirs)
        })
        .map_err(mutation_reject_frame)?;
        handle.rebuild_paths(saga_dirty_set(&plan));
        Ok((commits, last_live, dir_plan.stranded, untracked_dirs))
    })
    .await
    .expect("mutation task never panics");
    match outcome {
        Err(frame) => frame,
        Ok((commits, last_live, stranded, untracked_dirs)) => {
            let mut extra = serde_json::Map::new();
            // Only a delete that committed has a last-live commit to pin.
            if !commits.is_empty() {
                if let Some(sha) = last_live {
                    extra.insert("last_live_commit".into(), serde_json::Value::String(sha));
                }
            }
            extra.insert(
                "stranded".into(),
                serde_json::to_value(&stranded).expect("stranded serializes"),
            );
            extra.insert(
                "untracked_dirs".into(),
                serde_json::Value::Array(
                    untracked_dirs
                        .iter()
                        .map(|p| serde_json::Value::String(p.display().to_string()))
                        .collect(),
                ),
            );
            mutation_response_extra(handle, &dir, None, commits, extra)
        }
    }
}

/// The write half of [`delete_dir`], run inside its saga: remove exactly the
/// enumerated `files`, re-checked unchanged once before the removals, then every
/// directory of `dirs` left empty, children first. A directory still holding a
/// file created after the plan stays, with that file.
fn delete_dir_write(
    files: &[PathBuf],
    dirs: &[PathBuf],
) -> Result<(), crate::mutate::MutationReject> {
    let guarded: Vec<(&Path, Option<&str>)> = files.iter().map(|f| (f.as_path(), None)).collect();
    guard_unchanged_before_touch_all(&guarded)?;
    for f in files {
        std::fs::remove_file(f).map_err(|e| {
            crate::mutate::MutationReject::new(format!("cannot delete {}: {e}", f.display()))
        })?;
    }
    for d in dirs.iter().rev() {
        let _ = std::fs::remove_dir(d);
    }
    Ok(())
}

/// A folder move's referrer rewrites: each changed referrer's new bytes, and the
/// wire list, each shown at the path it ends at.
type DirRewrites = (Vec<(PathBuf, String)>, Vec<wire::PreviewRewriteView>);

/// A folder move's referrer rewrites, computed at the referrers' OLD paths
/// without writing: each changed referrer's new bytes, and the wire list, each
/// shown at the path it ends at, path-sorted.
///
/// Shared by [`move_dir_write`] and the folder-move preview, so the previewed
/// `rewrites` are exactly the ones the verb writes. Refuses when a referrer the
/// plan left out of the clean guard and the commit turns out to change: writing
/// it would land an uncommitted change.
fn move_dir_rewrites(
    plan: &crate::dirop::MoveDirPlan,
    not_written: &std::collections::BTreeSet<PathBuf>,
) -> Result<DirRewrites, crate::mutate::MutationReject> {
    let mut rewritten = Vec::new();
    let mut views = Vec::new();
    for path in plan.referrers.keys() {
        let content =
            read_referrer_checked(path, plan.referrer_hashes.get(path).copied().flatten())?;
        let Some(new) = plan.rewrite(path, &content)? else {
            continue;
        };
        if not_written.contains(path) {
            return Err(crate::mutate::MutationReject::new(format!(
                "referrer {} changed since the move was planned — retry",
                path.display()
            )));
        }
        let links = plan
            .link_changes(path, &content)?
            .into_iter()
            .map(|(from, to)| wire::PreviewLinkChangeView { from, to })
            .collect();
        let shown = plan
            .files
            .iter()
            .find(|(old, _)| old == path)
            .map_or(path, |(_, new)| new);
        views.push(wire::PreviewRewriteView {
            path: shown.display().to_string(),
            links,
        });
        rewritten.push((path.clone(), new));
    }
    views.sort_by(|a, b| a.path.cmp(&b.path));
    Ok((rewritten, views))
}

/// The write half of [`move_dir`], run inside its saga. Returns the referrers
/// whose bytes changed, each shown at the path it ends at, path-sorted.
fn move_dir_write(
    plan: &crate::dirop::MoveDirPlan,
    not_written: &std::collections::BTreeSet<PathBuf>,
) -> Result<Vec<wire::PreviewRewriteView>, crate::mutate::MutationReject> {
    use crate::mutate::MutationReject;
    // Phase one: every referrer rewritten at its old path. A moved file's own
    // rewrite is the baseline its pre-move re-check compares against.
    let (rewritten, views) = move_dir_rewrites(plan, not_written)?;
    let mut self_written: BTreeMap<PathBuf, String> = BTreeMap::new();
    for (path, new) in rewritten {
        std::fs::write(&path, &new).map_err(|e| {
            MutationReject::new(format!("cannot write referrer {}: {e}", path.display()))
        })?;
        if plan.moves(&path) {
            self_written.insert(path, new);
        }
    }
    // Phase two: every file moves, re-checked unchanged once, immediately
    // before the phase.
    let guarded: Vec<(&Path, Option<&str>)> = plan
        .files
        .iter()
        .map(|(old, _)| (old.as_path(), self_written.get(old).map(String::as_str)))
        .collect();
    guard_unchanged_before_touch_all(&guarded)?;
    for (old, new) in &plan.files {
        if new.symlink_metadata().is_ok() {
            return Err(MutationReject::new(format!(
                "destination appeared since the move was planned: {}",
                new.display()
            )));
        }
        if let Some(parent) = new.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                MutationReject::new(format!("cannot create {}: {e}", parent.display()))
            })?;
        }
        std::fs::rename(old, new).map_err(|e| {
            MutationReject::new(format!(
                "cannot move {} to {}: {e}",
                old.display(),
                new.display()
            ))
        })?;
    }
    // The skeleton: every mapped directory created, empty ones included, then
    // each emptied source removed, children first. A source still holding a
    // file created after the plan stays, with that file.
    for (_, new) in &plan.dirs {
        std::fs::create_dir_all(new)
            .map_err(|e| MutationReject::new(format!("cannot create {}: {e}", new.display())))?;
    }
    for (old, _) in plan.dirs.iter().rev() {
        let _ = std::fs::remove_dir(old);
    }
    Ok(views)
}

/// The success frame for a mutation: the read-response envelope, `version`
/// the post-mutation knowledge base version (the consumer's echo-suppression
/// correlation), the result carrying the file's identity, fresh diagnostics,
/// the saga's commits, and any primitive-specific `extra` fields.
///
/// Two commit fields, a clean anchor / provenance split ([[spec - git write
/// path - commit-per-mutation as a local saga over the workspace's materialized
/// repos]]):
/// - `commit`: HEAD of `target`'s own repo, the pin anchor for the file the
///   response is about. `null` only off-git. After a committing mutation it is
///   the new sha; after a no-op it is the unchanged HEAD. The read/mutate-
///   symmetric field, since the content read's `commit` is the same anchor (a
///   read never commits, so its `commit` is purely HEAD).
/// - `commits`: what THIS mutation committed, every repo keyed by name. `{}`
///   when nothing committed (off-git, or a no-op). Mutate-only provenance, a
///   read has none. `target`'s own repo is always among the committers when the
///   saga commits anything (the mutation's primary effect is on `target`), so a
///   non-empty `commits` means `commit` is the sha that just changed `target`.
///   Not first-of-N: `commit` follows `target`'s repo, never a different repo's
///   sha.
fn mutation_response_extra(
    handle: &EngineHandle,
    target: &Path,
    written_hash: Option<crate::ir::ContentHash>,
    commits: SagaCommits,
    extra: serde_json::Map<String, serde_json::Value>,
) -> serde_json::Value {
    // A folder op's target is a directory, whose diagnostics are its files'.
    let scope = if target.is_dir() {
        DiagnosticsScope {
            path_prefix: Some(target.to_path_buf()),
            ..Default::default()
        }
    } else {
        DiagnosticsScope {
            path: Some(target.to_path_buf()),
            ..Default::default()
        }
    };
    let read = handle.read(|v| {
        let diagnostics: Vec<Diagnostic> = v
            .diagnostics()
            .filter(|d| scope.matches(v, d))
            .cloned()
            .collect();
        let held_hash = v.catalog.get(target).and_then(|e| e.hash);
        // The response file's own repo: its name keys the commits map, its root
        // is where a no-op falls back to reading live HEAD for the anchor.
        let own_repo = v
            .repos
            .repo_of(target)
            .map(|r| (r.name.clone(), r.root.clone()));
        (diagnostics, held_hash, own_repo)
    });
    let Read::Ready {
        version,
        value: (diagnostics, held_hash, own_repo),
    } = read
    else {
        return serde_json::to_value(Response::not_ready()).expect("response serializes");
    };
    // Does the held knowledge base reflect the write that produced this response? The
    // disk write is canonical; `rebuild()` is best-effort and can give up on
    // churn or a failing build, leaving the held catalog hash behind what was
    // written. `reflected: false` tells the consumer these diagnostics and
    // this hash predate its write, so it should await a later version rather
    // than trust them (or re-issue the mutation, which would conflict). With
    // no write to compare (an idempotent no-op), there is nothing to lag.
    let reflected = match written_hash {
        Some(written) => held_hash == Some(written),
        None => true,
    };
    // Project the saga's commits two ways. `commits` is the provenance: what
    // this mutation committed, per repo. `commit` is the anchor: HEAD of the
    // response file's own repo. When that repo committed, its new sha is already
    // in the map (no git call); on a no-op the map lacks it, so read live HEAD,
    // which is null only off-git.
    let commits_obj: serde_json::Map<String, serde_json::Value> = commits
        .iter()
        .map(|(repo, sha)| {
            (
                repo.as_str().to_string(),
                serde_json::Value::String(sha.0.clone()),
            )
        })
        .collect();
    let commit = match own_repo {
        Some((own_name, own_root)) => commits
            .iter()
            .find(|(repo, _)| *repo == own_name)
            .map(|(_, sha)| sha.0.clone())
            .or_else(|| crate::gitwriter::head_commit_fast(&own_root))
            .map(serde_json::Value::String)
            .unwrap_or(serde_json::Value::Null),
        None => serde_json::Value::Null,
    };
    let mut result = serde_json::Map::new();
    result.insert(
        "path".into(),
        serde_json::Value::String(target.display().to_string()),
    );
    result.insert(
        "hash".into(),
        serde_json::to_value(held_hash.map(crate::mutate::hash_hex)).expect("hash"),
    );
    result.insert(
        "diagnostics".into(),
        serde_json::to_value(diagnostics).expect("diagnostics serialize"),
    );
    result.insert("reflected".into(), serde_json::Value::Bool(reflected));
    result.insert("commit".into(), commit);
    result.insert("commits".into(), serde_json::Value::Object(commits_obj));
    result.insert(
        "untracked_files".into(),
        serde_json::Value::Array(
            commits
                .untracked_files
                .iter()
                .map(|p| serde_json::Value::String(p.display().to_string()))
                .collect(),
        ),
    );
    result.extend(extra);
    serde_json::to_value(Response::ready(version, serde_json::Value::Object(result)))
        .expect("response serializes")
}

/// Run a batch: every file-level op, one saga, one commit per touched member.
///
/// All-or-nothing by construction: the ops run inside [`apply_saga`], so a
/// rejection on any op compensates the whole set and commits nothing. The ops
/// execute in the caller's order, which is why two ops on one path compose.
async fn batch_mutation(handle: &EngineHandle, args: BatchArgs) -> serde_json::Value {
    if args.ops.is_empty() {
        return mutation_reject_frame(crate::mutate::MutationReject::new(
            "`ops` is empty: a batch with nothing to do is not a mutation",
        ));
    }
    if args.ops.len() > MAX_BATCH_OPS {
        return mutation_reject_frame(crate::mutate::MutationReject::new(format!(
            "batch of {} ops exceeds the {MAX_BATCH_OPS}-op bound; split it into several commits",
            args.ops.len()
        )));
    }
    // Resolve every target first, so one out-of-scope path rejects the batch
    // before any write is planned.
    let mut targets = Vec::with_capacity(args.ops.len());
    for op in &args.ops {
        let path = match op {
            BatchOp::Write { path, .. }
            | BatchOp::Edit { path, .. }
            | BatchOp::Delete { path, .. } => path,
        };
        match resolve_mutation_target(handle, path) {
            Ok(t) => targets.push(t),
            Err(frame) => return frame,
        }
    }
    if args.require_clean {
        let set = match batch_change_set(&args.ops, &targets) {
            Ok(set) => set,
            Err(reject) => return mutation_reject_frame(reject),
        };
        if let Some(frame) = clean_check_batch(handle, set, &args.ops, &targets).await {
            return frame;
        }
    }
    let plan = saga_plan_off_thread(handle, targets.clone()).await;
    let marker_dir = saga_marker_dir(handle.root());
    let summary = format!("batch {} ops", args.ops.len());
    let blocking_handle = handle.clone();
    let ops = args.ops;
    let resolved = targets;
    let in_saga = resolved.clone();
    let outcome = tokio::task::spawn_blocking(move || {
        let (applied, commits) = apply_saga(&ShellGit, &marker_dir, &plan, summary, false, || {
            apply_batch_ops(&ops, &in_saga)
        })?;
        blocking_handle.rebuild_paths(saga_dirty_set(&plan));
        Ok::<_, crate::mutate::MutationReject>((applied, commits))
    })
    .await
    .expect("batch task never panics");
    match outcome {
        Err(reject) => mutation_reject_frame(reject),
        Ok((applied, commits)) => batch_response(handle, &resolved, &applied, commits),
    }
}

/// Apply every op in order, off the runtime thread. A rejection on any op
/// returns early; the caller's saga then compensates what already landed.
fn apply_batch_ops(
    ops: &[BatchOp],
    resolved: &[PathBuf],
) -> Result<Vec<serde_json::Value>, crate::mutate::MutationReject> {
    let mut applied = Vec::with_capacity(ops.len());
    for (op, target) in ops.iter().zip(resolved) {
        let entry = match op {
            BatchOp::Write {
                content,
                expected_hash,
                ..
            } => {
                let hash = crate::mutate::write_file(target, content, expected_hash.as_deref())?;
                serde_json::json!({
                    "op": "write_file",
                    "path": target.display().to_string(),
                    "hash": crate::mutate::hash_hex(hash),
                })
            }
            BatchOp::Edit {
                old_string,
                new_string,
                replace_all,
                ..
            } => {
                let hash = crate::mutate::edit_file(target, old_string, new_string, *replace_all)?;
                serde_json::json!({
                    "op": "edit_file",
                    "path": target.display().to_string(),
                    "hash": crate::mutate::hash_hex(hash),
                })
            }
            BatchOp::Delete { expected_hash, .. } => {
                crate::mutate::delete_file(target, expected_hash.as_deref())?;
                serde_json::json!({
                    "op": "delete_file",
                    "path": target.display().to_string(),
                    "hash": serde_json::Value::Null,
                })
            }
        };
        applied.push(entry);
    }
    Ok(applied)
}

/// The batch answer: one entry per op, the diagnostics of every touched path,
/// and the commits the saga made. `commit` is null at this level — a batch can
/// span members, so the provenance is `commits`, keyed by repo.
fn batch_response(
    handle: &EngineHandle,
    targets: &[PathBuf],
    applied: &[serde_json::Value],
    commits: SagaCommits,
) -> serde_json::Value {
    let paths: Vec<PathBuf> = targets.to_vec();
    // The FINAL hash per path, not one entry per op: `reflected` asks whether the held
    // catalog agrees with what the batch left on disk. Two ops on one path write two
    // hashes and the catalog can hold only the last, so keeping the intermediates makes
    // `reflected` false forever. An order-preserving map collapses them to the last write.
    let mut final_by_path: std::collections::BTreeMap<PathBuf, Option<crate::ir::ContentHash>> =
        std::collections::BTreeMap::new();
    for entry in applied {
        let Some(path) = entry["path"].as_str() else {
            continue;
        };
        // A null hash is a delete: the path's final state is "absent", and the catalog
        // agreeing means no entry, not a match on an earlier write's hash.
        let state = entry["hash"]
            .as_str()
            .and_then(|hash| u64::from_str_radix(hash, 16).ok())
            .map(crate::ir::ContentHash);
        final_by_path.insert(PathBuf::from(path), state);
    }
    let written: Vec<(PathBuf, Option<crate::ir::ContentHash>)> =
        final_by_path.into_iter().collect();
    let read = handle.read(move |v| {
        let diagnostics: Vec<Diagnostic> = v
            .diagnostics()
            .filter(|d| {
                paths.iter().any(|p| {
                    DiagnosticsScope {
                        path: Some(p.clone()),
                        ..Default::default()
                    }
                    .matches(v, d)
                })
            })
            .cloned()
            .collect();
        let reflected = written
            .iter()
            .all(|(path, state)| v.catalog.get(path).and_then(|e| e.hash) == *state);
        (diagnostics, reflected)
    });
    let Read::Ready {
        version,
        value: (diagnostics, reflected),
    } = read
    else {
        return serde_json::to_value(Response::not_ready()).expect("response serializes");
    };
    let commits_obj: serde_json::Map<String, serde_json::Value> = commits
        .iter()
        .map(|(repo, sha)| {
            (
                repo.as_str().to_string(),
                serde_json::Value::String(sha.0.clone()),
            )
        })
        .collect();
    let mut result = serde_json::Map::new();
    result.insert("ops".into(), serde_json::Value::Array(applied.to_vec()));
    result.insert(
        "diagnostics".into(),
        serde_json::to_value(diagnostics).expect("diagnostics serialize"),
    );
    result.insert("reflected".into(), serde_json::Value::Bool(reflected));
    result.insert("commit".into(), serde_json::Value::Null);
    result.insert("commits".into(), serde_json::Value::Object(commits_obj));
    result.insert(
        "untracked_files".into(),
        serde_json::Value::Array(
            commits
                .untracked_files
                .iter()
                .map(|p| serde_json::Value::String(p.display().to_string()))
                .collect(),
        ),
    );
    serde_json::to_value(Response::ready(version, serde_json::Value::Object(result)))
        .expect("response serializes")
}

/// A rejected mutation: an `error` frame, nothing written. `detail` carries
/// machine-usable context (e.g. the current hash on a staleness conflict).
fn mutation_reject_frame(reject: crate::mutate::MutationReject) -> serde_json::Value {
    let mut frame = serde_json::json!({
        "type": "error",
        "schema_version": SCHEMA_VERSION,
        "error": reject.message,
    });
    if let Some(detail) = reject.detail {
        frame["detail"] = detail;
    }
    frame
}

/// Validate a mutation's caller attribution into commit trailers, or the
/// error frame to return. A reserved-key collision or a malformed trailer
/// rejects the whole mutation before any write, so nothing lands half-attributed.
fn resolve_attribution(
    input: &[TrailerInput],
) -> Result<Vec<crate::gitwriter::CommitTrailer>, serde_json::Value> {
    let pairs: Vec<(String, String)> = input
        .iter()
        .map(|t| (t.key.clone(), t.value.clone()))
        .collect();
    crate::gitwriter::validate_attribution(&pairs)
        .map_err(|msg| mutation_reject_frame(crate::mutate::MutationReject::new(msg)))
}

/// The ref's lifecycle as the `ready` read and channel both report it.
fn lifecycle_json(handle: &EngineHandle) -> serde_json::Value {
    let (engine_state, ref_state) = handle.lifecycle();
    serde_json::json!({
        "engine": format!("{engine_state:?}").to_lowercase(),
        "ref": format!("{ref_state:?}").to_lowercase(),
    })
}

/// Drive one subscription for the life of the connection: ack, then the initial
/// value when the channel has one, then a change event on every relevant
/// rebuild. Ends when a send fails (the connection went away) or the engine's
/// version signal closes.
async fn run_subscription(
    handle: EngineHandle,
    frame_tx: FrameTx,
    id: u64,
    request_id: Option<serde_json::Value>,
    sub: Subscription,
) {
    // Register the version receiver before the ack, so a rebuild landing
    // between the ack and the channel's first await cannot be missed: the
    // receiver already holds the pre-ack version as its seen mark.
    let rx = handle.subscribe_version();
    if !send_frame(&frame_tx, ack_frame(id, sub.channel(), &request_id)).await {
        return;
    }
    match sub {
        Subscription::Changes => run_changes(handle, frame_tx, id, rx).await,
        Subscription::Lifecycle => run_ready(handle, frame_tx, id, rx).await,
        Subscription::Types => run_types(handle, frame_tx, id, rx).await,
        Subscription::Files => run_files(handle, frame_tx, id, rx).await,
        Subscription::Diagnostics(args) => {
            let scope = args.resolve(handle.root());
            run_diagnostics(handle, frame_tx, id, scope, rx).await
        }
        Subscription::LinkGraph(args) => {
            let scope = args.scope();
            let repo = args.repo;
            run_link_graph(handle, frame_tx, id, rx, repo, scope).await
        }
        Subscription::TypeGraph(args) => {
            let scope = args.scope();
            let repo = args.repo;
            let classes = EdgeClasses::from_arg(args.edges);
            run_type_graph(handle, frame_tx, id, rx, repo, scope, classes).await
        }
        // Liveness is the reflog watcher, not the version signal, so the
        // version receiver `rx` is unused here; the task subscribes to git
        // events itself.
        Subscription::RecentCommits(args) => run_recent_commits(handle, frame_tx, id, args).await,
    }
}

/// `changes`: no initial value, a change event on every rebuild, carrying the
/// net file delta in its scope hint. The task holds the file→content-hash map
/// it last reported and diffs it on each wake, so coalesced rebuilds yield one
/// event covering everything since the subscriber's last delivery. A wake
/// whose net delta is empty (rebuilds that cancelled out) does not fire.
async fn run_changes(
    handle: EngineHandle,
    frame_tx: FrameTx,
    id: u64,
    mut rx: watch::Receiver<u64>,
) {
    // The baseline at subscribe time. Empty while Deriving, so a subscriber
    // arriving before the first build sees every file as added by it.
    let mut last = files_snapshot(&handle).map(|(_, s)| s).unwrap_or_default();
    loop {
        if rx.changed().await.is_err() {
            return;
        }
        let Some((version, cur)) = files_snapshot(&handle) else {
            continue;
        };
        let delta = files_delta(&last, &cur);
        if !delta.is_empty() {
            if !send_frame(
                &frame_tx,
                files_change_event_frame(id, "knowledge-base-changed", version, &delta),
            )
            .await
            {
                return;
            }
            last = cur;
        }
    }
}

/// The catalogued files and their content hashes, coherent at one version.
/// `None` while Deriving. The hash is `None` for files the build doesn't read
/// (assets), so their edits never appear as modifications — matching the
/// rebuild trigger, which fingerprints them by presence only.
fn files_snapshot(
    handle: &EngineHandle,
) -> Option<(u64, BTreeMap<PathBuf, Option<crate::ir::ContentHash>>)> {
    match handle.read(|v| v.catalog.iter().map(|(p, e)| (p.clone(), e.hash)).collect()) {
        Read::Ready { version, value } => Some((version, value)),
        Read::NotReady => None,
    }
}

/// The net file delta between two snapshots: paths that appeared, vanished,
/// or changed content. Lists are sorted (snapshot order).
struct FilesDelta {
    added: Vec<String>,
    removed: Vec<String>,
    modified: Vec<String>,
}

impl FilesDelta {
    fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.modified.is_empty()
    }
}

fn files_delta(
    last: &BTreeMap<PathBuf, Option<crate::ir::ContentHash>>,
    cur: &BTreeMap<PathBuf, Option<crate::ir::ContentHash>>,
) -> FilesDelta {
    let mut delta = FilesDelta {
        added: Vec::new(),
        removed: Vec::new(),
        modified: Vec::new(),
    };
    for (path, hash) in cur {
        match last.get(path) {
            None => delta.added.push(path.display().to_string()),
            Some(last_hash) if last_hash != hash => delta.modified.push(path.display().to_string()),
            _ => {}
        }
    }
    for path in last.keys() {
        if !cur.contains_key(path) {
            delta.removed.push(path.display().to_string());
        }
    }
    delta
}

/// A `change_event` frame whose scope hint carries a file delta.
fn files_change_event_frame(
    subscription_id: u64,
    kind: &str,
    at_version: u64,
    delta: &FilesDelta,
) -> serde_json::Value {
    serde_json::json!({
        "type": "change_event",
        "schema_version": SCHEMA_VERSION,
        "subscription_id": subscription_id,
        "kind": kind,
        "at_version": at_version,
        "scope_hint": {
            "scope": "files",
            "added": delta.added,
            "removed": delta.removed,
            "modified": delta.modified,
        },
    })
}

/// `ready`: the current lifecycle immediately, then a `ready-changed` event on
/// the Deriving-to-Ready transition.
async fn run_ready(handle: EngineHandle, frame_tx: FrameTx, id: u64, mut rx: watch::Receiver<u64>) {
    let at_version = handle.version().unwrap_or(0);
    let (_, ref_state) = handle.lifecycle();
    let mut was_ready = ref_state == RefState::Ready;
    if !send_frame(
        &frame_tx,
        initial_value_frame(
            id,
            at_version,
            serde_json::json!({ "lifecycle": lifecycle_json(&handle) }),
        ),
    )
    .await
    {
        return;
    }
    loop {
        if rx.changed().await.is_err() {
            return;
        }
        let (_, ref_state) = handle.lifecycle();
        let now_ready = ref_state == RefState::Ready;
        if now_ready && !was_ready {
            let version = handle.version().unwrap_or(*rx.borrow());
            if !send_frame(
                &frame_tx,
                change_event_frame(id, "lifecycle-changed", version),
            )
            .await
            {
                return;
            }
        }
        was_ready = now_ready;
    }
}

/// `files`: the catalogued paths as the initial value, then a
/// `files-changed` event when the path set differs across a rebuild,
/// the scope hint carrying the added and removed paths. The task holds the
/// path set it last reported, so coalesced rebuilds yield one net delta.
async fn run_files(handle: EngineHandle, frame_tx: FrameTx, id: u64, mut rx: watch::Receiver<u64>) {
    if wait_until_ready(&handle, &mut rx).await.is_none() {
        return;
    }
    let Some((version, mut last)) = files_snapshot(&handle) else {
        return;
    };
    let files: Vec<serde_json::Value> = last
        .keys()
        .map(|p| serde_json::json!({ "path": p.display().to_string() }))
        .collect();
    if !send_frame(
        &frame_tx,
        initial_value_frame(id, version, serde_json::Value::Array(files)),
    )
    .await
    {
        return;
    }
    loop {
        if rx.changed().await.is_err() {
            return;
        }
        let Some((version, cur)) = files_snapshot(&handle) else {
            continue;
        };
        let added: Vec<String> = cur
            .keys()
            .filter(|p| !last.contains_key(*p))
            .map(|p| p.display().to_string())
            .collect();
        let removed: Vec<String> = last
            .keys()
            .filter(|p| !cur.contains_key(*p))
            .map(|p| p.display().to_string())
            .collect();
        if !(added.is_empty() && removed.is_empty()) {
            let frame = serde_json::json!({
                "type": "change_event",
                "schema_version": SCHEMA_VERSION,
                "subscription_id": id,
                "kind": "files-changed",
                "at_version": version,
                "scope_hint": { "scope": "files", "added": added, "removed": removed },
            });
            if !send_frame(&frame_tx, frame).await {
                return;
            }
            last = cur;
        }
    }
}

/// `types`: the workspace-wide type-def introspection as the initial value, the
/// same owner-deduped owner-annotated set the `types` read serves, then a
/// `types-changed` event when any type-def's wire introspection differs
/// across a rebuild, the scope hint naming the added, removed, and changed
/// types. Diffing the projection is more precise than the old type-def-file
/// content token: a file edit that leaves every introspection unchanged does
/// not fire.
async fn run_types(handle: EngineHandle, frame_tx: FrameTx, id: u64, mut rx: watch::Receiver<u64>) {
    if wait_until_ready(&handle, &mut rx).await.is_none() {
        return;
    }
    let Some((version, initial, mut last)) = types_snapshot(&handle) else {
        return;
    };
    if !send_frame(&frame_tx, initial_value_frame(id, version, initial)).await {
        return;
    }
    loop {
        if rx.changed().await.is_err() {
            return;
        }
        let Some((version, _, cur)) = types_snapshot(&handle) else {
            continue;
        };
        let mut added = Vec::new();
        let mut removed = Vec::new();
        let mut changed = Vec::new();
        // A scope-hint entry is the type's identity handle, `{ name, repo, hash }`,
        // not a bare name, so a consumer knows WHICH same-named type changed.
        let entry = |key: &(String, String), intro: &serde_json::Value| {
            serde_json::json!({
                "name": key.0,
                "repo": key.1,
                "hash": intro.get("hash").cloned().unwrap_or(serde_json::Value::Null),
            })
        };
        for (key, intro) in &cur {
            match last.get(key) {
                None => added.push(entry(key, intro)),
                Some(last_intro) if last_intro != intro => changed.push(entry(key, intro)),
                _ => {}
            }
        }
        for (key, intro) in &last {
            if !cur.contains_key(key) {
                removed.push(entry(key, intro));
            }
        }
        if !(added.is_empty() && removed.is_empty() && changed.is_empty()) {
            let frame = serde_json::json!({
                "type": "change_event",
                "schema_version": SCHEMA_VERSION,
                "subscription_id": id,
                "kind": "types-changed",
                "at_version": version,
                "scope_hint": {
                    "scope": "types",
                    "added": added,
                    "removed": removed,
                    "changed": changed,
                },
            });
            if !send_frame(&frame_tx, frame).await {
                return;
            }
            last = cur;
        }
    }
}

/// The type graph coherent at one version: the version, the `types` read's array
/// shape, and the same introspections keyed by type identity.
type TypesSnapshot = (
    u64,
    serde_json::Value,
    BTreeMap<(String, String), serde_json::Value>,
);

/// The type graph coherent at one version: the `types` read's array shape for
/// the wire, plus the same introspections keyed by type name for the change
/// diff. `None` while Deriving.
fn types_snapshot(handle: &EngineHandle) -> Option<TypesSnapshot> {
    match handle.read(|v| wire::introspect_workspace_types(v, wire::TypeScope::all())) {
        Read::Ready { version, value } => {
            // Key by IDENTITY, `(name, owner repo)`, not bare name. Two mounted
            // repos can each own a same-named type, so a bare-name key collides
            // and a change to the shadowed one is silently dropped.
            let by_id = value
                .iter()
                .map(|t| {
                    (
                        (t.def.name.clone(), t.repo.clone()),
                        serde_json::to_value(t).expect("type serializes"),
                    )
                })
                .collect();
            let array = serde_json::to_value(&value).expect("types serialize");
            Some((version, array, by_id))
        }
        Read::NotReady => None,
    }
}

/// Park on the version receiver until the ref is Ready, returning the version
/// then. `None` if the engine's version channel closes first.
async fn wait_until_ready(handle: &EngineHandle, rx: &mut watch::Receiver<u64>) -> Option<u64> {
    loop {
        if let Some(version) = handle.version() {
            return Some(version);
        }
        if rx.changed().await.is_err() {
            return None;
        }
    }
}

/// `diagnostics`: the in-scope diagnostics as the initial value, then a
/// `diagnostics-changed` event whenever the in-scope set differs across a
/// rebuild. The coarse delta tokens don't carry per-file detail under the
/// full-rebuild engine, so the task holds the last delivered set and diffs
/// it on each version advance.
async fn run_diagnostics(
    handle: EngineHandle,
    frame_tx: FrameTx,
    id: u64,
    scope: DiagnosticsScope,
    mut rx: watch::Receiver<u64>,
) {
    if wait_until_ready(&handle, &mut rx).await.is_none() {
        return;
    }
    let Some((version, flat, mut last_by_file)) = diagnostics_snapshot(&handle, &scope) else {
        return;
    };
    if !send_frame(&frame_tx, initial_value_frame(id, version, flat)).await {
        return;
    }
    loop {
        if rx.changed().await.is_err() {
            return;
        }
        let Some((version, _, cur_by_file)) = diagnostics_snapshot(&handle, &scope) else {
            continue;
        };
        let changed = changed_files(&last_by_file, &cur_by_file);
        if !changed.is_empty() {
            if !send_frame(
                &frame_tx,
                diagnostics_change_event_frame(id, version, &changed),
            )
            .await
            {
                return;
            }
            last_by_file = cur_by_file;
        }
    }
}

/// A coherent read of the diagnostics in the subscription's scope.
///
/// Returns the version, the flat array in the `diagnostics` read's shape and
/// knowledge base order for the wire, plus the same diagnostics grouped by file for
/// the change diff. An empty scope is whole-knowledge-base. `None` while Deriving.
fn diagnostics_snapshot(
    handle: &EngineHandle,
    scope: &DiagnosticsScope,
) -> Option<(u64, serde_json::Value, BTreeMap<String, serde_json::Value>)> {
    match handle.read(|v| {
        let filtered: Vec<&Diagnostic> = scoped_diagnostics(v, scope).collect();
        let flat = serde_json::to_value(&filtered).expect("diagnostics serialize");
        let mut by_file: BTreeMap<String, Vec<&Diagnostic>> = BTreeMap::new();
        for d in &filtered {
            by_file
                .entry(d.span.file.display().to_string())
                .or_default()
                .push(d);
        }
        let by_file = by_file
            .into_iter()
            .map(|(file, ds)| {
                (
                    file,
                    serde_json::to_value(ds).expect("diagnostics serialize"),
                )
            })
            .collect();
        (flat, by_file)
    }) {
        Read::Ready {
            version,
            value: (flat, by_file),
        } => Some((version, flat, by_file)),
        Read::NotReady => None,
    }
}

/// The files whose diagnostic set differs between two snapshots: changed,
/// added, or cleared. Sorted, deduplicated.
fn changed_files(
    last: &BTreeMap<String, serde_json::Value>,
    cur: &BTreeMap<String, serde_json::Value>,
) -> Vec<String> {
    let mut changed = Vec::new();
    for (file, diags) in cur {
        if last.get(file) != Some(diags) {
            changed.push(file.clone());
        }
    }
    for file in last.keys() {
        if !cur.contains_key(file) {
            changed.push(file.clone());
        }
    }
    changed.sort();
    changed.dedup();
    changed
}

/// A `diagnostics-changed` event. The scope hint is precise here, unlike the
/// coarse knowledge base hint other channels carry: the engine diffs diagnostics by
/// file, so it names exactly the files whose diagnostic set changed.
fn diagnostics_change_event_frame(
    subscription_id: u64,
    at_version: u64,
    files: &[String],
) -> serde_json::Value {
    serde_json::json!({
        "type": "change_event",
        "schema_version": SCHEMA_VERSION,
        "subscription_id": subscription_id,
        "kind": "diagnostics-changed",
        "at_version": at_version,
        "scope_hint": { "scope": "files", "files": files },
    })
}

/// `link_graph`: the full node+edge payload as the initial value, then a node/
/// edge DELTA per rebuild whose net change is non-empty. The task holds the
/// node and edge sets it last reported and diffs them on each wake, so a rebuild
/// that leaves the graph unchanged in scope does not fire. The same
/// hold-last-snapshot-and-diff shape `changes` / `files` / `diagnostics` use.
async fn run_link_graph(
    handle: EngineHandle,
    frame_tx: FrameTx,
    id: u64,
    mut rx: watch::Receiver<u64>,
    repo: Option<String>,
    scope: wire::TypeScope,
) {
    if wait_until_ready(&handle, &mut rx).await.is_none() {
        return;
    }
    let Some((version, mut last)) = link_graph_snapshot(&handle, repo.as_deref(), scope) else {
        return;
    };
    if !send_frame(
        &frame_tx,
        initial_value_frame(
            id,
            version,
            serde_json::to_value(&last).expect("link_graph serializes"),
        ),
    )
    .await
    {
        return;
    }
    loop {
        if rx.changed().await.is_err() {
            return;
        }
        let Some((version, cur)) = link_graph_snapshot(&handle, repo.as_deref(), scope) else {
            continue;
        };
        let delta = link_graph_delta(&last, &cur);
        if !delta.is_empty() {
            if !send_frame(
                &frame_tx,
                link_graph_change_event_frame(id, version, &delta),
            )
            .await
            {
                return;
            }
            last = cur;
        }
    }
}

/// A coherent read of the whole-graph payload in the subscription's scope.
/// `None` while Deriving.
fn link_graph_snapshot(
    handle: &EngineHandle,
    repo: Option<&str>,
    scope: wire::TypeScope,
) -> Option<(u64, LinkGraphView)> {
    match handle.read(|v| link_graph_view(v, repo, scope)) {
        Read::Ready { version, value } => Some((version, value)),
        Read::NotReady => None,
    }
}

/// The net change between two whole-graph payloads. Nodes diff by path: a node
/// is `added` when its path is new OR its record changed (a re-referenced file
/// re-emits with new ref counts), `removed` when its path is gone. Edges diff as
/// a MULTISET over the sorted edge lists, so preserved multiplicity in the read
/// carries through to the delta.
#[derive(Debug, Serialize)]
struct LinkGraphDelta {
    nodes_added: Vec<GraphNodeView>,
    nodes_removed: Vec<String>,
    edges_added: Vec<GraphEdgeView>,
    edges_removed: Vec<GraphEdgeView>,
}

impl LinkGraphDelta {
    fn is_empty(&self) -> bool {
        self.nodes_added.is_empty()
            && self.nodes_removed.is_empty()
            && self.edges_added.is_empty()
            && self.edges_removed.is_empty()
    }
}

fn link_graph_delta(last: &LinkGraphView, cur: &LinkGraphView) -> LinkGraphDelta {
    let last_nodes: std::collections::HashMap<&str, &GraphNodeView> =
        last.nodes.iter().map(|n| (n.path.as_str(), n)).collect();
    let cur_nodes: std::collections::HashMap<&str, &GraphNodeView> =
        cur.nodes.iter().map(|n| (n.path.as_str(), n)).collect();
    let nodes_added: Vec<GraphNodeView> = cur
        .nodes
        .iter()
        .filter(|n| last_nodes.get(n.path.as_str()).is_none_or(|old| *old != *n))
        .cloned()
        .collect();
    let nodes_removed: Vec<String> = last
        .nodes
        .iter()
        .filter(|n| !cur_nodes.contains_key(n.path.as_str()))
        .map(|n| n.path.clone())
        .collect();

    let (edges_added, edges_removed) = edge_multiset_diff(&last.edges, &cur.edges);

    LinkGraphDelta {
        nodes_added,
        nodes_removed,
        edges_added,
        edges_removed,
    }
}

/// The multiset difference of two edge lists, each already sorted by
/// `(from, to, kind, surface)`. A merge walk pairs equal edges, so surplus in
/// `cur` is `added` and surplus in `last` is `removed`, multiplicity-correct
/// (two identical edges dropped to one yields one `removed`).
fn edge_multiset_diff(
    last: &[GraphEdgeView],
    cur: &[GraphEdgeView],
) -> (Vec<GraphEdgeView>, Vec<GraphEdgeView>) {
    let key = |e: &GraphEdgeView| {
        (
            e.from.clone(),
            e.to.clone(),
            e.kind.to_string(),
            e.surface.to_string(),
        )
    };
    let (mut i, mut j) = (0usize, 0usize);
    let (mut added, mut removed) = (Vec::new(), Vec::new());
    while i < last.len() && j < cur.len() {
        match key(&last[i]).cmp(&key(&cur[j])) {
            std::cmp::Ordering::Less => {
                removed.push(last[i].clone());
                i += 1;
            }
            std::cmp::Ordering::Greater => {
                added.push(cur[j].clone());
                j += 1;
            }
            std::cmp::Ordering::Equal => {
                i += 1;
                j += 1;
            }
        }
    }
    removed.extend(last[i..].iter().cloned());
    added.extend(cur[j..].iter().cloned());
    (added, removed)
}

/// A `change_event` frame carrying the whole-graph node/edge delta in its scope
/// hint, `link_graph` scope.
fn link_graph_change_event_frame(
    subscription_id: u64,
    at_version: u64,
    delta: &LinkGraphDelta,
) -> serde_json::Value {
    serde_json::json!({
        "type": "change_event",
        "schema_version": SCHEMA_VERSION,
        "subscription_id": subscription_id,
        "kind": "link-graph-changed",
        "at_version": at_version,
        "scope_hint": {
            "scope": "link_graph",
            "nodes_added": delta.nodes_added,
            "nodes_removed": delta.nodes_removed,
            "edges_added": delta.edges_added,
            "edges_removed": delta.edges_removed,
        },
    })
}

/// `type_graph`: the schema-graph payload as the initial value, then a node/edge
/// DELTA per rebuild whose net change is non-empty. The type-side sibling of
/// [`run_link_graph`], the same hold-last-snapshot-and-diff shape.
async fn run_type_graph(
    handle: EngineHandle,
    frame_tx: FrameTx,
    id: u64,
    mut rx: watch::Receiver<u64>,
    repo: Option<String>,
    scope: wire::TypeScope,
    classes: EdgeClasses,
) {
    if wait_until_ready(&handle, &mut rx).await.is_none() {
        return;
    }
    let Some((version, mut last)) = type_graph_snapshot(&handle, repo.as_deref(), scope, classes)
    else {
        return;
    };
    if !send_frame(
        &frame_tx,
        initial_value_frame(
            id,
            version,
            serde_json::to_value(&last).expect("type_graph serializes"),
        ),
    )
    .await
    {
        return;
    }
    loop {
        if rx.changed().await.is_err() {
            return;
        }
        let Some((version, cur)) = type_graph_snapshot(&handle, repo.as_deref(), scope, classes)
        else {
            continue;
        };
        let delta = type_graph_delta(&last, &cur);
        if !delta.is_empty() {
            if !send_frame(
                &frame_tx,
                type_graph_change_event_frame(id, version, &delta),
            )
            .await
            {
                return;
            }
            last = cur;
        }
    }
}

/// A coherent read of the type-graph payload in the subscription's scope.
/// `None` while Deriving.
fn type_graph_snapshot(
    handle: &EngineHandle,
    repo: Option<&str>,
    scope: wire::TypeScope,
    classes: EdgeClasses,
) -> Option<(u64, TypeGraphView)> {
    match handle.read(|v| type_graph_view(v, repo, scope, classes)) {
        Read::Ready { version, value } => Some((version, value)),
        Read::NotReady => None,
    }
}

/// The net change between two type-graph payloads. Nodes diff by path (a node
/// re-emits when its record changes); edges are unique per
/// `(from, to, relation)`, so an edge re-emits when its `count` changes.
#[derive(Debug, Serialize)]
struct TypeGraphDelta {
    nodes_added: Vec<TypeGraphNodeView>,
    nodes_removed: Vec<String>,
    edges_added: Vec<TypeGraphEdgeView>,
    edges_removed: Vec<TypeGraphEdgeView>,
}

impl TypeGraphDelta {
    fn is_empty(&self) -> bool {
        self.nodes_added.is_empty()
            && self.nodes_removed.is_empty()
            && self.edges_added.is_empty()
            && self.edges_removed.is_empty()
    }
}

fn type_graph_delta(last: &TypeGraphView, cur: &TypeGraphView) -> TypeGraphDelta {
    use std::collections::HashMap;
    let last_nodes: HashMap<&str, &TypeGraphNodeView> =
        last.nodes.iter().map(|n| (n.path.as_str(), n)).collect();
    let cur_nodes: HashMap<&str, &TypeGraphNodeView> =
        cur.nodes.iter().map(|n| (n.path.as_str(), n)).collect();
    let nodes_added: Vec<TypeGraphNodeView> = cur
        .nodes
        .iter()
        .filter(|n| last_nodes.get(n.path.as_str()).is_none_or(|old| *old != *n))
        .cloned()
        .collect();
    let nodes_removed: Vec<String> = last
        .nodes
        .iter()
        .filter(|n| !cur_nodes.contains_key(n.path.as_str()))
        .map(|n| n.path.clone())
        .collect();

    let key = |e: &TypeGraphEdgeView| (e.from.clone(), e.to.clone(), e.relation);
    let last_edges: HashMap<(String, String, &'static str), &TypeGraphEdgeView> =
        last.edges.iter().map(|e| (key(e), e)).collect();
    let cur_edges: HashMap<(String, String, &'static str), &TypeGraphEdgeView> =
        cur.edges.iter().map(|e| (key(e), e)).collect();
    let edges_added: Vec<TypeGraphEdgeView> = cur
        .edges
        .iter()
        .filter(|e| last_edges.get(&key(e)).is_none_or(|old| *old != *e))
        .cloned()
        .collect();
    let edges_removed: Vec<TypeGraphEdgeView> = last
        .edges
        .iter()
        .filter(|e| !cur_edges.contains_key(&key(e)))
        .cloned()
        .collect();

    TypeGraphDelta {
        nodes_added,
        nodes_removed,
        edges_added,
        edges_removed,
    }
}

/// A `change_event` frame carrying the type-graph node/edge delta, `type_graph`
/// scope. The event kind stays kebab (`type-graph-changed`), reassigned from the
/// old introspection stream (now `types-changed`) to this drawable stream.
fn type_graph_change_event_frame(
    subscription_id: u64,
    at_version: u64,
    delta: &TypeGraphDelta,
) -> serde_json::Value {
    serde_json::json!({
        "type": "change_event",
        "schema_version": SCHEMA_VERSION,
        "subscription_id": subscription_id,
        "kind": "type-graph-changed",
        "at_version": at_version,
        "scope_hint": {
            "scope": "type_graph",
            "nodes_added": delta.nodes_added,
            "nodes_removed": delta.nodes_removed,
            "edges_added": delta.edges_added,
            "edges_removed": delta.edges_removed,
        },
    })
}

/// Test-only hooks into the request path.
///
/// The one thing a histogram can be wrong about is *what it timed*: a counter
/// that recorded a constant, or the frame's write, would look plausible and
/// prove nothing. `delay` makes the handler demonstrably slow for one named verb,
/// so a test can assert the bucket moved with it.
#[cfg(test)]
mod test_hooks {
    use std::sync::Mutex;
    use std::time::Duration;

    static DELAY: Mutex<Option<(&'static str, Duration)>> = Mutex::new(None);

    /// Arm one verb to sleep inside the handler, replacing any earlier arm.
    pub(crate) fn delay_verb(verb: &'static str, delay: Duration) {
        *armed() = Some((verb, delay));
    }

    /// Clear the arm, so a later request is not slowed by a finished test.
    pub(crate) fn clear() {
        *armed() = None;
    }

    pub(super) async fn delay(verb: &str) {
        let arm = *armed();
        if let Some((target, delay)) = arm {
            if target == verb {
                // A blocking task, not `tokio::time`: the crate does not enable
                // tokio's `time` feature for a hook that only tests use.
                let _ = tokio::task::spawn_blocking(move || std::thread::sleep(delay)).await;
            }
        }
    }

    fn armed() -> std::sync::MutexGuard<'static, Option<(&'static str, Duration)>> {
        // A panicking assertion in one test must not poison the hook for the rest.
        DELAY.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Answer one request, timing it for the per-verb counters.
///
/// The counters wrap the WHOLE dispatch, so a handler that rejects its arguments
/// early is counted like an answered read: the number that matters is "how long
/// does this verb take", not "how long does a successful call take". The clock
/// sits outside the dispatch precisely so an early `return` cannot skip it —
/// which is why the dispatch is its own function rather than the body of this
/// one.
///
/// The frame's write and the client's round trip are NOT counted; this is handler
/// time, so a histogram says what the engine spent, not what the socket did.
async fn handle_request(handle: &EngineHandle, req: Request) -> Response {
    let verb = req.verb();
    let started = Instant::now();
    // Test-only: an armed artificial delay proves the histogram times the
    // handler instead of recording a constant. A no-op outside `cfg(test)`.
    #[cfg(test)]
    test_hooks::delay(verb).await;
    let response = dispatch(handle, req).await;
    crate::verb_metrics::record(verb, started.elapsed());
    response
}

/// The dispatch itself: one arm per verb, answering against the held state.
async fn dispatch(handle: &EngineHandle, req: Request) -> Response {
    match req {
        Request::PreviewMutation(args) => preview_mutation(handle, args).await,
        Request::Shutdown => {
            // The daemon is NOT signalled here: the signal must not race the
            // response frame. `serve_connection` sends it only after the writer
            // has flushed this reply, so a client that asked to shut down always
            // receives the ack. `ready` tracks the ref's lifecycle so it stays coupled to
            // `version` (present exactly when Ready), like the `ready` probe.
            // The ack rides `result`, present regardless of readiness.
            let (_, ref_state) = handle.lifecycle();
            Response {
                kind: "response",
                schema_version: SCHEMA_VERSION,
                id: None,
                ready: ref_state == RefState::Ready,
                version: handle.version(),
                result: Some(serde_json::json!({ "shutting_down": true })),
                error: None,
            }
        }
        Request::Lifecycle => {
            let (_, ref_state) = handle.lifecycle();
            let ready = ref_state == RefState::Ready;
            Response {
                kind: "response",
                schema_version: SCHEMA_VERSION,
                id: None,
                ready,
                version: handle.version(),
                // The lifecycle probe answers even while not ready, so it wraps
                // its payload here rather than through `enveloped`.
                result: Some(serde_json::json!({ "lifecycle": lifecycle_json(handle) })),
                error: None,
            }
        }
        Request::Diagnostics(args) => {
            // Page over the served stream's stable order (by source, then
            // position). No total rides the array; a consumer pages until a
            // short page, or asks `diagnostic_counts` for the total.
            let offset = args.offset.unwrap_or(0);
            let limit = args.limit;
            let scope = args.resolve(handle.root());
            enveloped(
                "diagnostics",
                handle.read(move |v| {
                    scoped_diagnostics(v, &scope)
                        .skip(offset)
                        .take(limit.unwrap_or(usize::MAX))
                        .cloned()
                        .collect::<Vec<_>>()
                }),
            )
        }
        Request::DiagnosticCounts(args) => {
            // Counts over the FULL filtered set, paging ignored. The shape of
            // the problem without materializing every entry.
            let scope = args.resolve(handle.root());
            enveloped(
                "diagnostic_counts",
                handle.read(move |v| diagnostic_counts_view(v, &scope)),
            )
        }
        Request::Types(args) => {
            let offset = args.offset.unwrap_or(0);
            let limit = args.limit;
            let summary = args.summary;
            let scope = args.scope();
            let repo = args.repo;
            enveloped(
                "types",
                handle.read(move |v| match repo.as_deref() {
                    None => Some(wire::introspect_workspace_types_paged(
                        v, offset, limit, summary, scope,
                    )),
                    Some(name) => {
                        wire::introspect_repo_types_paged(v, name, offset, limit, summary, scope)
                    }
                }),
            )
        }
        Request::TypeCounts(args) => {
            let scope = args.scope();
            let repo = args.repo;
            enveloped(
                "type_counts",
                handle.read(move |v| wire::introspect_type_counts(v, repo.as_deref(), scope)),
            )
        }
        Request::TypeTree(args) => {
            let scope = args.scope();
            enveloped(
                "type_tree",
                handle.read(move |v| wire::introspect_type_tree(v, scope)),
            )
        }
        Request::Type(TypeArgs { name }) => enveloped(
            "type",
            handle.read(move |v| wire::introspect_type_by_name(v, &name)),
        ),
        Request::TypeBatch(TypeBatchArgs { names }) => enveloped(
            "type_batch",
            handle.read(move |v| wire::introspect_types_named(v, &names)),
        ),
        Request::InstancesOf(InstancesOfArgs {
            type_name,
            origins,
            instance,
            body,
        }) => instances_of_response(handle, type_name, origins, instance, body).await,
        Request::Imports(args) => {
            let scope = args.scope();
            enveloped(
                "imports",
                handle.read(move |v| wire::introspect_list_imports(v, scope)),
            )
        }
        Request::Subtypes(args) => {
            let scope = args.scope();
            let base = args.base;
            to_response(handle.read(move |v| wire::introspect_subtypes(v, &base, scope)))
        }
        Request::Instances => to_response(handle.read(|v| InstancesView {
            count: v.instances.size(),
            aborted_at_load: v.any_aborted(),
            instances: wire::introspect_kb_instances(v),
        })),
        Request::Candidates(args) => {
            let offset = args.offset.unwrap_or(0);
            let limit = args.limit;
            let summary = args.summary;
            to_response(handle.read(move |v| CandidatesView {
                aborted_at_load: v.any_aborted(),
                candidates: wire::introspect_kb_candidates_paged(v, offset, limit, summary),
            }))
        }
        Request::CandidateCounts(_) => enveloped(
            "candidate_counts",
            handle.read(wire::introspect_candidate_counts),
        ),
        Request::InstanceCounts(args) => {
            let scope = args.scope();
            let repo = args.repo;
            let origins = args.origins;
            enveloped(
                "instance_counts",
                handle.read(move |v| {
                    wire::introspect_instance_counts(v, repo.as_deref(), scope, origins.as_deref())
                }),
            )
        }
        Request::Instance { path } => {
            let target = handle.root().join(&path);
            enveloped("instance", handle.read(|v| resolved_view(v, &target)))
        }
        Request::ReferencesIn { path } => {
            let target = handle.root().join(&path);
            enveloped(
                "references_in",
                handle.read(|v| {
                    crate::verb_metrics::visit_scope("references_in", || backlinks_view(v, &target))
                }),
            )
        }
        Request::ReferencesOut { path } => {
            let target = abs_arg(handle.root(), &path);
            enveloped("references_out", handle.read(|v| outgoing_view(v, &target)))
        }
        Request::Pins {
            target,
            source_type,
        } => enveloped(
            "pins",
            handle.read(move |v| pins_view(v, &target, &source_type)),
        ),
        Request::CommitMeta { commits } => commit_meta(handle, commits).await,
        Request::FileHistory { path, repo } => file_history(handle, path, repo).await,
        Request::RollbackPreview(args) => rollback_preview(handle, args.repo, args.commit).await,
        Request::RecentCommits {
            members,
            limit,
            since,
        } => recent_commits(handle, members, limit, since).await,
        Request::Neighborhood(args) => {
            let seed = abs_arg(handle.root(), &args.path);
            // Cross-field validations serde cannot express, each an error frame.
            // The allowed kinds are the walk's own vocabulary, not a copy.
            use crate::backlinks::WALK_KINDS;
            if let Some(ks) = &args.kinds {
                if let Some(bad) = ks.iter().find(|k| !WALK_KINDS.contains(&k.as_str())) {
                    return Response::invalid(format!(
                        "unknown edge kind '{bad}'; expected one of {}",
                        WALK_KINDS.join(" / ")
                    ));
                }
            }
            // Past depth 1 an unfiltered walk explodes through navigational
            // fan-out, so a `kinds` set is required.
            if args.depth > 1 && args.kinds.is_none() {
                return Response::invalid(format!(
                    "`kinds` is required past depth 1 (an unfiltered deep walk \
                     explodes through navigational fan-out); name one or more of {}",
                    WALK_KINDS.join(" / ")
                ));
            }
            // The node cap floor is 1 (the seed always counts); 0 would return
            // the seed and set `truncated`, a contradiction. Reject it.
            if args.max_nodes == Some(0) {
                return Response::invalid("`max_nodes` must be at least 1; the seed always counts");
            }
            neighborhood_response(handle, seed, args).await
        }
        Request::ResolveTarget { target, origin } => {
            let origin = origin.map(|o| abs_arg(handle.root(), &o));
            enveloped(
                "resolve_target",
                handle.read(|v| resolve_target_view(v, &target, origin.as_deref())),
            )
        }
        Request::ResolveBlockId {
            target,
            block_id,
            origin,
        } => {
            let origin = origin.map(|o| abs_arg(handle.root(), &o));
            enveloped(
                "resolve_block_id",
                handle.read(|v| resolve_block_id_view(v, &target, &block_id, origin.as_deref())),
            )
        }
        Request::ResolveAnchor {
            target,
            anchor,
            origin,
        } => {
            let origin = origin.map(|o| abs_arg(handle.root(), &o));
            enveloped(
                "resolve_anchor",
                handle.read(|v| resolve_anchor_view(v, &target, &anchor, origin.as_deref())),
            )
        }
        Request::Anchors { target, origin } => {
            let origin = origin.map(|o| abs_arg(handle.root(), &o));
            enveloped(
                "anchors",
                handle.read(|v| anchors_view(v, &target, origin.as_deref())),
            )
        }
        Request::BlockIds { target, origin } => {
            let origin = origin.map(|o| abs_arg(handle.root(), &o));
            enveloped(
                "block_ids",
                handle.read(|v| block_ids_view(v, &target, origin.as_deref())),
            )
        }
        Request::Files(args) => {
            let scope = args.scope();
            let repo = args.repo.clone();
            let limit = args.limit.unwrap_or(usize::MAX);
            let offset = args.offset.unwrap_or(0);
            enveloped(
                "files",
                handle.read(move |v| files_view(v, repo.as_deref(), scope, limit, offset)),
            )
        }
        Request::Grep(args) => {
            if args.query.is_empty() {
                return Response::invalid("`query` must not be empty: it would match every line");
            }
            let scope = args.scope();
            let repo = args.repo.clone();
            let limit = args.limit.unwrap_or(GREP_DEFAULT_LIMIT).min(GREP_MAX_LIMIT);
            let offset = args.offset.unwrap_or(0);
            // The paths come from the held state; the bytes come from disk, the
            // same split the `content` read uses. Nothing here holds the read
            // lock while IO runs.
            let paths = match handle.read(move |v| grep_paths(v, repo.as_deref(), scope)) {
                Read::NotReady => return Response::not_ready(),
                Read::Ready { version, value } => (version, value),
            };
            let (version, paths) = paths;
            let query = args.query;
            let case_sensitive = args.case_sensitive;
            let scan = tokio::task::spawn_blocking(move || {
                // The scan runs OFF the request's own thread, so the visit scope is
                // opened HERE, inside the task: a scope is thread-local and the
                // handler's thread is not the one doing the walking.
                crate::verb_metrics::visit_scope("grep", || {
                    grep_scan(&paths, &query, case_sensitive, limit, offset)
                })
            })
            .await
            .unwrap_or_else(|_| GrepScan::default());
            Response::ready(version, serde_json::json!({ "grep": scan }))
        }
        Request::Search(args) => {
            if args.query.is_empty() {
                return Response::invalid("`query` must not be empty: it carries no term to rank");
            }
            let scope = args.scope();
            let repo = args.repo.clone();
            let limit = args
                .limit
                .unwrap_or(SEARCH_DEFAULT_LIMIT)
                .min(SEARCH_MAX_LIMIT);
            let offset = args.offset.unwrap_or(0);
            // The whole answer comes from the held index: no disk read and no
            // blocking task, unlike `grep`, whose bytes come from the working tree.
            let query = args.query;
            let min_should_match = args
                .min_should_match
                .unwrap_or_else(|| crate::search::default_min_should_match(&query));
            let exact = args.exact;
            enveloped(
                "search",
                handle.read(move |v| {
                    crate::verb_metrics::visit_scope("search", || {
                        search_view(
                            v,
                            &query,
                            exact,
                            min_should_match,
                            repo.as_deref(),
                            scope,
                            limit,
                            offset,
                        )
                    })
                }),
            )
        }
        Request::DirEntries { dir } => {
            let root = handle.root().to_path_buf();
            let abs = abs_arg(&root, &dir);
            enveloped("dir_entries", handle.read(move |v| children_view(v, &abs)))
        }
        Request::Frontmatter { path } => {
            let target = abs_arg(handle.root(), &path);
            enveloped(
                "frontmatter",
                handle.read(move |v| wire::file_frontmatter(v, &target)),
            )
        }
        Request::Content { path } => {
            // Content reads source text from disk — the sanctioned exception to
            // reads-over-held-state, the working tree is the materialised ref.
            // The hash rides with the content from ONE read of the bytes, so a
            // consumer that read a file always holds a guard-usable
            // `expected_hash`: the same FNV-1a `write_file`/`delete_file`
            // re-hash on disk, present whenever the file is readable, with no
            // dependence on catalog warmth. Deriving both from one buffer keeps
            // them coherent; a second read would open a TOCTOU window.
            // Capture readiness and version under the held lock, drop it, then
            // read off the lock and off the executor, so disk IO never blocks
            // either the held state or a runtime worker.
            // Resolve through the member map first: a path outside every member, or one a
            // symlink carries out of its member, is refused before any bytes are read.
            let root = handle.root().to_path_buf();
            let arg = path.clone();
            let target = match handle
                .read(move |v| resolve_read_target(v, &root, &arg))
                .value()
            {
                Some(Ok(target)) => target,
                Some(Err(message)) => return Response::invalid(message),
                None => return Response::not_ready(),
            };
            match handle.read(|_| ()) {
                Read::NotReady => Response::not_ready(),
                Read::Ready { version, .. } => {
                    let result = tokio::task::spawn_blocking(move || read_content_at(&target))
                        .await
                        .unwrap_or(None);
                    Response::ready(
                        version,
                        serde_json::json!({
                            "content": serde_json::to_value(result).expect("result serializes")
                        }),
                    )
                }
            }
        }
        Request::SemanticTokens { path } => {
            let target = abs_arg(handle.root(), &path);
            enveloped(
                "semantic_tokens",
                handle.read(|v| semantic_tokens_view(v, &target)),
            )
        }
        Request::ValidateValue {
            type_name,
            value,
            repo,
        } => enveloped(
            "validate_value",
            handle.read(move |v| {
                crate::value_validate::validate_value_verdicts(
                    v,
                    &type_name,
                    &value,
                    repo.as_deref(),
                )
            }),
        ),
        Request::TopLevelDirs(args) => {
            let scope = args.scope();
            let repo = args.repo;
            enveloped(
                "top_level_dirs",
                handle.read(move |v| top_level_dirs_view(v, repo.as_deref(), scope)),
            )
        }
        Request::Members(MembersArgs { repo }) => {
            let root = handle.root().to_path_buf();
            let cache_root = handle.package_cache_root().map(Path::to_path_buf);
            enveloped(
                "members",
                handle.read(move |v| {
                    // Same `repo` filter `overview.members` applies, so the two
                    // agree by construction and the mirror holds for this field.
                    wire::introspect_members(v, &root, cache_root.as_deref())
                        .members
                        .into_iter()
                        .filter(|m| repo.as_deref().is_none_or(|want| m.repo == want))
                        .collect::<Vec<_>>()
                }),
            )
        }
        Request::Overview(args) => {
            // The up-front orientation map: each sub-part is one call to its
            // own read with these same args, aggregated in one read closure so
            // the whole map is coherent at one version. That mirror property is
            // load-bearing, not bookkeeping — taking it seriously is what
            // caught that `diagnostic_counts` could not express repo scoping at
            // all and that `hubs` had no read whatsoever.
            let root = handle.root().to_path_buf();
            let cache_root = handle.package_cache_root().map(Path::to_path_buf);
            let scope = args.scope();
            let scope_str = scope.as_str();
            let repo = args.repo;
            let echo_repo = repo.clone();
            let diag_scope = DiagnosticsScope {
                repo: repo.clone(),
                scope,
                ..Default::default()
            };
            enveloped(
                "overview",
                handle.read(move |v| {
                    // One scope over the whole composed read: the hub ranking and
                    // the graph shape below report what they walked, and `overview`
                    // owns those numbers because it asked for them.
                    crate::verb_metrics::visit_scope("overview", || OverviewView {
                        repo: echo_repo.clone(),
                        scope: scope_str,
                        // `members` honors `repo` but NOT `scope`: it is the
                        // TOPOLOGY field, and `scope` filters CONTENT. Filtering it
                        // would hide the dependency set from the one field whose
                        // job is to report it, and `members.editable` is exactly
                        // how a consumer sees which repos `own` selected.
                        members: wire::introspect_members(v, &root, cache_root.as_deref())
                            .members
                            .into_iter()
                            .filter(|m| repo.as_deref().is_none_or(|want| m.repo == want))
                            .collect(),
                        top_level_dirs: top_level_dirs_view(v, repo.as_deref(), scope),
                        type_counts: wire::introspect_type_counts(v, repo.as_deref(), scope)
                            .unwrap_or_default(),
                        diagnostic_counts: diagnostic_counts_view(v, &diag_scope),
                        hubs: hub_ranking(v, repo.as_deref(), scope, HUB_LIMIT, 0),
                        graph_shape: graph_shape_view(v, repo.as_deref(), scope, false),
                    })
                }),
            )
        }
        Request::TypeClosure(args) => {
            let TypeClosureArgs { name, repo } = args;
            enveloped(
                "type_closure",
                handle.read(move |v| wire::introspect_type_closure(v, &name, repo.as_deref())),
            )
        }
        Request::Hubs(args) => {
            let scope = args.scope();
            let repo = args.repo;
            let limit = args.limit.unwrap_or(HUB_LIMIT);
            let offset = args.offset.unwrap_or(0);
            enveloped(
                "hubs",
                handle.read(move |v| {
                    crate::verb_metrics::visit_scope("hubs", || {
                        hub_ranking(v, repo.as_deref(), scope, limit, offset)
                    })
                }),
            )
        }
        Request::GraphShape(args) => {
            let scope = args.scope();
            let repo = args.repo;
            let orphan_paths = args.orphan_paths;
            enveloped(
                "graph_shape",
                handle.read(move |v| {
                    crate::verb_metrics::visit_scope("graph_shape", || {
                        graph_shape_view(v, repo.as_deref(), scope, orphan_paths)
                    })
                }),
            )
        }
        Request::GraphHealth(args) => {
            let scope = args.scope();
            let repo = args.repo;
            let limit = args
                .limit
                .unwrap_or(GRAPH_HEALTH_DEFAULT_LIMIT)
                .min(GRAPH_HEALTH_MAX_LIMIT);
            let want_verbs = args.verbs;
            let mut response = enveloped(
                "graph_health",
                handle.read(move |v| {
                    crate::verb_metrics::visit_scope("graph_health", || {
                        graph_health_view(v, repo.as_deref(), scope, limit)
                    })
                }),
            );
            // Opt-in instrumentation, a SIBLING of the payload: envelope metadata
            // ABOUT the read, never part of it, so `result.graph_health` keeps its
            // shape and a client that does not ask sees the same bytes as before.
            // Read off the process-global counters, so it needs no held state and
            // cannot disagree with a rebuild in flight.
            if want_verbs {
                if let Some(result) = response.result.as_mut().and_then(|r| r.as_object_mut()) {
                    result.insert(
                        "verbs".to_owned(),
                        serde_json::to_value(crate::verb_metrics::snapshot())
                            .expect("verb counters serialize"),
                    );
                }
            }
            response
        }
        Request::LinkGraph(args) => {
            let scope = args.scope();
            let repo = args.repo;
            enveloped(
                "link_graph",
                handle.read(move |v| link_graph_view(v, repo.as_deref(), scope)),
            )
        }
        Request::TypeGraph(args) => {
            let scope = args.scope();
            let repo = args.repo;
            let classes = EdgeClasses::from_arg(args.edges);
            enveloped(
                "type_graph",
                handle.read(move |v| type_graph_view(v, repo.as_deref(), scope, classes)),
            )
        }
        Request::Ignores { repo, resolve } => {
            // Capture the member set (name, root) under the held lock, then read
            // each `.auignore` (and, for `resolve`, walk) off the lock and off
            // the runtime workers — the same out-of-band, disk-reading shape
            // `content` uses, since scope config is not held state.
            let members = handle.read(move |v| {
                v.repos
                    .repos()
                    .iter()
                    // The builtin `au-engine` repo is a type source, not a
                    // workspace member (no real root), so it carries no ignores.
                    .filter(|r| !r.builtin)
                    .filter(|r| repo.as_deref().is_none_or(|want| r.name.as_str() == want))
                    .map(|r| (r.name.as_str().to_string(), r.root.clone()))
                    .collect::<Vec<_>>()
            });
            match members {
                Read::NotReady => Response::not_ready(),
                Read::Ready { version, value } => {
                    // A panic in the off-lock walk cannot answer; signal not-ready
                    // (the client's existing retry path) rather than let the
                    // JoinError panic the handler and sever this one connection.
                    match tokio::task::spawn_blocking(move || {
                        wire::ignores_view(&tb_parser::RealFileSystem, &value, resolve)
                    })
                    .await
                    {
                        Ok(view) => Response::ready(
                            version,
                            serde_json::to_value(view).expect("view serializes"),
                        ),
                        Err(_) => Response::not_ready(),
                    }
                }
            }
        }
        Request::ResolveMember { path } => {
            let target = abs_arg(handle.root(), &path);
            let cache_root = handle.package_cache_root().map(Path::to_path_buf);
            enveloped(
                "resolve_member",
                handle.read(move |v| wire::resolve_member(v, &target, cache_root.as_deref())),
            )
        }
        Request::DeviceConfig => {
            // The two device-global files sit OUTSIDE every knowledge base, so their
            // bytes are read out-of-band (disk, off the held lock and off the
            // runtime workers), the shape `content` / `ignores` use. Their paths
            // come from the engine's stated `ConfigSource`: a test tempdir
            // (`Dir`), the env default (`User`), or nothing at all (`Empty`).
            // The field-shape check then runs under the lock on the held
            // snapshot, where the builtin `au-engine` graph carries the defs.
            let config = handle.config().clone();
            let repos_path = crate::repo::user_registry_path(&config);
            let workspaces_path = crate::repo::user_workspaces_path(&config);
            let (rp, wp) = (repos_path.clone(), workspaces_path.clone());
            let bytes = tokio::task::spawn_blocking(move || {
                let read = |p: &Option<PathBuf>| p.as_deref().and_then(|p| std::fs::read(p).ok());
                (read(&rp), read(&wp))
            })
            .await
            // The read task only does `std::fs::read().ok()`, so it cannot panic;
            // degrade to no bytes rather than propagate a JoinError and sever the
            // connection.
            .unwrap_or((None, None));
            enveloped(
                "device_config",
                handle.read(move |v| {
                    wire::device_config_view(v, (repos_path, bytes.0), (workspaces_path, bytes.1))
                }),
            )
        }
        Request::Config(args) => config_response(handle, args).await,
    }
}

/// The `config` read: one consumer config file under the scoped channel. Machine
/// scope resolves the `.arsumbris` dir from the engine's `ConfigSource` and the
/// type in the served entry's graph; repo scope resolves under a declared member
/// root (explicit `root`, else the entry) and the type in that member's graph.
/// Path-safety and the reserved `au-engine` owner segment reject a bad segment
/// with an `error` response, an unknown repo-scope `root` likewise.
async fn config_response(handle: &EngineHandle, args: ConfigArgs) -> Response {
    // Path-safety is checked at the verb, independent of scope, so a malformed
    // segment rejects even when the scope has no resolvable base.
    if let Err(e) = crate::repo::check_config_segments(&args.consumer, &args.file) {
        return Response::invalid(e);
    }

    // Resolve the scope's `.arsumbris` dir and the graph the type resolves in,
    // under one lock. A machine scope with no device root yields `None` dir.
    enum Scope {
        Ready(Option<PathBuf>, String),
        BadRoot(String),
    }
    let base = handle.config().base();
    let want_root = args
        .root
        .as_deref()
        .map(|r| member_root_arg(handle.root(), r));
    let scope = args.scope;
    let resolved = handle.read(move |v| match scope {
        ConfigScope::Machine => {
            let entry = v
                .repos
                .entry()
                .map(|r| r.name.as_str().to_string())
                .unwrap_or_default();
            Scope::Ready(base, entry)
        }
        ConfigScope::Repo => {
            let member = match &want_root {
                Some(p) => v.repos.by_root(p),
                None => v.repos.entry(),
            };
            match member {
                Some(r) => Scope::Ready(
                    Some(crate::repo::identity_dir(&r.root)),
                    r.name.as_str().to_string(),
                ),
                None => Scope::BadRoot(format!(
                    "repo scope needs a declared member root; `{}` names none",
                    want_root
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| "<entry>".to_string())
                )),
            }
        }
    });
    let (dir, scope_repo) = match resolved {
        Read::NotReady => return Response::not_ready(),
        Read::Ready {
            value: Scope::BadRoot(msg),
            ..
        } => return Response::invalid(msg),
        Read::Ready {
            value: Scope::Ready(dir, repo),
            ..
        } => (dir, repo),
    };

    // Machine scope with no device root ($HOME unset): nowhere to author, a
    // null-path view rather than an error.
    let Some(dir) = dir else {
        return enveloped(
            "config",
            handle.read(|_v| wire::ConfigFileView {
                path: None,
                exists: false,
                content: None,
                diagnostics: Vec::new(),
            }),
        );
    };

    // Segments already checked, so this only rebuilds the path; a defensive
    // reject on the impossible error keeps the guarantee at the join site.
    let path = match crate::repo::config_path(&dir, &args.consumer, &args.file) {
        Ok(p) => p,
        Err(e) => return Response::invalid(e),
    };

    // The bytes are read off the runtime workers, off the held lock, then the
    // field-shape check runs under the lock on the held snapshot.
    let read_path = path.clone();
    let bytes = tokio::task::spawn_blocking(move || std::fs::read(&read_path).ok())
        .await
        .unwrap_or(None);
    let type_name = args.type_name;
    enveloped(
        "config",
        handle.read(move |v| wire::config_view(v, &path, bytes, &type_name, &scope_repo)),
    )
}

/// The `instances_of` read, with the optional per-match `instance` and `body`
/// facts.
///
/// Two phases, because `body` reads disk. The held read builds the base match
/// records (each already carrying `member`) and splices the `instance` fact when
/// asked, both pure held state under one lock. The `body` fact then reads disk
/// off the executor, so a per-match file read never blocks the held state or a
/// runtime worker — the same off-lock disk pattern the `content` read uses.
///
/// The `neighborhood` read, two-phase like [`instances_of_response`] so the
/// disk-reading enrichment never runs under the state lock.
///
/// Phase 1, under the lock: the walk, the DTO, and the in-memory `instance`
/// splice (`resolved_view`). Phase 2, off the executor: `content` / `body`, read
/// from the working tree once per unique file path; a block node slices its span
/// from that same read. The kinds validation already ran in the dispatch arm, so
/// `args` arrives validated.
async fn neighborhood_response(
    handle: &EngineHandle,
    seed: PathBuf,
    args: NeighborhoodArgs,
) -> Response {
    let want_content = args.content;
    let want_body = args.body;
    let want_instance = args.instance;
    let scope_own = args.scope().own_only();
    let kinds: Option<std::collections::BTreeSet<String>> =
        args.kinds.map(|ks| ks.into_iter().collect());
    let direction = args.direction.into();
    let depth = args.depth;
    let max_nodes = args.max_nodes.unwrap_or(NEIGHBORHOOD_MAX_NODES);

    // Phase 1: the held read. The walk, the DTO serialized to a mutable value,
    // the in-memory `instance` splice, and the per-node file path the disk phase
    // enriches (None for a block node).
    let read = handle.read(move |v| {
        // The walk is the work this read does, and this is the thread that does
        // it: one scope covers the traversal and the per-node projection.
        crate::verb_metrics::visit_scope("neighborhood", || {
            let params = crate::neighborhood::WalkParams {
                direction,
                depth,
                kinds: kinds.as_ref(),
                scope_own,
                max_nodes,
            };
            let walk = crate::neighborhood::walk(v, &seed, &params);
            let mut payload =
                serde_json::to_value(neighborhood_view(v, &walk)).expect("view serializes");

            // One enrichment site per node, in node order: the file path plus, for a
            // block node, its resolved span (None = the whole file). A block whose
            // id did not resolve has no span, so its slice is null.
            let sites: Vec<EnrichSite> = walk
                .nodes
                .iter()
                .map(|n| EnrichSite {
                    path: n.id.path.clone(),
                    span: n.block_span,
                    is_block: n.id.block_id.is_some(),
                })
                .collect();

            if want_instance {
                let nodes = payload["nodes"].as_array_mut().expect("nodes is an array");
                for (node, site) in nodes.iter_mut().zip(&sites) {
                    // A block node has no standalone resolved instance; only a file
                    // node carries one (null for a plain note).
                    node["instance"] = if site.is_block {
                        serde_json::Value::Null
                    } else {
                        serde_json::to_value(resolved_view(v, &site.path))
                            .expect("resolved serializes")
                    };
                }
            }
            (payload, sites)
        })
    });

    let (version, mut payload, sites) = match read {
        Read::NotReady => return Response::not_ready(),
        Read::Ready {
            version,
            value: (payload, sites),
        } => {
            // No disk facts requested: phase 1 is the whole answer.
            if !want_content && !want_body {
                return Response::ready(version, serde_json::json!({ "neighborhood": payload }));
            }
            (version, payload, sites)
        }
    };

    // Phase 2: read each unique file ONCE off the executor, deriving `content`,
    // `body`, and (for a path hosting a block) the bytes a span slices from.
    // Paths that host a block node, so their buffer is retained for slicing; a
    // pure-file-node path drops it after deriving content / body.
    let block_paths: std::collections::HashSet<PathBuf> = sites
        .iter()
        .filter(|s| s.span.is_some())
        .map(|s| s.path.clone())
        .collect();
    let unique: Vec<PathBuf> = {
        let mut seen = std::collections::HashSet::new();
        sites
            .iter()
            .filter(|s| seen.insert(s.path.clone()))
            .map(|s| s.path.clone())
            .collect()
    };
    let facts: std::collections::HashMap<PathBuf, FileFacts> =
        tokio::task::spawn_blocking(move || {
            unique
                .into_iter()
                .map(|p| {
                    let keep = block_paths.contains(&p);
                    let facts = read_neighborhood_file(&p, want_content, want_body, keep);
                    (p, facts)
                })
                .collect()
        })
        .await
        // The read task only does fallible IO; degrade to no facts rather than
        // sever the connection on a JoinError.
        .unwrap_or_default();

    let nodes = payload["nodes"].as_array_mut().expect("nodes is an array");
    for (node, site) in nodes.iter_mut().zip(&sites) {
        let file = facts.get(&site.path);
        let (content, body) = match site.span {
            // A block node: slice its span from the file bytes. A block is not a
            // file, so its content carries no hash / commit.
            Some(span) => {
                let slice = file
                    .and_then(|f| f.bytes.as_ref())
                    .and_then(|b| b.get(span.start..span.end))
                    .and_then(|s| std::str::from_utf8(s).ok());
                let content =
                    slice.map(|t| serde_json::json!({ "text": t, "hash": null, "commit": null }));
                (content, slice.map(str::to_string))
            }
            // A file node (or a block with no resolved span → null): the whole
            // file's content / body.
            None if !site.is_block => (
                file.and_then(|f| f.content.clone()),
                file.and_then(|f| f.body.clone()),
            ),
            None => (None, None),
        };
        if want_content {
            node["content"] = serde_json::to_value(content).expect("content serializes");
        }
        if want_body {
            node["body"] = serde_json::to_value(body).expect("body serializes");
        }
    }
    Response::ready(version, serde_json::json!({ "neighborhood": payload }))
}

/// One node's enrichment site: the file it lives in, and (for a block node) the
/// span to slice from that file. `span` is `None` for a file node (the whole
/// file) or a block whose id did not resolve.
struct EnrichSite {
    path: PathBuf,
    span: Option<tb_diagnostics::ByteRange>,
    is_block: bool,
}

/// The disk facts read once per unique file: the whole-file `content` value and
/// `body`, plus the raw bytes when a block slice is taken off them.
#[derive(Default)]
struct FileFacts {
    content: Option<serde_json::Value>,
    body: Option<String>,
    bytes: Option<Vec<u8>>,
}

/// Facts key by the match's FILE and are computed ONCE per unique path, then
/// spliced onto every record sharing it: a bare name matching several identities
/// in one file yields several records, and re-resolving or re-reading the file
/// per record would multiply the cost for no new information.
async fn instances_of_response(
    handle: &EngineHandle,
    type_name: String,
    origins: Option<Vec<wire::Origin>>,
    want_instance: bool,
    want_body: bool,
) -> Response {
    // Phase 1: the held read. Base records + `member` + the optional `instance`
    // fact, plus each record's file path for the body phase.
    let read = handle.read(move |v| {
        let matches = wire::introspect_instances_of(v, &type_name, origins.as_deref());
        let mut records: Vec<serde_json::Value> = matches
            .iter()
            .map(|m| serde_json::to_value(m).expect("match serializes"))
            .collect();

        if want_instance {
            let mut by_path: std::collections::HashMap<&str, serde_json::Value> =
                std::collections::HashMap::new();
            for m in &matches {
                by_path.entry(m.path.as_str()).or_insert_with(|| {
                    serde_json::to_value(resolved_view(v, Path::new(&m.path)))
                        .expect("resolved view serializes")
                });
            }
            for (rec, m) in records.iter_mut().zip(&matches) {
                rec["instance"] = by_path[m.path.as_str()].clone();
            }
        }

        let paths: Vec<String> = matches.into_iter().map(|m| m.path).collect();
        (records, paths)
    });

    let (version, mut records, paths) = match read {
        Read::NotReady => return Response::not_ready(),
        Read::Ready {
            version,
            value: (records, paths),
        } => {
            if !want_body {
                return Response::ready(version, serde_json::json!({ "instances_of": records }));
            }
            (version, records, paths)
        }
    };

    // Phase 2: the `body` fact, read from disk off the executor, once per unique
    // path.
    let unique: Vec<PathBuf> = {
        let mut seen = std::collections::HashSet::new();
        paths
            .iter()
            .filter(|p| seen.insert(p.as_str()))
            .map(PathBuf::from)
            .collect()
    };
    let bodies: std::collections::HashMap<PathBuf, Option<String>> =
        tokio::task::spawn_blocking(move || {
            unique
                .into_iter()
                .map(|p| {
                    let b = read_body_at(&p);
                    (p, b)
                })
                .collect()
        })
        .await
        // The read task only does fallible IO, never panics; degrade to no body
        // rather than sever the connection on a JoinError.
        .unwrap_or_default();
    for (rec, path) in records.iter_mut().zip(&paths) {
        // A file that could not be read (or an unterminated one) serializes
        // its body fact as null, never a dropped key.
        let b = bodies.get(Path::new(path)).cloned().flatten();
        rec["body"] = serde_json::to_value(b).expect("body serializes");
    }

    Response::ready(version, serde_json::json!({ "instances_of": records }))
}

/// Read a file's markdown BODY, the prose after its frontmatter. The `body`
/// half of the `content` read's whole-file text, split with the SAME
/// `split_frontmatter` the parser uses, so the two can never disagree on the
/// boundary.
///
/// `None` when the file is unreadable, not UTF-8, or opens an unterminated
/// frontmatter (a structural break the caller's own re-parse rejects). A file
/// with no frontmatter is all body.
fn read_body_at(target: &Path) -> Option<String> {
    let bytes = std::fs::read(target).ok()?;
    let text = String::from_utf8(bytes).ok()?;
    match tb_parser::split_frontmatter(&text) {
        Ok(Some(split)) => Some(split.body.to_string()),
        Ok(None) => Some(text),
        Err(_) => None,
    }
}

/// A wire `root` argument naming a member root, made absolute against the entry
/// and canonicalized, the form [`crate::repo::RepoMap::by_root`] matches. Runs the
/// filesystem canonicalize here, before a caller takes the held-state lock.
fn member_root_arg(entry: &Path, arg: &str) -> PathBuf {
    crate::repo::canonical_root(&abs_arg(entry, arg))
}

/// Resolve a request path argument against the root: absolute paths pass
/// through, relative paths join the knowledge base root. Catalog keys are absolute, so
/// either form a consumer sends lands on the same key.
fn abs_arg(root: &Path, arg: &str) -> PathBuf {
    let p = Path::new(arg);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        root.join(p)
    }
}

/// The file a `content`-family read may open: a path inside a declared member, resolved through
/// the same lexical rule writes use, plus a symlink check. A lexical path can stay inside the
/// member while a link on it points anywhere, and `std::fs::read` follows the link — so a
/// read-only client could fetch any file the daemon's user can open. The check is on the REAL
/// path, and only when the file exists: an absent path is answered as a null read by the caller.
fn resolve_read_target(kb: &KnowledgeBase, root: &Path, arg: &str) -> Result<PathBuf, String> {
    use std::path::Component;
    let joined = abs_arg(root, arg);
    let mut normalized = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::ParentDir => {
                if !normalized.pop() {
                    return Err(format!("path escapes the workspace: {arg}"));
                }
            }
            Component::CurDir => {}
            other => normalized.push(other),
        }
    }
    let Some(member) = kb.repos.repo_of(&normalized) else {
        return Err(format!("path is outside every workspace member: {arg}"));
    };
    if let Ok(real) = std::fs::canonicalize(&normalized) {
        let member_root =
            std::fs::canonicalize(&member.root).unwrap_or_else(|_| member.root.clone());
        if !real.starts_with(&member_root) {
            return Err(format!(
                "path leaves its workspace member through a symlink: {arg}"
            ));
        }
    }
    Ok(normalized)
}

/// Read a path's content and FNV-1a hash off the held lock, the shape the
/// `content` read returns. The hash rides with the content from one read of the
/// bytes, so a consumer always holds a guard-usable `expected_hash`.
fn read_content_at(target: &Path) -> Option<serde_json::Value> {
    let bytes = std::fs::read(target).ok()?;
    let hash = crate::mutate::hash_hex(crate::ir::ContentHash::of(&bytes));
    // The pin anchor for the returned bytes: HEAD of the repo owning the file,
    // null when the file is not under a git working tree. The bytes are the
    // working tree's (possibly dirty), so a consumer pinning `file*@<commit>`
    // anchors against HEAD, symmetric with a mutation's `result.commit`.
    // HEAD via a direct ref-file read (no per-open git subprocess), the shell
    // fallback covers any state the direct read cannot resolve.
    let commit = enclosing_dir(target).and_then(crate::gitwriter::head_commit_fast);
    let content = String::from_utf8(bytes).ok()?;
    // `text`, not `content`: the payload key is the read's own name, so a
    // `content` field inside it would make `result.content.content` the way to
    // the source string — the same silent-`undefined` trap one level down. The
    // envelope rule fixes a collision on the INSIDE. See `WIRE.md`.
    Some(serde_json::json!({ "text": content, "hash": hash, "commit": commit }))
}

/// Read one file ONCE and derive every requested `neighborhood` fact from the
/// single buffer: the whole-file `content` value, the `body`, and the raw bytes
/// a block node slices its span from. The `content` / `body` results match
/// [`read_content_at`] / [`read_body_at`] exactly, but a file feeding both plus
/// a block slice is read once rather than up to three times.
///
/// `keep_bytes` retains the buffer only for a path some block node lives in; a
/// pure-file-node path drops it.
fn read_neighborhood_file(
    target: &Path,
    want_content: bool,
    want_body: bool,
    keep_bytes: bool,
) -> FileFacts {
    let bytes = match std::fs::read(target) {
        Ok(b) => b,
        Err(_) => return FileFacts::default(),
    };
    // One UTF-8 view drives both text facts; a non-UTF-8 file yields neither,
    // matching the per-fact readers' `String::from_utf8(...).ok()?`.
    let text = std::str::from_utf8(&bytes).ok();
    let content = (want_content && text.is_some()).then(|| {
        let hash = crate::mutate::hash_hex(crate::ir::ContentHash::of(&bytes));
        let commit = enclosing_dir(target).and_then(crate::gitwriter::head_commit_fast);
        serde_json::json!({ "text": text.unwrap(), "hash": hash, "commit": commit })
    });
    let body = if want_body {
        // Same split as `read_body_at`: an unterminated frontmatter yields None.
        text.and_then(|t| match tb_parser::split_frontmatter(t) {
            Ok(Some(split)) => Some(split.body.to_string()),
            Ok(None) => Some(t.to_string()),
            Err(_) => None,
        })
    } else {
        None
    };
    FileFacts {
        content,
        body,
        bytes: keep_bytes.then_some(bytes),
    }
}

/// The directory a `git -C` query runs in, the file's parent.
fn enclosing_dir(target: &Path) -> Option<&Path> {
    target.parent()
}

/// Turn a [`Read`] into a response carrying the uniform result envelope:
/// `result` is an object holding the payload under the READ'S OWN NAME, so
/// `result[verb]` always reaches it. See `WIRE.md`, "The result envelope".
///
/// A nullable read wraps its null (`{ "type": null }`), so "not found" is
/// reached exactly like a hit — the shape never changes under the consumer.
///
/// `verb` is the wire name, which is why it is passed rather than derived: the
/// `Request` variant name and the wire name can differ, and the wire name is
/// the contract.
fn enveloped<T: Serialize>(verb: &'static str, read: Read<T>) -> Response {
    match read {
        Read::Ready { version, value } => Response::ready(
            version,
            serde_json::json!({ verb: serde_json::to_value(value).expect("result serializes") }),
        ),
        Read::NotReady => Response::not_ready(),
    }
}

/// Turn a [`Read`] of a serializable value into a response.
fn to_response<T: Serialize>(read: Read<T>) -> Response {
    match read {
        Read::Ready { version, value } => Response::ready(
            version,
            serde_json::to_value(value).expect("result serializes"),
        ),
        Read::NotReady => Response::not_ready(),
    }
}

fn resolved_view(kb: &KnowledgeBase, target: &Path) -> Option<ResolvedView> {
    let resolved = kb.instances.get(target)?;
    let Some(FileParse::Instance {
        instance: Some(inst),
        ..
    }) = kb.file_parse(target)
    else {
        return None;
    };

    // Authored form, so a `::repo` claim stays qualified, matching `instances_of`
    // / `instances`. It was served bare here.
    let claim = inst.type_claim.iter().map(|c| c.authored()).collect();
    // Owner-relative closure (WIRE §272), the same projection `instances` uses, so
    // a cross-repo ancestor keeps its `::repo` here too, rendered relative to the
    // instance's own graph. Empty when the claim did not resolve (no shape to fold).
    let resolution = kb
        .repos
        .repo_of(target)
        .and_then(|r| kb.resolution_graphs.of(&r.name));
    let own_graph = kb.graph_for_path(target);
    let closure = resolved
        .effective_shape
        .as_ref()
        .map(|s| wire::instance_closure_authored(own_graph, resolution, &inst.type_claim, s))
        .unwrap_or_default();
    let candidates = wire::candidates_for(kb, target)
        .iter()
        .map(|c| CandidateDto {
            type_name: c.type_name.0.clone(),
            inline_path: c.scope.inline_path.clone(),
        })
        .collect();
    // The full value layer, with provenance. Computed for any parsed instance;
    // an unresolved claim still carries frontmatter and body contributions.
    let layer = wire::instance_value_layer(kb, target);
    let (effective_values, section_presence, body_events) = match layer {
        Some(l) => (l.effective_values, l.section_presence, l.body_events),
        None => (Vec::new(), None, None),
    };
    let diagnostics = kb.diagnostics_for_file(target).cloned().collect();

    Some(ResolvedView {
        resolved: resolved.effective_shape.is_some(),
        claim,
        closure,
        candidates,
        effective_values,
        section_presence,
        body_events,
        record_block_ids: wire::record_block_ids_of(kb, target, inst, kb.line_index(target)),
        diagnostics,
    })
}

fn backlinks_view(kb: &KnowledgeBase, target: &Path) -> Vec<BacklinkDto> {
    let target_repo = kb.repos.repo_of(target).map(|r| r.name.clone());
    let inbound = kb.backlinks(target);
    // Instrumentation: the index answers this target directly, so the work is one
    // entry per inbound edge and nothing else — the read's whole point is that it
    // costs the answer, not the corpus.
    crate::verb_metrics::note_documents(inbound.len() as u64);
    inbound
        .iter()
        .map(|b| BacklinkDto {
            source: b.source.display().to_string(),
            // Carry the source repo only when it differs from the target's, the
            // cross-repo inbound edge.
            repo: kb
                .repos
                .repo_of(&b.source)
                .map(|r| r.name.clone())
                .filter(|src| Some(src) != target_repo.as_ref())
                .map(|src| src.as_str().to_string()),
            slot: b.slot.clone(),
            surface: match b.surface {
                RefSurface::Frontmatter => "frontmatter",
                RefSurface::Body => "body",
                RefSurface::Docstring => "docstring",
            }
            .to_string(),
            kind: crate::backlinks::coarse_edge_kind(b.surface, b.slot.as_deref()),
            span_start: b.span.start,
            span_end: b.span.end,
            // The span sits in the referencing source file, not the target.
            line_col: kb
                .line_index(&b.source)
                .map(|idx| idx.line_col_range(b.span)),
            block_id: b.block_id.clone(),
            source_block_id: b.source_block_id.clone(),
        })
        .collect()
}

/// Project a walked [`crate::neighborhood::Neighborhood`] into the wire DTO.
/// The walk is the analysis; this only renders. An edge span indexes its
/// `from` (referrer) file, so line/col comes from that file's index.
fn neighborhood_view(kb: &KnowledgeBase, n: &crate::neighborhood::Neighborhood) -> NeighborhoodDto {
    let node_ref = |id: &crate::neighborhood::NodeId| NodeRefDto {
        path: id.path.display().to_string(),
        block_id: id.block_id.clone(),
    };
    NeighborhoodDto {
        nodes: n
            .nodes
            .iter()
            .map(|w| NeighborhoodNodeDto {
                path: w.id.path.display().to_string(),
                block_id: w.id.block_id.clone(),
                depth: w.depth,
                repo: w.repo.as_ref().map(|r| r.as_str().to_string()),
                file_kind: w.file_kind,
                bytes: w.byte_len,
                body_bytes: w.body_bytes,
            })
            .collect(),
        edges: n
            .edges
            .iter()
            .map(|e| {
                // The rich fragments live on the parsed link, present only for
                // an outbound-discovered edge. An inbound edge carries none.
                let (repo, commit, anchor) = match &e.link {
                    Some(l) => (l.repo.clone(), l.commit.clone(), l.anchor.clone()),
                    None => (None, None, None),
                };
                NeighborhoodEdgeDto {
                    from: node_ref(&e.from),
                    to: e.to.as_ref().map(node_ref),
                    kind: e.kind,
                    surface: match e.surface {
                        crate::backlinks::RefSurface::Frontmatter => "frontmatter",
                        crate::backlinks::RefSurface::Body => "body",
                        crate::backlinks::RefSurface::Docstring => "docstring",
                    },
                    span_start: e.span.start,
                    span_end: e.span.end,
                    line_col: kb
                        .line_index(&e.from.path)
                        .map(|idx| idx.line_col_range(e.span)),
                    field: e.field.clone(),
                    source_block_id: e.source_block_id.clone(),
                    block_id: e.target_block_id.clone(),
                    repo,
                    commit,
                    anchor,
                }
            })
            .collect(),
        truncated: n.truncated,
        truncated_at_depth: n.truncated_at_depth,
        dropped: n
            .dropped
            .iter()
            .map(|d| DroppedNodeDto {
                path: d.id.path.display().to_string(),
                block_id: d.id.block_id.clone(),
                depth: d.depth,
                repo: d.repo.as_ref().map(|r| r.as_str().to_string()),
            })
            .collect(),
    }
}

/// EVERY outgoing wikilink edge of a file, frontmatter and body, each
/// classified by kind. Frontmatter edges first (declaration order), then body
/// edges in source order.
///
/// One read answers "what does this connect to?" completely. Before this it was
/// scattered: body-only here, typed frontmatter references only via the
/// `instance` read, and a `[[...]]` inside a frontmatter string nowhere in the
/// forward direction at all.
///
/// The traversal is SHARED with the backlink index
/// ([`crate::backlinks::walk_edges`]), so the two directions cannot disagree
/// about which values are scanned or how a target resolves. This read adds only
/// the classification, which needs the effective shape the walk deliberately
/// does not see.
fn outgoing_view(kb: &KnowledgeBase, target: &Path) -> Vec<OutgoingRefDto> {
    let Some(parse) = kb.file_parse(target) else {
        return Vec::new();
    };
    // The slot gate, three-way. A plain Note is DEFINITIVELY untyped (no `type:`
    // claim at all), so no slot can admit a reference, which is knowledge, not
    // uncertainty. A typed instance carries a shape, unless its claim did not
    // resolve or its repo's vocabulary aborted (`build.rs` skips those when
    // populating the resolved layer) — only then is the slot genuinely unknown.
    let gate = match parse {
        FileParse::Note { .. } => SlotGate::Untyped,
        _ => match kb
            .instances
            .get(target)
            .and_then(|r| r.effective_shape.as_ref())
        {
            Some(shape) => SlotGate::Shape(shape),
            None => SlotGate::Unknown,
        },
    };

    crate::backlinks::walk_edges(target, parse, &kb.indexes, &kb.repos, &kb.workspaces)
        .into_iter()
        .map(|e| {
            let kind = classify_edge(&e, gate);
            let w = e.link;
            OutgoingRefDto {
                span: wire::SpanRange::new(e.span.start, e.span.end).located(kb.line_index(target)),
                target: w.target,
                resolved: e.resolved.map(|p| p.display().to_string()),
                repo: w.repo,
                commit: w.commit,
                anchor: w.anchor,
                block_id: w.block_id,
                field: e.slot,
                source_block_id: e.source_block_id,
                surface: match e.surface {
                    crate::backlinks::RefSurface::Frontmatter => "frontmatter",
                    crate::backlinks::RefSurface::Body => "body",
                    crate::backlinks::RefSurface::Docstring => "docstring",
                },
                kind,
            }
        })
        .collect()
}

/// The reverse-by-target inert-pin fold: every pin naming `target_name` whose
/// source file contains an instance of `source_type`. See [`PinRecordDto`] and
/// [[spec - pinned references - a recorded resolved edge with an immutable past
/// and an on-demand forward trace]].
///
/// Scope, then fold. `source_type` scopes the candidate FILE set via
/// `wire::instance_files_of` — the files containing an instance of that type (a
/// file-level claim or a nested inline record), already deduped to one entry per
/// file. It shares the identity rule with `instances_of` but skips the match
/// projection a path-only caller would discard. For each such file, `walk_edges`
/// yields every outbound reference — the same walk `references_out` uses, so
/// nested-record pins carry their `source_block_id` — and this keeps the
/// commit-bearing ones naming the target.
///
/// No time awareness and no live resolution: the match is on the pin's RECORDED
/// target string, so a since-reused name returns every pin naming it. The
/// consumer windows by rename time to exclude a since-reused name.
fn pins_view(kb: &KnowledgeBase, target_name: &str, source_type: &str) -> Vec<PinRecordDto> {
    let mut out = Vec::new();
    for source in wire::instance_files_of(kb, source_type) {
        let Some(parse) = kb.file_parse(&source) else {
            continue;
        };
        for e in
            crate::backlinks::walk_edges(&source, parse, &kb.indexes, &kb.repos, &kb.workspaces)
        {
            // Only inert pins, and only those naming the requested target. An
            // empty-target commit-referent (`[[::@sha]]`) names no file, so it
            // never matches a named target and is excluded here for free.
            if !e.link.is_inert_pin() || e.link.target != target_name {
                continue;
            }
            let w = e.link;
            out.push(PinRecordDto {
                source: source.display().to_string(),
                source_block_id: e.source_block_id,
                span: wire::SpanRange::new(e.span.start, e.span.end)
                    .located(kb.line_index(&source)),
                slot: e.slot,
                surface: match e.surface {
                    crate::backlinks::RefSurface::Frontmatter => "frontmatter",
                    crate::backlinks::RefSurface::Body => "body",
                    crate::backlinks::RefSurface::Docstring => "docstring",
                },
                target: w.target,
                repo: w.repo,
                // A pin is commit-bearing by definition (`is_inert_pin`), so the
                // commit is present; default only defends against an impossible
                // shape rather than panicking on the socket path.
                commit: w.commit.unwrap_or_default(),
                block_id: w.block_id,
            });
        }
    }
    out
}

/// The slot-classification gate for a source file's frontmatter edges. Three
/// states, because "no shape" has three distinct causes and only one is
/// genuinely `unknown`.
#[derive(Clone, Copy)]
enum SlotGate<'a> {
    /// A typed instance with a resolved effective shape: consult the slot.
    Shape(&'a tb_core::EffectiveShape),
    /// A plain note, no `type:` claim at all. DEFINITIVELY untyped, so no slot
    /// can admit a reference — a whole-value frontmatter link is an untyped
    /// pointer, not an "we could not tell". Knowledge, not uncertainty.
    Untyped,
    /// A typed instance whose shape is unavailable: its `type:` claim did not
    /// resolve, or its repo's vocabulary aborted. The slot is genuinely
    /// undecidable, so a whole-value frontmatter link is `unknown`.
    Unknown,
}

/// Which KIND of edge this is. See [`OutgoingRefDto::kind`] for what each means.
///
/// Only the frontmatter pair needs the slot. A body edge is settled by the
/// link's own syntax, and an EMBEDDED frontmatter link is settled by
/// [[type reference::au-type-system]] at the value level, so neither can be `unknown`.
fn classify_edge(edge: &crate::backlinks::SourceEdge, gate: SlotGate) -> &'static str {
    // A commit-referent (`[[::@sha]]` / `[[::repo@sha]]`) names a COMMIT, not a
    // file. It resolves to nothing but is NOT dangling, so it carries its own
    // kind, and a consumer reading `resolved: null` does not misread it as a
    // broken link. Settled by the link's own syntax, surface-independent.
    if edge.link.is_commit_referent() {
        return "commit-referent";
    }
    if edge.surface == crate::backlinks::RefSurface::Body {
        return if edge.link.field.is_some() {
            "contributing"
        } else {
            "navigational"
        };
    }
    // A link inside a longer string is part of that string, never a reference,
    // whatever the slot admits. Decidable with no type information.
    if !edge.whole_value {
        return "field-string-wikilink";
    }
    // A whole-value frontmatter link. Only a resolved shape can promote it to a
    // `field-reference`; a definitively-untyped note demotes it to an untyped
    // pointer; a genuinely-undecidable slot is `unknown`.
    let shape = match gate {
        SlotGate::Shape(shape) => shape,
        SlotGate::Untyped => return "field-string-wikilink",
        SlotGate::Unknown => return "unknown",
    };
    let Some(key) = edge.slot.as_deref() else {
        return "unknown";
    };
    match shape.get(&tb_core::FieldName(key.to_string())) {
        Some(origin)
            if origin
                .canonical_decl()
                .parsed_shape
                .as_ref()
                .is_ok_and(tb_grammar::slot_admits_reference) =>
        {
            "field-reference"
        }
        // In the shape but absent from it: an EXTRA field, which has no slot at
        // all. That is knowledge, not uncertainty, so it is not `unknown`.
        None => "field-string-wikilink",
        // A resolved slot the shape does not admit a reference from.
        Some(_) => "field-string-wikilink",
    }
}

/// A file's typed highlight tokens, an ordered `(range, kind)` stream.
/// `None` when the path is not a file the build holds.
///
/// Two layers compose. The body-surface kinds (wikilink, block-id marker,
/// anchor) come from the markdown body, so they serve notes and markdown
/// instances. The value-layer kinds (field-value, type-claim, wikilink-
/// valued frontmatter, `^:` record block-ids) come from a parsed instance,
/// so they serve typed instances including pure-YAML ones with no body.
/// Tokens are sorted by `range.start`.
fn semantic_tokens_view(kb: &KnowledgeBase, target: &Path) -> Option<Vec<SemanticToken>> {
    let parse = kb.file_parse(target)?;
    let mut tokens = Vec::new();

    // A type-def file emits the shape layer.
    if let FileParse::TypeDef {
        type_def: Some(def),
        shape_leaves,
        ..
    } = parse
    {
        let lines = kb.line_index(target);
        type_def_tokens(kb, target, def, shape_leaves, lines, &mut tokens);
        tokens.sort_by_key(|t| t.range.start);
        return Some(tokens);
    }

    let lines = kb.line_index(target);
    let inst = match parse {
        FileParse::Instance {
            instance: Some(inst),
            ..
        } => Some(inst),
        _ => None,
    };

    if let Some((body, body_offset)) = parse.markdown_body() {
        body_tokens(kb, target, inst, body, body_offset, lines, &mut tokens);
    }
    if let Some(inst) = inst {
        frontmatter_tokens(kb, target, inst, lines, &mut tokens);
    }

    tokens.sort_by_key(|t| t.range.start);
    Some(tokens)
}

/// A claim / parent / meta type-name for a semantic token, carrying its `::repo`
/// qualifier so a consumer can tell a peer type from an own one. Parity with the
/// other wire projections (`InstanceMatch.claim`, meta / body-use targets), which
/// serve the authored qualified form.
fn qualified_token_name(base: &tb_core::TypeName, repo: Option<&str>) -> String {
    match repo {
        Some(r) => format!("{}::{}", base.as_str(), r),
        None => base.as_str().to_string(),
    }
}

/// A `type-claim` token for one authored claim in `source`: its authored name
/// plus the identity it resolves to.
///
/// The claim resolves in `source`'s repo, through
/// [`crate::resolution_build::resolve_claim_tid`], the per-claim rule validation
/// and `instance_counts` seed from, so the token's `(name, hash)` joins onto
/// every identity-keyed read. Every token-bearing position (a frontmatter,
/// inline-record, or typed-fence claim, a parent, a meta `type:`) seeds its
/// `::repo` form into that repo's fold, so the one scope covers them all.
///
/// The owner is the repo the claim names: its `::repo` qualifier, else the
/// source's own repo. Resolution keys on that authored form, a bare name
/// resolves only to the own def and `name::repo` only to that peer's, so a
/// resolved claim reached exactly that repo. Never the fold node's stored
/// `origin`: an in-sync identity own and peer both define folds to ONE node
/// whose origin is processing-order-dependent (see `ResolvedNode::authored`).
fn type_claim_kind(
    kb: &KnowledgeBase,
    source: &Path,
    base: &tb_core::TypeName,
    qualifier: Option<&str>,
) -> SemanticTokenKind {
    let repo = kb.repos.repo_of(source).map(|r| r.name.clone());
    let own = kb.graph_for_path(source);
    let rg = repo.as_ref().and_then(|r| kb.resolution_graphs.of(r));
    let owner = qualifier.or(repo.as_ref().map(|r| r.as_str()));
    let resolved = crate::resolution_build::resolve_claim_tid(own, rg, base, qualifier)
        .zip(owner)
        .map(|(tid, owner)| wire::ClosureIdentity {
            name: tid.name.as_str().to_string(),
            repo: owner.to_string(),
            hash: format!("{:016x}", tid.hash.0),
        });
    SemanticTokenKind::TypeClaim {
        name: qualified_token_name(base, qualifier),
        resolved,
    }
}

/// The type-def shape layer: `type-claim` for parent and `meta:` claims,
/// `type-ref` for sealed branches, and per field a `field-shape` container
/// (carrying the parsed `WireShape`) enclosing `type-ref` / `shape-builtin` /
/// `enum-member` leaves. `lines` is the file's index.
fn type_def_tokens(
    kb: &KnowledgeBase,
    target: &Path,
    def: &tb_core::TypeDef,
    shape_leaves: &[ShapeLeafSpan],
    lines: Option<&tb_diagnostics::LineIndex>,
    out: &mut Vec<SemanticToken>,
) {
    let emit = |out: &mut Vec<SemanticToken>, span: tb_diagnostics::ByteRange, kind| {
        out.push(SemanticToken {
            range: wire::SpanRange::new(span.start, span.end).located(lines),
            kind,
        });
    };

    // Parent claims and meta-block `type:` are type-claims, like an instance's.
    for parent in &def.parents {
        emit(
            out,
            parent.span,
            type_claim_kind(kb, target, &parent.name, parent.repo.as_deref()),
        );
    }
    if let Some(blocks) = &def.meta_blocks {
        for block in blocks {
            emit(
                out,
                block.type_name_span,
                type_claim_kind(kb, target, &block.type_name, block.repo.as_deref()),
            );
        }
    }

    // Sealed branches are type-def name references.
    for branch in &def.sealed {
        emit(
            out,
            branch.span,
            SemanticTokenKind::TypeRef {
                name: branch.name.as_str().to_string(),
            },
        );
    }

    // Each field: a field-shape container carrying the parsed shape.
    for field in &def.fields {
        let value_type = match &field.parsed_shape {
            Ok(shape) => Some(wire::WireShape::from(shape)),
            Err(_) => None,
        };
        emit(
            out,
            field.shape_span,
            SemanticTokenKind::FieldShape {
                field: field.name.as_str().to_string(),
                value_type,
            },
        );
    }

    // The decomposed leaf name / keyword / literal tokens. Their source spans
    // are computed once in the parse layer, where the file's bytes were in hand,
    // so a QUOTED shape (`"note*"`) carries leaves like an unquoted one rather
    // than being dropped for want of an offset mapping at read time.
    for leaf in shape_leaves {
        let kind = match leaf.role {
            ShapeSpanRole::TypeName => SemanticTokenKind::TypeRef {
                name: leaf.text.clone(),
            },
            ShapeSpanRole::Builtin => SemanticTokenKind::ShapeBuiltin {
                name: leaf.text.clone(),
            },
            ShapeSpanRole::EnumMember => SemanticTokenKind::EnumMember {
                value: leaf.text.clone(),
            },
        };
        emit(out, leaf.range, kind);
    }
}

/// The markdown-body kinds: wikilinks (resolved / broken), bare `^id`
/// block-id markers, headings (anchors), and fenced blocks. A fence's
/// trailing `^id` is a block-id; a typed `[:field]` fence on a typed
/// instance also yields a `typed-block` container plus its inner fields.
fn body_tokens(
    kb: &KnowledgeBase,
    target: &Path,
    inst: Option<&tb_core::Instance>,
    body: &str,
    body_offset: usize,
    lines: Option<&tb_diagnostics::LineIndex>,
    out: &mut Vec<SemanticToken>,
) {
    for ev in scan_body(body) {
        match ev {
            BodyEvent::Wikilink { raw, span } => {
                let Ok(w) = parse_wikilink_inner(raw) else {
                    continue;
                };
                out.push(SemanticToken {
                    range: span_in_body(span, body_offset, lines),
                    kind: wikilink_kind(kb, target, &w),
                });
            }
            BodyEvent::BlockIdMarker { id, span } => out.push(SemanticToken {
                range: span_in_body(span, body_offset, lines),
                kind: SemanticTokenKind::BlockId { id: id.to_string() },
            }),
            BodyEvent::Heading { text, span, .. } => out.push(SemanticToken {
                range: span_in_body(span, body_offset, lines),
                kind: SemanticTokenKind::Anchor {
                    text: text.to_string(),
                },
            }),
            BodyEvent::FencedBlock {
                info,
                body: fence_body,
                span,
                trailing_block_id,
            } => fenced_block_tokens(
                kb,
                target,
                inst,
                body,
                body_offset,
                info,
                fence_body,
                span,
                trailing_block_id,
                lines,
                out,
            ),
            _ => {}
        }
    }
}

/// A fenced block's tokens. The trailing `^id` is a block-id on any fence. A
/// typed `[:field]` fence on a typed instance also emits a `typed-block`
/// container, then its inner fields typed against the fence field's element
/// type, full parity with frontmatter.
#[allow(clippy::too_many_arguments)]
fn fenced_block_tokens(
    kb: &KnowledgeBase,
    target: &Path,
    inst: Option<&tb_core::Instance>,
    body: &str,
    body_offset: usize,
    info: &str,
    fence_body: &str,
    span: tb_diagnostics::ByteRange,
    trailing_block_id: Option<&str>,
    lines: Option<&tb_diagnostics::LineIndex>,
    out: &mut Vec<SemanticToken>,
) {
    if let Some(id) = trailing_block_id {
        // The id text sits right after the `^`; widen the span by one to
        // cover the marker.
        let at = offset_in(body, id);
        let marker = tb_diagnostics::ByteRange::new(at - 1, at + id.len());
        out.push(SemanticToken {
            range: span_in_body(marker, body_offset, lines),
            kind: SemanticTokenKind::BlockId { id: id.to_string() },
        });
    }

    // The typed-block container and its inner fields are value-layer, so a
    // typed instance only. A note's fence yields no typed-block.
    let (Some(field), Some(_inst)) = (extract_field_marker(info), inst) else {
        return;
    };
    out.push(SemanticToken {
        range: span_in_body(span, body_offset, lines),
        kind: SemanticTokenKind::TypedBlock {
            field: field.to_string(),
        },
    });

    // Parse the fence body into an inline record with file-absolute spans,
    // then type its fields against the fence field's element type, reusing
    // the value-layer walk. The fence body's file offset is its position in
    // the markdown body plus the body's own offset.
    let fence_offset = body_offset + offset_in(body, fence_body);
    let Some((inline, _)) = tb_core::parse_block_record(target, fence_body, fence_offset) else {
        return;
    };
    let element_shape = kb
        .instances
        .get(target)
        .and_then(|r| r.effective_shape.as_ref())
        .and_then(|es| field_shape(es, field))
        .map(|shape| match shape {
            Shape::List { inner, .. } => inner.as_ref(),
            other => other,
        });
    let value = tb_core::InstanceValue::Mapping(inline);
    // A mapping carries no links of its own; its fields carry theirs.
    walk_value_tokens(
        kb,
        target,
        kb.graph_for_path(target),
        &value,
        span,
        &[],
        field,
        element_shape,
        lines,
        out,
    );
}

/// The byte offset of a sub-slice within its parent string. Both must share
/// the same backing buffer, as `scan_body`'s slices do.
fn offset_in(parent: &str, child: &str) -> usize {
    child.as_ptr() as usize - parent.as_ptr() as usize
}

/// The frontmatter value-layer kinds: the `type:` claim, each scalar value
/// (typed via the field's resolved shape, descended to the leaf), wikilink-
/// valued fields, and `^:` inline-record block-ids.
fn frontmatter_tokens(
    kb: &KnowledgeBase,
    target: &Path,
    inst: &tb_core::Instance,
    lines: Option<&tb_diagnostics::LineIndex>,
    out: &mut Vec<SemanticToken>,
) {
    for claim in inst.type_claim.iter() {
        out.push(SemanticToken {
            range: located(claim.span, lines),
            kind: type_claim_kind(kb, target, &claim.name, claim.repo.as_deref()),
        });
    }

    let graph = kb.graph_for_path(target);
    let shape_of = kb
        .instances
        .get(target)
        .and_then(|r| r.effective_shape.as_ref());
    for field in &inst.fields {
        let shape = shape_of.and_then(|es| field_shape(es, &field.key));
        walk_value_tokens(
            kb,
            target,
            graph,
            &field.value,
            field.value_span,
            &field.nav_links,
            &field.key,
            shape,
            lines,
            out,
        );
    }

    // Block-id highlight tokens use only the record's SPAN, which is
    // shape-independent, so the own-graph walk suffices (no owner-relative context).
    for (id, record) in tb_core::collect_record_targets(graph, None, None, "", inst) {
        out.push(SemanticToken {
            range: located(record.span, lines),
            kind: SemanticTokenKind::BlockId { id },
        });
    }
}

/// Walk one frontmatter value against its shape, emitting a token per scalar
/// leaf. A wikilink-valued string is a wikilink token, not a field-value
/// token. A sequence descends with the list's element shape; a nested inline
/// record recurses (its field types are not resolved in this pass, so its
/// leaves carry a null `value_type`) and contributes its own type claim.
#[allow(clippy::too_many_arguments)]
fn walk_value_tokens(
    kb: &KnowledgeBase,
    source: &Path,
    graph: &tb_core::TypeGraph,
    value: &tb_core::InstanceValue,
    span: tb_diagnostics::ByteRange,
    nav_links: &[tb_core::NavLink],
    field: &str,
    shape: Option<&Shape>,
    lines: Option<&tb_diagnostics::LineIndex>,
    out: &mut Vec<SemanticToken>,
) {
    use tb_core::InstanceValue as V;
    match value {
        V::String(s) => {
            if let Ok(w) = parse_wikilink(s) {
                // The whole value is one wikilink: a single wikilink token
                // over the value span, the validated-or-navigational case.
                out.push(SemanticToken {
                    range: located(span, lines),
                    kind: wikilink_kind(kb, source, &w),
                });
            } else if nav_links.is_empty() {
                out.push(field_value_token(span, field, shape, "String", lines));
            } else {
                // A string with embedded links: a wikilink token per link, a
                // field-value token for each surrounding text run.
                emit_mixed_string_tokens(kb, source, span, nav_links, field, shape, lines, out);
            }
        }
        V::Integer(_) | V::Float(_) => {
            out.push(field_value_token(span, field, shape, "Number", lines));
        }
        V::Boolean(_) => {
            out.push(field_value_token(span, field, shape, "Boolean", lines));
        }
        V::Null | V::NotYetSupported => {}
        V::Sequence(elems) => {
            let inner = match shape {
                Some(Shape::List { inner, .. }) => Some(inner.as_ref()),
                _ => None,
            };
            for e in elems {
                walk_value_tokens(
                    kb,
                    source,
                    graph,
                    &e.value,
                    e.span,
                    &e.nav_links,
                    field,
                    inner,
                    lines,
                    out,
                );
            }
        }
        V::Mapping(inline) => {
            for claim in inline.type_claim.iter().flat_map(|tc| tc.iter()) {
                out.push(SemanticToken {
                    range: located(claim.span, lines),
                    kind: type_claim_kind(kb, source, &claim.name, claim.repo.as_deref()),
                });
            }
            // The record's type, from its explicit claim or the type the slot
            // pins. Either is local: the slot shape or the inline `type:`
            // names the type-def, so its field shapes are a graph lookup, no
            // candidate scan. A record with neither stays untyped (null), the
            // signal for a consumer to run the candidate scan itself.
            let record_shape = record_effective_shape(graph, inline, shape);
            for nested in &inline.fields {
                let nested_slot = record_shape
                    .as_ref()
                    .and_then(|s| field_shape(s, &nested.key));
                walk_value_tokens(
                    kb,
                    source,
                    graph,
                    &nested.value,
                    nested.value_span,
                    &nested.nav_links,
                    &nested.key,
                    nested_slot,
                    lines,
                    out,
                );
            }
        }
    }
}

/// A `field-value` token over a scalar. The type is the declared shape when
/// the engine knows it; otherwise it is inferred from the YAML literal
/// (`literal`, a primitive name) so an extra or untyped-record field still
/// colors by type.
fn field_value_token(
    span: tb_diagnostics::ByteRange,
    field: &str,
    shape: Option<&Shape>,
    literal: &'static str,
    lines: Option<&tb_diagnostics::LineIndex>,
) -> SemanticToken {
    let value_type = match shape {
        Some(s) => wire::WireShape::from(s),
        None => wire::WireShape::Primitive { name: literal },
    };
    SemanticToken {
        range: located(span, lines),
        kind: SemanticTokenKind::FieldValue {
            field: field.to_string(),
            value_type,
        },
    }
}

/// Tokenize a string value that carries embedded navigational links: a
/// wikilink token per link, a `field-value` token over each surrounding text
/// run. Links are parse-ordered and non-overlapping, so a single left-to-
/// right sweep over the value span covers it without gaps or overlaps.
// The token context (`kb`, `source`, `field`, `shape`, `lines`) is the caller's
// already; a struct would only restate that context at the one call site.
#[allow(clippy::too_many_arguments)]
fn emit_mixed_string_tokens(
    kb: &KnowledgeBase,
    source: &Path,
    span: tb_diagnostics::ByteRange,
    nav_links: &[tb_core::NavLink],
    field: &str,
    shape: Option<&Shape>,
    lines: Option<&tb_diagnostics::LineIndex>,
    out: &mut Vec<SemanticToken>,
) {
    let mut cursor = span.start;
    for nl in nav_links {
        if nl.span.start > cursor {
            // The text run before this link.
            let gap = tb_diagnostics::ByteRange::new(cursor, nl.span.start);
            out.push(field_value_token(gap, field, shape, "String", lines));
        }
        let kind = match parse_wikilink_inner(&nl.raw) {
            Ok(w) => wikilink_kind(kb, source, &w),
            Err(_) => {
                cursor = nl.span.end;
                continue;
            }
        };
        out.push(SemanticToken {
            range: located(nl.span, lines),
            kind,
        });
        cursor = nl.span.end;
    }
    if cursor < span.end {
        let tail = tb_diagnostics::ByteRange::new(cursor, span.end);
        out.push(field_value_token(tail, field, shape, "String", lines));
    }
}

/// The effective shape of an inline record, for typing its nested fields.
/// The record's type is its explicit `type:` claim, else the single type a
/// `name` / `name&` slot pins. `None` when neither names a type (a
/// compound / sealed slot without an explicit claim, already an
/// `inline-value-missing-type` diagnostic) — that leaves the nested leaves
/// untyped, the consumer's hook to run a candidate scan.
fn record_effective_shape(
    graph: &tb_core::TypeGraph,
    inline: &tb_core::InlineValue,
    slot: Option<&Shape>,
) -> Option<tb_core::EffectiveShape> {
    let names: Vec<tb_core::TypeName> = if let Some(claim) = &inline.type_claim {
        claim.iter().map(|c| c.name.clone()).collect()
    } else {
        vec![tb_core::TypeName(slot.and_then(pinned_name)?.to_string())]
    };
    tb_core::effective_shape(graph, &synth_claim(&names)).ok()
}

/// The single type-def name a `name` / `name&` slot pins onto a claim-less
/// inline record. Compound / primitive / list slots pin nothing.
fn pinned_name(shape: &Shape) -> Option<&str> {
    match shape {
        Shape::Record(name) | Shape::InlineOrReference(name) => Some(name.as_str()),
        _ => None,
    }
}

/// A zero-span `TypeClaim` over resolved names, the input to `effective_shape`.
fn synth_claim(names: &[tb_core::TypeName]) -> tb_core::TypeClaim {
    let items: Vec<tb_core::TypeNameClaim> = names
        .iter()
        .map(|n| tb_core::TypeNameClaim::own(n.clone(), tb_diagnostics::ByteRange::new(0, 0)))
        .collect();
    match &items[..] {
        [one] => tb_core::TypeClaim::Bare(one.clone()),
        _ => tb_core::TypeClaim::List {
            items,
            value_span: tb_diagnostics::ByteRange::new(0, 0),
        },
    }
}

/// A field's canonical declared shape within an effective shape, by bare key.
fn field_shape<'a>(shape: &'a tb_core::EffectiveShape, key: &str) -> Option<&'a Shape> {
    shape
        .get(&tb_core::FieldName(key.to_string()))
        .and_then(|origin| origin.canonical_decl().parsed_shape.as_ref().ok())
}

/// Resolve a parsed wikilink to its target file, scoped to the `origin` file
/// the link appears in. A `::repo`-qualified link resolves cross-repo into the
/// named repo; an unqualified link resolves repo-local against `origin`'s own
/// index. The single resolution path every reference read shares, so
/// `references_out`, `semantic_tokens`, and the navigation reads agree on what
/// a link points at.
fn resolve_wikilink(
    kb: &KnowledgeBase,
    origin: &Path,
    target: &str,
    repo: Option<&str>,
) -> Option<PathBuf> {
    match repo {
        Some(repo_q) => match crate::crossref::resolve_cross_repo(
            &kb.repos,
            &kb.indexes,
            &kb.workspaces,
            origin,
            repo_q,
            target,
        ) {
            crate::crossref::CrossRepoRef::Resolved(p) => Some(p),
            _ => None,
        },
        None => kb.index_for_path(origin).resolve(target).ok(),
    }
}

/// Resolve a wikilink to a resolved-or-broken token kind. An unqualified link
/// resolves against the source file's repo index (repo-local); a `::repo` link
/// resolves into the named repo, the same cross-repo resolution validation and
/// `references_out` use.
fn wikilink_kind(
    kb: &KnowledgeBase,
    source: &Path,
    w: &tb_references::WikilinkRef,
) -> SemanticTokenKind {
    // An inert pin (any `@sha`: a named pin `[[file::@sha]]` or the empty-target
    // commit-referent `[[::@sha]]`) is a coordinate into an immutable past — never
    // re-resolved live, never dangling. So it is its own kind, neither resolved nor
    // broken, over the same `is_inert_pin` predicate the backlink index and navigation
    // reads use (all treat a pin as forming no live edge). NOTE this folds every inert
    // pin into one kind, unlike `references_out`, which reserves the `commit-referent`
    // kind for the empty-target case and leaves a named pin its ordinary kind.
    // Checked first: `is_inert_pin` and `is_local` are exclusive (a pin has a commit,
    // a local form has none), but the pin case owns any `@sha`-carrying link.
    if w.is_inert_pin() {
        return SemanticTokenKind::WikilinkPinned {
            target: w.target.clone(),
            repo: w.repo.clone(),
            commit: w.commit.clone().unwrap_or_default(),
        };
    }
    // The LOCAL form (`[[^id]]` / `[[#head]]` / `[[^^id]]`) targets the source file
    // itself, per [[type reference::au-type-system]], so it is never dangling. Resolving it against the
    // repo index looks up an empty name and misreports it as broken.
    if w.is_local() && w.repo.is_none() {
        return SemanticTokenKind::WikilinkResolved {
            target: w.target.clone(),
            repo: w.repo.clone(),
            resolved: source.display().to_string(),
        };
    }
    let resolved =
        resolve_wikilink(kb, source, &w.target, w.repo.as_deref()).map(|p| p.display().to_string());
    let target = w.target.clone();
    let repo = w.repo.clone();
    match resolved {
        Some(resolved) => SemanticTokenKind::WikilinkResolved {
            target,
            repo,
            resolved,
        },
        None => SemanticTokenKind::WikilinkBroken { target, repo },
    }
}

/// A file-absolute `ByteRange` as a `SpanRange` with `line_col` attached.
fn located(
    span: tb_diagnostics::ByteRange,
    lines: Option<&tb_diagnostics::LineIndex>,
) -> wire::SpanRange {
    wire::SpanRange::new(span.start, span.end).located(lines)
}

/// Lift a body-relative `ByteRange` into a file-absolute `SpanRange` with
/// `line_col` attached. Body events span the markdown body, which starts at
/// `body_offset` into the physical file.
fn span_in_body(
    span: tb_diagnostics::ByteRange,
    body_offset: usize,
    lines: Option<&tb_diagnostics::LineIndex>,
) -> wire::SpanRange {
    wire::SpanRange::new(span.start + body_offset, span.end + body_offset).located(lines)
}

/// Resolve a navigation read's `target` to a file path, scoped to `origin`
/// when given. The `target` carries the wikilink fragment grammar, so an
/// embedded `::repo` qualifier is honored exactly as `references_out` resolves
/// a body link. With an `origin` the resolution is source-scoped, repo-local
/// for a bare target and cross-repo for a `::repo` one. Without an `origin` a
/// `::repo` target still resolves against the named repo's index, and a bare
/// target falls back to the scopeless union across every repo index, the
/// single-repo form. `None` when unresolved or ambiguous.
///
/// A commit-pinned target (`[[file::@sha]]` / `[[::@sha]]`) is an inert tombstone
/// into an immutable past: it resolves to NO live file, matching the backlink index
/// (`resolve_link`) and the outbound-pin contract (`references_out` carries
/// `resolved: null` for any commit-bearing edge). Live-resolving a pin by name would
/// misattribute on name reuse. See [[spec - pinned references - a recorded resolved edge with an immutable past and an on-demand forward trace]].
fn resolve_navigation_target(
    kb: &KnowledgeBase,
    target: &str,
    origin: Option<&Path>,
) -> Option<PathBuf> {
    let parsed = parse_wikilink_inner(target).ok();
    // An inert pin (any `@sha`) never resolves to a live file. Checked first, before
    // the local and name branches, so a pin is a tombstone whatever else it carries.
    if parsed.as_ref().is_some_and(|w| w.is_inert_pin()) {
        return None;
    }
    // The LOCAL form (`[[^id]]` / `[[#head]]` / `[[^^id]]`, an empty target with a
    // locating fragment) resolves to the origin file itself, per [[type reference::au-type-system]].
    // Name lookup is skipped, so it never dangles — the same resolution the backlink
    // index (`resolve_link`) and the validator (`is_local`) perform. Without an
    // origin there is no file to be local to, so it resolves nothing.
    // The parse fallback trims, matching `parse_wikilink_inner`'s own trim, so a
    // whitespace-only target reads as empty (local) rather than a name lookup on blanks.
    let is_local = parsed.as_ref().map_or(target.trim().is_empty(), |w| {
        w.is_local() && w.repo.is_none()
    });
    if is_local {
        return origin.map(Path::to_path_buf);
    }
    let (name, repo) = match parsed {
        Some(w) => (w.target, w.repo),
        None => (target.to_string(), None),
    };
    match origin {
        Some(origin) => resolve_wikilink(kb, origin, &name, repo.as_deref()),
        None => match &repo {
            // A `::repo` qualifier names its own scope, so it resolves without
            // an origin: the named repo's index.
            Some(repo_q) => kb.index_for_repo(repo_q)?.resolve(&name).ok(),
            None => kb.resolve_any(&name).ok(),
        },
    }
}

/// Resolve a wikilink target; `None` when unresolved or ambiguous. Scoped to
/// `origin` (the file the link appears in) when given, so member-local and
/// `::repo`-qualified targets resolve the same as `references_out`; scopeless
/// otherwise. Carries the engine's classification beside the path, so a
/// consumer knows what it resolved to without re-deriving it from the
/// extension.
///
/// `source` carries an openable container path plus a container-relative span
/// when the resolved path is a type-def, the same `{ file, span }` the type
/// reads carry: a consumer opens `source.file` and reveals `source.span`. A
/// non-type-def `path` is itself a real, directly-openable file and `source` is
/// `null`.
fn resolve_target_view(
    kb: &KnowledgeBase,
    target: &str,
    origin: Option<&Path>,
) -> Option<serde_json::Value> {
    let path = resolve_navigation_target(kb, target, origin)?;
    let entry = kb.catalog.get(&path);
    let kind = entry
        .map(|e| match e.kind {
            tb_parser::FileKind::TypeDef => "type-def",
            tb_parser::FileKind::Instance => "instance",
            tb_parser::FileKind::RepoRegistry => "repo-registry",
            tb_parser::FileKind::Workspace => "workspace",
            tb_parser::FileKind::RepoLock => "repo-lock",
            tb_parser::FileKind::Unclassified => "unclassified",
        })
        .unwrap_or("unclassified");
    // The content hash doubles as the mutation channel's `expected_hash`
    // source; `null` for files the build never read.
    let hash = entry.and_then(|e| e.hash).map(crate::mutate::hash_hex);
    let source = match kb.file_parse(&path) {
        Some(FileParse::TypeDef {
            type_def: Some(def),
            ..
        }) => Some(wire::source_loc_at(kb, &path, def.source_span)),
        _ => None,
    };
    Some(serde_json::json!({
        "path": path.display().to_string(),
        "kind": kind,
        "hash": hash,
        "source": source,
    }))
}

/// Resolve `[[target^block-id]]` to its addressable entity per
/// [[type block-id::au-type-system]]: an inline record carrying `^: id` (checked first,
/// frontmatter precedes the body), else the first body occurrence — a
/// typed `[:field]` fence, an untyped fence id, or a bare `^id` marker.
/// The read is the one resolution surface, navigation included; only an
/// unresolvable target or an absent id returns `None`.
fn resolve_block_id_view(
    kb: &KnowledgeBase,
    target: &str,
    block_id: &str,
    origin: Option<&Path>,
) -> Option<ResolvedBlockDto> {
    let path = resolve_navigation_target(kb, target, origin)?;
    if let Some(FileParse::Instance {
        instance: Some(inst),
        ..
    }) = kb.file_parse(&path)
    {
        if let Some(record) =
            crate::resolution_build::record_targets_of_kb(kb, &path, inst).get(block_id)
        {
            return Some(ResolvedBlockDto {
                file_path: path.display().to_string(),
                // The qualified form, so a `::repo` record claim stays qualified
                // (the `qualified` field preserves it; `claims` is bare).
                type_claim: record.qualified.iter().map(|c| c.authored()).collect(),
                span: wire::SpanRange::new(record.span.start, record.span.end)
                    .located(kb.line_index(&path)),
                kind: "record",
            });
        }
    }
    let (body, body_offset) = kb.file_parse(&path)?.markdown_body()?;
    for ev in scan_body(body) {
        match ev {
            BodyEvent::FencedBlock {
                info,
                body: block_body,
                span,
                trailing_block_id: Some(id),
            } if id == block_id => {
                let typed = extract_field_marker(info).is_some();
                return Some(ResolvedBlockDto {
                    file_path: path.display().to_string(),
                    type_claim: if typed {
                        block_type_claim(block_body)
                    } else {
                        Vec::new()
                    },
                    span: wire::SpanRange::new(span.start + body_offset, span.end + body_offset)
                        .located(kb.line_index(&path)),
                    kind: if typed { "typed_block" } else { "marker" },
                });
            }
            BodyEvent::BlockIdMarker { id, span } if id == block_id => {
                return Some(ResolvedBlockDto {
                    file_path: path.display().to_string(),
                    type_claim: Vec::new(),
                    span: wire::SpanRange::new(span.start + body_offset, span.end + body_offset)
                        .located(kb.line_index(&path)),
                    kind: "marker",
                });
            }
            _ => {}
        }
    }
    None
}

/// Resolve a `#head` anchor to its heading line in the target file.
/// Matching is the engine's contract per [[type reference::au-type-system]] —
/// case-insensitive exact heading text, first match in document order —
/// so consumers never re-implement it client-side. `None` when the
/// target doesn't resolve, isn't a markdown instance, or no heading
/// matches.
fn resolve_anchor_view(
    kb: &KnowledgeBase,
    target: &str,
    anchor: &str,
    origin: Option<&Path>,
) -> Option<ResolvedAnchorDto> {
    let path = resolve_navigation_target(kb, target, origin)?;
    let (body, body_offset) = kb.file_parse(&path)?.markdown_body()?;
    let events = scan_body(body);
    let span = tb_references::resolve_anchor(&events, anchor)?;
    Some(ResolvedAnchorDto {
        file_path: path.display().to_string(),
        span: wire::SpanRange::new(span.start + body_offset, span.end + body_offset)
            .located(kb.line_index(&path)),
    })
}

/// Every heading in a target file, in document order — the listing dual of
/// [`resolve_anchor_view`].
///
/// `None` only when the TARGET doesn't resolve, the catalog's unresolved-lookup
/// signal. A file that resolves but carries no markdown body (a pure-YAML
/// instance, an unread asset) answers an EMPTY array: it resolved, it simply
/// has no headings, and collapsing that into `None` would report "no such file"
/// for a file that exists.
fn anchors_view(kb: &KnowledgeBase, target: &str, origin: Option<&Path>) -> Option<Vec<AnchorDto>> {
    let path = resolve_navigation_target(kb, target, origin)?;
    let Some((body, body_offset)) = kb.file_parse(&path).and_then(|p| p.markdown_body()) else {
        return Some(Vec::new());
    };
    let line_index = kb.line_index(&path);
    Some(
        scan_body(body)
            .into_iter()
            .filter_map(|ev| match ev {
                BodyEvent::Heading { level, text, span } => Some(AnchorDto {
                    // Trimmed to match what `tb_references::resolve_anchor`
                    // compares, so the served text round-trips through
                    // `[[file#<text>]]` by local guarantee rather than by
                    // agreement with the parser.
                    text: text.trim().to_string(),
                    level,
                    span: wire::SpanRange::new(span.start + body_offset, span.end + body_offset)
                        .located(line_index),
                }),
                _ => None,
            })
            .collect(),
    )
}

/// The resolvable file set: every catalogued file, path-sorted, paged.
///
/// The catalogue IS the resolvable set — the reference index is built from it,
/// so what is catalogued is what a wikilink can reach. That is why an ASSET
/// appears: the walker records it by path and never reads it, precisely so
/// `file*` resolves against it ([[type-def shape file::au-type-system]]). A consumer completing
/// `[[` must be able to offer a target the validator accepts.
///
/// A projection of the held catalogue, so no walk and no disk read.
/// Default match page for `grep`: one page of hits is a search result, not a
/// dump of the file.
const GREP_DEFAULT_LIMIT: usize = 20;

/// The largest match page `grep` will return.
const GREP_MAX_LIMIT: usize = 200;

/// Default match page for `search`: one page of ranked notes.
const SEARCH_DEFAULT_LIMIT: usize = 20;

/// The largest match page `search` will return. Smaller than `grep`'s: a ranked
/// note is the start of a reading, not a line to jump to.
const SEARCH_MAX_LIMIT: usize = 100;

/// The largest file `grep` reads. A note is kilobytes; a megabyte is a binary
/// someone committed under a text extension, and scanning it per query is not a
/// cost a search surface should carry silently.
const GREP_MAX_FILE_BYTES: u64 = 1024 * 1024;

/// How far into a file `grep` looks for a NUL byte before treating it as binary.
const GREP_SNIFF_BYTES: usize = 8192;

/// The longest line text `grep` echoes back. A minified line would otherwise
/// put a megabyte in one match record.
const GREP_MAX_LINE_CHARS: usize = 400;

/// One literal match on one line.
#[derive(Debug, Serialize, PartialEq, Eq)]
struct GrepHit {
    path: String,
    repo: Option<String>,
    /// 1-based line number, matching how an editor and a diagnostic report it.
    line: usize,
    /// 1-based character column of the match's first character.
    column: usize,
    /// The line, trimmed of its line ending, cut to [`GREP_MAX_LINE_CHARS`].
    text: String,
    /// Whether `text` was cut, so a consumer never mistakes a truncation for the
    /// whole line.
    text_truncated: bool,
}

/// The `grep` answer: one page of hits plus the totals that make the page
/// interpretable.
#[derive(Debug, Default, Serialize)]
struct GrepScan {
    matches: Vec<GrepHit>,
    /// Matches over the whole scanned set, not just this page.
    total: usize,
    /// Whether the page ends before the last match.
    truncated: bool,
    /// Files not scanned, by the two bounds: too large, or binary.
    skipped_files: usize,
}

/// One ranked document.
#[derive(Debug, Serialize)]
struct SearchMatch {
    /// The absolute path, the same address space every other read serves.
    path: String,
    /// The BM25 score: the sum of the matched terms' contributions over both
    /// fields. Higher is better and comparable only within one query over one
    /// corpus.
    score: f64,
    /// How many distinct query terms this document holds, the quantity
    /// `min_should_match` is compared against.
    matched_terms: usize,
    /// TRUE when every query term this document matched is common in the corpus:
    /// the hit says "these words occur here", not "this base knows this". A caller
    /// that needs an answer should treat a page of weak hits as the base having
    /// nothing, and ask a sharper question.
    weak: bool,
    /// A short window of the note's BODY — never its frontmatter — around the
    /// first query term it holds, cut to [`search::EXCERPT_CHARS`] characters at a
    /// word boundary, one line, with `…` where an edge was cut from the middle.
    ///
    /// Enough for a caller to judge a hit without a second read: the context pack
    /// used to spend one `content` call per fragment for exactly this window
    /// (`docs/research/context-pack.md`). Absent when the catalogue holds no body
    /// for the path at read time, which a caller reads as "ask `content`".
    #[serde(skip_serializing_if = "Option::is_none")]
    excerpt: Option<String>,
    /// Present when this match was found by source identity — the query is a
    /// URL whose normalized identity equals this note's `source_url:` — and
    /// absent on every ranked match, so a caller can tell an exact lookup from
    /// a scored guess without comparing paths.
    #[serde(skip_serializing_if = "Option::is_none")]
    matched_source: Option<MatchedSource>,
}

/// Why a match was found by source identity, the proof an exact URL lookup
/// carries that a ranked hit does not.
#[derive(Debug, Serialize)]
struct MatchedSource {
    /// The note's `source_url:` as the note authored it.
    source_url: String,
    /// The identity both the query and that URL normalize to (the provenance
    /// rules: scheme, `www.`, fragment, query string and trailing slashes
    /// dropped; `arxiv.org/abs|pdf|html/<id>`, `docs.google.com/document/d/<id>`
    /// and `github.com/<owner>/<repo>` collapsed). See
    /// [`crate::source_url::identity`].
    identity: String,
}

/// The `search` answer: one page of ranked documents plus the totals that make
/// the page interpretable.
#[derive(Debug, Serialize)]
struct SearchAnswer {
    matches: Vec<SearchMatch>,
    /// Matches over the whole in-scope set, not just this page.
    total: usize,
    /// The page's start, echoed so a caller need not remember it.
    offset: usize,
    /// Where the next page starts, or `null` on the page that reached the end.
    /// Nullable rather than a sentinel: a count past the end is indistinguishable
    /// from a page the caller has already read.
    next_offset: Option<usize>,
    /// Whether the conjunction path answered.
    exact: bool,
    /// The character width each `matches[].excerpt` was cut to
    /// ([`search::EXCERPT_CHARS`]). Echoed once rather than per match, so a caller
    /// can budget the answer without reading it. It is the window, not a hard
    /// bound: a window that falls inside one long word runs forward to the end of
    /// it, and one cut from a shorter body is shorter.
    excerpt_chars: usize,
}

/// How far a `type: moc` topic map gives way to a note that matched at least as many query terms.
///
/// A map is a hub, not evidence: it states a topic in its title and body without answering the
/// question itself. The discount is post-ranking, so it cannot change `matched_terms`, the
/// `min_should_match` floor or the candidate set (`docs/research/task-set.md` §4).
const MAP_WEIGHT: f64 = 0.5;

/// Whether `path` is a `type: moc` instance.
fn is_map(kb: &KnowledgeBase, path: &std::path::Path) -> bool {
    kb.catalog.get(path).is_some_and(|entry| {
        matches!(entry.parse.as_ref(), FileParse::Instance { instance: Some(inst), .. }
            if inst.type_claim.iter().any(|claim| claim.name.as_str() == "moc"))
    })
}

/// Whether a map yields to the best non-map hit in the page.
///
/// The rule is `map >= best`, not `<=`: a map that matched **at least as many** query terms as
/// every ordinary note is the hub case this discount exists for — its breadth is why it matched
/// them, not evidence that it answers. A map that matched fewer terms than a note is already
/// behind on terms and is left alone. `docs/research/map-yield-contract-2026-11-18.md` records
/// why the sentence in `task-set.md` reads the other way: the registration's measured case was a
/// tie (six terms each), so no run distinguished the directions, and the code is the rule that
/// fixes the multi-term hub. Changing the sign needs a new measurement, not a reading.
fn map_yields(map_terms: usize, best_note_terms: usize) -> bool {
    map_terms >= best_note_terms
}

/// A map yields to the notes under it, so a hub cannot outrank the note it exists to point at.
/// Silent when the page holds no note at all.
fn yield_maps(kb: &KnowledgeBase, hits: &mut Vec<crate::search::SearchHit<'_>>) {
    let Some(best) = hits
        .iter()
        .filter(|hit| !is_map(kb, hit.path))
        .map(|hit| hit.matched_terms)
        .max()
    else {
        return;
    };
    let mut moved = false;
    for hit in hits.iter_mut() {
        if map_yields(hit.matched_terms, best) && is_map(kb, hit.path) {
            hit.score *= MAP_WEIGHT;
            moved = true;
        }
    }
    if moved {
        hits.sort_by(|left, right| {
            right
                .score
                .total_cmp(&left.score)
                .then_with(|| left.path.cmp(right.path))
        });
    }
}

/// The `search` answer over the held index.
///
/// Ranking and paging are separate passes on purpose: `total` counts every match
/// in scope, so a page is interpretable without a second call. The scope decides
/// the CANDIDATES, before any document is scored: a note the caller cannot
/// receive is not ranked either, so a read scoped to `own` does not pay for every
/// mounted member's notes (entry 1 of
/// `docs/research/ruvector-adr-2026-11-16.md`).
// Every argument is one field of the request this read serves, passed through
// unchanged; a struct would name the wire request twice.
#[allow(clippy::too_many_arguments)]
fn search_view(
    kb: &KnowledgeBase,
    query: &str,
    exact: bool,
    min_should_match: usize,
    repo: Option<&str>,
    scope: wire::TypeScope,
    limit: usize,
    offset: usize,
) -> SearchAnswer {
    let mask = scope_engaged(repo, scope).then(|| candidate_mask(kb, repo, scope));
    // A URL-shaped query is a lookup before it is a ranking: the notes whose
    // `source_url:` names the queried address are the exact answer, and they
    // lead the page so a caller cannot mistake them for scored guesses. The
    // ranking still runs and follows them, minus the paths the lookup named.
    // A query that is not a URL, or a URL no `source_url` holds, finds nothing
    // here and falls through to the ranking unchanged.
    let source_hits: Vec<crate::source_url::SourceHit> =
        crate::source_url::lookup(&kb.catalog, query)
            .into_iter()
            .filter(|hit| in_read_scope(kb, &hit.path, repo, scope))
            .collect();
    let source_paths: std::collections::HashSet<&std::path::Path> =
        source_hits.iter().map(|hit| hit.path.as_path()).collect();
    let mut ranked: Vec<crate::search::SearchHit<'_>> = kb
        .search
        .query(
            query,
            exact,
            min_should_match,
            mask.as_deref()
                .map_or(crate::search::DocMask::all(), crate::search::DocMask::new),
        )
        .into_iter()
        .filter(|hit| !source_paths.contains(hit.path))
        .collect();
    // A lookup stays a lookup: one or two terms name a note, and a map may be the note they name.
    // At three distinct terms the read answers a question, and a hub there is a pointer, not evidence.
    if !exact && crate::search::distinct_terms(query).len() >= 3 {
        yield_maps(kb, &mut ranked);
    }
    let total = source_hits.len() + ranked.len();
    let terms = crate::search::distinct_terms(query);
    let page_end = (offset + limit).min(total);
    let mut matches: Vec<SearchMatch> = Vec::with_capacity(page_end.saturating_sub(offset));
    for index in offset..page_end {
        matches.push(if let Some(hit) = source_hits.get(index) {
            source_match(kb, hit, &terms)
        } else {
            let hit = &ranked[index - source_hits.len()];
            SearchMatch {
                path: hit.path.display().to_string(),
                score: hit.score,
                matched_terms: hit.matched_terms,
                weak: hit.weak,
                excerpt: excerpt_of(kb, hit.path, &terms),
                matched_source: None,
            }
        });
    }
    let next_offset = (page_end < total).then_some(page_end);
    SearchAnswer {
        matches,
        total,
        offset,
        next_offset,
        exact,
        excerpt_chars: crate::search::EXCERPT_CHARS,
    }
}

/// The match a source-identity lookup answers with: the note itself, unranked.
///
/// `score` 0.0, `matched_terms` 0 and `weak` false say "no ranking happened" —
/// there is no BM25 answer here to interpret, and the existing fields keep
/// their meanings by staying at their no-match values. The proof the hit is
/// exact rather than a guess is `matched_source`: the identity both the query
/// and the note's own `source_url:` normalize to.
fn source_match(
    kb: &KnowledgeBase,
    hit: &crate::source_url::SourceHit,
    terms: &[String],
) -> SearchMatch {
    SearchMatch {
        path: hit.path.display().to_string(),
        score: 0.0,
        matched_terms: 0,
        weak: false,
        excerpt: excerpt_of(kb, &hit.path, terms),
        matched_source: Some(MatchedSource {
            source_url: hit.note_url.clone(),
            identity: hit.identity.clone(),
        }),
    }
}

/// A match's body excerpt, cut from the parse the catalogue already holds.
///
/// No disk read and no `spawn_blocking`: the body is a slice of the parse the ranking itself
/// was built from. `None` when the path has no catalogue entry or no body, which leaves the
/// `excerpt` field off the match rather than sending an empty string a caller could mistake for
/// "the note says nothing".
fn excerpt_of(kb: &KnowledgeBase, path: &std::path::Path, terms: &[String]) -> Option<String> {
    let own = path.to_path_buf();
    let entry = kb.catalog.get(&own)?;
    let body = crate::search::body_of(&entry.parse)?;
    Some(crate::search::excerpt(
        body,
        terms,
        crate::search::EXCERPT_CHARS,
    ))
}

/// Whether a `repo` / `scope` selection narrows anything at all.
///
/// One spelling of the condition, so a read that skips per-item work and the
/// predicate that does the work cannot disagree about whether a filter is on.
fn scope_engaged(repo: Option<&str>, scope: wire::TypeScope) -> bool {
    repo.is_some() || scope.own_only()
}

/// Whether a path is in the `repo` / `scope` selection the catalogue-wide reads
/// share, spelled once so `grep` and `search` cannot disagree about scope.
fn in_read_scope(
    kb: &KnowledgeBase,
    path: &std::path::Path,
    repo: Option<&str>,
    scope: wire::TypeScope,
) -> bool {
    if !scope_engaged(repo, scope) {
        return true;
    }
    let Some(owner) = kb.repos.repo_of(path) else {
        // An unowned file cannot answer "is this mine", so it is out of scope
        // whenever either filter is engaged.
        return false;
    };
    if let Some(want) = repo {
        if owner.name.as_str() != want {
            return false;
        }
    }
    scope.includes_repo(kb, owner)
}

/// The documents a scoped `search` may score: one flag per indexed document id.
///
/// The SAME [`in_read_scope`] predicate, asked once per repo instead of once per
/// document: the predicate resolves a path to its owning repo, and that walk is
/// the cost a candidate mask exists to remove. The repos go in the order
/// `repo::RepoMap::repo_of` resolves a path — root depth, then name — because
/// [`crate::search::SearchIndex::candidate_mask`] paints each repo's range of
/// document ids and lets a deeper root replace a shallower one's verdict. The
/// compiled-in engine repo's sentinel root matches no document, so its verdict
/// paints an empty range.
fn candidate_mask(kb: &KnowledgeBase, repo: Option<&str>, scope: wire::TypeScope) -> Vec<bool> {
    let mut repos: Vec<&crate::repo::Repo> = kb.repos.repos().iter().collect();
    repos.sort_by(|left, right| {
        left.root
            .components()
            .count()
            .cmp(&right.root.components().count())
            .then_with(|| left.name.as_str().cmp(right.name.as_str()))
    });
    kb.search.candidate_mask(repos.into_iter().map(|member| {
        (
            member.root.as_path(),
            in_read_scope(kb, &member.root, repo, scope),
        )
    }))
}

/// The catalogued paths a `grep` scans, paired with their owning repo.
fn grep_paths(
    kb: &KnowledgeBase,
    repo: Option<&str>,
    scope: wire::TypeScope,
) -> Vec<(std::path::PathBuf, Option<String>)> {
    kb.catalog
        .iter()
        .filter(|(path, _)| in_read_scope(kb, path, repo, scope))
        .map(|(path, _)| {
            (
                path.clone(),
                kb.repos.repo_of(path).map(|r| r.name.as_str().to_string()),
            )
        })
        .collect()
}

/// Scan the given files for a literal query.
///
/// Line-oriented and literal by design: it is the read an agent uses to find the
/// note that mentions a phrase, and the caller's own tool decides ranking.
fn grep_scan(
    paths: &[(std::path::PathBuf, Option<String>)],
    query: &str,
    case_sensitive: bool,
    limit: usize,
    offset: usize,
) -> GrepScan {
    let needle = if case_sensitive {
        query.to_owned()
    } else {
        query.to_lowercase()
    };
    let mut scan = GrepScan::default();
    // Instrumentation: this scan opens every path it was handed, so the visit
    // count is the path list's length — the corpus, whatever `limit` pages out.
    crate::verb_metrics::note_documents(paths.len() as u64);
    for (path, repo) in paths {
        let Ok(metadata) = std::fs::metadata(path) else {
            scan.skipped_files += 1;
            continue;
        };
        if !metadata.is_file() || metadata.len() > GREP_MAX_FILE_BYTES {
            scan.skipped_files += 1;
            continue;
        }
        let Ok(bytes) = std::fs::read(path) else {
            scan.skipped_files += 1;
            continue;
        };
        let sniff = &bytes[..bytes.len().min(GREP_SNIFF_BYTES)];
        if sniff.contains(&0) {
            scan.skipped_files += 1;
            continue;
        }
        let text = String::from_utf8_lossy(&bytes);
        for (index, line) in text.split('\n').enumerate() {
            let line = line.strip_suffix('\r').unwrap_or(line);
            let haystack;
            let haystack = if case_sensitive {
                line
            } else {
                haystack = line.to_lowercase();
                haystack.as_str()
            };
            let Some(byte_column) = haystack.find(&needle) else {
                continue;
            };
            let column = line[..byte_column].chars().count() + 1;
            scan.total += 1;
            if scan.total <= offset {
                continue;
            }
            if scan.matches.len() >= limit {
                continue;
            }
            let (shown, cut) = truncate_chars(line, GREP_MAX_LINE_CHARS);
            scan.matches.push(GrepHit {
                path: path.display().to_string(),
                repo: repo.clone(),
                line: index + 1,
                column,
                text: shown,
                text_truncated: cut,
            });
        }
    }
    scan.truncated = scan.total > offset + scan.matches.len();
    scan
}

/// Cut a line to `max` characters, reporting whether it was cut.
fn truncate_chars(line: &str, max: usize) -> (String, bool) {
    match line.char_indices().nth(max) {
        Some((index, _)) => (line[..index].to_owned(), true),
        None => (line.to_owned(), false),
    }
}

fn files_view(
    kb: &KnowledgeBase,
    repo: Option<&str>,
    scope: wire::TypeScope,
    limit: usize,
    offset: usize,
) -> Vec<FileDto> {
    kb.catalog
        .iter()
        .filter(|(path, _)| {
            if repo.is_none() && !scope.own_only() {
                return true;
            }
            let Some(owner) = kb.repos.repo_of(path) else {
                // An unowned file cannot answer "is this mine", so it is out of
                // scope whenever either filter is engaged.
                return false;
            };
            if let Some(want) = repo {
                if owner.name.as_str() != want {
                    return false;
                }
            }
            scope.includes_repo(kb, owner)
        })
        .skip(offset)
        .take(limit)
        .map(|(path, _)| FileDto {
            path: path.display().to_string(),
            stem: path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default(),
            repo: kb.repos.repo_of(path).map(|r| r.name.as_str().to_string()),
            kind: served_file_kind(kb.file_parse(path)),
        })
        .collect()
}

/// Every addressable id in a target file, in document order — the listing dual
/// of [`resolve_block_id_view`].
///
/// Both surfaces of [[type block-id::au-type-system]] in one stream: frontmatter inline records
/// carrying `^:`, and body occurrences (fence ids and bare markers). Sorted by
/// span, so frontmatter precedes the body the way the file does.
///
/// **Every occurrence is listed, duplicates included.** `resolve_block_id`
/// returns the FIRST and ignores the rest, which is right for resolution and
/// wrong for a listing: dropping the later ones would hide exactly what
/// `block-id-duplicate` reports. The listing is the file's surface, not the
/// resolver's verdict.
///
/// `None` only when the TARGET doesn't resolve, matching [`anchors_view`].
fn block_ids_view(
    kb: &KnowledgeBase,
    target: &str,
    origin: Option<&Path>,
) -> Option<Vec<BlockIdDto>> {
    let path = resolve_navigation_target(kb, target, origin)?;
    let line_index = kb.line_index(&path);
    let mut out: Vec<(usize, BlockIdDto)> = Vec::new();

    // Frontmatter records. Spans are already file-absolute.
    if let Some(FileParse::Instance {
        instance: Some(inst),
        ..
    }) = kb.file_parse(&path)
    {
        for (id, record) in
            crate::resolution_build::record_target_occurrences_of_kb(kb, &path, inst)
        {
            out.push((
                record.span.start,
                BlockIdDto {
                    id,
                    kind: "record",
                    // The qualified form, so a `::repo` record claim stays
                    // qualified, as `resolve_block_id` serves it.
                    type_claim: record.qualified.iter().map(|c| c.authored()).collect(),
                    span: wire::SpanRange::new(record.span.start, record.span.end)
                        .located(line_index),
                },
            ));
        }
    }

    // Body occurrences. Spans are body-relative, so shift by the offset.
    if let Some((body, body_offset)) = kb.file_parse(&path).and_then(|p| p.markdown_body()) {
        for ev in scan_body(body) {
            let (id, kind, type_claim, span) = match ev {
                BodyEvent::FencedBlock {
                    info,
                    body: block_body,
                    span,
                    trailing_block_id: Some(id),
                } => {
                    let typed = extract_field_marker(info).is_some();
                    (
                        id.to_string(),
                        if typed { "typed_block" } else { "marker" },
                        if typed {
                            block_type_claim(block_body)
                        } else {
                            Vec::new()
                        },
                        span,
                    )
                }
                BodyEvent::BlockIdMarker { id, span } => {
                    (id.to_string(), "marker", Vec::new(), span)
                }
                _ => continue,
            };
            let start = span.start + body_offset;
            out.push((
                start,
                BlockIdDto {
                    id,
                    kind,
                    type_claim,
                    span: wire::SpanRange::new(start, span.end + body_offset).located(line_index),
                },
            ));
        }
    }

    out.sort_by_key(|(start, _)| *start);
    Some(out.into_iter().map(|(_, dto)| dto).collect())
}

/// The `type:` claim of a marked fence's body, as a name list. Empty when
/// the body has no top-level `type:` key. A bare claim yields one name, a list
/// claim yields several.
fn block_type_claim(body: &str) -> Vec<String> {
    use tb_parser::yaml::{Scalar, YamlData, parse};
    let Ok(docs) = parse(body) else {
        return Vec::new();
    };
    let Some(doc) = docs.first() else {
        return Vec::new();
    };
    let YamlData::Mapping(map) = &doc.data else {
        return Vec::new();
    };
    for (k, v) in map.iter() {
        if !matches!(&k.data, YamlData::Value(Scalar::String(s)) if s.as_ref() == "type") {
            continue;
        }
        return match &v.data {
            YamlData::Value(Scalar::String(name)) => vec![name.to_string()],
            YamlData::Sequence(items) => items
                .iter()
                .filter_map(|it| match &it.data {
                    YamlData::Value(Scalar::String(name)) => Some(name.to_string()),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        };
    }
    Vec::new()
}

/// Direct children of a directory, files and subdirectories, hidden entries
/// (dot-prefixed) excluded. Derived from the held file catalog, so it reflects
/// the analyzed file set, the walker's ignore list already applied. Empty
/// directories do not appear, the catalog holds files, not bare directories.
fn children_view(kb: &KnowledgeBase, dir: &Path) -> Vec<DirEntryDto> {
    use std::collections::BTreeSet;
    let mut dirs: BTreeSet<PathBuf> = BTreeSet::new();
    let mut files: BTreeSet<PathBuf> = BTreeSet::new();
    // Path ordering keeps a directory's descendants contiguous, so seek to `dir`
    // and stop at the first key outside it, O(log n + descendants), not a scan
    // of the whole catalog. `take_while` on `starts_with` bounds the range.
    for (path, _) in kb
        .catalog
        .range(dir.to_path_buf()..)
        .take_while(|(p, _)| p.starts_with(dir))
    {
        if path.parent() == Some(dir) {
            files.insert(path.clone());
            continue;
        }
        // A descendant deeper than a direct child: its first component under
        // `dir` is a child directory. `strip_prefix` cannot fail here, the range
        // yields only descendants of `dir`.
        let Ok(rel) = path.strip_prefix(dir) else {
            continue;
        };
        let mut comps = rel.components();
        if let (Some(first), Some(_)) = (comps.next(), comps.next()) {
            dirs.insert(dir.join(first.as_os_str()));
        }
    }
    let mut out = Vec::new();
    for d in dirs {
        if let Some(name) = entry_name(&d) {
            out.push(DirEntryDto {
                path: d.display().to_string(),
                name,
                kind: "directory",
            });
        }
    }
    for f in files {
        if let Some(name) = entry_name(&f) {
            out.push(DirEntryDto {
                path: f.display().to_string(),
                name,
                kind: "file",
            });
        }
    }
    out
}

/// The top-level directories of EVERY mounted member, each tagged with its
/// owning repo: direct subdirectories holding at least one catalogued file,
/// hidden ones excluded, derived from the catalog.
///
/// Workspace-scoped, not entry-scoped. The former `top_level_graphs` walked
/// only the entry root, which answers approximately nothing in the expected
/// topology — a thin entry repo carrying `workspace.yaml` with the content in
/// `edit` members. It was also the sole entry-scoped field in an otherwise
/// workspace-scoped `overview`.
///
/// `repo` selects one member; absent spans every mounted one. `scope` filters
/// by CLASS: `own` keeps the user's editable repos, dropping dependencies whose
/// folders are noise for orientation and whose count grows with the transitive
/// dependency closure.
fn top_level_dirs_view(
    kb: &KnowledgeBase,
    repo: Option<&str>,
    scope: wire::TypeScope,
) -> Vec<TopLevelDirDto> {
    use std::collections::BTreeSet;
    let mut out: Vec<TopLevelDirDto> = Vec::new();
    for member in kb.repos.repos() {
        // The builtin `au-engine` repo is a compiled-in type source with no
        // real root on disk, so it has no directories to report.
        if member.builtin {
            continue;
        }
        if repo.is_some_and(|want| member.name.as_str() != want) {
            continue;
        }
        if !scope.includes_repo(kb, member) {
            continue;
        }
        let mut names: BTreeSet<String> = BTreeSet::new();
        for path in kb
            .catalog
            .range(member.root.clone()..)
            .take_while(|(p, _)| p.starts_with(&member.root))
            .map(|(p, _)| p)
        {
            let Ok(rel) = path.strip_prefix(&member.root) else {
                continue;
            };
            let mut comps = rel.components();
            if let (Some(first), Some(_)) = (comps.next(), comps.next()) {
                let name = first.as_os_str().to_string_lossy().to_string();
                if !name.starts_with('.') {
                    names.insert(name);
                }
            }
        }
        out.extend(names.into_iter().map(|name| {
            let path = member.root.join(&name);
            // A member rooted exactly here: the directory is another mounted
            // repo, reported for its container as well as for itself.
            let nested = kb
                .repos
                .repos()
                .iter()
                .find(|r| !r.builtin && r.root == path)
                .map(|r| r.name.as_str().to_string());
            TopLevelDirDto {
                repo: member.name.as_str().to_string(),
                path: path.display().to_string(),
                name,
                member: nested,
            }
        }));
    }
    out
}

/// The bounded top-N hubs the `overview` read carries, keeping the up-front
/// drop cheap. The standalone `hubs` read pages past it via `limit` / `offset`;
/// an absent `limit` there is this same bound, so `overview.hubs` mirrors an
/// argless call.
const HUB_LIMIT: usize = 20;

/// The default `neighborhood` node cap, applied when the caller names no
/// `max_nodes`. A safety ceiling on an unbounded walk, generous enough that the
/// common one- and two-hop calls never hit it, loud (`truncated` + `dropped`)
/// when they do.
const NEIGHBORHOOD_MAX_NODES: usize = 2000;

/// The `overview` read result: the up-front orientation map. Each field reuses
/// the shape of its own read, so a consumer reuses the DTOs it already holds.
#[derive(Debug, Serialize)]
struct OverviewView {
    /// The RESOLVED `repo` this map was computed at, echoed back. Null spans
    /// every mounted member.
    repo: Option<String>,
    /// The RESOLVED `scope` this map was computed at, echoed back — `own`
    /// unless the caller said otherwise.
    ///
    /// Echoed so a consumer drilling from a field into its own read passes
    /// these verbatim rather than re-deriving a default it cannot see. Without
    /// it, `overview({}).type_counts` and `type_counts({})` disagree silently:
    /// the summary card and the panel it opens would show different numbers,
    /// each plausible, neither an error. The catalog already echoes an argument
    /// this way (`subtypes.base`), and the envelope rule names an echoed
    /// argument as legitimate envelope metadata.
    scope: &'static str,
    // The members array directly (not the `members` read's `{ members: [...] }`
    // wrapper), so a consumer reads `overview.members`, not `.members.members`.
    members: Vec<wire::MemberView>,
    top_level_dirs: Vec<TopLevelDirDto>,
    type_counts: wire::TypeCountsView,
    diagnostic_counts: DiagnosticCountsView,
    hubs: Vec<Hub>,
    /// The scalar whole-graph summary, mirroring an argless `graph_shape` call
    /// at these same `repo` / `scope` args. The unbounded orphan path list is a
    /// summary card, so `orphan_paths` is off here — a consumer wanting it drills
    /// the `graph_shape` read.
    graph_shape: GraphShapeView,
}

/// Whether a file is in scope for a whole-graph fold, the shared `repo` /
/// `scope` predicate `hubs`, `graph_shape`, and `link_graph` apply so a second
/// spelling of "the user's own repos" cannot drift between them.
///
/// `repo` absent with `scope: all` is the fast path, everything in scope with no
/// owner resolution. Otherwise resolve the owning repo and gate on the named
/// `repo` and the `own`-vs-`all` class.
fn node_in_scope(
    kb: &KnowledgeBase,
    repo: Option<&str>,
    scope: wire::TypeScope,
    path: &Path,
) -> bool {
    if repo.is_none() && !scope.own_only() {
        return true;
    }
    let Some(owner) = kb.repos.repo_of(path) else {
        return false;
    };
    if let Some(want) = repo {
        if owner.name.as_str() != want {
            return false;
        }
    }
    scope.includes_repo(kb, owner)
}

/// Whether a catalog file is a knowledge-graph node. Engine-schema files
/// (`repo.yaml`, the workspace manifest, the `repo.lock`) are substrate CONFIG,
/// not knowledge, so they never count as a node, an orphan, or a component: a
/// consumer's orphan list must not surface `.arsumbris/repo.yaml`, and a graph
/// view must not draw it. The content kinds are type-defs, instances, and
/// everything else the walk catalogues (notes and assets, both `Unclassified`).
fn is_graph_node_kind(kind: tb_parser::FileKind) -> bool {
    use tb_parser::FileKind;
    matches!(
        kind,
        FileKind::TypeDef | FileKind::Instance | FileKind::Unclassified
    )
}

/// A disjoint-set forest with path halving and union by rank, for the
/// connected-component fold. Nodes are catalog indices.
struct UnionFind {
    parent: Vec<usize>,
    rank: Vec<u8>,
}

impl UnionFind {
    fn new(n: usize) -> Self {
        Self {
            parent: (0..n).collect(),
            rank: vec![0; n],
        }
    }

    fn find(&mut self, mut x: usize) -> usize {
        while self.parent[x] != x {
            self.parent[x] = self.parent[self.parent[x]];
            x = self.parent[x];
        }
        x
    }

    fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra == rb {
            return;
        }
        match self.rank[ra].cmp(&self.rank[rb]) {
            std::cmp::Ordering::Less => self.parent[ra] = rb,
            std::cmp::Ordering::Greater => self.parent[rb] = ra,
            std::cmp::Ordering::Equal => {
                self.parent[rb] = ra;
                self.rank[ra] += 1;
            }
        }
    }
}

/// The scalar whole-graph summary. Fold the in-scope catalog (the node universe)
/// and the in-scope backlink index (edges induced on in-scope endpoints) into
/// component count, orphan counts, degree histograms, and density. One
/// union-find pass yields components and the largest component; the same edge
/// walk yields the degrees, and the node walk the orphans and histograms.
///
/// Multiplicity matches `hubs`: a target's in-degree is `edges.len()`, so
/// `refs_total` on a `hubs` entry equals this node's in-degree. Component
/// connectivity ignores multiplicity.
fn graph_shape_view(
    kb: &KnowledgeBase,
    repo: Option<&str>,
    scope: wire::TypeScope,
    orphan_paths: bool,
) -> GraphShapeView {
    // The in-scope node universe, indexed for the union-find and the edge walk.
    let nodes: Vec<&Path> = kb
        .catalog
        .iter()
        .filter(|(_, e)| is_graph_node_kind(e.kind))
        .map(|(p, _)| p.as_path())
        .filter(|p| node_in_scope(kb, repo, scope, p))
        .collect();
    let index: std::collections::HashMap<&Path, usize> =
        nodes.iter().enumerate().map(|(i, p)| (*p, i)).collect();
    let n = nodes.len();

    let mut uf = UnionFind::new(n);
    let mut in_degree = vec![0usize; n];
    let mut out_degree = vec![0usize; n];
    let mut edges_structural = 0usize;
    let mut edges_navigational = 0usize;

    for (target, edges) in kb.backlinks.iter() {
        let Some(&ti) = index.get(target.as_path()) else {
            continue;
        };
        for edge in edges {
            // An edge counts only when BOTH endpoints are in scope; an
            // out-of-scope source is not a node here, so the edge is dropped.
            let Some(&si) = index.get(edge.source.as_path()) else {
                continue;
            };
            uf.union(si, ti);
            in_degree[ti] += 1;
            out_degree[si] += 1;
            if edge.slot.is_some() {
                edges_structural += 1;
            } else {
                edges_navigational += 1;
            }
        }
    }
    let edge_count = edges_structural + edges_navigational;
    // Instrumentation: the closure scans the whole in-scope node universe and then
    // every edge whose endpoints are both inside it. This read's answer is
    // corpus-wide by definition, so these numbers growing with the corpus is the
    // contract, not a defect — the counter exists to keep that visible.
    crate::verb_metrics::note_documents(n as u64 + edge_count as u64);

    // Component sizes from the union-find roots.
    let mut comp_size: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();
    for i in 0..n {
        let root = uf.find(i);
        *comp_size.entry(root).or_default() += 1;
    }
    let components = comp_size.len();
    let largest_component = comp_size.values().copied().max().unwrap_or(0);

    // Orphans and degree histograms in one node walk.
    let mut no_inbound = 0usize;
    let mut isolated = 0usize;
    let mut in_hist: BTreeMap<usize, usize> = BTreeMap::new();
    let mut out_hist: BTreeMap<usize, usize> = BTreeMap::new();
    let mut paths: Vec<OrphanPath> = Vec::new();
    for i in 0..n {
        *in_hist.entry(in_degree[i]).or_default() += 1;
        *out_hist.entry(out_degree[i]).or_default() += 1;
        if in_degree[i] == 0 {
            no_inbound += 1;
            let kind = if out_degree[i] == 0 {
                isolated += 1;
                "isolated"
            } else {
                "no_inbound"
            };
            if orphan_paths {
                paths.push(OrphanPath {
                    path: nodes[i].display().to_string(),
                    kind,
                });
            }
        }
    }
    if orphan_paths {
        paths.sort_by(|a, b| a.path.cmp(&b.path));
    }

    let density = if n == 0 {
        0.0
    } else {
        edge_count as f64 / n as f64
    };

    let to_buckets = |hist: BTreeMap<usize, usize>| {
        hist.into_iter()
            .map(|(degree, count)| DegreeBucket { degree, count })
            .collect()
    };

    GraphShapeView {
        repo: repo.map(|s| s.to_string()),
        scope: scope.as_str(),
        node_count: n,
        edge_count,
        edges_structural,
        edges_navigational,
        components,
        largest_component,
        orphans: OrphansView {
            no_inbound,
            isolated,
            paths: orphan_paths.then_some(paths),
        },
        degree: DegreeView {
            inbound: to_buckets(in_hist),
            out: to_buckets(out_hist),
        },
        density,
    }
}

/// Default cap per `graph_health` list.
const GRAPH_HEALTH_DEFAULT_LIMIT: usize = 20;

/// The largest per-list cap `graph_health` returns.
const GRAPH_HEALTH_MAX_LIMIT: usize = 200;

/// The graph's problems, joined in one answer.
#[derive(Debug, Serialize)]
struct GraphHealthView {
    repo: Option<String>,
    scope: &'static str,
    counts: GraphHealthCounts,
    /// Directed cycles: each entry is one strongly connected component with more
    /// than one node, or a single node with a self-loop. `nodes` is sorted, not
    /// the traversal order — the component is the fact, the order is incidental.
    cycles: Vec<CycleView>,
    /// Nodes nothing links to AND that link nowhere: a singleton island.
    isolated: Vec<HealthNode>,
    /// Nodes that link nowhere else (`out == 0`), the writing dead ends.
    dead_ends: Vec<HealthNode>,
    /// Nodes nothing points at (`in == 0`) but that do point onward.
    sources: Vec<HealthNode>,
    /// Reference diagnostics: a target that does not resolve or is ambiguous.
    broken_links: Vec<BrokenLink>,
    /// Undirected bridges: a link whose removal separates its ends. A pair joined
    /// by more than one edge is never a bridge, and a self-loop never is.
    bridges: Vec<BridgeView>,
    /// Nodes whose removal disconnects some of their neighbours.
    articulation_points: Vec<HealthNode>,
    /// How tightly the undirected projection is woven: triangles and global
    /// transitivity (closed triples over all triples).
    clustering: ClusteringView,
    /// Highest total degree, the navigational crossroads.
    central: Vec<HealthNode>,
    /// Whether any list was cut by `limit`.
    truncated: bool,
}

#[derive(Debug, Serialize)]
struct GraphHealthCounts {
    nodes: usize,
    edges: usize,
    components: usize,
    isolated: usize,
    dead_ends: usize,
    sources: usize,
    cycles: usize,
    /// Nodes inside a cycle, which is what a reader weighs, not the components.
    nodes_in_cycles: usize,
    broken_links: usize,
    /// Edges whose removal disconnects the undirected projection.
    bridges: usize,
    /// Nodes whose removal disconnects the undirected projection.
    articulation_points: usize,
}

#[derive(Debug, Serialize)]
struct CycleView {
    length: usize,
    nodes: Vec<String>,
}

/// One undirected bridge: the two ends, the smaller path first.
#[derive(Debug, Serialize)]
struct BridgeView {
    a: String,
    b: String,
}

#[derive(Debug, Serialize)]
struct ClusteringView {
    /// Triangles in the undirected simple projection.
    triangles: usize,
    /// Closed triples over all triples, `0.0` when the graph holds none.
    transitivity: f64,
}

#[derive(Debug, Serialize)]
struct HealthNode {
    path: String,
    r#in: usize,
    out: usize,
    total: usize,
}

#[derive(Debug, Serialize)]
struct BrokenLink {
    path: String,
    code: String,
    message: String,
    line: Option<usize>,
}

/// Compose cycles, dead ends, sources, broken links, and centrality over the same
/// in-scope node set `graph_shape` counts.
fn graph_health_view(
    kb: &KnowledgeBase,
    repo: Option<&str>,
    scope: wire::TypeScope,
    limit: usize,
) -> GraphHealthView {
    let nodes: Vec<&Path> = kb
        .catalog
        .iter()
        .filter(|(_, e)| is_graph_node_kind(e.kind))
        .map(|(p, _)| p.as_path())
        .filter(|p| node_in_scope(kb, repo, scope, p))
        .collect();
    let index: std::collections::HashMap<&Path, usize> =
        nodes.iter().enumerate().map(|(i, p)| (*p, i)).collect();
    let n = nodes.len();

    let mut out_adj: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut in_degree = vec![0usize; n];
    let mut out_degree = vec![0usize; n];
    let mut uf = UnionFind::new(n);
    let mut edges = 0usize;
    // The same edges with direction dropped, and how many parallel edges join
    // each pair: the projection the fragility pass reads.
    let mut undirected_pairs: std::collections::HashMap<(usize, usize), usize> = Default::default();
    for (target, backlinks) in kb.backlinks.iter() {
        let Some(&ti) = index.get(target.as_path()) else {
            continue;
        };
        for edge in backlinks {
            let Some(&si) = index.get(edge.source.as_path()) else {
                continue;
            };
            out_adj[si].push(ti);
            in_degree[ti] += 1;
            out_degree[si] += 1;
            uf.union(si, ti);
            edges += 1;
            if si != ti {
                *undirected_pairs
                    .entry((si.min(ti), si.max(ti)))
                    .or_default() += 1;
            }
        }
    }

    let mut comp_size: std::collections::HashMap<usize, usize> = Default::default();
    for i in 0..n {
        *comp_size.entry(uf.find(i)).or_default() += 1;
    }
    // Instrumentation: one node scan plus one edge walk over the whole in-scope
    // graph, the SCC pass below walks the same adjacency once more, and the
    // fragility pass builds the undirected projection and walks it. The answer is
    // corpus-wide by definition, so the count growing with the corpus is the
    // contract; the counter keeps it visible beside the request count.
    crate::verb_metrics::note_documents((n + edges + edges + edges) as u64);

    let mut isolated = Vec::new();
    let mut dead_ends = Vec::new();
    let mut sources = Vec::new();
    let mut isolated_count = 0usize;
    let mut dead_end_count = 0usize;
    let mut source_count = 0usize;
    let mut central: Vec<HealthNode> = Vec::with_capacity(n);
    for i in 0..n {
        let node = HealthNode {
            path: nodes[i].display().to_string(),
            r#in: in_degree[i],
            out: out_degree[i],
            total: in_degree[i] + out_degree[i],
        };
        if in_degree[i] == 0 && out_degree[i] == 0 {
            isolated_count += 1;
            isolated.push(node);
        } else if out_degree[i] == 0 {
            dead_end_count += 1;
            dead_ends.push(node);
        } else if in_degree[i] == 0 {
            source_count += 1;
            sources.push(node);
        }
        central.push(HealthNode {
            path: nodes[i].display().to_string(),
            r#in: in_degree[i],
            out: out_degree[i],
            total: in_degree[i] + out_degree[i],
        });
    }

    // Directed cycles come from SCCs. Iterative Tarjan: a deep chain of notes
    // must not be able to overflow the stack of the process that serves reads.
    let mut cycles: Vec<CycleView> = Vec::new();
    let mut nodes_in_cycles = 0usize;
    for component in strongly_connected(&out_adj) {
        let self_loop = component.len() == 1 && out_adj[component[0]].contains(&component[0]);
        if component.len() < 2 && !self_loop {
            continue;
        }
        nodes_in_cycles += component.len();
        let mut paths: Vec<String> = component
            .iter()
            .map(|&i| nodes[i].display().to_string())
            .collect();
        paths.sort();
        cycles.push(CycleView {
            length: paths.len(),
            nodes: paths,
        });
    }
    // Shortest cycles first: a two-node loop is the one to break first.
    cycles.sort_by(|a, b| a.length.cmp(&b.length).then_with(|| a.nodes.cmp(&b.nodes)));
    let cycle_count = cycles.len();

    // Fragility of the undirected projection: the same resolved edges with
    // direction dropped. Parallel edges collapse for the structure (removing a
    // node takes all of them), but a pair joined by more than one edge is never
    // a bridge — removing one leaves the other — so those are filtered out.
    let mut undirected: Vec<Vec<usize>> = vec![Vec::new(); n];
    for &(left, right) in undirected_pairs.keys() {
        undirected[left].push(right);
        undirected[right].push(left);
    }
    let (triangles, transitivity) = triangles_and_transitivity(&mut undirected);
    let (bridge_ends, articulations) = bridges_and_articulation(&undirected);
    let mut bridges: Vec<BridgeView> = bridge_ends
        .into_iter()
        .filter(|&(left, right)| {
            undirected_pairs
                .get(&(left.min(right), left.max(right)))
                .copied()
                .unwrap_or(0)
                == 1
        })
        .map(|(left, right)| {
            let (first, second) = if nodes[left] <= nodes[right] {
                (left, right)
            } else {
                (right, left)
            };
            BridgeView {
                a: nodes[first].display().to_string(),
                b: nodes[second].display().to_string(),
            }
        })
        .collect();
    bridges.sort_by(|left, right| left.a.cmp(&right.a).then_with(|| left.b.cmp(&right.b)));
    let bridge_count = bridges.len();
    let articulation_count = articulations.len();
    let mut articulation_points: Vec<HealthNode> = articulations
        .into_iter()
        .map(|node| HealthNode {
            path: nodes[node].display().to_string(),
            r#in: in_degree[node],
            out: out_degree[node],
            total: in_degree[node] + out_degree[node],
        })
        .collect();

    let mut broken: Vec<BrokenLink> = kb
        .diagnostics()
        .filter(|d| is_broken_link_code(d.code.as_str()))
        .filter(|d| node_in_scope(kb, repo, scope, &d.span.file))
        .map(|d| BrokenLink {
            path: d.span.file.display().to_string(),
            code: d.code.as_str().to_owned(),
            message: d.message.clone(),
            line: d.span.line_col.map(|lc| lc.start.line),
        })
        .collect();
    broken.sort_by(|a, b| {
        a.path
            .cmp(&b.path)
            .then_with(|| a.line.unwrap_or(0).cmp(&b.line.unwrap_or(0)))
    });
    let broken_count = broken.len();

    isolated.sort_by(|a, b| a.path.cmp(&b.path));
    articulation_points.sort_by(|a, b| b.total.cmp(&a.total).then_with(|| a.path.cmp(&b.path)));
    dead_ends.sort_by(|a, b| a.total.cmp(&b.total).then_with(|| a.path.cmp(&b.path)));
    sources.sort_by(|a, b| a.total.cmp(&b.total).then_with(|| a.path.cmp(&b.path)));
    central.sort_by(|a, b| b.total.cmp(&a.total).then_with(|| a.path.cmp(&b.path)));

    let truncated = isolated.len() > limit
        || dead_ends.len() > limit
        || sources.len() > limit
        || broken.len() > limit
        || central.len() > limit
        || cycles.len() > limit
        || bridges.len() > limit
        || articulation_points.len() > limit;

    GraphHealthView {
        repo: repo.map(str::to_owned),
        scope: scope.as_str(),
        counts: GraphHealthCounts {
            nodes: n,
            edges,
            components: comp_size.len(),
            isolated: isolated_count,
            dead_ends: dead_end_count,
            sources: source_count,
            cycles: cycle_count,
            nodes_in_cycles,
            broken_links: broken_count,
            bridges: bridge_count,
            articulation_points: articulation_count,
        },
        cycles: cycles.into_iter().take(limit).collect(),
        isolated: isolated.into_iter().take(limit).collect(),
        dead_ends: dead_ends.into_iter().take(limit).collect(),
        sources: sources.into_iter().take(limit).collect(),
        broken_links: broken.into_iter().take(limit).collect(),
        bridges: bridges.into_iter().take(limit).collect(),
        articulation_points: articulation_points.into_iter().take(limit).collect(),
        clustering: ClusteringView {
            triangles,
            transitivity,
        },
        central: central.into_iter().take(limit).collect(),
        truncated,
    }
}

/// The diagnostic codes a reader calls a broken link: a target that does not
/// resolve, resolves ambiguously, or could never resolve from this repo.
fn is_broken_link_code(code: &str) -> bool {
    matches!(
        code,
        "reference-target-missing"
            | "reference-target-ambiguous"
            | "reference-path-escapes-repo"
            | "navigational-target-not-found"
    )
}

/// Triangle count and global transitivity of an undirected simple graph.
///
/// The neighbours are sorted once here, so the edge sweep below can intersect two
/// sorted lists; the sort also leaves the adjacency in the order the Tarjan pass
/// prefers. A triangle is counted once per edge (`left < right`, common neighbour in
/// both lists), so the sweep's total is three per triangle and `triangles` divides
/// it. Transitivity is closed triples over all triples: `3T / Σ C(deg, 2)`, `0.0` when
/// the graph has no triple at all — the one place where a ratio of zero is the answer
/// rather than a division by zero.
fn triangles_and_transitivity(adjacency: &mut [Vec<usize>]) -> (usize, f64) {
    for neighbours in adjacency.iter_mut() {
        neighbours.sort_unstable();
    }
    let mut triangle_edges = 0usize;
    for left in 0..adjacency.len() {
        for &right in &adjacency[left] {
            if right <= left {
                continue;
            }
            let (mut at_left, mut at_right) = (0usize, 0usize);
            while at_left < adjacency[left].len() && at_right < adjacency[right].len() {
                match adjacency[left][at_left].cmp(&adjacency[right][at_right]) {
                    std::cmp::Ordering::Less => at_left += 1,
                    std::cmp::Ordering::Greater => at_right += 1,
                    std::cmp::Ordering::Equal => {
                        triangle_edges += 1;
                        at_left += 1;
                        at_right += 1;
                    }
                }
            }
        }
    }
    let mut triples = 0usize;
    for neighbours in adjacency.iter() {
        let degree = neighbours.len();
        triples += degree * degree.saturating_sub(1) / 2;
    }
    let transitivity = match triples {
        0 => 0.0,
        count => triangle_edges as f64 / count as f64,
    };
    (triangle_edges / 3, transitivity)
}

/// Bridges and articulation points of an undirected simple graph, iterative Tarjan.
///
/// A bridge is an edge whose removal disconnects its ends; an articulation point is
/// a node whose removal disconnects some of its neighbours. The graph must be simple
/// (no duplicate neighbours, no self-loops) — the caller builds it from a multiplicity
/// map, so a pair joined twice is one edge here and its bridge is filtered by the
/// caller, which still holds the count.
fn bridges_and_articulation(adjacency: &[Vec<usize>]) -> (Vec<(usize, usize)>, Vec<usize>) {
    let n = adjacency.len();
    let mut discovered = vec![usize::MAX; n];
    let mut low = vec![0usize; n];
    let mut parent = vec![usize::MAX; n];
    let mut is_articulation = vec![false; n];
    let mut bridges = Vec::new();
    let mut time = 0usize;
    let mut frames: Vec<(usize, usize)> = Vec::new();
    for start in 0..n {
        if discovered[start] != usize::MAX {
            continue;
        }
        discovered[start] = time;
        low[start] = time;
        time += 1;
        let mut root_children = 0usize;
        frames.push((start, 0));
        while let Some(&(node, next_child)) = frames.last() {
            if next_child < adjacency[node].len() {
                frames.last_mut().expect("frame").1 += 1;
                let next = adjacency[node][next_child];
                if next == parent[node] {
                    continue;
                }
                if discovered[next] == usize::MAX {
                    if node == start {
                        root_children += 1;
                    }
                    parent[next] = node;
                    discovered[next] = time;
                    low[next] = time;
                    time += 1;
                    frames.push((next, 0));
                } else {
                    low[node] = low[node].min(discovered[next]);
                }
            } else {
                frames.pop();
                if let Some(&(up, _)) = frames.last() {
                    low[up] = low[up].min(low[node]);
                    if low[node] > discovered[up] {
                        bridges.push((up, node));
                    }
                    if up != start && low[node] >= discovered[up] {
                        is_articulation[up] = true;
                    }
                }
            }
        }
        if root_children > 1 {
            is_articulation[start] = true;
        }
    }
    // A node with no edges is not an articulation point: removing it disconnects
    // nothing, and the root rule above would otherwise mark a lone node with a
    // self-loop-free empty neighbourhood.
    let articulation_points = is_articulation
        .iter()
        .enumerate()
        .filter(|&(node, marked)| *marked && !adjacency[node].is_empty())
        .map(|(node, _)| node)
        .collect();
    (bridges, articulation_points)
}

/// Strongly connected components, iterative Tarjan.
fn strongly_connected(adjacency: &[Vec<usize>]) -> Vec<Vec<usize>> {
    let n = adjacency.len();
    let mut index_of = vec![usize::MAX; n];
    let mut low = vec![0usize; n];
    let mut on_stack = vec![false; n];
    let mut stack: Vec<usize> = Vec::new();
    let mut next_index = 0usize;
    let mut components: Vec<Vec<usize>> = Vec::new();
    // (node, next child position) frames.
    let mut frames: Vec<(usize, usize)> = Vec::new();
    for start in 0..n {
        if index_of[start] != usize::MAX {
            continue;
        }
        frames.push((start, 0));
        while let Some(&(node, child)) = frames.last() {
            if child == 0 {
                index_of[node] = next_index;
                low[node] = next_index;
                next_index += 1;
                stack.push(node);
                on_stack[node] = true;
            }
            if child < adjacency[node].len() {
                frames.last_mut().expect("frame").1 += 1;
                let next = adjacency[node][child];
                if index_of[next] == usize::MAX {
                    frames.push((next, 0));
                } else if on_stack[next] {
                    low[node] = low[node].min(index_of[next]);
                }
            } else {
                frames.pop();
                if let Some(&(parent, _)) = frames.last() {
                    low[parent] = low[parent].min(low[node]);
                }
                if low[node] == index_of[node] {
                    let mut component = Vec::new();
                    while let Some(member) = stack.pop() {
                        on_stack[member] = false;
                        component.push(member);
                        if member == node {
                            break;
                        }
                    }
                    components.push(component);
                }
            }
        }
    }
    components
}

/// The full whole-graph node+edge payload. Nodes are the in-scope content
/// catalog (isolated files included); edges are the resolved backlink index
/// induced on in-scope endpoints. Node ref counts are over the in-scope inbound
/// edges, so a node's `refs_total` equals its inbound-edge count in the same
/// payload. Nodes and edges are sorted for a deterministic result.
fn link_graph_view(
    kb: &KnowledgeBase,
    repo: Option<&str>,
    scope: wire::TypeScope,
) -> LinkGraphView {
    use crate::backlinks::RefSurface;

    // The in-scope content node set, the shared universe with `graph_shape`.
    let node_set: std::collections::HashSet<&Path> = kb
        .catalog
        .iter()
        .filter(|(_, e)| is_graph_node_kind(e.kind))
        .map(|(p, _)| p.as_path())
        .filter(|p| node_in_scope(kb, repo, scope, p))
        .collect();

    let mut refs_structural: std::collections::HashMap<&Path, usize> =
        std::collections::HashMap::new();
    let mut refs_total: std::collections::HashMap<&Path, usize> = std::collections::HashMap::new();
    let mut edges: Vec<GraphEdgeView> = Vec::new();

    for (target, bls) in kb.backlinks.iter() {
        let tp = target.as_path();
        if !node_set.contains(&tp) {
            continue;
        }
        for b in bls {
            // Both endpoints in scope, else the edge is out of the induced graph.
            let sp = b.source.as_path();
            if !node_set.contains(&sp) {
                continue;
            }
            *refs_total.entry(tp).or_default() += 1;
            if b.slot.is_some() {
                *refs_structural.entry(tp).or_default() += 1;
            }
            edges.push(GraphEdgeView {
                from: sp.display().to_string(),
                to: tp.display().to_string(),
                kind: crate::backlinks::coarse_edge_kind(b.surface, b.slot.as_deref()),
                surface: match b.surface {
                    RefSurface::Frontmatter => "frontmatter",
                    RefSurface::Body => "body",
                    RefSurface::Docstring => "docstring",
                },
            });
        }
    }

    let mut nodes: Vec<GraphNodeView> = node_set
        .iter()
        .map(|&p| GraphNodeView {
            path: p.display().to_string(),
            repo: kb.repos.repo_of(p).map(|r| r.name.as_str().to_string()),
            kind: served_file_kind(kb.file_parse(p)),
            refs_structural: refs_structural.get(&p).copied().unwrap_or(0),
            refs_total: refs_total.get(&p).copied().unwrap_or(0),
        })
        .collect();

    nodes.sort_by(|a, b| a.path.cmp(&b.path));
    edges.sort_by(|a, b| {
        a.from
            .cmp(&b.from)
            .then_with(|| a.to.cmp(&b.to))
            .then_with(|| a.kind.cmp(b.kind))
            .then_with(|| a.surface.cmp(b.surface))
    });

    LinkGraphView {
        repo: repo.map(|s| s.to_string()),
        scope: scope.as_str(),
        nodes,
        edges,
    }
}

/// The edge classes a `type_graph` read may request. An unknown value fails
/// deserialization (a malformed-read error frame), never a silent default. The
/// authored forms are `subtype` / `field-type` / `instance-of` / `meta`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum EdgeClass {
    Subtype,
    FieldType,
    InstanceOf,
    Meta,
}

/// The resolved edge-class selection for a `type_graph` fold. Absent `edges`
/// defaults to the type-to-type backbone (`subtype` + `field-type`);
/// `instance-of` and `meta` are opt-in, since `instance-of` pulls every claiming
/// instance in as a node.
#[derive(Debug, Clone, Copy)]
struct EdgeClasses {
    subtype: bool,
    field_type: bool,
    instance_of: bool,
    meta: bool,
}

impl EdgeClasses {
    fn from_arg(edges: Option<Vec<EdgeClass>>) -> Self {
        match edges {
            None => Self {
                subtype: true,
                field_type: true,
                instance_of: false,
                meta: false,
            },
            Some(list) => Self {
                subtype: list.contains(&EdgeClass::Subtype),
                field_type: list.contains(&EdgeClass::FieldType),
                instance_of: list.contains(&EdgeClass::InstanceOf),
                meta: list.contains(&EdgeClass::Meta),
            },
        }
    }
}

/// One node of the `type_graph` payload: a type-def, or an instance when
/// `instance-of` is requested. The node identity is `path`, so the payload
/// merges with `link_graph` by path. Node sizing is the consumer's, derived from
/// the edges it receives (subtype / instance-of in-degree); the reference-graph
/// ref counts stay on `link_graph`, the reference graph's own concern.
#[derive(Debug, Clone, Serialize, PartialEq)]
struct TypeGraphNodeView {
    path: String,
    repo: Option<String>,
    kind: &'static str,
}

/// One edge of the `type_graph` payload. `relation` is the edge class
/// (`subtype` / `field-type` / `instance-of` / `meta`); `count` is the deduped
/// multiplicity, the number of field references to the target folded into one
/// `field-type` edge (an edge weight; a field whose shape names the target twice
/// counts twice). The other relations are single by construction, so `count` is
/// 1.
#[derive(Debug, Clone, Serialize, PartialEq)]
struct TypeGraphEdgeView {
    from: String,
    to: String,
    relation: &'static str,
    count: usize,
}

/// The `type_graph` read result: the schema graph as a node+edge payload, the
/// type-side sibling of `link_graph`. `nodes` are the in-scope type-defs (plus
/// claiming instances when `instance-of` is on); `edges` are the resolved
/// type-graph edges induced on in-scope endpoints. Sorted for a deterministic
/// payload.
#[derive(Debug, Serialize)]
struct TypeGraphView {
    repo: Option<String>,
    scope: &'static str,
    nodes: Vec<TypeGraphNodeView>,
    edges: Vec<TypeGraphEdgeView>,
}

/// Fold the held type graph into a node+edge payload, the type-side sibling of
/// [`link_graph_view`]. Nodes are in-scope type-def files (plus claiming
/// instances when `instance-of` is requested); edges are the resolved
/// subtype / field-type / meta / instance-of relations, induced on in-scope
/// endpoints (a cross-repo edge whose target is out of scope drops, the same
/// rule `link_graph` uses). `field-type` edges dedup across fields into one edge
/// per pair carrying the field count.
fn type_graph_view(
    kb: &KnowledgeBase,
    repo: Option<&str>,
    scope: wire::TypeScope,
    classes: EdgeClasses,
) -> TypeGraphView {
    // The in-scope type-def node universe, the always-present nodes.
    let typedef_paths: std::collections::HashSet<&Path> = kb
        .catalog
        .iter()
        .filter(|(_, e)| matches!(e.kind, tb_parser::FileKind::TypeDef))
        .map(|(p, _)| p.as_path())
        .filter(|p| node_in_scope(kb, repo, scope, p))
        .collect();

    // Resolve a `(name, ::repo)` type reference to a type-def source path: a
    // bare name in the referrer's OWN repo graph, a `::repo` name in the named
    // peer's graph. File-level resolution is unambiguous (a repo owns one type
    // of a name), so the per-repo `TypeGraph` suffices; identity/hash concerns
    // do not bear on which FILE is repo X's type Y.
    let resolve =
        |from_repo: Option<&str>, name: &str, repo_qual: Option<&str>| -> Option<PathBuf> {
            let graph = match repo_qual {
                Some(dep) => kb.graph_for_repo(dep)?,
                None => kb.graph_for_repo(from_repo?)?,
            };
            graph
                .get(&tb_core::TypeName(name.to_string()))
                .map(|d| d.source_path.clone())
        };

    // Edges keyed by (from, to, relation) accumulate a count; a field-type pair
    // reached through several fields folds into one edge weighted by the count.
    let mut edge_counts: std::collections::HashMap<(PathBuf, PathBuf, &'static str), usize> =
        std::collections::HashMap::new();
    let mut instance_nodes: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();

    let mut add_edge = |from: &Path, to: PathBuf, relation: &'static str| {
        *edge_counts
            .entry((from.to_path_buf(), to, relation))
            .or_default() += 1;
    };

    // Type-to-type edges: walk each in-scope type-def once.
    for &path in &typedef_paths {
        let from_repo = kb.repos.repo_of(path).map(|r| r.name.as_str());
        let Some(FileParse::TypeDef {
            type_def: Some(def),
            ..
        }) = kb.file_parse(path)
        else {
            continue;
        };

        if classes.subtype {
            for parent in &def.parents {
                if let Some(tp) = resolve(from_repo, parent.name.as_str(), parent.repo.as_deref()) {
                    if typedef_paths.contains(tp.as_path()) {
                        add_edge(path, tp, "subtype");
                    }
                }
            }
        }
        if classes.field_type {
            for (base, repo_qual) in tb_core::field_type_refs(def) {
                if let Some(tp) = resolve(from_repo, &base, repo_qual.as_deref()) {
                    if typedef_paths.contains(tp.as_path()) {
                        add_edge(path, tp, "field-type");
                    }
                }
            }
        }
        if classes.meta {
            if let Some(blocks) = &def.meta_blocks {
                for mb in blocks {
                    if let Some(tp) = resolve(from_repo, mb.type_name.as_str(), mb.repo.as_deref())
                    {
                        if typedef_paths.contains(tp.as_path()) {
                            add_edge(path, tp, "meta");
                        }
                    }
                }
            }
        }
    }

    // Instance-of edges: each in-scope instance's claims to an in-scope type-def.
    // A claiming instance becomes a node; a claim onto an out-of-scope type drops.
    if classes.instance_of {
        for (path, entry) in kb.catalog.iter() {
            if !matches!(entry.kind, tb_parser::FileKind::Instance) {
                continue;
            }
            if !node_in_scope(kb, repo, scope, path) {
                continue;
            }
            let from_repo = kb.repos.repo_of(path).map(|r| r.name.as_str());
            let Some(FileParse::Instance {
                instance: Some(inst),
                ..
            }) = kb.file_parse(path)
            else {
                continue;
            };
            for claim in inst.type_claim.iter() {
                if let Some(tp) = resolve(from_repo, claim.name.as_str(), claim.repo.as_deref()) {
                    if typedef_paths.contains(tp.as_path()) {
                        add_edge(path, tp, "instance-of");
                        instance_nodes.insert(path.to_path_buf());
                    }
                }
            }
        }
    }

    // Nodes: the type-defs, plus any instance that gained an instance-of edge.
    let mut nodes: Vec<TypeGraphNodeView> = typedef_paths
        .iter()
        .map(|p| p.to_path_buf())
        .chain(instance_nodes.iter().cloned())
        .map(|p| TypeGraphNodeView {
            path: p.display().to_string(),
            repo: kb.repos.repo_of(&p).map(|r| r.name.as_str().to_string()),
            kind: served_file_kind(kb.file_parse(&p)),
        })
        .collect();
    nodes.sort_by(|a, b| a.path.cmp(&b.path));

    let mut edges: Vec<TypeGraphEdgeView> = edge_counts
        .into_iter()
        .map(|((from, to, relation), count)| TypeGraphEdgeView {
            from: from.display().to_string(),
            to: to.display().to_string(),
            relation,
            count,
        })
        .collect();
    edges.sort_by(|a, b| {
        a.from
            .cmp(&b.from)
            .then_with(|| a.to.cmp(&b.to))
            .then_with(|| a.relation.cmp(b.relation))
    });

    TypeGraphView {
        repo: repo.map(|s| s.to_string()),
        scope: scope.as_str(),
        nodes,
        edges,
    }
}

/// The most-referenced files over the typed reference graph, folded from the
/// backlink index, the top `limit`. Per target, inbound edges split by
/// `slot.is_some()` into structural (fills a typed slot) and navigational
/// (prose). Ranked structural desc, then total desc, then path for stability.
/// Each hub carries its kind and owning repo. A file with no inbound edges is
/// absent, the backlink index only holds referenced files.
fn hub_ranking(
    kb: &KnowledgeBase,
    repo: Option<&str>,
    scope: wire::TypeScope,
    limit: usize,
    offset: usize,
) -> Vec<Hub> {
    // One plain local increment per candidate, so counting costs the fold
    // nothing it was not already doing.
    let mut examined = 0u64;
    let mut hubs: Vec<Hub> = kb
        .backlinks
        .iter()
        // Scope BEFORE ranking and truncation, which is the whole point of the
        // filter: `limit` used to cut the workspace-wide ranking first, so a
        // large dependency could fill every slot and crowd the user's own
        // content out of the top-N entirely. Filtering after would return fewer
        // than `limit` own hubs while own hubs existed.
        .filter(|(target, _)| {
            examined += 1;
            node_in_scope(kb, repo, scope, target)
        })
        .map(|(target, edges)| {
            let refs_structural = edges.iter().filter(|b| b.slot.is_some()).count();
            let refs_total = edges.len();
            Hub {
                path: target.display().to_string(),
                repo: kb
                    .repos
                    .repo_of(target)
                    .map(|r| r.name.as_str().to_string()),
                kind: served_file_kind(kb.file_parse(target)),
                refs_structural,
                refs_navigational: refs_total - refs_structural,
                refs_total,
            }
        })
        .collect();
    // Instrumentation: the ranking folds the WHOLE backlink index — one entry per
    // referenced file — then sorts it and pages. `limit` cuts the answer, not this
    // scan, which is why the visit count is the candidates examined.
    crate::verb_metrics::note_documents(examined);
    hubs.sort_by(|a, b| {
        b.refs_structural
            .cmp(&a.refs_structural)
            .then_with(|| b.refs_total.cmp(&a.refs_total))
            .then_with(|| a.path.cmp(&b.path))
    });
    hubs.into_iter().skip(offset).take(limit).collect()
}

#[cfg(test)]
mod hub_ranking_tests {
    use super::*;
    use tb_parser::MemoryFileSystem;

    #[test]
    fn ranks_structural_over_navigational_and_excludes_leaves() {
        let mut fs = MemoryFileSystem::new();
        fs.insert("/v/.arsumbris/repo.yaml", b"name: v\n".to_vec());
        fs.insert(
            "/v/type/note.type.yaml",
            b"fields:\n  title: String\n  rel: note*\n".to_vec(),
        );
        // A structural hub: an instance many others point their `rel` slot at.
        fs.insert("/v/hub.md", b"---\ntype: note\ntitle: hub\n---\n".to_vec());
        // A navigational hub: a plain note many others prose-link.
        fs.insert("/v/moc.md", b"# moc\n".to_vec());
        // A leaf: an instance nobody references.
        fs.insert(
            "/v/leaf.md",
            b"---\ntype: note\ntitle: leaf\n---\n".to_vec(),
        );
        for i in 0..5 {
            // A frontmatter wikilink is a quoted string; unquoted `[[hub]]` is a
            // YAML nested sequence, not a reference.
            let body =
                format!("---\ntype: note\ntitle: r{i}\nrel: \"[[hub]]\"\n---\n\nsee [[moc]].\n");
            fs.insert(format!("/v/ref-{i}.md"), body.into_bytes());
        }
        let kb = crate::build::build(Path::new("/v"), &fs).unwrap();

        let hubs = hub_ranking(&kb, None, wire::TypeScope::all(), 10, 0);

        // Structural hub ranks first, five `rel` edges, no prose edges, an instance.
        let hub = &hubs[0];
        assert!(
            hub.path.ends_with("/hub.md"),
            "structural hub first: {}",
            hub.path
        );
        assert_eq!(hub.kind, "instance");
        assert_eq!(hub.refs_structural, 5);
        assert_eq!(hub.refs_navigational, 0);
        assert_eq!(hub.repo.as_deref(), Some("v"));

        // The MOC ranks below it, five prose edges, no structural, a note.
        let moc_pos = hubs
            .iter()
            .position(|h| h.path.ends_with("/moc.md"))
            .unwrap();
        assert!(
            moc_pos > 0,
            "navigational hub ranks below the structural one"
        );
        let moc = &hubs[moc_pos];
        assert_eq!(moc.kind, "note");
        assert_eq!(moc.refs_structural, 0);
        assert_eq!(moc.refs_navigational, 5);

        // A file with no inbound edges never appears.
        assert!(!hubs.iter().any(|h| h.path.ends_with("/leaf.md")));
    }

    #[test]
    fn truncates_to_the_limit() {
        let mut fs = MemoryFileSystem::new();
        fs.insert("/v/.arsumbris/repo.yaml", b"name: v\n".to_vec());
        // Ten notes each prose-linking the next, so ten targets have one edge.
        for i in 0..10 {
            let next = (i + 1) % 10;
            fs.insert(
                format!("/v/n-{i}.md"),
                format!("see [[n-{next}]].\n").into_bytes(),
            );
        }
        let kb = crate::build::build(Path::new("/v"), &fs).unwrap();
        assert_eq!(
            hub_ranking(&kb, None, wire::TypeScope::all(), 3, 0).len(),
            3,
            "top-N is bounded by the limit"
        );
    }
}

#[cfg(test)]
mod graph_shape_tests {
    use super::*;
    use tb_parser::MemoryFileSystem;

    /// A vault of plain notes with a known shape:
    /// - `a → b`, `a → c` (one island of three).
    /// - `d → e` (a second island of two).
    /// - `lonely` (nothing, either way — a singleton island).
    /// - `out → a` (points in, unreferenced — a `no_inbound` orphan that is NOT
    ///   `isolated`, and it joins the first island).
    fn known_shape_fs() -> MemoryFileSystem {
        let mut fs = MemoryFileSystem::new();
        fs.insert("/v/.arsumbris/repo.yaml", b"name: v\n".to_vec());
        fs.insert("/v/a.md", b"see [[b]] and [[c]].\n".to_vec());
        fs.insert("/v/b.md", b"# b\n".to_vec());
        fs.insert("/v/c.md", b"# c\n".to_vec());
        fs.insert("/v/d.md", b"see [[e]].\n".to_vec());
        fs.insert("/v/e.md", b"# e\n".to_vec());
        fs.insert("/v/lonely.md", b"# lonely\n".to_vec());
        fs.insert("/v/out.md", b"see [[a]].\n".to_vec());
        fs
    }

    #[test]
    fn folds_a_known_shape() {
        let kb = crate::build::build(Path::new("/v"), &known_shape_fs()).unwrap();
        let s = graph_shape_view(&kb, None, wire::TypeScope::all(), true);

        // The 7 content notes; `.arsumbris/repo.yaml` is engine-schema, not a node.
        assert_eq!(s.node_count, 7);
        // Three islands: {a,b,c,out}, {d,e}, {lonely}.
        assert_eq!(s.components, 3);
        assert_eq!(s.largest_component, 4);
        // Four navigational edges: a→b, a→c, d→e, out→a. No typed slots here.
        assert_eq!(s.edge_count, 4);
        assert_eq!(s.edges_structural, 0);
        assert_eq!(s.edges_navigational, 4);
        // `no_inbound`: d, lonely, out. `isolated` (also no outbound): lonely only.
        assert_eq!(s.orphans.no_inbound, 3);
        assert_eq!(s.orphans.isolated, 1);
        assert_eq!((s.density * 1000.0).round(), 571.0, "4 edges / 7 nodes");

        // The path list tags each orphan by its kind, sorted by path.
        let paths = s.orphans.paths.expect("orphan_paths requested");
        let by_path: std::collections::HashMap<&str, &str> =
            paths.iter().map(|o| (o.path.as_str(), o.kind)).collect();
        assert_eq!(by_path.get("/v/lonely.md"), Some(&"isolated"));
        assert_eq!(by_path.get("/v/d.md"), Some(&"no_inbound"));
        assert_eq!(by_path.get("/v/out.md"), Some(&"no_inbound"));

        // In-degree histogram: 3 nodes with 0 inbound, 4 with 1.
        assert_eq!(
            s.degree.inbound,
            vec![
                DegreeBucket {
                    degree: 0,
                    count: 3
                },
                DegreeBucket {
                    degree: 1,
                    count: 4
                },
            ]
        );
    }

    #[test]
    fn orphan_paths_flag_gates_the_list() {
        let kb = crate::build::build(Path::new("/v"), &known_shape_fs()).unwrap();
        let s = graph_shape_view(&kb, None, wire::TypeScope::all(), false);
        // Counts are always present; the unbounded path list is opt-in.
        assert_eq!(s.orphans.no_inbound, 3);
        assert_eq!(s.orphans.isolated, 1);
        assert!(s.orphans.paths.is_none());
    }

    #[test]
    fn splits_structural_from_navigational_edges() {
        let mut fs = MemoryFileSystem::new();
        fs.insert("/v/.arsumbris/repo.yaml", b"name: v\n".to_vec());
        fs.insert(
            "/v/type/note.type.yaml",
            b"fields:\n  title: String\n  rel: note*\n".to_vec(),
        );
        fs.insert("/v/hub.md", b"---\ntype: note\ntitle: hub\n---\n".to_vec());
        // One structural edge (the `rel` slot) and one navigational edge (prose),
        // both pointing at hub.
        fs.insert(
            "/v/r.md",
            b"---\ntype: note\ntitle: r\nrel: \"[[hub]]\"\n---\n\nsee [[hub]].\n".to_vec(),
        );
        let kb = crate::build::build(Path::new("/v"), &fs).unwrap();
        let s = graph_shape_view(&kb, None, wire::TypeScope::all(), false);
        assert_eq!(s.edges_structural, 1, "the rel slot edge");
        assert_eq!(s.edges_navigational, 1, "the prose edge");
        assert_eq!(s.edge_count, 2);
    }

    #[test]
    fn link_graph_carries_every_content_node_and_resolved_edge() {
        let kb = crate::build::build(Path::new("/v"), &known_shape_fs()).unwrap();
        let g = link_graph_view(&kb, None, wire::TypeScope::all());

        // The 7 content notes, isolated ones included; repo.yaml is not a node.
        assert_eq!(g.nodes.len(), 7);
        let node = |p: &str| g.nodes.iter().find(|n| n.path == p).expect("node present");
        // The isolated node is present with zero refs.
        let lonely = node("/v/lonely.md");
        assert_eq!((lonely.refs_structural, lonely.refs_total), (0, 0));
        assert_eq!(lonely.kind, "note");
        // `a` is pointed at once (by out), navigationally.
        assert_eq!(node("/v/a.md").refs_total, 1);
        assert_eq!(node("/v/a.md").refs_structural, 0);

        // Four resolved edges, all body/navigational, both endpoints content.
        assert_eq!(g.edges.len(), 4);
        assert!(
            g.edges
                .iter()
                .all(|e| e.surface == "body" && e.kind == "navigational")
        );
        assert!(
            g.edges
                .iter()
                .any(|e| e.from == "/v/out.md" && e.to == "/v/a.md")
        );
        // Deterministic: sorted by (from, to, ...).
        let mut sorted = g.edges.clone();
        sorted.sort_by(|a, b| a.from.cmp(&b.from).then_with(|| a.to.cmp(&b.to)));
        assert_eq!(g.edges, sorted);
    }

    #[test]
    fn link_graph_edge_kinds_match_references_in() {
        let mut fs = MemoryFileSystem::new();
        fs.insert("/v/.arsumbris/repo.yaml", b"name: v\n".to_vec());
        fs.insert(
            "/v/type/note.type.yaml",
            b"fields:\n  title: String\n  rel: note*\n".to_vec(),
        );
        fs.insert("/v/hub.md", b"---\ntype: note\ntitle: hub\n---\n".to_vec());
        fs.insert(
            "/v/r.md",
            b"---\ntype: note\ntitle: r\nrel: \"[[hub]]\"\n---\n\nsee [[hub]].\n".to_vec(),
        );
        let kb = crate::build::build(Path::new("/v"), &fs).unwrap();
        let g = link_graph_view(&kb, None, wire::TypeScope::all());

        // The frontmatter `rel` edge is `field` / `frontmatter`; the prose edge
        // is `navigational` / `body`. hub carries both.
        let hub = g.nodes.iter().find(|n| n.path == "/v/hub.md").unwrap();
        assert_eq!((hub.refs_structural, hub.refs_total), (1, 2));
        assert!(
            g.edges
                .iter()
                .any(|e| e.to == "/v/hub.md" && e.kind == "field" && e.surface == "frontmatter")
        );
        assert!(
            g.edges
                .iter()
                .any(|e| e.to == "/v/hub.md" && e.kind == "navigational" && e.surface == "body")
        );
    }

    /// Two catalogs differing by one added prose link `a → b`.
    fn before_after() -> (LinkGraphView, LinkGraphView) {
        let mut before = MemoryFileSystem::new();
        before.insert("/v/.arsumbris/repo.yaml", b"name: v\n".to_vec());
        before.insert("/v/a.md", b"# a\n".to_vec());
        before.insert("/v/b.md", b"# b\n".to_vec());
        let kb1 = crate::build::build(Path::new("/v"), &before).unwrap();

        let mut after = MemoryFileSystem::new();
        after.insert("/v/.arsumbris/repo.yaml", b"name: v\n".to_vec());
        after.insert("/v/a.md", b"see [[b]].\n".to_vec());
        after.insert("/v/b.md", b"# b\n".to_vec());
        let kb2 = crate::build::build(Path::new("/v"), &after).unwrap();

        (
            link_graph_view(&kb1, None, wire::TypeScope::all()),
            link_graph_view(&kb2, None, wire::TypeScope::all()),
        )
    }

    #[test]
    fn delta_reports_the_added_edge_and_the_re_referenced_node() {
        let (before, after) = before_after();
        let d = link_graph_delta(&before, &after);

        // The added edge.
        assert_eq!(d.edges_added.len(), 1);
        assert_eq!(
            (d.edges_added[0].from.as_str(), d.edges_added[0].to.as_str()),
            ("/v/a.md", "/v/b.md")
        );
        assert!(d.edges_removed.is_empty());

        // b's record changed (0 → 1 inbound), so it re-emits; a is unchanged.
        let added_paths: Vec<&str> = d.nodes_added.iter().map(|n| n.path.as_str()).collect();
        assert_eq!(added_paths, vec!["/v/b.md"]);
        assert_eq!(d.nodes_added[0].refs_total, 1);
        assert!(d.nodes_removed.is_empty());
    }

    #[test]
    fn delta_is_empty_when_the_graph_is_unchanged() {
        let (_before, after) = before_after();
        assert!(link_graph_delta(&after, &after).is_empty());
    }

    #[test]
    fn edge_multiset_diff_is_multiplicity_correct() {
        let e = |from: &str| GraphEdgeView {
            from: from.to_string(),
            to: "/v/t.md".to_string(),
            kind: "navigational",
            surface: "body",
        };
        // Two identical edges dropped to one yields exactly one removal.
        let (added, removed) = edge_multiset_diff(&[e("/v/s.md"), e("/v/s.md")], &[e("/v/s.md")]);
        assert!(added.is_empty());
        assert_eq!(removed.len(), 1);
        // The mirror: one grown to two yields one addition.
        let (added, removed) = edge_multiset_diff(&[e("/v/s.md")], &[e("/v/s.md"), e("/v/s.md")]);
        assert_eq!(added.len(), 1);
        assert!(removed.is_empty());
    }
}

/// The basename of a path as a string, `None` for the hidden (dot-prefixed)
/// entries the repo-files contract excludes.
fn entry_name(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_string_lossy().to_string();
    if name.starts_with('.') {
        None
    } else {
        Some(name)
    }
}

/// Read one length-prefixed frame off an async reader: a 4-byte big-endian
/// length, then that many bytes. `None` at a clean EOF before a frame. Generic
/// over the reader so it serves both a whole stream and a split read half.
async fn read_frame_async<R: tokio::io::AsyncRead + Unpin>(
    stream: &mut R,
) -> std::io::Result<Option<Vec<u8>>> {
    let mut len_buf = [0u8; 4];
    match stream.read_exact(&mut len_buf).await {
        Ok(_) => {}
        Err(e) if e.kind() == ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > MAX_FRAME {
        return Err(std::io::Error::new(
            ErrorKind::InvalidData,
            format!("frame length {len} exceeds maximum {MAX_FRAME}"),
        ));
    }
    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf).await?;
    Ok(Some(buf))
}

/// Write one length-prefixed frame to an async writer.
async fn write_frame_async<W: tokio::io::AsyncWrite + Unpin>(
    stream: &mut W,
    bytes: &[u8],
) -> std::io::Result<()> {
    let len = bytes.len() as u32;
    stream.write_all(&len.to_be_bytes()).await?;
    stream.write_all(bytes).await?;
    stream.flush().await
}

/// Read one length-prefixed frame off the sync client socket. `None` at a clean
/// EOF before a frame.
fn read_frame(stream: &mut UnixStream) -> std::io::Result<Option<Vec<u8>>> {
    let mut len_buf = [0u8; 4];
    match stream.read_exact(&mut len_buf) {
        Ok(()) => {}
        Err(e) if e.kind() == ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > MAX_FRAME {
        return Err(std::io::Error::new(
            ErrorKind::InvalidData,
            format!("frame length {len} exceeds maximum {MAX_FRAME}"),
        ));
    }
    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf)?;
    Ok(Some(buf))
}

/// Write one length-prefixed frame to the sync client socket.
fn write_frame(stream: &mut UnixStream, bytes: &[u8]) -> std::io::Result<()> {
    let len = bytes.len() as u32;
    stream.write_all(&len.to_be_bytes())?;
    stream.write_all(bytes)?;
    stream.flush()
}

/// How long a request-reply [`Client::query`] waits for its reply before giving
/// up. A wedged or unreachable daemon must surface as a timeout error, never an
/// infinite freeze. Generous, so a legitimately slow reply (a large workspace's
/// resolve) still lands; it is a safety net, not a latency budget.
/// Subscriptions opt out, they idle between pushed events.
const CLIENT_QUERY_TIMEOUT: Duration = Duration::from_secs(30);

/// A short-lived client for the framed wire. For driving and testing the
/// endpoint from another process or thread.
pub struct Client {
    stream: UnixStream,
}

impl Client {
    /// Connect to a served socket.
    pub fn connect(socket: impl AsRef<Path>) -> std::io::Result<Client> {
        Ok(Client {
            stream: UnixStream::connect(socket)?,
        })
    }

    /// Send a request (a JSON value naming a `read`) and read the response.
    ///
    /// The reply read is bounded by [`CLIENT_QUERY_TIMEOUT`]: a wedged daemon
    /// surfaces as a timeout error rather than parking this thread forever.
    /// The bound is scoped to this exchange, so a later subscription `recv` on
    /// the same client stays unbounded.
    pub fn query(&mut self, request: &serde_json::Value) -> std::io::Result<serde_json::Value> {
        self.send(request)?;
        self.stream.set_read_timeout(Some(CLIENT_QUERY_TIMEOUT))?;
        let received = self.recv();
        let _ = self.stream.set_read_timeout(None);
        received?.ok_or_else(|| std::io::Error::new(ErrorKind::UnexpectedEof, "server closed"))
    }

    /// Send one request frame without waiting for a reply. For a `subscribe`,
    /// whose replies are a stream of frames drained with [`Client::recv`].
    pub fn send(&mut self, request: &serde_json::Value) -> std::io::Result<()> {
        let bytes = serde_json::to_vec(request)?;
        write_frame(&mut self.stream, &bytes)
    }

    /// Read the next frame from the socket, blocking until one arrives. `None`
    /// at a clean EOF.
    pub fn recv(&mut self) -> std::io::Result<Option<serde_json::Value>> {
        match read_frame(&mut self.stream)? {
            Some(frame) => Ok(Some(serde_json::from_slice(&frame)?)),
            None => Ok(None),
        }
    }
}

/// The multi-repo saga over synthetic per-repo plans. No wire driver produces an
/// N>1 mutation yet, so these exercise `apply_saga` directly over temp git repos.
/// Three repos, not two: only N=3 produces a mid-saga failure with more than one
/// member on each side of the compensation split (revert the committed, restore
/// the written-not-committed).
#[cfg(test)]
mod content_read_tests {
    use super::*;
    use std::fs;
    use std::process::Command;
    use tempfile::TempDir;

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?} failed");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// Off a git working tree, `commit` is present-but-null, never absent.
    #[test]
    fn content_at_commit_is_null_off_git() {
        let dir = TempDir::new().unwrap();
        let plain = dir.path().join("note.md");
        fs::write(&plain, "hi\n").unwrap();

        let value = read_content_at(&plain).unwrap();
        assert!(value.get("commit").is_some(), "commit field present");
        assert!(value["commit"].is_null(), "commit null off git");
    }

    /// In a git working tree, `commit` is HEAD of the owning repo.
    #[test]
    fn content_at_commit_is_head_in_a_git_repo() {
        let dir = TempDir::new().unwrap();
        let p = dir.path();
        git(p, &["init", "-q", "-b", "main"]);
        git(p, &["config", "user.name", "Tester"]);
        git(p, &["config", "user.email", "tester@example.com"]);
        fs::write(p.join("note.md"), "hi\n").unwrap();
        git(p, &["add", "."]);
        git(p, &["commit", "-q", "-m", "seed"]);
        let head = git(p, &["rev-parse", "HEAD"]);

        let plain = read_content_at(&p.join("note.md")).unwrap();
        assert_eq!(plain["commit"].as_str().unwrap(), head);
    }
}

#[cfg(test)]
mod saga_tests {
    use super::*;
    use crate::gitwriter::GitWriteError;
    use crate::repo::RepoName;
    use std::fs;
    use std::process::Command;
    use tempfile::TempDir;

    /// A temp git repo with one seed commit, so HEAD exists.
    fn init_repo() -> TempDir {
        let dir = TempDir::new().unwrap();
        let p = dir.path();
        run(p, &["init", "-q", "-b", "main"]);
        run(p, &["config", "user.name", "Tester"]);
        run(p, &["config", "user.email", "tester@example.com"]);
        fs::write(p.join("seed.md"), "seed\n").unwrap();
        run(p, &["add", "."]);
        run(p, &["commit", "-q", "-m", "seed"]);
        dir
    }

    fn run(repo: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// A git member touching one path, `f.md`.
    fn member(root: &Path, name: &str) -> SagaMember {
        SagaMember {
            tree_root: root.to_path_buf(),
            repo_names: vec![RepoName(name.to_string())],
            paths: vec![PathBuf::from("f.md")],
            git: true,
        }
    }

    /// The monorepo shape, run through the real grouping: N daemon-owned repos
    /// living in subdirectories of ONE git working tree, each touching its own
    /// `f.md`.
    ///
    /// Deliberately built as the per-repo map `resolve_saga_plan` produces and
    /// then coalesced, so a regression in the grouping shows up here rather than
    /// being assumed away by hand-building the answer.
    fn nested_members(tree: &Path, subs: &[&str]) -> Vec<SagaMember> {
        let mut by_repo: BTreeMap<PathBuf, (RepoName, Vec<PathBuf>)> = BTreeMap::new();
        for sub in subs {
            by_repo.insert(
                tree.join(sub),
                (RepoName(sub.to_string()), vec![tree.join(sub).join("f.md")]),
            );
        }
        coalesce_by_working_tree(by_repo)
    }

    /// Seed a tracked `f.md` under each subdirectory and commit it, so a
    /// later restore-to-HEAD has an original to return to.
    fn seed_nested(tree: &Path, subs: &[&str]) {
        for sub in subs {
            fs::create_dir_all(tree.join(sub)).unwrap();
            fs::write(tree.join(sub).join("f.md"), "seed\n").unwrap();
        }
        run(tree, &["add", "."]);
        run(tree, &["commit", "-q", "-m", "seed nested"]);
    }

    /// How many commits in `tree` carry this `Mutation-Id`.
    fn commits_carrying(tree: &Path, mutation_id: &str) -> usize {
        let log = run(
            tree,
            &[
                "log",
                "--format=%H",
                &format!("--grep=^Mutation-Id: {mutation_id}$"),
            ],
        );
        log.lines().filter(|l| !l.trim().is_empty()).count()
    }

    /// A `GitWriter` that delegates to real git but fails `commit` for one
    /// chosen repo, so a mid-saga failure can be forced while the asserted git
    /// state stays real.
    struct FailAt {
        fail_repo: PathBuf,
    }

    impl GitWriter for FailAt {
        fn commit(
            &self,
            repo: &Path,
            paths: &[PathBuf],
            message: &CommitMessage,
        ) -> Result<CommitSha, GitWriteError> {
            if repo == self.fail_repo {
                return Err(GitWriteError::new("forced commit failure"));
            }
            ShellGit.commit(repo, paths, message)
        }
        fn revert_commit(&self, repo: &Path, id: &MutationId) -> Result<(), GitWriteError> {
            ShellGit.revert_commit(repo, id)
        }
        fn restore_paths(&self, repo: &Path, paths: &[PathBuf]) -> Result<(), GitWriteError> {
            ShellGit.restore_paths(repo, paths)
        }
        fn is_clean(&self, repo: &Path, paths: &[PathBuf]) -> Result<bool, GitWriteError> {
            ShellGit.is_clean(repo, paths)
        }
        fn ignored_untracked(
            &self,
            repo: &Path,
            paths: &[PathBuf],
        ) -> Result<std::collections::BTreeSet<PathBuf>, GitWriteError> {
            ShellGit.ignored_untracked(repo, paths)
        }
    }

    #[test]
    fn three_repo_saga_commits_all_with_one_mutation_id() {
        let (a, b, c) = (init_repo(), init_repo(), init_repo());
        let plan = SagaPlan {
            attribution: Vec::new(),
            dirs: DirJournal::default(),
            unclassified: None,
            not_written: Default::default(),
            outside_git: Default::default(),
            mutation_id: "m-saga-1".to_string(),
            members: vec![
                member(a.path(), "ra"),
                member(b.path(), "rb"),
                member(c.path(), "rc"),
            ],
        };
        let roots: Vec<PathBuf> = vec![a.path().into(), b.path().into(), c.path().into()];
        let marker = TempDir::new().unwrap();
        let open_during_write = std::cell::Cell::new(false);
        let (_, commits) = apply_saga(
            &ShellGit,
            marker.path(),
            &plan,
            "test saga".to_string(),
            false,
            || {
                // The marker opens the saga before any tree is touched.
                open_during_write.set(marker.path().join(INTENT_MARKER_NAME).exists());
                for r in &roots {
                    fs::write(r.join("f.md"), "content\n").unwrap();
                }
                Ok::<(), MutationReject>(())
            },
        )
        .expect("the saga commits all members");
        assert_eq!(commits.len(), 3, "one commit per member");
        assert!(
            open_during_write.get(),
            "the intent marker was not open while the trees were written"
        );
        // The intent marker is removed once the saga settles to commit-all.
        assert!(
            !marker.path().join(INTENT_MARKER_NAME).exists(),
            "the intent marker was not closed after a successful saga"
        );
        for (dir, name) in [(&a, "ra"), (&b, "rb"), (&c, "rc")] {
            let body = run(dir.path(), &["log", "-1", "--format=%B"]);
            assert!(
                body.contains("Mutation-Id: m-saga-1"),
                "{name} body: {body}"
            );
            // The member set is identical and name-sorted across every commit.
            assert!(
                body.contains("Mutation-Members: ra, rb, rc"),
                "{name} body: {body}"
            );
            assert!(
                ShellGit
                    .is_clean(dir.path(), &[PathBuf::from("f.md")])
                    .unwrap(),
                "{name} left dirty"
            );
            assert!(dir.path().join("f.md").exists(), "{name} missing f.md");
        }
    }

    /// A member touching `f.md` (written) and `g.md` (in the set, not written),
    /// with `g.md` committed then dirtied by a human. The `not_written` shape a
    /// refactor gives a commit-pinned referrer.
    fn member_with_unwritten_dirty(root: &Path, name: &str) -> SagaMember {
        fs::write(root.join("g.md"), "committed\n").unwrap();
        run(root, &["add", "g.md"]);
        run(root, &["commit", "-q", "-m", "g"]);
        fs::write(root.join("g.md"), "human edit\n").unwrap();
        SagaMember {
            tree_root: root.to_path_buf(),
            repo_names: vec![RepoName(name.to_string())],
            paths: vec![PathBuf::from("f.md"), PathBuf::from("g.md")],
            git: true,
        }
    }

    #[test]
    fn the_pre_touch_guard_passes_a_clean_path_and_rejects_a_drifted_one() {
        let a = init_repo();
        let seed = a.path().join("seed.md");
        guard_unchanged_before_touch(&seed, None).expect("a clean path passes");
        // A human edit after the sweep: the move or delete must not carry it.
        fs::write(&seed, "human edit\n").unwrap();
        let err = guard_unchanged_before_touch(&seed, None).unwrap_err();
        assert!(
            err.message
                .contains("changed since the mutation was planned"),
            "{}",
            err.message
        );
    }

    #[test]
    fn the_pre_touch_guard_compares_an_engine_rewritten_path_to_its_bytes() {
        let a = init_repo();
        let seed = a.path().join("seed.md");
        // The engine rewrote it (a self-reference): dirty at HEAD by design, so
        // the baseline is the bytes it wrote, not HEAD.
        fs::write(&seed, "engine rewrite\n").unwrap();
        guard_unchanged_before_touch(&seed, Some("engine rewrite\n"))
            .expect("the engine's own write is the baseline");
        fs::write(&seed, "human edit on top\n").unwrap();
        assert!(guard_unchanged_before_touch(&seed, Some("engine rewrite\n")).is_err());
    }

    #[test]
    fn the_pre_touch_guard_passes_a_path_under_no_working_tree() {
        let dir = TempDir::new().unwrap();
        let f = dir.path().join("f.md");
        fs::write(&f, "anything\n").unwrap();
        guard_unchanged_before_touch(&f, None).expect("off-git has no clean precondition");
    }

    #[test]
    fn the_batched_guard_checks_every_path_once_and_names_the_drifted_one() {
        let (a, b) = (init_repo(), init_repo());
        for r in [&a, &b] {
            fs::create_dir_all(r.path().join("d")).unwrap();
            for f in ["d/x.md", "d/y.md"] {
                fs::write(r.path().join(f), "x\n").unwrap();
            }
            run(r.path(), &["add", "."]);
            run(r.path(), &["commit", "-q", "-m", "d"]);
        }
        let off = TempDir::new().unwrap();
        fs::write(off.path().join("z.md"), "off-git\n").unwrap();
        let rewritten = a.path().join("seed.md");
        fs::write(&rewritten, "engine rewrite\n").unwrap();
        let paths = |a: &Path, b: &Path| {
            vec![
                a.join("d/x.md"),
                a.join("d/y.md"),
                b.join("d/x.md"),
                b.join("d/y.md"),
                off.path().join("z.md"),
            ]
        };
        let all = paths(a.path(), b.path());
        let mut guarded: Vec<(&Path, Option<&str>)> =
            all.iter().map(|p| (p.as_path(), None)).collect();
        guarded.push((rewritten.as_path(), Some("engine rewrite\n")));
        // Clean across two trees, an off-git path, and the engine's own rewrite.
        guard_unchanged_before_touch_all(&guarded).expect("everything is as planned");

        // One human edit among many: rejected, the drifted file named.
        fs::write(b.path().join("d/y.md"), "human edit\n").unwrap();
        let err = guard_unchanged_before_touch_all(&guarded).unwrap_err();
        assert!(err.message.contains("d/y.md"), "{}", err.message);
        assert!(
            err.message
                .contains("changed since the mutation was planned"),
            "{}",
            err.message
        );

        // An edit on top of the engine's rewrite is caught by its byte compare.
        run(b.path(), &["checkout", "--", "d/y.md"]);
        fs::write(&rewritten, "human edit on top\n").unwrap();
        assert!(guard_unchanged_before_touch_all(&guarded).is_err());
    }

    #[test]
    fn saga_never_commits_a_path_it_does_not_write() {
        let a = init_repo();
        let plan = SagaPlan {
            attribution: Vec::new(),
            dirs: DirJournal::default(),
            unclassified: None,
            not_written: [a.path().join("g.md")].into_iter().collect(),
            outside_git: Default::default(),
            mutation_id: "m-unwritten-commit".to_string(),
            members: vec![member_with_unwritten_dirty(a.path(), "ra")],
        };
        let marker = TempDir::new().unwrap();
        apply_saga(
            &ShellGit,
            marker.path(),
            &plan,
            "s".to_string(),
            false,
            || {
                fs::write(a.path().join("f.md"), "content\n").unwrap();
                Ok::<(), MutationReject>(())
            },
        )
        .expect("a dirty path the saga does not write does not block it");
        let changed = run(a.path(), &["show", "--name-only", "--format=", "HEAD"]);
        assert_eq!(changed, "f.md", "the mutation's commit swept in: {changed}");
        assert_eq!(
            fs::read_to_string(a.path().join("g.md")).unwrap(),
            "human edit\n",
            "the human's uncommitted edit was lost"
        );
        assert!(
            !ShellGit
                .is_clean(a.path(), &[PathBuf::from("g.md")])
                .unwrap(),
            "the human's edit should still be uncommitted"
        );
    }

    #[test]
    fn saga_rollback_never_restores_a_path_it_does_not_write() {
        let a = init_repo();
        let plan = SagaPlan {
            attribution: Vec::new(),
            dirs: DirJournal::default(),
            unclassified: None,
            not_written: [a.path().join("g.md")].into_iter().collect(),
            outside_git: Default::default(),
            mutation_id: "m-unwritten-restore".to_string(),
            members: vec![member_with_unwritten_dirty(a.path(), "ra")],
        };
        let marker = TempDir::new().unwrap();
        let git = FailAt {
            fail_repo: a.path().to_path_buf(),
        };
        let res = apply_saga(&git, marker.path(), &plan, "s".to_string(), false, || {
            fs::write(a.path().join("f.md"), "content\n").unwrap();
            Ok::<(), MutationReject>(())
        });
        assert!(res.is_err(), "the forced commit failure rejects");
        assert!(
            !a.path().join("f.md").exists(),
            "the written path was not rolled back"
        );
        assert_eq!(
            fs::read_to_string(a.path().join("g.md")).unwrap(),
            "human edit\n",
            "the rollback discarded the human's uncommitted edit"
        );
    }

    #[test]
    fn saga_skips_a_member_it_writes_nothing_in() {
        let (a, b) = (init_repo(), init_repo());
        // b touches only g.md, which the mutation does not write: no commit, no
        // whole-tree pathspec, its dirty edit untouched.
        let mut mb = member_with_unwritten_dirty(b.path(), "rb");
        mb.paths = vec![PathBuf::from("g.md")];
        let plan = SagaPlan {
            attribution: Vec::new(),
            dirs: DirJournal::default(),
            unclassified: None,
            not_written: [b.path().join("g.md")].into_iter().collect(),
            outside_git: Default::default(),
            mutation_id: "m-unwritten-member".to_string(),
            members: vec![member(a.path(), "ra"), mb],
        };
        let marker = TempDir::new().unwrap();
        let (_, commits) = apply_saga(
            &ShellGit,
            marker.path(),
            &plan,
            "s".to_string(),
            false,
            || {
                fs::write(a.path().join("f.md"), "content\n").unwrap();
                Ok::<(), MutationReject>(())
            },
        )
        .expect("the saga commits the written member");
        assert_eq!(commits.len(), 1, "only the written member commits");
        assert_eq!(commits_carrying(b.path(), "m-unwritten-member"), 0);
        assert_eq!(
            fs::read_to_string(b.path().join("g.md")).unwrap(),
            "human edit\n"
        );
    }

    #[test]
    fn three_repo_saga_failure_compensates_every_member() {
        let (a, b, c) = (init_repo(), init_repo(), init_repo());
        let plan = SagaPlan {
            attribution: Vec::new(),
            dirs: DirJournal::default(),
            unclassified: None,
            not_written: Default::default(),
            outside_git: Default::default(),
            mutation_id: "m-saga-2".to_string(),
            members: vec![
                member(a.path(), "ra"),
                member(b.path(), "rb"),
                member(c.path(), "rc"),
            ],
        };
        let roots: Vec<PathBuf> = vec![a.path().into(), b.path().into(), c.path().into()];
        // Fail the middle member's commit: ra commits, rb fails, rc is written
        // but never reached for commit.
        let git = FailAt {
            fail_repo: b.path().to_path_buf(),
        };
        let marker = TempDir::new().unwrap();
        let result = apply_saga(
            &git,
            marker.path(),
            &plan,
            "test saga".to_string(),
            false,
            || {
                for r in &roots {
                    fs::write(r.join("f.md"), "content\n").unwrap();
                }
                Ok::<(), MutationReject>(())
            },
        );
        assert!(
            result.is_err(),
            "the saga rejects on the middle member's commit failure"
        );
        // The intent marker is removed once the saga settles to compensate-all.
        assert!(
            !marker.path().join(INTENT_MARKER_NAME).exists(),
            "the intent marker was not closed after a compensated saga"
        );
        // Every member is rolled back: the created f.md is gone, the tree clean.
        for (dir, name) in [(&a, "ra"), (&b, "rb"), (&c, "rc")] {
            assert!(
                !dir.path().join("f.md").exists(),
                "{name}: f.md survived compensation"
            );
            assert!(
                ShellGit
                    .is_clean(dir.path(), &[PathBuf::from("f.md")])
                    .unwrap(),
                "{name}: left dirty after compensation"
            );
        }
        // ra committed, so it is compensated by a revert, not a path-restore.
        let a_log = run(a.path(), &["log", "--format=%s"]);
        assert!(
            a_log.contains("Revert"),
            "repoA's commit was not reverted by Mutation-Id: {a_log}"
        );
        // rb and rc never committed, so the revert is unique to ra.
        let c_log = run(c.path(), &["log", "--format=%s"]);
        assert!(
            !c_log.contains("Revert") && !c_log.contains("test saga"),
            "repoC should carry no mutation commit: {c_log}"
        );
    }

    /// Several daemon-owned repos inside ONE git working tree make ONE commit.
    ///
    /// Git commits per working tree, and one commit per accepted mutation is a
    /// property of this write path, so the saga's member is the tree and the
    /// repos inside it coalesce. N commits carrying one `Mutation-Id` would
    /// break the compensation contract below.
    #[test]
    fn a_shared_working_tree_takes_exactly_one_commit() {
        let tree = init_repo();
        seed_nested(tree.path(), &["repo-a", "repo-b", "repo-c"]);
        let plan = SagaPlan {
            attribution: Vec::new(),
            dirs: DirJournal::default(),
            unclassified: None,
            not_written: Default::default(),
            outside_git: Default::default(),
            mutation_id: "m-shared-1".to_string(),
            members: nested_members(tree.path(), &["repo-a", "repo-b", "repo-c"]),
        };
        let subs = ["repo-a", "repo-b", "repo-c"];
        let marker = TempDir::new().unwrap();
        let (_, commits) = apply_saga(
            &ShellGit,
            marker.path(),
            &plan,
            "test shared tree".to_string(),
            false,
            || {
                for sub in subs {
                    fs::write(tree.path().join(sub).join("f.md"), "mutated\n").unwrap();
                }
                Ok::<(), MutationReject>(())
            },
        )
        .expect("the saga commits");

        assert_eq!(
            commits_carrying(tree.path(), "m-shared-1"),
            1,
            "one mutation must leave one commit in one working tree"
        );
        // The wire serves `commits: { [repo]: sha }`, so each coalesced repo is
        // reported and they SHARE the one sha: a consumer that touched repo-b
        // still learns which commit carries its change.
        assert_eq!(commits.len(), 3, "every covered repo is reported");
        let shas: std::collections::BTreeSet<_> =
            commits.iter().map(|(_, s)| s.0.clone()).collect();
        assert_eq!(shas.len(), 1, "the covered repos must share one sha");
        let names: Vec<&str> = commits.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["repo-a", "repo-b", "repo-c"], "name-sorted");

        // The trailer names every covered repo, name-sorted. Member order is
        // tree-ordered, an internal detail, so a reader must not see it.
        let body = run(tree.path(), &["log", "-1", "--format=%B"]);
        let listed: Vec<&str> = body
            .lines()
            .find_map(|l| l.strip_prefix("Mutation-Members: "))
            .expect("the commit carries a Members trailer")
            .split(", ")
            .collect();
        assert_eq!(listed, ["repo-a", "repo-b", "repo-c"]);
    }

    /// A nested repo's move records its `Moved:` trailer against the enclosing
    /// tree, with tree-relative paths.
    ///
    /// `settle_saga` looks moves up by the member's `tree_root`. Keyed by the
    /// repo root, a nested repo's rename would find no entry and commit with NO
    /// trailer: the forward trace would lose the edge with nothing failing.
    #[test]
    fn a_nested_repos_move_is_keyed_and_rebased_onto_its_tree() {
        let tree = init_repo();
        seed_nested(tree.path(), &["repo-a"]);
        let repo_root = tree.path().join("repo-a");

        let moves = moves_for(
            &repo_root,
            &PathBuf::from("old.md"),
            &PathBuf::from("new.md"),
        );

        let recorded = moves
            .get(tree.path())
            .expect("keyed by the enclosing tree, not the repo root");
        assert_eq!(recorded[0].from, PathBuf::from("repo-a/old.md"));
        assert_eq!(recorded[0].to, PathBuf::from("repo-a/new.md"));
        assert!(
            !moves.contains_key(&repo_root),
            "the repo root must not key the record, settle_saga would miss it"
        );
    }

    /// A repo that IS its own working tree keeps repo-relative paths, so the
    /// common single-repo case is unchanged by the rebasing.
    #[test]
    fn a_standalone_repos_move_is_unchanged_by_the_rebase() {
        let repo = init_repo();
        let moves = moves_for(
            repo.path(),
            &PathBuf::from("old.md"),
            &PathBuf::from("new.md"),
        );
        let recorded = moves.get(repo.path()).expect("keyed by its own root");
        assert_eq!(recorded[0].from, PathBuf::from("old.md"));
        assert_eq!(recorded[0].to, PathBuf::from("new.md"));
    }

    /// Reverting a shared tree's ONE commit restores every repo it covered.
    ///
    /// Compensation addresses a mutation's commit by its `Mutation-Id`, and
    /// `find_mutation_sha` REFUSES when more than one commit carries it. If the
    /// coalescing regressed to a member per repo, one mutation would land N
    /// commits in this tree and compensation would fail outright rather than
    /// half-apply — loud, but still a broken rollback this guards against.
    ///
    /// Forced by failing a SECOND tree's commit, after the shared tree has
    /// committed: with coalescing there is no "second commit into one tree" to
    /// fail, which is the property under test.
    #[test]
    fn a_shared_tree_failure_restores_every_coalesced_path() {
        let tree = init_repo();
        seed_nested(tree.path(), &["repo-a", "repo-b", "repo-c"]);
        let other = init_repo();
        let subs = ["repo-a", "repo-b", "repo-c"];

        let mut members = nested_members(tree.path(), &subs);
        members.push(member(other.path(), "other"));
        let plan = SagaPlan {
            attribution: Vec::new(),
            dirs: DirJournal::default(),
            unclassified: None,
            not_written: Default::default(),
            outside_git: Default::default(),
            mutation_id: "m-shared-2".to_string(),
            members,
        };
        // The shared tree commits first (tree-root ordered), then this fails.
        let git = FailAt {
            fail_repo: other.path().to_path_buf(),
        };
        let marker = TempDir::new().unwrap();
        let result = apply_saga(
            &git,
            marker.path(),
            &plan,
            "test shared tree".to_string(),
            false,
            || {
                for sub in subs {
                    fs::write(tree.path().join(sub).join("f.md"), "mutated\n").unwrap();
                }
                fs::write(other.path().join("f.md"), "mutated\n").unwrap();
                Ok::<(), MutationReject>(())
            },
        );
        assert!(result.is_err(), "the saga rejects on the commit failure");

        // Every coalesced repo is back at its seeded content, none left applied
        // by a revert that could only reach one commit.
        for sub in subs {
            let body = fs::read_to_string(tree.path().join(sub).join("f.md")).unwrap();
            assert_eq!(
                body, "seed\n",
                "{sub}: not restored, a coalesced repo survived compensation"
            );
        }
        assert!(
            ShellGit
                .is_clean(
                    tree.path(),
                    &subs
                        .iter()
                        .map(|s| PathBuf::from(s).join("f.md"))
                        .collect::<Vec<_>>()
                )
                .unwrap(),
            "the shared tree is left dirty after compensation"
        );
    }

    #[test]
    fn dirty_set_spans_every_member_path() {
        let plan = SagaPlan {
            attribution: Vec::new(),
            dirs: DirJournal::default(),
            unclassified: None,
            not_written: Default::default(),
            outside_git: Default::default(),
            mutation_id: "m".to_string(),
            members: vec![
                SagaMember {
                    tree_root: PathBuf::from("/a"),
                    repo_names: vec![RepoName("ra".to_string())],
                    paths: vec![PathBuf::from("x.md"), PathBuf::from("y.md")],
                    git: true,
                },
                SagaMember {
                    tree_root: PathBuf::from("/b"),
                    repo_names: vec![RepoName("rb".to_string())],
                    paths: vec![PathBuf::from("z.md")],
                    git: false,
                },
            ],
        };
        let dirty = saga_dirty_set(&plan);
        // One rebuild over the union: every member's path, so the version bumps
        // once after settle rather than once per member.
        assert_eq!(dirty.len(), 3, "dirty set: {dirty:?}");
        assert!(dirty.contains(&PathBuf::from("/a/x.md")));
        assert!(dirty.contains(&PathBuf::from("/a/y.md")));
        // A non-git member's written file is still re-read into the IR.
        assert!(
            dirty.contains(&PathBuf::from("/b/z.md")),
            "dirty set: {dirty:?}"
        );
    }

    /// Write the intent marker for a crashed saga over `members` (each touching
    /// `f.md`), as the live saga would have before touching the trees.
    fn crashed_marker(marker_root: &Path, id: &str, members: &[(&Path, &str)]) {
        let plan = SagaPlan {
            attribution: Vec::new(),
            dirs: DirJournal::default(),
            unclassified: None,
            not_written: Default::default(),
            outside_git: Default::default(),
            mutation_id: id.to_string(),
            members: members.iter().map(|(p, n)| member(p, n)).collect(),
        };
        write_intent_marker(
            &marker_root.join(".arsumbris").join("au-engine").join("run"),
            &plan,
            &[],
        )
        .unwrap();
    }

    /// A crashed saga over ONE working tree holding several repos recovers by
    /// reverting its single commit, restoring every repo the tree covered.
    ///
    /// The marker records one line per TREE, so recovery classifies once and the
    /// one revert reaches every coalesced repo. A marker written per repo would
    /// classify the same tree N times and revert its one commit repeatedly.
    #[test]
    fn recovery_over_a_shared_tree_restores_every_coalesced_repo() {
        let ws = TempDir::new().unwrap();
        let tree = init_repo();
        let subs = ["repo-a", "repo-b", "repo-c"];
        seed_nested(tree.path(), &subs);

        // The saga committed its one coalesced commit, then crashed before
        // closing the marker.
        for sub in subs {
            fs::write(tree.path().join(sub).join("f.md"), "mutated\n").unwrap();
        }
        let paths: Vec<PathBuf> = subs.iter().map(|s| PathBuf::from(s).join("f.md")).collect();
        let message = CommitMessage {
            summary: "crashed shared-tree mutation".to_string(),
            mutation_id: MutationId("m-crash-shared".to_string()),
            members: subs.iter().map(|s| RepoName(s.to_string())).collect(),
            moves: Vec::new(),
            reverts: None,
            attribution: Vec::new(),
        };
        ShellGit.commit(tree.path(), &paths, &message).unwrap();

        let plan = SagaPlan {
            attribution: Vec::new(),
            dirs: DirJournal::default(),
            unclassified: None,
            not_written: Default::default(),
            outside_git: Default::default(),
            mutation_id: "m-crash-shared".to_string(),
            members: nested_members(tree.path(), &subs),
        };
        write_intent_marker(
            &ws.path().join(".arsumbris").join("au-engine").join("run"),
            &plan,
            &[],
        )
        .unwrap();

        let report = recover_crashed_saga(ws.path());
        let recovered = report.recovered.expect("the open marker is recovered");
        assert_eq!(
            recovered.reverted,
            vec![tree.path().to_path_buf()],
            "one tree, one revert, not one per covered repo"
        );
        assert!(
            report.failures.is_empty(),
            "recovery reported failures: {:?}",
            report.failures
        );
        for sub in subs {
            let body = fs::read_to_string(tree.path().join(sub).join("f.md")).unwrap();
            assert_eq!(body, "seed\n", "{sub}: not restored by the single revert");
        }
    }

    /// A committed mutation write of `f.md` carrying `id`'s trailer.
    fn commit_mutation(repo: &Path, id: &str) {
        fs::write(repo.join("f.md"), "mutated\n").unwrap();
        let message = CommitMessage {
            summary: "crashed mutation".to_string(),
            mutation_id: MutationId(id.to_string()),
            members: vec![RepoName("m".to_string())],
            moves: Vec::new(),
            reverts: None,
            attribution: Vec::new(),
        };
        ShellGit
            .commit(repo, &[PathBuf::from("f.md")], &message)
            .unwrap();
    }

    #[test]
    fn recovery_is_a_noop_without_a_marker() {
        let ws = TempDir::new().unwrap();
        let report = recover_crashed_saga(ws.path());
        assert!(report.recovered.is_none(), "a clean start finds no marker");
        assert!(report.failures.is_empty(), "and no failure sentinels");
    }

    #[test]
    fn recovery_reverts_committed_and_restores_written_members() {
        let ws = TempDir::new().unwrap();
        let (ra, rb, rc) = (init_repo(), init_repo(), init_repo());

        // The saga m-crash committed ra, then crashed before committing rb / rc,
        // which are written but not committed.
        commit_mutation(ra.path(), "m-crash");
        fs::write(rb.path().join("f.md"), "mutated\n").unwrap();
        fs::write(rc.path().join("f.md"), "mutated\n").unwrap();
        crashed_marker(
            ws.path(),
            "m-crash",
            &[(ra.path(), "ra"), (rb.path(), "rb"), (rc.path(), "rc")],
        );

        let report = recover_crashed_saga(ws.path());
        assert!(
            report.failures.is_empty(),
            "a clean recovery has no failures"
        );
        let recovery = report.recovered.expect("a marker was present");
        assert_eq!(recovery.mutation_id, "m-crash");
        assert_eq!(
            recovery.reverted.len(),
            1,
            "ra had committed, so it reverts"
        );
        assert_eq!(recovery.restored.len(), 2, "rb and rc are path-restored");

        // ra's committed f.md is reverted away; rb and rc's dirty f.md is removed.
        for r in [&ra, &rb, &rc] {
            assert!(
                !r.path().join("f.md").exists(),
                "f.md should be gone after recovery"
            );
            assert!(
                ShellGit
                    .is_clean(r.path(), &[PathBuf::from("f.md")])
                    .unwrap(),
                "tree not clean after recovery"
            );
        }
        // The marker is cleared, so a second start is a no-op.
        assert!(
            !ws.path()
                .join(".arsumbris")
                .join("au-engine")
                .join("run")
                .join(INTENT_MARKER_NAME)
                .exists(),
            "the marker was not cleared"
        );
    }

    #[test]
    fn recovery_rolls_back_a_crash_before_the_first_commit() {
        let ws = TempDir::new().unwrap();
        let (ra, rb) = (init_repo(), init_repo());

        // Crash after writing the trees but before any commit: zero trailers, the
        // marker is the only signal these dirty trees are a crashed mutation.
        fs::write(ra.path().join("f.md"), "mutated\n").unwrap();
        fs::write(rb.path().join("f.md"), "mutated\n").unwrap();
        crashed_marker(
            ws.path(),
            "m-crash2",
            &[(ra.path(), "ra"), (rb.path(), "rb")],
        );

        let recovery = recover_crashed_saga(ws.path())
            .recovered
            .expect("a marker was present");
        assert!(
            recovery.reverted.is_empty(),
            "nothing committed, nothing to revert"
        );
        assert_eq!(recovery.restored.len(), 2, "both written members restored");
        assert!(!ra.path().join("f.md").exists(), "ra not rolled back");
        assert!(!rb.path().join("f.md").exists(), "rb not rolled back");
        assert!(
            !ws.path()
                .join(".arsumbris")
                .join("au-engine")
                .join("run")
                .join(INTENT_MARKER_NAME)
                .exists(),
            "the marker was not cleared"
        );
    }

    #[test]
    fn recovery_with_a_conflicting_revert_leaves_a_failure_sentinel() {
        let ws = TempDir::new().unwrap();
        let ra = init_repo();

        // The saga m-confl committed f.md in ra, then crashed.
        commit_mutation(ra.path(), "m-confl");
        // A human committed over the engine's commit, changing f.md, so reverting
        // m-confl will conflict.
        fs::write(ra.path().join("f.md"), "human edit\n").unwrap();
        run(ra.path(), &["add", "f.md"]);
        run(ra.path(), &["commit", "-q", "-m", "human edit"]);
        crashed_marker(ws.path(), "m-confl", &[(ra.path(), "ra")]);

        let report = recover_crashed_saga(ws.path());
        // The intent marker was processed and removed.
        assert!(report.recovered.is_some(), "the marker was processed");
        assert!(
            !ws.path()
                .join(".arsumbris")
                .join("au-engine")
                .join("run")
                .join(INTENT_MARKER_NAME)
                .exists(),
            "the intent marker was not cleared"
        );
        // The conflicted revert was recorded durably, not silently dropped.
        assert_eq!(
            report.failures.len(),
            1,
            "the failure must surface: {report:?}"
        );
        assert_eq!(report.failures[0].mutation_id, "m-confl");
        assert!(
            ws.path()
                .join(".arsumbris")
                .join("au-engine")
                .join("run")
                .join("saga.failed.m-confl")
                .exists(),
            "the failure sentinel must be on disk"
        );

        // A restart re-surfaces the sentinel and finds no marker.
        let again = recover_crashed_saga(ws.path());
        assert!(
            again.recovered.is_none(),
            "no intent marker the second time"
        );
        assert_eq!(again.failures.len(), 1, "the sentinel is re-surfaced");
    }

    /// A repo ignoring `scratch.md` (the rule committed), with a tracked `f.md`
    /// and an untracked `scratch.md` holding `old`, plus the one member touching
    /// both, classified the way `resolve_saga_plan` does.
    fn repo_with_an_ignored_scratch() -> (TempDir, SagaPlan) {
        let dir = init_repo();
        let p = dir.path();
        fs::write(p.join(".gitignore"), "scratch.md\n").unwrap();
        fs::write(p.join("f.md"), "seed\n").unwrap();
        run(p, &["add", "."]);
        run(p, &["commit", "-q", "-m", "ignore scratch"]);
        fs::write(p.join("scratch.md"), "old\n").unwrap();
        let mut m = member(p, "r");
        m.paths.push(PathBuf::from("scratch.md"));
        let members = vec![m];
        let outside_git = classify_outside_git(&ShellGit, &members, &Default::default()).unwrap();
        let plan = SagaPlan {
            attribution: Vec::new(),
            dirs: DirJournal::default(),
            unclassified: None,
            not_written: Default::default(),
            outside_git,
            mutation_id: "m-ignored".to_string(),
            members,
        };
        (dir, plan)
    }

    #[test]
    fn classification_finds_ignored_untracked_paths_only() {
        let (dir, plan) = repo_with_an_ignored_scratch();
        let p = dir.path();
        assert_eq!(
            plan.outside_git,
            [p.join("scratch.md")].into_iter().collect(),
            "only the ignored, untracked path is outside git"
        );
        assert_eq!(
            plan.written_paths(&plan.members[0]),
            vec![PathBuf::from("f.md")],
            "the git-facing path set drops it"
        );

        // A tracked file stays git-recorded whatever the ignore rules say, and a
        // not-yet-existing destination classifies by pattern.
        fs::write(p.join(".gitignore"), "scratch.md\nf.md\nlater/\n").unwrap();
        let m = SagaMember {
            tree_root: p.to_path_buf(),
            repo_names: vec![RepoName("r".to_string())],
            paths: vec![PathBuf::from("f.md"), PathBuf::from("later/new.md")],
            git: true,
        };
        assert_eq!(
            classify_outside_git(&ShellGit, &[m], &Default::default()).unwrap(),
            [p.join("later/new.md")].into_iter().collect(),
            "tracked f.md stays in git, the absent destination is ignored by pattern"
        );
    }

    #[test]
    fn an_ignored_path_is_written_but_never_committed() {
        let (dir, plan) = repo_with_an_ignored_scratch();
        let p = dir.path().to_path_buf();
        let marker = TempDir::new().unwrap();
        let (_, commits) = apply_saga(
            &ShellGit,
            marker.path(),
            &plan,
            "mixed write".to_string(),
            false,
            || {
                fs::write(p.join("f.md"), "changed\n").unwrap();
                fs::write(p.join("scratch.md"), "new\n").unwrap();
                Ok::<(), MutationReject>(())
            },
        )
        .expect("a commit beside an ignored path succeeds");
        assert_eq!(commits.len(), 1, "the tracked change committed");
        assert_eq!(
            run(&p, &["show", "--name-only", "--format=", "HEAD"]),
            "f.md",
            "the commit names only the tracked path"
        );
        assert_eq!(fs::read_to_string(p.join("scratch.md")).unwrap(), "new\n");
    }

    #[test]
    fn a_failed_saga_restores_an_ignored_path_byte_identical() {
        let (dir, plan) = repo_with_an_ignored_scratch();
        let p = dir.path().to_path_buf();
        let marker = TempDir::new().unwrap();
        let git = FailAt {
            fail_repo: p.clone(),
        };
        let result = apply_saga(
            &git,
            marker.path(),
            &plan,
            "mixed write".to_string(),
            false,
            || {
                fs::write(p.join("f.md"), "changed\n").unwrap();
                fs::write(p.join("scratch.md"), "new\n").unwrap();
                Ok::<(), MutationReject>(())
            },
        );
        assert!(result.is_err(), "the forced commit failure rejects");
        assert_eq!(
            fs::read_to_string(p.join("f.md")).unwrap(),
            "seed\n",
            "the tracked path is restored to HEAD"
        );
        assert_eq!(
            fs::read_to_string(p.join("scratch.md")).unwrap(),
            "old\n",
            "the ignored path is restored from its parked copy, byte-identical"
        );
        assert!(
            !preimage_dir(marker.path(), "m-ignored").exists(),
            "the parked copies are dropped once the saga settles"
        );
    }

    /// A repo holding a tracked `d/a.md`, an empty `d/empty`, and an empty
    /// `notes/` that existed before the op, plus the plan moving `d/` to
    /// `notes/x/`: the file, the skeleton (`notes/x/empty` created, `d/empty`
    /// and `d` removed), and `notes/` kept, since the rollback's pruning of the
    /// created file would otherwise take it.
    fn folder_move_fixture() -> (TempDir, SagaPlan) {
        let dir = init_repo();
        let p = dir.path();
        fs::create_dir_all(p.join("d/empty")).unwrap();
        fs::create_dir_all(p.join("notes")).unwrap();
        fs::write(p.join("d/a.md"), "a\n").unwrap();
        run(p, &["add", "."]);
        run(p, &["commit", "-q", "-m", "seed d"]);
        let plan = SagaPlan {
            attribution: Vec::new(),
            unclassified: None,
            dirs: DirJournal {
                created: vec![p.join("notes/x"), p.join("notes/x/empty")],
                kept: vec![p.join("d"), p.join("d/empty"), p.join("notes")],
            },
            not_written: Default::default(),
            outside_git: Default::default(),
            mutation_id: "m-folder".to_string(),
            members: vec![SagaMember {
                tree_root: p.to_path_buf(),
                repo_names: vec![RepoName("r".to_string())],
                paths: vec![PathBuf::from("d/a.md"), PathBuf::from("notes/x/a.md")],
                git: true,
            }],
        };
        (dir, plan)
    }

    /// The folder move's writes, stopping early when `upto_skeleton` is false.
    fn do_folder_move(p: &Path, upto_skeleton: bool) {
        fs::create_dir_all(p.join("notes/x")).unwrap();
        fs::rename(p.join("d/a.md"), p.join("notes/x/a.md")).unwrap();
        if upto_skeleton {
            fs::create_dir_all(p.join("notes/x/empty")).unwrap();
            fs::remove_dir(p.join("d/empty")).unwrap();
            fs::remove_dir(p.join("d")).unwrap();
        }
    }

    fn assert_folder_unmoved(p: &Path) {
        assert_eq!(fs::read_to_string(p.join("d/a.md")).unwrap(), "a\n");
        assert!(
            p.join("d/empty").is_dir(),
            "the removed empty directory is back"
        );
        assert!(
            p.join("notes").is_dir(),
            "the pre-existing empty parent survives"
        );
        assert!(
            !p.join("notes/x").exists(),
            "every created directory is gone"
        );
    }

    #[test]
    fn a_folder_delete_leaves_a_file_created_after_the_plan_uncommitted_and_in_place() {
        let (dir, _) = folder_move_fixture();
        let p = dir.path().to_path_buf();
        let plan = SagaPlan {
            attribution: Vec::new(),
            unclassified: None,
            dirs: DirJournal {
                created: Vec::new(),
                kept: vec![p.join("d"), p.join("d/empty")],
            },
            not_written: Default::default(),
            outside_git: Default::default(),
            mutation_id: "m-delete-dir".to_string(),
            members: vec![SagaMember {
                tree_root: p.clone(),
                repo_names: vec![RepoName("r".to_string())],
                paths: vec![PathBuf::from("d/a.md")],
                git: true,
            }],
        };
        let marker = TempDir::new().unwrap();
        let files = vec![p.join("d/a.md")];
        let dirs = vec![p.join("d"), p.join("d/empty")];
        let (_, commits) = apply_saga(
            &ShellGit,
            marker.path(),
            &plan,
            "delete".to_string(),
            true,
            || {
                // A human creates a file in the folder after it was enumerated.
                fs::write(p.join("d/late.md"), "late\n").unwrap();
                delete_dir_write(&files, &dirs)
            },
        )
        .unwrap();
        assert_eq!(commits.commits.len(), 1);
        assert!(!p.join("d/a.md").exists(), "the enumerated file is deleted");
        assert!(
            !p.join("d/empty").exists(),
            "the emptied directory is removed"
        );
        assert_eq!(
            fs::read_to_string(p.join("d/late.md")).unwrap(),
            "late\n",
            "the late file survives, with its directory"
        );
        let committed = run(&p, &["show", "--name-status", "--format=", "HEAD"]);
        assert_eq!(
            committed, "D\td/a.md",
            "only the enumerated file is in the commit"
        );
        assert_eq!(
            run(&p, &["status", "--porcelain", "--untracked-files=all"]),
            "?? d/late.md"
        );
    }

    #[test]
    fn require_git_ignores_a_non_git_member_the_op_never_writes() {
        let off = PathBuf::from("/off-git");
        let mut plan = SagaPlan {
            attribution: Vec::new(),
            unclassified: None,
            dirs: DirJournal::default(),
            not_written: [off.join("r.md")].into_iter().collect(),
            outside_git: Default::default(),
            mutation_id: "m-require-git".to_string(),
            members: vec![SagaMember {
                tree_root: off.clone(),
                repo_names: vec![RepoName("off".to_string())],
                paths: vec![PathBuf::from("r.md")],
                git: false,
            }],
        };
        assert!(
            require_git_reject(&plan).is_none(),
            "a member holding only byte-identical referrers is never written"
        );
        plan.members[0].paths.push(PathBuf::from("w.md"));
        let reject = require_git_reject(&plan).expect("a written path still needs a tree");
        assert!(reject.message.contains("off"), "{}", reject.message);
    }

    #[test]
    fn a_folder_move_leaves_a_file_created_after_the_plan_uncommitted_in_its_source_dir() {
        let dir = init_repo();
        let p = fs::canonicalize(dir.path()).unwrap();
        fs::create_dir_all(p.join(".arsumbris")).unwrap();
        fs::write(p.join(".arsumbris/repo.yaml"), "name: r\n").unwrap();
        fs::create_dir_all(p.join("d/empty")).unwrap();
        fs::write(p.join("d/a.md"), "a\n").unwrap();
        run(&p, &["add", "."]);
        run(&p, &["commit", "-q", "-m", "seed d"]);
        let kb = crate::build::build(&p, &tb_parser::RealFileSystem).unwrap();
        let dir_plan = crate::dirop::plan_move_dir(&kb, &p.join("d"), &p.join("notes/x")).unwrap();
        let plan = SagaPlan {
            attribution: Vec::new(),
            unclassified: None,
            dirs: DirJournal {
                created: dir_plan.created_dirs(),
                kept: dir_plan.kept_dirs(),
            },
            not_written: Default::default(),
            outside_git: Default::default(),
            mutation_id: "m-move-dir".to_string(),
            members: vec![SagaMember {
                tree_root: p.clone(),
                repo_names: vec![RepoName("r".to_string())],
                paths: vec![PathBuf::from("d/a.md"), PathBuf::from("notes/x/a.md")],
                git: true,
            }],
        };
        let marker = TempDir::new().unwrap();
        let (_, commits) = apply_saga(
            &ShellGit,
            marker.path(),
            &plan,
            "move".to_string(),
            true,
            || {
                // A human creates a file in the folder after it was enumerated.
                fs::write(p.join("d/late.md"), "late\n").unwrap();
                move_dir_write(&dir_plan, &Default::default())
            },
        )
        .unwrap();
        assert_eq!(commits.commits.len(), 1);
        assert_eq!(fs::read_to_string(p.join("notes/x/a.md")).unwrap(), "a\n");
        assert!(
            p.join("notes/x/empty").is_dir(),
            "the empty directory moves with the folder"
        );
        assert!(
            !p.join("d/empty").exists(),
            "the emptied source directory is removed"
        );
        assert_eq!(
            fs::read_to_string(p.join("d/late.md")).unwrap(),
            "late\n",
            "the late file stays in its source directory, which is kept"
        );
        let committed = run(&p, &["show", "-M", "--name-status", "--format=", "HEAD"]);
        assert_eq!(
            committed, "R100\td/a.md\tnotes/x/a.md",
            "only the enumerated file is in the commit"
        );
        assert_eq!(
            run(&p, &["status", "--porcelain", "--untracked-files=all"]),
            "?? d/late.md"
        );
    }

    #[test]
    fn a_failed_commit_puts_the_directory_skeleton_back() {
        let (dir, plan) = folder_move_fixture();
        let p = dir.path().to_path_buf();
        let marker = TempDir::new().unwrap();
        let git = FailAt {
            fail_repo: p.clone(),
        };
        let result = apply_saga(&git, marker.path(), &plan, "move".to_string(), true, || {
            do_folder_move(&p, true);
            Ok::<(), MutationReject>(())
        });
        let err = result.unwrap_err();
        assert!(!err.message.contains("incomplete"), "{}", err.message);
        assert_folder_unmoved(&p);
    }

    #[test]
    fn a_write_failing_part_way_puts_the_directory_skeleton_back() {
        let (dir, plan) = folder_move_fixture();
        let p = dir.path().to_path_buf();
        let marker = TempDir::new().unwrap();
        let result = apply_saga(
            &ShellGit,
            marker.path(),
            &plan,
            "move".to_string(),
            true,
            || {
                do_folder_move(&p, false);
                Err::<(), _>(MutationReject::new("forced write failure"))
            },
        );
        assert!(result.is_err());
        assert_folder_unmoved(&p);
    }

    #[test]
    fn crash_recovery_puts_the_directory_skeleton_back() {
        let ws = TempDir::new().unwrap();
        let (dir, plan) = folder_move_fixture();
        let p = dir.path().to_path_buf();
        let marker_dir = ws.path().join(".arsumbris").join("au-engine").join("run");
        write_intent_marker(&marker_dir, &plan, &[]).unwrap();
        do_folder_move(&p, true);
        let report = recover_crashed_saga(ws.path());
        assert!(report.recovered.is_some());
        assert!(
            report.failures.is_empty(),
            "failures: {:?}",
            report.failures
        );
        assert_folder_unmoved(&p);
    }

    #[test]
    fn a_created_directory_holding_foreign_content_is_left_and_reported() {
        let ws = TempDir::new().unwrap();
        let (dir, plan) = folder_move_fixture();
        let p = dir.path().to_path_buf();
        let marker_dir = ws.path().join(".arsumbris").join("au-engine").join("run");
        write_intent_marker(&marker_dir, &plan, &[]).unwrap();
        do_folder_move(&p, true);
        // A file the saga never wrote lands in a directory it created.
        fs::write(p.join("notes/x/empty/human.md"), "mine\n").unwrap();
        let report = recover_crashed_saga(ws.path());
        assert_eq!(report.failures.len(), 1, "the undo is reported incomplete");
        assert!(report.failures[0].detail.contains("notes/x/empty"));
        assert_eq!(
            fs::read_to_string(p.join("notes/x/empty/human.md")).unwrap(),
            "mine\n",
            "content the saga did not write is never removed"
        );
        assert_eq!(fs::read_to_string(p.join("d/a.md")).unwrap(), "a\n");
    }

    /// A member moving the ignored `scratch.md` to `sub/scratch.md` (ignored at
    /// any depth) beside a tracked `f.md` change.
    fn ignored_move_plan(p: &Path) -> SagaPlan {
        let m = SagaMember {
            tree_root: p.to_path_buf(),
            repo_names: vec![RepoName("r".to_string())],
            paths: vec![
                PathBuf::from("f.md"),
                PathBuf::from("scratch.md"),
                PathBuf::from("sub/scratch.md"),
            ],
            git: true,
        };
        let members = vec![m];
        SagaPlan {
            attribution: Vec::new(),
            dirs: DirJournal::default(),
            unclassified: None,
            not_written: Default::default(),
            outside_git: classify_outside_git(&ShellGit, &members, &Default::default()).unwrap(),
            mutation_id: "m-ignored-move".to_string(),
            members,
        }
    }

    fn do_ignored_move(p: &Path) {
        fs::write(p.join("f.md"), "changed\n").unwrap();
        fs::create_dir_all(p.join("sub")).unwrap();
        fs::rename(p.join("scratch.md"), p.join("sub/scratch.md")).unwrap();
    }

    #[test]
    fn a_failed_saga_moves_an_ignored_path_back() {
        let (dir, _) = repo_with_an_ignored_scratch();
        let p = dir.path().to_path_buf();
        let plan = ignored_move_plan(&p);
        assert_eq!(
            plan.outside_git.len(),
            2,
            "both sides of the move are outside git"
        );
        let marker = TempDir::new().unwrap();
        let git = FailAt {
            fail_repo: p.clone(),
        };
        let result = apply_saga(
            &git,
            marker.path(),
            &plan,
            "move".to_string(),
            false,
            || {
                do_ignored_move(&p);
                Ok::<(), MutationReject>(())
            },
        );
        assert!(result.is_err());
        assert_eq!(fs::read_to_string(p.join("scratch.md")).unwrap(), "old\n");
        assert!(
            !p.join("sub/scratch.md").exists(),
            "the destination is removed"
        );
        assert!(
            !p.join("sub").exists(),
            "the directory the move created is pruned"
        );
        assert_eq!(fs::read_to_string(p.join("f.md")).unwrap(), "seed\n");
    }

    #[test]
    fn a_settled_saga_keeps_the_ignored_write_and_drops_the_copies() {
        let (dir, _) = repo_with_an_ignored_scratch();
        let p = dir.path().to_path_buf();
        let plan = ignored_move_plan(&p);
        let marker = TempDir::new().unwrap();
        apply_saga(
            &ShellGit,
            marker.path(),
            &plan,
            "move".to_string(),
            false,
            || {
                do_ignored_move(&p);
                Ok::<(), MutationReject>(())
            },
        )
        .expect("the saga settles");
        assert_eq!(
            fs::read_to_string(p.join("sub/scratch.md")).unwrap(),
            "old\n"
        );
        assert!(!p.join("scratch.md").exists());
        assert!(
            !preimage_dir(marker.path(), "m-ignored-move").exists(),
            "the parked copies are dropped"
        );
        assert!(!marker.path().join(INTENT_MARKER_NAME).exists());
    }

    #[test]
    fn crash_recovery_restores_an_ignored_path_from_its_parked_copy() {
        let ws = TempDir::new().unwrap();
        let (dir, _) = repo_with_an_ignored_scratch();
        let p = dir.path().to_path_buf();
        let plan = ignored_move_plan(&p);
        let marker_dir = ws.path().join(".arsumbris").join("au-engine").join("run");

        // The saga parked, opened its marker, wrote, then crashed before settling.
        let preimages = park_preimages(&marker_dir, &plan).unwrap();
        write_intent_marker(&marker_dir, &plan, &preimages).unwrap();
        do_ignored_move(&p);
        // An orphan from an earlier crash between closing its marker and
        // dropping its copies.
        fs::create_dir_all(preimage_dir(&marker_dir, "m-orphan")).unwrap();

        let report = recover_crashed_saga(ws.path());
        assert!(report.recovered.is_some(), "the open marker is recovered");
        assert!(
            report.failures.is_empty(),
            "failures: {:?}",
            report.failures
        );
        assert_eq!(fs::read_to_string(p.join("scratch.md")).unwrap(), "old\n");
        assert!(!p.join("sub/scratch.md").exists());
        assert_eq!(fs::read_to_string(p.join("f.md")).unwrap(), "seed\n");
        assert!(
            !preimage_dir(&marker_dir, "m-ignored-move").exists()
                && !preimage_dir(&marker_dir, "m-orphan").exists(),
            "recovery sweeps every parked copy"
        );
    }
}

#[cfg(test)]
mod verb_tests {
    use super::*;
    use crate::verb_metrics::{BUCKET_COUNT, VERBS, snapshot};
    use std::sync::Mutex;
    use std::sync::MutexGuard;

    /// WIRE.md's verb census: the read catalog's entries, then the one control
    /// verb. A read verb heads its own catalog entry as `**verb.**`, so the
    /// document's own shape is the parse — no second list to keep in step.
    fn wire_census() -> Vec<String> {
        let wire = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/WIRE.md"));
        let section = |start_marker: &str, end_marker: &str| -> String {
            let start = wire
                .find(start_marker)
                .unwrap_or_else(|| panic!("WIRE.md must carry `{start_marker}`"));
            let rest = &wire[start..];
            let end = rest.find(end_marker).unwrap_or_else(|| {
                panic!("WIRE.md must carry `{end_marker}` after `{start_marker}`")
            });
            rest[..end].to_string()
        };
        let mut verbs = Vec::new();
        for body in [
            section("## The read catalog", "## Subscriptions"),
            section("## The control verb", "## Daemon-less member resolution"),
        ] {
            for line in body.lines() {
                // `**graph_health.**` — a verb, not the `**\`schema_version\` N**`
                // prose the same sections also carry.
                let Some(rest) = line.strip_prefix("**") else {
                    continue;
                };
                let Some(name) = rest.strip_suffix(".**") else {
                    continue;
                };
                if !name.is_empty()
                    && name
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
                {
                    verbs.push(name.to_string());
                }
            }
        }
        verbs
    }

    /// Every `Request` variant, constructed with throwaway arguments. The point
    /// is not the arguments: it is that the only way to enumerate the variants is
    /// to name them, and naming each one here is what lets the test below ask
    /// `verb()` what it calls it.
    fn every_request() -> Vec<Request> {
        vec![
            Request::Diagnostics(DiagnosticsFilterArgs::default()),
            Request::DiagnosticCounts(DiagnosticsFilterArgs::default()),
            Request::Types(TypesArgs::default()),
            Request::TypeCounts(TypeCountsArgs::default()),
            Request::TypeTree(TypeTreeArgs::default()),
            Request::Type(TypeArgs {
                name: String::new(),
            }),
            Request::TypeBatch(TypeBatchArgs { names: Vec::new() }),
            Request::InstancesOf(InstancesOfArgs {
                type_name: String::new(),
                origins: None,
                instance: false,
                body: false,
            }),
            Request::Imports(ImportsArgs::default()),
            Request::Grep(GrepArgs::default()),
            Request::Search(SearchArgs::default()),
            Request::Subtypes(SubtypesArgs {
                base: String::new(),
                scope: ScopeArg::default(),
            }),
            Request::Instances,
            Request::Candidates(CandidatesArgs::default()),
            Request::CandidateCounts(CandidateCountsArgs::default()),
            Request::InstanceCounts(InstanceCountsArgs::default()),
            Request::Instance {
                path: String::new(),
            },
            Request::ReferencesIn {
                path: String::new(),
            },
            Request::ReferencesOut {
                path: String::new(),
            },
            Request::Pins {
                target: String::new(),
                source_type: String::new(),
            },
            Request::CommitMeta {
                commits: Vec::new(),
            },
            Request::FileHistory {
                path: String::new(),
                repo: None,
            },
            Request::RecentCommits {
                members: None,
                limit: None,
                since: None,
            },
            Request::Neighborhood(NeighborhoodArgs {
                path: String::new(),
                direction: WireDirection::default(),
                depth: 1,
                kinds: None,
                scope: ScopeArg::default(),
                max_nodes: None,
                content: false,
                body: false,
                instance: false,
            }),
            Request::ResolveTarget {
                target: String::new(),
                origin: None,
            },
            Request::ResolveBlockId {
                target: String::new(),
                block_id: String::new(),
                origin: None,
            },
            Request::ResolveAnchor {
                target: String::new(),
                anchor: String::new(),
                origin: None,
            },
            Request::Anchors {
                target: String::new(),
                origin: None,
            },
            Request::BlockIds {
                target: String::new(),
                origin: None,
            },
            Request::Files(FilesArgs::default()),
            Request::DirEntries { dir: String::new() },
            Request::Frontmatter {
                path: String::new(),
            },
            Request::Content {
                path: String::new(),
            },
            Request::SemanticTokens {
                path: String::new(),
            },
            Request::ValidateValue {
                type_name: String::new(),
                value: serde_json::Value::Null,
                repo: None,
            },
            Request::Members(MembersArgs::default()),
            Request::Ignores {
                repo: None,
                resolve: false,
            },
            Request::ResolveMember {
                path: String::new(),
            },
            Request::DeviceConfig,
            Request::Config(ConfigArgs {
                scope: ConfigScope::Machine,
                consumer: String::new(),
                file: String::new(),
                type_name: String::new(),
                root: None,
            }),
            Request::TopLevelDirs(TopLevelDirsArgs::default()),
            Request::Overview(OverviewArgs {
                repo: None,
                scope: ScopeArg::default(),
            }),
            Request::TypeClosure(TypeClosureArgs {
                name: String::new(),
                repo: None,
            }),
            Request::Hubs(HubsArgs::default()),
            Request::GraphShape(GraphShapeArgs::default()),
            Request::RollbackPreview(RollbackPreviewArgs {
                repo: String::new(),
                commit: String::new(),
            }),
            Request::GraphHealth(GraphHealthArgs::default()),
            Request::LinkGraph(LinkGraphArgs::default()),
            Request::TypeGraph(TypeGraphArgs::default()),
            Request::PreviewMutation(PreviewMutationArgs::DeleteFile {
                path: String::new(),
            }),
            Request::Lifecycle,
            Request::Shutdown,
        ]
    }

    /// The `scope` a request's own argument declares: its absent meaning
    /// ([`ScopedArgs::DEFAULT`]) and whether a location pin is present. `None`
    /// means the read takes no `scope` argument.
    ///
    /// Written out per variant with NO wildcard, for the same reason `verb()` is:
    /// totality is the point. A new read either says `None` or hands its
    /// declaration over, so it cannot carry a `scope` and stay out of the table
    /// the test below checks.
    fn declared_scope(req: &Request) -> Option<(ScopeDefault, bool)> {
        match req {
            Request::Diagnostics(a) | Request::DiagnosticCounts(a) => {
                Some((DiagnosticsFilterArgs::DEFAULT, a.pin_moves_default()))
            }
            Request::Types(a) => Some((TypesArgs::DEFAULT, a.pin_moves_default())),
            Request::TypeCounts(a) => Some((TypeCountsArgs::DEFAULT, a.pin_moves_default())),
            Request::TypeTree(a) => Some((TypeTreeArgs::DEFAULT, a.pin_moves_default())),
            Request::Imports(a) => Some((ImportsArgs::DEFAULT, a.pin_moves_default())),
            Request::Subtypes(a) => Some((SubtypesArgs::DEFAULT, a.pin_moves_default())),
            Request::InstanceCounts(a) => {
                Some((InstanceCountsArgs::DEFAULT, a.pin_moves_default()))
            }
            Request::Neighborhood(a) => Some((NeighborhoodArgs::DEFAULT, a.pin_moves_default())),
            Request::Files(a) => Some((FilesArgs::DEFAULT, a.pin_moves_default())),
            Request::Grep(a) => Some((GrepArgs::DEFAULT, a.pin_moves_default())),
            Request::Search(a) => Some((SearchArgs::DEFAULT, a.pin_moves_default())),
            Request::Overview(a) => Some((OverviewArgs::DEFAULT, a.pin_moves_default())),
            Request::TopLevelDirs(a) => Some((TopLevelDirsArgs::DEFAULT, a.pin_moves_default())),
            Request::Hubs(a) => Some((HubsArgs::DEFAULT, a.pin_moves_default())),
            Request::GraphShape(a) => Some((GraphShapeArgs::DEFAULT, a.pin_moves_default())),
            Request::GraphHealth(a) => Some((GraphHealthArgs::DEFAULT, a.pin_moves_default())),
            Request::LinkGraph(a) => Some((LinkGraphArgs::DEFAULT, a.pin_moves_default())),
            Request::TypeGraph(a) => Some((TypeGraphArgs::DEFAULT, a.pin_moves_default())),
            Request::Type(_)
            | Request::TypeBatch(_)
            | Request::InstancesOf(_)
            | Request::Instances
            | Request::Candidates(_)
            | Request::CandidateCounts(_)
            | Request::Instance { .. }
            | Request::ReferencesIn { .. }
            | Request::ReferencesOut { .. }
            | Request::Pins { .. }
            | Request::CommitMeta { .. }
            | Request::FileHistory { .. }
            | Request::RecentCommits { .. }
            | Request::ResolveTarget { .. }
            | Request::ResolveBlockId { .. }
            | Request::ResolveAnchor { .. }
            | Request::Anchors { .. }
            | Request::BlockIds { .. }
            | Request::DirEntries { .. }
            | Request::Frontmatter { .. }
            | Request::Content { .. }
            | Request::SemanticTokens { .. }
            | Request::ValidateValue { .. }
            | Request::Members(_)
            | Request::Ignores { .. }
            | Request::ResolveMember { .. }
            | Request::DeviceConfig
            | Request::Config(_)
            | Request::TypeClosure(_)
            | Request::RollbackPreview(_)
            | Request::PreviewMutation(_)
            | Request::Lifecycle
            | Request::Shutdown => None,
        }
    }

    /// THE census invariant: the map is total (the compiler enforces an arm per
    /// variant), it names each variant exactly once, and the set of names is
    /// exactly `WIRE.md`'s census — the document's read catalog plus its control
    /// verb. A rename on either side fails here, which is what keeps the counters
    /// labelling requests with the wire's own words.
    #[test]
    fn the_verb_map_agrees_with_the_wire_census() {
        let census = wire_census();
        assert_eq!(
            census.len(),
            52,
            "the census lost or gained a verb: {census:?}"
        );

        let mut mapped: Vec<String> = every_request()
            .iter()
            .map(|r| r.verb().to_string())
            .collect();
        let mut expected = census.clone();
        expected.sort();
        expected.dedup();
        assert_eq!(
            expected.len(),
            census.len(),
            "WIRE.md names a verb twice: {census:?}"
        );

        mapped.sort();
        let mut deduped = mapped.clone();
        deduped.dedup();
        assert_eq!(
            deduped.len(),
            mapped.len(),
            "two Request variants share one verb: {mapped:?}"
        );
        assert_eq!(
            mapped, expected,
            "the map and WIRE.md disagree; the map must name each variant as the document does"
        );

        // The counters INDEX this table, so it is the same census, sorted.
        let table: Vec<String> = VERBS.iter().map(|v| v.to_string()).collect();
        assert_eq!(table, expected, "the counter table is not the census");
    }

    /// THE scope-default invariant: the table beside [`Request`] states every
    /// scope-bearing read's absent meaning, and each read's own argument declares
    /// the same thing. Both directions are checked, so a read that gains `scope`
    /// without a row and a row without a read both fail.
    #[test]
    fn scope_defaults_are_the_table() {
        let mut declared: BTreeMap<&'static str, ScopeDefault> = BTreeMap::new();
        for req in every_request() {
            let verb = req.verb();
            match declared_scope(&req) {
                Some((default, _)) => {
                    assert!(
                        declared.insert(verb, default).is_none(),
                        "{verb} declares a scope default twice"
                    );
                }
                None => assert!(
                    !SCOPE_TABLE.iter().any(|row| row.verb == verb),
                    "{verb} takes no `scope` but the table gives it a default"
                ),
            }
        }
        for row in SCOPE_TABLE {
            assert!(
                !row.reason.is_empty(),
                "{} has a default and no reason; the table IS the reason",
                row.verb
            );
        }
        let tabled: BTreeMap<&'static str, ScopeDefault> = SCOPE_TABLE
            .iter()
            .map(|row| (row.verb, row.default))
            .collect();
        assert_eq!(
            tabled.len(),
            SCOPE_TABLE.len(),
            "a verb is tabled twice: {SCOPE_TABLE:?}"
        );
        assert_eq!(
            declared, tabled,
            "the declared scope defaults and the table disagree"
        );
    }

    /// The resolver, in isolation: absence takes the read's declared default, a
    /// stated value always wins, and a location pin moves only the default that
    /// declares it can.
    #[test]
    fn a_stated_scope_wins_and_absence_takes_the_default() {
        let absent = ScopeArg::default();
        for (default, pinned, own) in [
            (ScopeDefault::All, false, false),
            (ScopeDefault::All, true, false),
            (ScopeDefault::Own, false, true),
            (ScopeDefault::Own, true, true),
            (ScopeDefault::OwnUnlessPinned, false, true),
            (ScopeDefault::OwnUnlessPinned, true, false),
        ] {
            assert_eq!(
                absent.resolve(default, pinned).own_only(),
                own,
                "absent scope under {default:?}, pinned {pinned}"
            );
        }
        for (arg, own) in [
            (ScopeArg(Some(Scope::All)), false),
            (ScopeArg(Some(Scope::Own)), true),
        ] {
            for default in [
                ScopeDefault::All,
                ScopeDefault::Own,
                ScopeDefault::OwnUnlessPinned,
            ] {
                for pinned in [false, true] {
                    assert_eq!(
                        arg.resolve(default, pinned).own_only(),
                        own,
                        "a stated {arg:?} must beat {default:?}, pinned {pinned}"
                    );
                }
            }
        }
    }

    /// A location pin comes from the args, and only the reads whose default
    /// depends on one let it move: `search` / `grep` accept a `repo` but their
    /// default stays `own` when it is named.
    #[test]
    fn only_the_reads_that_declare_it_move_on_a_pin() {
        let named = || Some("base".to_owned());

        assert!(!HubsArgs::default().pin_moves_default());
        assert!(
            HubsArgs {
                repo: named(),
                ..Default::default()
            }
            .pin_moves_default()
        );
        assert!(
            !HubsArgs {
                repo: named(),
                ..Default::default()
            }
            .scope()
            .own_only(),
            "a named member answers `all`"
        );

        assert!(!DiagnosticsFilterArgs::default().pin_moves_default());
        for pinned in [
            DiagnosticsFilterArgs {
                repo: named(),
                ..Default::default()
            },
            DiagnosticsFilterArgs {
                path: Some("app/a.md".to_owned()),
                ..Default::default()
            },
            DiagnosticsFilterArgs {
                path_prefix: Some("app/".to_owned()),
                ..Default::default()
            },
        ] {
            assert!(pinned.pin_moves_default());
            assert!(!pinned.scope().own_only(), "a pin answers `all`");
        }

        // `search` / `grep` accept a `repo`, but their default has no pin axis:
        // naming the member narrows WHICH one is searched, not the scope, so the
        // resolved scope is still `own`.
        let searched = SearchArgs {
            repo: named(),
            ..Default::default()
        };
        assert!(
            searched.scope().own_only(),
            "a fixed default does not move on a pin"
        );
        let grepped = GrepArgs {
            repo: named(),
            ..Default::default()
        };
        assert!(grepped.scope().own_only());
    }

    /// Bridges and articulation points answer the definitions they claim: a path
    /// edge whose removal splits the graph, and a node whose removal does, on the
    /// undirected projection; a cycle contributes neither, and a lone node is not
    /// an articulation point.
    #[test]
    fn bridges_and_articulation_points_match_their_definitions() {
        // 0—1—2 plus the triangle 2—3—4—2: both path edges are bridges, and 1 and
        // 2 are the articulation points.
        let adjacency = vec![vec![1], vec![0, 2], vec![1, 3, 4], vec![2, 4], vec![2, 3]];
        let (mut bridges, mut articulation) = bridges_and_articulation(&adjacency);
        bridges.sort_unstable();
        articulation.sort_unstable();
        assert_eq!(bridges, vec![(0, 1), (1, 2)]);
        assert_eq!(articulation, vec![1, 2]);

        // A lone node: no bridge, no articulation point.
        let (bridges, articulation) = bridges_and_articulation(&[Vec::<usize>::new()]);
        assert!(bridges.is_empty() && articulation.is_empty());

        // A cycle contributes neither. The two-node case is NOT this test's: the
        // projection gives one undirected edge with multiplicity two, and the
        // multiplicity filter that keeps it from being a bridge lives in
        // `graph_health_view`, not in this simple-graph helper.
        let triangle = vec![vec![1, 2], vec![0, 2], vec![0, 1]];
        let (bridges, articulation) = bridges_and_articulation(&triangle);
        assert!(bridges.is_empty() && articulation.is_empty());
    }

    /// The triangle count and transitivity read the same projection the fragility
    /// pass does: one triangle and its two pendant edges give one triangle, six
    /// triples, and the ratio between the closed and all triples.
    #[test]
    fn triangles_and_transitivity_count_the_same_projection() {
        let mut adjacency = vec![vec![1], vec![0, 2], vec![1, 3, 4], vec![2, 4], vec![2, 3]];
        let (triangles, transitivity) = triangles_and_transitivity(&mut adjacency);
        assert_eq!(triangles, 1);
        assert!((transitivity - 0.5).abs() < 1e-12, "{transitivity}");

        // No triple at all: the ratio is zero, not undefined.
        let (triangles, transitivity) = triangles_and_transitivity(&mut Vec::new());
        assert_eq!((triangles, transitivity), (0, 0.0));
        let (triangles, transitivity) = triangles_and_transitivity(&mut [vec![1], vec![0]]);
        assert_eq!((triangles, transitivity), (0, 0.0));
    }

    /// A process-wide lock: these tests share the process-global counters and a
    /// single armed delay hook, so they must not overlap each other. They are the
    /// only tests in this binary that drive the request path.
    fn counter_lock() -> MutexGuard<'static, ()> {
        static LOCK: Mutex<()> = Mutex::new(());
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A one-repo knowledge base with two notes, on disk, plus a served socket
    /// and a connected client.
    fn served() -> (
        tempfile::TempDir,
        crate::Engine,
        tempfile::TempDir,
        ServeHandle,
        Client,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap().join("v");
        std::fs::create_dir_all(root.join(".arsumbris")).unwrap();
        std::fs::write(
            root.join(".arsumbris/repo.yaml"),
            "name: v\ntype: au.engine.repo::au-engine\n",
        )
        .unwrap();
        std::fs::write(root.join("a.md"), "see [[b]].\n").unwrap();
        std::fs::write(root.join("b.md"), "# b\n").unwrap();
        let engine = crate::Engine::new(&root, crate::repo::ConfigSource::Empty);
        engine.rebuild();
        let sock_dir = tempfile::tempdir().unwrap();
        let socket = sock_dir.path().join("s");
        let server = serve(engine.handle(), &socket).expect("serve");
        let client = Client::connect(&socket).expect("connect");
        (dir, engine, sock_dir, server, client)
    }

    fn delta(
        before: &crate::verb_metrics::VerbCount,
        after: &crate::verb_metrics::VerbCount,
    ) -> u64 {
        after.count - before.count
    }

    /// Counters count REQUESTS: three reads of one verb and one of another move
    /// exactly those two counts, nothing else, and the histogram accounts for
    /// every request counted.
    #[test]
    fn counters_count_requests_not_versions() {
        let _guard = counter_lock();
        let (_dir, _engine, _sock_dir, _server, mut client) = served();
        let before = snapshot();
        for _ in 0..3 {
            let resp = client
                .query(&serde_json::json!({ "read": "diagnostics" }))
                .unwrap();
            assert_eq!(resp["ready"], true, "the read answers: {resp}");
        }
        client
            .query(&serde_json::json!({ "read": "lifecycle" }))
            .unwrap();
        let after = snapshot();

        assert_eq!(delta(&before["diagnostics"], &after["diagnostics"]), 3);
        assert_eq!(delta(&before["lifecycle"], &after["lifecycle"]), 1);
        for verb in VERBS {
            if verb != "diagnostics" && verb != "lifecycle" {
                assert_eq!(
                    delta(&before[verb], &after[verb]),
                    0,
                    "{verb} moved without a request"
                );
            }
        }
        let row = &after["diagnostics"];
        assert_eq!(
            row.buckets.iter().sum::<u64>(),
            row.count,
            "every counted request sits in exactly one bucket"
        );
    }

    /// The histogram times the HANDLER, not a constant and not the frame's write:
    /// the same verb lands in a low bucket unarmed, and in a bucket at or above the
    /// armed delay once the test hook slows the handler by 50 ms.
    ///
    /// The assertion is a BOUND, not one exact octave: the delayed request must be
    /// at or above bucket 16 (whose lower bound is 32.8 ms, and the sleep is 50 ms),
    /// and strictly above the undelayed one. Pinning the octave would make the test
    /// fail on a loaded machine for a reason that is not about the wiring.
    #[test]
    fn the_histogram_tracks_handler_time() {
        let _guard = counter_lock();
        let (_dir, _engine, _sock_dir, _server, mut client) = served();
        let before = snapshot();
        // The control: an undelayed lifecycle read.
        client
            .query(&serde_json::json!({ "read": "lifecycle" }))
            .unwrap();
        let mid = snapshot();
        test_hooks::delay_verb("lifecycle", std::time::Duration::from_millis(50));
        client
            .query(&serde_json::json!({ "read": "lifecycle" }))
            .unwrap();
        test_hooks::clear();
        let after = snapshot();

        let moved = |from: &crate::verb_metrics::VerbCount,
                     to: &crate::verb_metrics::VerbCount|
         -> Vec<usize> {
            (0..BUCKET_COUNT)
                .filter(|&i| to.buckets[i] != from.buckets[i])
                .collect()
        };
        let control = moved(&before["lifecycle"], &mid["lifecycle"]);
        assert_eq!(control.len(), 1, "one request, one bucket: {control:?}");
        let delayed = moved(&mid["lifecycle"], &after["lifecycle"]);
        assert_eq!(delayed.len(), 1, "one request, one bucket: {delayed:?}");

        let (fast, slow) = (control[0], delayed[0]);
        assert!(
            slow >= 16,
            "a 50 ms handler landed below the 32.8 ms octave: bucket {slow}"
        );
        assert!(
            fast < slow,
            "the histogram did not move with the handler time: unarmed bucket {fast}, \
             armed bucket {slow}"
        );
        assert_eq!(delta(&mid["lifecycle"], &after["lifecycle"]), 1);
        assert_eq!(
            after["lifecycle"].buckets.iter().sum::<u64>(),
            after["lifecycle"].count
        );
    }

    /// A rebuild moves the knowledge base version and leaves the counters alone:
    /// they count requests, so the requests before it are still counted after it,
    /// bucket for bucket.
    #[test]
    fn a_rebuild_does_not_reset_the_counters() {
        let _guard = counter_lock();
        let (_dir, engine, _sock_dir, _server, mut client) = served();
        let before = snapshot();
        for _ in 0..2 {
            client
                .query(&serde_json::json!({ "read": "files" }))
                .unwrap();
        }
        let mid = snapshot();
        engine.rebuild();
        client
            .query(&serde_json::json!({ "read": "files" }))
            .unwrap();
        let after = snapshot();

        assert_eq!(delta(&before["files"], &after["files"]), 3);
        for (bucket, (mid, after)) in mid["files"]
            .buckets
            .iter()
            .zip(after["files"].buckets.iter())
            .enumerate()
        {
            assert!(
                after >= mid,
                "bucket {bucket} lost counts across a rebuild: {mid} then {after}"
            );
        }
        assert_eq!(
            after["files"].buckets.iter().sum::<u64>(),
            after["files"].count
        );
    }

    /// The wire change: `graph_health` carries a `verbs` object ONLY when asked,
    /// beside the payload, one row per verb with a count and a full histogram.
    #[test]
    fn graph_health_serves_the_verbs_only_when_asked() {
        let _guard = counter_lock();
        let (_dir, _engine, _sock_dir, _server, mut client) = served();

        let plain = client
            .query(&serde_json::json!({ "read": "graph_health" }))
            .unwrap();
        assert!(plain["result"]["graph_health"].is_object());
        assert!(
            plain["result"].get("verbs").is_none(),
            "the counters must not appear unless asked for: {}",
            plain["result"]
        );

        let asked = client
            .query(&serde_json::json!({ "read": "graph_health", "verbs": true }))
            .unwrap();
        let verbs = asked["result"]["verbs"]
            .as_object()
            .unwrap_or_else(|| panic!("no verbs object: {asked}"));
        assert_eq!(verbs.len(), VERBS.len(), "one row per verb, none invented");
        for verb in VERBS {
            let row = verbs
                .get(verb)
                .unwrap_or_else(|| panic!("{verb}: no row in the verbs object"));
            let count = row
                .get("count")
                .and_then(|c| c.as_u64())
                .unwrap_or_else(|| panic!("{verb}: no count in {row}"));
            let buckets = row
                .get("buckets")
                .and_then(|b| b.as_array())
                .unwrap_or_else(|| panic!("{verb}: no buckets in {row}"));
            assert_eq!(buckets.len(), BUCKET_COUNT, "{verb}: bucket count");
            let sum: u64 = buckets
                .iter()
                .map(|b| b.as_u64().expect("bucket count"))
                .sum();
            assert_eq!(sum, count, "{verb}: buckets must account for the count");
        }
        // The plain call above answered before this one was read, so it is counted.
        assert!(
            verbs
                .get("graph_health")
                .and_then(|r| r.get("count"))
                .and_then(|c| c.as_u64())
                .unwrap_or(0)
                >= 1,
            "the verb that answers must itself be counted: {}",
            verbs["graph_health"]
        );
    }
}

#[cfg(test)]
mod map_yield_tests {
    use super::map_yields;

    /// The rule is `map >= best`, and the unequal cases pin it.
    ///
    /// A map that matched more terms than every note is the hub this discount exists for: its
    /// breadth is why it matched, not evidence that it answers. A map that matched fewer terms
    /// is already behind and is left alone. The registration in `task-set.md` measured a tie, so
    /// no run has ever distinguished the two directions — this test is what stops a later reader
    /// "fixing" the sign and silently undoing the multi-term case.
    #[test]
    fn a_map_yields_when_it_matched_at_least_as_many_terms_as_the_best_note() {
        assert!(map_yields(3, 2), "the hub case yields");
        assert!(map_yields(2, 2), "a tie yields");
        assert!(!map_yields(1, 2), "a weaker map is already behind");
    }
}
