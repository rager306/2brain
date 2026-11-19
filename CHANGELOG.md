# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog 1.1.0](https://keepachangelog.com/en/1.1.0/), and this project adheres to
[Semantic Versioning 2.0.0](https://semver.org/spec/v2.0.0.html).

Two versions move independently in this monorepo: the **engine** crates (`tb-engine/`, currently
`0.0.5`, inherited from the fork point) and the **MCP server** binary (`tb-mcp-rs`, `0.1.0`). Entries
below state which one they change when it is not both. The binary reports its own version and the commit
it was built from (`2brain --version` → `2brain 0.1.0+<sha>`), so a deployed binary always names its
source.

## [Unreleased]

### Changed

- **The old name is gone: one spelling on disk, on the wire and in every name (engine, adapter,
  tooling, content).** ADR-0005 closes the compatibility window instead of extending it. In one
  migration the identity directory is `.2brain` only (`.arsumbris` is refused, not preferred), the
  type namespace is `tb.engine.*`/`::tb-engine` only (the canonicaliser and the
  `engine-schema-legacy-spelling` advisory are deleted), `arsumbris://` is no longer a resource, the
  state runtime directory is `<state root>/engine/` (was `au-engine`), and the engine binary is
  `tb-engine` (was `au`).
  - Code: `LEGACY_IDENTITY_DIR`, `LEGACY_STATE_DIR`, `LEGACY_ENGINE_NAMESPACE`,
    `LEGACY_BUILTIN_ENGINE_REPO`, `LEGACY_RESOURCE_SCHEME` and the identity-migration rewriter are
    removed with their advisory; `migrate identity` is now a check that reports `current` or
    `missing`. Two negative tests prove the old spelling now *fails* (`concat!`-split so no source
    file spells it).
  - Content: 16 `repo.yaml` names, 79 `deps:`, the templates (now `tb-*-template`), 1 561 wikilinks
    and 306 `au.engine.*` claims moved together — the identity name is a join key, so a half-applied
    rename is a build error. The served vault carried zero `::au-*` links, so nothing in it moved.
  - Disk: 288 `.arsumbris/` directories (12 identities, 276 scenarios) renamed to `.2brain/`; 213
    scenario snapshots re-blessed, and the only diff beyond the path spelling is the renamed type
    files in `engine-namespace-reserved`.
  - `tools/kb/legacy_audit.py --check` now reports **0** live, 0 format, 0 content, 0 directories;
    the only remainder is dated records (`history`, 895) and the lineage paragraph (3).

### Added

- **`tools/kb/legacy_audit.py` and ADR-0005: the measured inventory of the old name and the plan to
  finish it.** The tool classifies every surviving trace by its consumer — `live` (code or an operator
  reads it now), `format` (on disk or wire, owned by a deployed reader), `content` (repository identity
  and wikilinks), `history` (frozen records), `lineage` (the fork paragraph) — and `--check` fails on a
  live trace. Measured 2026-11-19: live 59 (38 `au_agent_guide` in content repos, 21 binary `au`),
  format 1572, content 1643, 288 `.arsumbris/` directories on disk; history 815 and lineage 9 stay by
  design. ADR-0005 makes the rename irreversible by deleting the legacy readers in one atomic migration
  (identity names are a join key, so names, `deps`, registry and wikilinks move together), and leaves the
  live daemon cutover as a separate window under the runbook.

### Changed

- **The engine schema moved to `tb.engine.*` and `::tb-engine`** (engine). The 12 hardwired type
defs, every floor name a file kind assigns, `ENGINE_META_TYPE` and `BUILTIN_ENGINE_REPO` are renamed;
`ENGINE_NAMESPACE` is `tb.engine` and the legacy spelling is a named constant beside it. The legacy
spelling is still ACCEPTED: a file claiming `au.engine.repo::au-engine` is canonicalized forward on
read, and a read argument goes through the same canonicalization (`canonical_query_name`), which is
what keeps a release-1 client answering — before that seam `instances_of {type: "au.engine.repo::au-engine"}`
returned 0 rows. On a copy of the live vault the old and new binaries answer `overview`, `hubs`,
`diagnostics`, `graph_health`, `files` and `search` byte-identically; 242 scenario snapshots were
re-blessed and their diff is the rename plus the hash and sort-position consequences. Not in this
change: dropping the legacy acceptance, which is what closes the window
(`docs/research/release-2-schema-2026-11-17.md`).

- **A new repository gets `.2brain/`, not `.arsumbris/`** (engine). `CREATE_IDENTITY_DIR` is now
`IDENTITY_DIR`, so a root that holds neither spelling is created with the new name. Release 1 wrote the legacy
one so a previous binary could read what it wrote; readers still accept both spellings, so no existing tree
changes meaning and the migration window stays open. This is the first piece of tier C step 7; the `DEFS` rename,
the builtin repo name and the claim canonicalizer remain.

### Changed

- **A fresh home gets `.2brain` as its device root, and the reserved-root check knows both spellings**
(engine). `state_dir_in` preferred an existing `.2brain`, then an existing `.arsumbris`, then — with neither
there — the legacy name, so a new installation wrote the old spelling forever; the third step is now
`STATE_DIR`, the same write rule a repository already follows for its identity directory. `reserved_against`
compared only `<root>/.arsumbris` with the device root, which is exactly the pairing a `.2brain` device root
and a `.2brain` repository identity produce, so the newest layout was the one collision it could not refuse;
it now compares both spellings. Nothing moves on a migrated machine: existence still decides. Found by the
install acceptance in `docs/internal/install-from-checkout-2026-11-18.md`.

### Changed

- **Every operator environment variable lost its `AU_` prefix** (engine, server, tooling). `AU_TRACE`,
  `AU_IDLE_RECONCILE`, `AU_BUILD_COMMIT`, `AU_ENGINE_BIN`, `AU_MCP_RS_BIN` and 25 siblings are
  `TB_TRACE`, `TB_IDLE_RECONCILE`, … now. These are this repository's own operator surface — nothing
  outside it reads them — and the old prefix was the last place a lowercase-name search still found the
  retired project. A host that exported one keeps working only by exporting the new name.

### Fixed

- **A `shutdown` read could lose its own acknowledgement** (engine). `dispatch` signalled the daemon on
  `Request::Shutdown` before the reply frame reached the writer, so `wait_for_shutdown` woke and dropped
  the runtime, killing the connection before the ack was flushed. A client calling `{ "read": "shutdown" }`
  saw `UnexpectedEof` about two times in three. The read loop now records the intent, breaks so the writer
  drains the reply, and only then signals — five consecutive runs of
  `daemon::shutdown_read_couples_ready_and_version` pass.

- **The ingest guard only knew the legacy identity directory** (tooling). `tb-ingest/ingest.mjs`
  refused writes into `.arsumbris/` but not into `.2brain/`, the directory a fresh repo gets, so a
  capture could be written into the current identity directory the engine rejects; its repo-boundary
  check also looked only for `.arsumbris/repo.yaml`, so a repo carrying `.2brain/` was not recognised.
  Both now read a shared `IDENTITY_DIRS` (`.2brain`, `.arsumbris`), and the nested-repository test
  covers the current spelling. `converters/engine.test.mjs` was removed: it imported
  `tb-mcp-rs/src/daemon/broker.ts` and `@arsumbris/au-mcp-sdk`, both gone with the TypeScript daemon,
  so it could not run and no runner referenced it.

- **Live text still named the retired prefix** (engine, adapter, tooling). `tb_engine`/`tb_cli`/`tb_core`/
  `tb_parser` READMEs and comments, the `~/.arsumbris` messages `EntryReserved`/`HomeUnset`/the open
  warning printed, the `engine_schema` lock docs (which also had nested backticks from an earlier replace,
  in the builtin type text and its captured fixture), the `repogen` fixture generator (wrote `.arsumbris/`
  where a fresh repo gets `.2brain/`), `docs/internal/hermes-brief.md` (told Hermes to call
  `2brain__au_overview`/`au_content`, names that answer `tool not found`), and the `tools/fsm` sample log
  (`au_graph_health`, `au_validate_value`). Historical records — `CHANGELOG`, `docs/trajectory.md`,
  `docs/research/`, `docs/internal/*` run logs, and the `au_list_imports → tb_imports` maps — are left as
  written. Left by decision: the `au` binary name, `au.engine.*`/`.arsumbris/` on disk and the
  `arsumbris://` parse alias (the compat window), and the content repos' `au-*` package names (tier C,
  `docs/rebrand-plan.md`).

- **The verification scripts delegated and then checked for tool names that do not exist** (tooling).
  `mutation-turn.sh`, `recovery-turn.sh`, `batch-turn.sh`, `rollback-turn.sh` and `model-turn.sh` ran an
  agent with `--allowedTools 'mcp__2brain__au_content,…'` and then grepped the transcript for
  `"name":"mcp__2brain__au_write_file"`. The adapter has advertised `tb_*` for a release, so the grant
  covered no tool and every check searched for a call that could never appear: four scripts that always
  failed, silently. They name the real tools now. A new test
  (`every_tool_a_script_names_is_one_the_router_advertises`) walks `tb-mcp-rs/scripts` and fails on any
  `mcp__2brain__<name>` the router does not advertise — proven by reintroducing `au_content` and watching
  it fail.

- **CI never ran the real-engine suite** (CI). `cargo test` skips the three `#[ignore]`d tests in
  `tb-mcp-rs/tests/real_engine.rs`, which are the only place the socket path, the member gate and the
  cross-member grant run end to end. The adapter job now builds the engine binary, resolves the shared
  target directory through `cargo metadata`, and runs the suite with `TB_ENGINE_BIN`.

- **The installer's fallback could hide an existing state root** (installer). When `/usr/local/bin`
  refuses the engine, the fallback created `$HOME/.2brain/bin`. On a machine that still keeps its state
  in `$HOME/.arsumbris`, creating `.2brain` makes `repo::state_dir_in` prefer the new, empty root, and the
  old registry, sockets and logs become unreachable. The fallback now resolves the state root by the
  engine's own rule — `$2BRAIN_HOME`, an existing `.2brain`, an existing `.arsumbris`, else `.2brain` —
  and installs there.

- **A user namespace under `au.engine.` raised a false deprecation warning** (engine). `legacy_spelling_diag`
  flagged any claim whose namespace started with `au.engine`, including a knowledge base's own
  `au.engine.custom` type, which canonicalization never rewrites and the migration never touches. The note
  now fires only when the canonical form actually differs from the authored one.

### Security

- **The member grant covers the remaining refactor verbs** (engine + server). `inline`, `promote` and
  `rename_block_id` rewrite cross-file referrers too, and a referrer can live in another member; none of
  them carried the grant, so a policy granting one member still let them write into another. All three now
  accept `allowed_members` and check it against their saga plan through the same `grant_refusal`
  `rename` uses. `inline`/`promote` refuse a cross-repo `path`/`into` by design, which does NOT bound
  their writes: the referrers are what cross the member line. An absent list is the absence of a grant, so
  a policy-free launch is unchanged.

- **The member grant now covers `rename`'s cross-member cascade** (engine + server). `rename` rewrites
  every inbound referrer, and a referrer can live in another member; the engine checked editability but
  never the caller's grant, so a policy granting one member still rewrote and committed references in
  another. `rename` now accepts the optional `allowed_members` the adapter takes from the policy, and the
  engine checks it against the saga PLAN it is about to write, under the write lock — the same rule
  ADR-0003 set for `rename_type`. An empty list is the absence of a grant, not a grant of nothing, so a
  policy-free launch sends nothing and its request is unchanged. (Review finding #16; `inline` and
  `promote` are the same shape and remain open.)

- **A content read could leave its member** (engine). `content` took a path, joined it to the root and
  read it: an absolute path outside every member was fetched, and a symlink inside a member that pointed
  out was followed, so a read-only client could obtain any file the daemon's user can open. The read now
  resolves through the member map first and checks the REAL path when the file exists, refusing both
  cases; an absent path still answers a null read. (Review finding #7.)

- **A member grant did not cover the member the engine actually writes** (server). A path can reach into
  a nested editable member, and the engine assigns it to that member, while the adapter checked the
  member named in the arguments. A policy granting the parent therefore authorised a write inside a
  child it never listed. The adapter now asks the engine who owns the joined path and refuses when that
  owner is not in the allowlist; asked only when a policy lists members, so a launch without one is
  unchanged. (Review finding #21.)

### Fixed

- **A batch that touches one path twice reported `reflected: false` forever** (engine). `reflected`
answers "does the held catalog agree with what the batch wrote", and it compared every op's hash,
including the intermediate ones. The catalog can hold only the final content, so two writes to one
path could never both match and a consumer waiting for the write to be reflected waited without end.
It now compares the FINAL state per path — the last write's hash, or absence for a path the batch
deleted.

- **`--payload structured` left the read gate unarmed** (server). `tb_content` in structured mode
sends the payload only in `structuredContent`, and `served_hash` read the text block alone, so the
caller's very next write was refused for a read it had just made. The hash is now read from
`structuredContent` first, then the text block.

- **One file-addressed read cost two read permits** (server). `file_read` and `target_read` acquired a
permit and then called `read`, which acquired a second, so eight concurrent reads exhausted a limit of
eight with only four in flight. The engine exchange for a caller that already holds a permit is one
helper now (`read_under_permit`), which is also where the cancellation arm lives; the three identical
argument guards collapsed into `file_read_guard`. A new test runs eight `tb_frontmatter` calls
concurrently and fails with "capacity reached" when the double charge is reintroduced.

- **Verification scripts resolved the Cargo target directory by hand** (tooling). The shared
`target-dir` is `.cargo-target` (`.cargo/config.toml`), but `client-matrix.sh` and eight sibling
scripts defaulted to `tb-engine/target` / `tb-mcp-rs/target`; on a clean checkout those do not exist
and the `tb-mcp-rs/target` symlink is untracked. A shared `lib/target-dir.sh` / `lib/target_dir.py`
asks `cargo metadata` now, with the `.cargo-target` fallback and the env overrides kept.
`tb-mcp-rs/scripts/install.sh` does the same instead of trusting the symlink, and `strict-client.py`
no longer hardcodes an absolute checkout path.

- **Fixture daemons outlived their runner** (tooling). A fixture killed without its EXIT trap left a
live daemon and its directory, and the next run skipped starting a daemon merely because the
directory existed. The fixture records its owner's pid, and the reap step kills a daemon whose owner
is gone and removes the stale root, leaving live owners and kept fixtures alone.

- **The verification scripts looked for the engine under the legacy state root** (tooling). Five scripts
(`live_parity.py`, `strict-client.py`, `capture-mutation-fixtures.py`, `rollback_rehearsal.py`,
`lib/disposable-fixture.sh`) joined `$HOME/.arsumbris/au-engine/run` by hand, and their fixtures wrote
`.arsumbris/repo.yaml` with an `au.engine.repo::au-engine` claim. A fresh `$HOME` writes `.2brain`, so every
one of them would miss the daemon it had just started, and the fixture writer was the last writer still
emitting the legacy schema. They now ask `lib/mcp_stdio.py`'s `state_run_dir`/`newest_socket`, which find the
run directory instead of spelling it, and the fixtures are authored in the current spelling.

- **`scripts/run-known-root-tests.sh` looked for `au_engine-*`** (tooling). The test binary is
`tb_engine-<hash>`, so the glob matched nothing but stale `.d` files and the script exited "test binaries not
found" before running a test. Fixed, and the recorded baseline's header and counts now match the measured
suite (752 lib / 741 it).

- **The ignored real-engine tests were dead, and nothing said so** (server + engine). `real_engine.rs`
is `#[ignore]`d, so CI never ran it, and it had drifted in three ways at once: it called tools as
`au_overview`, a name the adapter has not had since the `tb_` rename; it joined
`$HOME/.arsumbris/au-engine/run` to find the private engine's socket, which a fresh `$HOME` no longer
uses; and its recorded read fixtures predated the schema rename and still carried `au.engine.*`. Three
tests, three independent silent failures. They now find the run directory instead of spelling it, call
the tools by their real names, and the fixtures are re-captured — capture and verification agree in one
run. `tb-cli`'s `it` suite had the same literal in `daemon.rs` and failed for it; it is green (36/36).

- **The engine wrote its saga markers into the legacy identity directory of a NEW repository** (engine).
`saga_marker_dir` joined the literal `.arsumbris`, so a repository whose identity directory is `.2brain`
got a second one, `.arsumbris/au-engine/run/`, as a side effect of its first mutation — exactly the
foreign body the reserved-root check and the identity migration then have to reason about. It now
resolves through `identity_dir_for_write_with`, the rule that already says "follow what exists, else the
current name". Three cases are pinned: new, legacy, and neither.

- **`<au-engine-builtin>` was the last wire-visible `au` name** (engine). Every builtin type-def's
`origin_path` carried it. It is `<tb-engine-builtin>` now, and 210 `tb-cli` scenario snapshots were
re-blessed; their diff is that string and insta's `assertion_line` header line, which the re-bless
scripted away so the repository keeps its existing snapshot shape.

### Added

- **`require_clean` reaches `batch` (ADR-0002)** (engine + server). The last verb without the rider. The refusal
names the operation that would introduce the error — `batch op 2 (write_file <path>): <code> in <path>` — which is
the report the earlier `WIRE.md` note said did not exist: the composed change set is built in request order
through `mutate::compute_edit`, the same pure transform the committing op uses, so previewing it answers the
question; a diagnostic's file maps to the LAST op that touched it, and a file no op touched is named `elsewhere
in the tree` rather than blamed on an innocent op. Preview and write read the same held version, because the set
is previewed and then applied inside one `handle_mutation` under the write lock. An optional flag, so it travels
only when a caller sets it: a new adapter paired with an engine that predates this record still answers, and the
flag needs a deploy window to be usable end to end.

- **`tools/probe/mcp_smoke.py`** (tooling). One MCP session over stdio against any installed binary:
a read, a clean create, the hash a read returns, a gated write, and a `require_clean` create that must be
refused with `policy/diagnostics` naming its code. Exit code 1 on the first failure, so an install script can
end with it. The run that motivated it — a binary installed from a clean checkout, 8 checks, doctor 7 ok
— is `docs/internal/install-from-checkout-2026-11-18.md`.

- **The cascade grant travels with `rename_type` (ADR-0003)** (engine + server). The adapter previewed the
  cascade, checked every rewritten member against its policy, then sent the mutation with `old_name` and
  `new_name` only — so the engine planned again on whatever state it held. A referrer added between the two
  requests was rewritten outside the grant. `rename_type` now takes an optional `allowed_members`, and the engine
  checks the plan it is about to execute, under the write lock: a path outside the list refuses the whole
  mutation with `policy/member_grant: rename_type … rewrites N path(s) outside the grant: member <name> (<path>)`,
  and nothing is written, moved or committed. A path with no owning member counts as outside. The adapter sends
  its allowlist only when the policy has one. An engine that predates the argument refuses the unknown field
  rather than running the write unguarded.
- **`tb_write_file` can refuse instead of advising: the first slice of ADR-0002** (engine + server). An optional
  `require_clean` rider makes the write a preconditioned promotion: the engine builds the state the write would
  produce through the preview it already serves, and refuses with `policy/diagnostics` naming every ERROR code
  when that state has one — no file write, no commit, no rebuild. Default off, so the advisory contract and every
  existing frame are unchanged. The adapter sends the key **only when a caller asks for it**: the engine refuses
  unknown fields, so an explicit `false` would break a caller paired with an engine that predates the rider, and
  the halves deploy in a window of their own. Two engine tests cover the refusal (bytes unchanged, the code named)
  and the clean write; an adapter test asserts the key is forwarded. `edit_file`, `create_file` and `batch` do not
  accept it yet, `WIRE.md` says so, and ADR-0002 stays `proposed` until the default path is measured.
- **The clean-check rider now covers `edit_file` and `create_file`** (engine + server). The check moved into one
  helper over the preview's `PreviewOp`, so an edit refuses before its fragment lands and a create refuses before
  the file exists. Tests: a refused edit leaves the bytes untouched, a refused create leaves no file, and a clean
  edit lands once its target type exists. `batch` is the one verb left, and the reason is in `WIRE.md`: a batch
  refusal must name the operation that would introduce the error.
- **`tools/cards/gates.py`** (tooling). `cargo test` prints one `test result:` line per suite and no grand total,
  and the adapter count had been typed by hand three ways — 211 in the card, 256 in the trajectory, 260 from a
  run. The script reads a saved log, sums `passed`, counts `failed`, and writes the value, the failure count, the
  method and the date into `metrics.json`; `--selftest` proves the tally on a synthetic log, and a real run over
  the 46-suite log reproduced 260 with the cards still matching.
- **`2brain migrate identity`** (server). `au migrate identity` was the one engine subcommand the installed
  binary did not re-export, so a workspace owner needed a second binary they were never told about. The
  subcommand forwards to `tb_cli::migrate::identity` unchanged, and a test runs it against a legacy fixture in
  `--dry-run` and asserts the plan is printed, the repo is named, and the tree is byte-identical.
- **ADR-0002 fixes the shape of promote-with-checks** (engine, proposed). An optional `require_clean` rider on
  `write_file`, `edit_file`, `create_file` and `batch`: default false and unchanged, and when true the engine
  applies the change to a scratch copy of the held IR, computes the diagnostics, and refuses with
  `policy/diagnostics` naming every ERROR code before anything reaches the disk. Rejected alternatives are
  recorded: the adapter previewing then writing (two calls are not one precondition), a new `promote` verb
  (promotion is a status change), and write-then-revert (a crash between the two leaves the tree in the state the
  check exists to prevent). None of it is implemented; the ADR names the gate that must pass first.
- **A `NOT_EXPOSED_MUTATIONS` registry, the mutation half of the catalogue test** (server). `WIRE.md` documents
  24 mutations and the adapter sends 16; the eight it does not send had no pin, so a documented mutation could
  appear or vanish without a failing test. `tests/wire_catalog.rs` now names each with its reason, fails when a new
  documented mutation has neither a tool nor a row, and fails when a listed mutation becomes exposed. The read side
  has worked this way since the file was written.
- **`search` lifts a note whose title states the query's rare terms, and a topic map yields to a note** (engine).
Two post-ranking multipliers, neither an index field, so neither can change `matched_terms`, the
`min_should_match` floor or the candidate set: a hit whose file stem states a query term with `idf >= RARE_IDF`
is multiplied by `1 + (stated ÷ rare)²` (at most 2×), and on the ranked path for a query of three or more
distinct terms a `type: moc` hit is halved when a non-map hit matched at least as many terms. `hub-vs-atomic`
moved 8 → 1 with every other pre-registered class unchanged; the indexing route stays rejected
(`docs/research/task-set.md` §4). The wire shape is unchanged — `schema_version` stays 29 — but scores differ,
so the search capture is re-blessed in the same change.

- **`graph_health` names the graph's fragile joints** (engine, adapter). A bridge is an edge whose removal
disconnects its ends; an articulation point is a node whose removal does. Both are computed on the undirected
projection of the same resolved in-scope edges, with parallel edges collapsed for the structure but never reported
as a bridge (removing one leaves the other) and self-loops ignored. The read gains `bridges` (`{a, b}`, smaller path
first, sorted) and `articulation_points` (`{path, in, out, total}`), each capped by `limit`, plus the two counts.
`clustering` reports how tightly that projection is woven: `{triangles, transitivity}`, the triangle count and the
closed-over-all-triples ratio (`0.0` when the projection has no triple). Additive: `schema_version` stays 29; the synthetic fixture and the golden `graph_health` capture are re-captured in
the same change because the payload gained keys. The idea comes from sparx's biconnectivity diagnostics
(`docs/research/sparx-2026-11-17.md`), not from its library, and the algorithm is an iterative Tarjan lowlink so a
deep chain cannot overflow the serving stack.

- **A query term the index never saw is retried as a word form of itself** (engine). `capsules` returned
  nothing while `capsule` returned two notes, because the tokenizer keeps a plural and the corpus has only the
  singular. After an exact miss the index now also looks up `-s`/`-es`/`-ies` and their reverses, so a plural
  costs one dictionary lookup instead of the whole query: on a copy of the live vault `capsules` **0 → 2**.
  It is a lookup, not a stemmer: `capsule` and `capsules` stay separate terms with separate frequencies, no
  score of a query that already worked changes, and the corpus is not re-indexed. It does not fix a
  vocabulary mismatch — `portable capsule` stays 0 because no note holds both words (the vault calls the thing
  an "RVF container"), and that is a wording problem, not a morphology one.

- **`search` matches carry a body excerpt** (engine, adapter). Every `tb_search` match now carries
  `excerpt` — up to 240 characters of the note's BODY around the first query term it holds, one line,
  cut on word boundaries, `…` where an edge was cut — and the page states `excerpt_chars`. The text is a
  slice of the parse the ranking already holds, so quoting a match reads no disk and the index keeps no
  second copy of it. Additive: `schema_version` stays 29; the golden `tb_search` capture is re-blessed
  for the two new fields. On the twelve-task context pack it removes the per-fragment `tb_content` read:
  **4.00 calls and 8.86 KB per answer → 1.00 call and 2.44 KB**, with the evidence unchanged (7/9 found,
  corroboration 7/9 at 1.0). `tools/kb/context_pack.py` labels each fragment `search-excerpt` or
  `content`, because a fragment built from a search answer carries no `hash`, `commit` or `status`.
- **`2brain init <dir>`** — one command to a workspace that works: `.2brain/repo.yaml`, a root README the
  engine expects (so a fresh workspace reports no `repo-missing-readme`), a `git init` when the directory is
  not already a work tree, and the exact next command printed. `--template minimal|blank` copies a starter —
  from an installation's `share/2brain/templates/`, a checkout's `tb-defaults/`, or `$2BRAIN_TEMPLATES` — and
  renames the starter's own identity rather than replacing it, so its `deps:` and `workspace.yaml` survive.
  It refuses a directory that already holds a workspace (even with `--force`), a non-empty directory without
  `--force`, and never overwrites a file.
- **`2brain doctor [<target>]`** — one command that answers "is this working, and if not what do I do": the
  binary's version and build commit, the `au` engine binary and its version, the target, the socket it
  resolves to, the daemon on that socket, the workspace's member and diagnostic counts, and the MCP server's
  own `tools/list` count. Every line is `ok`/`warn`/`fail`; the exit code is non-zero only on a `fail`.
- **`./install.sh`** at the repository root — builds both halves and installs `2brain`, `au` and the starter
  templates under `PREFIX` (default `/usr/local`), then prints the two commands that come next. It replaces
  files and never signals a running daemon; `PREFIX=$HOME/.local ./install.sh` needs no root.
- **`au migrate identity --apply`** — the tier-C migration performs the move it plans: `git mv` of the
  identity directory, exact-byte splicing of every `type:`/`extends:` claim line, one commit per
  repository, never a push. Every precondition is re-checked immediately before writing, and a failed
  commit restores the repository byte-for-byte. A bare `au migrate identity` still only prints the plan.
- **Ranking: phrase adjacency.** Two query terms within five tokens of each other in a note's body now
  raise its score. Measured on a labelled set and a durable-knowledge class before shipping: literal
  hit@1 0.571 → **0.714**, topical 0.642 → 0.660, durable 0.500 → 0.538.
- **A durable-knowledge query class** (`tools/ranking/durable_probe.py`): pairs of notes that describe the
  same subject with different declared `freshness`, so "does the ranker prefer what stays true?" is a
  number instead of an opinion.
- **A candidate engine harness** (`tools/ranking/candidate_engine.py`): a second engine over a copy of the
  vault with its own HOME, registry and socket, so a ranking change is measured without swapping the served
  binary. `eval.py` and `durable_probe.py` accept `--socket`/`--vault`.
- **Cards drawn from measured data** (`tools/cards/`): `metrics.json` records every number with the command
  and date, `render.py` draws light and dark SVGs and refuses to invent a value, and `--check` runs in CI.
- **CI** (`.github/workflows/ci.yml`): adapter fmt/clippy/tests, engine fmt/unit/end-to-end, and a card check.
- **`llms.txt`** and this changelog, so an agent landing on the repository can find the documents that answer
  its question instead of reading the tree.
- **`create_file` — a create that cannot be raced** (engine, adapter). `tb_create_file` used to read the path,
  see nothing, and send a plain `write_file`: one server process serialised that pair, two client processes did
  not, so both read absence, both wrote, and the second silently replaced the first. The engine now has a
  `create_file` mutation whose absence check **is** its write (one `O_CREAT | O_EXCL` open inside the serialized
  mutation); an existing path rejects with `detail.reason: "already_exists"` and the incumbent's hash, and the
  adapter reports it as the same `policy/absence` message the local pre-check uses. Additive: `schema_version`
  stays 29, no golden capture re-blessed. `tb-mcp-rs/tests/create_race.rs` runs two adapter processes against one
  private engine for twenty rounds.

- **`rename_type` is previewable, and its cascade is held to the member grant** (engine, adapter). A type rename
  rewrites the def file, claims, slot shapes, meta and wikilinks wherever they live, but `tb_rename_type` checked
  the grant on the member owning the definition only: a referrer in another member was written even when that
  member was outside the launch's `members` allowlist. `preview_mutation` now accepts `op: rename_type` and answers
  an `affected` list — every path the cascade rewrites, with the member that owns it and what changes there
  (`definition_move`, `claim`, `slot_shape`, `qualifier`, `meta`, `body_use`, `wikilink`) — computed by the
  mutation's own planning code, so the two cannot disagree. The adapter previews before it mutates, and refuses
  with `policy/member_grant` (audited as `denied:member_grant`) when any affected member is outside the grant,
  naming the members and a few paths in each. Additive: one new previewable op and one new product field,
  `schema_version` stays 29. `tb-mcp-rs/tests/real_engine.rs` runs the workspace this needs — member `base`
  defines the type, member `app` holds the instance — against a private engine, refusing with `app` outside the
  grant and applying the same rename once both members are allowed.

### Changed

- **Every shipped template copies into a current workspace** (content). The four starters that still kept a
  `.arsumbris/` identity directory now ship `.2brain/`, and their `.gitignore` entries follow the rename. The
  template test asks both questions at once — no `au.engine.*` spelling and no legacy identity directory — and
  walks every template `init` can resolve, so a new starter is covered the day it lands.
- **The ADR directory holds records only** (engine). `0001-implementation-plan.md` and
  `0001-tool-inventory.md` moved to `tb-engine/doc/plans/`, so no three files share one id; the ADR index also
  stopped calling ADR-0001 proposed, which it had been since the record was accepted on 2026-11-16.
- **BREAKING for callers who relied on the default: `tb_search` and `tb_grep` read the workspace, not its
  dependencies.** Both reads defaulted `scope` to `all`, so a question the served vault could not answer came back
  answered by a dependency repository's README — measured, not hypothesised: *"the exact number of stars on the
  2brain repository"* returned `/root/2brain/tb-mcp-sdk/README.md` as its top hit. `scope` now defaults to `own`
  (the workspace's own editable members) and `all` is an explicit request. The task-level set in
  `tools/ranking/tasks.json` is what made the failure visible.

- **A task-level query set** (`tools/ranking/tasks.json`, `tasks_eval.py`): twelve hand-written tasks — exact
  identifiers, paraphrases, a Russian question about English material, multi-note questions, one ambiguous query,
  and three the base should not answer — scored per class. Its first run found the two defects below.


- **The README quickstart is four commands** — clone and `./install.sh`, then `2brain init my-workspace`,
  then `2brain daemon start my-workspace` with `2brain doctor my-workspace` in another shell. The manual
  `mkdir .2brain` / `printf repo.yaml` / hand-copied-binary steps are gone.
- **BREAKING: tool names are `tb_*`, not `au_*`.** All 52 names in `tools/list`, in `tb_describe_tool`, in the
  skill and in the generated tool map. There is no alias: a client calling `au_content` gets `tool not found`.
  The reason for the break rather than a compatibility window is in `docs/naming.md`; the short of it is one
  consumer under our control and a catalogue that is regenerated and drift-checked, against an alias that would
  keep two names alive everywhere for one release. Vault notes that still say `au_content` are data and move
  separately.

- **Identity is read per root through one seam** (tier C step 2): 38 sites in 12 engine files resolve
  `.2brain/` or `.arsumbris/` through `identity_dir`/`identity_marker`, with the read rule and the write rule
  separated. `FileSystem` gained a required `is_dir`; `consistency_diagnostics` takes a filesystem.
- **A schema claim in the future spelling resolves** (tier C step 3): `schema_type_matches`,
  `is_hardwired_engine_name`, `canonical_schema_claim` and `is_builtin_repo`, applied in three parse
  wrappers, so `type: tb.engine.repo::au-engine` lands on the builtin node instead of vanishing.
- **Postings are delta-encoded varint blocks**, and description positions fit inside them: 4 703 →
  **2 624** bytes per document without positions, 3 359 with them, inside an unchanged 6 144 budget.
- **The MCP adapter names each tool's engine read once** (`READ_VERBS`), and the hot reads are checked
  against typed payloads — tolerantly: the type of a field when present, not its completeness.
- **`docs/internal/`** holds operations (host paths, units, runbooks) and `docs/` holds design; the rule is
  in `AGENTS.md` and `docs/internal/README.md`.
- The shared cargo target directory is **relative**, so a checkout that is not the maintainer's can build.
- The result metadata key is `_meta["io.2brain/engineVersion"]` (was `io.arsumbris/engineVersion`). It is
  diagnostic-only — no client compares it — but it was the one string in the protocol still carrying the retired
  name.

### Fixed

- **`install.sh` finds the binaries it just built** (tooling). The script looked for the engine at
  `tb-engine/target/release`, but that path is a symlink ignored by `tb-engine/.gitignore` — a clean clone has
  no such directory, and the build writes to `.cargo-target` through `.cargo/config.toml`. It now asks cargo for
  the target directory (`cargo metadata`), so the config, `CARGO_TARGET_DIR` and a plain checkout all resolve.
  Verified on a fresh `git worktree`: the old path is absent there and cargo reports
  `<checkout>/.cargo-target`.
- **The map-yield rule is stated as it is implemented** (engine). The function halves a `type: moc` hit when it
  matched **at least as many** terms as the best non-map hit; the doc comment, the commit that introduced it and
  the registration in `task-set.md` all said the opposite inequality. The measured case was a tie (six terms
  each), so no run ever distinguished the two directions. `serve.rs` now states the implemented rule,
  `map_yields` holds it as one function, and `map_yield_tests` pins both unequal cases. The correction is
  recorded in `docs/research/map-yield-contract-2026-11-18.md`; changing the direction needs a new measurement.
- **`2brain init` writes the canonical engine schema, so a fresh workspace opens with zero diagnostics**
  (server). The starter spelled `au.engine.repo::au-engine` in `.2brain/repo.yaml` and
  `au.engine.readme::au-engine` in the root `README.md`, so every new workspace carried two
  `engine-schema-legacy-spelling` hints and `2brain doctor` reported a warn. Two tests were red at that commit —
  `tests/init.rs` asked for zero diagnostics and `tests/doctor.rs` asked for "no diagnostics" — and neither was
  noticed because the window advisory landed after the last full adapter run. Both pass now, and
  `no_shipped_template_carries_the_release_one_spelling` walks every template `init` can copy and fails on any
  `au.engine.` in the result. Five starters were repaired the same way: `au-arscontexta-template`,
  `au-tree-research-template`, `au-weave-template`, `studio`, and the two bundle comments.
- **The README diagnostics stop advising the release-1 spelling** (engine). Three advisories told a writer to
  declare `au.engine.readme::au-engine` — guidance that sends them back into the window the schema rename exists
  to close. They now name `tb.engine.readme::tb-engine`, and a unit test asserts that no README message or fix
  description carries `au.engine.` while every fix names the canonical type.
- **`--help` counts its surface instead of typing it** (server). The SURFACE paragraph said "three mutating
  tools" while the router advertised sixteen — a reader had no way to notice. The text moved out of `lib.rs` to
  `src/help.rs`, where the sentence is built from `capability::CAPABILITIES`, and `tests/cli.rs` now cross-checks
  `--help` against `--info` (both count the catalogue) instead of pinning a number. `lib.rs` lost 39 production
  lines and the ratchet followed down, 4546 → 4507.
- **A rare matched term satisfies `min_should_match` on its own** (engine). The floor was a count applied after
  scoring, so it dropped notes the ranking had already placed. `oracle matrix tests` returned five notes that
  mention the words and not `f/patterns/pattern-oracle-drift-ci.md`, which is about the thing asked; the same
  ranking with `min_should_match: 1` puts that note **third of 83**. A document is now kept when it matches the
  floor's count **or** when its rarest matched term has an idf of at least `RARE_IDF` (2.0, the constant the
  `weak` flag already uses), which is the rule `docs/research/task-set.md` registered before the change.
  Measured on a candidate engine over a copy of the live vault, before and after on the same copy: the
  `mixed-language` class **1/2 → 2/2**, the note at rank 3 of 83 at the default floor (the page is identical to
  the `min_should_match: 1` page), `absent` stays **0/3**, and no other class moves. `exact` is untouched — it
  takes the conjunction path, so it still requires every term. What it does not fix: rarity cannot tell a
  rare-and-unrelated word from a rare-and-central one, so the three `absent` questions still return confident
  answers.
- **The audit ledger names the tool that ran.** 17 of 28 records in a production log said `"tool":"mutation"`;
  the shared mutation helper now takes the tool name, and a test asserts the ledger carries `tb_write_file`
  and `tb_delete_file` rather than a placeholder.
- **`--payload text` is not safe for a schema-validating client.** It broke six tool calls in 46 seconds —
  "has an output schema but did not return structured content" — before the client's circuit breaker reported
  the server as unreachable. The default stays `both`.
- Two leftovers of the crate rename: `golden_captures` expected `tb_diagnostics.json` (the fixture is
  `tb_diagnostics.json`) and `register_wire` expected `tb-engine/config/repos.yaml` (the engine writes the
  builtin repository's own directory).

### Removed

- The TypeScript MCP layers (`au-mcp`, `au-mcp-core`, both adapters, the codegen), the Node daemon and the
  Python `au-mcp-bridge` — the serving path is Rust only.

### Rejected after measurement

Recorded because a rejected change is a result, and a changelog that lists only what shipped teaches the
next reader nothing:

- **A title field in the search index.** Six builds were measured on one frozen vault copy. A weight large enough
to lift `hub-vs-atomic` (rank 8 → 1) also moves `exact-and` from 1/1 to 0/1, and any third field flips `ambiguous`
from 1/1 to 0/1, because a title term counts as matched evidence even when its score weight is zero — it changes
the `min_should_match` floor and the coverage factor, not only the score. A rarity gate repairs `exact-and` and
moves the `exact` `ReasoningBank` task from rank 2 to 6. Variants and numbers: `docs/research/task-set.md` §4.
- **A compact term map** in the search index: 4 703 → 4 695 bytes per document, against ~840 predicted. The
  arithmetic was wrong about what dominates the index; reverted.
- **A freshness prior**, twice: at `c = 0.10` it lifts durable hit@1 0.538 → 0.654 and drops literal hit@1
  0.714 → 0.571 (the value before proximity); at `c = 0.02` it changes nothing and literal still falls.
- **Connecting the engine in-process** instead of over its socket: the daemon holds the graph at 72.6 MiB, so
  every session would duplicate it — and become a second writer of an authoritative store.

## [0.0.5] — 2026-11-12

The fork point, preserved as the tag `archive/upstream-0.0.5` (commit `44b32f4`): the upstream arsumbris
`0.0.5`-era engine, before the Rust MCP serving path, the BM25 index, the write gate and the identity
migration.
