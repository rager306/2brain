# The 160-question set, measured: keyword strings win, questions lose, and half the noise returns nothing

First run of `tools/ranking/questions.json` through `tools/ranking/run_questions.py`, against two engines: the **served**
binary (which predates seven commits) and a **candidate** built from `HEAD` over a `cp -a` copy of the vault. Both at
`scope: own`, page of 10, hit window 5.

## Headline

| engine | answerable rows | hit@1 | hit@5 | not on the page | MRR |
|---|---:|---:|---:|---:|---:|
| served | 145 | 73 | 111 | 25 | 0.619 |
| candidate (HEAD) | 145 | 75 | 115 | 22 | 0.637 |

The deployment lag costs **4 rows at hit@5 and 0.018 MRR** — small, but it is the difference between a base that answers
`capsules` and one that does not, and it is why the regression class scores 4/13 rather than 7/13 on the served binary.

## Per class (candidate)

| class | n | hit@1 | hit@5 | not on the page |
|---|---:|---:|---:|---:|
| exact identifier | 15 | 15 | 15 | 0 |
| ambiguous | 15 | 13 | 15 | 0 |
| cross-lingual | 16 | 6 | 15 | 1 |
| multi-hop | 20 | 9 | 18 | 1 |
| paraphrase | 35 | 19 | 27 | 5 |
| regression | 13 | 4 | 7 | 6 |
| silly / noise | 19 | 9 | 12 | 7 |
| structural | 12 | **0** | 6 | 3 |

## Per axis (candidate)

The axes were the reason to build this set, and they say more than the classes:

| axis | value | n | hit@1 | hit@5 |
|---|---|---:|---:|---:|
| form | keyword | 35 | **31** | 35 |
| form | question | 78 | 31 | 61 |
| form | silly | 19 | 9 | 12 |
| length | short | 59 | 42 | 51 |
| length | medium | 77 | 31 | 58 |
| length | long | 9 | 2 | 6 |
| entity | artifact (a pasted path) | 15 | **15** | 15 |
| entity | concept | 126 | 58 | 97 |
| language | english | 93 | 50 | 72 |
| language | russian | 38 | 17 | 30 |

Three of those are the whole document:

1. **A keyword string is a different query from the question it came from.** 31 of 35 keyword rows hit at rank 1; the
   same questions phrased naturally hit 31 of 78. The paired rows make this attributable rather than anecdotal
   (`x03`/`x04`: three keywords rank 1, the 17-word version never enters the page).
2. **A pasted path always works and a concept rarely does.** 15 of 15 artifact rows hit at rank 1 — the index is excellent
   at finding a name you already have — against 58 of 126 conceptual rows. That is the shape of a base that is good at
   *confirming* and weak at *discovering*.
3. **Structural questions are not search questions.** 0 of 12 at rank 1, and the ones that reach the page do so by
   accident. Listing a directory or a queue is `tb_children`/frontmatter work; `tb_search` should not be asked.

## What this costs the noise class

Half the noise rows (7 of 19) return nothing at all, and the ones that do answer are mostly the typo pairs — a one-character
edit survives because the other terms carry the query. A trailing `?` is not the cause (measured: `oracle matrix` returns
nothing because a two-term default floor is an AND, not because of punctuation); a Russian word form is
(`битемпоральная` finds a note, `битемпоральность` finds none, because the word-form lookup only knows Latin suffixes).

## How to read any future number from this set

`run_questions.py` stores every row's page, so a change is diffable row by row. The set is 160 rows whose axes are unevenly
populated (two axes carry 159 of 160 rows in one value, so they have no contrast), which is enough to attribute a miss to a
class and, for `form`, `length` and `entity`, to name the axis **when the class is held fixed** — and it is not enough to
certify a small effect.

**No MDE is quoted for this set.** A second opinion (`docs/research/second-opinion-2026-11-16.md`) found that the power
constant used from `task-set.md` was computed from the cross-task spread of one system over a different set of fifteen
tasks, not from paired deltas between comparable systems; the honest state is that the σ this set needs has not been
measured, and it becomes measurable only after two graded runs over these 160 rows exist. Until then the rule stands: this
set names a mechanism and shows the row it moved; it does not certify that the movement is real.

## What the noise class actually contains

Nineteen rows, and the failures sort into four shapes — only the first two are defects:

| row | query | total | what happened |
|---|---|---:|---|
| `z01` | `agnet memory retrievel` | **0** | two misspellings, so no term has a posting and an AND over unknown terms is zero |
| `z20` | `битемпоральность` | **0** | the note says `битемпоральная`; the word-form lookup only knows Latin suffixes |
| `z08` | `https://arxiv.org/abs/2602.20021` | 109 | the note whose **`source_url` is that URL** is not in the top ten: the index holds body and description, never the URL field |
| `z10` | `WHY DOES THE EVENT LOG BEAT THE GRAPH` | 356 | case is not the problem — the tokenizer lowercases — the hub ranks first, the known hub-over-atomic defect |
| `z12` | `agent memory memory` | 228 | a repeated term is deduplicated, so this is the plain two-term query, and the expected note is outside the top ten |

The four that pass are the instructive part: **a single degenerate feature does not break a query** — `oracle matrix?`
(rank 2), `memory decay?? 🔥` (rank 1), a pasted path with a `#block-id` (rank 1), a pasted `tb_search(...)` call (rank 1),
ALL CAPS on a topic that is not crowded (rank 1). What breaks a query is either *every* content term being unknown
(`z01`), or a term that exists in the base under a different form (`z20`), or a field that is not indexed at all (`z08`).

That last one is the cheapest real gap the set has found: a note carries its `source_url` in frontmatter, an agent pastes
the URL it was given, and the engine cannot connect them — and it is the same gap the provenance-echo work hit from the
other side (identity recorded, nothing parses it).

## Does the service-file noise cost anything at the top? Measured, and it is smaller than it looks

The pages the 160 questions returned hold **1 234 note hits, 156 service hits** (16 `.claude/skills/*/SKILL.md` and 4
`ops/skills-archive/*.md`) **and 30 other** (`manual/`, `ops/queue/`, `type/`). Service files are 11% of all hits, and the
harvesting run counted them in a top-3 slot on 25 of 160 rows.

Simulating their removal — drop only those two directories, keep `manual/` and `ops/queue/`, which are legitimate answers
to structural questions:

| page | hit@1 | hit@5 | not on the page |
|---|---:|---:|---:|
| as returned | 75 | 115 | 22 |
| service files removed | **78** | **116** | 22 |

So the cost is **three top-1 slots over 145 answerable questions (4% relative) and one top-5**. That is worth having, and
it is not the 17.9% the share of hits suggests: the noise costs a reader *context*, not *rank*, because a skill file that
outranks the answer is rare — the four pages where one is first are long natural-language questions, which is exactly the
`form: question` axis that already scores 31/78.

Two consequences. The cheap fix is a `kind` field per hit (note | map | skill | manual | queue) so a caller can discount
what it does not want, and a default that omits `skill` unless asked — the same shape as `scope: own`, and for the same
reason. The expensive fix — removing the archive from the served tree — would fix four pages and take away a record of what
the vault used to contain; not worth it for three slots.
