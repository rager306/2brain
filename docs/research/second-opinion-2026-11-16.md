# Second opinion on the measurement stack, 2026-11-16

*Read-only adversarial review, as requested. It objects to the structure of the arguments in five
documents — `semantic-eval-practices-2026-11-16.md`, `task-set.md`,
`question-set-baseline-2026-11-16.md`, `semantic-judgements-2026-11-16.md`,
`intelligence-axes-2026-11-16.md` — and to the set definition they measure against
(`tools/ranking/questions.json`). It does not redo arithmetic and takes no measurements: every number
below is either quoted from these documents, read from the set's own file, or recomputed from their
quoted inputs. Where a document already states a caveat and then contradicts it, the contradiction is
the finding.*

## The four questions, answered first

1. **Does "nDCG@5 with grades and MDE" fix the evaluation?** No. The grades exist for 2 of 15 scored
   rows; the 160-question set carries no grades at all; and the σ every MDE here is computed from is
   of the wrong type, from a different set, on correlated rows (O1, O3, O8). **No MDE constant in
   this stack is quotable.** The quotable statement is the one the stack itself wrote in another
   document: "σ is unmeasured."
2. **Is "160 questions with nine axes and a pairwise design" a design?** No. Five of the nine
   declared axes are never reported, four of the reported cells have no comparable contrast, and the
   pairing evidence for the document's strongest claim is one clean pair plus one pair the set's own
   file files under a different axis (O6, O7). Axis marginals are descriptions of how the queries
   were written, not findings about the engine.
3. **What can the semantic judgement of 42 questions conclude?** Verifiable per-hit facts only:
   rank of the target path, absence of a string in the corpus, whether `weak` fired. Every
   level-derived aggregate — the 38% headline, the level distribution, the taxonomy counts, the
   "fixes 16 of 42" projection — is one judge's reading with no agreement measurement, over a sample
   of which 24 questions were written by the judge itself (O4, O5, O10).
4. **Does the MDE say a 160-question set reaches 0.077?** The formula assumes independent rows; the
   set contains ten deliberate pairs (ρ ≈ 1 by construction) and nine mechanism-clustered classes,
   and every one of those correlations moves the quoted number in the flattering direction. 0.077 is
   a floor on the honest MDE, not an estimate of it (O8).

---

## O1 — The two repairs have never met, and the σ the MDE needs is unmeasured

**Claim as written.** "Read the conservative column: at n = 18 the smallest nDCG@5 change this set
can certify is about 0.23 … A 160-question set would reach 0.077." (`task-set.md`, §σ.) And: "at
σ = 0.391 the MDE at n = 145 is about 0.08 nDCG" (`question-set-baseline-2026-11-16.md`).

**Objection.** Webber's `δ = 2.486·σ/√n` takes σ = the standard deviation of *paired per-topic
deltas between two comparable systems*, estimated over several system pairs on the topics being
tested. The stack feeds the formula three stand-ins at once: σ measured as the *cross-task spread of
one system's scores* (a different quantity), on *15 tasks of a different question set* (the
adversarial 18-task diagnostics, not the 160-question convenience set), with the alternative column's
σ = 0.129 taken from *one system pair and contaminated by the effect it certifies*. Calling the
first column "conservative" fixes the direction for similar systems; it does not make the output an
estimate of anything. Worse, the components of the proposed fix have never been applied in the same
place: `tasks.json` grades 2 of 15 rows ("Two tasks carry a grade-1 label"), the 160-question set
has `expect` paths only (binary), and the 42 semantic judgements live on a third, disjoint
question set — so graded nDCG@5 cannot be computed on the large set without a judgement pass
nobody has run. Three disjoint instruments (grades, size, σ) are presented as one repair.

**Evidence.** `task-set.md`: "the headline uses the cross-task σ, which treats the fifteen scored
questions as a sample and is therefore conservative (it is also an unpaired bound: a paired test
would subtract the between-run correlation, which one deterministic pair cannot estimate)" — the
caveat is stated in the same paragraph that prints three decimals. `semantic-eval-practices`
§6: "**σ is unmeasured.** Every MDE in this note is arithmetic over an assumed σ ∈ {0.15, 0.30}."
`task-set.md`: "**Two tasks carry a grade-1 label**"; "Thirteen tasks stay binary"; `questions.json`
rows carry `expect`, never a grade.

**Smallest fix.** Delete the MDE constants (0.229, 0.077, 0.083, 0.025) from both documents. State:
"σ for paired nDCG@5 deltas on this corpus is unmeasured; the MDE will be computed from the first
two graded runs on the 160-question set." Keep the formula and the Webber citation.

**Verdict. Withdraw the constants, keep the method.**

## O2 — One document says the graded metric makes arguments "decidable"; its sibling says it cannot

**Claim as written.** `intelligence-axes-2026-11-16.md`: "nDCG@10 over the existing labelled set is
a day of work and **makes every later ranking argument decidable**."

**Objection.** This is the same category error as O1 in a different document: a metric repair sold
as a power repair. The stack's own practices note says the opposite, explicitly, about the same
change: "this change buys expressiveness, not power. It makes small effects *visible*; it does not
make them *significant*." At n = 18 with σ anywhere in the plausible band, a graded metric cannot
make a one-flip change decidable, and `task-set.md`'s own headline says so: "**the eighteen-task set
cannot certify the size of change we actually ship.**" Two documents of the same stack assert
contradictory things about the same metric on the same set, and the one that reads into the backlog
("what we are missing, #1") is the wrong one.

**Evidence.** Quotes above; plus `semantic-eval-practices` §5: "at n = 18 the detectable δ is
around 0.09–0.18 depending on σ."

**Smallest fix.** One word: replace "decidable" with "expressive" in `intelligence-axes`, or cite
the sibling sentence there.

**Verdict. Soften.**

## O3 — At the point of use, the caveat compares an MRR delta to an nDCG MDE and defers to an instrument that does not exist

**Claim as written.** `question-set-baseline-2026-11-16.md`: "at σ = 0.391 the MDE at n = 145 is
about 0.08 nDCG, and the shipped-vs-candidate difference above (0.018 MRR) is below it. The set
names the mechanism; the bootstrap decides whether it is real."

**Objection.** One sentence, three substitutions. Metric: σ was measured on nDCG@5, the compared
delta is MRR — different per-question distributions (MRR carries mass at 0 and at 1/r; nDCG@5's
spread is what 0.391 describes), so "below it" compares numbers in different units. Set: the σ comes
from the 18-task set, applied to the 160-question set's n = 145. Instrument: the bootstrap that
"decides" was validated only on binary pass vectors, and the graded interval is unbuilt future work
in the stack's own words ("graded.py … should print a paired interval" — §2.3, the verb is *should*).
And the second sentence contradicts `task-set.md`'s rule verbatim: "A bootstrap run after the change
cannot supply the hypothesis. It tests the difference in pass counts, not the mechanism the change
was made for." This is the requested pattern exactly: the caveat is stated in one document and
contradicted by its own neighbour.

**Evidence.** Quotes above.

**Smallest fix.** Compare deltas only to an MDE in the same metric over the same set; replace "the
bootstrap decides whether it is real" with the name of the instrument that would decide it and what
it would compute.

**Verdict. Withdraw** (the sentence, not the 160-question set).

## O4 — One judge, no kappa, while the stack's remedy document names kappa as the condition of shipping

**Claim as written.** `semantic-judgements-2026-11-16.md`: "Top hit answers the question for
**16 of 42** (38%)." And its Limits section: "The judgements are one reader's. … the levels are
coarse enough that a second reader should agree on most; `partially` is where two readers will
differ most (19 hits)."

**Objection.** All 207 level assignments come from one judge (an LLM) with no agreement measurement;
the assertion that a second reader "should agree on most" is itself unmeasured. The stack's own
practices note sets the shipping condition this document skips: "Do not ship a judge whose kappa
against the human slice is unmeasured" (§2.6), and names the pattern "The most common mistake in
the field (§4.1)". The boundary that decides the headline — a rank-1 hit scored P instead of A — is the
exact boundary the document concedes is least stable, and 19 of 207 judged hits (9%) sit on it.
What *can* be concluded from this document: verifiable facts (the target path's rank, absence of
`max_connections` in 0 files, `weak` firing 0/8 on unanswerable top hits, the meta-note artefact on
T10). What *cannot*: the 38%, the level distribution, the taxonomy's ordering by count, and — via
the grade propagation below — the two grade-1 labels in `tasks.json` ("Both grade-1 notes are `P` …
in `semantic-judgements`"), which import the uncalibrated judgement into the metric's labels.

**Evidence.** Quotes above; `semantic-eval-practices` §2.6: "Report Cohen's kappa, not raw
agreement"; §6: "whether two labellers apply it the same way is a kappa we have not computed."

**Smallest fix.** A second reader (human, or a judge from a disjoint model family — §2.7's rule)
double-labels 20–30 hits; report kappa on the four-level scale and on answers-vs-not. Until then,
print the headline as "one judge's reading of 42 questions".

**Verdict. Soften.**

## O5 — The admitted unfairness is stated and then not applied: 24 of 42 questions are the judge's own, and every aggregate mixes them in

**Claim as written.** Limits: "24 of the 42 questions are mine, written after reading the vault's
titles. That makes the sample fair for finding failure shapes and **unfair as a score**."

**Objection.** The caveat is two sections below the scores, and nothing above it is re-scored
without them: the "16 of 42" headline, the level-distribution table, and the failure taxonomy's
counts (8 > 7 > 3 > …) are all computed over the mixed sample. The author of 24 questions judged
hits aimed at notes whose wording they knew — generator and judge are the same agent, which is §4.2's
trap in the labelling direction, compounded by §4.4's self-targeted-question bias the stack itself
documents. The taxonomy's causal ranking ("what dominates") is exactly the aggregate most exposed to
question-provenance: several of its groups (cross-lingual, no-abstention) are populated
disproportionately by self-written rows.

**Evidence.** Quotes above; the level-distribution and taxonomy tables in
`semantic-judgements-2026-11-16.md` carry no provenance split.

**Smallest fix.** Split every aggregate — headline, distribution, taxonomy counts — into the 18
pre-registered `tasks.json` rows and the 24 self-written rows. The 18-row subset is the defensible
score; the 42-row set remains a shape catalogue.

**Verdict. Soften** (keep the taxonomy, split the numbers).

## O6 — "Nine axes" is not a factorial design: five axes are never reported, and four cells have no contrast to make

**Claim as written.** "The set is 160 rows with nine axes" and "The axes were the reason to build
this set, and they say more than the classes" (`question-set-baseline-2026-11-16.md`).

**Objection.** A factorial needs comparable cells; this set has decorated rows. In
`tools/ranking/questions.json`, the declared axis values over the 160 rows are: `scope` own 159 /
all 1; `temporality` none 156 / as-of 4; `lifecycle` proposed 1 (of 160); `entity` error 3 /
decision 1; `type` claim 2 / decision 1. No contrast on those axes is possible at any n — one row
per cell is anecdote wearing a column header. And the document reports marginals for four axes only
(form, length, entity, language); `hop`, `lifecycle`, `type`, `scope`, `temporality` — five of the
nine — appear in no table in any of the five documents. To answer the specific question: supported
by ≥ 5 rows *as association* are form (keyword 35 vs question 78), length (short 59 vs medium 77;
the long cell is 9 rows, so any long-specific claim is anecdote-sized), language (ru 38 vs en 93 —
confounded, see O7), entity (artifact 15 vs concept 126 — degenerate: "pasted path" vs everything
else, and 15/15 at rank 1 is a tautology of the input design, not a discovery about the engine).
Anecdote with a number attached: everything on the error/decision/claim/proposed/as-of/all cells,
and each named single-row mechanism (z01, z20, z08) — honestly labelled with row ids, but still
n = 1.

**Evidence.** `questions.json` axis tallies as quoted above; the per-axis table in
`question-set-baseline-2026-11-16.md` covering four axes; `axes_enum` declaring ten keys.

**Smallest fix.** Stop calling it a factorial: either report the four axes that have contrast and
delete the decorative five from the schema, or rebalance the next set so every axis contrast has
≥ 10 rows on both sides.

**Verdict. Withdraw the factorial framing; keep the four marginals as description.**

## O7 — The strongest sentence in the baseline document cites a pair the set's own file files under a different axis

**Claim as written.** "**A keyword string is a different query from the question it came from.** …
The paired rows make this attributable rather than anecdotal (`x03`/`x04`: three keywords rank 1,
the 17-word version never enters the page)." (`question-set-baseline-2026-11-16.md`.)

**Objection.** The file that defines the pairs classifies `x03`/`x04` as `axis: "length"` — the
pair changes form *and* length, so it cannot attribute the effect to form. The only pair tagged
`axis: "form"` (`x05`/`x06`, same topic, keyword vs question) measured keyword at **rank 4** — not
rank 1 — against question at rank 12, which supports "keyword ranks higher" and contradicts the
strength of the surrounding claim ("31 of 35 keyword rows hit at rank 1"). The cross-sectional
contrast (31/35 vs 31/78) is confounded with topic and author intent by construction: keyword rows
were written for lexically distinctive notes. The 10-pair tally the file enables — 6 pairs with a
difference, 4 ties — is never reported, so the reader cannot see that the within-question evidence
is two pairs, one of them confounded.

**Evidence.** `questions.json` `pairs`: `{"a": "x03", "b": "x04", "axis": "length", "ranks": [1,
null]}`; `{"a": "x05", "b": "x06", "axis": "form", "ranks": [4, 12]}`.

**Smallest fix.** Report the pair tally with per-axis counts (6 diffs / 4 ties; form: 1 clean pair)
and restrict the keyword conclusion to what the pairs show until more pairs exist.

**Verdict. Soften** (from "attributable" to "consistent with the two form/length pairs").

## O8 — The language axis measures corpus answer-language coverage, not ranking, and the table sells it as engine performance

**Claim as written.** Per-axis table: `language | russian | 38 | 17 | 30` beside `english | 93 |
50 | 72` (`question-set-baseline-2026-11-16.md`).

**Objection.** The stack's own task-set files this exact gap elsewhere: "bilingual — **defect**, in
the corpus, not in the ranker." The set's own language pair (`x01`/`x02`, same question in English
and Russian: ranks 1 vs absent) shows the mechanism is query↔answer language mismatch — a corpus
property, not a ranking one. The `russian` axis value mixes same-language Russian rows with
cross-lingual rows, two different defects under one label; and the file has a third value,
`mixed` (19 rows over the 160), which the reported table omits entirely. A table that attributes a
corpus-coverage defect to the ranking axis cannot be cited for ranking conclusions.

**Evidence.** Quotes above; `task-set.md` §"bilingual — defect, in the corpus, not in the ranker";
`questions.json` `{"a": "x01", "b": "x02", "axis": "language", "ranks": [1, null]}`; file tallies
`language {en: 102, ru: 39, mixed: 19}`.

**Smallest fix.** Split the language marginal into same-language and cross-lingual rows; move the
cross-lingual gap to the corpus-coverage defect list where the stack already filed it.

**Verdict. Soften.**

## O9 — Rows whose correct outcome is a refusal are scored in a ranking table, so every number in it is ambiguous between two success criteria

**Claim as written.** Per-axis/class tables: `form | silly | 19 | 9 | 12`, "not on the page 7";
structural `0` at rank 1 (`question-set-baseline-2026-11-16.md`).

**Objection.** The stack's own documents define the correct behaviour for these rows as refusal:
"for them the correct answer shape is a refusal, not a hit" (`semantic-judgements`), "Structural
questions are not search questions … `tb_search` should not be asked" (baseline). A ranker that
abstains perfectly scores 0 in this table, and the "not on the page" column counts correct
refusals as misses. Marginals computed across rows with incompatible success criteria — "the
ranker failed" vs "the system did the right thing" — cannot support axis or class conclusions, and
they also contaminate the n that the power arithmetic uses (O8's 145 includes 19 silly and 12
structural rows the document itself says are not ranking measurements).

**Evidence.** Quotes above.

**Smallest fix.** Two tables: ranking rows (answerable, graded) and abstention rows (scored by
refusal correctness, e.g. did `no_evidence` fire); never aggregate across them.

**Verdict. Soften** (restructure the table).

## O10 — The "single change that would fix the most questions" is a projection over the same judge's labels, written as a measurement

**Claim as written.** `semantic-judgements-2026-11-16.md`: "The single change with the largest
**measured** effect is a claim-level check over the top 20 … it fixes 7 promotions, 1 referent and
8 refusals, 16 of 42." And: "One mechanism, **measured against this sample**:"

**Objection.** The claim-level check does not exist — the document says so itself ("it needs a
judge … and a decision rule"). What was "measured" is arithmetic over the level labels of O4: an
uncalibrated single judge, 24 of whose 42 questions it wrote itself, evaluating a proposal its own
author advanced — the circularity §4.2 and §4.7 name, applied to an effect size. The projection
also inherits the P/A boundary instability (several "promotions" depend on `partially` vs
`answers` calls), so its input error is largest exactly where its counts are smallest.

**Evidence.** "The claim check is not free: it needs a judge … and a decision rule for what
`states the claim` means" — the instrument is unbuilt in the very document that quotes its
effect size.

**Smallest fix.** Rename to "projected effect under one judge's labels"; validate on 5 questions
with a second judge family before any "fixes N of 42" is quoted.

**Verdict. Soften.**

---

## What this stack should stop measuring

It should stop measuring **small ranking deltas on hand-built sets** — every per-change "+0.02,
+0.03" claim, every bootstrap verdict quoted as a gain estimate, and every MDE constant derived
from a σ nobody measured. The field's own cited arithmetic says a δ = 0.03 needs ~164 independent
topics at σ = 0.15 (Webber, quoted in the practices note), and this stack's own numbers say its
shipped changes are +0.033 — below every MDE it computed, at every n it will plausibly build with
correlated rows. At this scale the honest outputs are the ones the stack already produces well:
pre-registered class criteria that name a mechanism and its two rows, and the reversed-ranker
veto. Second, it should stop measuring **"X of N questions answered" from a single uncalibrated
judge** — the number looks like a benchmark, carries no agreement measurement, and the stack cites
the remedy (Cohen's kappa, §2.6) and then skips it in the very document that needed it. Third, it
should stop reporting **marginal hit@k by axis** from a confounded convenience sample: with rows
written by the same authors who chose the axes, these tables can only re-describe how the queries
were written, never what the engine does. Fourth, it should stop counting **refusal-correct rows
as ranking failures**. What survives all five documents is narrow and worth keeping: mechanism-
named defects with pre-registered criteria, the veto, abstention measured by subject-coverage and
attribute-presence (which needs no judge), and rank-of-target facts, which are checkable without
judgement at all. Everything else in this stack is decoration until σ, grades on the large set,
and a second reader exist.
