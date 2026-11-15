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
   `hubs`, `graph_health`) — the shape table is the first half; typing the payload is the second, and the
   gate is that a payload field the tool promises cannot disappear without a compile error.
2. **one source for the verb list** — `VERB_SHAPES` holds every read verb, but the tool→verb mapping still
   lives in the router. Gate: a test that every read tool names a verb in the table.
3. **result modes** — measured and decided (`docs/research/payload-modes.md`): the default stays `both`,
   Hermes is pinned to `--payload text` (47.1% fewer bytes, JSON-equal). Revisit only for a
   schema-validating client.

### 2. Ranking: proximity, paid for by a compact term map

The literal class is the finding: a phrase occurring in exactly one note reaches rank 1 in 57% of cases,
because BM25 scores a bag of terms with no proximity credit. Coverage weighting already moved literal
hit@1 from 0.429 to 0.571 with no regression on topical.

Proximity needs per-posting term positions, which needs room in `MEMORY_BUDGET_PER_DOCUMENT = 6144` bytes.
The term map is where that room is: `term_ids: HashMap<String, u32>` over the vault's **21 001 terms and
156 KB of keys** costs ~1.4 MiB, against ~554 KiB for 10-byte open-addressed buckets over one key arena —
**0.84 KB per document, 14% of the budget** (`docs/research/pogocache-patterns.md` §2). So the compact map
is not a memory diet (1.2% of RSS); it is the payment for proximity.

- **Order**: compact map first, measured alone; then positions, measured against the same 60 queries.
- **Gate**: compact map changes no ranking output and saves ≥0.5 KB/doc; proximity lifts literal hit@1
  above 0.571 with topical hit@1 not below 0.642, golden `au_search` blessed in the same commit.
- **Risk**: the index's per-document footprint doubles with positions; the budget is the constraint, not
  the code.

### 3. Tier C — the engine's on-disk identity

Step 1 landed (`tb-parser/src/identity.rs`, five tests, no call site changed). Step 2 routes the 30 identity
read sites through it and makes the walk floor accept both spellings — a **wire change**
(`MemberIgnoresView.floor`), so its snapshot is re-blessed in the same commit. Step 3 adds
`schema_type_matches`, `canonical_schema_claim` and `is_builtin_repo`, because a predicate alone leaves every
legacy claim unresolvable while the floor check stays silent (`docs/research/tier-c-seams.md`).

- **Gate**: a migrated repository opens with 0 diagnostics and the unmigrated tree still opens.
- **Cost**: the largest single change left, and the only one that touches files on disk.

### 4. Counters, then whatever they justify

The engine has no per-verb counters, so "is `au_grep` slow or is the vault big" is answered with a
stopwatch. One histogram per verb, aggregated per version, is ~80 lines and makes the next protocol or
ranking experiment answerable without one. Nothing else in this queue should be sized before that data
exists.

### 5. The recency query class — a prerequisite, not a feature

The freshness prior was measured, cost 1.9 points of topical hit@1 and was reverted: the labelled set asks
"which note holds these words", and the expected note does not change when the note goes stale. A freshness
prior answers "what is *currently true*", which needs queries whose expected answer changes over the note's
lifecycle.

### 6. Tool names — one decision, then a mechanical commit

52 names, 618 occurrences in 81 files, 34 fixtures named after them, 91 vault mentions. The safe form is
`tb_*` with `au_*` accepted as a deprecated alias for one release. Nothing moves until the prefix is
chosen; `docs/naming.md` records the open state.

## What runs in parallel, and why it is safe

These tracks touch different files and different gates, so they can run at the same time:

| track | touches | gate | conflicts with |
|---|---|---|---|
| MCP path (typed payloads) | `tb-mcp-rs/src/{wire,schemas,lib}.rs` | adapter suite + live parity | none |
| compact term map, then proximity | `tb-engine/crates/tb-engine/src/search.rs` | ranking eval + engine suite | none |
| tier C step 2 | `tb-parser` (`scope.rs`, `fs.rs`), `tb-engine` repo discovery, `wire.rs` floor | vault opens with 0 diagnostics | none (different crates) |
| verb counters | `tb-engine/src/serve.rs` | engine suite | tier C (same crate) |
| tool names | every tool string | adapter suite + live server | MCP path (same files) |


The one ordering constraint: **the tool rename comes after the MCP path**, because both rewrite the router
and the schemas, and a rename on top of an unfinished refactor makes the diff unreadable.
