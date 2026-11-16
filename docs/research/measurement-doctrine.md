# Measurement doctrine: five rules, each paid for by a defect

Every rule below cost a wrong number in this repository. They are written as tests of a *reported measurement*, not as
advice, because each failure had the same shape: the number was correct about its instrument and wrong about the thing.

## 1. A metric must say what it counts, and the count must come from the system, not from the text

`census.py` reported 76 notes "with no link of any kind" from a regex over bodies and block-list `topics:`. The engine said
**13**. The regex could not see the inline array spelling and could not see inbound references at all, and "unlinked"
turned out to be a question about in-degree — which the regex never computed.

- **Test**: for every count in a report, name the resolver. If the resolver is a regex or a `grep`, the claim is about the
  file format, and it must be labelled that way.
- **Corollary**: when two counts of one thing disagree, the one that resolves the data wins, and the disagreement is the
  finding. The intake pass moved the regex by 1 and the graph by 34 edges; only one of those was the change.

## 2. A new metric is validated against a deliberately degraded input before it is trusted

`graded.py` first shipped nDCG@10 with `IDCG = 1` and binary gains — which is `1/log2(rank+1)`, a monotone function of
`1/rank`. It could not disagree with MRR, and nobody noticed until a review of the field's practice read the formula.

- **Test**: reverse the page, drop the answer, or otherwise degrade the input in a way whose direction is known. A metric
  that does not move is not measuring. The reversal now moves nDCG by −0.568 against MRR's −0.434, which is how we know the
  two are different instruments.
- **Test, second form**: a self-pair must be identical. A bootstrap that calls identical input "better" is broken; ours
  returns INCONCLUSIVE with an interval of exactly zero.

## 3. A constant borrowed from another corpus is not a measurement of this one

The MDE table quoted 0.23 at n = 18 and 0.077 at n = 160, from a σ reported in the literature for other systems over other
questions, and from one of our own pairs contaminated by the effect it was meant to certify. A second opinion found it; all
three constants are withdrawn.

- **Test**: every constant names the population it was measured on and the run that measured it. If the population is not
  ours, the constant is a hypothesis.
- **Corollary**: power arithmetic hides its assumption in one symbol. A σ of the wrong type makes a table look quantitative
  and decides nothing.

## 4. Two defects that present identically are not one bug

`portable capsules` returned nothing. It looked like the plural defect that `capsules` had. It was not: no note holds both
words — the base calls the thing an "RVF container". Morphology is a lookup we can add; vocabulary is a statement the note
has to make about itself.

- **Test**: before fixing a symptom, produce the note that would answer and show **why** it was not retrieved (missing term,
  different form, unindexed field, ranked below, not on the page). Five different fixes live behind one empty page.

## 5. A passing gate that is narrower than the claim is worse than no gate

The lint gate passed because it ran on one crate; the compatibility claim was about all of them. The tool-count claim passed
because nobody counted; the actual surface is 36 tools and 14.7k tokens per session. The vacuous green is the expensive
kind.

- **Test**: for each green, state the domain it covered and the domain of the claim. If they differ, the gate is a
  statement about the gate.
- **Corollary**: measure the gate's own resolution. At n = 18 a bootstrap cannot see one flipped task; publishing the
  verdict without that sentence invites a decision the instrument cannot support.

## What these rules cost, and what they buy

Each test is cheap: one engine call instead of one regex, one reversed page, one paired run, one note opened to see why it
was missed, one sentence stating a domain. In this session they caught six wrong numbers before they reached a decision,
including two of the author's own — the census metric and the nDCG.

The general form is one sentence: **a measurement is a claim about a system, so the cheapest check is to ask the system.**
