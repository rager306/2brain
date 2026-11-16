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

Fix, pending: `tb_search` and `tb_grep` default to `own`, `scope: "all"` stays as the escape. It is a behaviour
change and belongs in the CHANGELOG, not in a footnote.

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
