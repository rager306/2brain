
# The durable-knowledge query class, and what it says about ranking

Built 2026-11-16 as the prerequisite the freshness prior was missing (`docs/trajectory.md` item 4). The prior
had been measured once and reverted, for a reason that was structural rather than numerical: the 60-query
labelled set asks "which note holds these words", and the expected note does not change when a note goes
stale. The claim the prior makes is different — *for the same words, prefer the note that stays true* — and
nothing in the set tested it.

## How the class is built (`tools/ranking/durable_probe.py`)

The vault declares a lifecycle per note in frontmatter: `freshness` on 784 notes — volatile 299, slow 198,
stable 157, evergreen 123 — and `status` on 991, of which 979 are `current` and 5 `pending`. There are no
superseded or obsolete notes, so a "what is currently true" class cannot be derived from supersession.

So the class is built from **pairs**: two notes that describe the same subject but declare different
freshness. Same subject is measured, not assumed — their descriptions must share at least three rare words (a
word appearing in more than six descriptions carries no subject information). The query is those shared words;
the expected answer is the durable note (stable/evergreen/slow), the rival is the other.

This is a *policy* probe, not ground truth: it asserts the durable note answers better, which is exactly the
claim P1-5 makes. Its size is its confidence — 26 pairs, small because the vault's descriptions rarely overlap.

## Baseline, measured against the live engine

```
pairs                       26
hit@1                    0.500
hit@5                    1.000
MRR                      0.737
transient beats durable     13 of 26
```

Read it precisely: both notes are almost always in the top five (hit@5 1.0), so retrieval works and this is
purely an ordering question. **In half the pairs the note that declares itself transient is ranked above the
durable one.** For a knowledge base whose notes carry a lifecycle on purpose, that is the defect the prior
was invented for, and now it is a number.

## What it does not settle

The prior itself cannot be measured without a daemon swap: the ranking lives in the engine binary the vault
is served by, and `swap-engine.sh` plus `docs/internal/engine-swap-2026-11-16.md` is the only procedure that changes it
without degrading serving. Before that swap, the decision has to be stated as a trade: the earlier
measurement had the prior cost 1.9 points of topical hit@1 on the 60-query set. The question this class makes
answerable is whether it buys more than that on preference — and whether a gentler curve (a multiplier per
declared class, not a binary durable/not) keeps the topical cost near zero.

Pick the coefficient from that comparison, not from taste, and bless the golden `tb_search` capture in the same
commit.

## The swap that would answer this, written before it is run

The decision rule is stated now, so the measurement cannot be read backwards into a conclusion later.

1. **Branch and build.** Implement the prior on `fork/freshness-prior` as a per-declared-class multiplier on
   the BM25 score in `search.rs` (not a binary durable/not): `volatile < slow < stable < evergreen`, with
   `evergreen = 1.0` and the others stepping down by a coefficient `c`. Start at `c = 0.05` per step, the
   gentlest curve that can move an ordering.
2. **Measure offline first.** `tools/ranking/eval.py` and `tools/ranking/durable_probe.py` both read the live
   engine, so neither can score an unbuilt change. Use the swap procedure (`tb-engine/scripts/swap-engine.sh`,
   `docs/internal/engine-swap-2026-11-16.md`) on a **copied vault** with its own socket and HOME, and run both probes
   against that socket. Nothing is decided on the production daemon.
3. **Accept only if both hold.** Durable probe hit@1 rises above 0.50 by more than the noise of 26 pairs
   (one pair is 0.038, so the bar is **≥ 0.58**, i.e. at least two pairs flipping), **and** the 60-query
   labelled set loses less than 1.0 point of topical hit@1 (the earlier binary prior cost 1.9, which is why
   the curve is gentler). If only one holds, the coefficient moves once and the test repeats; if the second
   attempt fails, the prior is rejected again and this document records the number.
4. **Bless on the same commit** if accepted: the golden `tb_search` capture, plus a note in `docs/verification.md`
   with both probe results and the coefficient. A rejected prior leaves `search.rs` untouched and the document
   gains the measurement instead of the code.

What makes this different from the first attempt: the first had no class that could show a gain, so it was
judged only by what it cost. This one is judged by both sides.

## The prior, measured twice, and rejected again — with a second reason

Run on a candidate engine over a copy of the vault, with the decision rule from the section above applied as
written.

| class | shipped baseline | prior, `c = 0.10` | prior, `c = 0.02` |
|---|---:|---:|---:|
| literal hit@1 | **0.714** | 0.571 | 0.571 |
| topical hit@1 | **0.660** | 0.660 | 0.642 |
| durable hit@1 | 0.538 | **0.654** | 0.538 |
| transient ahead of durable | 12 / 26 | **8 / 26** | 12 / 26 |

At `c = 0.10` the prior does exactly what it was built to do — eight of 26 durable pairs stop losing to their
transient rival — and it costs the phrase class everything: literal hit@1 falls from 0.714 to 0.571, which is the
value *before* proximity shipped. At `c = 0.02` it changes nothing at all, and literal still falls to 0.571.

That second column is the finding, and it is not about the coefficient. A prior small enough to be neutral is
still enough to disturb the phrase signal, which means the two are not independent multipliers on this corpus:
the durable class is built from descriptions, the phrase class from bodies, and re-weighting by lifecycle moves
documents across the boundary where adjacency tie-breaks. Two attempts, both rejected by the rule stated before
the run; `search.rs` is unchanged and the shipped numbers are the baseline column.

What would make a third attempt worth running: a signal that separates the two classes before scoring rather
than re-weighting after it (a lifecycle facet in the index, queried rather than multiplied), or a durable class
built from bodies instead of descriptions so the two tests stop sharing their evidence. Neither is a constant.
