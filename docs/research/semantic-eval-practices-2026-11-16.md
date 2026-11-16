# Retrieval evaluation practice: what to take, what to refuse

*Research note, 2026-11-16. Scope: how the field measures retrieval quality and answer quality in 2026, and what
changes in `tools/ranking/` and `docs/research/task-set.md` follow from it. No engine change, no vault write.*

The question this answers: our base is 1 032 typed notes served over MCP. Our ranking evidence is eighteen
hand-written tasks scored pass/fail, plus a paired bootstrap that says one flipped task is invisible. We want
(a) a metric that moves smoothly, (b) many more questions without fooling ourselves, (c) a judgement of
*semantically sufficient* rather than path-matched.

Two things must be said before the list, because they decide how the list is read.

**The formulas are old and settled; the practice is not.** nDCG dates to 2002 and the reciprocal-rank measures to the late 1990s, and the reference
implementations have not changed meaning. What changed after 2023 is the *judge*: relevance judgements and answer
grading are now produced by models, and the 2024–2026 literature is largely about how that goes wrong. So the
ranked list below mixes two kinds of item on purpose — metrics that are cheap and safe, and judge protocols that are
expensive and dangerous — and ranks each by value per unit of risk, not by novelty.

**A framework names a metric; it does not define it.** Where the definition is a formula, I read the implementation,
not the marketing page. Where a framework's own docs could not answer, this note says so instead of paraphrasing
from memory. The gaps are listed in §0.3; two of them are load-bearing for the recommendation — BEIR's generator ships no quality
filter (F2), and BEIR's metric definition lives in another project's source (F1).

---

## 0. What I read, and what it refused to say

### 0.1 Primary sources (implementation or formula)

| # | source | what it actually gave |
|---|---|---|
| S1 | `https://raw.githubusercontent.com/usnistgov/trec_eval/master/m_ndcg_cut.c` | the nDCG@k arithmetic, in code: gain is the qrels relevance value, `sum += gain / log2(i + 2)` for a 0-based `i`, ideal DCG computed at the same cutoff, default cutoffs `5,10,15,20,30,100,200,500,1000`. Its own doc string: *"Gain values are the relevance values in the qrels file. For now, if you want different gains, change the qrels file appropriately."* |
| S2 | `https://raw.githubusercontent.com/usnistgov/trec_eval/master/m_ndcg.c` | the uncut `ndcg`: same gain and discount, over the whole retrieved list, gains overridable per relevance level on the command line. |
| S3 | `https://ir-measur.es/en/latest/measures.html` | the parameter surface of each measure across five backends (`pytrec_eval`, `gdeval`, `ranx`, `cwl_eval`, `trectools`): `nDCG` takes `dcg='log2'` (default) or `'exp-log2'`, plus `gains` and `judged_only`; `RR`; `AP`; and the naming trap quoted in §3.3. |
| S4 | `https://raw.githubusercontent.com/run-llama/llama_index/main/llama-index-core/llama_index/core/evaluation/retrieval/metrics.py` | llama_index's exact code: `HitRate` binary by default; `MRR` first-hit by default; `Precision` divides by `len(retrieved_set)`, not by `k`; `discounted_gain` implements both `rel/log2(i+1)` and `(2**rel - 1)/log2(i+1)`; and `NDCG` carries the docstring *"Currently only supports binary relevance (rel=1 if document is in expected_ids, otherwise rel=0) since we assume that expected_ids is unordered"*, with `mode="linear"` as default. |
| S5 | `https://docs.ragas.io/en/stable/concepts/metrics/available_metrics/faithfulness/` | the Faithfulness formula, plus `FaithfulnesswithHHEM`: a local T5 classifier (Vectara HHEM-2.1-Open) replacing the LLM in the entailment step, described as "free, small, and open-source... very efficient in production use cases". |
| S6 | `https://docs.ragas.io/en/stable/concepts/metrics/available_metrics/context_recall/` | the Context Recall formula, and the existence of non-LLM and *ID-based* variants of it. |
| S7 | `https://docs.ragas.io/en/stable/concepts/metrics/available_metrics/context_precision/` | the Context Precision formula, which is average precision over binary usefulness verdicts. |
| S8 | `https://docs.ragas.io/en/stable/concepts/metrics/available_metrics/answer_relevance/` | Answer Relevancy: the mean cosine between the input question and three questions generated *from the answer*; the docs' own caveat that the score is "not guaranteed" inside [0,1]. |
| S9 | `https://amenra.github.io/ranx/compare/` | `compare()`: `stat_test` of `student` (default, two-sided paired t), `fisher` (randomization, 1000 permutations) or `Tukey`; `max_p` default 0.01; `make_comparable` adds empty results for missing queries and drops queries absent from the qrels. |
| S10 | `https://people.eng.unimelb.edu.au/jzobel/fulltext/cikm08.pdf` | Webber, Moffat & Zobel, *Statistical Power in Retrieval Experimentation*, CIKM 2008: the power formula quoted in §2.2, the 164-topic result, and the shallow-vs-deep finding. |
| S11 | `https://ar5iv.labs.arxiv.org/html/2305.14627` | ALCE: the operational definitions of citation recall and citation precision, the NLI model used, the claim-recall metric for correctness, and §3.4 "robust to shortcut cases". |

### 0.2 Primary sources (agreement, bias, economics of judging)

| # | source | what it actually gave |
|---|---|---|
| S12 | `https://arxiv.org/abs/2409.15133` (Soboroff; *Information Retrieval Research* 1(1), Mar 2025, 29–46) | "The bottom-line-up-front message is, don't use LLMs to create relevance judgments for TREC-style evaluations." |
| S13 | `https://arxiv.org/abs/2412.17156` (Clarke & Dietz, rev. Jan 2026) | a system *"deliberately crafted to exploit automatic evaluation metrics"* that inflates scores without improving retrieval; and simulation of circularity when every system adopts the judge as a final re-ranker. |
| S14 | `https://arxiv.org/html/2502.13908v1` (*Judging the Judges: A Collection of LLM-Generated Relevance Judgements*) | 42 judgement sets from 8 teams on TREC 2023 Deep Learning; the 5 fine-tuned submissions show the highest correlation; open-source judges more stable, proprietary ones more variable; circularity named as a standing concern; and (citing Thomas et al.) that judgement quality *"can vary with simple paraphrases of prompts"*. |
| S15 | `https://arxiv.org/abs/2406.12624` (*Judging the Judges: ... Vulnerabilities*) | 13 judges, 9 exam-taker models: "only the best (and largest) models achieve reasonable alignment with humans", still behind inter-human agreement, scores off by up to 5 points. |
| S16 | `https://arxiv.org/abs/2306.05685` (MT-Bench / Chatbot Arena) | the named biases: position, verbosity, self-enhancement, plus "limited reasoning ability". |
| S17 | `https://aclanthology.org/2025.ijcnlp-long.18.pdf` (IJCNLP 2025, position bias) | 15 judges, ~40 solution models, >150 000 instances: position bias "is not due to random chance" and varies by judge and task. |
| S18 | `https://arxiv.org/abs/2404.13076` (*LLM Evaluators Recognize and Favor Their Own Generations*) | self-preference: an evaluator scores its own outputs higher while humans call them equal; and a causal link to self-recognition, shown by fine-tuning. |
| S19 | `https://arxiv.org/abs/2404.18796` (PoLL) | a panel of *smaller* models from disjoint families beats a single large judge on human correlation, with less intra-model bias, at "over seven times" lower cost. |
| S20 | `https://arxiv.org/abs/2311.09476` (ARES, NAACL 2024) | fine-tuned lightweight judges plus *"a small set of human-annotated datapoints for prediction-powered inference (PPI)"*; accurate with "only a few hundred human annotations". |
| S21 | `https://arxiv.org/abs/2405.04727` (snippet read) | unjudged documents are scored as non-relevant and "these missing judgments may inadvertently introduce biases into the evaluation as their prevalence for a retrieval model is heavily contingent on the pooling process". |
| S22 | `https://arxiv.org/abs/2405.07767` (SIGIR 2024, synthetic test collections) | fully synthetic collections "can reliably be used for retrieval evaluation"; and their diagnosis that GPT-4 *under*-labels the top grade — for documents humans called "Perfectly relevant" it produced the same judgement in 28% of cases. |
| S23 | `https://arxiv.org/abs/2504.20879` (*The Leaderboard Illusion*) | selective disclosure and multiple private trials distort a public leaderboard, ending in "overfitting to Arena-specific dynamics rather than general model quality". |
| S24 | `https://github.com/shmsw25/FactScore` + `https://arxiv.org/abs/2305.14251` (snippet read) | FActScore = share of atomic facts supported by a knowledge source; `# facts` reported beside it is factual recall. |

### 0.3 Framework docs, read through DeepWiki — including where they were silent

DeepWiki answers from the repository's own documentation. It is useful precisely because it will say *"the supplied
context does not include that"*. Those refusals are recorded here, because a claim I could not verify is a claim this
note does not make.

| id | target | what it gave | what it refused |
|---|---|---|---|
| F1 | `beir-cellar/beir`, *3.2 Evaluation Framework* | `EvaluateRetrieval.evaluate()` delegates nDCG, MAP, recall and precision to `pytrec_eval`, averages per query, rounds to 5 decimals; `k_values` default `[1, 3, 5, 10, 100, 1000]`; MRR implemented locally as mean of `1/r` over qrels with rel > 0; `ignore_identical_ids=True` by default. | the nDCG gain/discount itself — *"implemented by the external `pytrec_eval` dependency, not shown in the provided code"*. That is why §3 resolves it from `trec_eval` directly (S1). |
| F2 | `beir-cellar/beir`, *7.2 Query Generation* | `QGenModel` wraps `AutoModelForSeq2SeqLM`, samples with `top_p=0.95`, `top_k=25`, `max_length=64`; dedups generated strings *per passage* with a `set`; writes a qrel of 1 for the source passage. | any BM25 threshold filter, any query-to-passage similarity filter — *"there is no BM25 scoring or threshold check in the shown `QueryGenerator` flow"*; and no documented statement of generated-query bias. |
| F3 | `explodinggradients/ragas`, *3.2 RAG Evaluation Metrics* | the code flow for Answer Relevancy (embed, cosine, noncommittal gate), Context Precision (average precision with a `1e-10` guard), Noise Sensitivity (`relevant`/`irrelevant` modes). | the faithfulness and context-recall implementations — *"not present in the provided code context"*. Resolved from the docs instead (S5–S7). |
| F4 | `explodinggradients/ragas`, *3.6 Metric training, alignment & optimization* | `judge_alignment` compares judge verdicts to human pass/fail; `DiscreteMetric.get_correlation` computes **Cohen's kappa**; `DSPyOptimizer`/MIPROv2 tunes metric prompts; guidance of 20–50+ annotated samples, 50+ diverse ones for production, and validation on a holdout. | any general reliability guarantee — *"the docs present human agreement as an alignment target ... they do not provide a general reliability guarantee for LLM-based metrics."* |
| F5 | `explodinggradients/ragas`, *3.4 / limitations* | LLM metrics "can be somewhat non-deterministic on repeated identical inputs"; `AspectCritic` collects three verdicts and takes a majority; rubrics carry a description per score level (typically 1–5), domain-wide or per-instance. | judge self-bias — *"the provided documentation does not discuss judge self-bias"*. |
| F6 | `explodinggradients/ragas`, *TestsetGenerator* | knowledge-graph transforms (summaries, themes, entities, embeddings, chunking); single-hop and multi-hop synthesizers; persona/style/length variation. | any question-quality filter — *"the provided files do not document a procedure for validating generated questions for correctness, usefulness, or other quality criteria."* |
| F7 | `promptfoo/promptfoo`, *2.3 Assertions and Grading* | assertion `weight` defaults to 1 and a test-level `threshold` applies to the weighted average; `llm-rubric` passes only if the grader's `pass` is true **and** the score meets the threshold, so without a threshold `pass: true, score: 0` still passes; `g-eval` threshold defaults to 0.7; `--repeat 3` reruns each test; the grader provider can be pinned separately from the target and *"takes precedence"*; candidate output should be treated as untrusted input. | a claim that any judge is reliable — the docs say to calibrate against labelled examples first. |
| F8 | `run-llama/llama_index`, *3.7 Evaluation Framework* | only that `RetrieverEvaluator`, `HitRate`, `MRR` and the faithfulness/relevancy/correctness evaluators exist as exports. | the implementations — *"the provided code also does not establish whether precision, recall, nDCG, and MAP are implemented"*. Resolved by reading the file itself (S4). |
| F9 | `zilliztech/InPars` | nothing: *"Repository not found. Visit https://deepwiki.com/zilliztech/InPars to index it."* | everything. No claim about InPars-style query filtering is made in this note. |

### 0.4 Our own baseline, read from this tree

| file | what it establishes |
|---|---|
| `docs/research/task-set.md` | eighteen hand-written tasks in eleven classes, including three `absent`; scoring in `tasks_eval.py`; the stated consequence: *"With eighteen tasks, one position is 5.6 percentage points, so this set is a smoke test, not a benchmark"*. |
| `tools/ranking/tasks.json` | the label schema: `evidence` (minimal set), `accept` (equivalent alternatives), `expect`, optional `args` and `min_total`. |
| `tools/ranking/graded.py` | MRR, nDCG@10 and rank distance over the same runs. The code is explicit: gains are binary, labels are a *set* of acceptable paths, *"the ideal ranking is that hit at rank 1, so IDCG = 1"*, and `ndcg10 = 1.0 / log2(rank + 1)`. |
| `tools/ranking/bootstrap.py` | paired bootstrap over per-task ±1 vectors, 2 000 resamples, percentile interval, seeded; prints `resolution` (`1/n`), McNemar, and the names of flipped tasks. |

That last row is the whole problem in one line: `1/log2(r+1)` is a strictly decreasing function of the first
acceptable hit's rank, and so is `1/r`. **Our nDCG@10 and our MRR can never disagree about which run is better** —
the nDCG only re-weights the gaps. It is smooth in the arithmetic sense and empty in the evidentiary sense: it
carries exactly one fact per query, the rank of one note.

---

## 1. What we are actually missing

Three defects, each stated against a source rather than against taste.

**D1 — the metric has one degree of freedom per query.** With binary gains and `IDCG = 1`, a task contributes
`1/log2(rank+1)` and nothing else. A run that puts the *second* necessary note at rank 2 while the first is at rank
1 is scored identically to a run that finds only the first note at rank 1. The `multi-hop` class — and the
`evidence`/`accept` label schema itself — already say that evidence is a set; the metric does not. llama_index has the same limitation and says so in its docstring (S4); trec_eval's answer is
that graded judgements are what fix it (S1).

**D2 — n = 18 cannot see the effect we care about.** Webber et al. give the power of a one-sided paired test as
`P ≈ Φ(√n·δ/σ − z_{1−α})` and report a concrete case: σ = 0.15, δ = 0.033 → **164 topics** for power 0.8 at α = 0.05,
and they note the traditional 50-topic collection is inadequate (S10). Small-δ is the regime a ranking tweak lives
in. Their other finding is the one that changes the plan, not just the arithmetic: *"greater statistical power is
achieved for the same relevance assessment effort by evaluating a large number of topics shallowly than a small
number deeply"* (S10).

**D3 — "did it pass" is path matching, and the pass is applied to the answer.** A task passes when an acceptable
path appears at the rank the task asks for. Nothing checks whether the text next to that path answers the question.
This is exactly what ALCE separates into citation recall (does the cited passage entail the statement) and
correctness/claim recall (does the answer contain the reference's claims), and its §3.4 shows why the split
matters: a system that returns the top-1 passage and cites itself gets *almost perfect* citation scores while its
correctness and fluency collapse (S11). Path matching is a metric a lazy baseline maxes out.

---

## 2. The ranked practices

Ranked by value per unit of risk, where risk means "can this produce a confident wrong answer, or cost more than it
returns". Cost is S (hours), M (a day or two), L (a week or more).

### 1. Grade the evidence, then use nDCG@5 with a real IDCG — **S**

**Mechanism.** Give every judged note a grade: `2` = directly answers (the note a person would quote), `1` = necessary
support (must be read to answer; the second hop, the definition, the constraint), `0` = irrelevant. Then
`nDCG@k = DCG@k / IDCG@k` with linear gain and a log discount (formula in §3.1), and the *ideal* ranking is the
labels sorted, so `IDCG ≠ 1` and the second piece of evidence earns credit. Report `MRR@10` beside it and
`Success@1` for the `absent` class.

**Source.** trec_eval's `ndcg_cut` (S1) is the reference arithmetic; ir-measures (S3) is the portable wrapper and
documents both DCG formulations; BEIR reports nDCG at `[1,3,5,10,100,1000]` (F1). The grade meanings are a
judgement call, but "declare the scale, gain, discount and cutoff before comparing runs" is stated as practice in a
2026 retrieval-evaluation write-up (S25, blog, secondary).

**What it changes here.** `tools/ranking/graded.py` stops computing `1/log2(rank+1)`: `tasks.json` rows grow a grade
per evidence path, `score_task()` builds `DCG` over *all* graded hits in the page and `IDCG` over the sorted grades.
`tasks_eval.py` keeps its pass/fail view unchanged — it answers a different question and its pre-registered
acceptance rules in `task-set.md` still hold.

**Failure prevented.** D1: an improvement that promotes the second necessary note is invisible today and scored
tomorrow. Also prevents the inverse: a run that swaps two equally-good paths being scored as a loss.

### 2. Size the set by a power calculation, and never by "add tasks until significant" — **S to compute, M to label**

**Mechanism.** Before the next change, measure σ of the per-task metric deltas from two existing runs (one line of
Python over two `graded.py --json` outputs), plug into `n = (2.486·σ/δ)²` for 80 % power at α = 0.05 one-sided
(2.486 = 1.644 + 0.842, from S10's formula), and state the smallest δ the current n can certify. Publish that number
beside every result.

**Source.** Webber, Moffat & Zobel (S10). Their warning is the part usually skipped: iteratively adding topics *until
power is achieved* "leads to a bias in favour of finding both power and significance", and estimating σ from a small
trial "leaves wide margins of error" — at 30 trial topics with s = 0.15, a 95 % upper bound of 0.183 calls for 243
topics, not 164.

**What it changes here.** A `--mde` mode in `bootstrap.py` (or a sibling script) that prints the detectable δ for the
observed `n` and σ, so `task-set.md`'s "smoke test, not a benchmark" becomes a number instead of an adjective.
Arithmetic for our n, if σ lands near 0.15/0.30 — the two values the paper's own data brackets:

| n | MDE at σ = 0.15 | MDE at σ = 0.30 |
|---|---|---|
| 18 | 0.088 | 0.176 |
| 40 | 0.059 | 0.118 |
| 100 | 0.037 | 0.075 |
| 150 | 0.030 | 0.061 |

**Failure prevented.** D2, and the specific dishonest version of it: a change that moves two tasks becoming "a gain"
because the interval was never computed before the change was made.

### 3. Compare paired, name the test, report the interval and the effect — **S**

**Mechanism.** Two runs over the same queries give per-query deltas. Report the mean delta, a 95 % interval, the test
used, and the number of queries. `bootstrap.py` already does the percentile bootstrap and McNemar; add the interval
on the *graded* metric (not just the pass count) and print the interval, not only BETTER/WORSE/INCONCLUSIVE.

**Source.** `ranx.compare()` (S9) is the practitioner packaging: paired t by default, Fisher randomization with 1000
permutations, Tukey HSD for many systems, `max_p = 0.01`, and `make_comparable` — which is a correctness feature
disguised as a parameter. `bootstrap.py`'s own docstring already cites the same shape from `ruvector-sota-bench`.

**What it changes here.** `graded.py before.json after.json` currently prints a difference; it should print a paired
interval over the per-task nDCG deltas, reusing `bootstrap.py`'s resampling core.

**Failure prevented.** Comparing two means as if they were independent, and reading a 0.02 difference as a change
when the interval is ±0.14.

### 4. Judge answers by claims against the cited note, not by the presence of the path — **M**

**Mechanism.** Two numbers per answer, both claim-level and both cheap:
- **citation recall** = (statements entailed by their cited note) / (statements), entailment by an NLI model;
- **claim recall** = (reference claims entailed by the answer) / (reference claims), the reference claims being the
  atomic facts of the note's relevant blocks.

Report both. A statement with no citation is a recall failure, not a neutral event.

**Source.** ALCE §3.3 defines citation recall as `1` for a statement iff it has at least one citation `C` and
`NLI(C, s) = 1`, averaged over statements; citation precision is defined per citation, and deliberately *not* as
"minimal citation set", because human writing cites redundantly (S11). FActScore is the same idea in the
factuality literature (S24). Ragas Faithfulness and Context Recall are the same two ratios with the reference
swapped for context — the docs state both formulas explicitly (S5, S6) — and `FaithfulnesswithHHEM` shows the
entailment step can run on a local T5 classifier instead of an LLM (S5), which matters for us: evaluation-time NLI
does not imply embeddings on the serving path.

**What it changes here.** A new `tools/ranking/sufficiency.py`: for each task, take the top-note blocks the task
names, ask the agent's answer (or a fixed answering prompt) for an answer with citations, decompose into claims,
score with a local NLI model. Output per task: `citation_recall`, `claim_recall`, plus the failing statements
quoted. It does **not** feed back into `tasks_eval.py`; it is a second, orthogonal view.

**Failure prevented.** D3: an answer that names the right note and states the wrong thing passes today.

### 5. Audit your own metric for a shortcut before trusting a gain — **S**

**Mechanism.** Write down the degenerate system that maximises the metric, then score it. For a path metric that is
"return the top-1 note and pass": almost always near-perfect. Fix by reporting the orthogonal numbers (correctness,
coverage, abstention) in the same table, and by adding tasks the shortcut cannot satisfy — multi-note questions where
one note is insufficient.

**Source.** ALCE §3.4 names the two shortcut cases explicitly — the top-1 passage as the response citing itself, and
its first two sentences — and observes both have almost-perfect citation scores, which is why the benchmark reports
correctness and fluency alongside (S11). Clarke & Dietz go further and *submit* such a system to a shared task,
showing it "can achieve artificially inflated scores without truly improving retrieval quality" (S13). The
`Leaderboard Illusion` is the same phenomenon at leaderboard scale (S23).

**What it changes here.** One row in every results table: "degenerate baseline (top-1 note, self-cited)". It costs a
line of code and permanently caps how much a path score can impress.

**Failure prevented.** A metric that only a lucky baseline can win, adopted as progress.

### 6. Calibrate the judge against human labels and publish Cohen's kappa — **M**

**Mechanism.** Label 50 answers by hand against the same criterion the judge applies — pass/fail, or the 1–5 rubric
of §2.12; kappa is defined for both. Run the judge on the same 50. Report Cohen's kappa, not raw agreement, and
re-measure when the judge prompt, model, or criterion changes. Do not ship a judge whose kappa against the human
slice is unmeasured.

**Source.** Ragas ships exactly this loop: `judge_alignment` compares judge verdicts to human labels, and
`DiscreteMetric.get_correlation` computes **Cohen's kappa**; the same docs recommend 20–50+ annotated samples, 50+
diverse ones for production, and holdout validation (F4). The 2025 LLMJudge collection reached the same
conclusion empirically across 42 judgement sets: the five fine-tuned submissions showed the highest correlation with
human judgements, and model choice moved agreement a lot (S14). *Judging the Judges* found only the largest models
reached "reasonable" alignment, and still short of inter-human agreement (S15).

**What it changes here.** `docs/research/task-set.md` gains a labelled slice and a kappa column; the judge prompt is
pinned in-tree like the tasks are.

**Failure prevented.** The most common mistake in the field (§4.1): a judge adopted because its outputs look
plausible, with nobody having measured whether it agrees with a person.

### 7. Separate the judge from the generator, and use more than one judge — **S**

**Mechanism.** The model that answers must not be the model that grades. Pin the grader explicitly. Where the
stakes justify it, use three small judges from different families and take the majority (or the mean), rather than
one large judge. Randomise or swap the order of compared answers.

**Source.** promptfoo makes the separation a first-class setting and says the explicit judge "takes precedence" over
the target's own provider (F7). PoLL: a panel of smaller models from disjoint families beats a single large
judge, with less intra-model bias, at over seven times lower cost (S19). Self-preference is real and *causal*, not
decorative: LLM evaluators favour their own generations, and the effect tracks their ability to recognise them, as
shown by fine-tuning (S18). Position bias is not noise: 15 judges, 150 000+ instances, and it "is not due to random
chance" (S17, S16).

**What it changes here.** Any judged number in our reports carries the judge's identity and family; a single-model
judge is a documented exception, not the default.

**Failure prevented.** A measured "improvement" that is the answering model agreeing with itself.

### 8. Fill the judgement holes instead of scoring them zero — **M**

**Mechanism.** Assigning every retrieved-but-unjudged note the grade `0` measures the pooling process, not the
ranker. Either restrict scoring to judged notes and say so (`judged_only`), or judge the holes — a model with a
calibrated kappa (§2.6) is enough for the low grades.

**Source.** The problem statement: unjudged documents score as non-relevant and their prevalence "is heavily
contingent on the pooling process" (S21). The hygiene flags exist in the tooling: `judged_only` across measures
(S3) and `make_comparable` in ranx (S9). Clarke & Dietz's circularity analysis is the extreme case of the same
mechanism (S13).

**What it changes here.** A new ranker will surface notes the eighteen tasks never judged; today those are silent
zeros. `graded.py` should print `found`/`expected` *and* the number of unjudged notes in the page, and either abstain
from scoring them or send them to the judge.

**Failure prevented.** The new ranking approach being unable to win, because its wins are in territory the labels
never covered.

### 9. Generate questions in bulk, then filter for diagnosability — **M**

**Mechanism.** Generate from the notes in bulk (a seq2seq or LLM generates a question per note/block), then apply a
filter the frameworks do *not* ship: drop questions that a lexical retriever answers from the note's own heading or
title words, drop near-duplicates, and require each surviving question to name the note it came from. Keep the
generator prompt and the filter versioned with the set.

**Source.** BEIR's `QueryGenerator` is the bulk generator, and its documented behaviour is the warning: it dedups
strings *per passage* and creates a qrel of 1 for the source passage, with **no** BM25 threshold and **no**
query-to-passage similarity filter (F2). Ragas' `TestsetGenerator` is the richer version — knowledge-graph themes,
entities, personas, single-hop and multi-hop synthesizers — and its docs document no question-quality filter either
(F6). The empirical status of synthetic questions *as such* is positive: SIGIR 2024 found fully synthetic
collections "can reliably be used for retrieval evaluation" (S22) — while also finding GPT-4 under-labels the top
grade (28 % agreement on "Perfectly relevant"), so synthetic *judgements* need §2.6 more than synthetic *queries* do.

**What it changes here.** `tools/ranking/build_queries.py` already builds from filenames; the new set needs the
filter, a `source` field per row, and a `class` per row so `absent` and multi-hop do not vanish into the average.

**Failure prevented.** A thousand questions whose answers are the note's own title, which flatter BM25 and teach us
nothing.

### 10. Freeze the harness: questions, labels, corpus snapshot, judge prompt, filter — **S**

**Mechanism.** One directory holds `queries.json`, `labels.json`, `corpus.rev`, `judge.md`, `filter.py`. A result is
reproducible from those five plus the engine build. A held-out slice — 20 % — is never used to choose a variant.

**Source.** The failure this prevents is circularity, named as a standing risk in both relevance-judgement surveys
(S13, S14) and demonstrated at leaderboard scale where multiple private trials plus selective disclosure end in
"overfitting to Arena-specific dynamics" (S23). The mechanism-level version: judgement quality "can vary with simple
paraphrases of prompts" (S14, citing Thomas et al.) — so an unpinned prompt is an unpinned measurement. The
`make_comparable` flag in ranx (S9) is the reproducibility cousin: without it, two runs are scored against
different query sets.

**What it changes here.** `tasks.json` and `labelled-queries.json` already carry a `vault` field and a note on
provenance; extend that to a snapshot revision and a filter version, and mark the held-out rows.

**Failure prevented.** Tuning on the eval set and then reporting the tuning as an improvement.

### 11. Prediction-powered inference: many machine labels, few human labels, valid interval — **L**

**Mechanism.** Score the whole set with a calibrated judge; score a random subset by hand. PPI combines them into an
estimate whose confidence interval is valid under the human labelling, so the machine labels buy variance reduction
without buying bias.

**Source.** ARES (NAACL 2024): fine-tuned lightweight judges plus "a small set of human-annotated datapoints for
prediction-powered inference", accurate "using only a few hundred human annotations" (S20).

**What it changes here.** It is the principled version of §2.9's bulk set: 150 machine-labelled questions with 40 human
labels and an honest interval, instead of 18 human tasks with none.

**Failure prevented.** "We have 400 questions now" as a substitute for validity — and its mirror, staying at n = 18
because human labelling does not scale.

### 12. Rubric-based, level-scored criteria, worded against the field names — **M**

**Mechanism.** Where a judgement is genuinely graded (is this answer sufficient? is the note the *right* note?),
score 1–5 against descriptions of each level, reuse the rubric across samples for one criterion and per-sample for
idiosyncratic ones, and use the schema's own words (`reference`, `evidence`) in the rubric.

**Source.** Ragas rubrics: `DomainSpecificRubrics` reuses one rubric, `InstanceSpecificRubrics` takes criteria per
sample, each level carries a description, and the docs explicitly advise matching rubric terminology to the sample
schema — "using `reference` rather than `ground truth` when the schema calls the field `reference`" (F5).
promptfoo's `g-eval` is the same idea with a chain-of-thought prompt and a default passing threshold of 0.7 (F7).
Both frameworks describe their chain-of-thought rubric scoring as following G-Eval; I did not read the G-Eval paper.

**What it changes here.** §2.4's sufficiency judge should use a 1–5 rubric with written level descriptions rather than
a bare pass/fail, so a partial improvement registers — and so the kappa in §2.6 is computed on a scale that has room
to disagree.

**Failure prevented.** A binary judge at the boundary: two nearly identical answers landing on opposite sides, and
the aggregate moving by 1/18 for no reason.

---

## 3. The formulas, written out

### 3.1 nDCG@k (graded relevance)

For a query `q`, a ranking `d_1 … d_k`, and a grade `g(d) ∈ {0, 1, 2, …}`:

```
DCG@k  = Σ_{i=1..k}  gain(g(d_i)) / log2(i + 1)
IDCG@k = Σ_{i=1..k}  gain(g*_i)   / log2(i + 1)     where g* is the grades sorted descending
nDCG@k = DCG@k / IDCG@k                              (undefined if IDCG@k = 0)
```

with two gain conventions in use:

```
linear:       gain(g) = g
exponential:  gain(g) = 2^g − 1
```

`IDCG@k = 0` happens when no note for that query is judged relevant at all. That query must be **reported**, not
silently dropped: an unanswerable query belongs in an abstention measurement, not in a relevance
average (S25, practitioner note).

**Which convention the reference tools use, precisely.** trec_eval's `ndcg_cut` uses the qrels relevance value as
the gain — *linear* — and `log2(i + 2)` for a 0-based `i`, i.e. `log2(rank + 1)`; the normalisation is by the ideal
DCG *at the same cutoff*, and the default cutoffs are `5,10,15,20,30,100,200,500,1000` (S1). The exponential gain
is the Järvelin–Kekäläinen "cumulated gain" variant and is available in ir-measures as `dcg='exp-log2'` (S3) and in
llama_index as `mode="exponential"` (S4). BEIR reports nDCG at `[1,3,5,10,100,1000]` over `pytrec_eval` (F1), so
a BEIR number is linear-gain unless the qrels were transformed (S1's doc string says exactly that: "if you want
different gains, change the qrels file appropriately").

**Recommendation:** linear gain, `k = 5`, grades 2/1/0. The exponential gain saturates: with two required notes both
graded 2, `2² − 1 = 3` at rank 1 against `1` at rank 2 is a 3:1 pull toward the first note, which hides exactly the
multi-hop failure D1 describes.

### 3.2 MRR@k

```
RR@k = 1 / rank(first relevant note)   if that rank ≤ k
     = 0                                otherwise
MRR@k = (1/|Q|) · Σ_q RR@k(q)
```

trec_eval's `RR` is uncut by default and takes `rel` to decide what counts as relevant (S3); BEIR implements it
locally as the mean of `1/r` over qrels with relevance > 0, dividing by the number of queries (F1);
llama_index's default is the reciprocal rank of the first hit, with an optional "granular" variant that averages
`1/rank` over *all* relevant hits (S4). Three tools, three notions of the denominator — state which one a number came
from.

### 3.3 Recall@k, and the naming trap

```
Recall@k = |{relevant notes} ∩ {top-k retrieved}| / |{relevant notes}|
```

This is the TREC convention, and ir-measures states the collision outright: *"Some tasks define Recall@k as whether
any relevant documents are found in the top k results. This software follows the TREC convention and refers to that
measure as Success@k."* (S3). llama_index's `HitRate` is precisely that `Success@k` — binary by default, with a
"granular" mode that is actually recall (S4). Our `tasks_eval.py` pass/fail rule is `Success@k` under another name.

Two implementation traps found while reading code, both worth checking in whatever we adopt: llama_index's
`Precision` divides by `len(retrieved_set)` rather than by `k`, so a retriever that returns fewer than `k` notes has
its precision inflated (S4); and `judged_only=False` treats unjudged notes as non-relevant, which is §2.8's failure
built into a default (S3).

### 3.4 MAP

```
AP = ( Σ_{i: d_i relevant} P@i ) / |{relevant notes}|
MAP = mean of AP over queries
```

Simple and sharp when relevance is binary; it loses the grade information nDCG keeps. ir-measures documents it as
"the mean of the precision scores at each relevant item returned in a search results list" (S3).

### 3.5 RAG metrics: faithfulness, answer relevance, context precision/recall

Ragas' four, verbatim from the docs (S5–S8):

```
Faithfulness        = |{claims in the response supported by the retrieved context}| / |{claims in the response}|
Context Recall      = |{claims in the reference supported by the retrieved context}| / |{claims in the reference}|
Context Precision@K = Σ_{k=1..K} ( P@k · v_k ) / |{relevant items in the top K}|,  P@k = TP@k / (TP@k + FP@k)
Answer Relevancy    = (1/N) · Σ_{i=1..N} cosine( E(generated question_i), E(input question) )
```

with `v_k ∈ {0,1}` the usefulness verdict at rank `k`, `N = 3` generated questions by default
(`strictness`), and — the docs' own caveat — a score "not guaranteed" inside `[0,1]` because cosine ranges over
`[−1,1]` (S8).

Three observations that matter for us:

1. **Faithfulness and Context Recall differ only in what is decomposed** — the response, or the reference. Both are
   claim-level entailment ratios. Ours would decompose the *answer* and entail against the *note*.
2. **The entailment step need not be an LLM.** `FaithfulnesswithHHEM` uses Vectara's HHEM-2.1-Open, a T5 classifier,
   described as free, small, open-source and efficient in production (S5). ALCE used the TRUE T5-11B NLI model (S11).
   A local cross-encoder is not "embeddings on the serving path"; it runs in the eval harness.
3. **Answer Relevancy is a similarity proxy, not a sufficiency measure** — it embeds questions, not answers, and
   compares them to the *input question*. It cannot detect an answer that is on-topic and wrong. See §4.3.

### 3.6 Citation recall and precision (ALCE)

Per statement `s` with citation set `C_s`, `C = concat(c for c in C_s)`, and an NLI function `m`:

```
citation_recall(s) = 1  iff  C_s ≠ ∅  and  m(C, s) = 1,   else 0
Citation Recall    = mean over statements s
```

Citation precision is defined per citation, and deliberately is *not* "the minimal citation set": a citation `c` is
"irrelevant" iff (a) `m(c, s) = 0` **and** (b) removing `c` does not affect the support from the remaining citations;
then `citation_precision(c) = 1` iff `s` has recall and `c` is not irrelevant, averaged over citations (S11).

The design choice is instructive: precision penalises *useless* citations, not *redundant* ones, because human
writing cites for credibility. Any sufficiency metric of ours that penalises a second supporting note is measuring
tidiness, not truth.

---

## 4. The trap section

### 4.1 An LLM judge adopted without measuring its agreement — the most common mistake

**The trap.** Score answers with a model, watch the number move, ship the number.

**What the literature says.** Soboroff's bottom line, at the LLM4Eval keynote: *"don't use LLMs to create relevance
judgments for TREC-style evaluations"* (S12). Clarke & Dietz demonstrate the mechanism concretely — a system crafted
to exploit automatic metrics inflates its score without improving retrieval, and if every system re-ranks with the
judge, the judge is measuring itself (S13). *Judging the Judges* found that of 13 judges, only the largest reach
"reasonable" alignment with humans, still behind inter-human agreement, with scores off by up to 5 points (S15).
DeepWiki's read of the Ragas docs is the same caution from inside a framework: LLM metrics "can be somewhat
non-deterministic on repeated identical inputs" (F5), and the docs offer no general reliability guarantee (F4).

**The non-trap version.** The same field that says "don't use LLM judgements" ships the antidote: measure Cohen's
kappa against a human slice (F4), and the 2025 LLMJudge collection shows what that measurement looks like across
42 judgement sets, where fine-tuned judges led on correlation and model family moved agreement substantially (S14).
Use the judge, publish the kappa.

### 4.2 A judge scoring its own outputs

**The trap.** One model answers and grades, "to save a call".

**What the literature says.** Self-preference is real and causal: evaluators score their own generations higher
while humans consider them equal, and the bias tracks self-recognition, which fine-tuning can increase or decrease
(S18). MT-Bench names position, verbosity and self-enhancement as the standing biases of the paradigm (S16).
Position bias is measurable and not random across 15 judges and >150 000 instances (S17).

**The non-trap version.** Pin a different grader (promptfoo's explicit judge overrides the target's provider,
F7); use a panel of smaller models from disjoint families — better human correlation, less intra-model bias,
seven times cheaper (S19); swap the order of compared answers.

### 4.3 Semantic similarity as a proxy for correctness

**The trap.** `cosine(answer_embedding, reference_embedding) > θ` as "the answer is right".

**What the literature says.** Ragas' Answer Relevancy is exactly this shape — mean cosine between the input question
and questions generated from the answer — and its own docs concede the range is not bounded as a result (S8). It is
a *topicality* measure: it cannot separate "on-topic and wrong" from "on-topic and right", because the answer's
content never enters the comparison. The alternatives that do work are claim-level entailment (S5, S11, S24) or
human-labelled slices with an interval (S20). ALCE's authors also record the limit of the entailment approach
honestly: their NLI model "cannot detect partial support", which is why they gate recall as a prerequisite for
precision rather than averaging the two (S11).

### 4.4 Generated questions that quote the source

**The trap.** Generate a question from a note's text with the note's own vocabulary intact, then celebrate when BM25
finds the note. The question is a copy of the answer key.

**What the literature says.** BEIR's `QueryGenerator` performs *no* filtering for this — dedup per passage, qrel 1
for the source passage, no BM25 threshold, no query-to-passage similarity check (F2). Ragas' richer
`TestsetGenerator` documents no question-quality filter either (F6). So the filter is not something to adopt from
a framework; it has to be written. The good news is that synthetic test collections *as a construction* were
validated: SIGIR 2024 found fully synthetic collections can be reliable, and specifically tested the worry that they
favour systems built on the generating model, finding "little to no bias" toward LLM-based systems in their
collection (S22). The bad news from the same paper: GPT-4 under-labels the top relevance grade (28 % agreement with
humans on "Perfectly relevant"), so synthetic *judgements* are the weaker half.

### 4.5 A metric whose noise band exceeds its effect

**The trap.** A smooth metric, a small set, and a confident sentence.

**What the literature says.** This is the CIKM 2008 result, and it is arithmetic rather than opinion: at σ = 0.15 and
δ = 0.033, 164 topics are needed for 80 % power; a 50-topic collection is inadequate (S10). It applies to our current
n with a vengeance — at n = 18 the detectable δ at σ = 0.15 is 0.088, five times the effect a ranking tweak usually
produces. And two related traps are named in the same paper: estimating σ from a small trial gives wide bounds
(30 trial topics with s = 0.15 → 243 topics needed), and adding topics iteratively until power is reached biases
toward finding significance (S10).

**The non-trap version.** Compute the MDE before the experiment, publish it with the result, and prefer many shallow
topics to few deep ones — the same assessment effort buys more power (S10).

### 4.6 Counting unjudged documents as irrelevant

**The trap.** The new ranker surfaces a good note the old pool never judged; it scores zero; the new ranker "lost".

**What the literature says.** Missing judgements bias the evaluation, and their prevalence depends on the pooling
process rather than on the system (S21). The tooling offers two honest escapes: score only judged documents and say
so (`judged_only`, S3), or fill the holes (S21's whole thesis). Clarke & Dietz's circularity scenario is the same
mechanism taken to its limit (S13).

### 4.7 Tuning on the set you report

**The trap.** Try variants, keep the best, report its score on the same tasks — with no record of how many variants
were tried.

**What the literature says.** At leaderboard scale, multiple private trials plus selective disclosure produce
"overfitting to Arena-specific dynamics rather than general model quality" (S23). At the level of a single
benchmark, circularity is named as a standing concern precisely because the judge and the ranked system can become
the same object (S13, S14). And judgement quality moves under prompt paraphrase (S14, citing Thomas et al.), so an
unpinned judge prompt is a second, silent degree of freedom.

**The non-trap version.** Freeze the harness (§2.10), hold out a slice, and record every variant tried — including
the rejected ones, which `task-set.md` already does well for its two negative experiments.

### 4.8 One more, specific to us: a graded metric over binary labels

Not a field trap but ours, and worth writing down because the code looks correct. `graded.py` computes
`1/log2(rank+1)` with `IDCG = 1`. That is a smooth number and a monotone function of the same variable as MRR — so it
can never disagree with MRR about which run is better, and it never sees more than one note per query. Calling it
nDCG is accurate about the formula and misleading about the evidence. The fix is graded labels (§2.1), not a
different discount.

---

## 5. Recommendations

### The smallest change that would let us judge a ranking change tomorrow

**Grade the evidence, and score nDCG@5 with a real IDCG — on the same eighteen tasks.**

Concretely: add `grade` to each `evidence`/`accept` entry in `tasks.json` (2 = directly answers, 1 = necessary
support, 0 = irrelevant); make `graded.py` build `DCG@5` over every graded hit in the page and `IDCG@5` from the
sorted grades; print `nDCG@5`, `MRR@10`, `Success@1`, and a paired 95 % interval over the per-task nDCG deltas
between two runs.

Why this is the smallest sufficient change:
- it needs no new judgements — only a second pass over the eighteen tasks we already read, to say which notes are
  support rather than answer;
- it makes the metric answer a question MRR cannot: whether the *second* necessary note moved;
- it costs hours, not days, and it cannot mislead: `tasks_eval.py`'s pass/fail rule and its pre-registered acceptance
  rules are untouched, so the old evidence stays comparable;
- it puts the interval in the same command, so "one task flipped" becomes "Δ nDCG@5 = +0.04, 95 % [−0.01, +0.09]"
  — an honest inconclusive instead of a verdict at the resolution of 1/18.

What it will *not* do, stated plainly: at n = 18 the detectable δ is around 0.09–0.18 depending on σ, so this change
buys expressiveness, not power. It makes small effects *visible*; it does not make them *significant*. That is the
week's work.

### The larger one worth building over a week

**A 150-question frozen set with machine labels, a 40-question human calibration slice, and two orthogonal scores.**

1. **Generate** questions from notes in bulk (BEIR-style generation, F2), then filter: drop questions answerable
   from the note's own heading or title words, drop near-duplicates, keep a `source` note per row (§2.9).
2. **Grade** every judged note 2/1/0, and fill the holes across systems instead of scoring them zero (§2.8).
3. **Score** two things per question: nDCG@5 over the graded notes (ranking), and claim-level citation recall plus
   claim recall against the note's blocks (sufficiency, §2.4), with a local NLI model (S5's HHEM path).
4. **Calibrate**: hand-label 40 of the 150; publish Cohen's kappa for the judge and the filter (§2.6). If kappa is
   low, the machine labels are a diagnostic, not a measurement.
5. **Size**: state the MDE at n = 150 with the measured σ before the first comparison (§2.2). At σ = 0.15 that is
   δ ≈ 0.030 — the regime a ranking tweak lives in; at σ = 0.30, δ ≈ 0.061, which is why the σ measurement comes
   first and the topic count follows it.
6. **Freeze**: queries, grades, corpus revision, judge prompt, filter version in one directory, with a 20 % held-out
   slice never used to choose a variant (§2.10).

The order matters: 1 and 2 are mechanical, 3 is the new capability, 4 is what keeps 3 honest, 5 decides how much of
1 is enough, 6 keeps the whole thing from rotting. If a week is not available, do 3 on the existing eighteen tasks
first — it is the only item that changes what we can *say* about an answer, as opposed to how precisely we can score
a rank.

---

## 6. What this note does not settle

- **σ is unmeasured.** Every MDE in this note is arithmetic over an assumed σ ∈ {0.15, 0.30}, taken from the range
  Webber et al. report for AP deltas on TREC Robust (S10). One run of `graded.py --json` on two engine builds
  replaces the assumption with a measurement, and should be the first thing done.
- **The grade definitions are a judgement, not a measurement.** "2 = directly answers" is a criterion; whether two
  labellers apply it the same way is a kappa we have not computed.
- **NLI entailment is not a truth oracle.** ALCE's own authors record that their NLI model cannot detect partial
  support (S11), which is why they gate recall before precision. A claim-level metric inherits that blind spot.
- **Two claims rest on summaries, not on the papers.** UMBRELA's system-ranking correlation (Kendall's τ 0.80–0.90)
  and ALCE's human agreement (Cohen's κ 0.698 citation recall, 0.525 citation precision) were read in secondary
  summaries during search, not in the papers; both appear nowhere in §2–§5 for that reason.
- **No claim about InPars-style query filtering.** Its repository is not indexed on DeepWiki, and I did not read its
  paper.
- **Nothing here was measured on the live vault.** All numbers are either quoted from a source or arithmetic over
  our own documented n and the paper's σ values.

## 7. Source list

Primary, implementation or formula: S1 `trec_eval/m_ndcg_cut.c`; S2 `trec_eval/m_ndcg.c`; S3
`ir-measur.es/en/latest/measures.html`; S4 `llama_index/.../evaluation/retrieval/metrics.py`; S5–S8
`docs.ragas.io` (faithfulness, context_recall, context_precision, answer_relevance); S9 `amenra.github.io/ranx/compare`;
S10 `people.eng.unimelb.edu.au/jzobel/fulltext/cikm08.pdf`; S11 `ar5iv.labs.arxiv.org/html/2305.14627`.

Primary, judging and bias: S12 `arxiv.org/abs/2409.15133`; S13 `arxiv.org/abs/2412.17156`; S14
`arxiv.org/html/2502.13908v1`; S15 `arxiv.org/abs/2406.12624`; S16 `arxiv.org/abs/2306.05685`; S17
`aclanthology.org/2025.ijcnlp-long.18.pdf`; S18 `arxiv.org/abs/2404.13076`; S19 `arxiv.org/abs/2404.18796`; S20
`arxiv.org/abs/2311.09476`; S21 `arxiv.org/abs/2405.04727`; S22 `arxiv.org/abs/2405.07767`; S23
`arxiv.org/abs/2504.20879`; S24 `github.com/shmsw25/FactScore`, `arxiv.org/abs/2305.14251`.

Secondary (labelled as such where used): S25 `rubrex.ai/blog/rag-retrieval-ndcg` (practice write-up, 2026-10-16).

Framework documentation read through DeepWiki: `beir-cellar/beir` (3.2, 7.2); `explodinggradients/ragas` (3.2, 3.4,
3.6, TestsetGenerator); `promptfoo/promptfoo` (2.3); `run-llama/llama_index` (3.7, which could not show the metric
code — resolved from the file itself); `UKPLab/sentence-transformers` (asked about nDCG/binary/SAS evaluation: it
documented nDCG in `RerankingEvaluator` and `NanoBEIREvaluator` and average precision in
`BinaryClassificationEvaluator`, and explicitly could not supply the SAS rationale).
