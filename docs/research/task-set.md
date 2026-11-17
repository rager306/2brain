# The task-level set, and the three defects it found on its first run

`tools/ranking/tasks.json` holds eighteen hand-written tasks in eleven classes: exact identifiers, paraphrases, a
Russian question about English material, multi-note questions, one ambiguous query, three the base should **not**
answer, and five classes added from the coverage audit (section *The five failure classes*, below).
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

## The decision rule this table supports, and the one it does not

The table answers "does this class still pass", not "how much better is this build". Eighteen rows over eleven
classes give one to three rows per class, so a single row is 33–100% of its class and no percentage has a
sampling error behind it; a bootstrap over eighteen non-independent hand-written rows would be arithmetic
without a model. The rule is therefore per row and fixed before the run:

- a change is **accepted** only when its pre-registered rows pass and no other row moves against its recorded
  rank (a row that flips for an unrelated reason is a finding, not noise to average away);
- a change that moves a row it did not name is **rejected**, even if the move looks benign;
- a claim of improvement names the row and the rank, never a percentage (`hub 8 → 1`, not "+9%");
- repeated runs of one binary are not variance: the ranking is deterministic, and the only variance in this
  repository is the pilot's twelve fixtures across arms, which is a different experiment and is not mixed with
  this table.

A second query per class would be the cheapest way to earn a threshold; until then, `n` is part of the result.

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

**The same lesson, applied to `ambiguous` on 2026-11-17.** The query `agent memory` accepted three paths, and a
title-match experiment moved notes literally titled after the query above all three — which read as a regression but
was the label being narrower than the question. The accepted set is now **18**: the three definition notes plus the
fifteen notes whose filename states both query words and whose description is an agent-memory system or method. The
candidates were enumerated by a title predicate over a clean tree and reviewed by hand; no ranking was consulted, and
the `why` field records the rule. Measured against the live server (new engine, adapter `0.1.0+020d0de`): `ambiguous` **1/1 at
rank 1**, with the top hit `knowledge/memory-agent-abstraction-enables-natural-language-memory-operations.md`.

## What this set is for

Stage 3 of the plan needs a threshold written down **before** the next ranking change. With eighteen tasks, one
position is 5.6 percentage points, so this set is a smoke test, not a benchmark: it is sized to catch a regression and
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

## The five failure classes from the coverage audit, measured

`vault-coverage-kutha-themes.md` §3 found eleven defects by hand. Five of them are retrieval failures a user feels,
and a hand-found failure is a memory until something re-runs it. Each is now a class in
`tools/ranking/tasks.json`. Two of the five need more than a query, so a task may carry `args` (the flag its class
measures) and `min_total` (the width of the candidate set the documented behaviour implies); `tasks_eval.py`
passes both through.

### The run

Measured 2026-11-16 on a **candidate build**, not the served binary:

- source: this working tree at HEAD `8e81b4e`, clean when it was built, so `tb-cli` carries `c3c7ee4` (the word-form
  retry); built with `cargo +1.99.0 build --offline -p tb-cli`, and the adapter built `--offline` in `tb-mcp-rs`,
  both debug profiles. A concurrent instrumentation change reached the working tree after these binaries were
  linked; it is not in the numbers below;
- corpus: a `cp -a` copy of `/root/vault` (50 MB, 1 065 notes) at `/tmp/candidate-engine/vault`, served by its own
  daemon with its own `HOME` and socket (`tools/ranking/candidate_engine.py`), so the live daemon and the vault were
  never touched;
- command: `BRAIN_BIN=…/debug/2brain BRAIN_HOME=/tmp/candidate-engine/home python3
  tools/ranking/tasks_eval.py --socket … --vault /tmp/candidate-engine/vault`.

The run is deterministic: a second run after stopping and restarting the engine produced identical JSON.

| class | passed | what it measures |
|---|---|---|
| exact | 1/2 | an identifier finds its note first |
| paraphrase | 2/3 | a question in other words |
| ru-cross | 1/1 | a Russian question about Russian material |
| multi-hop | 2/2 | a question whose evidence is a note's argument |
| ambiguous | 1/1 | any of several notes is accepted |
| absent | 0/3 | the base should not answer |
| **word-form** | **1/1** | a plural finds the singular the corpus holds |
| **exact-and** | **1/1** | `exact` is a conjunction of terms, not a phrase |
| **mixed-language** | **1/2** | a note whose body is in the other language |
| **hub-vs-atomic** | **0/1** | the atomic note outranks the map |
| **bilingual** | **0/1** | a question reaches its cross-language sibling |

The old six classes did not move: `exact` 1/2, `paraphrase` 2/3, `ru-cross` 1/1, `multi-hop` 2/2, `ambiguous`
1/1 (rank 9, weak), `absent` 0/3 — the same numbers as the first run. That is the check that the new classes add
information instead of relabelling the old ones. The misses that remain are the ones already documented above:
`ReasoningBank` returns the agentdb note at rank 2 behind a note whose filename also carries the identifier, the
skills paraphrase is still outside the top ten, and `absent` is a reporting target rather than a ranking one.

Cost: **0.4 s** for the eighteen queries on a warm candidate engine, and **7.9 s** for a cold start (copy the vault,
start the daemon, open the socket). A full run from nothing is therefore well under ten seconds plus the build,
which is the expensive half and is cached.

### 1. word-form — pass, and the fix is pinned

Query `capsules` (class `word-form`). The corpus holds only the singular, in two notes: Evolver's Capsule (a gene
plus one concrete case) and StatCraft's CapsuleCraft `.capsule` knowledge base. At HEAD the query returns both, and
`capsule` returns the same two with the same scores (35.89, 12.37):

```
ok   word-form   rank 1 of 2   capsules
```

Before `c3c7ee4` this was 0 hits. The task passes for the right reason — the engine retries an absent term as a word
form the dictionary holds — and no query that already worked moved: the committed fix says `capsule`-style answers
keep their scores, and this run is consistent with that.

### 2. exact-and — pass, and the documented meaning is the one measured

Query `as of`, `exact: true` (class `exact-and`). The bitemporal note documents AS OF queries and ranks first; the
page's `total` is 218, i.e. every note that holds both words anywhere:

```
ok   exact-and   rank 1 weak of 218   as of
```

The task also carries `min_total: 100`, because a rank alone cannot tell the two behaviours apart: the same note
would be first under an adjacency rule. A literal scan of the vault's 1 065 markdown files finds 236 notes holding
both words and **7** holding the pair `as of` adjacently, so a phrase implementation would return about seven hits
and fail the row, while the documented AND returns 218. The tool description now says the same thing ("it does not
require them adjacent, so a literal phrase is tb_grep's job"), and this row keeps description and behaviour together.

### 3. mixed-language — **defect**, in the `min_should_match` floor

Query `oracle matrix tests` against the note that answers the theme: `f/patterns/pattern-oracle-drift-ci.md`, a
pattern for running one test corpus against a live oracle in CI and diffing the two outputs.

| run | total | where the note is |
|---|---|---|
| default (`min_should_match` = 2 for a three-word query) | 5 | absent |
| `min_should_match: 1` | 83 | rank 3, score 25.62, 1 matched term, `weak: false` |

The five default hits are `inbox/insight-verifiable-artifact-beats-green-gates.md` 29.96,
`inbox/matsakis-agentic-links-deepdive.md` 16.68, `inbox/egm-vs-superpowers-vs-compound-engineering.md` 7.77,
`inbox/exact-discipline-deep.md` 7.08 and
`inbox/allspark-applicability-analysis-15-research-sources-full-disposition.md` 3.61 — every one matching 2 of 3
terms, none of them about oracle testing. Read in the files, four of the five hold `oracle` and `tests` somewhere and
none holds `matrix`; the fifth holds `matrix` and `tests`. So the floor admitted notes that happen to mention the
words while dropping the note whose subject is an oracle test corpus.

**Verdict: a genuine defect of the floor rule.** The floor is applied *after* scoring, and the scoring already puts
the note third of 83 once it is allowed in: the ranker can separate the topical note from the other single-term hits,
and a count applied before the page hides it anyway. The cost is asymmetric — five notes are returned, none of them
about oracle testing, and the caller gets no signal that the answer was filtered rather than absent. The one term
the note does match is specific (`oracle` occurs in 38 of 1 065 notes, 3.6%, and the hit is not `weak`), but the
floor is a count and cannot tell "mentions two query words" from "is about the query": four of the five admitted
hits hold `oracle` and `tests` somewhere in the file.

The repair is the one D3 names — keep the note when `matched_terms >= min_should_match` **or** `best_idf >=
RARE_IDF` — and the class has two rows precisely so the change can be judged: the default row must move to pass
while the `min_should_match: 1` row stays at rank 3.

The counter-argument, stated because it is real: the query carries `matrix`, a word this corpus does not use in that
sense, and `tests`, which the note has only as `тестов`. A count-only floor is not being unreasonable. What makes
this a defect is the direction of the miss: the engine ranks the right note well and then drops it on a count, while
returning notes that only mention the words.

#### The rare-term override, measured (2026-11-16)

The rule registered above is now the behaviour: `matched >= msm || best_idf >= RARE_IDF`, with `RARE_IDF = 2.0`
the constant the `weak` flag already uses. The floor stayed a count, and one matched term at or above that line
satisfies it alone. `exact` is untouched — it takes the conjunction path (`exact()`), so it still requires every
term; the escape widens only `min_should_match`.

Measured on a candidate engine (debug build of `tb-cli` from the working tree, `tools/ranking/candidate_engine.py`)
over a `cp -a` copy of the live vault, the before and the after runs served from the **same copy**, with the
eighteen tasks in `tools/ranking/tasks.json`:

| class | before | after | what the after column holds |
|---|---|---|---|
| exact | 1/2 | 1/2 | `ReasoningBank` still rank 2 |
| paraphrase | 2/3 | 2/3 | the skills question still outside the top ten |
| ru-cross | 1/1 | 1/1 | rank 1 |
| multi-hop | 2/2 | 2/2 | rank 1 on both rows |
| ambiguous | 1/1 | 1/1 | rank 9, `weak` — unchanged |
| absent | 0/3 | **0/3** | unchanged; the pages widened, the answers did not |
| word-form | 1/1 | 1/1 | rank 1, two candidates |
| exact-and | 1/1 | 1/1 | 218 candidates, unchanged |
| **mixed-language** | **1/2** | **2/2** | the defect this measures |
| hub-vs-atomic | 0/1 | 0/1 | rank 8 |
| bilingual | 0/1 | 0/1 | still outside the page |

`oracle matrix tests` at the default floor: **5 hits → 83**, and `f/patterns/pattern-oracle-drift-ci.md` at
**rank 3**, score 25.62, one matched term, `weak: false` — the position the `min_should_match: 1` row already held
before the change, which is the whole point: the ranking knew, and the count hid it. The default row and the
`min_should_match: 1` row now return an identical page (83 candidates, same order, same scores), so the default
floor costs nothing on this query that the OR would have found.

The absent class moved only in the way the rule allows: its candidate counts grew (46 → 85, 121 → 122, 433 → 457)
because notes matching a single rare term are no longer dropped, while all three top hits are the same notes with
the same matched-term counts (5, 2, 5) and none is `weak`. **0/3 is unchanged.** Every other class keeps its rank;
where a candidate count widened it widened only outside the page (`NVDA translation addon blind user` 8 → 148,
`event log as source of truth` 329 → 369, `перехват аргументов tool-call до исполнения` 166 → 193), which is the
same escape at work on a two-term default and answers no task differently.

**What the rule does not fix:** it repairs a page that was filtered, not an answer that is wrong — rarity still
cannot tell a rare-and-unrelated word from a rare-and-central one, so the three `absent` questions keep returning
confident multi-term pages, which stays the reporting target §2 names rather than another threshold.

### 4. hub-vs-atomic — **defect**, the title is not a ranking field

Query `event log as source of truth`, which is the atomic note's own title and the rule it states (append-only log
primary; graph and memory are deterministic projections):

```
MISS hub-vs-atomic   rank 8 of 329   event log as source of truth
1 knowledge/kutha-temporal-graphs.md                       75.24  6 terms  (type: moc, the topic map)
8 knowledge/event-log-as-source-of-truth-agent-memory.md   36.63  6 terms
```

The hub is a legitimate second hit: it lists the note and repeats the phrase in its description, which is weighted
3× the body. But it is a map, and a map should not be the first answer to what the event-log rule is. The atomic
note matches **all six** terms and still sits behind notes matching four and five. The repair is the one D7 names —
a title-match boost plus a down-weight for `type: moc` when a non-MOC note matched the same terms. `hub-vs-atomic`
is the measurement that will say whether it works: it goes 0/1 → 1/1 only when the atomic note is first.

**Experiment, 2026-11-17 — the title field is rejected.** Six builds were measured against one frozen copy of the
vault (1 080 notes, vault HEAD `9f59787`, `/tmp/s1/vault`) with the same 18-task set; the baseline reproduced this
page (hub rank 8, 376 candidates against the page's 329, the difference being the notes added since). Every variant
added a third index field — the title, taken as the file stem with `-`/`_` as spaces — and optionally a `type: moc`
discount of ×0.4 applied when a non-map note matched at least as many terms:

| variant | hub-vs-atomic | exact-and | ambiguous | exact `ReasoningBank` rank | paraphrase |
|---|---|---|---|---|---|
| **baseline**, no title field | 0/1, rank 8 | **1/1**, rank 1 | **1/1**, rank 9 | 2 | 2/3 |
| title ×3.0 | **1/1** | 0/1, rank 4 | 0/1 | 2 | 2/3 |
| title ×2.0 | **1/1** | 0/1, rank 3 | 0/1 | 2 | 2/3 |
| title ×1.0 | 0/1, rank 5 | **1/1** | 0/1 | 2 | 2/3 |
| title ×1.0 + MOC ×0.4 | 0/1, rank 4 | **1/1** | 0/1 | 5 | 2/3 |
| title ×3.0 + rare-term gate + MOC ×0.4 | **1/1** | **1/1** | 0/1 | 6 | 2/3 |

The rule written above is what rejects all six: a title weight large enough to lift the atomic note also lifts notes
whose titles contain `as`/`of` (the `exact-and` task 1/1 → 0/1), and **any** third field flips `ambiguous`, because
a title term counts as matched evidence even when its score weight is zero — it moves the `min_should_match` floor
and the coverage factor, not just the score. The rarity gate repairs `exact-and` but then moves the `exact`
`ReasoningBank` task from rank 2 to 6 through the MOC discount.

What this does not test: a **post-ranking** title bonus that never enters the index, so it cannot change `matched`,
the floor or the candidate set, gated on the query's rare terms. That is the next hypothesis and it must be
pre-registered before it runs; the six builds above are kept as the evidence that the indexing route is closed.

**Next hypothesis, registered 2026-11-17 — post-ranking title coverage.** The title is not indexed: after the
two-field ranking is complete, each hit whose path stem contains a query term whose idf is at least `RARE_IDF` is
multiplied by `1 + TITLE_COVERAGE × (rare terms in the title ÷ rare query terms)²`, with `TITLE_COVERAGE = 1.0`.
The constants are registered before the run, not tuned after it. Gate: `hub-vs-atomic` 1/1; the `exact` and
`paraphrase` tasks keep their ranks; `exact-and` stays 1/1; `ambiguous` stays 1/1 under the widened label. Any move
rejects it, and the measurement runs on the same frozen copy as the six rejected builds.

**Run 2026-11-17 — rejected, the closest miss.** Hub moved **8 → 2** (scores: the map 75.11, the atomic note
73.24), and every other class stayed exactly where it was: `exact` 1/2, `exact-and` 1/1, `ambiguous` 1/1,
`paraphrase` 2/3. The gate required rank 1, so the hypothesis is rejected as registered; the remaining gap is
the `type: moc` map that matches the same six terms.

**Next registration, same day — title coverage plus the map discount.** The same post-ranking multiplier, plus
`MAP_WEIGHT = 0.5` applied post-ranking to a `type: moc` hit when a non-map hit matched at least as many terms.
Both constants are fixed before the run. Gate: hub 1/1; the `exact` and `paraphrase` ranks unchanged; `exact-and`
1/1; `ambiguous` 1/1; `absent` 0/3. Any move rejects the pair.

**Run 2026-11-17 — rejected on the `exact` guard.** `hub-vs-atomic` reached **1/1**, but `exact ReasoningBank`
moved **2 → 6**: the expected answer, `agentdb-architecture`, is itself a `type: moc`, so a blanket map discount
demotes the exact answer. A map is the answer to a conjunction lookup and a hub for a ranked one.

**Next registration, same day — the map yield only on the ranked path.** The same pair, with `yield_maps`
skipped when `exact == true`: `exact` is a lookup whose prefix (`agentdb-architecture`) may be a map, while the
ranked path answers a question and a hub there is a pointer, not evidence. Gate unchanged, and it now names the
`exact` ranks explicitly: hub 1/1; `exact` 1/2 with the `ReasoningBank` rank at 2; `exact-and` 1/1; `ambiguous`
1/1; `paraphrase` 2/3; `absent` 0/3.

**Run 2026-11-17 — rejected, the wire flag is the wrong boundary.** `ReasoningBank` stayed at rank 6: the task is
called `exact` because its expected note states the term exactly, not because the request sets `exact: true`.
The query is one token, and the map it should find is the answer. (The first attempt at this run measured the
previous binary: a failed `cargo build` piped through `tail` printed no error, and the copy that followed copied
the stale artifact. The run was discarded and repeated; every number above names a binary whose build exit code
was checked.)

**Next registration, same day — the map yield only for a multi-term query.** A one- or two-term query is a
lookup; at three distinct terms the read answers a question, and a hub there is a pointer. The rule counts the
query's distinct terms (`distinct_terms`), not the wire flag. Gate: hub 1/1; `exact` 1/2 with the
`ReasoningBank` rank at 2; `exact-and` 1/1; `ambiguous` 1/1; `paraphrase` 2/3; `absent` 0/3.

**Run 2026-11-17 — accepted.** The gate passed: `hub-vs-atomic` **1/1 at rank 1** (from 8), `exact` 1/2 with
`ReasoningBank` back at rank 2, `exact-and` 1/1, `ambiguous` 1/1, `paraphrase` 2/3 with the same ranks,
`absent` 0/3, `multi-hop` 2/2, `mixed-language` 2/2, `bilingual` 0/1, `word-form` 0/1, `ru-cross` 1/1. The two
multipliers ship together: title coverage (≤2×) and the multi-term map yield (½). The two rejected boundaries —
a blanket map discount, and the wire `exact` flag — are what pin the rule: a lookup keeps its map, a question
does not.

### 5. bilingual — **defect**, in the corpus, not in the ranker

The Russian question — `битемпоральная модель ребра отслеживает валидность и время загрузки отдельно` — asks what
`knowledge/zeroclaw-bi-temporal-edge-model-tracks-validity-and-ingestion-separately.md` says word for word: every
edge carries four timestamps, and validity in the world is tracked separately from ingestion time. The note does not
appear in the top **100** of 346 hits. The top hit is `inbox/pattern-blocker-rubric-grading.md` at 32.10 with 3 of 9
terms; the Russian sibling `knowledge/bitemporal-valid-transaction-time.md` ranks third (20.95) because it shares
`битемпоральн…`/`валидност…` — it is a different note, about the general model rather than the edge model.

**Verdict: a real failure, and lexical search cannot fix it** — the two notes share no token. D1's smallest fix is
corpus-side: an English `description:` on the note (that field is weighted 3× the body) or an `aliases:` field
indexed with it. The task stays in the set as the measurement of that convention rather than of a ranker change: it
passes when the note enters the page, whatever the mechanism.

### What a fix has to preserve

In the shape the earlier experiments used, written down before the change:

- the rare-term override (for `mixed-language`) must take that class to 2/2, must not raise `absent` (its hits
  already clear the floor on two terms, so the override cannot add one there), and must leave `word-form`,
  `exact-and`, `hub-vs-atomic` and `bilingual` where they are — **met 2026-11-16**: 1/2 → 2/2, `absent` 0/3,
  and every other class unchanged (§3, the rare-term override);
- the title boost and MOC down-weight (for `hub-vs-atomic`) must not move any `exact` or `paraphrase` task down;
- the description convention (for `bilingual`) is a vault edit, so it is measured on a fresh copy and cannot be
  checked by the engine's tests at all.

Any of them that misses its row is rejected and published here, like the stopword experiment and the freshness
prior.

## What a paired bootstrap can and cannot say about eighteen tasks

`ruvector-crates-2026-11-16.md` entry 2 proposes a paired bootstrap with three outcomes over a graded query set, and
warns that its source repository cannot be used as evidence: that harness synthesizes BM25 tokens from vector
components and reports `recall_at_1`, `recall_at_10` and `recall_at_100` as the same number. The lesson is not "do not
measure" but "validate the instrument against a ranker that is worse by construction before reading its verdict".
`tools/ranking/bootstrap.py` is the instrument; this section is its validation and the one real pair it was run on.

The tool takes two `tasks_eval.py --json` runs over the same queries and vault, pairs them by position after checking
that the query/class sequence is identical (the `mixed-language` class has two rows with the same query and different
`args`, so the JSON cannot pair on its own keys), and resamples the per-task deltas 2 000 times. It prints one of
three words and never a fourth: **better** when the 95% interval's lower bound is above zero, **worse** when the upper
bound is below zero, otherwise **inconclusive**. The RNG is seeded from the deltas themselves, so a verdict is
reproducible from the two files. Beside the interval it prints the exact McNemar p-value, the tasks whose pass/fail
changed (named), and the smallest net gain this n could certify.

### The two controls

Both ran on the candidate build that carries `006ac0b`, over a `cp -a` copy of the live vault
(`tools/ranking/candidate_engine.py`), with `tasks_eval.py` unmodified. That build's run reproduced the accepted
`after.json` byte for byte, so the real pair below is the one the change was accepted on.

| control | before | after | changed | delta | 95% interval | verdict | McNemar |
|---|---|---|---|---|---|---|---|
| self-pair, two processes | 11/18 | 11/18 | 0 | 0 | [0.000, 0.000] | **inconclusive** | p = 1.000 |
| page order reversed | 11/18 | 5/18 | 8 | −6 (−0.333) | [−0.611, −0.056] | **worse** | p = 0.070 |

The self-pair is the check that matters most: a statistic that calls identical input "better" is broken, and this one
cannot. Its interval is [0, 0] because every resample of a zero vector is zero, and the word for that is
`inconclusive` — the tool cannot certify no effect any more than it can certify an effect.

The degraded ranker is a stdio proxy (`tools/ranking/degraded_ranker.py`) that forwards every MCP message unchanged
and reverses the `matches` array of a `tb_search` reply. The engine, the vault and `tasks_eval.py` are untouched, so
the only difference between the arms is the order of the page. It loses seven tasks and gains one: the seven are every
row that passed at rank 1 (seven rows in five classes), and the gain is `absent` — reversed, the Kafka page's head is
`inbox/bui-blender-ui-library.md` with one matched term, and one term is that class' pass condition. That gain is
the first useful thing the control says: **the pass count is not monotone in ranking quality**, because the `absent`
class rewards a page whose head is a thin match. The verdict is `worse`, which is what it must be.

The second thing it says is a caveat on the tool. The bootstrap interval excludes zero while exact McNemar does not
(p = 0.070 on seven losses to one gain). Both read the same eight flips and answer different questions: the
percentile interval is an interval on the *mean delta* and treats the ten unchanged tasks as evidence about that
mean, while McNemar asks only about the direction of the discordant pairs. For binary data the bootstrap is the more
liberal of the two, and here it is liberal by enough to change a 5% decision. The output prints both and says so; a
gate that reads only the verdict word reads the weaker signal.

### The change we shipped today

| n | before | after | changed | delta | 95% interval | verdict | McNemar |
|---|---|---|---|---|---|---|---|
| 18 | 10/18 | 11/18 | 1 | +1 (+0.056) | [0.000, 0.167] = [0, +3] tasks | **inconclusive** | p = 1.000 |

`006ac0b` gives `min_should_match` a rare-term escape: a match survives the floor when one matched term has
`best_idf >= RARE_IDF`. The one changed task is the default `mixed-language` row — `oracle matrix tests` goes from 5
candidates with the right note absent to 83 candidates with it at rank 3. Every other class keeps its verdict.
Eleven more rows moved in rank or candidate count and contribute nothing to the statistic; that is the metric's
blindness, not a null result.

The arithmetic behind `inconclusive` is short. A resample of 18 tasks misses a single flipped task with probability
(17/18)^18 = 0.357, so 36% of resamples of the observed delta vector have a mean of exactly zero, and a 95%
percentile interval must include zero. One flip cannot be separated from none at n=18 by this tool or by any paired
test: exact McNemar on one discordant pair is p = 1.000.

What the interval does say is worth as much as what it refuses. Its upper bound is +3 tasks, so the data are
consistent with a gain anywhere from zero to 16.7 points. The honest sentence is not "a small improvement" but "an
improvement too small for this set to separate from none, and no larger than three tasks". The lower bound is 0
because that is the smallest value the metric can produce, not because the change was measured as neutral.

What the number cannot support, in order of how easy it is to forget:

- **It is not a statement about questions in general.** The rows are a hand-built diagnostic set, not a sample of a
  population. The interval describes how far the pass count would move if these same 18 tasks were drawn again — a
  null model for noise, not an estimate of user-visible quality.
- **The rows are not independent, and 18 overstates n.** There are 11 classes; `mixed-language` is two rows of the
  same query with different flags; the three `absent` rows share a pass rule that a refusing ranker satisfies.
- **A pass count is the coarsest available metric.** It cannot see rank 3 becoming rank 2, which is what most of the
  eleven moved rows did. A change that improves ranking without moving a task across its threshold scores zero.
- **The tool's resolution is four net flips, and it barely moves with n.** (1−k/18)^18 first falls below 0.025 at
  k=4, so `resolution` prints 4; n=40 and n=100 need the same 4, which is 10% and 4% of those sets. Growing the set
  buys resolution in *rate*, linearly; it never buys resolution in *number of tasks*, because a resample of n tasks
  misses four of them with probability about e^−4. And a `better` verdict at four flips is still short of the exact
  test (McNemar p = 0.125), which is the same liberality the control showed.
- **A bootstrap run after the change cannot supply the hypothesis.** It tests the difference in pass counts, not the
  mechanism the change was made for.

### The criterion, and what to gate on next

The acceptance rule for `006ac0b` was pre-registered and class-level: `mixed-language` 2/2, `absent` 0/3, every other
class unmoved. It was the right instrument, and the bootstrap does not change that — it explains the limit:

- the criterion is a **defect detector**. It names a mechanism (a floor that dropped a note the ranker had already
  placed third of 83), the two rows that show it, and the guard the fix must not break. Its evidence is direct: 5
  candidates to 83, rank 3, `weak: false`, and the same three `absent` top hits at 5, 2 and 5 matched terms. A
  pass-count aggregate cannot see any of that, and at n=18 it could not certify a one-task move either.
- the bootstrap is a **gain estimator**, and at this n it is only a screen. With a resolution of four tasks,
  requiring `better` would reject every ranking change of the size we actually ship, on the day it is measured.

So the split that should hold until the set changes shape: **gate on task-level criteria, publish the bootstrap
beside them, and treat `worse` as a veto even when the class criterion passes.** That veto is the one thing the
statistic adds to the current process, and the degraded control is a pair it calls `worse`.

The bootstrap becomes the gate when its resolution falls below the effect we intend to claim. With pass/fail that is
about 100 independent tasks for a four-point effect, and the reason is the metric rather than the statistic: four
discordant pairs is the smallest number a 95% interval on a mean can separate at any n. A graded metric (nDCG@10 or
MRR over two to four relevance grades, as entry 2 proposes) makes the per-task value continuous, so the interval uses
every row instead of only the rows that crossed a threshold. That, not a larger pass/fail set, is what would make a
statistical gate worth installing.

Reproduction:

```sh
tools/ranking/candidate_engine.py --start                                  # vault copy, its own socket
BRAIN_BIN=<candidate adapter> python3 tools/ranking/tasks_eval.py --json \
  --socket <socket> --vault <copy> > a.json
BRAIN_BIN=tools/ranking/degraded_ranker.py BRAIN_REAL_ADAPTER=<adapter> \
  python3 tools/ranking/tasks_eval.py --json --socket <socket> --vault <copy> > reversed.json
python3 tools/ranking/bootstrap.py a.json a.json            # inconclusive, [0.000, 0.000]
python3 tools/ranking/bootstrap.py a.json reversed.json     # worse, [−0.611, −0.056]
python3 tools/ranking/bootstrap.py before.json after.json   # the pair above
python3 tools/ranking/bootstrap.py --selftest               # the arithmetic, on known shapes
```

## A graded view of the same eighteen tasks

`tools/ranking/graded.py` scores the runs `tasks_eval.py` already produces, by how far evidence moved
instead of whether a task crossed its threshold. It reads `tools/ranking/tasks.json` for the labels and one or
two `tasks_eval.py --json` records for the pages (the record carries the whole page, because a note at rank 4
is not a miss). Deterministic, standard library only; `--selftest` checks the arithmetic on shapes whose
answer is known by hand (14/14).

### The formula, and why the gain is linear

For one query, the page `d_1 … d_5` and a grade `g ∈ {2, 1, 0}` per note:

```
DCG@5  = Σ_{i=1..5}  g(d_i) / log2(i + 1)
IDCG@5 = Σ_{i=1..5}  g*_i   / log2(i + 1)      g* = the task's grades, sorted descending
nDCG@5 = DCG@5 / IDCG@5                        undefined when IDCG@5 = 0
```

- **Linear gain** (`gain = g`) is trec_eval's default: the qrels relevance value *is* the gain, and
  `log2(rank + 1)` is the discount. The exponential variant `2^g − 1` was rejected for a measured reason:
  with two grade-2 notes it gives the first 3:1 against the second and would hide the multi-hop miss the
  grades exist to expose.
- **A true IDCG**, not 1. That is the whole repair: the ideal page is the task's labels sorted, so a second
  graded note can earn credit, and two runs that put the *same* first note at the same rank but differ in
  the second note now get different scores.
- **IDCG = 0 is reported, not scored zero.** Every `absent` row has no grade, so it is excluded and counted
  (`absent 3`) rather than averaged in as a failure.
- **Cutoff 5.** The graded signal is the top of the page. A grade-2 note at rank 6–10 scores zero nDCG@5
  while MRR still gives it 1/6, and the report prints both so the cost is visible.
- **MRR is unchanged in definition**: 1/rank of the first **grade-2** path, 0 when no answer is in the page.
  Grade 1 is support, not an answer. That keeps the MRR column of the earlier tables comparable, and it is
  what lets the two numbers disagree.
- **Rank distance** is that answer's rank − 1, linear.

### The grades the set can support

`tasks.json` keeps `evidence`/`accept` as the answer set — every path there is grade 2, the label's existing
claim — and gains an optional `support` list for grade-1 notes. `support` is a separate field, so
`tasks_eval.py`'s pass rules (pre-registered before this metric existed) are untouched.

**Two tasks carry a grade-1 label: `hub-vs-atomic` and `bilingual`.** Both were already described as support
in their own `why` text before the field existed:

| task | grade 2 (answers) | grade 1 (support) | why it is support |
|---|---|---|---|
| hub-vs-atomic | `knowledge/event-log-as-source-of-truth-agent-memory.md` | `knowledge/kutha-temporal-graphs.md` | the map states the rule in one clause (*append-only event log = source of truth; the working graph is a deterministic fold*) but not the argument; the row's own `why` says "a map is not the answer to what the event-log rule is" |
| bilingual | `knowledge/zeroclaw-bi-temporal-edge-model-tracks-validity-and-ingestion-separately.md` | `knowledge/bitemporal-valid-transaction-time.md` | the general bitemporal model the edge note specialises — the decision behind the rule; the row's `why` calls it "a different note (the general model, not the edge model)" |

**Thirteen tasks stay binary, and that is a statement about the task, not a gap in the tooling:**

- `exact` (2), `ru-cross`, `exact-and`, `word-form`, `mixed-language` (2): one note answers, and the next
  hits are neighbouring subjects rather than steps a reader needs. `capsules` labels two notes grade 2
  because the query is ambiguous between two senses, not because one supports the other.
- `paraphrase` (3): the second hits are a mention (the skills question's `jev-context-curation-pattern`) or a
  second example (`rem-cycle-dreaming…` is another nightly consolidation, not the 3:00 job).
- `multi-hop` (2): the class name says the evidence is an *argument*, not that two notes are required. For
  Zettelkasten the 2nd and 3rd hits (`amem-search-agentic-graph-traversal`, `amem-zettelkasten-retroactive-
  evolution`) repeat the same paper, and the answer note already carries both the write-time and retrieval
  halves; a second grade-2 would be a duplicate. For 2FA the second hit is another project's privacy layer
  (Meta PrivacyLib). **This corpus has no task whose answer requires a second note** — that is the honest
  answer to where the multi-hop second hop is.
- `ambiguous`: all three `accept` paths are grade 2 by the label's own words ("any of these passes").
- `absent` (3): no labels, IDCG = 0, excluded from the average and counted.

Two rules kept the grades honest:

- **No duplicate answers.** A note that repeats the answer (the A-MEM sibling notes) is not added as an
  alternative: with a true IDCG, two required copies would give a page holding only one of them half marks.
  A path is graded only if a reader would want it *in the ideal top five*.
- **The grades follow the label, and the semantic pass is the check.** Both grade-1 notes are `P`
  ("partially answers") in `semantic-judgements-2026-11-16.json`; no `A` note on those pages was added,
  because the ones that exist are duplicates (above). The two places where the semantic pass disagrees with
  the label — `T01`'s labelled answer is a MOC the pass calls `P` while the definition note at rank 5 is
  unlabelled, and `T09`'s `a-mem` `accept` path is `P` — are left as the label has them and reported under
  *what the metric cannot see*, because silently re-grading a pre-registered answer set would move the
  numbers it was compared on.

### The current candidate build

Measured 2026-11-16 on the candidate engine: debug `tb-cli` built from this working tree at `8f87a37`, the
adapter built from `tb-mcp-rs`, a `cp -a` copy of `/root/vault` served by its own daemon with its own `HOME`
(`tools/ranking/candidate_engine.py`). n = 18, 15 scored, **2 tasks with no answer in the page**.

| class | n | nDCG@5 | MRR | first answer at 1 | zero hits | mean distance |
|---|---:|---:|---:|---:|---:|---:|
| ambiguous | 1 | 0.000 | 0.111 | 0 | 0 | 8.0 |
| bilingual | 1 | 0.190 | 0.000 | 0 | 1 | — |
| exact | 2 | 0.815 | 0.750 | 1 | 0 | 0.5 |
| exact-and | 1 | 1.000 | 1.000 | 1 | 0 | 0.0 |
| hub-vs-atomic | 1 | 0.380 | 0.125 | 0 | 0 | 7.0 |
| mixed-language | 2 | 0.500 | 0.333 | 0 | 0 | 2.0 |
| multi-hop | 2 | 1.000 | 1.000 | 2 | 0 | 0.0 |
| paraphrase | 3 | 0.667 | 0.667 | 2 | 1 | 0.0 |
| ru-cross | 1 | 1.000 | 1.000 | 1 | 0 | 0.0 |
| word-form | 1 | 1.000 | 1.000 | 1 | 0 | 0.0 |
| **all scored** | **15** | **0.680** | **0.627** | **8** | **2** | **1.538** |

Macro over classes: nDCG@5 0.655, MRR 0.599. The pass count from the same run is 11/18.

The aggregate barely moved from the binary reading (0.683 → 0.680) and that is expected, not disappointing:
**13 of the 15 scored rows are binary**, and for a task with one grade-2 note and a true IDCG, nDCG@5 is
`1/log2(rank+1)` — identical to the old number. Only three rows read differently, and they separate the two
repairs: **grades** move `bilingual` 0.000 → 0.190 (its support note at rank 3 now earns credit) and
`hub-vs-atomic` 0.315 → 0.380 (the map at rank 1 earns less than a full answer, more than nothing), while
the **cutoff** moves `ambiguous` 0.301 → 0.000 (its accepted answer sits at rank 9, outside the five; MRR
still sees it at 0.111). The repair is real and local; the set needs more graded rows before the aggregate
can move.

### The validation pair

| control | nDCG@5 | MRR | passes | first at 1 | zero hits | verdict |
|---|---|---|---:|---:|---:|---|
| self-pair, two processes | 0.680 → 0.680 | 0.627 → 0.627 | 11 → 11 | 8 → 8 | 2 → 2 | **identical** |
| page order reversed | 0.680 → **0.112** | 0.627 → **0.193** | 11 → 5 | 8 → 1 | 2 → 2 | **worse** |

The self-pair is exact: identical scores, identical pages, 0/15 nonzero deltas.

The degraded ranker (`degraded_ranker.py`, which reverses every `tb_search` page) moves **nDCG@5 down 0.568
(−83.6 %)** and MRR down **0.434 (−69.2 %)**. The graded metric moves more than MRR in both absolute and
relative terms, which is the point of the change: the cutoff and the second label both bite. Twelve of the
fifteen rows moved their answer; the seventh thing to note is the row MRR cannot see:

- `bilingual` — the answer is absent in both arms, so MRR is 0.000 → 0.000, but the support note falls from
  rank 3 (nDCG 0.190) to rank 8 (0), and the graded view records a loss;
- `hub-vs-atomic` — the reversed page promotes the answer from rank 8 to rank 3, so **MRR improves
  0.125 → 0.333 while nDCG@5 stays 0.380**: the map, which was at rank 1, leaves the cutoff as the answer
  enters it. Two graded notes in a five-slot page are what the score rewards, and the reversal trades one
  for the other.

That second row is also a warning about reading either number alone: on this task the "degraded" ranker is
better by MRR. The arithmetic is not wrong; the task has two relevant notes, and the page holds one of them
either way.

### Application 1: today's change, `006ac0b`, on grades

Same corpus, same adapter, engine build the only difference: `tb-cli` from `006ac0b^` (`8454735`) in a
scratch worktree versus HEAD `8f87a37`.

| run | nDCG@5 | MRR | passes | first at 1 | zero hits |
|---|---|---:|---:|---:|---:|
| before `006ac0b` | 0.647 | 0.605 | 10/18 | 8/15 | 3/15 |
| after (HEAD `8f87a37`) | 0.680 | 0.627 | 11/18 | 8/15 | 2/15 |
| delta | **+0.033 (+5.2 %)** | +0.022 (+3.7 %) | +1 | 0 | −1 |

**The graded reading agrees with the binary one and does not turn it into evidence.** The whole change is
one row — `oracle matrix tests` at the default floor, absent → rank 3, nDCG 0.000 → 0.500 — and because that
task has one grade-2 note and the new rank is inside both the old and the new cutoff, the graded and binary
numbers coincide to three decimals. One of fifteen deltas is nonzero; the sd of the deltas is 0.129, and
0.033 is smaller than the smallest change this set can certify at any size below ~95 questions (§σ below).
The honest sentence is unchanged: a real, small, one-row improvement, not a measured gain.

### Application 2: the floor forced high

`tasks_eval.py --require-real-terms` sets `min_should_match` to the number of non-stopword terms.

| build | nDCG@5 | MRR | passes | first at 1 | zero hits | verdict |
|---|---|---:|---:|---:|---:|---|
| before `006ac0b` | 0.647 | 0.605 | 10/18 | 8/15 | 3/15 | — |
| … floor forced high | 0.434 | 0.406 | 8/18 | 5/15 | 6/15 | **worse** |
| delta | **−0.213 (−32.9 %)** | −0.199 (−32.9 %) | −2 | −3 | +3 | |
| current build | 0.680 | 0.627 | 11/18 | 8/15 | 2/15 | — |
| … floor forced high | 0.680 | 0.627 | 11/18 | 8/15 | 2/15 | **identical** |
| delta | 0.000 | 0.000 | 0 | 0 | 0 | |

The current-build row is the one to keep: **identical scores, different pages.** No answer moves and no
graded score changes; only the candidate set narrows (nine rows change `total`, and the two pages that were
rearranged do so below their evidence). That is `006ac0b` doing what it claimed, seen on grades as well as on
ranks.

### σ, and the smallest effect this set can see

The power arithmetic is Webber, Moffat & Zobel (2008): `δ = 2.486·σ/√n` for 80 % power at α = 0.05 one-sided.
Three σ values were measured, and only one of them belongs in the formula.

| quantity | value | what it measures |
|---|---|---|
| σ of the self-pair deltas | **0.000** (0/15 nonzero) | the noise floor: the engine and harness are deterministic, so repeating one build changes nothing |
| σ of nDCG@5 across tasks, one run | **0.391** | how much the metric varies from question to question — the only estimate not derived from an effect |
| σ of the per-task deltas, before → after `006ac0b` | **0.129** (1/15 nonzero) | the observed pair's own spread; it contains the effect it is meant to certify |

The self-pair σ is the answer to "run it twice and measure the noise", and it is useless in the formula: a
σ of zero makes every effect infinitely significant. What the formula's σ describes is not the repeatability
of one run but the spread of the per-question effect, so the headline uses the cross-task σ, which treats
the fifteen scored questions as a sample and is therefore conservative (it is also an unpaired bound: a
paired test would subtract the between-run correlation, which one deterministic pair cannot estimate). The
delta σ is reported because it is the only *observed* cross-build spread we have, and because it predicts
what a slightly larger set would buy — but it is contaminated by the effect, and the report says so.

| n | MDE at σ = 0.391 | MDE at σ = 0.129 |
|---:|---:|---:|
| 15 (the scored tasks) | 0.251 | 0.083 |
| **18 (this set)** | **0.229** | **0.076** |
| **160 (the size the coverage work is building)** | **0.077** | **0.025** |

**Withdrawn on second opinion (`docs/research/second-opinion-2026-11-16.md`, O1):** the constants in the table above are
not quotable. Weber's formula needs the σ of *paired deltas between comparable systems*; what was substituted is the
cross-task spread of one system over fifteen rows of a different set, and the delta σ (0.129) comes from a single pair and
contains the effect it was meant to certify. So:

- **σ is not measured yet.** The first honest estimate needs two graded runs over the *same* graded set, and today only two
  of fifteen rows carry grades at all.
- **no MDE number is quoted** — not 0.229, not 0.077, not "about 95 questions". The method (declare σ, derive n, name it
  with the claim) survives; the arithmetic that used a σ of the wrong type does not.
- **what stands unchanged**: the eighteen-task set cannot certify the size of change we ship. It can name a mechanism, show
  the row it moved, and veto a regression (the reversed control), and those are the jobs it should keep.

To make an MDE real, the steps are: grade all eighteen rows (or use the 160-question set with grades), run two builds over
the same rows, take the paired-delta σ, and only then state the n a given effect needs.

### What nDCG@5 still cannot see

- **An unlabelled answer.** `ReasoningBank` is the worked example: the label names
  `knowledge/agentdb-architecture.md`, a MOC that states the layer in one line, while the note that defines
  the four components ("ReasoningBank is the cognitive layer of AgentDB") sits at rank 5 and is not in the
  label set. A graded metric scores it 0 exactly like a wrong note. The same is true of `oracle matrix tests`
  — its "matrix" half is not in the corpus, so no grade can exist for it. This is the pooling bias the
  practices note names (§4.6), and grades do not fix it.
- **Abstention.** The three `absent` rows have IDCG = 0 and are excluded, so a ranker that loses six ranking
  tasks while gaining an `absent` pass reads as six losses here. The abstention measurement stays in
  `tasks_eval.py`.
- **Candidate-set width.** `min_total` is not a rank: a page that keeps its first hit while collapsing 218
  candidates to 7 scores the same. Application 2 shows it: identical scores, different pages.
- **Anything below the cutoff.** An accepted answer at rank 9 (`agent memory`) and an answer at rank 8
  (`hub-vs-atomic`) are zero nDCG@5 and nonzero MRR. That is why MRR is still printed beside it.
- **The grade definitions themselves.** "Needed as support" is a criterion, not a measurement, and there is
  no kappa against a second labeller. The two grade-1 labels are argued row by row above; a second reader
  could reasonably grade the A-MEM duplicates differently.
- **The weighting choice.** Linear gain with a true IDCG says two relevant notes in the page beat the answer
  alone at a lower rank: in the `--selftest` case, answer at 1 with support outside the page is 0.760, while
  support at 1 and answer at 2 is 0.860. That is a value judgement about recall, not a fact about relevance,
  and exponential gain would say the opposite.
- **Generality.** The rows are a hand-built diagnostic set, not a sample of a population. Everything above
  is true of these eighteen questions; the MDE says how far it is from a claim about questions in general.

### Reproduction

```sh
tools/ranking/candidate_engine.py --start                                  # vault copy, its own socket
BRAIN_BIN=<adapter> python3 tools/ranking/tasks_eval.py --json --socket <socket> --vault <copy> > a.json
BRAIN_BIN=tools/ranking/degraded_ranker.py BRAIN_REAL_ADAPTER=<adapter> \
  python3 tools/ranking/tasks_eval.py --json --socket <socket> --vault <copy> > reversed.json
python3 tools/ranking/graded.py a.json                     # the per-class table above
python3 tools/ranking/graded.py a.json a.json              # self-pair: identical, 0/15 deltas
python3 tools/ranking/graded.py a.json reversed.json       # page reversed: worse, nDCG −0.568 vs MRR −0.434
python3 tools/ranking/graded.py before.json after.json     # two builds, or two flags
python3 tools/ranking/graded.py --mde before.json after.json   # σ, the MDE at n and at --target-n
python3 tools/ranking/graded.py --selftest                 # the arithmetic, on known shapes (14/14)
```
