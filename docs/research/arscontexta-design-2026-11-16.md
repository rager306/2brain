# Ars Contexta → 2brain: system design extraction

Written 2026-11-16. **Status: research/model.** Source tree: `/root/vendor-source/arscontexta`
(read-only, plugin v0.8.0), documents under `reference/` plus the repo `README.md`. Everything in
sections 1-4 describes Ars Contexta as its own reference documents state it; every proposal for
2brain is marked **[design]**. Live 2brain numbers cite the 2brain documents that measured them.

---

## 1. The model

### 1.1 Three spaces

Source: `reference/three-spaces.md`. The separation is justified not by taste but by six named
conflation failures: each way of mixing two spaces has a documented, predictable breakdown
(three-spaces.md, "Six Failure Modes of Conflation").

- **self/ — the agent's persistent mind.** `identity.md`, `methodology.md`, `goals.md`; grows by
  tens of files, never hundreds, and is fully loaded at every session start. Configurable: off by
  default for research vaults (goals route to `ops/goals.md`, methodology to `ops/methodology/`,
  identity into the context file), on for personal assistants. Design rule: "only what the agent
  needs about itself" — it is not a second knowledge graph.
- **notes/ — the user's knowledge graph.** Atomic prose-titled notes (title IS the API signature:
  `[[Mom prefers phone calls on Sunday mornings]]`), wiki links with prose context, MOC overlay
  (hub → domain → topic), topics footer declaring membership. Grows 10-50 notes/week; too large to
  load, so it is read by progressive disclosure: description field before body, MOC before note.
  Structural constants hold across domains; names and fields adapt (vocabulary transforms,
  `reference/vocabulary-transforms.md`).
- **ops/ — operational coordination.** Queue state, session logs, health reports, observations and
  tensions, reminders, the derivation record. Temporal by design: content flows through and either
  graduates or is archived. Promotion is strictly one-directional — `ops/observations/` → `notes/`
  or `self/methodology.md` when it earns permanence; durable knowledge never flows back into ops.

### 1.2 Claims and dimensions (the derivation engine)

Sources: `README.md` (setup flow), `reference/dimension-claim-map.md`, `reference/interaction-constraints.md`.

- The plugin ships a research corpus of **249 claims** (`methodology/`), each an atomic
  prose-titled note held to the composability gate: "claims must be specific enough to be wrong"
  (`reference/claim-map.md`, note-design section). This corpus is the plugin's own vault — it eats
  its own dog food.
- **Eight configuration dimensions** parameterize every generated system: granularity, organization,
  linking philosophy, processing intensity, navigation depth, maintenance sensitivity, schema
  density, automation level. `dimension-claim-map.md` gives each dimension a default position and
  the specific claims that back it, plus a cross-dimension interaction table.
- **Derivation flow** (README, 6 phases): detect environment → understand the domain in 2-4
  conversation turns → derive (signals → dimension positions with confidence) → propose in the
  user's vocabulary → generate everything (context file, folders, templates, skills, hooks, manual)
  → validate all kernel primitives and smoke-test the pipeline. Derivation, not templating: "every
  choice traces to specific research claims".
- **Interaction constraints** prune the configuration space: 8 dimensions × 3 positions = 6,561
  theoretical combinations, "most are incoherent". Three cascades (granularity, automation, volume)
  are documented; hard constraints BLOCK, soft constraints WARN, compensating mechanisms cover the
  rest. The document's own conclusion: "granularity cascades are hard, automation cascades are soft"
  — you cannot atomic-granularity your way out of missing navigation depth, but discipline can
  compensate for missing automation.

### 1.3 A session

Source: `reference/session-lifecycle.md`. Kernel primitive `session-rhythm`: orient → work → persist.

- **Orientation has a loading order that matters**: identity → methodology → goals → task context.
  The claim: identity constrains behavior, so loading task context first produces an agent that
  knows what to do but not how — voice drift was traced to sessions that did this.
- **The smart zone** is the first ~40% of context; quality degrades progressively past it. Session
  types get different budgets: processing sessions orient from queue state (~10%), exploration
  sessions from MOCs (~25%), maintenance from health metrics, capture from almost nothing.
- **Four session types** with distinct rhythms: processing (queue in, queue advanced out),
  maintenance (health checks as the sole focus, not a side activity), exploration (start from a
  MOC, checkpoint during traversal, persist discoveries immediately), capture (zero-friction
  filing; capture sessions never process).
- **`goals.md` is the handoff vehicle** — a living current-state document, explicitly preferred
  over append-only session logs for orientation. `ops/reminders.md` carries time-bound items.
  Session capture is an invariant primitive: the Stop hook saves transcripts to `ops/sessions/`
  and auto-creates mining tasks, so the learning loop works even when the agent forgets.
- **Skipping persist is named "the most common and most damaging session failure"** — which is why
  the transcript-saving part is automated and the rest is placed prominently with CRITICAL framing.
- **Condition-based triggers replace all time-based scheduling**: "topic MOC exceeds 50 notes"
  fires when true, "not on a Tuesday"; conditions do not stack during inactivity. The morning
  briefing aggregates queue state, fired conditions, and health into one orientation snapshot.
- **Discovery during work** is captured minimally and deferred: "give it its own session", so a
  tangent cannot consume the current task's context.

### 1.4 The evolution

Source: `reference/evolution-lifecycle.md`.

- **Seed → evolve → reseed.** The generated system is "a hypothesis about what the user needs",
  validated through use. Reseed is triggered by coherence degradation (context file contradicting
  itself, schema fields nobody queries, hooks enforcing undocumented rules), never by a calendar.
  Partial reseed regenerates one module; `ops/user-overrides.md` survives reseed as immutable
  constraints — without that guarantee, reseed destroys accumulated learning.
- **The operational learning loop** is the system's immune system: observations and tensions are
  captured as atomic notes; thresholds (10 pending observations, 5 pending tensions) surface a
  review; triage dispositions are promote / implement / archive / keep-pending (tensions resolve or
  dissolve); invalid transitions are forbidden (you cannot un-promote).
- **v1.6 reversed Gall's Law**: every vault ships with full automation and users opt down via
  `/architect`, because unused features cost near zero while discovering and adding them later cost
  more than expected. Safe disabling is the invariant: every optional feature has a documented
  fallback path; INVARIANT primitives cannot be disabled at all.
- **Drift detection has three types and three timings**: staleness (config newer than methodology
  notes — checked at session start, <1 s), coverage gap (active feature with no methodology note —
  checked at `/next`, 5-10 s), assertion mismatch (specifications contradicting each other — checked
  at `/rethink`, 30-60 s). All findings become observations; resolution of mismatches needs judgment.
- **Guardrail**: the kernel is invariant across all evolution. "A system that evolves away from
  prose-sentence titles ... has evolved into incoherence."

### 1.5 Comparison with 2brain's model — and the verdict

| axis | Ars Contexta (reference docs) | 2brain (engine + MCP server) |
|---|---|---|
| unit of knowledge | atomic prose-titled note, one claim per file (`reference/components.md`) | typed note: frontmatter `type:` claim resolved against type definitions; unresolvable claim is a diagnostic (`/root/2brain/README.md`, Concepts) |
| schema | template `_schema` blocks; templates are the single source of truth; validated by PostToolUse hooks | frontmatter contracts in the engine, `schema_version` 29 on the wire, refuses mismatch instead of guessing |
| edges | wiki links with prose context ("WHY" in the sentence) | wiki links + addressable `^block-ids`; `family:`/`source_url` provenance fields |
| enforcement | hooks + conventions inside one host agent; "hook enforcement guarantees quality while instruction enforcement merely suggests it" (`dimension-claim-map.md`, Dimension 8) | fail-closed write gate in the serving path: prior read + hash, staged refusals (`policy/prior_read`, `policy/absence`), lost reply = `outcome_unknown`, reconciled not retried |
| identity | `self/` per vault — one operator, one agent mind | principals, per-call attribution, audit ledger — many agents, one store |
| sessions | prompted rhythm + SessionStart/Stop hooks in the host | MCP sessions over a socket: 52 tools, 16 mutations behind `--allow-mutations`, version counter on every read, serialized mutation slots |
| evolution | reseed = re-derive generated files from a new conversation | engine swap with golden-capture diffs and a versioned wire |
| search | optional qmd embeddings + BM25 + LLM rerank, with a mandatory non-semantic fallback chain (`semantic-vs-keyword.md`) | engine BM25 + phrase adjacency; `weak` and `excerpt` fields landing; ranking measured in `tools/ranking/` |

**Verdict: for a Rust engine serving many agents, 2brain's contract-first model is the right
substrate, and Ars Contexta is the right methodology layer on top of it.** The reasons are mostly
Ars Contexta's own claims turned back on it:

1. **Its sharpest enforcement claim favors the engine.** "Hook enforcement guarantees quality while
   instruction enforcement merely suggests it" (`dimension-claim-map.md`, Dimension 8; kernel
   primitive `schema-enforcement` grounding: "instruction compliance degrades as context fills").
   A Rust write gate is the strongest possible hook: validation stops living in a generated context
   file each host loads and starts living in the one authority every client passes through. A hook
   belongs to one agent's process; the gate belongs to the store.
2. **`self/` cannot serve many agents.** One identity per vault is a single-operator assumption.
   N agents sharing one store need N identities or a shared lie; 2brain's principals, attribution
   and audit are the multi-agent answer. Ars Contexta agrees this is unsolved: multi-agent
   coordination is listed in its exclusion notes (`session-lifecycle.md`), and "Multi-Agent
   Collaboration in Generated Systems" is an open research question (`reference/open-questions.md`).
3. **Dimension positions are per-deployment prose.** After derivation, the 8 dimension choices live
   in a generated context file the host loads. The engine cannot enforce what it cannot see; 2brain's
   equivalents (type definitions, workspace templates, wire schema) are machine-checked instead.
4. **Where Ars Contexta is genuinely ahead** is the lifecycle, not the contract: orient/persist as
   an explicit protocol, condition-based maintenance replacing schedules, the observation/tension
   loop with thresholds and dispositions, three-typed drift detection, and the falsifiability
   discipline of claims backing decisions. 2brain has fragments of all of these (tb-govern findings
   and corrections, `2brain doctor`, `tb_graph_health`, vault note statuses) but no composed loop.

So: keep the typed-notes/write-gate core; adopt Ars Contexta's loop as engine-native mechanisms,
which is a strictly stronger form of the same idea than her hooks.

---

## 2. The claim map as a falsifiability register

Sources: `reference/claim-map.md`, `reference/dimension-claim-map.md`.

**Exact shape.** The claim map is two documents that bind decisions to named, attackable claims:

- **Registry** (`claim-map.md`): six topic areas (graph-structure, agent-cognition,
  discovery-retrieval, processing-workflow, note-design, maintenance-evolution). Per area: Core
  Claims — each a wiki-linked prose proposition plus a one-line gloss — and Tensions/Risks lists
  that state what would count against the area's defaults ("curation quality vs scale — does human
  curation work past ~10,000 notes?", "hub concentration vs peripheral neglect"). A Cross-Topic
  Bridge Claims table maps claims appearing in several topics as synthesis opportunities.
- **Usage map** (`dimension-claim-map.md`): per dimension, a table (Claim | What It Says | Informs)
  followed by a **Default position** line including the condition under which to soften it
  ("Atomic — soften for reference-heavy domains"), then the cross-dimension interaction table.

**Why it functions as a falsifiability register** — three properties, all load-bearing:

1. Claims are propositions "specific enough to be wrong" (their own quality gate), not topics.
2. Every default position names the claims that back it: attacking the claim attacks the decision.
   The derivation engine refuses to generate a rationale it cannot trace (`AUDIT-REPORT.md` shows
   the template was designed to enforce per-claim Summary/Implication/Source).
3. The tensions are the falsification hooks — each records the observation that would defeat the
   default.

**What it lacks**: no status field, no evidence link, no measurement, no version per claim. The
register binds decisions to prose, not to data — and its own bookkeeping drifts: `kernel.yaml`
declares "Fifteen Universal Primitives" and defines 16 (verified by counting `- id:` entries); 14
carry `enforcement: invariant` while `interaction-constraints.md` names only 4 as truly
non-disablable (wiki-links, schema-enforcement, methodology-folder, session-capture). Two files
state the kernel's invariant boundary differently, and the audit report shows 9 of 14 non-exempt
reference documents non-compliant with their own template (`AUDIT-REPORT.md`). The register shape
is right; its upkeep is the hard part.

**What keeping one costs 2brain.**

- **Duplication risk (the big one).** WIRE.md, `docs/verification.md` and
  `tools/cards/metrics.json` are already the evidence of record; the vault's rule is that a card
  cannot drift from its number because CI re-measures it. A second prose register that does not
  cite those artifacts becomes a parallel source of truth — the exact failure the per-verb-counter
  work closed (the WIRE census is the contract; a 40-vs-52-arm mismatch lived in a doc for weeks).
- **Claim rot.** Ars Contexta's claims have no status; 2brain's vault notes already carry
  `status: proposed/accepted/superseded`. A register without statuses goes stale the same way the
  served binary lags the repository.
- **Curation cost.** 249 claims were a plugin-scale writing effort. 2brain's decision surface is
  far smaller — 52 tools, ~30 wire verbs, a handful of defaults worth defending — so a register of
  roughly 40-80 decision claims is realistic.

**[design]** Keep the shape, drop the medium: implement the register as typed notes, not a
hand-maintained index. `type: claim` (proposition + `evidence:` pointing at the measured artifact —
a metrics.json key, a benchmark doc, a test id) and `type: decision-note` with `backed_by:` claim
links, `supersedes:`, and a status. Then `tb_backlinks`/`tb_hubs` provide the navigation
claim-map.md provides by hand, and the register inherits the vault's own statuses and gates. Marginal
cost: one claim note per defended default at decision time. Marginal engine cost: none — fields and
types already exist.

---

## 3. `failure-modes.md` cross-referenced with live 2brain failures

Source: `reference/failure-modes.md` (10 failure modes: Collector's Fallacy, Orphan Drift, Link
Rot, Schema Erosion, MOC Sprawl, Verbatim Risk, Cognitive Outsourcing, Over-Automation, Productivity
Porn, Temporal Staleness; each with prevention and warning signs, plus a per-domain vulnerability
matrix). Cross-referenced against what 2brain measured on 2026-11-16:

| live failure (measurement) | Ars Contexta name | where | their prevention pattern |
|---|---|---|---|
| **Provenance echoes**: 36 notes are 17 observations (2.1x over-count); only 4 of 17 clusters declare kinship, all via `family:`; echo clusters are the exception to link density — 5 of 17 have any edge, none mutual (`docs/research/lineage-echoes-2026-11-16.md`) | **Verbatim Risk** (copying instead of transforming; "notes that read like summaries") amplified by **Collector's Fallacy** (560 inbox vs 384 knowledge notes; capture outpacing processing) | #6, #1 | generation-effect gate: "every note must transform, not just store"; the composability test ("This note argues that [title]"); source attribution separating original from transformation; WIP limits |
| **Hub outranks the atomic note**: for the event-log question the hub answers in one clause at rank 1 while the atomic argument sits at rank 8; also a 330-hit query puts a survey above the note titled after the question (`docs/research/semantic-judgements-2026-11-16.md`; `docs/trajectory.md`) | **Hub concentration vs peripheral neglect** — named not as a failure mode but as a recorded *tension* of the graph-structure topic, plus MOC Sprawl's "unmaintained MOC ... provides false confidence" | claim-map.md Tensions; #5 | MOC size thresholds (split at 35-40 links); interestingly their rubric rules "a hub that states the fact answers" — the hub is legitimate, the problem is ranking, which their stack does not solve either |
| **Cross-lingual retrieval**: Russian queries with zero shared content tokens return five wrong hits (T03, T18); the base also asserts on absent facts (T10-T12) (`semantic-judgements-2026-11-16.md`) | **Vocabulary divergence** — "same concept, different words". Not one of the 10 failure modes; it is the motivating case of the search-modality document, whose answer is the semantic-search tier | `semantic-vs-keyword.md` throughout | embedding search for cross-vocabulary matching; fallback chain so search failure never blocks work. 2brain has BM25 only, so this failure shape is exactly the one Ars Contexta added a whole primitive for |
| **No abstention**: 8 of 42 unanswerable questions get a confident wrong top hit ("absent 0/3 is a base asserting on questions it cannot answer"); a claim-level check over the top 20 that can return `no_evidence` fixes 16 of 42 (`semantic-judgements-2026-11-16.md`, bottom line) | **Cognitive Outsourcing** prevention: "flag confidence levels: the system should express uncertainty"; **Over-Automation**: "automation should fail loudly, not fix silently" | #7, #8 | uncertainty expression as a requirement, not a nicety. The `weak` field landing in the deploy batch is the weak form; `no_evidence` abstention is the strong form |
| **Deployment lag**: served binary behind the repository — MRR 0.637 at HEAD vs 0.619 served; six changes in repo, not in the served binary (`docs/trajectory.md`; `docs/internal/deploy-2026-11-16-batch.md`) | **Temporal Staleness** — "content becomes outdated but isn't flagged. Old notes presented as current." Warning sign: "no maintenance activity in domains with high change rates" | #10 | `meta_state` field (current / outdated / speculative); staleness sweeps; date-aware checks. The swap checklist flipping `2brain-serving-state-lags-the-repository` to `superseded` is precisely their staleness-field pattern applied to deployment |
| (adjacent) 35 of 36 `f/patterns` notes with zero inbound (`census` measurement, 2026-11-16) | **Orphan Drift** — warning sign fires at ">10% of notes have no incoming wiki-links"; theirs is ~97% within the type | #2 | mandatory connect step; topics footer as minimum connection; health checks |

Reading: four of the five live failures have direct precedents in the old system's vocabulary, and
two of them (abstention, echoes) have their prevention already written down as one-line policies
that were never mechanized. Ars Contexta's failure modes are behavioral because its enforcement
surface is behavioral (prompts and hooks). In 2brain each named prevention becomes an engine
mechanism: a gate, a field, a diagnostic, a refusal — which is the transfer, section 5.

---

## 4. `kernel.yaml` — what a kernel of a methodology system is

Source: `reference/kernel.yaml` (+ `reference/validate-kernel.sh`, `reference/derivation-validation.md` Test 5).

**What it contains.** Sixteen primitive entries (the header says fifteen — see section 2), each
with: `id`, `name`, `description`, `layer` (`foundation` / `convention` / `automation`), `requires`
(dependencies), `enforcement` (`invariant` or `configurable` — 14/2), a `validation` block with a
concrete check and threshold (">95% of notes have a description that differs from the title",
">90% of wiki links resolve", "<5% orphan notes after the creation window"), and a
`cognitive_grounding` line citing the research claim behind the primitive. A tail comment carries
the dependency DAG and the layer mapping. `validate-kernel.sh` mechanically checks the primitives;
`derivation-validation.md` Test 5 proves every primitive is satisfiable on a platform with no
hooks at all — "conventions ARE implementations".

**What a kernel IS.** In a system whose whole product is *variation* (a derivation engine that
generates a different vault per user), the kernel is the constant that makes variation safe:

- it is what reseed validates and what evolution may not remove ("evolution adds on top of the
  kernel; it never removes or contradicts kernel primitives" — `evolution-lifecycle.md`);
- it is the compatibility contract between two independently derived systems, stated as checkable
  primitives with thresholds rather than prose;
- it is the floor that keeps degraded modes honest — Test 5 shows what degrades at
  convention-only (validation reliability, semantic reach) and what does not (titles, links,
  MOCs, session rhythm).

**Should 2brain have one? Yes — a small one, and it is mostly implicit already.** 2brain's actual
invariants are: files are the truth and the graph is derived; `type:` claims resolve against type
definitions or diagnose; wiki links resolve; `^block-ids` address; one write authority (the gate,
fail-closed, lost replies are `outcome_unknown`); the wire schema version refuses mismatch; every
read carries the version counter; content problems are diagnostics, never serving failures;
provenance fields exist (`family:`, `source_url`); engine swaps go through golden captures. These
are enforced by code and tests, not enumerated as a validated list with thresholds.

**[design]** A `kernel.yaml` for 2brain would declare roughly ten primitives — files-are-truth,
typed-claims, wiki-links-resolve, block-ids-addressable, one-write-authority, wire-schema-version,
version-counter-on-reads, diagnostics-not-blocks, provenance-fields, engine-swap-safety — each
with `enforcement: invariant`, a named `2brain doctor` check, and a threshold where one makes
sense. Doctor already exists (one ok/warn/fail line per check); schema-contract tests already bind
wire to docs; so the work is documentation plus wiring, and the payoff is that an engine swap or
the pending format migration gets a machine-checked compatibility statement instead of prose.
The Ars Contexta inconsistency (15 declared, 16 defined; 14 vs 4 invariant) is the cautionary
example: a kernel that is not machine-counted will drift, so the count itself belongs in the check.

---

## 5. Ranked transfers

Ten transfers, ranked by measured payoff against live failures first, structural value second.
Cost: S = days, M = one to two weeks, L = a lane. Risk names the failure mode of the transfer.

| # | transfer | source | cost | risk | why here |
|---|---|---|---|---|---|
| 1 | **Abstention: `no_evidence` in ranking.** Claim-level check over the top 20 that can refuse instead of answering; extends the landing `weak` field. Measured ceiling: fixes 16 of 42 judged queries, including all 8 no-abstention failures. | `failure-modes.md` #7 (express uncertainty); `semantic-judgements-2026-11-16.md` | M | moderate — false refusals need a tuning pass; the judgement corpus exists to tune against | largest measured fix for the worst live failure (confident wrong answers) |
| 2 | **Condition-based maintenance queue.** graph-health thresholds (orphan %, dangling %, echo density, MOC size) evaluate to fired conditions that surface as tasks on a read (an `/next` analog), replacing time-based sweeps. | `evolution-lifecycle.md`, `session-lifecycle.md` (conditions fire when true, not on a Tuesday) | S-M | low — additive read surface; no behavior change | turns maintenance from cron into state; serves many agents with one shared, evidence-based queue |
| 3 | **Observation/tension loop into tb-govern.** Threshold triggers (N open findings → review task) plus the disposition vocabulary: promote / implement / archive / keep-pending, resolved / dissolved, with forbidden transitions. | `kernel.yaml` operational-learning-loop; `derivation-validation.md` Test 9 | S | low — tb-govern already has finding/correction state machines; this composes, not replaces | the learning loop is the old system's immune system; 2brain has the organs (findings) without the reflex (threshold → review) |
| 4 | **Cross-lingual query expansion table.** A per-workspace term/synonym table consulted at query time (ru ↔ en being the live pair); the deterministic, local version of their semantic-search tier. | `semantic-vs-keyword.md`; `vocabulary-transforms.md` | M | moderate — table upkeep; recall vs precision trade-off needs the probe harness (`tools/probe/`) | directly kills the zero-shared-token failure (T03/T18) without embedding infrastructure |
| 5 | **Durability classes + one-directional promotion.** Types carry durability (temporal vs durable); inbox → knowledge promotion is a real verb; durable content never demotes. | `three-spaces.md` (promotion is one-directional; six conflation failures) | M | low | structural answer to echoes and debris: the graph stops counting repeat extractions as independent observations once families gate ranking |
| 6 | **Description-quality diagnostics.** Engine diagnostic when `description:` is missing or restates the title; description treated as a retrieval filter (layer: heuristic → mechanism → implication). | `kernel.yaml` description-field + discovery-first | S | low | cheap deterministic enforcement of a primitive the old system could only prompt for; improves every ranking and judgement pass |
| 7 | **Claim register as typed notes.** `type: claim` with `evidence:` links to measured artifacts; `type: decision-note` with `backed_by:`/`supersedes:`; statuses inherited from the vault. | `claim-map.md`, `dimension-claim-map.md` (shape analyzed in section 2) | S | low — the risk is curation discipline, mitigated by citing metrics.json keys, not prose | makes engine defaults falsifiable; each default names the claim an attacker must defeat |
| 8 | **Session rhythm as opt-in MCP injects.** Orient = morning briefing (fired conditions + queue + reminders); persist = session capture note + auto-created mining task. Uses the existing `mcp.inject`/`agent-profile` vocabulary. | `session-lifecycle.md` | M | moderate — scope creep into host concerns; keep it per-agent-profile and opt-in | gives every connected agent the old system's best behavioral pattern without a hooks platform |
| 9 | **Kernel invariant list bound to doctor.** ~10 primitives with enforcement, a doctor check id and thresholds per section 4. | `kernel.yaml` (+ its own inconsistencies as motivation) | S | low | machine-checked compatibility contract for engine swaps and the format migration |
| 10 | **Interaction constraints at template composition.** When `tb-defaults` workspace templates grow composable knobs, validate combinations (hard = block, soft = warn) the way the derivation engine does. | `interaction-constraints.md` | S | low | lowest value today — 2brain has few knobs; park until composition grows |

Not transferred, deliberately: derivation-by-conversation (2brain has no per-user onboarding
surface and does not want one), `self/` identity space (superseded by principals), vocabulary
transformation as cosmetic renaming (the useful residue is transfer #4), full-automation-from-day-
one as a product posture (2brain already ships fail-closed, the stricter version).

---

## Appendix: sources read

`/root/vendor-source/arscontexta/README.md`; `reference/`: components.md, claim-map.md,
dimension-claim-map.md, three-spaces.md, semantic-vs-keyword.md, session-lifecycle.md,
evolution-lifecycle.md, failure-modes.md, interaction-constraints.md, derivation-validation.md,
kernel.yaml, vocabulary-transforms.md, personality-layer.md, AUDIT-REPORT.md, open-questions.md,
templates/ (11 templates). 2brain side: `README.md`, `AGENTS.md`,
`docs/research/semantic-judgements-2026-11-16.md`, `docs/research/lineage-echoes-2026-11-16.md`,
`docs/internal/deploy-2026-11-16-batch.md`, `docs/trajectory.md`,
`docs/research/intake-decisions-2026-11-16.md`, tb-govern type definitions.

Inconsistencies noted in the source tree (usable as cautionary evidence): kernel.yaml declares
"fifteen" primitives and defines 16; kernel.yaml marks 14 primitives invariant while
`interaction-constraints.md` names 4 as non-disablable; `AUDIT-REPORT.md` shows 9 of 14 non-exempt
reference documents non-compliant with their own structural template; `claim-map.md` and
`dimension-claim-map.md` overlap by design and the audit recommends merging them.
