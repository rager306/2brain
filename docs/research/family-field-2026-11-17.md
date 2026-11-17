# `family:` — what it does today, and the hybrid that keeps both jobs

Measured 2026-11-17 on `/root/vault` at `a8aba4b` (frontmatter of 1 063 notes). This is an analysis
for a decision, not a migration that has run.

## The numbers

| fact | value |
|---|---|
| notes with `family:` | 827 |
| distinct family values | 429 |
| family values with exactly one note | **345 (80 %)** |
| notes with `topics:` | 950 |
| notes with both | **804** |
| family values that look like provenance (`f/arxiv-*`, `f/gh-*`, `f/docs-*`, domains) | 191 values, 326 notes |
| family values that look like a topic or a series (`f/patterns`, `f/agent-architectures`, …) | 238 values, 501 notes |
| topic-like family values with one note | 186 of 238 |

The largest values make the split concrete: `f/patterns` (105 notes) is a curated **series**;
`f/arxiv-2602.20021` (16) and `f/docs-google-com` (14) are **provenance**; `f/graph-theory-books` (14)
is a series again. A single field carries all three meanings, and `topics:` already carries the
subject meaning for 950 notes.

## Why the 345 singles are not the defect

A provenance family with one note is correct: one paper, one note. The defect is a **topic-like**
family with one note — a promise of a group nobody joined — and there are 186 of those. Any rule that
"splits the 345 singles" would rewrite provenance that is already right.

## Scenarios

| scenario | what changes | what it gives | cost / risk |
|---|---|---|---|
| **A. leave it, add a lint** | nothing; a report lists single-member families | a number, no semantics | zero; the ambiguity stays |
| **B. semantic split, big bang** | every topic-like family becomes a `topics:` entry; `family:` is provenance only | decidable queries: lineage vs subject | 238 values rewritten, 501 notes touched, history noisy |
| **C. discriminator** (`family_kind: provenance\|topic`) | one field, one enum | readers can branch | every query must branch; a type change; no data becomes cleaner |
| **D. hybrid: family = series/namespace, topics = subject** | a written rule + a tooling audit; topic-like families are re-homed *when a note is edited anyway* | provenance stays reliable, series stay addressable, subject queries already work; singles become legitimate | no big-bang; the rule is enforced by a report, not a mutation |

**Recommendation: D.** State the rule in `AGENTS.md`/the type docs: `family:` names the note's series
or source namespace (a provenance id or a curated series such as `f/patterns`), `topics:` names the
subjects the note belongs to. A single-member family is then legitimate; a *topic-shaped* family with
one member is a finding for the next intake pass, not a schema error. Re-home values opportunistically
(where `topics:` already names the same thing), never in one sweep.

What D does not fix, stated plainly: `family` remains a string without a registry, so two spellings of
one series still coexist, and a family that is a topic by name is not prevented — only reported.

## The audit that makes the rule real

`tools/kb/family_audit.py` (tooling layer, standard library) reads the vault, classifies each family
by the rule above (provenance pattern vs series/topic), and prints: single-member values by class,
values whose name is also a `topics:` entry, and notes whose family and topics disagree. It must not
write. Its first run is what decides whether the 238 topic-like values are three families or thirty.
