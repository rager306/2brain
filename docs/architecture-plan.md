# Architecture plan: guardrails, thin ports, a legible gate, an ontology with a consumer

Trajectory for the directions ranked in `docs/architecture-assessment.md`. This file is the
plan of record; it is re-checked by code and commands, not by its own claims — §7 is a ledger
that only fills with evidence.

Scope: `au-mcp-rs` (the adapter) and the two dogfood defects. The engine's crate graph, the
Node-stack retirement (`docs/phase-d.md`) and anything that changes the wire contract are
**out of scope** and stay separate decisions.

## 1. The contract every merge keeps

These hold after every step, without exception. A step that cannot keep them is not ready.

1. `cargo +1.99.0 test --offline` in `au-mcp-rs`: **39 suites, 173 passed, 0 failed, 2
   ignored** (baseline re-measured 2026-11-15), plus whatever the step adds.
2. `au-engine/tests-golden/engine/*.json` (4 captures) byte-identical, or an intentional diff
   regenerated in the same commit with the reason in the message.
3. `scripts/live_parity.py` on a disposable fixture: 5/5 facts `identical`, exit 0.
4. `scripts/strict-client.py`: PASS on write and delete (the MCP SDK validates
   `structuredContent` against `outputSchema`).
5. `cargo fmt --check` clean; `clippy --locked --all-targets --all-features -- -D warnings`
   clean for `au-mcp-rs`.
6. `--info` still reports 51 registered / 35 advertised / wire schema 29 / 16 MiB frames /
   30 s deadline / 8 in-flight reads.
7. No new advertised tool and no new grant unless the step says so explicitly.

A step's own acceptance criteria come on top; they never replace these.

## 2. Trajectory

### P0 — Guardrails and evidence (no behaviour change except two bug fixes)

**P0.1 Capability axis (direction D, the cheap half).**
`src/capability.rs`: `enum Capability { Read, Mutate, Discover, Watch, Serve, Compose }`,
`fn of(tool: &str) -> Capability` as a 51-row static table, `fn advertised(cap, granted)`.
`au_search_tools` gains an optional `capability` filter (additive). No new tool.
Acceptance: table is total and disjoint over the advertised catalogue; `{"capability":"read"}`
returns 35, `{"capability":"mutate"}` returns 16 with grants and 0 without; a tool added
without a row fails the test.

**P0.2 Architecture tests (executable guardrails).**
`tests/architecture.rs`: (a) no module cycle outside one declared exception pair
(`wire ↔ lib`, scheduled for P1.1); (b) `lib.rs` production-line ratchet, a constant that may
only decrease; (c) `MUTATION_TOOLS` / `COMPOSITION_TOOLS` / `WORKSPACE_TOOLS` are total over the
catalogue and **nested by design** — measured 2026-11-15: 16 / 2 / 3, and both smaller sets are
subsets of `MUTATION_TOOLS`, so a "disjoint" assertion would be wrong; (d) every catalogue entry
has an `output_schema`; (e) every catalogue entry has exactly one capability row (P0.1: 32 read,
16 mutate, 2 discover, 1 watch = 51).
Acceptance: green, and each of (a)–(e) fails when deliberately violated (5/5 checked by hand).
Merge after P0.1 so the ratchet starts from the post-P0.1 number.

**P0.3 Dogfood defect: `au_instances_of` on its last page.**
`next_offset` is `null` there while the declared schema says `integer`; a strict client rejects
the answer. Fix the declaration (union with `null`) and add a *general* test: every paged read
tool is called with a page size that lands on the last page, and the answer is validated against
its declared schema. This is the class, not the instance.
Acceptance: the repro passes; the new test covers every tool that emits `next_offset`;
`schema_types.rs` still green.

**P0.4 Capture the decision in the vault (dogfood).**
Append the defect to `inbox/arsumbris-fork-backlog.md` (P0) and write
`inbox/decision-tool-surface-trajectory.md` (`type: decision`, `status: current`) that points at
this plan. Written through our own mutation tools, read back through `au_content`.
Acceptance: vault `au_diagnostics` unchanged (0), `graph_health.broken_links` unchanged (0),
one commit per note, no other vault file touched.

### P1 — Thin hexagonal (direction A): invert the five cheap seams

Each step is one branch and one merge, in the order below, because 1.4 and 1.1 both rewrite
`lib.rs` regions.

**P1.1 `Transport` port.** `trait Transport { fn exchange(&self, req: &Value) -> Result<Value>;
fn watch(&self, req: &Value) -> Result<Value>; }` in `src/transport.rs`; `UnixTransport` keeps
today's codec; `MemoryTransport` (test support) answers from a script. Move `SCHEMA_VERSION`
out of the crate root so `wire ↔ lib` disappears. Migrate two test suites to `MemoryTransport`
as proof; do not convert all 28 fake-daemon suites in this step.
Acceptance: cycle test (P0.2a) now passes with **no** exception; 173 + tests green; golden
untouched; one suite's fake daemon deleted (−~200 lines) without losing a single assertion.

**P1.2 `bounded_stdio` as a parameter.** One call site (`lib.rs:4629`). Acceptance: identical
behaviour under `tests/stdio_boundaries.rs`; no `Option` plumbed through the handler layer.

**P1.3 `resources` as pure functions.** URI parse/format and the member table take an injected
table instead of reaching for the engine. Acceptance: `tests/resources.rs` 4/4 plus a pure unit
test with no socket.

**P1.4 `schemas` as one table.** Replace 15 `fn *_schema()` with `schemas::for_tool(verb)`;
the 51 `output_schema` attributes read from it. Acceptance: every declared schema byte-identical
to today's (`schema_contract` unchanged), `--info` unchanged. This is the step that makes
direction D generative: the capability table and the schema table are then one table with two
columns.

**P1.5 `ReadLedger`.** Extract `self.served` and its six sites into `src/ledger.rs` with an
explicit API (`record_read`, `check_freshness`). Acceptance: `policy/freshness` and
`policy/prior_read` refusals still distinguishable (they are two codes today, confirmed live);
the six tests that pin them stay unchanged.

After P1: `lib.rs` ≤ 3 200 production lines, gate still the largest remaining block, no cycle.

### P2 — C1: the mutation gate becomes data

**P2.1 Reduction apart from dispatch.** `src/gate.rs`: `fn reduce(&Prepared, &GateInput) ->
Verdict` is pure; `arm_file`/`finish_mutation` keep the IO. Consequences to keep: the slot is
taken last (after editability), a denial names its stage, `deny:<stage>` is audited.

**P2.2 `Prepared` is a value.** `Prepared { stage, member, path, expected_hash, armed_at }`,
serializable, so a mutation can be described, logged, and resumed as data. `Armed`/`Denial`
types move next to it.

**P2.3 `allowed_next` in every denial.** The refusal payload lists the stages reachable from the
current one, so an agent asks "what may I do here" instead of learning the rule by being
refused. Additive wire field → annotate per `au-engine/crates/au-engine/WIRE.md`.
Acceptance: the 31 gate tests unchanged; a property test asserts the transition table is total
and that every denial carries a non-empty `allowed_next`; one live client run (Hermes or
`scripts/mutation-turn.sh`) shows the field.

### P3 — C2: state machines in the agent layer, only where they are earned

**P3.1 Two procedures, expressed once.** `mutation-turn.sh` and `recovery-turn.sh` are already
deterministic state-per-prompt procedures. Write each as a state list with entry/exit checks and
code transitions (a single `scripts/lib/procedure.py` runner, ≤120 lines), then re-run them.
Acceptance: 4/4 and 5/5 as today; each state has a negative case that fails when its check is
removed; the state list is derived from the checks in code, not from prose.
**P3.2 One vault note** (`type: pattern`) recording the rule: the FSM owns the process, the
model owns the reasoning inside a state, only the runtime may declare a state complete. Not a
framework — a rule with two examples.

### P4 — D: make the ontology generative, and prove it by keeping a document honest

**P4.1 Generate `manual/mcp-access.md` and the tools table** from the capability table plus
`tools/list` (51 rows, grant matrix 35/46/51). A test regenerates and compares: drift fails.
Acceptance: the manual states 51/35/46/51 and the real argument names (`type_name`,
`origin_path`), replacing today's stale 44-tool note; the drift test fails when one row is
edited by hand.

## 3. What blocks what

```
P0.1 capability ──> P0.2 guardrails ──> P1.* ──> P2.* ──> P4.1
      │                                   │
      └──────────────> P4.1 <─────────────┘   (schemas table P1.4 feeds it too)
P0.3 defect  (independent, merge any time)
P0.4 vault   (independent, merge any time)
P3.*         (independent of P1/P2; only needs the scripts as they are)
```

The critical path is `P0.1 → P0.2 → P1.4 → P2.2 → P4.1`. P0.3, P0.4 and P3 sit off it and can
land whenever a track is free.

## 4. Parallel work: waves, files, conflicts

The unit of parallelism is a **branch with exclusive file ownership**, merged in a fixed order
after a rebase. Two tracks that edit the same region of `lib.rs` are not parallel, no matter how
independent they look.

### Wave 1 (now)

| track | deliverable | files it owns | parallel with | merge order |
|---|---|---|---|---|
| **B** capability axis | `src/capability.rs`, filter in `search_tools` | new module + `lib.rs` (search region) | C, D | 1st |
| **C** last-page defect | schema union + general paged-read test | `src/schemas.rs`, `tests/schema_types.rs`, `tests/fixtures/` | B, D | 2nd |
| **D** vault capture | backlog P0 entry + `type: decision` note | `/root/vault/inbox/*` only | B, C | any time |
| **A** guardrails | `tests/architecture.rs` | that one file | written in parallel, **merged after B** (its line ratchet must start from B's number) | 3rd |

Checks after wave 1: A's five failure modes demonstrated; 173+ tests; `--info` unchanged;
vault `au_diagnostics` still 0.

### Wave 2 (P1, two passes)

Pass 1 — disjoint files, one merge each, any order: **P1.2** (`bounded_stdio`, 1 site),
**P1.3** (`resources.rs`), **P1.5** (`ledger.rs` + 6 sites in the gate region).
Pass 2 — both rewrite `lib.rs` widely, so they are sequential: **P1.1** (`transport.rs` +
`wire.rs` + read sites), then **P1.4** (`schemas.rs` table + 51 attributes).
Rule for pass 2: whoever is second rebases onto the first and re-runs the golden set before
merging; the first must not touch schema declarations.

### Wave 1b — retrieval probe (evidence track, no engine change)

**R1. Is a bundled retrieval engine worth it?** `docs/graph-engine-assessment.md` §7 step 5 asks
for a spike; this is its place in the plan. It ships evidence, not features.

- Corpus: a deterministic export of the live vault (1 036 notes, 5 046 wikilinks), built from
  files, never written back.
- Three queries, timed on every engine: hub ranking, 2-hop neighbourhood with a type filter,
  and "similar notes inside topic Y" (BM25 + vector with a graph prefilter).
- Engines: HelixDB local server (image, no source build), the current engine (already measured:
  9.8 / 20.5 / 132.8 ms), and a `tantivy` baseline for the text half.
- Acceptance: one table in `docs/retrieval-spike.md` with per-engine latency, a capability
  column for the hybrid query, and a go/no-go written against the rule in §7 step 5.
- Guardrail: no `au-engine` or `au-mcp-rs` file is touched; the export lives under `/tmp`; the
  vault is read-only; the engine's own numbers are re-measured in the same session for fairness.

### Wave 3

**P2** is one track: the gate, its types, the payload field and the tests are one change
because the invariant is the stage order. **P3** may run in parallel with P2 (it touches
`scripts/` and the vault, not `src/`). **P4.1** waits for P1.4 and P0.1.

### Sizing (wall clock, one implementer per track)

| wave | tracks | estimate |
|---|---|---|
| 1 | A, B, C, D | 1 day (A merges last) |
| 2 pass 1 | P1.2, P1.3, P1.5 | 0.5 day parallel, 1 merge each |
| 2 pass 2 | P1.1 → P1.4 | 1.5 days sequential |
| 3 | P2 and P3 in parallel, then P4.1 | 1.5 days |

Total to a legible gate and a generative ontology: about 4–5 working days, with the two
dogfood defects fixed on day 1.

## 5. Risk register

| risk | signal | response |
|---|---|---|
| a seam extraction changes the wire | golden diff that is not intentional | revert the step; the seam was not a seam |
| test churn hides behaviour change | >5% of assertions edited in a step | stop, re-measure, split the step |
| line ratchet gamed by moving code | `lib.rs` shrinks while imports grow | replace the ratchet with a per-responsibility import test (P0.2a extended) |
| capability table becomes a new drift source | a tool with two classes or none | total/disjoint test (P0.1) + generated docs test (P4.1) |
| `MemoryTransport` quietly weakens tests | suites that stop asserting frames | keep at least one suite on the real codec end-to-end |
| gate refactor breaks the slot order | a mutation succeeding without capacity | the 31 existing gate tests are the contract; do not edit them |
| vault capture pollutes the live vault | `au_diagnostics` > 0 or a stray file | one note per commit, read back, stop on any diagnostic |
| parallel tracks collide in `lib.rs` | merge conflicts in the same hunks | the wave rule: one owner per region, second rebases and re-runs golden |

Stop rules: any invariant in §1 broken twice in one step means the step is wrong, not the
invariant. Any step that cannot state its acceptance in a command is not a step — it is a hope.

## 6. Deferred (explicitly not scheduled)

- **Full onion** (`domain` + `ports` + adapter crates): revisit only when a second adapter or a
  second transport is actually scheduled; after P1.1 the `Transport` port already exists, which
  is the only part of it that would pay today.
- **Engine crate split** (`serve.rs` 16 772): separate project, separate risk; the engine serves
  production while this plan runs.
- **Node-stack retirement** (`docs/phase-d.md`): a decision and a window, not a task.
- **Text index (FTS5 `bm25()` or `tantivy`)** and the storage question around it: a product
  feature, not an architecture step. It is the cheapest fix for the measured 133 ms `au_grep`
  and belongs to backlog P1; the storage argument is settled in `docs/graph-engine-assessment.md`
  (do not migrate; a graph store does not touch the text scan). Schedule it before any storage
  discussion, not as part of the port work.
- **TLA+ for the gate**: the vault says a table plus property tests covers this size; revisit if
  the stage count passes ~10 or a concurrency bug appears.

## 7. Ledger (evidence only — no entry without a command)

| step | state | evidence | date |
|---|---|---|---|
| P0.1 capability axis | **done** (`298960c`) | 51 rows: 32 read / 16 mutate / 2 discover / 1 watch; inline test equals the router set and agrees with `policy::MUTATION_TOOLS`; `tests/capability.rs` 4/4; suite 40 suites, 180 passed | 2026-11-15 |
| P0.2 architecture tests | **done** (`e74dde7`) | `tests/architecture.rs` 4/4; each rule proved to bite (leaf reference, undeclared edge, one line over the ratchet, a removed `output_schema`), files restored by sha256; no cycle beyond the 7 declared edges | 2026-11-15 |
| P0.3 last-page defect | **done** (`c34879c`) | `next_offset` declared `["integer","null"]`; both new tests failed before the fix with `declared [integer] but holds null`, and the revert-proof was re-run on the final tree | 2026-11-15 |
| P0.4 vault capture | **done** | vault commit `f26cc54` (`inbox/decision-tool-surface-trajectory.md`, type decision, status current) and `4bcc4f2` (backlog P0 item 4, closed defect with its fix commit); both written and read back through `arsumbris-rs`; `au_diagnostics` 0, `graph_health.broken_links` 0 | 2026-11-15 |
| P1.1 transport port | not started | — | — |
| P1.2 bounded_stdio | **done** (`328baa3`) | `Bounds` parameter + `over(input, output, …)`; two tests on a 16-byte duplex prove the queue blocks a third send and releases it on drain, and that a closed transport refuses; suite 190 passed | 2026-11-15 |
| P1.3 resources | **done** (`a4228c0`) | `member_roots(payload)` and the new `resource_page` are pure; the `resources -> wire` edge is gone (ALLOWED 7 → 6); `lib.rs` 4659 → 4631 production lines and the ratchet tightened to 4631; 4 inline tests without a socket | 2026-11-15 |
| P1.4 schemas table | not started | — | — |
| P1.5 read ledger | not started | — | — |
| P2.1–P2.3 gate | not started | — | — |
| P3.1–P3.2 agent layer | not started | — | — |
| P4.1 generated manual | not started | — | — |
| R1 retrieval probe | **measured 2026-11-15** — not adopted | `docs/retrieval-spike.md`: HelixDB 0.0.16 on 1 036 nodes / 2 730 edges — hubs 103.5 ms vs 9.8 ms, 2-hop count 54.7 ms vs `au_neighborhood` 102.2 ms for 253 materialised nodes, BM25 24.2 ms vs `au_grep` 132.8 ms, variable-length patterns unsupported; decision: tantivy plus our own traversal | 2026-11-15 |
