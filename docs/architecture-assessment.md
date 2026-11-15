# Architecture assessment: hexagonal layers, FSM control, and an ontology for the tool surface

> **Historical measurement, 2026-11-15.** This assessed the tree as it was *before* the
> TypeScript and Python layers were deleted (2026-11-16). The findings stand as evidence for the
> decisions taken; the file counts and module lists do not describe the current tree — see
> `docs/repo-inventory.md` for that.


Status: assessment, 2026-11-15. **No code was changed to produce it.** Every number below was
measured on `main` by reading the tree; every external claim is attributed in §8; every vault
claim cites a note that was read through our own MCP server. The test baseline was re-measured
on this checkout (`cargo +1.99.0 test --offline`: 39 suites, 173 passed, 2 ignored, `rc=0`).

## 1. The question, split into four

The proposal mixes four different things that fail differently. Separating them is most of
the answer:

| # | Direction | What it would change | Where it lives |
|---|---|---|---|
| A | **Thin hexagonal** — invert the seams that already exist | module borders inside `au-mcp-rs` | code |
| B | **Full onion** — domain crate + ports crate + adapters | crate graph | code + build |
| C | **FSM control layer** — C1 in the runtime, C2 in the agent/prompt layer | mutation gate / skill procedures | code (C1), process artefacts (C2) |
| D | **Ontology of the tool surface** — typed capability model | schemas, discovery, docs, tests | contract |

A and B are alternatives, not stages. C and D are orthogonal to both, and D partly exists
already — see §3.2.

Trajectory and parallel tracks for the recommendation in §6: `docs/architecture-plan.md`.

## 2. What the code is, measured

### 2.1 Adapter `au-mcp-rs` (6 917 src lines, 8 878 test lines in 36 files)

| file | lines | tests in file | production |
|---|---:|---:|---:|
| `src/lib.rs` | 5 182 | 546 | 4 636 |
| `src/schemas.rs` | 483 | 0 | 483 |
| `src/policy.rs` | 431 | 106 | 325 |
| `src/wire.rs` | 300 | 0 | 300 |
| `src/bounded_stdio.rs` | 218 | 0 | 218 |
| `src/bin/arsumbris.rs` | 195 | 0 | 195 |
| `src/resources.rs` | 102 | 0 | 102 |

`lib.rs` holds **51 `#[tool(` handlers in 2 064 lines** (601–2664) plus seven more
responsibilities:

| range | responsibility | lines |
|---|---|---:|
| 1–56 | module glue, `SCHEMA_VERSION`, protocol versions | 56 |
| 58–560, 3523–3900 | argument DTOs (56 struct/enum) and their schemas | 881 |
| 601–2664 | 51 handlers, descriptions, inline wire requests | 2 064 |
| 2666–3196 | the mutation gate: `guarded_mutation`, `arm_file`, `finish_mutation`, `prepare_create`, `prepare_mutation`, `workspace_mutation`, `composition_mutation` | 531 |
| 3197–3248 | read wrapper, timeout, cancellation | 52 |
| 3266–3522 | `impl ServerHandler` (initialize, call_tool, list_tools, resources) | 257 |
| 3901–4140 | tool catalogue: `search_tools`, `tokenize`, `page_array`, `served_hash` | 240 |
| 4110–4470 | payload shaping: `success_payload`, `strip_descriptions`, `window_content`, paging | 360 |
| 4470–4636 | CLI parsing and `run` | 166 |

Dependency edges: `lib → bounded_stdio | policy | wire | resources | schemas`, plus a
**cycle**: `wire.rs:15` reads `SCHEMA_VERSION` from the crate root while the root glob
re-exports `wire::*` (`lib.rs:8`), and `lib → resources → wire → lib` closes through the
root.

### 2.2 Where the policy lives

`policy.rs` (325 production lines) declares `MUTATION_TOOLS` (16), `COMPOSITION_TOOLS` (2),
`WORKSPACE_TOOLS` (3), `Policy`, `Stage` (8 variants), `MUTATION_STAGES` (7),
`CREATE_STAGES` (6), `Audit`, `digest`. **Declarations are out; enforcement is not:**
`Stage` has 11 call sites in `lib.rs` (2732–2930, 3887–3892), `expected_hash` appears at
49 sites, `self.served` (the read-before-write ledger) at 6.

### 2.3 Engine `au-engine/crates`

| crate | src lines | prod | tests in src | test files | pub fn / types |
|---|---:|---:|---:|---:|---|
| `au-engine` | 61 613 | 46 134 (75%) | 15 479 | 79 | 388 / 198 |
| `au-core` | 40 015 | 22 001 (55%) | 18 014 | 0 | 166 / 77 |
| `au-grammar` | 4 968 | 2 376 | 2 592 | 0 | 19 / 15 |
| `au-parser` | 4 035 | 2 073 | 1 962 | 0 | 25 / 14 |
| `au-references` | 2 251 | 1 166 | 1 085 | 0 | 20 / 7 |
| `au-testkit` | 1 520 | 1 350 | 170 | 14 | 30 / 9 |
| `au-cli` | 1 203 | 1 001 | 202 | 5 | 6 / 0 |
| `au-diagnostics` | 407 | 276 | 131 | 0 | 10 / 10 |

The split at crate level is already hexagonal in spirit: `au-diagnostics → ∅`, then
`grammar/parser/references` (11.6k lines of pure functions), then `au-core` (40k of
analysis), then `au-engine` (61k of state and IO), then `au-cli` (1.2k of rendering).
The problem is **inside** `au-engine`: repositories, build, incremental, git writer, package
cache, daemon, wire, mutation sagas and client in one crate —
`serve.rs 16 772`, `wire.rs 5 759`, `build.rs 4 654`, `repo.rs 4 519`,
`gitwriter.rs 3 279`, `resolution_build.rs 3 180`, `pkgcache.rs 3 148`, `incremental.rs 3 011`.
`au-core/src/validate.rs` is 15 544 lines.

### 2.4 Seams that already exist, ranked by inversion cost

| # | seam | sites outside it | inversion cost |
|---|---|---:|---|
| 1 | `bounded_stdio` (218) | 1 (`lib.rs:4629`) | trivial: parameter to `run()` |
| 2 | dependency on `au-cli` (`bin/arsumbris.rs:125–130`) | 6 | trait over daemon commands |
| 3 | frame codec (`wire.rs:51–105`) | 0 | `trait Transport` + 2 impls |
| 4 | resource URI and member table (`resources.rs:42–102`) | 1 | pure functions + injected table |
| 5 | tool schemas (`schemas.rs`) | 51 attributes | one `schemas::for_tool(verb)` table |
| 6 | policy declarations | 12 | `trait MutationPolicy`; `Stage` leaks |
| 7 | payload shaping | 10 + 4 + 5 | pure functions + `PayloadMode` |
| 8 | read-before-write ledger | 6 | extract `ReadLedger` |
| 9 | read timeout and cancellation | 22 | follows from (3) |
| 10 | mutation stage machine | 6 + 5 + 4 + 3 + 2 | `ArmedFile`/`PreparedMutation`/`Denial` + gate trait |
| 11 | handler router (51 `#[tool]`) | 51 | handler table instead of macro — expensive |
| 12 | build pipeline | `crate::build::` from 15 modules | already behind `EngineHandle::read(closure)` (68 sites) |
| 13 | engine wire vocabulary | 47 of 82 verbs | enum is data; contract is a port |

## 3. What our own vault already says (dogfood)

All of this was retrieved through `arsumbris-rs`, read-only, in this session.

### 3.1 The FSM idea is already in the vault, twice, from production systems

- `inbox/meta-thin-orchestrator.md` (extract, current): Meta's orchestrator is 1 300 lines
  routing 20+ workflows to 14 agents; **workflow templates are deterministic state
  machines**; LLM reasoning happens only at the delegation step, inside an isolated
  subagent; knowledge files load at runtime.
- `inbox/allspark-reactivegraph-per-file-fsm-and-completeness-envelope.md` (insight):
  "deterministic FSM orchestrates, WASM executes, LLM is a capability, never the
  orchestrator"; hexagonal H1–H4 *inside* layers L2–L5; every read returns
  `Completeness{Complete|Partial|Stale}` plus `next_step_hint()`; results are
  `VerifiedToolResult` with readback.
- `inbox/octopus-kernel-patterns-deepdive.md` (pattern): state in types
  (`PreparedVerification → ReadyVerification`), "nothing emits marks" — only the runtime may
  declare a step done.
- `inbox/switch-decision-model-harness-framework.md` (extract): Domain/State/Question/Pick/
  Transition as a state machine; the code enumerates the options, the model only picks.
- `inbox/adaptive-skill-granularity-runtime-routing-recovery.md` (extract): recovery is an
  explicit FSM transition, not a retry; a skill contract is
  `(name, args, pre, effect, V, F, fallback)`.
- `knowledge/tla-plus-prototype-only-for-state-machines-and-concurrency.md` and
  `knowledge/model-checking-as-ci-gate.md`: TLA+ only for state machines and concurrency,
  as a CI gate.

### 3.2 The ontology idea is already partly built — as types, not as prose

`au-mcp-sdk/type/` contains `mcp.tool.type.yaml`, `mcp.inject.type.yaml`,
`agent-profile.type.yaml`, `tool-access-meta.type.yaml`, `required-scope-meta.type.yaml`;
`au-mcp-core/type/` contains one `mcp.tool.<verb>.type.yaml` per mutation. The model is:
a tool's **fields are its input**; orchestration data goes into a typed `meta:`; discovery
runs through `subtypes('mcp.tool')`; delivery has three classes — tool (on call), skill (on
match), inject (always on, hard character budget).

So "an ontology for the tool surface" is not a greenfield project. What is missing is the
*capability axis* (§5 D) and a consumer that uses it — the pieces above are documentation
instances, and the code does not read them.

### 3.3 Two contradictions and one gap in the knowledge base

- **Contradiction:** the ReactiveGraph note deliberately does **not** adopt that system's
  `RunFSM` ("our lifecycles are modelled"), while the later BPMN note
  (`inbox/bpmn-agentic-workflows-semtalk.md`, 11.11) says a harness without an explicit
  orchestration model is the defect. Both are `current`. The style question is genuinely
  open in our own knowledge base.
- **Contradiction:** no doctrine of hexagonal/onion/clean exists in the vault — only
  extracts that describe ports-and-adapters as *code discipline* (flat port traits, serde only
  at two boundary crates, `deny_unknown_fields` ×29, verification at admission/promotion/
  conformance) in `inbox/law-nexus-serialization-traits-applicability.md`. Whoever writes
  the first style note sets the canon.
- **Gap:** saga/compensation is not covered at all. In our type definitions the word means
  "one atomic multi-file mutation with a single Mutation-Id" — atomicity, not compensation.
  The engine's `batch` handles that; there is no vault note about rollback as a state machine.

### 3.4 Dogfood findings (evidence that the surface needs a naming/capability discipline)

1. **`au_instances_of` fails on the last page.** `next_offset` is `null` there while the
   schema declares `integer` → a real client rejects the answer
   (`None is not of type 'integer'`). Reproduced with `{type_name:"moc", limit:200}` and
   `{type_name:"manual", limit:100}`; `limit:10` is fine. Same nullable-union class as the
   `hash`/`commits` bugs already fixed in the adapter, but on the engine side.
2. **Argument names are not guessable:** `type_name`, not `type`; `origin_path`, not
   `origin`. Errors do list accepted fields, but the manual note (`manual/mcp-access.md`)
   still documents 44 tools, the `mcp__arsumbris__` prefix, the `au-mcp` socket and
   `active/outdated/superseded` statuses — a stale instruction file of exactly the kind the
   vault's own rule about instruction drift describes.
3. **Path shape is inconsistent by verb:** absolute paths in `au_hubs`/`au_diagnostics`,
   member-relative in `au_content`/`au_frontmatter`.

## 4. What external practice says

Condensed to the claims that decide something here; attribution in §8.

1. **Hexagonal is a discipline about *who knows whom*, not about the number of layers.**
   The canonical Rust write-ups invert ports, keep port types next to the port, and test per
   ring. The canonical critique (Rentea) is blunt: applying it "by the book" is
   overengineering — the blueprint must be simplified to the problem, and a wrong split
   impedes the next refactor on a different axis.
2. **In Rust the state-machine choice is concrete:** a runtime transition table (data,
   queryable, `possible_transitions()`, serializable) versus typestate (invalid transitions
   are compile errors, but the state stops being a value — cannot be returned as "whichever
   state", cannot be deserialized without a runtime check). Use the table for anything that
   crosses a boundary or needs explaining to an agent; typestate for the short, closed,
   in-process phases. Nested enums model hierarchy.
3. **State-per-prompt works when the process is known** (Zylos; Meta's thin orchestrator):
   a state owns its prompt/instruction set, the FSM owns sequence/branching/recovery, and the
   model is free *inside* a state. It fails when states are invented ahead of evidence, and
   it is explicitly advised **not** to call the model on state entry — that couples the
   process to model latency and failure.
4. **Effects must be separated from the reduction** (Manolachi): the dispatcher performs
   effects, so the graph is testable without the model or the API.
5. **Durable execution is the industrial version of the same idea:** checkpoint each
   non-idempotent step, replay on resume, get exactly-once for the parts that are not
   naturally idempotent (Temporal/Restate/Hatchet comparison, 2026). Our
   `outcome_unknown → au_reconcile` is a hand-built instance of this: the write is
   checkpointed by the engine's git history, and reconciliation reads it back instead of
   replaying.
6. **Ontologies only pay when they are compiled into something executable:** with tool
   interfaces (arXiv 2602.03439), with a typed kernel for reasoning (2608.22974), or with a
   self-evolving semantic layer (2609.15779). A prose ontology with no consumer is
   documentation with extra steps.
7. **Tool descriptions are the agent's only user manual** (MCP 2026 playbook): when to call,
   argument format, what to do on failure. Active discovery beats loading the whole catalogue
   (MCP-Zero).
8. **Prompts are code** only when they are versioned and evaluated on every change
   (prompt-versioning practice, 2025–2026). Our scripts in `scripts/*-turn.sh` are already
   evals of exactly this kind.
9. **Dependency rules must be executable tests** (ArchUnit-style; `ArchUnitRust`,
   `architecture-fitness` crates), otherwise they decay.

## 5. Evaluation

### A. Thin hexagonal — invert the five cheap seams, keep `lib.rs` as assembly

What: `Transport` (frame codec), `bounded_stdio` as a parameter, `resources` as pure
functions over an injected member table, `schemas` as one `for_tool(verb)` table,
`ReadLedger` extracted from `self.served`. All five have ≤6 call sites today (§2.4 rows
1–5) and none changes the wire shapes.

Cost: roughly 400–600 moved lines, a day or two, no behavioural change, and the 173-test
suite plus the golden fixtures decide whether it was safe.

Win: `lib.rs` becomes a router + payload shaper (~3 000 lines) around explicit modules, the
`wire ↔ lib` cycle disappears, and the mutation gate is then the *only* remaining monolith —
so the next decision becomes small.

Fails when: the seams are extracted but nothing constrains them from re-growing. The
constraint must be executable (a dependency test plus a line budget), otherwise this is a
one-off rearrangement.

### B. Full onion — domain crate, ports crate, adapter crates

What: `arsumbris-domain` (stages, policy reduction, payload rules, no IO),
`arsumbris-ports` (traits), `au-mcp-rs` as one adapter, engine client as another.

Cost: the 51 handlers, the 531-line gate and the payload layer all move or get duplicated;
crate boundaries in Rust also mean compile-time plumbing (`dyn` at the boundary, per the
Google Rust guide, is a vtable — acceptable at a boundary, wrong in a hot loop). Weeks, not
days. Risk of the exact failure Rentea describes: a split along the wrong axis, made
expensive enough that nobody dares redo it.

Justified when: a second adapter is actually being written (a second transport, a second
consumer with a different policy) **and** the seams of A are already clean. Right now there
is one adapter and one transport, which is the textbook condition for "not yet".

### C. FSM control layer — do not conflate C1 and C2

**C1, in the runtime.** The mutation gate already *is* a state machine: `Stage` with 8
variants, `MUTATION_STAGES` (7) and `CREATE_STAGES` (6), a denial that names its stage, and a
ledger of prior reads. What it lacks is not FSM-ness but three properties the literature
names:

- **the transition table is data but the effects are inline** — `arm_file`, `finish_mutation`
  and `prepare_*` perform IO while deciding; separating reduction from dispatch makes the
  gate testable without a socket (it already is testable, at the cost of 546 lines of tests
  living inside `lib.rs`);
- **no `possible_transitions()`** — an agent cannot ask "what may I do from here", so it
  learns the rule by being denied. ReactiveGraph's `next_step_hint()` and Zylos' explicit
  phases are the same fix from two directions, and our vault already records the pattern;
- **the state is not a value** — `ArmedFile`/`PreparedMutation` are local variables, so a
  mutation cannot be described, logged, or resumed as data.

Proposal: make the stage machine a table + a `Prepared`/`Armed` value with the current stage
inside it, expose transitions in the denial payload (`allowed_next: [...]`), and keep the
existing tests as the contract. This is a bounded refactor of ~531 lines, and it is exactly
what makes C1 pay: the *agent* half of the story needs the machine to be legible.

**C2, in the agent/prompt layer.** The "meta-prompt as FSM" idea is sound for **known,
repeated procedures** and unsound as a general control layer:

- Sound: our own `scripts/*-turn.sh` are already state-per-prompt procedures with
  deterministic checks (read → hash → write → one commit; read → write once → reconcile).
  Turning a repeated one into an explicit skill graph — states with entry/exit checks,
  transitions that are code, the model free inside a state — is a direct application of
  `inbox/graphskilla-a-skill-graph-you-can-locally-edit-test-and-roll-back.md` and
  `knowledge/dict-first-agent-interface-pattern.md`.
- Unsound: FSM-ifying *open* work (design, debugging, research). There the states are
  discovered, not declared; a declared graph becomes a checklist that hides the case it did
  not anticipate. Our own session today found two schema bugs by dogfooding — no pre-declared
  state list contains "the read answer is rejected by the client".

Rule that keeps it honest: **the FSM owns the process, the model owns the reasoning inside a
state, and only the runtime may declare a state complete** (Octopus' "nothing emits marks";
our `finish_mutation` already obeys this). A prompt that says "you are now in state 3" with
no code behind it is prose, not a state machine.

### D. Ontology of the tool surface — small, and it has a real consumer

Already present: typed `mcp.tool` / `mcp.inject` / `agent-profile` definitions (§3.2),
`au_search_tools` + `au_describe_tool` as discovery, `output_schema` on all 51 tools, and
three tests that check declared types against actual payloads.

Missing, and cheap: **one capability axis** — every tool is exactly one of
`read | mutate | discover | watch | serve | compose`, declared in one place in the adapter and
checked by a test. What it buys, concretely:

1. `au_search_tools` can filter by capability, so a read-only session never sees a mutate
   name (today that is achieved by *absence*, which is correct but unexplainable);
2. documentation and the `au_describe_tool` answer can be generated from the same table
   instead of drifting (the `manual/mcp-access.md` drift in §3.4 is the current cost of
   drift);
3. a schema test can assert the invariant "a capability class is total and disjoint", which
   is the kind of property that catches a new tool added without a decision.

Explicitly **not**: inference. The vault's own comparison
(`inbox/open-ontologies-vs-arsumbris.md`) records that arsumbris validates structure and does
not reason; a capability ontology that tried to *derive* which tool to call would be a new
system with no consumer. Derive *descriptions and filters*, not *decisions*.

## 6. Recommendation

Ordered by evidence-per-cost, with a stop rule at each step:

1. **Now, cheap (half a day): executable architecture tests.**
   `tests/architecture.rs`: no new module cycles (the `wire ↔ lib` one may be grandfathered
   with an explicit exception list), `lib.rs` line budget as a ratchet, and a capability
   classification for all 51 tools (direction D). This is the fitness-function layer the
   literature says must exist *before* restructuring, and it costs nothing to start.
2. **Then, one to two days (direction A):** invert rows 1–5 of §2.4. Success criteria are
   mechanical: `cargo test` unchanged at 173 passing (baseline re-measured 2026-11-15:
   39 suites, 173 passed, 0 failed, 2 ignored, `rc=0`), golden fixtures byte-identical,
   `lib.rs` under ~3 200 lines, no cycle outside the exception list.
3. **Then, decide on C1 with data:** if the gate is still the biggest block, split
   reduction (pure: stage → verdict) from dispatch (IO), and add `allowed_next` to denials.
   Do not build a general FSM engine; the table + a `Prepared` value is the whole design.
4. **In parallel, direction C2 only for repeated procedures:** pick the two scripts that are
   already deterministic (`mutation-turn.sh`, `recovery-turn.sh`) and express each as
   states with code transitions; keep them as shell/python until a second consumer exists.
5. **Do not do B** while there is one adapter and one transport. Revisit when a second
   adapter is actually scheduled; the seams from step 2 make that rewrite cheap later, which
   is the entire point of doing A first.
6. **Fix the dogfood bugs**, because they are cheap and they are the honest measure of D:
   `au_instances_of` `next_offset: null`, argument names (`type_name`, `origin_path`), and
   the absolute/member-relative path inconsistency.

### What would make me reverse a recommendation

- A second transport (in-memory or TCP) scheduled this month → B's `ports` crate becomes
  justified early, but only the `Transport` port, not the whole onion.
- `lib.rs` shrinking without the seams being extracted → the line budget is measuring the
  wrong thing; replace it with a per-responsibility import test.
- A second agent client whose policy differs (a read-only reviewer, a writer) → the
  `MutationPolicy` port (row 6) moves from "medium" to "now", because the policy leak stops
  being theoretical.

## 7. Risks and non-goals

- **Risk: architecture theatre.** Four layers of indirection for a 6.9k-line adapter is a net
  loss; the measurement in §2.4 exists to keep the choice honest.
- **Risk: FSM theatre.** A prompt-level state machine with no code transitions is a
  checklist; it will be believed *because* it looks formal. The rule in §5 C2 is the guard.
- **Risk: ontology theatre.** Typed descriptions that no code reads decay exactly like the
  manual note in §3.4. Every ontology artefact must name its consumer.
- **Non-goals:** rewriting the engine's crate graph; adding inference; making the engine's
  wire vocabulary a Rust trait hierarchy; retiring the Python bridge (that is `docs/phase-d.md`
  and a separate decision).

## 8. Sources

Vault (read through `arsumbris-rs`, all `current` unless noted):
`inbox/meta-thin-orchestrator.md`, `inbox/allspark-reactivegraph-per-file-fsm-and-completeness-envelope.md`,
`inbox/octopus-kernel-patterns-deepdive.md`, `inbox/switch-decision-model-harness-framework.md`,
`inbox/adaptive-skill-granularity-runtime-routing-recovery.md`, `inbox/bpmn-agentic-workflows-semtalk.md`,
`inbox/paperqa2-vs-daily-archive-comparison.md`, `inbox/law-nexus-serialization-traits-applicability.md`,
`inbox/google-rust-style-guide.md`, `inbox/open-ontologies-vs-arsumbris.md`,
`inbox/neo4j-ontology-graph-specification-open-ai-first.md`,
`f/patterns/pattern-semantic-context-beside-schema.md`,
`f/patterns/pattern-immutable-published-schema-current-alias.md`,
`f/patterns/pattern-uniform-capability-boundary.md`, `f/patterns/pattern-describe-dont-render.md`,
`f/patterns/pattern-failure-layer-routing.md`, `f/patterns/pattern-rules-earn-place.md`,
`knowledge/tla-plus-prototype-only-for-state-machines-and-concurrency.md`,
`knowledge/model-checking-as-ci-gate.md`, `knowledge/dict-first-agent-interface-pattern.md`,
`knowledge/llm-compiler-not-executor-principle.md`,
`knowledge/mcp-tools-as-typescript-api-outperforms-direct-tool-calling.md`,
`knowledge/tool-use.md`, `knowledge/index.md`, `inbox/arsumbris-fork-backlog.md`,
`inbox/hermes-adapter-run-2026-11-15.md`, `manual/mcp-access.md` (stale),
and the type definitions in `au-mcp-sdk/type/` and `au-mcp-core/type/`.

External:
1. V. Rentea, "Overengineering in Onion/Hexagonal Architectures" — victorrentea.ro.
2. A. Morrison, "Master Hexagonal Architecture in Rust" — howtocodeit.com.
3. angelocatalani/hexagonal_architecture (Rust PoC) — port types beside the port, adapters outside.
4. minikin, "Transition Systems in Rust" (2025) — runtime table vs typestate, nested enums.
5. Zylos Research, "Finite State Machines and Statecharts for AI Agent Orchestration" (2026) — state-per-prompt.
6. Meta Engineering, "The Thin Orchestrator" (2026-10-19) — workflow templates as deterministic state machines.
7. Turion, "Durable Execution for AI Agents: Temporal vs Restate vs Hatchet vs Inngest" (2026).
8. arXiv 2602.03439 — ontology-to-tools compilation.
9. arXiv 2609.15779 (EvoOntology); arXiv 2608.22974 (ontology as a kernel).
10. arXiv 2506.01056 (MCP-Zero) — active tool discovery.
11. "MCP Best Practices: Production Playbook for the 2026 Spec" — aigrowthagent.co.
12. Prompt versioning / evals — montecarlo.ai, braintrust.dev (2025–2026).
13. ArchUnitRust; `architecture-fitness` crate (APS-V1-0002) — fitness functions as executable tests.
