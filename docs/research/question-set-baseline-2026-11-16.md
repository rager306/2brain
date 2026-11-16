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

`run_questions.py` stores every row's page, so a change is diffable row by row. The set is 160 rows with nine axes, which
is enough to attribute a miss to a class or an axis and **not** enough to certify a small effect: at σ = 0.391 the MDE at
n = 145 is about 0.08 nDCG, and the shipped-vs-candidate difference above (0.018 MRR) is below it. The set names the
mechanism; the bootstrap decides whether it is real.
