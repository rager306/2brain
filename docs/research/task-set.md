# The task-level set, and the three defects it found on its first run

`tools/ranking/tasks.json` holds twelve hand-written tasks: exact identifiers, paraphrases, a Russian question about
English material, multi-note questions, one ambiguous query, and three the base should **not** answer.
`tools/ranking/tasks_eval.py` scores them. The synthetic set (`labelled-queries.json`) measures rank stability; this
one measures whether a real question finds its evidence.

First run, live vault, 2026-11-16:

| class | passed |
|---|---|
| exact | 1/2 |
| paraphrase | 2/3 |
| ru-cross | 1/1 |
| multi-hop | 2/2 |
| ambiguous | 1/1 (rank 9) |
| absent | 0/3 |

The misses are the useful part.

## 1. `tb_search` searches dependency repositories by default

The query *"the exact number of stars on the 2brain repository"* — which this vault cannot answer — returned
`/root/2brain/tb-mcp-sdk/README.md`, a file **outside the served vault**, as its top hit. The vault's
`.arsumbris/repo.yaml` declares a `mcp-bundle` dependency, and the default scope follows dependencies.

With `scope: "own"` the same query returns vault notes only. Two consequences:

- **Default scope should be `own`.** An agent asking about its project should not receive a dependency repository's
  README unless it asks for everything. This is the failure IWE #410 reports (one Claude, two bases, same tool), and
  the review's §2.4 in its own words.
- The three `absent` tasks cannot be measured at all while the default is `all`: the corpus answering them is not
  the corpus under test.

**Fixed 2026-11-16.** `tb_search` and `tb_grep` default to `own`; `scope: "all"` stays as the escape. The behaviour
change is in the CHANGELOG, and two end-to-end tests pin it: a default query answers from the own workspace, an
explicit `all` sees the mounted dependency as well.

## 2. Function words count as matches, so no question is unanswerable

Even scoped to the vault, *"the exact number of stars on the 2brain repository"* matches five query terms — `the`,
`of`, `on`, `number`, `exact` — because BM25 here has no stopword handling. An absent question therefore always
returns something, and the caller gets no signal that the base is guessing.

Two candidate treatments, both measurable on this set:

- drop a small English stopword list at query time (cheap, and it changes document frequencies);
- a minimum-idf floor per matched term, so a term that appears in most notes contributes nothing.

The honest test is not "does the top hit look relevant" but "do the `absent` tasks stop returning hits while `exact`
and `ru-cross` keep their rank". The second half of that sentence is what makes this safe to try.

## 3. The exact-identifier label was too narrow, and that is a labelling lesson

`ReasoningBank` found `knowledge/agentdb-architecture.md` at rank 2, behind a note whose filename also carries the
term. Both are legitimate evidence. The task schema now allows `accept` (any of several correct paths) beside
`evidence` (the minimal set), because a single-path label measures the label as much as the search.

## What this set is for

Stage 3 of the plan needs a threshold written down **before** the next ranking change. With twelve tasks, one
position is 8 percentage points, so this set is a smoke test, not a benchmark: it is sized to catch a regression and
to name a defect, not to prove a gain. Growing it to forty tasks with handled evidence is the next step, and the
`absent` class has to become answerable first — which means fixing (1) and (2).

## Experiment 1: function words as noise — a negative result, measured

Hypothesis: the `absent` class fails because stopwords (`the`, `of`, `on`) count as matches, so dropping them from
the term count would make an unanswerable question look unanswerable.

Two runs on the live vault, before the scope default changed (so the dependency hit is still present and the two
effects are visible separately):

| run | absent | exact | paraphrase | multi-hop | ru-cross | ambiguous |
|---|---|---|---|---|---|---|
| baseline | 0/3 | 1/2 | 2/3 | 2/2 | 1/1 | 1/1 |
| `min_should_match` = number of non-stopword terms | 2/3 | 1/2 | 1/3 | **0/2** | 1/1 | 1/1 |

**Rejected.** Requiring every content word answers two absent questions by making the engine refuse to answer
questions it *can* answer: `как держать 2FA-коды вне доступа агента` and `ночная задача в 3:00 компилирует день в
контекст` both returned nothing. A conjunction over all terms is the wrong shape for a question, and a stopword
list cannot fix a relevance threshold.

The third absent task still matched five terms even with the list applied, because its content words — `exact`,
`number`, `stars`, `repository` — are frequent in a vault about software. The signal that separates "this base knows"
from "these words occur" is **term rarity**, not word class.

### The next experiment, with its rule written before the measurement

Mechanism: after ranking, drop a match in which **no matched term has an idf above a floor**, and let the floor be
chosen from the corpus's own idf distribution (reported as a histogram, not tuned against the task set).

Pre-registered acceptance, all of it:

- `absent` reaches **3/3**;
- `exact` 2/2, `ru-cross` 1/1, `multi-hop` 2/2 are unchanged;
- topical hit@1 on `labelled-queries.json` falls by no more than **0.02**, and literal hit@1 does not fall at all;
- the rule is measured on the candidate engine, never on the served vault.

If it misses any one of those, it is rejected and published here, like the freshness prior and this one.

## Experiment 2: the `weak` flag — half a result

The pre-registered rule was: drop or flag matches whose best term is common. The flag shipped as `weak` (best
matched term's idf < `RARE_IDF` = 2.0), measured on a candidate engine (debug build, copy of the vault, own socket):

| class | baseline | with `weak` | verdict |
|---|---|---|---|
| absent | 0/3 | **0/3** | fails the rule |
| exact | 1/2 | 1/2 | unchanged |
| paraphrase | 2/3 | 2/3 | unchanged |
| multi-hop | 2/2 | 2/2 | unchanged |
| ru-cross | 1/1 | 1/1 | unchanged |
| ambiguous | 1/1 | 1/1, top hit **weak** | flagged |

So the flag does not make an unanswerable question look unanswerable, and the pre-registered acceptance fails. It is
kept as a **signal** because the ambiguous case shows it is true and useful in the other direction: `agent memory`
is a legitimate question whose top hit really does rest on two corpus-wide words, and telling the caller that is
worth a boolean.

Why the absent class still fails is the finding worth keeping:

- `какая лицензия у Apache Kafka` matched three terms, one of them (`apache`) rare in this corpus — and `apache`
  is rare **and unrelated**, which the idf cannot distinguish from rare and central.
- `the exact number of stars on the 2brain repository` matched five, including `repository` and `2brain`.
- `сколько стоит подписка Netflix в Таиланде` matched two, including `netflix`.

Rarity answers "is this word unusual here", not "is this note about the thing asked". A lexical engine cannot
close that gap with a threshold: the next candidate needs corroboration — a rare term that also matches the note's
**description** (the field the author wrote to say what the note is about) — or a semantic signal, which this
engine deliberately does not have.

### Consequence for the plan

The `absent` class is not a ranking target; it is a **reporting** target. What an agent needs from this base is not
"no results" but "these are the closest notes and here is what none of them establishes" — which is the `context`
contract in the trajectory, not another scoring term. The flag stays, documented as a signal rather than a verdict,
and the next ranking attempt has to name which of those two shapes it is improving.
