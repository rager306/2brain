# Meta / prompt FSM: states, guards, and where it lives

Goal: make a 2brain agent run **replayable and checkable** — an explicit state machine whose states are
roles (`docs/agent-roles.md`), whose exits are machine-checkable, and whose predicates may be answered by
the judge (a calibrated classifier, 7.6 ms per question in batches of 100 — `judge-capabilities.md`).

Non-negotiables, inherited from the system as built:
- the engine is the only writer; the adapter is a fail-closed gate (`policy/<stage>` refusals);
- determinism first: a graph read or a string check decides whatever it can; the judge decides only what
  code cannot (`[[pattern-deterministic-first-llm-fallback]]`);
- the judge answers *predicates*, never authors content that lands unfiltered;
- every state exit is a tool call or an engine read — never "the model says it is done";
- a run is replayable from its transcript plus its inputs.

## State vocabulary

| state | entry | exit (machine-checkable) | judge may decide | tools |
|---|---|---|---|---|
| **ORIENT** | a task arrives | `tb_overview` answered and a member named | which topic family this looks like (optional) | `tb_overview`, `tb_members`, `tb_types`, `tb_search` |
| **LOCATE** | orient done | a non-empty candidate set with ids | "is this candidate the same subject?" | `tb_search`, `tb_grep`, `tb_neighborhood` |
| **READ** | candidates chosen | every candidate read with a hash | — | `tb_content`, `tb_frontmatter` |
| **PROPOSE** | read done | a typed proposal object (what, where, why) | duplicate / contradiction / relevance among candidates | `tb_validate_value` |
| **VALIDATE** | proposal exists | `tb_validate_value` returns valid; diagnostics unchanged | nothing — this is deterministic | `tb_validate_value`, `tb_diagnostics`, `tb_graph_health` |
| **APPLY** | validation green | engine answers `applied` with a commit, or `outcome_unknown` | — | `tb_write_file`, `tb_edit_file`, `tb_rename`, `tb_batch` |
| **RECOVER** | `outcome_unknown` | `tb_reconcile` returns a settled hash | — | `tb_reconcile` |
| **VERIFY** | apply settled | `tb_diagnostics == 0` and the written text reads back | — | `tb_content`, `tb_diagnostics` |
| **CONSOLIDATE** | verify done, or a scheduled pass | proposal set emitted as notes, not edits | pairwise duplicate / contradiction | `tb_search`, `tb_batch`, judge batch |

Nine states, one linear spine plus two loops (`RECOVER` back to `APPLY`'s aftermath, `CONSOLIDATE` back to
`PROPOSE` for each merged pair).

## Transition guards

Deterministic (no model may override):

- `diagnostics_zero`: `tb_diagnostics {severity: "warning"}` returns `[]`.
- `hash_fresh`: the `expected_hash` came from a read in this session (the gate enforces it; the FSM must
  not paper over a `policy/freshness`).
- `settled`: `tb_reconcile` reports a hash, not `unknown`.
- `dry_run_safe`: `tb_rollback_preview` reports `safe: true` before any revert.
- `budget`: at most K mutation attempts per run (default 3). Exceeding it is a **hard stop**, not a retry.

Judged (a probability with a threshold, recorded with the run):

- `same_subject p ≥ 0.8` — candidate is about the same thing (LOCATE).
- `duplicate p ≥ 0.9` — the pair is the same claim (CONSOLIDATE). Threshold is high because a merge is
  expensive to undo, and the measured detector is 1.000/0.889.
- `stale_description p ≤ 0.2` (inverted question) — triage only, never an automatic rewrite; recall was
  0.333 per note, so the FSM routes these to PROPOSE with `needs_human: true`.

Every judged guard records `{question, probability, threshold, model, batch_id}` in the run log, so a
disagreement is auditable rather than mysterious.

## Where the FSM lives

Three candidates, judged against the codebase:

1. **In the engine** — rejected. The engine is deterministic and model-free by design; putting a judge in
   it would put a sampler on the request path of every client.
2. **In the adapter** — rejected for the *orchestration*, kept for the *gate*. The adapter already owns the
   fail-closed policy and the audit ledger; adding a state machine there would make one binary both a
   protocol server and a workflow engine, and the workflow would have to be redeployed with every change.
3. **In an external orchestrator** (Hermes / prime-agent session) — **chosen**. It already has the model,
   the tools, and the ability to run for hours; the FSM is the *contract* it follows, expressed as a small
   skill plus a run-log schema. The engine and adapter stay dumb and safe, which is what makes them
   trustworthy.

The FSM therefore ships as: a skill document (the states and guards), a run-log format (one JSON line per
transition), and a checker (`tools/fsm/check_run.py`) that replays a log and fails when a transition had no
guard evidence.

## Failure and recovery

| failure | detection | response |
|---|---|---|
| judge low confidence | `confidence < 0.6` on a guard | do not transition; gather one more deterministic fact (another read, a grep) |
| judged guards disagree with each other | two guards on the same pair conflict | stop, emit a "needs human" proposal — do not break the tie with a third model call |
| `outcome_unknown` | the tool result says so | RECOVER state; **never** re-apply; `tb_reconcile` |
| oscillation | the same state entered 3× without progress | hard stop with the run log; this is a bug in the guards, not in the task |
| diagnostics nonzero after APPLY | VERIFY gate | propose a revert via `tb_rollback_preview`; never silently continue |

## Three traces

**1. Capture a decision from a session.**
ORIENT (`tb_overview`) → LOCATE (`tb_search` "decision we took about X") → READ 2–3 notes → PROPOSE one
`decision` note → VALIDATE (`tb_validate_value`, diagnostics) → APPLY (`tb_write_file`) → VERIFY.
Cost: 1 model turn, 0 judge calls, 5 tool calls.

**2. Consolidate duplicates (nightly).**
CONSOLIDATE: `tb_search` per topic → shingle prefilter → **one judge batch of ≤100 pairs** (`duplicate p`)
→ for each p ≥ 0.9, PROPOSE a merged note and a delete → VALIDATE → APPLY as `tb_batch` → VERIFY.
Cost: 1 judge call (100 pairs, 0.76 s), 0 model turns until a human accepts. This is the whole reason the
batch measurement matters: 1 036 notes would otherwise be 1 036 judge calls.

**3. Background hygiene pass (nightly).**
CONSOLIDATE: components/orphans/cycles from `graph_health`; staleness candidates from git (body changed,
description did not); **one judge batch of ≤100 candidates** (`stale p`); emit a single `inbox/` note per
night with the list. No writes to existing notes. Cost: 1 judge call, 1 note write.

## What would make this wrong

1. **Guards that are really vibes.** If a guard is "the model thinks the note is fine", the FSM is
   decoration with a state diagram. Every guard must name the call that evaluates it.
2. **Judge thresholds tuned on nothing.** 0.8/0.9 are placeholders until a labelled sample (30–50) exists
   per question; shipping them untuned would make the FSM confident and wrong.
3. **A run log nobody reads.** The log is only worth its cost if `check_run.py` runs in CI or on every
   nightly pass; otherwise the FSM is documentation of an intention.

## Next steps that are cheap

1. Ship the run-log schema and `check_run.py` (no model involved) — it makes existing runs auditable today.
2. Label 50 pairs for `duplicate p` on our corpus and set the threshold from data; the detector already
   measured 1.000/0.889 on 50 notes.
3. Implement the nightly hygiene pass (#3) as the first real user of the batched judge: it is read-mostly,
   produces a note, and touches no existing content.
