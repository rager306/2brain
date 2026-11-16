# Judge (Subq) capabilities — measured, 2026-11-16

The judge is a small calibrated classifier behind `POST /v1/systemone`: you send `state` (the material)
and `questions` (an object keyed by question id), it answers each question with a `noul` probability, a
`choice`, or a `score`. This document records what it can be asked, what it costs, and where it must not
be used. Numbers come from `tools/judgement/batch_probe.py` on 2026-11-16 unless marked otherwise.

## Batch behaviour — the fact that decides the architecture

One request, N independent questions over one shared `state`:

| questions | state | wall time | per question | answered |
|---:|---:|---:|---:|---|
| 1 | 277 B | 0.42 s | 424 ms | 1/1 |
| 5 | 1.4 KB | 0.37 s | 74 ms | 5/5 |
| 20 | 5.6 KB | 0.44 s | 22 ms | 20/20 |
| 50 | 14 KB | 0.52 s | 10.3 ms | 50/50 |
| 100 | 28 KB | 0.76 s | 7.6 ms | 100/100 |
| 150 | 42 KB | — | — | **HTTP 422** |

Latency is **flat in the number of questions**: 50 questions in one call cost 0.52 s, while 50 separate
calls cost ~21 s. The ceiling sits between 100 and 150 questions per request (422 above it). So a judged
pass over the whole vault (1 036 notes) is **11 requests of 100**, about 9 s, not 1 036 requests.

Practical rule: **batch by 100 questions, one shared `state`, one question id per item.** Passages that
must not leak into each other still share a `state`, so the material has to be separable by the model
(one note per block, delimited) — do not batch items whose answers depend on each other.

## Question shapes

| shape | request | answer | calibrated? |
|---|---|---|---|
| `noul` | `criteria: {true, false}` | `probability` + `confidence` | yes — it is a probability, so a threshold is a decision surface, not a coin flip |
| `choice` | `criteria: {option: description-or-null}` | one option | yes, per option; the escape (`other`/`none`) is what makes it usable as a router |
| `score` | `criteria: [levels...]` | level | yes, but our one experiment (staleness, 5 levels) used only 1 of 5 levels |

Rules that our two experiments actually established (`docs/internal/w4.1-judgement-run-2026-11-16.md`):

1. **Polarity is the first failure.** A question phrased "is this stale?" scored 0.116 accuracy until it
   was inverted to "does the description still match the body?" (0.884). Write the question so that
   *true* means *the good state*, and check one known-positive and one known-negative before trusting a run.
2. **`criteria` carries the meaning.** The same question with vague criteria gave 0.729 on type assignment;
   the neighbouring-class confusion was the model's, not the criteria's — so type assignment needs either
   more context or a deterministic check, not a louder instruction.
3. **Per-note framing collapses on multi-claim notes.** Staleness measured 0.884 accuracy / 0.333 recall
   per note: a note with one stale and three fresh claims reads as "fresh". The unit of a claim-level
   question must be the claim (`per-claim`), which needs an extractor that produces claims first.
4. **Fewer options, better routing.** A 5-way staleness choice used one option for every note; the model
   retreats to the safest label when the options are close. Prefer a `noul` gate followed by a small
   `choice` among the survivors.
5. **Cost is not the constraint, calls are.** At $0.04/1M tokens a 100-question batch over the vault costs
   fractions of a cent; what costs is the round trip, which batching removes.

## Scenario inventory

Ranked by value × fitness. "Batch" is the number of questions per request; "gate" is what makes a wrong
answer cheap.

| scenario | shape | batch | gate | why the judge and not code |
|---|---|---:|---|---|
| duplicate detection | `noul` "same claim?" | 100 pairs | deterministic prefilter (shingle Jaccard ≥ 0.5) keeps the pair set small | 1.000/0.889 measured on 50 labelled notes; string similarity cannot see paraphrase |
| contradiction between two notes | `noul` "can both be true?" | 100 pairs | only between notes sharing a topic or a type | graph gives the candidate pairs, the judge decides |
| type assignment for new notes | `choice` over 9 types | 100 | `tb_validate_value` must accept the result | a wrong type is caught by the contract, so the judge only proposes |
| staleness of a description | `noul` inverted | 100 | per-claim extraction first | measured 0.884 accuracy, recall 0.333 per note — usable as triage, not as truth |
| link suggestion | `choice` over candidate targets | 50 | candidates from `tb_neighborhood`/BM25 | only the judge can tell "related" from "referenced" |
| tag/keyword extraction | `choice` per facet, or `noul` per candidate tag | 100 | vocabulary is the type system's | keeps the vocabulary closed |
| answer selection ("which of these k notes answers this") | `choice` over k | 50 | k comes from `tb_search` | reranker on top of BM25, not a replacement |
| rename/migration safety ("does any note depend on this name?") | **not a judge question** | — | `tb_grep` + `tb_references` | determinism wins: a missed dependency is a broken vault |
| diagnostics triage | `noul` "is this a real defect?" | 100 | the diagnostic code | cheaper than reading 300 findings |

## Where the judge must not be used

- **Anything a graph or string operation answers**: who links here, what type is this, does this path
  exist, is this hash current. A model is a sampler; the engine is a function.
- **The write path.** The judge may decide *whether to propose*, never *what lands*: content goes through
  `tb_validate_value` and the engine's gate. A judge answer is a predicate, not an author.
- **Renames, migrations, deletions.** A probabilistic "seems safe" is the wrong instrument for an
  irreversible act; the gates for those are `tb_grep`, `tb_references` and a dirty-tree check.
- **Per-note claim questions on multi-claim notes** until an extractor exists (rule 3).

## Open measurements

- The 422 boundary is known only as "between 100 and 150"; the error body was not read, so whether it is a
  question-count cap or a payload cap is **not determined**. Read the body before relying on 100.
- Calibration on our own corpus exists only for the three W4.1 questions. Any new question needs a
  labelled sample of 30–50 before it may gate anything.
