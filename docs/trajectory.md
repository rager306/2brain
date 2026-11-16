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
| read contract | one `VERB_SHAPES` table plus typed payloads for the eight hot reads; both checked by the captured fixtures, not by memory |
| audit | hash-chained `mcp_policy` records on stderr; the tool name is recorded (fixed from the literal `mutation`), the principal comes from `--principal` and Hermes now passes `hermes` |
| judge | batched: 100 questions per call, 0.76 s, 7.6 ms/question (`docs/research/judge-capabilities.md`) |
| rebrand | tiers A, B, **D done**; tier C steps 1–2 done (parser + engine halves, 38 sites), step 3 (schema seams) open |
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

1. **per-verb typed payloads — done** (`270f57f`): eight hot reads check the type of every field they name,
   tolerantly (a missing field is not an error; a field of the wrong type is). Completeness stays the
   client's `outputSchema` job, and the first strict attempt was rejected for coupling eight unrelated test
   files to the payload shape.
2. **one source for the verb list** — `VERB_SHAPES` holds every read verb, but the tool→verb mapping still
   lives in the router. Gate: a test that every read tool names a verb in the table.
3. **result modes — settled, and the first answer was wrong.** Measured: `both` 22.0 MB, `text` 11.6 MB,
   `structured` 10.4 MB over 36 reads. I concluded Hermes reads only text and pinned it to `--payload text`;
   six tool calls then failed in 46 seconds with "has an output schema but did not return structured
   content", and the client's circuit breaker reported the server as unreachable. The schema check lives in
   the MCP client layer, not in Hermes' own source, which is why searching Hermes found nothing. The default
   stays `both` and the Hermes entry is back on it; `text` is only safe for a client that ignores schemas,
   and this one does not. Account: `docs/research/hermes-log-findings.md`.

### 2. Ranking: proximity, paid for by a compact term map

The literal class is the finding: a phrase occurring in exactly one note reaches rank 1 in 57% of cases,
because BM25 scores a bag of terms with no proximity credit. Coverage weighting already moved literal
hit@1 from 0.429 to 0.571 with no regression on topical.

Proximity needs per-posting term positions, which needs room in `MEMORY_BUDGET_PER_DOCUMENT = 6144` bytes.
The term map is where that room is: `term_ids: HashMap<String, u32>` over the vault's **21 001 terms and
156 KB of keys** costs ~1.4 MiB, against ~554 KiB for 10-byte open-addressed buckets over one key arena —
**0.84 KB per document, 14% of the budget** (`docs/research/pogocache-patterns.md` §2). So the compact map
is not a memory diet (1.2% of RSS); it is the payment for proximity.

- **Compact map: built, measured, reverted.** 4 703 → 4 695 bytes per document on the budget-test corpus
  (8 bytes, against ~840 predicted); the vault's vocabulary transfers to ~0.22 KB/doc, below the 0.5 KB/doc
  criterion set in advance. The arithmetic was wrong about what dominates the index — postings do, not the
  term keys. Recorded in `docs/research/pogocache-patterns.md` §3.
- **So proximity must pay for itself differently**: either a tighter posting encoding (delta + varint
  document ids and positions) or a budget raised with the reason written next to it. Deciding that needs the
  budget measured on the real vault, not on the generated corpus.
- **Gate**: proximity lifts literal hit@1 above 0.571 with topical hit@1 not below 0.642, golden `au_search`
  blessed in the same commit, and `memory_bytes()/doc` stays inside the budget after the change.
- **Risk**: the index's per-document footprint doubles with positions; the budget is the constraint, not
  the code.

### 3. Tier C — the engine's on-disk identity

Steps 1–2 landed: the seam module, the parser half (floor, nested markers, memory-FS filter), and the engine
half — 38 places across 12 files now resolve identity per root through `identity_dir` / `identity_marker`,
with the read rule and the write rule separated (`identity_dir_for_write_with` follows the directory a root
already holds; only a root with neither gets `CREATE_IDENTITY_DIR`, the legacy spelling in release 1 so a
previous-release binary still reads what this one writes). The end-to-end proof is `tests/it/identity_dir.rs`:
a `.2brain/` repository is discovered, serves `search`, and its `.auignore` is read from the new spelling.

Two things the step forced: `FileSystem` gained a required `is_dir` (probing `<root>/.2brain/repo.yaml` was
wrong wherever the directory exists without a marker), and `consistency_diagnostics` now takes a filesystem so
its four span joins resolve per root.

Step 3 adds `schema_type_matches`, `canonical_schema_claim` and `is_builtin_repo`, because a predicate alone
leaves every legacy claim unresolvable while the floor check stays silent (`docs/research/tier-c-seams.md`).

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
