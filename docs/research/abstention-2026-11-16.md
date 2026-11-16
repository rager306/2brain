# Abstention signal from existing data — 2026-11-16

**Verdict: ship nothing.** No one- or two-feature rule over the captured top-20 data separates the
8 unanswerable task queries from the 34 answerable ones at the agreed bar (catch ≥ 6/8 while
false-refusing ≤ 3/34). The only in-sample rule that touches the bar has zero margin and drops to
**0/8 catch under leave-one-out**. `confidence: answered | partial | no_evidence` was therefore
**not** implemented: `tb-engine/crates/tb-engine/src/serve.rs` (`search_view` / `SearchAnswer`),
`tb-engine/crates/tb-engine/WIRE.md`, the `tb_search` description, and `tools/ranking/` are all
unchanged. A false `no_evidence` on a real answer is a worse failure than today's silence.

## Data and labels

- The 42 hand-written task queries of `docs/research/semantic-judgements-2026-11-16.json`, joined
  row-for-row with a top-20 search capture (`base20.json`: per-hit `path, score, terms, weak`, plus
  `total`) taken in the same run. The capture file was ephemeral; the judgement file is in-repo and
  the capture is regenerable from it.
- Labels come from `expect`: `none` = 8 unanswerable (classes `absent`×7, `absent-ru`×1;
  T10–T12, MS–MW), `hit`/`any` = 34 answerable.
- The judged per-hit levels (answers / partially / mentions / wrong) are ground truth, **not
  features** — they are not observable at serving time. Only serving-time observables were used.
- Confirmed from the same rows: top-1 answers only 16/42 (38%); all 8 `none` rows have a wrong top
  hit; the existing `weak` flag is not discriminative (1.79 weak hits in the top-20 on answerable
  vs 1.88 on unanswerable queries).

## Feature table

`overlap` is the empirical overlap coefficient of the two class distributions (0 = disjoint,
1 = identical). `AUC` is P(random unanswerable sits **lower** on the feature than a random
answerable), so > 0.5 means unanswerables tend lower, < 0.5 means they tend higher.

| feature | mean answerable | mean unanswerable | overlap | AUC |
|---|---:|---:|---:|---:|
| `total` (candidate count) | 376.8 | 327.8 | 0.55 | 0.62 |
| top-1 score | 81.0 | 41.8 | 0.72 | 0.79 |
| top-1 matched terms | 5.41 | 3.88 | 0.44 | 0.72 |
| score gap hit 1 → 2 | 26.6 | 7.1 | 0.85 | 0.65 |
| gap / top score | 0.240 | 0.159 | 0.66 | 0.61 |
| share of query terms in ANY top-20 path | 0.50 | 0.27 | 0.50 | 0.70 |
| query-term overlap with top-1 path | 0.25 | 0.03 | 0.75 | 0.70 |
| query-term overlap with top-5 paths | 0.13 | 0.04 | 0.79 | 0.74 |
| mean score, top-20 | 35.3 | 24.1 | 0.60 | 0.71 |
| min score, top-20 | 23.6 | 17.6 | 0.56 | 0.68 |
| noise share of top-20 (service-file dirs) | 0.14 | 0.22 | 0.70 | 0.39 (higher on unanswerable) |
| `weak` hits in top-20 | 1.79 | 1.88 | 0.87 | 0.39 |

Boolean: top-1 is a service file 6% vs 12%; top-3 contains a service file 15% vs 50% — not
separating either. Every overlap is ≥ 0.44 and no AUC passes 0.79: no single feature separates.

Matched terms agree with the earlier reading: mean top-1 terms 4.41 on the 17 wrong-top-hit rows
vs 5.75 on the 16 answered rows — a visible lean with heavy overlap, i.e. not a discriminator.

## Rules

- **Single feature:** no threshold on any single feature meets the bar even in-sample.
- **Two features:** exactly one conjunction meets the bar in-sample, with zero margin:
  `no_evidence` iff `top_score < 53.6` **and** `ov_top5 < 0.057`. It catches 6/8 (misses T12, MW)
  and false-refuses exactly T03, T13, T18 — the Russian paraphrase, the word-form query, and the
  bilingual query, i.e. three real answers whose hits live on English paths.
- **Three-feature conjunctions:** even in-sample, nothing beats (6 catch, 3 refuse). The
  zero-margin pair rule is the ceiling for conjunctions of these observables.

**Leave-one-out** (thresholds refit on the other 41 rows per fold, same search space):

| rule space | catch | false refusals | folds where no rule fit the bar |
|---|---|---|---|
| single feature | **0 / 8** | 0 / 34 | 42 / 42 |
| two features | **0 / 8** | 3 / 34 | 6 / 42 |

Caveat, stated plainly: everything above is fitted and evaluated on the same 42 rows. Leave-one-out
is the only honest number here, and it is 0/8.

## Why it fails

1. **Absolute scores do not transfer across queries.** Scoring is query-dependent, so a global
   `top_score` threshold is meaningless. All 8 unanswerable top scores lie in [25.5, 53.6]; 8
   answerable queries (T01, T03, T13, T15, T16, T18, MQ, MR) share that exact band.
2. **The language confound.** The observable signature of "the asked-about entity is absent" —
   moderate score, near-zero query-term overlap with hit paths — is identical to the signature of
   "right note, wrong language surface". T03, T13, T18 are answerable but their Russian /
   reworded queries share no surface terms with English path slugs, so any rule that catches the
   unanswerables refuses precisely these three real answers. The rule's only viable in-sample
   shape trades 3 real answers for 6 abstentions; under LOO it trades everything for nothing.
3. **n = 42 with 8 positives** leaves no margin; LOO turns the zero margin into zero catch.

## What this does not claim

- Not that abstention is impossible — only that it is not derivable from the observables captured
  here (counts, scores, term counts, path-term overlap, noise flags) with a small threshold rule on
  42 rows.
- Not that the ranking is fine: top-1 answers only 38%, and the base still asserts hits on
  questions it cannot answer. Both remain open problems, separate from this signal.
- The `absent` acceptance gate (`3/3` in `tools/ranking/tasks_eval.py`, every other class
  unchanged) was never run against a candidate, because no candidate met the analysis bar.

## What a viable signal would need

- A query-normalized confidence (score relative to the query's own term-weight ceiling), not an
  absolute score cut.
- Content-space, cross-lingual-aware coverage instead of path-slug overlap, so a Russian query
  about English material does not read as "absent".
- More labelled absent questions than 8, collected for the purpose, before a wire-visible field
  like `confidence` is pinned in `WIRE.md`.
