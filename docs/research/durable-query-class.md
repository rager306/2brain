
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

Pick the coefficient from that comparison, not from taste, and bless the golden `au_search` capture in the same
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
4. **Bless on the same commit** if accepted: the golden `au_search` capture, plus a note in `docs/verification.md`
   with both probe results and the coefficient. A rejected prior leaves `search.rs` untouched and the document
   gains the measurement instead of the code.

What makes this different from the first attempt: the first had no class that could show a gain, so it was
judged only by what it cost. This one is judged by both sides.
