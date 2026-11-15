# Trajectory after the rebrand — three tracks, in order of payoff per unit of risk

Written 2026-11-16, after tier A and B of the rebrand landed and the research documents under
`docs/research/` were measured. This replaces "what next" with a ranked queue: every track names its
gate, its cost, and the evidence it would produce.

## Where the system stands

| | state |
|---|---|
| serving path | `/usr/local/bin/2brain` → engine socket, Rust only |
| state root | `~/.2brain` (legacy `~/.arsumbris` is a symlink), unit `2brain-engine.service` |
| tools | 52; `au_search` answers through the live binary (ranked + exact verified) |
| judge | batched: 100 questions per call, 0.76 s, 7.6 ms/question (`docs/research/judge-capabilities.md`) |
| rebrand | tier A + B done; tier C (format) and D (scheme) open with recipes |
| FSM run log | **done** — `tools/fsm/check_run.py`, seven rules, `--selftest` proves each bites |
| hygiene pass | **done and applied** — `tools/hygiene/pass.py`, `inbox/hygiene-2026-11-16.md` |
| ranking eval | **baseline measured** — 60 labelled queries, hit@1 0.43 (literal) / 0.60 (topical) |

## Track 1 — make a run auditable (no model, small)

Ship the FSM run log and its checker (`docs/research/meta-prompt-fsm.md` §"Next steps" 1):
`tools/fsm/check_run.py` validates a JSON-lines run log — every transition has a state, a guard name, the
call that evaluated it, and an outcome; a transition without evidence fails the check.

- **Gate**: the checker fails on a hand-written bad log (a real negative test) and passes on a real run.
- **Cost**: a day, no model calls, no live risk.
- **Why first**: it makes every later track's output auditable, and it is the only piece of the FSM that
  needs no judgment at all.

## Track 2 — nightly hygiene pass (first real batched-judge user)

One job, read-mostly, writes exactly one note per night:

1. deterministic candidates: orphans, cycles, components, staleness by git (body changed, description not);
2. one judge batch of ≤100 candidates asking the inverted staleness question;
3. emit `inbox/hygiene-<date>.md` with the ranked list; never edit an existing note.

- **Gate**: the note validates (`au_validate_value`), diagnostics stay 0, the job is idempotent for the
  same vault commit, and a dry run writes nothing.
- **Cost**: ~1 s of graph work + 1 judge call + 1 write.
- **Why second**: it exercises batch, the FSM's CONSOLIDATE state, and the write gate against real
  content, without touching anyone's notes.

## Track 3 — ranking (engine, measurable) — baseline exists, change not started

The measuring instrument now exists: `tools/ranking/build_queries.py` derives 60 queries from the vault
(7 `literal` — a phrase occurring in exactly one note, so any correct ranker must return that note first —
and 53 `topical` — content words from a note's own filename), and `tools/ranking/eval.py` scores hit@1,
hit@5 and MRR per class.

Baseline on 2026-11-16, before any change:

| class | n | hit@1 | hit@5 | MRR |
|---|---:|---:|---:|---:|
| literal | 7 | 0.429 | 0.714 | 0.568 |
| topical | 53 | 0.604 | 0.811 | 0.697 |

Nine queries do not place the expected note in the top ten at all. **The literal class is the finding**:
a phrase that occurs in exactly one note reaches rank 1 only 43% of the time, because BM25 scores bag of
terms — no proximity, no phrase credit. So the first change is not freshness; it is ranking what matched.

Candidate changes, each scored against this baseline:

1. **phrase proximity** — score a document higher when query terms appear adjacent (the literal class is
   the direct test);
2. **type-aware freshness prior** — `current 1.0, aging 0.7, stale 0.3, superseded 0.05`, durable
   families undecayed (the topical class is where a stale note currently outranks a fresh one);
3. **field weights** — title/description vs body, already partially there.

Multiplicative freshness prior on top of BM25: `current 1.0, aging 0.7, stale 0.3, superseded 0.05`, and a
family multiplier so durable types (`pattern`, `decision`, `extract`) do not decay while inbox/session
notes get a half-life. This is the backlog's P1-5, and it now has a search surface to measure against.

- **Gate**: a labelled query set (30–50 queries with a known best note) shows no regression and at least
  one clear improvement; the golden `au_search` capture is blessed in the same commit.
- **Cost**: ~150 lines in `search.rs` + tests.
- **Why not first**: without the labelled set it is a guess with a plausible story.

## Track 4 — finish the rebrand (tier C, then D)

Tier C: engine accepts `.arsumbris/` and `.2brain/`, `au.engine.*` and `tb.engine.*`, plus a migration
command; gate is `au_diagnostics == 0` on the vault after the rewrite. Tier D: serve both resource
schemes for one release, then drop.

- **Gate**: tier C — a repository migrated by the command opens with 0 diagnostics and the old tree still
  opens; tier D — both schemes answer, fixtures blessed in the drop commit.
- **Cost**: tier C is the largest single change left; tier D is small once C exists.
- **Why last**: it is the only track that changes files users have on disk.

## Track 5 — consolidation (only after 1–3)

Duplicate families via MinHash prefilter → one judge batch → proposals as `inbox/` notes. Deferred until
tracks 1–3 give it a log, a batched path, and a ranking to sort proposals by.

## Evidence discipline for all of them

Each track adds a row to `docs/verification.md` with the command that re-measures it, and each measurement
that came from a model run records the model, the batch size and the probability. "It looked right" is not
a result in this repository.
