# Tier C seams: `.arsumbris` → `.2brain`, `au.engine.*` → `tb.engine.*`

Measured 2026-11-16 in `/root/2brain` (graph index + `lsp symbols`; grep only for counts); no source modified. Plan for
tier C of `docs/rebrand-plan.md`: two releases, accept both, then drop.

**The rule.** Two things are spelled `.arsumbris`; only one moves. A *repository marker* (`.arsumbris/repo.yaml`,
`workspace.yaml`, the locks, `.auignore`) is **identity**, and is migrated. The *device root*
(`~/.2brain/au-engine/{run,config}`) is **layout**, moved by tier B, and tier C must not touch it. A third class —
engine state written *inside* a repo (`<repo>/.arsumbris/au-engine/{run,logs}`) — is not identity, but it moves
with the directory it is joined onto.

## 1. Read sites: 30 identity, 8 device-layout, 3 repo-local state (2 engine, 1 CLI)

**tb-parser, 5 identity** (the walk floor; no root I/O allowed here). `scope.rs:22` `FLOOR_DIR_NAMES` — never
descends, reported on the wire as `MemberIgnoresView.floor` (`tb-engine/src/wire.rs:3052`, test `wire.rs:5715`), via
`is_floored_name` / `is_floored_path` (`fs.rs:17,24`). `fs.rs:56` `nested_repo_roots` and `fs.rs:359,381` `walk_core`
(`FlooredDir` vs `RepoMarker`, nested-repo boundary) and `fs.rs:626` `walk_files` — the same decision in the memory FS.

**tb-engine, 24 identity**

| site | function | decides |
|---|---|---|
| `repo.rs:201,202` | `REGISTRY_REL`, `REPO_LOCK_REL` | the relative path 13 further joins use: `register_entry:1248`, `malformed_candidate:1540`, `resolve_repo_name:1712`, `assemble_members:1883,1962`, `registry_files:2016`, `lock_files:2075`, `discover_repos:2126`, `parse_repo_deps:2356`, `declared_name_at:2477`, `entry_repo_load:2492`, `readme.rs:127`, `pkgcache.rs:531` |
| `repo.rs:603,614` | `repo_lock_path`, `workspace_lock_path` | lock write/read paths |
| `repo.rs:2523` | `folder_repo_entry` | is this `workspace.yaml` a folder-repo manifest |
| `build.rs:1459,1491` | `resolve_entry`, `is_folder_repo_manifest` | entry manifest; entry-only vs composed workspace |
| `build.rs:1551` | `member_walk_filter` | which `.auignore` scopes a member |
| `build.rs:1910,2019,2086,2110,2132` | `report_undeclared_nested_repos`, `consistency_diagnostics` | diagnostic spans; "the engine's own" member |
| `serve.rs:6724` | `composition_write` | write target of `add_workspace_member` / dep verbs |
| `serve.rs:7287,11800` | `set_config_repo`, `config_response` | repo-scope config base |
| `serve.rs:7391,7434` | `set_ignores` | `.auignore` read and write |
| `wire.rs:3010` | `member_ignores` | the `ignores` read |
| `dirop.rs:619` | `check_destination_ancestors` | refuse a move into a nested repo |
| `diagnostics.rs:111` | `auignore_empty_scope_diag` | span of `auignore-empty-scope` |
| `incremental.rs:149`, `engine.rs:366` | `is_engine_schema_path`, `scope_filter_dirty` | a schema/`.auignore` edit forces a full rebuild |
| `mutate.rs:94` | `resolve_repo_path` | write guard ("writes under `.arsumbris/` are rejected") |

**Device layout, 8, unchanged.** `repo.rs:819` (`HomeUnset` message), `:842` `STATE_DIR = ".2brain"`, `:848`
`LEGACY_STATE_DIR`, `:856` `state_dir_in`, `:892` `reserved_against`, `:937` `user_registry_path`, `:1117`
`user_workspaces_path`, `build.rs:86` (`ReservedRoot`). All read `$HOME`; editing them repeats the tier-B trap.

**Repo-local engine state, 3, moves with the directory.** `serve.rs:4548` `saga_marker_dir`;
`tb-cli/src/daemon.rs:736` `install_tracing` (`AU_TRACE` under `<entry>/.arsumbris/au-engine/logs/trace/`); the namespace
documented at `ignored_dir.rs:4`. Writes, not markers: a release-1 binary must find them under the new name after the move.

## 2. `au.engine.` sites (39 non-comment occurrences in `src`)

- **parsing / matching, 28**: `engine_schema.rs:42,47,100` (`ENGINE_META_TYPE`, `ENGINE_NAMESPACE`,
  `is_engine_namespace`), `:193-310` (`DEFS`: 12 names plus field refs), `readme.rs:32` (`README_TYPE_NAME`); floor
  table `build.rs:1941-1943`; stamped names `build.rs:244,273,1957`; `wire.rs:2688,2689`; `pkgcache.rs:762,763`.
- **engine-file writing, 4**: `repo.rs:1038` `serialize_user_registry`, `:657` `serialize_package_lock`
  (`type: {type_name}::au-engine`), `serve.rs:6851` `add_workspace_member`.
- **documentation / messages / fixes, 7**: `engine_schema.rs:142`, `readme.rs:133,144,147,158,161`, `build.rs`.
- **builtin repo identity**: `BUILTIN_ENGINE_REPO = "au-engine"` (`engine_schema.rs:35`), 18 non-test references:
  `engine_schema.rs:363`, `repo.rs:467,664,982`, `build.rs:245,280,423,460,806,824,829,1958,2639`, `wire.rs:2344,2720`,
  `incremental.rs:882,1085`, `ensure_mixin.rs:189`. **Test fixtures** hold the rest: 273 `.rs`, 384 `.yaml`, 3 174 `.snap`.

**repo.yaml count.** 308 tracked `repo.yaml` (the 314 in `naming.md` is 6 high today): **304** carry
`type: au.engine.repo::au-engine`; 1 the wrong floor (`tb-engine/scenarios/engine-schema-type-floor-omitted/`); 3
no written `type:` (`tb-type-system/.arsumbris/repo.yaml`, two fixtures). Rewritten: **304 claim lines + 308
directory moves**, 30 `workspace.yaml` (all carry `au.engine.workspace::au-engine`), 12 `.auignore` (move only), 0
tracked locks, and 11 more `extends: au.engine.meta::au-engine` lines (8 in `tb-mcp-sdk/type/`, 3 in a
`tb-type-system` note; 18 more sit in `tb-engine/scenarios/`). Member `name:` values are **not** rewritten.

## 3. Seam functions

- `tb-parser/src/identity.rs` (new, re-exported from `lib.rs`): `pub const IDENTITY_DIR: &str = ".2brain"`,
  `LEGACY_IDENTITY_DIR: &str = ".arsumbris"`, `pub fn is_identity_dir_name(name: &str) -> bool` (floor predicate, no
  I/O), `pub fn identity_dir_with(root: &Path, exists: impl Fn(&Path) -> bool) -> PathBuf`, `identity_marker_with(…)`.
- `tb-engine/src/repo.rs`: `pub fn identity_dir(root: &Path) -> PathBuf` =
  `identity_dir_with(root, |p| p.exists())`; `pub fn identity_marker(root: &Path) -> PathBuf`, the only source
  for `REGISTRY_REL` joins.
- `tb-engine/src/engine_schema.rs`: `pub(crate) fn schema_type_matches(name: &str) -> bool` (namespace, both
  spellings); `is_hardwired_engine_name(name: &str) -> bool` (DEFS name, both); `canonical_schema_claim(c:
  TypeNameClaim) -> TypeNameClaim`; `is_builtin_repo(name: &str) -> bool`.

**Read rule.** `.2brain` wins when it exists, `.arsumbris` only when it does not — **per root, never per file**, so one
repo cannot yield two markers; the probe is a closure because the walk must ask a `MemoryFileSystem` (a key map), not
`std::fs`. **Write rule.** Follow the existing directory; if neither exists create `CREATE_IDENTITY_DIR` (`.arsumbris`
in release 1, `.2brain` in release 2), which keeps the previous binary reading every tree release 1 writes.

**The seam the inventory missed: a predicate cannot make a claim resolve.** `type: au.engine.repo::au-engine`
resolves by name against the builtin graph (`crossref.rs:267` `peer_type_id` → `repo.rs:437` `by_name`). Rename `DEFS`
and every member's claim finds no def → `unknown-type-claim` everywhere, while the floor check sees an empty closure
and skips (`build.rs:814-815`) — the floor signal is silent, the failure is not. `canonical_schema_claim` rewrites every
claim a file yields in the two tb-engine wrappers (`parse.rs:243` type-def, `parse.rs:465,579` instance) — not in
tb-core, which stays domain-pure — when the repo qualifier `is_builtin_repo`, or the claim is unqualified and
`is_hardwired_engine_name`; a knowledge-base def named `au.engine.custom` keeps its own identity. Release 1 keeps the
legacy spelling canonical (`DEFS`, closure hashes, golden fixtures, 215 snapshots unchanged); release 2 flips the constant
and renames `DEFS`. `is_builtin_repo` is the fourth seam: the builtin's displayed name stays `au-engine` in release 1
(`repo`/`types` wire strings) and flips in release 2.

## 4. `au migrate identity --dry-run`

New `TopCommand::Migrate { identity: { dry_run, root: Option<PathBuf>, all: bool } }` in `tb-cli/src/main.rs`;
no daemon, no graph. **Reads** `ConfigSource::User` → `repos.yaml` + `workspaces.yaml` (paths only) plus `--root`;
per repo `git status --porcelain` (clean required), `git ls-files .arsumbris`, each identity file's `type:` line,
`saga_marker_dir(root)/saga.intent` (refuse: mutation in flight), the daemon socket (refuse: live writer).
**Rewrites**, one commit per repo: `git mv .arsumbris .2brain`; the `type:` line of `repo.yaml`, every
`workspace.yaml`, `repo.lock`/`workspace.lock` when present, and `extends: au.engine.meta::au-engine` in member
content — byte splices keeping comments and order (`mutate::splice_*`, as `add_workspace_member` does); the
machine-scope `repos.yaml`/`workspaces.yaml`. Commit `migrate identity: <name>`; no push. **Reports** the plan, then
`{migrated, already, skipped, errors}` with each rewritten file and line; `--dry-run` prints the same plan, exits 0,
and must not create `.2brain` to "check". **No-op when migrated**: the plan derives only from the legacy spelling (a
repo is in scope iff `.arsumbris/` exists or some `type:`/`extends:` matches `au.engine.*` / `::au-engine`), so a
second run derives an empty plan, writes nothing, commits nothing. Test: run twice; second plan empty, HEAD unchanged.

## 5. Steps (each keeps `cargo test` green)

| # | step | if skipped |
|---|---|---|
| 1 | add `identity.rs`; change no call site | later steps each invent a join, and one forgets `fs.rs` |
| 2 | route the 30 sites through it; `FLOOR_DIR_NAMES = [".git", ".2brain", ".arsumbris"]`; wire `floor` reports both; re-bless `wire.rs:5715` | a migrated repo's `.2brain/` is walked as content: `repo.yaml` ingested as a node, plus a diagnostic per member |
| 3 | add `schema_type_matches`, `canonical_schema_claim`, `is_builtin_repo`; canonical stays legacy; no `DEFS`/writer change | step 5's output is unreadable by the same binary: `unknown-type-claim` per member |
| 4 | add `au migrate identity`, tested on a tempdir git fixture and with `--dry-run` twice | the move becomes a hand `mv` + `sed`, with no clean-tree refusal and no per-repo commit |
| 5 | migrate one leaf member (`tb-type-system`), then the monorepo one commit per repo, then the vault | — the one step the *previous* binary cannot read |
| 6 | gate `tb_diagnostics == 0`, `graph_health.broken_links == 0`, golden reads unchanged | a partial migration ships "green" because each repo alone is consistent |
| 7 | release 2: flip `CREATE_IDENTITY_DIR`, rename `DEFS`, `BUILTIN_ENGINE_REPO = "tb-engine"`, writers emit new, canonicalizer maps legacy → new; re-bless snapshots | writers keep emitting a dead spelling and the format never converges |
| 8 | release 3: drop the legacy spelling, with a diagnostic naming `au migrate identity` | a hard failure with no pointer to the fix |

**Compatibility.** Steps 1-4 change no file, so the previous binary reads them. Step 5 is the format change: the previous
binary refuses a migrated tree (it looks for `.arsumbris/repo.yaml`, and would ingest `.2brain/repo.yaml` as a note — loud,
non-destructive, `tb_diagnostics > 0`). Order: install release 1, then migrate; rollback = `git revert` the migration commit
**and** reinstall the old binary. The tier-B `symlink .arsumbris -> .2brain` does **not** transfer: the walk floors by
*name*, so the old binary walks the new name as content.

## 6. Three likely regressions

1. **Two identity dirs in one repo** (half-finished `mv`, a copy, an old binary recreating `.arsumbris/`): the repo is
   discovered twice, or a stale legacy `repo.lock` wins. Test: fixture with both dirs holding the same `name:` → exactly
   one `Repo`, no `duplicate-repo-name`, `lock_files` reads the `.2brain` lock.
2. **Legacy claim unresolvable after the `DEFS` rename** (release 2 without `canonical_schema_claim`):
   `unknown-type-claim` on every member. Test: parse `type: au.engine.repo::au-engine` against release-2 `DEFS`;
   assert zero diagnostics and that the floor id resolves (not the empty-closure skip at `build.rs:814`).
3. **Schema paths stop forcing a rebuild.** `is_engine_schema_path` (`incremental.rs:149`) and `scope_filter_dirty`
   (`engine.rs:366`) match the component `.arsumbris`, so a `.2brain/repo.yaml` edit classifies as content and a member
   add or scope change never re-walks, silently. Test: `classify_dirty` on `.2brain/repo.yaml` → `NeedsFull`, plus an
   end-to-end `set_ignores` on a migrated fixture.
