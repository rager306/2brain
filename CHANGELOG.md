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

### Changed

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

### Fixed

- **The audit ledger names the tool that ran.** 17 of 28 records in a production log said `"tool":"mutation"`;
  the shared mutation helper now takes the tool name, and a test asserts the ledger carries `au_write_file`
  and `au_delete_file` rather than a placeholder.
- **`--payload text` is not safe for a schema-validating client.** It broke six tool calls in 46 seconds —
  "has an output schema but did not return structured content" — before the client's circuit breaker reported
  the server as unreachable. The default stays `both`.
- Two leftovers of the crate rename: `golden_captures` expected `tb_diagnostics.json` (the fixture is
  `au_diagnostics.json`) and `register_wire` expected `tb-engine/config/repos.yaml` (the engine writes the
  builtin repository's own directory).

### Removed

- The TypeScript MCP layers (`au-mcp`, `au-mcp-core`, both adapters, the codegen), the Node daemon and the
  Python `au-mcp-bridge` — the serving path is Rust only.

### Rejected after measurement

Recorded because a rejected change is a result, and a changelog that lists only what shipped teaches the
next reader nothing:

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
