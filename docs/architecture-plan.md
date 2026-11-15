# Architecture plan: guardrails, thin ports, a legible gate, an ontology with a consumer

**Status 2026-11-16: closed.** Every track is either executed or cancelled by measurement, and the
adapter this plan describes is now the only serving path (`/usr/local/bin/2brain`). The closeout with
before/after numbers is in §8; `docs/detach-and-consolidate.md` carries what came after, and
`docs/repo-inventory.md` describes the tree this plan produced.

Trajectory for the directions ranked in `docs/architecture-assessment.md`. This file is the
plan of record; it is re-checked by code and commands, not by its own claims — §7 is a ledger
that only fills with evidence.

Scope: `au-mcp-rs` (the adapter) and the two dogfood defects. The engine's crate graph, the
Node-stack retirement (`docs/phase-d.md`, **executed 2026-11-16**) and anything that changes the wire
contract are **out of scope** and stay separate decisions.

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

**P1.4 `schemas` as one table — reconsidered, not done as written (2026-11-15).** The 15
functions encode genuinely different shapes: an envelope over one verb, the instances envelope
with its own counts, grep's match array, the typed/type-tree/follow shapes, the mutation answer,
the batch and reconcile answers. Merging them into `for_tool(name)` moves each shape into a match
arm and removes no knowledge — the "one table, two columns" win was about *adding a tool in one
place*, and the invariant that actually matters (every advertised tool declares an output
schema) is already asserted by `tests/architecture.rs`. So the step is dropped as churn.

What replaces it: when T1 adds `au_search`, the new tool's declaration must be written once, in
the same file as its capability row's neighbour, and `tests/architecture.rs` must still pass
without touching its ALLOWED list or the 51-row count (the count becomes 52 — that is the one
number T1 is allowed to change).

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

**P3.1 Two procedures, expressed once — layer decided 2026-11-16.** The runtime is
`scripts/lib/procedure.py`, i.e. **tooling**, not product: its only consumer is `mutation-turn.sh`
and no Rust file imports it. The split is now written into `AGENTS.md` (product code = Rust only;
tooling = `scripts/`/`tools/`, any language; models = allowed but labelled and never cited as
shipped). If a product consumer ever appears — the server running a procedure itself — the runtime
moves to `au-mcp-rs/src/procedure.rs` and gains a tool.

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

## 2b. Status after the first execution pass (2026-11-15)

| step | state | evidence |
|---|---|---|
| P0.1 capability axis | done `298960c` | 51 rows; partition test 19 + 32 |
| P0.2 architecture tests | done `e74dde7` | 4 rules, each proved to bite |
| P0.3 last-page defect | done `c34879c` | union + revert-proof |
| P0.4 vault capture | done | vault `f26cc54`, `4bcc4f2` |
| P1.1 transport port | done `4b66625` | cycle gone; `MemoryTransport`; 2 tests |
| P1.2 bounded_stdio | done `328baa3` | backpressure proved on a 16-byte duplex |
| P1.3 resources | done `a4228c0` | pure functions; `resources -> wire` edge removed |
| P1.4 schemas table | dropped | 15 shapes are not one table |
| P1.5 read ledger | done `3e402e8` | `ReadLedger` + 3 tests |
| T1 phase 1 | done `3d4bc40` | `search.rs`; **release** p50 79 µs OR / 9.7 µs exact, 4.7 KiB/doc |
| P2 first slice | done `ac91a9d` | `Denial` beside `Stage`; P2.3 remedy table measured out |
| P2 core | done `2f00339` | `gate.rs`: pure `reduce` + 7 tests; `lib.rs` 4521 → 4424 |
| T1 engine half | done `d888abe` | index on `KnowledgeBase` + `search` parity fact; `Request::Search` from held state; 6 integration tests |
| T1 adapter half | done `c3472e5` | `au_search` is the 52nd tool; counts moved together; suite 203 |
| T1 dead code | done `44c6967` | `Bm25Index::is_empty` removed with the crate-wide allow |
| W4.1 phase 1 | done `6b8f0f0` | `tools/judgement/`: 50-note labelled sample + gated client |
| W4.2 roles | done `3c16fc7` | `docs/agent-roles.md` |

Ratchet trail, which is the cheapest honest progress metric: `lib.rs` production lines
4 636 (start) → 4 612 → 4 561 → **4 521**, and the budget follows it down each time.

Three measurements changed the plan since it was written, and are the reason this section
exists: the reconciliation of the text gap (T1 is worth it and is cheap), the refutation of the
remedy table (the refusal messages already say what to do), and the collapse of P1.4 (the schema
functions carry distinct shapes).

## 2c. What is left, and what can run at the same time

| track | work | crate / surface | conflicts with |
|---|---|---|---|
| **A** P2 core | `gate.rs`: pure `reduce(stage, ctx) -> Step`, `Prepared` as a value, the stage loop moved out of `lib.rs`; 31 gate tests are the net | `au-mcp-rs` (`lib.rs`, new `gate.rs`) | nothing (B is another crate, C is tooling) |
| **B** T1 phase 2 | `Request::Search` + args, index on `KnowledgeBase`, build-pass construction, incremental patch, WIRE.md annotation, golden capture, engine tests | `au-engine` | nothing |
| **C** W4.1 phase 1 | `tools/judgement/`: the labelled 50-note sample with three questions and thresholds, plus a `systemone`-shaped client behind an endpoint/key env var — no vendor call without a key | new `tools/` dir, docs | nothing |

**Why these three in parallel:** A and B are different crates with no shared file; C writes only a
new directory and touches no code that either A or B compiles. The one rule that still holds: T1's
**adapter** half (`au_search`, the 52nd tool) waits until B lands, because it needs the verb.

**Acceptance.**
- A: `lib.rs` production lines fall again; `MUTATION_STAGES`/`CREATE_STAGES` order unchanged; the
  31 gate tests untouched and green; a denial still names its stage; a mutation can be described
  as a `Prepared` value in a test.
- B: the eight probe queries answer ≤ 15 ms median on the live vault; a write changes the ranking
  of the changed note on the next read; `au_grep` unchanged; golden capture added; WIRE.md
  annotated; no schema-version bump.
- C: 50 notes labelled with the source they came from; the runner exits non-zero without a key
  rather than silently passing; the report names question, probability, threshold and verdict.

**Re-baselined estimate:** A ≈ 1 day, B ≈ 1–1.5 days, C ≈ 0.5 day, running together → about
1.5–2 days wall, then the adapter `au_search` (0.5 day) after B.

## 2d. Second revision (2026-11-16)

Three things changed since 2c, and each one changes what to do next.

1. **The ratchet gained one explicit exception.** Adding `au_search` cost 81 lines in `lib.rs`, so the
   budget moved 4 424 → 4 505 with the reason written into the constant. The rule now reads: the
   ratchet guards a refactor from re-growing the file; a feature pays its own lines once, and the
   number and its reason are recorded in the same edit.
2. **A cleanup lesson worth keeping.** Deleting `.cargo-target` while another `cargo` held the
   build lock made that build start restoring the directory; the target is shared by two workspaces
   and by other sessions on this host. Clean it only when idle. The "duplicate" crates in
   `deps/` (16 `mio`, 14 `syn`, 12 `au_mcp_rs`) are the **feature matrix** — `clippy
   --all-features` versus `test` versus release — not a misconfiguration, and not worth chasing.
3. **T1 is closed but unmeasured on the real thing.** The engine tests prove behaviour; the
   acceptance in 2b (≤ 15 ms median on the eight probe queries, ≥ 5× faster than `au_grep`) has
   never been run against the new verb on a vault-shaped corpus, because the live daemon still runs
   the previous engine binary.

**What is left, and what can run together:**

| track | work | surface | conflicts with |
|---|---|---|---|
| ~~**D** T1 acceptance measurement~~ | **done 2026-11-16** — both clauses met with margin; the table is in `docs/verification.md`. The live daemon still runs the previous engine binary, so promoting `search` to production is a separate decision and needs the golden contract, not a measurement | `/tmp` + fixture | — |
| **A** P2.2 | `Prepared` as a value: the armed mutation carries its stage and the recorded hash, so it can be described, logged and asserted in a test | `au-mcp-rs` (`gate.rs`, `lib.rs`) | nothing |
| **B** P3 | two procedures (`mutation-turn.sh`, `recovery-turn.sh`) expressed as states with code transitions and negative cases | `scripts/` | nothing |

**Order:** D first because it is the only one producing evidence about what just shipped; A and B
are code work that can follow in either order. W4.1 phase 2 stays blocked on a vendor key, and the
local-first default means it is not on the critical path.

## 2e. Third revision (2026-11-16, after acceptance)

T1 is measured and closed: `search` beats `grep` by 105×–698× on a vault copy, worst p95 3.47 ms,
identical answers across two daemon starts. P2's remaining two items were dropped by measurement
(no consumer adds truth; the audit log is what would justify them). What is left is small, and two of
the pieces are consequences of the last stretch rather than of the original plan.

| track | work | why now | surface | conflicts with |
|---|---|---|---|---|
| ~~**E** fixture hygiene~~ | **done `ea164f9`** — the trap was already there; what was missing is that a killed process group runs no trap. So `prepare_fixture` now reaps daemons whose fixture root no longer exists (proven: one real orphan, killed, `fixtures left: 0`), cleanup asks the engine to stop before signalling, and `AU_KEEP_FIXTURE=1` keeps a failing fixture | found 14 orphans | `scripts/lib/` | — |
| ~~**F** production parity~~ | **done 2026-11-16** — HEAD and the served binary are byte-identical on a frozen copy for 8/8 reads, so `search` is the whole delta and nothing must be regenerated for a swap | measured | `au-engine` + `/tmp` | — |
| ~~**G** P3 procedures~~ | **done `2f8d29f`** — `procedure.py` with two state machines, `--selftest` proving twelve rejection cases, and `mutation-turn.sh` cross-checking its own assertions against the machine. Follow-up: remove the duplicate assertions after one live run confirms they agree | done | `scripts/` | — |

**Order:** F in parallel with E; G after E, because both edit `scripts/`. Promotion of `search` to the
live daemon waits for F and for a window, and it is a decision, not a step.

**Two things that stay out of the trajectory unless asked:** the audit record gaining `repo:path@hash`
(that is the consumer `Prepared` would need, and it changes a hash-chained log format), and W4.1 phase
2 (blocked on a judge key; local-first is the default).

## 2f. Closeout (2026-11-16): the planned work is done; what remains are decisions

Every track in this plan is closed. The measured outcome, in one place, because the numbers are the
deliverable:

| what | before | after |
|---|---|---|
| `lib.rs` production lines | 4 636 | **4 524** (ratchet lowered five times, raised once for a feature with the reason recorded) |
| module dependency edges | one cycle (`wire ↔ lib`) | **no cycle**, 11 declared edges, a test that rejects an undeclared one |
| tools | 51 | **52** (+`au_search`), capability axis total and disjoint |
| adapter tests | 173 | **205** registered, 203 running |
| text search | literal scan, 132.8 ms | `search` 0.16–1.03 ms on a vault copy — **105×–698×**, worst p95 3.47 ms |
| refusals | a stage name | a stage name *and* one formatter in `policy.rs`, with the two codes kept distinct by test |
| duplicates judge | unmeasured | precision **1.000**, recall 0.889 |
| stale-description judge | unmeasured | accuracy 0.884 but recall **0.333** — not a detector yet |
| production parity | unproven | HEAD vs the served binary byte-identical on 8/8 reads of a frozen copy |
| fixture orphans | 14 left behind | a reaper proven to kill exactly the orphans |

Four things are **open decisions**, not tasks, and each names its price:

1. **Promote `search` to the live daemon** (fifth swap). Parity is proven and nothing must be
   regenerated; what is missing is a window, the install step, the live-socket read comparison and the
   rollback artifact. `docs/engine-swap-2026-11-16.md` rehearses it on a copy.
2. ~~**A better staleness question**~~ — **measured 2026-11-16, and the answer is no**
   (`docs/w4.1-judgement-run-2026-11-16.md`, second experiment). A five-way `choice` moved the
   trade-off without moving the frontier: yes/no is 0.67/0.33 precise/recall, the choice is
   0.36/0.50, and the union adds false positives while the intersection collapses to yes/no. Four of
   its five options went unused — every failure came back as `missing`, which is also what a false
   positive says. So P0-3 is not solved by the judgement layer as it stands; the product answer is a
   triage pass (flags 18 of 50, catches half the stale ones) and the next design is **per-claim**
   scoring, which needs a claim extractor — a generative model, not a judge.
3. **The audit record gaining `repo:path@hash`.** This is the consumer that would justify a
   `Prepared` value, and it changes a hash-chained log format — its own decision.
4. **W4.1 in the ingest path.** The duplicates judge is good enough to use; wiring it into
   `au-ingest` is a feature with a product consumer, and it belongs to that repo's plan.

Everything else on the improvement backlog (FTS5 date filters, ingest validation of broken links) is
engine or ingest work that this trajectory does not touch.

**The trajectory continues elsewhere.** This plan ends at the architecture; what comes next is the
detach-and-consolidate plan (`docs/detach-and-consolidate.md`), whose inventory
(`docs/repo-inventory.md`) assigns every directory a layer and a fate. The two agree on the rule that
makes both work: a component is judged by whether it is on the serving path, and the serving path is
Rust.

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

Pass 1 — disjoint files, one merge each, any order: **P1.2** (`bounded_stdio`, 1 site) — done,
**P1.3** (`resources.rs`) — done, **P1.5** (`ledger.rs` + 6 sites in the gate region).
Pass 2 — both rewrite `lib.rs` widely, so they are sequential: **P1.1** (`transport.rs` +
`wire.rs` + read sites), then **P1.4** (`schemas.rs` table + 51 attributes).

Revised after P1.2/P1.3: the two finished steps removed the `resources -> wire` edge and tightened
the `lib.rs` ratchet to 4 631 lines, so P1.5 must not spend the headroom it gained — the ledger
extraction is expected to *lower* the number, and the ratchet moves with it. **T1's engine half
runs in parallel with pass 1 and pass 2** because it touches `au-engine`, not this crate; its
adapter half is the last item, after P1.4.
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

### Wave 2b — text index (T1), the one measured product gap

`docs/retrieval-spike.md` measured it: `au_grep` is a literal byte scan at **132.8 ms**, and a
BM25 index over the same 5.4 MiB answers in the low tens of milliseconds while ranking. The
backlog already wants this (P1: bm25 with type-aware freshness). It is a product feature, not an
architecture step, so it is scheduled on its own terms and does not block the ports.

**Where it lives: the engine.** `grep` is an engine read; every client — this adapter, Hermes
through the bridge — goes through it. An index in the adapter would be a second source of truth
inside a single client. So the engine gains a new read verb `search` and the adapter gains one
read tool `au_search`. `au_grep` keeps its exact contract as the literal escape hatch.

**Ripple checklist (all mechanical, all must be in the same change):**

| item | change |
|---|---|
| engine verb | `Request::Search` beside `Request::Grep`; `wire.rs` catalogue; describe entry |
| WIRE.md | annotate the new verb; `SCHEMA_VERSION` does not move (additive) |
| engine index | in-memory per repo, built during the existing build and patched on the incremental path; rebuilt from files on start, never persisted |
| ranking | BM25 over `body`, with `description` weighted; type-aware freshness multiplier as a **second** step, measured separately |
| adapter | one tool `au_search` (capability `read`), schema with declared `score`, fixtures |
| counts | 51 → 52 tools, 35 → 36 advertised without grants; `tests/architecture.rs`, `tests/capability.rs`, the capability table, `--info`, `docs/verification.md`, roadmap |
| golden | a capture for `search` in `au-engine/tests-golden/engine/`, and the adapter's read fixture |
| tests | an engine test that a mutation changes the ranking; an adapter test that the declared schema holds a float score |

**Prototype numbers (Python, `/tmp/bm25-probe`, 2026-11-15):** build 1.7 s; index 3.4–3.8 MiB
for 5.4 MiB of text (+12 MiB RSS); 45 708 terms, 282 886 postings; query latency **0.003–0.56 ms**
against 110–135 ms for the literal scan (198×–43 000×); `description` weighted ×3 costs +18%
postings for a clearly better top-5 on two of three spot checks.

**What the prototype says the design must include, beyond plain BM25:**

1. **AND or a minimum-should-match.** Pure OR returned 335–343 documents for a two- or
   three-term query, which makes the top-5 close to arbitrary.
2. **A phrase path for punctuation.** BM25 ranked `error->fix` over 118 documents and put the
   wrong one first; the literal scan found exactly the one document that contains the token.
   Both are needed: ranked for recall, literal for exactness.
3. **Per-field weight, not token repetition.** `description` deserves its own field.
4. **Path hygiene.** `templates/` (6 files) and `.claude/` (16) polluted results; 547 of 1 037
   files live in `inbox/`, and hub notes (`knowledge/index.md`) rank on link text alone.

**Acceptance:** median ≤ 15 ms for the eight probe queries on the live vault; ≥ 5× faster than
`au_grep` on the same query; top-5 stable across runs; no existing verb changes shape; the
literal escape hatch unchanged; and — now that the prototype exists — a labelled check on three
queries where the ranked top-3 must contain the document the literal scan finds for the same
token.

**Order:** the engine half can start in parallel with P1.5/P1.1 (different crate, no shared
files). The adapter half waits for P1.4, so the new tool is declared through the schema table
instead of adding one more hand-written function to it.

### T1 phase 2 — where the index lives in the engine (built 2026-11-15)

Reading the engine to place it, rather than guessing:

- `FileParse::Instance { body: String, .. }` and the `Note` variant hold the note text in the
  **catalog** (`ir.rs:415`, `FileEntry.parse`), so the index is built from held state and a
  `search` read needs no disk IO — unlike `grep`, which reads bytes in `spawn_blocking`.
- Placement: an `Arc<SearchIndex>` field on `KnowledgeBase` beside `backlinks`, built in the same
  pass that fills `instances`/`backlinks`, with `doc_id` assigned from the catalog's path order so
  a build is deterministic.
- Incremental: the patch point is `incremental.rs:1280-1352`, where catalog/instances/backlinks/
  diagnostics are patched by delta — the same delta drives `remove(changed)` + `add(changed)`. A
  type-def or manifest change already forces a full rebuild, which rebuilds the index by
  construction.
- Verb: `Request::Search(SearchArgs)`, answered through `handle.read(|kb| …)` (no blocking task),
  response `{"search": {matches: [{path, score, matched_terms}], total, offset, next_offset}}`.
  `min_should_match` defaults to the number of distinct query terms for a 1–2 term query and to 2
  above that, so a three-term query is not an OR that returns a third of the vault; `exact: true`
  routes to the conjunction path instead.
- Fields and weights at build time: `[body, description]` with `[1.0, 3.0]`. Path hygiene is a
  query-side filter (a `path_prefix` argument), not baked into the index, so it stays inspectable.

**Acceptance for phase 2:** the eight probe queries from `docs/retrieval-spike.md` answer in ≤ 15 ms
median on the live vault (measured p95 is 2.45 ms on 1 000 synthetic documents); a mutation changes
the ranking of the changed note in the next read; `au_grep` unchanged; a golden capture added for
`search`; WIRE.md annotated; no schema-version bump (additive verb).

### Wave 3 in detail — P2, the gate becomes legible data

The gate already is a state machine (`Stage` 8 variants, `MUTATION_STAGES` 7, `CREATE_STAGES` 6,
denials that name their stage, a ledger of prior reads). What it lacks is what the assessment
called out: the state is not a value, the transitions are not queryable, and a caller learns the
rule by being refused. Concretely:

**P2.1 reduction apart from dispatch.** `fn reduce(stage: Stage, ctx: &GateInput) -> Step` is a
pure function; `Step` is `Denied(Denial)` or `Continue(Progress)`, and `Progress` accumulates what
later stages read (`recorded` hash, resolved target, armed file). `arm_file`, `finish_mutation`
and the engine call stay where they are: they are dispatch, and a reduction that performs IO
cannot be tested without a socket.

**P2.2 the armed mutation is a value — measured out, 2026-11-16.** The plan wanted
`Prepared { stage, repo, path, expected_hash, recorded }` so a mutation in flight could be
"described, logged and resumed as data". Reading the consumers before building it: the only place a
description could go is the `outcome_unknown` message, and there it would mislead — `au_reconcile`
wants the digest the write *should have produced*, not the pre-write hash the gate holds, so naming
the held hash next to "reconcile" would invite exactly the wrong call. The path and repo are already
the caller's own arguments. So the value would be a struct nothing reads, which is the same finding
as P2.3.

What would make it pay, recorded here rather than built: the **audit log** records `applied`,
`rejected` and `outcome_unknown` at member granularity (`audit.record(principal, "mutation", repo,
verdict)` in `lib.rs`), so an operator cannot reconstruct which file and which hash were in flight.
Extending the record to carry `repo:path@hash` is the consumer that would justify `Prepared` — and it
changes a hash-chained log format, so it needs its own decision, not a refactor commit.

The original wording is kept below for the record.

**P2.2 the armed mutation is a value.** `Prepared { stage, repo, path, expected_hash, recorded }`
so a mutation in flight can be logged, described and resumed as data instead of living in locals.
`Denial` keeps its stage and gains `next: Vec<Next>`.

**P2.3 a refusal says what unblocks it — measured out, 2026-11-15.** Built as a per-stage
`remedy()` table appended to the message; `tests/faults.rs` pins the refusal text and caught it, and
reading that text showed why: the messages **already** name the step that clears them. The table
duplicated them and was reverted. What is actually missing is a machine-readable field, which means
structured error content — a separate change with its own client-compatibility question. What did
land from P2's first slice: `Denial` moved beside `Stage` in `policy.rs`, one `text()` formatter,
`lib.rs` 4561 → 4521, and a test pinning `policy/<stage>: ` as the code half.

The original wording is kept below for the record.

**P2.3 a refusal says what unblocks it.** `Next { stage: &'static str, tool: Option<&'static str> }`
— for example `{"stage":"prior_read","tool":"au_content"}` or
`{"stage":"freshness","tool":"au_content"}`. An agent that was refused at `editability` learns it
must satisfy `prior_read` first, instead of trying mutations until one lands. Additive wire field;
annotate `WIRE.md`; no existing refusal text changes.

**Acceptance:** the 31 tests that pin the gate order and the two refusal codes stay unchanged
(they are the contract); a property test asserts the stage table is total and that every denial
carries a non-empty `next`; one live client run (`scripts/mutation-turn.sh`) shows the field; the
denial payload is the only shape that changes.

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

## 5b. Wave 4 — the judgement layer and the agent roles

`docs/agents-and-decisions-assessment.md` is the assessment; this is its place in the plan.

| track | deliverable | why here |
|---|---|---|
| **W4.1 ingest auditor** | a `systemone`-shaped client (local `jev` first, vendor only as an opt-in comparison) running three questions over the vault: stale description (`noul`), type (`choice`), duplicate (`noul` over candidate pairs from the T1 ranker) | offline, batch, bounded; closes backlog P0-3 and feeds P1's enrichment without touching the serving path |
| **W4.2 role definitions** | the four roles (librarian, editor, auditor, scout) written as `agent-profile` instances with tool whitelists and code-checkable exits | P3's procedures name a role instead of a prompt |

**Acceptance for W4.1:** precision ≥ 0.9 on a hand-labelled 50-note sample at the chosen
threshold; a full pass over the live vault leaves every file unmodified; ≤ $0.10 per pass if the
vendor is used; the report names the notes, the question, the probability and the threshold.

**Hard boundary:** no judge call in the read path (it cannot meet T1's ≤ 15 ms), no model in the
mutation gate (a refusal must be reproducible), and no new dependency for vector search until T1
has landed and hybrid retrieval is shown to be wanted. The egress decision (A/B/C in the
assessment's §4) is the user's, and the default is local-first.

**Order:** W4.1 starts when T1's engine module lands (it needs the same tokenisation for candidate
pairs); W4.2 is documentation and runs in parallel with anything.

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
| P1.1 transport port | **done** (`4b66625`) | `transport.rs` 241 lines with `UnixTransport` + `MemoryTransport`; the `wire ↔ lib` cycle is gone (no `crate::SCHEMA_VERSION` in `wire.rs`); ALLOWED is 9 edges; `lib.rs` 4612 → 4561 and the ratchet tightened; `--info` unchanged; 2 new tests (one exchange, no retry) | 2026-11-15 |
| P1.2 bounded_stdio | **done** (`328baa3`) | `Bounds` parameter + `over(input, output, …)`; two tests on a 16-byte duplex prove the queue blocks a third send and releases it on drain, and that a closed transport refuses; suite 190 passed | 2026-11-15 |
| P1.3 resources | **done** (`a4228c0`) | `member_roots(payload)` and the new `resource_page` are pure; the `resources -> wire` edge is gone (ALLOWED 7 → 6); `lib.rs` 4659 → 4631 production lines and the ratchet tightened to 4631; 4 inline tests without a socket | 2026-11-15 |
| P1.4 schemas table | **dropped as written** (2026-11-15) | the 15 functions carry distinct shapes; the invariant that matters is already a test (P0.2d). T1's new tool is the only place this had to be paid for, and it is paid there | 2026-11-15 |
| P1.5 read ledger | **done** (`3e402e8`) | `ledger::ReadLedger` with 3 tests (forget, member in the key, clone shares); `lib.rs` 4631 → 4612 and the ratchet tightened; the two refusal codes unchanged | 2026-11-15 |
| T1 text index (engine half, phase 2) | **done** (`d888abe`) | index on `KnowledgeBase` with a `search` parity fact; `Request::Search` from held state; 6 integration tests including "a body rewrite changes the ranking on the next read"; golden capture from the fixture | 2026-11-15 |
| T1 text index (adapter half) | **done** (`c3472e5`) | `au_search` is the 52nd tool; capability 33 read / 16 mutate / 2 discover / 1 watch; counts in architecture, capability, stdio, fixture_schema and wire_catalog all moved together; suite 203 passed | 2026-11-15 |
| T1 acceptance | **measured 2026-11-16** | release engine from `fec77bd` over a vault copy: `search` 0.16–1.03 ms vs `grep` 101–112 ms (105×–698×), worst p95 3.47 ms, identical top-5 across two daemon starts; `exact` 0.26 ms; `bm25 ranking` answerable only by the ranked verb | 2026-11-16 |
| T1 text index (engine half) | **phase 1 done** (`3d4bc40`) | `search.rs` 982 lines; 7 tests + ignored measurement; build 1.07 s / 1 000 docs, 4.7 KiB/doc, OR p50 1.65 ms p95 2.45 ms, exact p50 73 µs; engine `--lib` 685 passed / 1 known root-only failure; **no verb yet** | 2026-11-15 |

| P2.1–P2.3 gate | not started | — | — |
| W4.1 judge run | **measured 2026-11-16** (`docs/w4.1-judgement-run-2026-11-16.md`) | 50 notes, $0.0019/pass: duplicates 1.000/0.889 (adopt), stale recall 0.333 (not usable as a detector), type 0.729 with adjacent confusions; two harness bugs found by running it (vendor wire, inverted polarity) | 2026-11-16 |
| W4.1 ingest auditor | **phase 1 done** (`6b8f0f0`) | `tools/judgement/`: 50-note stratified sample with hand labels (10 stale / 40 not), a `systemone`-shaped client with `--dry-run` (exit 0, ~$0.0018 per pass) and exit 3 without a key; real path proven against a local fake; vendor response shape unverified (no key) | 2026-11-15 |
| W4.2 role definitions | not started | assessment §1.1 | — |
| P3.1–P3.2 agent layer | **P3.1 done** (`2f8d29f`) | `procedure.py` + `--selftest` (12 cases, all bite) + cross-check wired into `mutation-turn.sh`; P3.2 (the vault pattern note) not written | 2026-11-16 |
| P4.1 generated manual | not started | — | — |
| R1 retrieval probe | **measured 2026-11-15** — not adopted | `docs/retrieval-spike.md`: HelixDB 0.0.16 on 1 036 nodes / 2 730 edges — hubs 103.5 ms vs 9.8 ms, 2-hop count 54.7 ms vs `au_neighborhood` 102.2 ms for 253 materialised nodes, BM25 24.2 ms vs `au_grep` 132.8 ms, variable-length patterns unsupported; decision: tantivy plus our own traversal | 2026-11-15 |
