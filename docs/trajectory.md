# Trajectory after the rebrand — what stands, what is open, what runs in parallel

Written 2026-11-16, revised the same evening after tier B, tier D, the wire refactor and the first measured
ranking change landed. This replaces "what next" with a queue where every open item names its gate, its
cost, and the evidence it would produce.

## Where the system stands

| | state |
|---|---|
| serving path | `/usr/local/bin/2brain` → engine socket; Rust only, one binary (`2brain mcp <target>`) |
| state root | `~/.2brain` (legacy `~/.arsumbris` is a symlink), unit `2brain-engine.service` |
| tools | 52 (33 read, 16 mutate behind three grants, 2 discover, 1 watch) |
| read contract | one `VERB_SHAPES` table; the table is checked by the captured fixtures, not by memory |
| judge | batched: 100 questions per call, 0.76 s, 7.6 ms/question (`docs/research/judge-capabilities.md`) |
| rebrand | tiers A, B, **D done**; tier C open with an inventory (`docs/research/tier-c-inventory.md`) |
| tool names | `au_*` — a client contract; the rename is a decision, not a sweep (`docs/naming.md`) |
| FSM run log | done — `tools/fsm/check_run.py`, seven rules, `--selftest` proves each bites |
| hygiene pass | done and applied — `tools/hygiene/pass.py`, `inbox/hygiene-2026-11-16.md` |
| ranking | coverage weighting landed and measured; freshness prior measured and **rejected** |

Gates that must stay green on every change: adapter 207 tests, engine `--lib` 685/1 (the one failure is the
documented root-runner), `search_read` 6/6, `fmt --check`, `clippy -D warnings` on the adapter, and
`scripts/live_parity.py` against the installed binary.

## The open queue, in order of payoff per unit of risk

### 1. The MCP↔engine path (`docs/research/mcp-engine-path.md`)

Measured: a raw engine frame costs 1.1–1.2 ms with or without a fresh connection; the adapter adds ~1.7 ms
including the client's own stdio round trip; the daemon holds the IR at 72.6 MiB, so an in-process engine
would duplicate that per session *and* become a second writer. What is left is not transport, it is
ceremony:

1. **per-verb typed payloads** for the hot reads (`overview`, `content`, `files`, `search`, `diagnostics`,
   `hubs`, `graph_health`) — the shape table is the first half; typing the payload is the second. Cost:
   a day. Gate: the fixture replay test fails to compile when a payload field it pins disappears.
2. **one source for the verb list** — `VERB_SHAPES` now holds every read verb, but the tool→verb mapping
   still lives in the router. Gate: a test that every `au_*` read tool names a verb in the table.
3. **`--payload structured` as the default** — measured before deciding (`docs/research/payload-modes.md`).

### 2. Ranking: proximity (`tb-engine/crates/tb-engine/src/search.rs`)

The literal class is the finding: a phrase occurring in exactly one note reaches rank 1 in 57% of cases,
because BM25 scores a bag of terms with no proximity credit. Coverage weighting already moved literal
hit@1 from 0.429 to 0.571 with no regression on topical.

Next: store term positions in the index and let adjacency raise the score. That changes the index structure,
so it must be measured for memory against the 6 144 B/doc budget before it is measured for quality.

- **Gate**: `tools/ranking/eval.py` on the same 60 queries — literal hit@1 above 0.571 with topical hit@1
  not below 0.642; golden `au_search` capture blessed in the same commit.
- **Cost**: ~120 lines in `search.rs` + the index structure + a memory measurement.
- **Risk**: the index doubles its per-document footprint; the budget is the constraint, not the code.

### 3. Tier C — the engine's on-disk identity

`.arsumbris/` (79 literals, 12 files) and `au.engine.*` (52 occurrences, 28 files) plus 314 `repo.yaml`
files. Two releases: accept both behind `identity_dir()` and `schema_type_matches()`, then migrate with
`au migrate identity --dry-run`, then drop the legacy spelling. Seam plan: `docs/research/tier-c-seams.md`.

- **Gate**: a migrated repository opens with 0 diagnostics and the unmigrated tree still opens.
- **Cost**: the largest single change left, and the only one that touches files on disk.
- **Why not first**: it buys brand consistency, not capability.

### 4. The recency query class — a prerequisite, not a feature

The freshness prior was measured, cost 1.9 points of topical hit@1 and was reverted. The reason is that the
labelled set asks "which note holds these words", and the expected note is the expected note whether it is
`current` or `stale`. A freshness prior answers "what is *currently true*", which needs queries whose
expected answer changes over the note's lifecycle. Until that class exists, any freshness constant is a
guess with a plausible story.

- **Gate**: a third class in `tools/ranking/labelled-queries.json` where the expected note is the one that
  supersedes an older note; measured before and after the prior.
- **Cost**: building the class is the work; the prior itself is already written and reverted.

### 5. Tool names — one decision, then a mechanical commit

52 names, 618 occurrences in 81 files, 34 fixtures named after them, 91 vault mentions. The safe form is
`tb_*` with `au_*` accepted as a deprecated alias for one release. Nothing moves until the prefix is
chosen; `docs/naming.md` records the open state.

## What runs in parallel, and why it is safe

These tracks touch different files and different gates, so they can run at the same time:

| track | touches | gate | conflicts with |
|---|---|---|---|
| MCP path | `tb-mcp-rs/src/{wire,schemas,lib}.rs` | adapter suite + live parity | none |
| ranking | `tb-engine/crates/tb-engine/src/search.rs` | ranking eval + engine suite | none |
| tier C | `tb-engine/crates/tb-core`, `tb-engine` repo discovery | vault opens with 0 diagnostics | none (different crates) |
| tool names | every tool string | adapter suite + live server | MCP path (same files) |

The one ordering constraint: **the tool rename comes after the MCP path**, because both rewrite the router
and the schemas, and a rename on top of an unfinished refactor makes the diff unreadable.
