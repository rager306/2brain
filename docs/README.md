# Documentation map

**Read this first.** The repository has 80 documents and three kinds. They answer different questions, and
mixing them is how a design page ends up describing one host's systemd unit.

| kind | where | answers | does not answer |
|---|---|---|---|
| **design** | `docs/*.md` | what the system is, why it is shaped this way, what was measured | anything about this machine |
| **research** | `docs/research/*.md` | one question, one measurement, one verdict — dated and never edited afterwards | what the current system does |
| **operations** | `docs/internal/*.md` | this host: paths, units, sockets, deploy logs, one operator's run | what the system is |

The split is a rule, not a habit. A document that names `/root/vault`, a systemd unit, a socket path or a client
config belongs in `docs/internal/`. `docs/internal/README.md` holds the reasoning, and design documents never
require an internal one to be understood — the repository is public.

---

## Start here

| if you want to… | read |
|---|---|
| understand the system in one page | [`architecture.md`](architecture.md) |
| see what is done, open, or rejected | [`trajectory.md`](trajectory.md) |
| know which decisions govern change | `repowise decision list` — records in `.repowise/decisions.yaml` |
| use it as an agent | [`../tb-mcp-rs/docs/agent-interface.md`](../tb-mcp-rs/docs/agent-interface.md), then [`progressive-disclosure.md`](../tb-mcp-rs/docs/progressive-disclosure.md) |
| connect a client | [`../tb-mcp-rs/docs/connecting.md`](../tb-mcp-rs/docs/connecting.md) |
| read the wire contract | [`../tb-engine/crates/tb-engine/WIRE.md`](../tb-engine/crates/tb-engine/WIRE.md) |
| judge a measurement in this repo | [`research/measurement-doctrine.md`](research/measurement-doctrine.md) |

---

## Design documents (14)

- [`agent-roles.md`](agent-roles.md) — Agent roles
- [`agents-and-decisions-assessment.md`](agents-and-decisions-assessment.md) — Agents, FSM roles, and a typed decision layer (subq / jev / ruVector)
- [`architecture-assessment.md`](architecture-assessment.md) — Architecture assessment: hexagonal layers, FSM control, and an ontology for the tool surface
- [`architecture-plan.md`](architecture-plan.md) — Architecture plan: guardrails, thin ports, a legible gate, an ontology with a consumer
- [`architecture.md`](architecture.md) — 2brain — architecture, in one page
- [`build-layout.md`](build-layout.md) — Build layout and disk budget
- [`graph-engine-assessment.md`](graph-engine-assessment.md) — Graph-engine assessment: is migrating to Samyama Graph (or another) justified?
- [`graph-surface.md`](graph-surface.md) — Surfacing the graph diagnostics
- [`naming.md`](naming.md) — Naming: the product is 2brain
- [`phase-d.md`](phase-d.md) — Phase D: retiring the Node stack
- [`rebrand-plan.md`](rebrand-plan.md) — Rebrand plan: from `arsumbris` to `2brain`
- [`repo-inventory.md`](repo-inventory.md) — Repository inventory: what each directory is, and what happens to it
- [`retrieval-spike.md`](retrieval-spike.md) — Retrieval spike (R1): a bundled engine against the current one
- [`trajectory.md`](trajectory.md) — Trajectory after the rebrand — what stands, what is open, what runs in parallel

## Research notes (52)

One question each, dated, and left as written. A note's verdict is the part that still counts; its measurements
are true of the commit it names. Grouped by the question they settle:

### What the system should be
- [`research/answer-contract-2026-11-16.md`](research/answer-contract-2026-11-16.md) — The answer contract, shipped as MCP prompts and workflow resources
- [`research/context-pack.md`](research/context-pack.md) — The context pack: what an agent needs, and why it is a contract rather than a tool
- [`research/durable-query-class.md`](research/durable-query-class.md) — The durable-knowledge query class, and what it says about ranking
- [`research/graph-algorithms.md`](research/graph-algorithms.md) — Graph, statistical and mathematical algorithms for 2brain
- [`research/intelligence-axes-2026-11-16.md`](research/intelligence-axes-2026-11-16.md) — The axes we can move, what each costs, and the number that decides it
- [`research/mcp-engine-path.md`](research/mcp-engine-path.md) — The MCP↔engine path, measured — what to delete now that there is no TypeScript
- [`research/payload-modes.md`](research/payload-modes.md) — Result payload modes, measured on the live adapter
- [`research/visit-bound-2026-11-16.md`](research/visit-bound-2026-11-16.md) — Work per read, measured at three corpus sizes
- [`research/write-contracts.md`](research/write-contracts.md) — The four write contracts, as they are

### Ranking and retrieval
- [`research/abstention-2026-11-16.md`](research/abstention-2026-11-16.md) — Abstention signal from existing data — 2026-11-16
- [`research/alias-field-2026-11-17.md`](research/alias-field-2026-11-17.md) — The alias class: measured 2026-11-17, not adopted
- [`research/candidate-set-first-2026-11-16.md`](research/candidate-set-first-2026-11-16.md) — Candidate-set first: what a scoped read pays for the notes it discards
- [`research/question-set-2026-11-16.md`](research/question-set-2026-11-16.md) — The 160-question set: how it was built, what it measures, and what it already shows
- [`research/question-set-baseline-2026-11-16.md`](research/question-set-baseline-2026-11-16.md) — The 160-question set, measured: keyword strings win, questions lose, and half the noise returns nothing
- [`research/second-opinion-2026-11-16.md`](research/second-opinion-2026-11-16.md) — Second opinion on the measurement stack, 2026-11-16
- [`research/self-retrieval-2026-11-16.md`](research/self-retrieval-2026-11-16.md) — Does every note answer to its own description? Yes — 1037 of 1037 — and the four exceptions are duplicates
- [`research/semantic-eval-practices-2026-11-16.md`](research/semantic-eval-practices-2026-11-16.md) — Retrieval evaluation practice: what to take, what to refuse
- [`research/semantic-judgements-2026-11-16.md`](research/semantic-judgements-2026-11-16.md) — Semantic judgements: does a returned note answer the question?
- [`research/source-url-lookup-2026-11-16.md`](research/source-url-lookup-2026-11-16.md) — Source-identity lookup: a pasted URL finds the note that carries it
- [`research/task-set.md`](research/task-set.md) — The task-level set, and the three defects it found on its first run
- [`research/tool-surface-tokens-2026-11-16.md`](research/tool-surface-tokens-2026-11-16.md) — Tool surface tokens: can the concise profile be the default?

### The knowledge base itself
- [`research/claim-register-2026-11-16.md`](research/claim-register-2026-11-16.md) — The claim register: typed claims bound to measured evidence (2026-11-16)
- [`research/claim-type-2026-11-17.md`](research/claim-type-2026-11-17.md) — `type: claim` — one name, two records, and the split that survives review
- [`research/duplicate-findings-2026-11-17.md`](research/duplicate-findings-2026-11-17.md) — One finding recorded twice: 7 duplicate pairs merged, measured 2026-11-17
- [`research/family-field-2026-11-17.md`](research/family-field-2026-11-17.md) — `family:` — what it does today, and the hybrid that keeps both jobs
- [`research/hermes-log-findings.md`](research/hermes-log-findings.md) — What Hermes' logs say about the 2brain server
- [`research/intake-2026-11-16.md`](research/intake-2026-11-16.md) — The 76 unlinked notes were 13 — and the hole was in-degree
- [`research/intake-decisions-2026-11-16.md`](research/intake-decisions-2026-11-16.md) — The 38 refusals, decided — and the arithmetic that says the metric cannot move here
- [`research/intake-targets-2026-11-17.md`](research/intake-targets-2026-11-17.md) — Intake targets: 33 content notes nothing points at
- [`research/kb-intake.md`](research/kb-intake.md) — Where the knowledge base actually grows, measured
- [`research/lineage-echoes-2026-11-16.md`](research/lineage-echoes-2026-11-16.md) — Lineage echoes: 36 notes are 17 observations, and 13 of the 17 echo groups carry no kinship signal
- [`research/moc-coverage-2026-11-17.md`](research/moc-coverage-2026-11-17.md) — Topic maps link half their members: 372 of 732 declarations, measured 2026-11-17
- [`research/vault-coverage-kutha-themes.md`](research/vault-coverage-kutha-themes.md) — Vault coverage of the Kutha ADR themes

### Migration and schema
- [`research/release-2-schema-2026-11-17.md`](research/release-2-schema-2026-11-17.md) — Release 2, tier C step 7: the engine schema moved to `tb.engine.*` and `::tb-engine`
- [`research/release-2-window-2026-11-17.md`](research/release-2-window-2026-11-17.md) — The migration window, made observable (release 2, step 7b)
- [`research/tier-c-inventory.md`](research/tier-c-inventory.md) — Tier C inventory: what the on-disk format change actually touches
- [`research/tier-c-seams.md`](research/tier-c-seams.md) — Tier C seams: `.arsumbris` → `.2brain`, `au.engine.*` → `tb.engine.*`

### External systems studied
- [`research/arscontexta-design-2026-11-16.md`](research/arscontexta-design-2026-11-16.md) — Ars Contexta → 2brain: system design extraction
- [`research/arscontexta-methodology-2026-11-16.md`](research/arscontexta-methodology-2026-11-16.md) — arscontexta methodology corpus → what 2brain dropped (2026-11-16)
- [`research/arscontexta-skills-2026-11-16.md`](research/arscontexta-skills-2026-11-16.md) — arscontexta skills → 2brain: what the operational layer has that the MCP successor lacks
- [`research/cross-agent-handoff-2026-11-16.md`](research/cross-agent-handoff-2026-11-16.md) — A cross-agent handoff, run on the live vault (2026-11-16)
- [`research/judge-capabilities.md`](research/judge-capabilities.md) — Judge (Subq) capabilities — measured, 2026-11-16
- [`research/kutha-ideas-2026-11-16.md`](research/kutha-ideas-2026-11-16.md) — What kutha-graph has that applies to a notes knowledge base — twenty mechanisms, ranked
- [`research/meta-prompt-fsm.md`](research/meta-prompt-fsm.md) — Meta / prompt FSM: states, guards, and where it lives
- [`research/pogocache-patterns.md`](research/pogocache-patterns.md) — Pogocache patterns, and which of them 2brain should take
- [`research/ruvector-adr-2026-11-16.md`](research/ruvector-adr-2026-11-16.md) — What the ruvector ADR corpus decides that a git-backed note base can use
- [`research/ruvector-agent-2026-11-16.md`](research/ruvector-agent-2026-11-16.md) — What the RuVector agent layer knows that a memory product needs
- [`research/ruvector-crates-2026-11-16.md`](research/ruvector-crates-2026-11-16.md) — RuVector crates: twelve mechanisms a notes knowledge base can use
- [`research/semantic-eval-practices-2026-11-16.md`](research/semantic-eval-practices-2026-11-16.md) — Retrieval evaluation practice: what to take, what to refuse
- [`research/semantic-judgements-2026-11-16.md`](research/semantic-judgements-2026-11-16.md) — Semantic judgements: does a returned note answer the question?
- [`research/sparx-2026-11-17.md`](research/sparx-2026-11-17.md) — sparx: static graph analytics and fit for 2brain
- [`research/superpowers-gsd-egm-2026-11-17.md`](research/superpowers-gsd-egm-2026-11-17.md) — Superpowers, GSD, and Elephant–Goldfish: fit for 2brain
- [`research/vault-coverage-kutha-themes.md`](research/vault-coverage-kutha-themes.md) — Vault coverage of the Kutha ADR themes

## Operations (14)

Everything here names this host. `docs/internal/README.md` explains why that is a directory rather than a
discipline violation.

- [`internal/clean-host-gate-2026-11-17.md`](internal/clean-host-gate-2026-11-17.md) — Clean-host gate, 2026-11-17: two binaries and a vault copy on bkk-edge01
- [`internal/deploy-2026-11-16-batch.md`](internal/deploy-2026-11-16-batch.md) — Deploy checklist: the 2026-11-16 batch
- [`internal/deploy-2026-11-17-r2.md`](internal/deploy-2026-11-17-r2.md) — Deploy, 2026-11-17: release 2's schema flip
- [`internal/deploy-2026-11-17-s1.md`](internal/deploy-2026-11-17-s1.md) — Deploy, 2026-11-17: the S1 ranking change
- [`internal/deploy-2026-11-17-window.md`](internal/deploy-2026-11-17-window.md) — Deploy, 2026-11-17: the migration-window advisory
- [`internal/deploy-2026-11-17.md`](internal/deploy-2026-11-17.md) — Deploy window, 2026-11-17: engine, adapter, and the vault identity migration
- [`internal/detach-and-consolidate.md`](internal/detach-and-consolidate.md) — Detach from upstream and converge on one language
- [`internal/engine-swap-2026-11-14.md`](internal/engine-swap-2026-11-14.md) — Engine swap — 2026-11-14 13:07 +07
- [`internal/engine-swap-2026-11-16.md`](internal/engine-swap-2026-11-16.md) — Fifth engine swap: `tb_search` into the served daemon
- [`internal/hermes-2brain.md`](internal/hermes-2brain.md) — Pointing Hermes at 2brain
- [`internal/hermes-brief.md`](internal/hermes-brief.md) — Бриф для Hermes: работа с волтом через Rust-адаптер
- [`internal/hermes-switch.md`](internal/hermes-switch.md) — Switching a Hermes session to the Rust adapter
- [`internal/serving-parity.md`](internal/serving-parity.md) — Serving parity: what the Node daemon and the bridge give, and what the Rust adapter does not
- [`internal/w4.1-judgement-run-2026-11-16.md`](internal/w4.1-judgement-run-2026-11-16.md) — Judgement run: subq-snap-1.2 over a labelled sample (W4.1 phase 2)

---

## How a document gets written, and when it is retired

1. **A question first.** A research note opens with the question, the method, the numbers, and a verdict —
   `docs/research/measurement-doctrine.md` says what makes a number trustworthy here.
2. **A rejected change is recorded, not deleted.** The note that says "measured, refused, because…" is the
   most valuable kind in this tree: it stops the same idea being re-tried every month.
3. **Design documents are edited in place; research notes are not.** When a design page stops being true, the
   history is in Git and the note it superseded keeps its date.
4. **A decision that governs change belongs in `.repowise/decisions.yaml`**, not only in prose. That store is
   queryable (`repowise decision list`, `repowise why`) and `repowise decision health` names the stale ones.
5. **`repowise doc-drift` reports a document the code no longer matches.** It is the check that this map stays
   honest.
