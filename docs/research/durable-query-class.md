
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
is served by, and `swap-engine.sh` plus `docs/engine-swap-2026-11-16.md` is the only procedure that changes it
without degrading serving. Before that swap, the decision has to be stated as a trade: the earlier
measurement had the prior cost 1.9 points of topical hit@1 on the 60-query set. The question this class makes
answerable is whether it buys more than that on preference — and whether a gentler curve (a multiplier per
declared class, not a binary durable/not) keeps the topical cost near zero.

Pick the coefficient from that comparison, not from taste, and bless the golden `au_search` capture in the same
commit.
