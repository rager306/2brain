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

### Added

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
