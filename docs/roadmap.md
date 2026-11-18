# 2brain — what it is for, what exists, and what comes next

**What this page is.** The only page in this repository that says what happens next. Three pages answer three
different questions and do not repeat each other:

| page | question |
|---|---|
| [`README.md`](../README.md) | what the product is, and what it is not |
| [`architecture.md`](architecture.md) | how the system is built today |
| **this page** | why it exists, what of it is finished, and the order of the rest |
| [`trajectory.md`](trajectory.md) | the dated log of what landed, and what it cost |

Counts are not written here by hand. `tools/cards/metrics.json` records every number with the command that
produced it and the date; the cards in the README are rendered from the same file and CI fails when a card and
its number disagree (`tools/cards/render.py --check`). The status column below names a commit and a date, and a
number in prose is a memory.

**Measured at** `c0ad7cd`, 2026-11-18.

---

## 1. Why this exists

An agent session starts blind. What is lost when it ends is not documents — a repository has plenty — but the
**record of decisions**: what was chosen, what was refused and why, and what measurement backs it. A search
index returns the note that mentions a decision; it cannot say whether the decision was accepted, what it
replaced, or which test would notice if it stopped being true.

The thesis, kept from the independent review of 2026-11-16: **2brain is shared, checkable project memory for
several agents.** Three consequences shape every stage below:

- **The record is typed.** `type: decision` carries `backed_by`; `type: claim` carries `evidence`. "What backs
  this?" is a two-hop typed walk, not a reading exercise.
- **The files are authoritative and the graph is derived.** A decision recorded in markdown survives the engine
  being deleted and rebuilt. That is also why the graph can be optimised without fear.
- **A write is gated before it lands.** An agent can only write what the operator granted, and a refusal names
  its stage in the caller's own words, so the next attempt is informed rather than blind.

**Done means one scenario, not one component:** two different agents safely continue one project on one
knowledge base —

1. agent A records a decision with its evidence;
2. agent B finds it inside a bounded context and continues;
3. a concurrent edit produces the intended conflict;
4. a lost reply is reconciled without double-applying;
5. the change is in history;
6. every step is reproducible on a published SHA.

**The one thing this page does not claim is demand.** A study with external users is not executable here, so it
was dropped rather than promised. What replaces it is dogfooding with measurement: this vault, two clients, the
scenario above, run repeatedly, with the same numbers recorded that would be reported to a user. Demand is
argued from that record, or not argued.

## 2. What exists today

The serving path is one Rust binary over a Unix socket; there is no other language on the request path
(decision `7688c303`). What a caller gets:

| capability | what it means for a caller | check |
|---|---|---|
| structural reads | `overview`, `hubs`, `files`, `references_in`, typed `instances_of`, page-bounded | `2brain mcp <root>`, then `tb_overview` |
| ranked search | BM25 with phrase adjacency; title coverage and map yield applied after ranking | `tb_search`, measured in `tools/ranking/` |
| diagnostics | 0 errors on the live vault; codes carry spans and suggested fixes | `tb_diagnostics` |
| graph shape | isolated notes, components, cycles, bridges, articulation points, triangles, transitivity | `tb_graph_health` |
| document reads | one file, a line range, or many by path, each with the hash a write needs | `tb_content` |
| writes | create, write, edit, delete, batch, rename, inline/promote, type rename, workspace membership — 16 tools | `tb_*` behind a grant |
| refusals | a failed stage names itself (`policy/prior_read: …`) and the audit trail is hash-chained | `--allow-mutations`, stderr |
| recovery | an unanswered mutation is `outcome_unknown`, never retried; `tb_reconcile` says what happened | `tb_reconcile` |
| discovery | two tools answer "which tool does X" without loading the catalogue | `tb_search_tools` |
| waiting | `tb_watch` blocks until the workspace changes | `tb_watch` |

The counts, and the command behind each: `tools/cards/metrics.json` (`surface`, `gates`, `memory`,
`knowledge_base`, `graph`). What a consumer installs, and the trust boundary it accepts, are in
[`../tb-mcp-rs/docs/connecting.md`](../tb-mcp-rs/docs/connecting.md).

The MCP surface is deliberately a **subset** of the engine: 34 of the 51 wire reads and 16 of the 24 wire
mutations have a tool. The 17 unexposed reads and the 8 unexposed mutations are a decision, not an oversight —
`tests/wire_catalog.rs` pins both sides with a reason per verb (`NOT_EXPOSED` and `NOT_EXPOSED_MUTATIONS`), and
the test fails when a documented verb is neither exposed nor explained.

## 3. The five stages, and what "finished" means

The stages are ordered by what unblocks the next decision, not by what is interesting. Each has an exit
criterion that a command can check, so "done" is not an opinion.

| stage | goal | exit criterion | status at `c0ad7cd` | evidence |
|---|---|---|---|---|
| **1. The shared-memory contract is a contract** | a write either lands with a stated precondition or is refused; a refusal names why | four tests, each running the two-process case it describes, and `WIRE.md` plus the tool text say what the engine guarantees | **3 of 4** — concurrent create refuses (`create_new`), `expected_hash` is documented as a fragment precondition, a type rename that reaches an ungranted member is refused by `preview_mutation`; **promoting a draft into accepted knowledge is not a mode** | `docs/research/write-contracts.md`, `WIRE.md` |
| **2. Release hygiene** | a stranger can install it and read the terms | a clean machine installs from the published SHA, runs `doctor`, completes one read and one write | **partial** — CI green on both halves, license and trust boundary stated, clean-host gate passed on a toolchain-less host; **no published artifact** (`install.sh` builds with cargo) and **macOS unproven** | `docs/internal/clean-host-gate-2026-11-17.md` |
| **3. Task-level evidence for search** | ranking claims are per query class and pre-registered | a stated threshold per class, written before the next ranking change | **partial** — the task set is **19 rows over 12 classes** (`tools/ranking/tasks.json`) with a per-row gate (`M1`), and two changes moved named rows; the 40-question target is not met, and 1–3 rows per class is not a sample | `docs/research/task-set.md` |
| **4. The project-memory template** | a new workspace starts with the record types it needs | `2brain init --template project-memory` yields zero diagnostics, and the two-agent scenario passes with its transcript kept | **partial** — the template ships with six record types and a test that starts a daemon on a fresh workspace; **the transcript is 2 of 3** | `tb-mcp-rs/tests/init.rs`, `docs/research/cross-agent-handoff-2026-11-16.md` |
| **5. Knowledge-base intake** | the base grows deliberately: every note sourced, linked, and proposed before acceptance | the isolated share and the unsourced share fall while the note count stays flat | **partial** — the census card exists and three maintenance instruments run with `--selftest` (intake, maps, duplicates); the acceptance gate is the missing piece, and it is stage 1's fourth test | `docs/research/moc-coverage-2026-11-17.md`, `intake-targets`, `duplicate-findings` |

Two stages share one missing mechanism. **Promote-with-checks** is stage 1's fourth test and stage 5's
acceptance gate: today diagnostics are advisory on every normal mutation, which is right for a draft and wrong
for promoting one into accepted knowledge. That is why it is first in the queue.

## 4. The queue, in order of payoff per unit of risk

| # | item | why now | exit / gate | needs | group |
|---|---|---|---|---|---|
| 1 | **promote-with-checks** — a mutation rider that refuses on ERROR diagnostics instead of advising | closes stage 1 and unblocks stage 5's acceptance gate; the only open write-path gap. **ADR-0002** fixes the shape and **`write_file` is implemented** (engine check before the saga, adapter forwards the key only when asked); the remaining three mutations and the default-path measurement are open | what is left of the ADR-0002 gate: the client-level refusal, the same rider on `edit_file`/`create_file`/`batch`, and a measured default path | one Rust lane | **B** |
| 2 | **a published artifact**, and `install.sh` that uses it | a toolchain-less host can run the binaries but cannot install them | `install.sh` from the artifact on a host without cargo, then `doctor` | packaging; no cargo | **C** |
| 3 | **the task set to 40 rows** | 1–3 rows per class is not a sample; a percentage over it is arithmetic without a model | a pre-registered threshold per class, written before the next ranking change | `tools/ranking/`, no cargo | **C** |
| 4 | **the two-agent transcript** | stage 4's exit is a recorded run, not a template | the six-step scenario on a published SHA, transcript committed | live vault, one writer | **D** |
| 5 | **close the release-2 window** — refuse the legacy spelling | no writer emits it any more; the advisory proved the window is observable | `instances_of` with the legacy name fails with a named error; fixtures re-blessed in the same change | owner decision, one Rust lane | **B**, then **E** |
| 6 | the exact `grep` total | measured free (the scan is file-bound); the contract is the only question | a one-line answer in `WIRE.md` | owner decision | — |
| 7 | `claim` → `ingest-task` | one name, two records; the queue item is not a proposition | `instances_of claim` needs no `evidence` filter | **blocked** on the external queue writer, which is not in this repository | — |
| 8 | the map residue — 102 declarations across 21 maps | the last 27% of a measured 372; everything left is ≤4 notes per map | `tools/kb/moc_links.py --check` passes for the maps touched | one vault writer | **D** |
| 9 | semantic retrieval | **not a queue item until candidate generation proves to be the bottleneck** | recall of the candidate set measured first; selection measured separately | — | — |


## 5. How work runs in parallel

The constraint is not the number of lanes but three shared resources:

| resource | rule |
|---|---|
| **the cargo target lock** | one compile at a time. Every crate shares one target directory, so two Rust lanes do not run beside each other whatever they touch. A stale `cargo test` from an earlier session holds the lock for days (`pgrep -af "cargo test"`). |
| **the vault writer** | one writer. Multi-write passes (intake, map passes, migration) do not interleave with another writer. |
| **the deploy window** | one window, after every lane whose binary it must carry; the runbook is one capture / compare / rollback sequence. |

Safe to overlap: documentation and decision lanes (read-only) with any Rust lane; measurement lanes
(`tools/ranking`, `tools/probe`, `tools/kb` — Python only) with one Rust lane; the vault lanes with each other
only when they write disjoint paths.

Groups in the queue column: **B** a Rust lane (serial with every other B), **C** tooling and packaging (no
cargo), **D** vault writers (serial with each other), **E** the deploy window (after B).

## 6. What we refuse, and why

A refusal is the most valuable line in this repository: it stops a plausible idea being re-tried every month.
The full register, with the number behind each, is [`ideas.md`](ideas.md). The refusals a reader is most likely
to propose again:

| refused | the number |
|---|---|
| collapsing the 52 tools into 6 | the surface is a discovery ladder already; the concise profile was measured (`docs/research/tool-surface-tokens-2026-11-16.md`) |
| alias expansion at query time | 15 → 132 candidates (8.27×), the expected note ranked 21st (`docs/research/alias-field-2026-11-17.md`) |
| a confidence/abstention score | best rule 6/8 in-sample, **0/8 under leave-one-out** (`docs/research/abstention-2026-11-16.md`) |
| a freshness prior in ranking | rejected twice, the second time for a new reason (`docs/research/`, `task-set.md` §5) |
| a generated "Named by" section in maps | it is a second copy of the frontmatter index; written, measured, reverted (`moc-coverage-2026-11-17.md`) |

Two ideas that are often treated as refusals are **not** refusals here: a TUI, and a Hopfield-style selector
before labelled data. Nothing in this repository argues against either — the register itself carried both as
"refused" until this pass found no document behind the sentence, and demoted them to `unknown`. That correction
is the rule in action: an argued refusal has a number; an unargued one is a gap.

## 7. How this page stays honest

1. **A status is a commit and a date.** "Shipped" without both is a memory.
2. **An exit criterion is a command.** If it cannot be checked, it is a wish and does not belong in the table.
3. **A change of plan is a record, not an edit.** A decision that governs change goes to
   `.repowise/decisions.yaml` (`repowise decision add … --evidence …`, then `confirm`), and this page cites it.
   The decision that produced this page is `86a36bde`: the roadmap plans, the trajectory logs.
4. **`trajectory.md` records what happened; it never carries the plan.** When a stage moves, this page changes
   and the trajectory gains a dated line.
5. **Four checks keep it from drifting**: `repowise doc-drift` (a document the code no longer matches),
   `repowise decision health` (a stale or conflicting record), `tools/cards/render.py --check` (a card that
   disagrees with `metrics.json`), and `tools/docs/links.py --check` (every relative link and heading anchor;
   805 documents, 0 broken on 2026-11-18).
6. **A count in prose is a summary.** The instrument is `tools/cards/metrics.json`, which carries the command
   and the date; a prose number that disagrees with it is a stale sentence, not a second source of truth. The
   gate counts are written by `tools/cards/gates.py --from-log <cargo log> --suite <name>`, so they are read
   from the run rather than typed.
