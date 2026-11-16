# Semantic judgements: does a returned note answer the question?

Written 2026-11-16. This is a **sample for a rubric**, not a benchmark: 18 questions come from `tools/ranking/tasks.json`, 24 were written for this pass, and every hit was judged by reading its text. The aim is to name what pass/fail at rank k cannot name — and to find out which failure shapes actually dominate on this corpus today.

**Bottom line.** Top hit answers the question for **16 of 42** (38%); 6 partially, 3 mention only, 17 are wrong. The five largest failure groups are: no abstention (8), the answer is on the page but below a weaker hit (7), non-note files outranking notes (3), cross-lingual questions with no shared content token (2), and a general note outranking the specific one (2). The single change with the largest measured effect is a claim-level check over the top 20 that both reranks and can return `no_evidence`: it fixes 7 promotions, 1 referent and 8 refusals, 16 of 42.


## What was measured

- **Engine**: `/root/2brain/tb-engine/target/debug/au`, sha256 `dc0170ce…61b7e4a`, `au 0.0.5`, built from the working tree; a compiler lane was active in the repository, so the binary is the debug build, not the installed release.
- **Corpus**: `cp -a /root/vault` at `/tmp/semantic-judge/engine/vault`, byte-identical to `/root/vault` when copied (`diff -rq` reported 0 differences). Own `HOME` with a `repos.yaml` whose vault entry points at the copy; own socket `/tmp/semantic-judge/engine/home/.2brain/au-engine/run/1d634491fd16e4ae.sock`.
- `/root/vault` was read only. No engine edits, no git.
- **Queries**: 42. The 18 from `tasks.json` were run with their own `args` (`exact`, `min_should_match`). The 24 written here are natural-language questions, including 5 that the base should not be able to answer.
- **Hits judged**: 207 (42 pages; one page, `capsules`, holds 2 hits). Every judgement carries the sentence it was made from, verified as a substring of the note.

Two controls were run to keep the diagnosis honest:

- `min_should_match=1` on four failing queries (MD, MI, T03, T18) **changed the candidate total and not the order**: 596→855, 378→586, 731→794, 351→784, with the same first four hits. So on this corpus the floor is not what hides these answers; the ranking is.
- `tasks.json` rows T15 and T16 (the same query with and without `min_should_match=1`) returned the **same page and the same total (83)**. The flag was not binding for that query at all.

## The rubric

Each hit is judged on its own, against the question as a reader would ask it. The unit of judgement is a sentence in the note, not the note's topic.

| level | test |
|---|---|
| **answers** | The note contains a sentence that states the asked fact, including any condition the question names. A one-line claim in a topic map counts if it states the fact; a link to a note that states it does not. |
| **partially answers** | The note states the fact but not the condition, or the condition but not the fact; or it gives one of two asked parts; or its own title and body give different numbers. |
| **mentions only** | The query's words occur and the note makes a claim about something else. A related-notes line (`[[target]] — short phrase`), a table label, or a bare term is a mention. |
| **wrong** | The note is about a different subject, or contradicts the corpus's answer (another project's number, another licence, a different repository). |

Three scoring conventions, stated once:

1. **Unanswerable questions.** For the 8 questions the base cannot answer, every hit is `wrong`: it asserts something about another subject. These 8 are also counted separately, because for them the correct answer shape is a refusal, not a hit.
2. **Ambiguous queries** (`agent memory`, `capsules`) are scored by what a reader wants from the term: an orientation for the ambiguous noun, the note that defines the term for the keyword. A single facet is `partially`.
3. **A hub that states the fact answers.** A topic map that says `append-only event log = source of truth` has answered the question in one clause, even though the atomic note argues it. The failure there is not the level of the hit but that the argument is seven places lower.

## The 42 questions

`levels` gives ranks 1-5 as **A** answers, **P** partially answers, **M** mentions only, **W** wrong. `target` is the note the question was written against (from `tasks.json` where present, otherwise the note whose title states the answer), found by searching to depth 100; `—` means it does not appear in the top 100. `A@` is the first `answers` hit on the page.

| id | question | class | levels | target | A@ | decisive sentence (rank 1) |
|---|---|---|---|---|---|---|
| T01 | ReasoningBank | exact | M P M P A | 2 | 5 | agentic-flow provides 66 pre-built specialized agents, each with autonomous self-learning powered by ReasoningBank |
| T02 | NVDA translation addon blind user | exact | A A M W W | 1 | 1 | I have been blind since birth. I use a screen reader program. |
| T03 | агент не вызывает доступные скиллы, если выбор оставлен на его усмотрение | paraphrase | W W W W W | — | — | Jev решает, что попадает в контекст и что остаётся |
| T04 | перехват аргументов tool-call до исполнения | ru-cross | A P M W W | 1 | 1 | Хук перед вызовом тула возвращает не только block/reason, но и input: подменённые аргументы ревалидируются по схеме |
| T05 | как Zettelkasten применяют к памяти агента | multi-hop | A P A W W | 1 | 1 | A-MEM (NeurIPS 2025) применяет Zettelkasten метод к памяти LLM агентов |
| T06 | Q-learning меняет топологию графа агентов | paraphrase | A M M W W | 1 | 1 | Модуль `ruvector-dag` реализует самообучающиеся ориентированные ациклические графы |
| T07 | ночная задача в 3:00 компилирует день в контекст | paraphrase | A P W W W | 1 | 1 | ночью в 3:00 агент перечитывает все диалоги за день |
| T08 | как держать 2FA-коды вне доступа агента | multi-hop | A M W W W | 1 | 1 | 2FA-коды, сбросы паролей и банковские уведомления физически не доходят до него |
| T09 | agent memory | ambiguous | P P M P P | — | — | OpenSage's memory agent provides natural language interface to graph memory |
| T10 | какая лицензия у Apache Kafka | absent | W W W W W | n/a | — | флаг weak в tb_search помечает «эти слова здесь встречаются» |
| T11 | сколько стоит подписка Netflix в Таиланде | absent | W W W W W | n/a | — | Netflix Prize убит linkage attack |
| T12 | the exact number of stars on the 2brain repository | absent | W W W W W | n/a | — | expo/exact internal discipline deep dive |
| T13 | capsules | word-form | P P | 1 | — | CapsuleCraft KB (.capsule/ 341 .eml-капсула с provenance |
| T14 | as of | exact-and | A M M W W | 1 | 1 | AS OF-запросы по обеим осям |
| T15 | oracle matrix tests | mixed-language | W M A M M | 3 | 3 | Зелёные гейты и повторное ревью не гарантируют корректность |
| T16 | oracle matrix tests | mixed-language | W M A M M | 3 | 3 | Зелёные гейты и повторное ревью не гарантируют корректность |
| T17 | event log as source of truth | hub-vs-atomic | P P P M M | 8 | — | append-only event log = source of truth; рабочий граф = детерминированный fold |
| T18 | битемпоральная модель ребра отслеживает валидность и время загрузки отдельно | bilingual | W W P W W | — | — | Двухслойный гейт оценки работы агента |
| MA | Why did ZeroClaw not adopt RuVector? | negation | P A A W M | 3 | 2 | When evaluating large multi-crate projects like RuVector (90+ crates), adopt only the validated components |
| MB | How many timestamps does a ZeroClaw edge carry, and what do they separate? | number+condition | A A W W W | 2 | 1 | facts are never deleted, only invalidated via 4 timestamps per edge |
| MC | At what entity-resolution accuracy does knowledge-graph reasoning start to break? | near-duplicate-numbers | P A M M A | 2 | 2 | When ER accuracy drops below 85%, errors compound |
| MD | When is building a graph index a mistake, and what should happen first? | paraphrase | W W W W W | — | — | First session guide — creating your first insight and building connections |
| ME | Do self-generated skills improve agent pass rates? | yes/no-negative | A M A W M | 1 | 1 | Curated Skills boost agent pass rate by 16pp while self-generated Skills degrade by 1pp |
| MF | What share of agent tasks remain unsolvable even with curated skills? | number | P A M M M | 2 | 2 | Even optimally curated Skills hit a ceiling |
| MG | Сколько операций в секунду выдаёт HNSW в ruvector и на какой задержке? | ru-cross | A M W W M | 1 | 1 | 52K вставок/сек и 0.019ms p50 задержку поиска |
| MH | How much RAM does an HNSW index need for 100k vectors at 1536 dimensions? | number | A M W M W | 1 | 1 | 100K vectors at 1536 dims потребляет 600MB+ |
| MI | How fast is ruvector's distance kernel across hardware targets? | number | M M M M M | 17 | — | HNSW indexes the hyperbolic vectors produced by claude-flow's embedding pipeline |
| MJ | Why was rten replaced for embeddings in ZeroClaw? | why | A A W M M | 1 | 1 | rten was 30-50x slower, lacked INT8 quantization |
| MK | When does the ZeroClaw agent loop trim history? | condition | A W M W M | 1 | 1 | Auto-Trims History at 50 Messages |
| ML | How many skill modules should be loaded at once, and why not more? | number+why | M A W W W | 2 | 2 | 2-3 focused domain Skills hit the sw |
| MM | What do skills save, and what problem comes with that saving? | two-part | W W W W M | — | — | Surface the most valuable next action |
| MN | Which sandbox mechanisms does auto-detection try, and in what order? | ordered-list | W W A W W | 3 | 3 | does not address consolidation--transient adaptation does not become permanent knowledge |
| MO | How many MCP tools does claude-flow expose, and across how many categories? | number+hub | A W A W M | 3 | 1 | MCP-first design with 19 tool categories |
| MP | What does the weak flag on a search answer mean? | definition | A M W W W | 1 | 1 | флаг оставлен как сигнал, а не как гейт |
| MQ | Why does search default to the own workspace? | why | W W W A W | 4 | 4 | Extract structured knowledge from source material |
| MR | How does ZeroClaw's retrieval combine two routes? | multi-hop | W A P W W | 2 | 2 | Store feedback in vector DB for similar-problem retrieval |
| MS | What does the vault say about Kafka Streams exactly-once guarantees? | absent | W W W W W | n/a | — | exactly-once handoffs между вендорами |
| MT | Как настроить max_connections в Postgres для 2brain? | absent-ru | W W W W W | n/a | — | Just use Postgres |
| MU | Which database does Notion use for its search index? | absent | W W W W W | n/a | — | the most ambitious open-source Rust vector database |
| MV | What is ZeroClaw's weekly npm download count? | absent | W W W W W | n/a | — | count incoming [[links]] |
| MW | Which cloud region hosts the 2brain API? | absent | W W W W W | n/a | — | trust region optimization |
| MX | What fraction of every API call is fixed overhead? | number | A W M W W | 1 | 1 | 73% of every API call is fixed overhead |

Row notes — what the levels alone do not say:

- **T01** — Top hit says agents learn *via* ReasoningBank; the note that defines it is rank 5.
- **T02** — Rank 2 is the category index, but it carries the same quote and the same claim.
- **T03** — English answer; every top hit is a Russian note sharing only function words. Target not in the top 100.
- **T04** — Rank 1 is the hook with input-override; rank 2 transforms arguments for repair, not interception.
- **T05** — Rank 1 answers; rank 2 gives one mechanism; rank 3 repeats the claim in English.
- **T06** — Rank 1 answers; ranks 2-4 mention Q-learning without the topology change.
- **T07** — Rank 1 answers; rank 2 is the sibling REM design, which distinguishes itself.
- **T08** — Rank 1 answers; rank 2 is a data-boundary module, no 2FA.
- **T09** — Ambiguous by construction: every hit is one facet, none orients. All five carry `weak=true`. The expected path is a near-duplicate of rank 2 (`amem-zettelkasten-retroactive-evolution`), which is why the target itself is not in the top 100.

- **T10** — Top hit is the note that quotes this very question as its example of a bad match.
- **T11** — Only "Netflix Prize" exists; the rest match "стоит".
- **T12** — "exact" matches a repository name; no star count exists in the corpus.
- **T13** — Only two notes hold "capsule"; both name the artifact without defining it.
- **T14** — The AS OF note answers; 2-5 use "as of" as a date stamp or "as-of facts".
- **T15** — Answer is rank 3; ranks 1-2 are verification and oracle-baseline notes, not oracle-matrix CI.
- **T16** — Same page as T15: `min_should_match=1` changed neither order nor total (83).
- **T17** — Hub answers in one clause; the atomic note that argues the rule is rank 8.
- **T18** — Russian question, English answer; Russian notes match "модель/валидность/отдельно".
- **MA** — The principle note ranks 1; the two notes that record 168 downloads / single-author risk rank 2-3.
- **MB** — Rank 1 answers with the four stamps; ranks 4-5 are skill files matching "timestamps".
- **MC** — Rank 1's title says 90%, its body says below 85%; ranks 2 and 5 say 85%.
- **MD** — All five are skill/manual files; the two notes with this claim are not in the top 100.
- **ME** — Rank 1 answers; rank 4 is Reflexion, not self-generated skills.
- **MF** — Rank 1 says "even curated Skills hit a ceiling"; the 19% note is rank 2.
- **MG** — Rank 1 answers with 52K inserts/sec and 0.019ms p50; 3-4 are the rejection decision.
- **MH** — Rank 1 answers; 3 and 5 are other systems' 1536-dim configs.
- **MI** — All five only link to the 16M ops/sec note; that note itself is rank 17.

- **MJ** — Ranks 1-2 answer (decision note, then its topic map); rank 3 is vault setup.
- **MK** — Rank 1 answers (50 messages); ranks 3 and 5 only link to it.
- **ML** — Rank 1 links to the answer; the answer is rank 2; ranks 3-5 are skill files.
- **MM** — Ranks 1-4 are two copies each of the next/refresh skills; the answer is absent.
- **MN** — Answer is rank 3; ranks 1-2 match "mechanisms" and "auto-detection".
- **MO** — Hub answers (171 tools, 19 categories); ranks 2 and 4 are agentic-flow's 213.
- **MP** — Rank 1 answers; ranks 3-4 are skill text using "weak" as a judgement word.
- **MQ** — Answer is rank 4, behind three skill files; notes-only it would be rank 1.
- **MR** — Answer is rank 2; rank 3 gives the pattern without ZeroClaw's adoption.
- **MS** — No Kafka note exists; "exactly-once" appears only about vendor handoffs.
- **MT** — Postgres notes rank 1-4; none mentions connection settings.
- **MU** — No Notion note; ranks 1-2 are vector databases.
- **MV** — Only "count" matches; ranks 1-2 are the same skill file twice.
- **MW** — Only "region", "2brain", "api" match; no hosting fact.
- **MX** — Rank 1 answers (73%); ranks 3-5 match "fixed" or "every API call".

## Level distribution

Top hit per question (what a reader sees first):

| level | all 42 questions | the 34 answerable questions |
|---|---:|---:|
| answers | 16 | 16 |
| partially answers | 6 | 6 |
| mentions only | 3 | 3 |
| wrong | 17 | 9 |

All judged hits (207): **34 answers, 19 partially, 44 mentions only, 110 wrong**. On the 167 hits belonging to answerable questions: 34 / 19 / 44 / 70 — a useful hit (answers or partially) is **31.7%** of the page.

The headline: **16 of 42 top hits answer the question (38%)**. On the 34 answerable questions it is 16/34 (47%). Rank-1 hit rate by path is not the same number: the expected note is rank 1 for 15 of 34, in the top 5 for 27, in the top 10 for 28, and outside the top 100 for 5.

Three measured facts that explain why the path metric flatters the engine:

1. **A note can be the right path and still not answer.** `T17`'s expected note is rank 8 while the hub that states the rule in one clause is rank 1; `T01`'s expected path is rank 2, but rank 2 is a topic-map line that names ReasoningBank's functions, and the note that defines it is rank 5.

2. **`matched_terms` does not separate an answer from a wrong hit.** Mean matched terms: answers 5.32, partially 3.53, mentions 4.73, wrong 4.75. The number counts query terms present, not the claim stated.

3. **`weak` fired on 11 of 207 hits and on 0 of the 8 unanswerable top hits.** The flag measures rarity in the corpus, and every unanswerable question matched a rare-but-irrelevant word (`apache`, `postgres`, `exact`, `2brain`). This is the failure the `weak` note predicted; the sample confirms it eight times.

Non-note files are **37 of 207 hits (17.9%)**: 34 `.claude/skills` or `ops/skills-archive` skill files, 2 other `ops/` files, 1 `manual/` page. They appear on 14 of 42 pages and hold rank 1 on four (`MD`, `MM`, `MQ`, `MV`). Two of those pages show the same skill twice, once live and once as a 2026-03-06 archive copy.

## The failure taxonomy, from the data

26 of 42 questions do not have `answers` as their top hit. Each is assigned one primary cause; several have a second cause, named in the row note. The groups below are ordered by count.

### 1. No abstention — 8 questions

The base cannot answer, and it answers anyway. Every top hit is `wrong` (a different subject), and on all eight the top hit's `weak` is false, so the one signal that exists does not fire.

`T10` `T11` `T12` `MS` `MT` `MU` `MV` `MW`

> `MT` "Как настроить max_connections в Postgres для 2brain?" → rank 1 `inbox/paradedb-pg-search-postgres.md`: "Just use Postgres" — a note about a BM25 extension. The corpus holds Postgres notes; it holds no `max_connections` string (measured: 0 files).

One sub-shape worth separating: **the meta note**. `T10` ("какая лицензия у Apache Kafka") returns as rank 1 the note that uses this exact question as its worked example of a bad match — "флаг weak в tb_search помечает «эти слова здесь встречаются»". A note that quotes the question outranks any note that could answer it. That is a retrieval artefact created by writing about retrieval failures.

### 2. The answer exists, below a weaker hit — 7 questions

The note that states the fact is on the page, at ranks 2-5, but a mentioning or partially-answering note is above it. This is where a path-based metric says "pass" and a reader says "that is not the answer".

`T01` `T15` `T16` `MF` `ML` `MN` `MR`

> `T15` "oracle matrix tests" → rank 1 `inbox/insight-verifiable-artifact-beats-green-gates.md`: "Зелёные гейты и повторное ревью не гарантируют корректность" (a different claim about verification), while the note that answers — `f/patterns/pattern-oracle-drift-ci.md`, "Реин implementations/эмуляции проверяются в CI дифференциально против живого эталона" — is rank 3.

The same shape in a definitional query: `T01` "ReasoningBank" puts a note that says agents learn *via* ReasoningBank first, and the note whose subject is ReasoningBank fifth.

### 3. Non-note files outrank notes — 3 questions

Skill instruction files and archive copies are indexed as documents and match generic words harder than notes do (`building`, `index`, `first`, `save`, `problem`, `search`, `default`). On the 42 pages they are 17.9% of hits.

`MD` `MM` `MQ`

> `MM` "What do skills save, and what problem comes with that saving?" → ranks 1 and 2 are `.claude/skills/next/SKILL.md` and its `ops/skills-archive/next-2026-03-06-pre-upgrade.md` twin: "Surface the most valuable next action". The note that answers (`skills-are-a-reasoning-cache-saving-63-percent-but-rot-needs-management.md`) is not in the top 100.

`MQ` is the clean case for the fix: the answering note is rank 4, and on a notes-only page it would be rank 1. `MD` and `MM` would not be fixed by filtering alone — their answers are not in the top 100 of the notes either.

### 4. Cross-lingual questions match function words, not content — 2 questions

Both are Russian questions whose answer note is English-only. No content token is shared, so the page fills with Russian notes that share only `модель`, `валидность`, `отдельно`, `если`, `выбор`.

`T03` `T18`

> `T03` "агент не вызывает доступные скиллы, если выбор оставлен на его усмотрение" → rank 1 `knowledge/jev-context-curation-pattern.md`: "Jev решает, что попадает в контекст и что остаётся". The answer, `agent-skills-go-unused-in-56-percent-of-cases-when-invocation-is-left-to-agent-discretion.md`, is not in the top 100.

This is not the floor: `min_should_match=1` left the first four hits unchanged on both queries. It is the absence of a shared vocabulary, plus Russian function words counting as matches.

### 5. A general note outranks the specific one — 2 questions

`T17` `MA`

> `T17` "event log as source of truth" → rank 1 `knowledge/kutha-temporal-graphs.md` (a hub): "append-only event log = source of truth; рабочий граф = детерминированный fold". The note that argues the rule, `event-log-as-source-of-truth-agent-memory.md`, is rank 8. The hub states the claim, so it is `partially`, not `wrong` — but a reader who asks why the log is primary gets a one-clause summary and not the answer.

`MA` is the same shape with a principle instead of a map: `selective-dependency-adoption-over-monolithic-integration.md` answers "adopt only the validated components" at rank 1, while the two notes that record the actual reason (`168 downloads`, `single-author risk`) are ranks 2 and 3.

### 6. Every hit is a referrer, not the referent — 1 question

`MI` "How fast is ruvector's distance kernel across hardware targets?" returns five notes that all contain a link to `ruvector-core-hnsw-with-simd-delivers-16m-distance-ops-per-second-across-four-hardware-targets.md`, and none states the number. The target is rank 17:

> rank 4 `claude-flow-hybrid-memory-…`: "RuVector's HNSW is the ultimate backend"; rank 5 `agentdb-auto-selects-backend-…`: "RuVector is the top-tier backend".

A link annotation repeats the target's name, so the linking notes contain the query's words and the target's claim does not. Five mentions-only hits in a row is the signature.

### 7. Near-duplicate notes that disagree on the number — 1 question

`MC` "At what entity-resolution accuracy does knowledge-graph reasoning start to break?"

> rank 1 `entity-resolution-accuracy-below-90-percent-…`: title says 90%, body says "When ER accuracy drops below 85%, errors compound". Rank 2 and rank 5 say below 85%.

Three notes carry the same claim with two different thresholds, and the engine ranks the self-contradicting one first. The reader gets a number, and cannot tell which one the corpus means. This is the duplicate problem the vault's own `/connect` rules are meant to prevent, made visible by search.

### 8. Ambiguous query, no disambiguation — 1 question

`T09` "agent memory" returns five true-but-narrow facets, all `weak=true`: a memory-agent abstraction, A-MEM, a pinned OpenAPI, a SurrealDB stack map, a SurrealDB-vs-Kutha contrast. Nothing tells the reader these are different questions. `weak` is the only signal that fires, and it is a boolean on each hit, not an instruction to narrow.

### 9. Word form — 1 question, and it cost almost nothing

`T13` "capsules" (the corpus holds only `capsule`) returns total=2, both relevant, both `partially` (they name the artefact: "CapsuleCraft KB (.capsule/ 341 .eml-капсула с provenance)" and "Capsule=source+provenance"). The plural cost nothing here — unlike the coverage audit's expectation. Where it does cost is `MB`, where `timestamps` is also a plural and the answer is rank 1; the plural was not the problem there either.

### What did not dominate

- `min_should_match` cutting: measured on four failing queries, lowering it to 1 changed totals but not the first four hits. It is not what hides these answers.

- Plural/word form: 1 case, and the page was still on-topic.

- Hub outranking the atomic note: 2 cases — real, but not the dominant shape.

What dominates is duller and more expensive to fix: the engine cannot say no (8), and when it can answer, the answering note is often not the one it puts first (7), with non-note files (3), no shared vocabulary across languages (2) and general-before-specific (2) behind them.

## The eight questions the base cannot answer

All eight returned five hits. None abstained. Whether it should have is not a matter of taste: for four of the eight the corpus has no note on the subject at all, and for the other four the question's *entity* is covered while the asked *attribute* is not. That distinction is the one a useful signal must carry.

| question | what the corpus actually holds | should it have said so? | signal that would carry it |
|---|---|---|---|
| `T10` Kafka licence | one file contains "kafka": the `weak` note, quoting this very question. No Kafka note. | yes, plainly | subject coverage 0 → `no_evidence` |
| `T11` Netflix price in Thailand | one note mentions the Netflix Prize (a datasets chapter); no price, no Thailand. | yes | subject coverage 0 for "price"/"Thailand" → `no_evidence` |
| `T12` stars on the 2brain repo | 2brain is well covered; "stars" appears in 13 files, all about other repositories. No star count. | yes | entity covered, attribute absent → `partial_coverage` |
| `MS` Kafka Streams exactly-once | no Kafka note; "exactly-once" appears once, about vendor handoffs. | yes | subject coverage 0 → `no_evidence` |
| `MT` Postgres `max_connections` | four Postgres notes (ParadeDB, PGX, RuVector-Postgres); `max_connections` appears in 0 files. | yes, and specifically: "Postgres is covered, connection settings are not" | entity covered, attribute absent → `partial_coverage` |
| `MU` Notion search index | "Notion" appears in 8 files, none about its search index or database. | yes | entity mentioned only → `no_evidence` |
| `MV` ZeroClaw weekly npm downloads | 32 files mention npm, 25 mention downloads (including RuVector's 168); no ZeroClaw count. | yes | entity covered, attribute absent → `partial_coverage` |
| `MW` 2brain cloud region | 2brain is covered by one note, about a stale binary; nothing about hosting. | yes | entity covered, attribute absent → `partial_coverage` |

The important negative result: **term rarity cannot carry this**. `weak` was false on all eight top hits, and the reason is in the note that documents the flag — `apache`, `postgres`, `exact`, `2brain` are rare in the corpus and irrelevant to the question. A signal that works has to be about the **subject and the attribute**, not about how unusual a word is. Both are computable without a model: subject coverage from title+description postings, attribute presence from the candidate sentences themselves. The claim check in the next section is what decides when a note only talks around the subject.

## What the answer shape should be

Every failure above is a fact the reader needed and the answer did not carry. The order is by leverage, largest first.

1. **`confidence: answered | partial | no_evidence`, with the basis.** `no_evidence` is the field that turns 8 wrong answers into correct refusals. It cannot be computed from the current score: the basis must be `subject_covered` and `attribute_present`, not `matched_terms` (which was 4.75 on average for wrong hits and 5.32 for answers — no signal). For one question in eight (`T12`) the honest value is not `no_evidence` but "the base knows this repository and does not record its star count".

2. **`answer_span`: the sentence, not the 240-character window.** Today the `excerpt` is centred on matched terms. On `MI` it shows a link list; on `T14` the match sits in the frontmatter description while the definition is in the body. A hit that cannot produce a sentence should be labelled `mentions_only` in the answer — the field and the evidence are the same thing.

3. **`kind: note | map | skill | manual | archive`.** The vault already knows this (`type: moc`, the directory, the `ops/skills-archive` prefix); the answer does not say it. Without it a caller cannot discount a map (17 hits, 2 answers) or a non-note file (37 hits, 0 answers), and the caller is the one paying for the wrong context.

4. **`why_this_hit`, in one clause, chosen from the hit's own relation to the question**: "states the rule", "links to the note that states it", "uses the term in another sense", "from another project". This is what separates `T15` rank 1 (a different verification claim) from rank 3 (the oracle-matrix note) when both have similar scores.

5. **`conflicts` / `duplicate_of`.** `MC` needs it: three notes, two thresholds (85% and 90%), and the top one contradicts itself. A reader can be given both figures with their sources; today they get the wrong one first and no warning.

6. **`next_question` or `narrow_with` for ambiguous queries.** `T09` has no answer because it is five questions. The co-occurrence needed to suggest "agent memory vs graph memory" or "memory system architecture" is already in the index. This is the cheapest of the six: one field, no model.

What the answer does **not** need: a better score, a longer excerpt, or more hits. Every group above is about a missing distinction — answered vs mentioned, note vs skill, specific vs general, one question vs many — and a distinction is a field, not a weight.

## The single change that would fix the most questions

**Score the top 20 by whether a sentence in the hit states the asked claim, and let that same check return `no_evidence` when no hit does.** One mechanism, measured against this sample:

- it promotes the answering note in the 7 answer-below questions (all are ranks 2-5, so already in the top 20);
- it pulls `MI`'s answer from rank 17, where no lexical signal reaches it;
- it refuses the 8 unanswerable questions, because no candidate sentence states the asked claim;
- it flags `MC`'s contradiction instead of silently choosing the 90% title.

That is **16 of 42 questions whose top answer changes for the better** (7 promotions + 1 referent + 8 refusals), and one more whose wrong number acquires a warning. For comparison, the cheap structural fixes:

| change | questions fixed at rank 1 | what else it does |
|---|---:|---|
| exclude non-note files from the index | 1 (`MQ`) | removes 37/207 noise hits and 4 rank-1 wrong hits; `MD` and `MM` still fail because their answers are not in the notes' top 100 either |
| drop function words from `min_should_match` and from scoring | 0 measured | the four probed queries kept the same first four hits; it would clean the counts, not the order |
| translate or expand Russian queries | 2 (`T03`, `T18`) | the only fix for a query whose answer shares no token |
| demote hubs | 1 (`MA`) | `T17`'s atomic note is rank 8, so demoting the hub alone would not surface it |

The claim check is not free: it needs a judge (a small model or a cross-encoder over 20 candidate sentences) and a decision rule for what "states the claim" means. But it is the only one of these that addresses the two largest groups at once, and it produces the field the answer is missing — a sentence that can be quoted, or an honest no.

## Limits of this sample

- 24 of the 42 questions are mine, written after reading the vault's titles. That makes the sample fair for finding failure shapes and unfair as a score: I knew the wording of the notes I aimed at, and I still produced 5 unanswerable questions of my own and 2 whose answer note is English against a Russian question.

- The judgements are one reader's. The quotes are verifiable, the rubric is explicit, and the levels are coarse enough that a second reader should agree on most; `partially` is where two readers will differ most (19 hits).

- The engine is the debug build from a tree with a compiler lane active; the installed release may rank differently. Every number here belongs to the binary with sha256 `dc0170ce…`.

- `T15`/`T16` are the same query by design, so the 42 rows are 41 distinct questions.

---

## Appendix: every judgement

For each question: the page's total candidate count, the level of each hit, the sentence judged from, and the path. Levels are `A` answers, `P` partially answers, `M` mentions only, `W` wrong.

### T01 — ReasoningBank

`class=exact` · `total=50` · top-5: M P M P A

1. **M** — "agentic-flow provides 66 pre-built specialized agents, each with autonomous self-learning powered by ReasoningBank" — `knowledge/agentic-flow-deploys-66-specialized-agents-across-seven-categories-each-with-reasoningbank-self-learning.md`
2. **P** — "ReasoningBank Cognitive Layer" — `knowledge/agentdb-architecture.md`
3. **M** — "self-learning (SONA patterns, ReasoningBank, trajectory learning" — `knowledge/claude-flow-deploys-15-agent-hierarchical-mesh-with-queen-coordinator-and-three-domain-clusters.md`
4. **P** — "ReasoningBank | стратегии, индуцированные из само-оценённых успехов и провалов" — `inbox/routed-continual-learning-meta.md`
5. **A** — "ReasoningBank is the cognitive layer of AgentDB" — `knowledge/agentdb-reasoningbank-has-four-components-pattern-matcher-experience-curator-memory-optimizer-context-synthesizer.md`

### T02 — NVDA translation addon blind user

`class=exact` · `total=148` · top-5: A A M W W

1. **A** — "I have been blind since birth. I use a screen reader program." — `knowledge/a-blind-since-birth-user-shipped-an-nvda-translation-addon.md`
2. **A** — "Blind since birth, I built an NVDA translator addon with Nous" — `knowledge/hermes-stories-general.md`
3. **M** — "[[a-blind-since-birth-user-shipped-an-nvda-translation-addon]] — доступность, построенная тем, кому она нужна" — `knowledge/hermes-user-stories.md`
4. **W** — "HermesAI Translator, an open-source, AI-powered browser extension" — `knowledge/hermes-stories-content-creation.md`
5. **W** — "every model has blind spots the others would have caught" — `knowledge/hermes-stories-research.md`

### T03 — агент не вызывает доступные скиллы, если выбор оставлен на его усмотрение

`class=paraphrase` · `total=731` · top-5: W W W W W

1. **W** — "Jev решает, что попадает в контекст и что остаётся" — `knowledge/jev-context-curation-pattern.md`
2. **W** — "LLM-агент НИКОГДА не производит финальную поверхность" — `f/patterns/pattern-describe-dont-render.md`
3. **W** — "нужно ли перепроверять запись перед действием — не свойство записи" — `inbox/amv-guard-action-conditioned-verification.md`
4. **W** — "одиночный агент со скиллами регулярно превосходит" — `inbox/claude-single-loop-folds-subagents.md`
5. **W** — "Разговор, порождающий код, становится истинным источником нашего ПО" — `inbox/deltadb-zed-between-commits.md`

### T04 — перехват аргументов tool-call до исполнения

`class=ru-cross` · `total=193` · top-5: A P M W W

1. **A** — "Хук перед вызовом тула возвращает не только block/reason, но и input: подменённые аргументы ревалидируются по схеме" — `knowledge/agent-tool-call-breakpoint-pattern.md`
2. **P** — "L0 coerce (unwrap-массив, null→{}, строка-JSON→объект)" — `knowledge/toolcall-repair-layers-pattern.md`
3. **M** — "graveyard тупиков с ценой (try/why/cost) проверяется ДО исполнения вызова" — `f/patterns/pattern-enforced-memory.md`
4. **W** — "каждый model/tool call — записи в event log прогона" — `inbox/vercel-durable-harness.md`
5. **W** — "Train a 64M-parameter LLM from scratch in 2h" — `inbox/minimind-64m-from-scratch.md`

### T05 — как Zettelkasten применяют к памяти агента

`class=multi-hop` · `total=460` · top-5: A P A W W

1. **A** — "A-MEM (NeurIPS 2025) применяет Zettelkasten метод к памяти LLM агентов" — `knowledge/a-mem-uses-zettelkasten-method-for-agentic-memory-with-retroactive-evolution.md`
2. **P** — "exploits the Zettelkasten "follow the card links" retrieval pattern" — `knowledge/amem-search-agentic-graph-traversal.md`
3. **A** — "A-MEM (NeurIPS 2025) applies the Zettelkasten method to agent memory" — `knowledge/amem-zettelkasten-retroactive-evolution.md`
4. **W** — "MemoryRouter (SaaS, $20/мес): коммерческая реализация идеи" — `inbox/memoryrouter-cross-ai-vault.md`
5. **W** — "runner agents (триггер→скилл→docs PR)" — `inbox/mintlify-self-updating-docs-architecture.md`

### T06 — Q-learning меняет топологию графа агентов

`class=paraphrase` · `total=172` · top-5: A M M W W

1. **A** — "Модуль `ruvector-dag` реализует самообучающиеся ориентированные ациклические графы" — `knowledge/adaptive-dag-routing-uses-q-learning-to-optimize-agent-topology.md`
2. **M** — "Q-Learning | Value-Based | Discrete action spaces" — `knowledge/agentdb-ships-12-rl-algorithms-from-decision-transformer-to-hierarchical-rl-via-plugin-wizard.md`
3. **M** — "[[adaptive-dag-routing-uses-q-learning-to-optimize-agent-topology]] — Q-learning для топологии агентов" — `knowledge/learning-patterns.md`
4. **W** — "Q-learning: 1 бит информации за сэмпл в high-dim" — `inbox/ppa-ch11-12-decision-making-rl.md`
5. **W** — "точность резко падает с глубиной композиции" — `inbox/implicithop-context-vs-weights.md`

### T07 — ночная задача в 3:00 компилирует день в контекст

`class=paraphrase` · `total=597` · top-5: A P W W W

1. **A** — "ночью в 3:00 агент перечитывает все диалоги за день" — `knowledge/a-3am-dreaming-job-compiles-the-day-into-tomorrows-context-across-28-cron-jobs.md`
2. **P** — "вместо одного «в 3:00 прочти всё» здесь 5 распределённых «REM-циклов»" — `knowledge/rem-cycle-dreaming-runs-at-night-for-0-014-on-haiku.md`
3. **W** — "STORY-кластеризация" — `inbox/muckscraper-story-aggregator.md`
4. **W** — "Razorpay выдал 220+ сотрудникам по собственному always-on Hermes-агенту" — `knowledge/razorpay-runs-220-isolated-per-employee-agents-on-kubernetes.md`
5. **W** — "детерминированное ядро валидирует и компилирует в точный запрос" — `inbox/ln-legal-intent-bridge.md`

### T08 — как держать 2FA-коды вне доступа агента

`class=multi-hop` · `total=260` · top-5: A M W W W

1. **A** — "2FA-коды, сбросы паролей и банковские уведомления физически не доходят до него" — `knowledge/a-local-email-gatekeeper-keeps-2fa-codes-out-of-agent-reach.md`
2. **M** — "приватность как модули на границе доступа к данным" — `inbox/meta-privacylib-data-access-boundary.md`
3. **W** — "production-память агентов из трёх паттернов доступа" — `inbox/memcurrent-databricks-persistent-memory.md`
4. **W** — "Агент уничтожил кодовую базу за 3 секунды" — `knowledge/rollback-restores-destroyed-codebase-and-agent-context-in-one-command.md`
5. **W** — "тест идеи до выбора стека" — `inbox/rauch-agentic-coding-workflow-seven-practices.md`

### T09 — agent memory

`class=ambiguous` · `total=228` · top-5: P P M P P

1. **P** — "OpenSage's memory agent provides natural language interface to graph memory" — `knowledge/memory-agent-abstraction-enables-natural-language-memory-operations.md`
2. **P** — "A-MEM (NeurIPS 2025) applies the Zettelkasten method to agent memory" — `knowledge/amem-zettelkasten-retroactive-evolution.md`
3. **M** — "Pinned Agent Memory OpenAPI 0.2.1" — `knowledge/surrealdb-agent-memory-openapi-facts-pin.md`
4. **P** — "SurrealDB stack map Oct 2026" — `knowledge/surrealdb-stack-map.md`
5. **P** — "SurrealDB Agent Memory vs kutha" — `knowledge/surrealdb-agent-memory-vs-kutha.md`

### T10 — какая лицензия у Apache Kafka

`class=absent` · `total=85` · top-5: W W W W W

1. **W** — "флаг weak в tb_search помечает «эти слова здесь встречаются»" — `knowledge/search-weak-flag-is-a-signal-not-a-verdict.md`
2. **W** — "код Apache-2.0; веса modified OpenRAIL-M" — `inbox/indra-ocr-1m-parameters-read-49-pages-per-second-on-cpu-and-the-open-tier-sells-the-ceiling.md`
3. **W** — "SDK Pullenti: rule-based лингвистический анализ русского текста" — `knowledge/pullenti-sdk-analysis.md`
4. **W** — "Vela 2.0 (vLLM Semantic Router): open decision-модели со span-ответами" — `inbox/vela-20-adds-span-answers-to-the-systemone-decision-interface.md`
5. **W** — "Лицензия весов: Modified MIT" — `inbox/mistral-timestral.md`

### T11 — сколько стоит подписка Netflix в Таиланде

`class=absent` · `total=122` · top-5: W W W W W

1. **W** — "Netflix Prize убит linkage attack" — `inbox/ppa-ch8-datasets-benchmark-science.md`
2. **W** — "Branch predictor: сколько if-ов слишком много" — `inbox/cloudflare-branch-predictor-btb.md`
3. **W** — "Современные приёмы и алгоритмы в Rust-экосистеме 2026" — `knowledge/rust-modern-algorithms-patterns.md`
4. **W** — "SciDB = Sci-Hub 2.0" — `inbox/annas-archive-shadow-library-infrastructure.md`
5. **W** — "Confidence у Choice/Score — метрика того, стоит ли действовать" — `knowledge/jev-confidence-is-a-second-axis-not-a-stronger-answer.md`

### T12 — the exact number of stars on the 2brain repository

`class=absent` · `total=457` · top-5: W W W W W

1. **W** — "expo/exact internal discipline deep dive" — `inbox/exact-discipline-deep.md`
2. **W** — "View and manage the task stack and processing queue" — `.claude/skills/tasks/SKILL.md`
3. **W** — "Обслуживаемый бинарь 2brain отстаёт от репозитория" — `knowledge/2brain-serving-state-lags-the-repository.md`
4. **W** — "awesome-legal-nlp (345 stars" — `inbox/law-ai-nlp-landscape.md`
5. **W** — "Ценные паттерны PGX для kutha-graph" — `inbox/pgx-patterns-for-kutha-graph.md`

### T13 — capsules

`class=word-form` · `total=2` · top-5: P P

1. **P** — "CapsuleCraft KB (.capsule/ 341 .eml-капсула с provenance" — `inbox/statcraft-safe-rust-spss-alternative.md`
2. **P** — "Capsule=source+provenance" — `inbox/evolver-gene-experience-framework.md`

### T14 — as of

`class=exact-and` · `total=218` · top-5: A M M W W

1. **A** — "AS OF-запросы по обеим осям" — `knowledge/bitemporal-valid-transaction-time.md`
2. **M** — "FF5 green as of 2026-10-23" — `knowledge/kutha-temporal-graphs.md`
3. **M** — "рынок ожидает as-of факты" — `knowledge/graphiti-bitemporal-fact-edges-contrast.md`
4. **W** — "repetitive product setup UI sequences" — `f/patterns/pattern-dashboard-flows-as-mcp-tools.md`
5. **W** — "Agents lack stable internal models of social hierarchy" — `knowledge/agents-of-chaos-social-coherence-failure.md`

### T15 — oracle matrix tests

`class=mixed-language` · `total=83` · top-5: W M A M M

1. **W** — "Зелёные гейты и повторное ревью не гарантируют корректность" — `inbox/insight-verifiable-artifact-beats-green-gates.md`
2. **M** — "oracle (идеальные входные данные вручную)" — `inbox/pattern-three-anchors-before-tuning.md`
3. **A** — "Реин implementations/эмуляции проверяются в CI дифференциально против живого эталона" — `f/patterns/pattern-oracle-drift-ci.md`
4. **M** — "oracle-факт в окне 96%" — `inbox/properagent-memory-driven-proactive-assistance.md`
5. **M** — "oracle-union до +10pt" — `inbox/pattern-latent-memory-complement.md`

### T16 — oracle matrix tests

`class=mixed-language` · `total=83` · top-5: W M A M M

1. **W** — "Зелёные гейты и повторное ревью не гарантируют корректность" — `inbox/insight-verifiable-artifact-beats-green-gates.md`
2. **M** — "oracle (идеальные входные данные вручную)" — `inbox/pattern-three-anchors-before-tuning.md`
3. **A** — "Реин implementations/эмуляции проверяются в CI дифференциально против живого эталона" — `f/patterns/pattern-oracle-drift-ci.md`
4. **M** — "oracle-факт в окне 96%" — `inbox/properagent-memory-driven-proactive-assistance.md`
5. **M** — "oracle-union до +10pt" — `inbox/pattern-latent-memory-complement.md`

### T17 — event log as source of truth

`class=hub-vs-atomic` · `total=369` · top-5: P P P M M

1. **P** — "append-only event log = source of truth; рабочий граф = детерминированный fold" — `knowledge/kutha-temporal-graphs.md`
2. **P** — "tri-temporal app tier on multi-model DB vs event-log SoT engine" — `knowledge/surrealdb-agent-memory-vs-kutha.md`
3. **P** — "event log SoT, deterministic fold, harness H5" — `knowledge/kutha-graph-project-snapshot.md`
4. **M** — "модель редакций закона как event log" — `knowledge/pullenti-decree-model-bridge.md`
5. **M** — "временной (event log → проекция)" — `knowledge/kutha-stca-paradigm.md`

### T18 — битемпоральная модель ребра отслеживает валидность и время загрузки отдельно

`class=bilingual` · `total=351` · top-5: W W P W W

1. **W** — "Двухслойный гейт оценки работы агента" — `inbox/pattern-blocker-rubric-grading.md`
2. **W** — "слой 1: пучок над графом ЭКСТРАКТОРОВ" — `inbox/claims-slot-graph-balance-gate-for-proofgate.md`
3. **P** — "Bi-temporal модель: valid time (мировая истина) × transaction time (время записи/веры)" — `knowledge/bitemporal-valid-transaction-time.md`
4. **W** — "Защита от выгрузки во время исполнения" — `inbox/hotreload-in-rust-dynamic-libraries.md`
5. **W** — "формально доказанной авторизационной" — `inbox/nyxis-lean-formally-verified-authorization.md`

### MA — Why did ZeroClaw not adopt RuVector?

`class=negation` · `total=232` · top-5: P A A W M

1. **P** — "When evaluating large multi-crate projects like RuVector (90+ crates), adopt only the validated components" — `knowledge/selective-dependency-adoption-over-monolithic-integration.md`
2. **A** — "rejecting RuVector (168 downloads) due to low adoption risk" — `knowledge/zeroclaw-stack-validated-hnsw-rs-tantivy-petgraph-fastembed-over-ruvector.md`
3. **A** — "citing 168 total downloads and single-author risk" — `knowledge/ruvector-rejected-for-zeroclaw-due-to-168-downloads-and-single-author-risk.md`
4. **W** — "ZeroClaw's 9-phase roadmap sequences memory capabilities in strict dependency order" — `knowledge/fact-extraction-must-precede-graph-construction-intelligence-before-infrastructure.md`
5. **M** — "validated stack: hnsw_rs + tantivy + petgraph" — `knowledge/zeroclaw-decisions.md`

### MB — How many timestamps does a ZeroClaw edge carry, and what do they separate?

`class=number+condition` · `total=497` · top-5: A A W W W

1. **A** — "facts are never deleted, only invalidated via 4 timestamps per edge" — `knowledge/tkg-bi-temporal-edge-model-tracks-four-timestamps-for-non-lossy-history.md`
2. **A** — "Every edge in the graph carries four timestamps" — `knowledge/zeroclaw-bi-temporal-edge-model-tracks-validity-and-ingestion-separately.md`
3. **W** — "Current agentic systems lack an explicit stakeholder model" — `knowledge/agents-of-chaos-no-stakeholder-model.md`
4. **W** — "Surface the most valuable next action by combining task stack" — `ops/skills-archive/next-2026-03-06-pre-upgrade.md`
5. **W** — "Surface the most valuable next action by combining task stack" — `.claude/skills/next/SKILL.md`

### MC — At what entity-resolution accuracy does knowledge-graph reasoning start to break?

`class=near-duplicate-numbers` · `total=479` · top-5: P A M M A

1. **P** — "When ER accuracy drops below 85%, errors compound" — `knowledge/entity-resolution-accuracy-below-90-percent-corrupts-knowledge-graph-reasoning-chains.md`
2. **A** — "When ER accuracy drops below 85%, errors compound with each graph traversal" — `knowledge/entity-resolution-accuracy-below-85-percent-corrupts-graph.md`
3. **M** — "90% accuracy gate" — `knowledge/zeroclaw-fact-extraction.md`
4. **M** — "Entity quality importance" — `knowledge/hierarchical-graph-schema-enables-semantic-abstraction-layers.md`
5. **A** — "Entity Resolution ниже 85% приводит к каскадным ошибкам при обходе графа" — `knowledge/entity-resolution-below-85-percent-cascades-into-graph-corruption.md`

### MD — When is building a graph index a mistake, and what should happen first?

`class=paraphrase` · `total=596` · top-5: W W W W W

1. **W** — "First session guide — creating your first insight and building connections" — `manual/getting-started.md`
2. **W** — "Dangling links block graph traversal → fix first" — `.claude/skills/next/SKILL.md`
3. **W** — "Dangling links block graph traversal → fix first" — `ops/skills-archive/next-2026-03-06-pre-upgrade.md`
4. **W** — "You are building a traversable knowledge graph, not tagging documents." — `ops/skills-archive/connect-2026-03-06-pre-upgrade.md`
5. **W** — "You are building a traversable knowledge graph, not tagging documents." — `.claude/skills/connect/SKILL.md`

### ME — Do self-generated skills improve agent pass rates?

`class=yes/no-negative` · `total=406` · top-5: A M A W M

1. **A** — "Curated Skills boost agent pass rate by 16pp while self-generated Skills degrade by 1pp" — `knowledge/curated-skills-boost-agent-pass-rate-by-16pp-while-self-generated-skills-degrade-by-1pp.md`
2. **M** — "SkillsBench warns that auto-consolidated skills may face the same quality gap as self-generated ones" — `knowledge/agentdb-skill-library-auto-consolidates-successful-patterns-into-parameterized-composable-skills.md`
3. **A** — "self-generated Skills fail not just because of quality" — `knowledge/optimal-skill-count-is-two-to-three-modules-at-1500-tokens-four-plus-causes-cognitive-overload.md`
4. **W** — "agents store execution episodes with self-critique" — `knowledge/agentdb-reflexion-memory-stores-episodes-with-self-critique-for-experience-replay-learning.md`
5. **M** — "Пара статей, формирующих новое поле harness learning" — `inbox/harness-learning-field-map.md`

### MF — What share of agent tasks remain unsolvable even with curated skills?

`class=number` · `total=521` · top-5: P A M M M

1. **P** — "Even optimally curated Skills hit a ceiling" — `knowledge/curated-skills-boost-agent-pass-rate-by-16pp-while-self-generated-skills-degrade-by-1pp.md`
2. **A** — "16 of 84 SkillsBench tasks have 0% pass rate across all models" — `knowledge/nineteen-percent-of-agent-tasks-remain-unsolvable-even-with-curated-skills-defining-current-capability-ceiling.md`
3. **M** — "[[nineteen-percent-of-agent-tasks-remain-unsolvable-even-with-curated-skills" — `knowledge/optimal-skill-count-is-two-to-three-modules-at-1500-tokens-four-plus-causes-cognitive-overload.md`
4. **M** — "even domain-specific Skills cannot breach tasks requiring multi-step pipelines" — `knowledge/domain-specific-skills-boost-agents-42-to-52pp-in-healthcare-manufacturing-but-only-4-to-6pp-in-software-engineering.md`
5. **M** — "SkillsBench's eval harness explicitly integrated Skills into the agent workflow" — `knowledge/passive-context-agents-md-achieves-100-percent-pass-rate-where-on-demand-skills-achieve-53-percent-by-eliminating-invocation-decision.md`

### MG — Сколько операций в секунду выдаёт HNSW в ruvector и на какой задержке?

`class=ru-cross` · `total=868` · top-5: A M W W M

1. **A** — "52K вставок/сек и 0.019ms p50 задержку поиска" — `knowledge/ruvector-hnsw-achieves-52k-ops-sec-with-sub-millisecond-latency.md`
2. **M** — "HNSW vector search is production-ready" — `knowledge/ruvector-hnsw-core-validated-but-gnn-experimental.md`
3. **W** — "rejected RuVector as a dependency" — `knowledge/ruvector-rejected-for-zeroclaw-due-to-168-downloads-and-single-author-risk.md`
4. **W** — "rejecting RuVector (168 downloads)" — `knowledge/zeroclaw-stack-validated-hnsw-rs-tantivy-petgraph-fastembed-over-ruvector.md`
5. **M** — "Оценивать граф-механики на уровне ОТДЕЛЬНЫХ операций" — `inbox/pattern-role-locus-operation-attribution.md`

### MH — How much RAM does an HNSW index need for 100k vectors at 1536 dimensions?

`class=number` · `total=388` · top-5: A M W M W

1. **A** — "100K vectors at 1536 dims потребляет 600MB+" — `knowledge/hnsw-memory-bloat-index-requires-ram-residency-100k-vectors-at-1536-dims-needs-600mb-plus.md`
2. **M** — "memory constraints" — `knowledge/zeroclaw-semantic-memory.md`
3. **W** — "Default configuration targets production workloads: vectorDimension=1536" — `knowledge/claude-flow-hybrid-memory-combines-sqlite-acid-with-agentdb-hnsw-for-150x-to-12500x-search-speedup.md`
4. **M** — "when to add HNSW" — `knowledge/zeroclaw-decisions.md`
5. **W** — "Vercel compressed 40KB docs to 8KB pipe-delimited index" — `knowledge/compressed-8kb-docs-index-preserves-full-agent-effectiveness-agents-need-pointers-not-content.md`

### MI — How fast is ruvector's distance kernel across hardware targets?

`class=number` · `total=378` · top-5: M M M M M

1. **M** — "HNSW indexes the hyperbolic vectors produced by claude-flow's embedding pipeline" — `knowledge/hyperbolic-geometry-appears-at-every-layer-of-the-ruvnet-stack-from-sql-functions-to-onnx-embeddings.md`
2. **M** — "the architectural capability" — `knowledge/ruvector-rejected-for-zeroclaw-due-to-168-downloads-and-single-author-risk.md`
3. **M** — "ZeroClaw's System-1 component" — `knowledge/dual-route-memory-combines-vector-similarity-with-hierarchical-graph-traversal.md`
4. **M** — "RuVector's HNSW is the ultimate backend" — `knowledge/claude-flow-hybrid-memory-combines-sqlite-acid-with-agentdb-hnsw-for-150x-to-12500x-search-speedup.md`
5. **M** — "RuVector is the top-tier backend" — `knowledge/agentdb-auto-selects-backend-from-ruvector-rust-down-to-wasm-sqlite-for-universal-runtime.md`

### MJ — Why was rten replaced for embeddings in ZeroClaw?

`class=why` · `total=377` · top-5: A A W M M

1. **A** — "rten was 30-50x slower, lacked INT8 quantization" — `knowledge/zeroclaw-replaced-rten-with-ort-for-embeddings-due-to-performance-gap.md`
2. **A** — "ORT 30-50x faster than rten with INT8 quantization" — `knowledge/zeroclaw-decisions.md`
3. **W** — "Why each configuration dimension was chosen" — `ops/methodology/derivation-rationale.md`
4. **M** — "embedding provider iterations (rten, ORT hybrid" — `knowledge/zeroclaw-uses-inserted-phases-for-tech-debt-and-integration-fixes.md`
5. **M** — "Topic map for ZeroClaw Phase 4 — vector embeddings" — `knowledge/zeroclaw-semantic-memory.md`

### MK — When does the ZeroClaw agent loop trim history?

`class=condition` · `total=529` · top-5: A W M W M

1. **A** — "Auto-Trims History at 50 Messages" — `knowledge/zeroclaw-agent-loop-enforces-max-10-tool-iterations-and-auto-trims-history-at-50-messages.md`
2. **W** — "Agent refused to 'share' PII but complied when asked to 'forward'" — `knowledge/agents-of-chaos-forwarding-reframes-bypass.md`
3. **M** — "[[zeroclaw-agent-loop-enforces-max-10-tool-iterations-and-auto-trims-hi" — `knowledge/miroflow-orchestrator-loop-uses-turn-based-execution.md`
4. **W** — "does not address consolidation" — `knowledge/rl2f-consolidation-problem-remains-unsolved.md`
5. **M** — "loop constants" — `knowledge/zeroclaw-decisions.md`

### ML — How many skill modules should be loaded at once, and why not more?

`class=number+why` · `total=449` · top-5: M A W W W

1. **M** — "2-3 focused domain Skills hit the sw" — `knowledge/domain-specific-skills-boost-agents-42-to-52pp-in-healthcare-manufacturing-but-only-4-to-6pp-in-software-engineering.md`
2. **A** — "tasks with 2-3 Skills gained +18.6 percentage points, while tasks with 4+ Skills dropped to +5.9pp" — `knowledge/optimal-skill-count-is-two-to-three-modules-at-1500-tokens-four-plus-causes-cognitive-overload.md`
3. **W** — "Only report patterns that pass all four checks" — `.claude/skills/review/SKILL.md`
4. **W** — "Surface the most valuable next action" — `.claude/skills/next/SKILL.md`
5. **W** — "Surface the most valuable next action" — `ops/skills-archive/next-2026-03-06-pre-upgrade.md`

### MM — What do skills save, and what problem comes with that saving?

`class=two-part` · `total=479` · top-5: W W W W M

1. **W** — "Surface the most valuable next action" — `ops/skills-archive/next-2026-03-06-pre-upgrade.md`
2. **W** — "Surface the most valuable next action" — `.claude/skills/next/SKILL.md`
3. **W** — "Update old insights with new connections" — `.claude/skills/refresh/SKILL.md`
4. **W** — "Update old insights with new connections" — `ops/skills-archive/refresh-2026-03-06-pre-upgrade.md`
5. **M** — "even well-consolidated skills are worthless if agents fail to invoke them" — `knowledge/agentdb-skill-library-auto-consolidates-successful-patterns-into-parameterized-composable-skills.md`

### MN — Which sandbox mechanisms does auto-detection try, and in what order?

`class=ordered-list` · `total=473` · top-5: W W A W W

1. **W** — "does not address consolidation--transient adaptation does not become permanent knowledge" — `knowledge/rl2f-consolidation-problem-remains-unsolved.md`
2. **W** — "12 trigger types with auto-detection" — `knowledge/claude-flow-runs-12-background-workers-from-ultralearn-to-testgaps-with-daemon-management.md`
3. **A** — "tries landlock → bubblewrap → firejail → docker → noop sandbox" — `knowledge/sandbox-auto-detection-tries-landlock-bubblewrap-firejail-docker-in-order.md`
4. **W** — "NEVER auto-implement proposals" — `.claude/skills/review/SKILL.md`
5. **W** — "Surface the most valuable next action" — `ops/skills-archive/next-2026-03-06-pre-upgrade.md`

### MO — How many MCP tools does claude-flow expose, and across how many categories?

`class=number+hub` · `total=403` · top-5: A W A W M

1. **A** — "MCP-first design with 19 tool categories" — `knowledge/claude-flow-architecture.md`
2. **W** — "213 MCP tools" — `knowledge/agentic-flow-exposes-213-mcp-tools-for-swarm-memory-neural-github-and-performance-operations.md`
3. **A** — "organizing 171 tools into 19 categories" — `knowledge/claude-flow-exposes-171-mcp-tools-across-19-categories-with-multi-transport-stdio-http-websocket.md`
4. **W** — "213 across 7 tool groups" — `knowledge/agentic-flow-architecture.md`
5. **M** — "64 specialized agents organized across 25 subdirectories" — `knowledge/claude-flow-organizes-64-specialized-agents-in-25-subdirectories-with-yaml-config-and-mcp-integration.md`

### MP — What does the weak flag on a search answer mean?

`class=definition` · `total=557` · top-5: A M W W W

1. **A** — "флаг оставлен как сигнал, а не как гейт" — `knowledge/search-weak-flag-is-a-signal-not-a-verdict.md`
2. **M** — "решения о search/weak/create_file описаны в коде" — `knowledge/2brain-serving-state-lags-the-repository.md`
3. **W** — "evaluated: weak, only surface vocabulary" — `ops/skills-archive/connect-2026-03-06-pre-upgrade.md`
4. **W** — "evaluated: weak, only surface vocabulary" — `.claude/skills/connect/SKILL.md`
5. **W** — "Full hybrid search with LLM reranking compensates for weak descriptions" — `.claude/skills/check/SKILL.md`

### MQ — Why does search default to the own workspace?

`class=why` · `total=498` · top-5: W W W A W

1. **W** — "Extract structured knowledge from source material" — `.claude/skills/extract/SKILL.md`
2. **W** — "Extract structured knowledge from source material" — `ops/skills-archive/extract-2026-03-06-pre-upgrade.md`
3. **W** — "Why each configuration dimension was chosen" — `ops/methodology/derivation-rationale.md`
4. **A** — "вопрос, на который волт ответить не может" — `knowledge/search-scope-defaults-to-the-own-workspace.md`
5. **W** — "Research a topic and grow your knowledge graph" — `.claude/skills/learn/SKILL.md`

### MR — How does ZeroClaw's retrieval combine two routes?

`class=multi-hop` · `total=288` · top-5: W A P W W

1. **W** — "Store feedback in vector DB for similar-problem retrieval" — `knowledge/rl2f-consolidation-problem-remains-unsolved.md`
2. **A** — "System-1 provides fast retrieval (<50ms target) via vector similarity search (HNSW)" — `knowledge/zeroclaw-retrieval-uses-mnemis-dual-route-system-1-vector-plus-system-2-graph.md`
3. **P** — "Mnemis introduces a dual-route memory architecture where two complementary retrieval mechanisms work together" — `knowledge/dual-route-memory-combines-vector-similarity-with-hierarchical-graph-traversal.md`
4. **W** — "Routes natural language questions to graph scripts" — `.claude/skills/graph/SKILL.md`
5. **W** — "old and new embeddings exist in different" — `knowledge/embedding-drift-silent-retrieval-killer.md`

### MS — What does the vault say about Kafka Streams exactly-once guarantees?

`class=absent` · `total=273` · top-5: W W W W W

1. **W** — "exactly-once handoffs между вендорами" — `inbox/ai-memory-akitaonrails-coding-agent-memory.md`
2. **W** — "Before triaging observations, check whether the system has drifted" — `.claude/skills/review/SKILL.md`
3. **W** — "Surface the most valuable next action" — `ops/skills-archive/next-2026-03-06-pre-upgrade.md`
4. **W** — "Surface the most valuable next action" — `.claude/skills/next/SKILL.md`
5. **W** — "Memory anchored to code symbols" — `f/patterns/pattern-memory-anchored-to-code-symbols.md`

### MT — Как настроить max_connections в Postgres для 2brain?

`class=absent-ru` · `total=794` · top-5: W W W W W

1. **W** — "Just use Postgres" — `inbox/paradedb-pg-search-postgres.md`
2. **W** — "два пути уйти от embedding-пайплайна в Postgres" — `inbox/two-paths-out-of-the-embedding-pipeline-one-index-versus-one-swiss-army-knife.md`
3. **W** — "Обслуживаемый бинарь 2brain отстаёт от репозитория" — `knowledge/2brain-serving-state-lags-the-repository.md`
4. **W** — "Сравнение PGX" — `inbox/pgx-vs-sqlite-vs-turso-for-prime-agent.md`
5. **W** — "Writing a hypermedia app in Rust" — `inbox/geer-hypermedia-rust-tutorial.md`

### MU — Which database does Notion use for its search index?

`class=absent` · `total=344` · top-5: W W W W W

1. **W** — "the most ambitious open-source Rust vector database" — `knowledge/ruvector-architecture.md`
2. **W** — "SQLite for ACID/FTS plus AgentDB HNSW for vector search" — `knowledge/claude-flow-hybrid-memory-combines-sqlite-acid-with-agentdb-hnsw-for-150x-to-12500x-search-speedup.md`
3. **W** — "Run semantic search for conceptually related" — `ops/skills-archive/connect-2026-03-06-pre-upgrade.md`
4. **W** — "Run semantic search for conceptually related" — `.claude/skills/connect/SKILL.md`
5. **W** — "Combined verification" — `.claude/skills/check/SKILL.md`

### MV — What is ZeroClaw's weekly npm download count?

`class=absent` · `total=347` · top-5: W W W W W

1. **W** — "count incoming [[links]]" — `.claude/skills/next/SKILL.md`
2. **W** — "count incoming [[links]]" — `ops/skills-archive/next-2026-03-06-pre-upgrade.md`
3. **W** — "while turn_count < max_turns" — `knowledge/miroflow-orchestrator-loop-uses-turn-based-execution.md`
4. **W** — "ZeroClaw's Memory trait extension principle" — `knowledge/zeroclaw-memory-trait-extends-without-breaking-backward-compatibility.md`
5. **W** — "View and manage the task stack" — `.claude/skills/tasks/SKILL.md`

### MW — Which cloud region hosts the 2brain API?

`class=absent` · `total=200` · top-5: W W W W W

1. **W** — "trust region optimization" — `knowledge/rlvr-optimization.md`
2. **W** — "Обслуживаемый бинарь 2brain отстаёт от репозитория" — `knowledge/2brain-serving-state-lags-the-repository.md`
3. **W** — "Заявление FulcrumSec" — `inbox/pingcap-tidb-breach-claim.md`
4. **W** — "Background workers are the universal operational pattern" — `knowledge/every-ruvnet-project-runs-background-workers-as-the-operational-form-of-self-evolving-intelligence.md`
5. **W** — "same API on Node, browser, edge, MCP" — `knowledge/agentdb-auto-selects-backend-from-ruvector-rust-down-to-wasm-sqlite-for-universal-runtime.md`

### MX — What fraction of every API call is fixed overhead?

`class=number` · `total=449` · top-5: A W M W W

1. **A** — "73% of every API call is fixed overhead" — `knowledge/seventy-three-percent-of-every-api-call-is-fixed-overhead.md`
2. **W** — "WASM + AST analysis handles var->const" — `knowledge/agentic-flow-agent-booster-handles-trivial-edits-via-wasm-plus-ast-at-zero-cost-and-sub-millisecond-latency.md`
3. **M** — "fixed part (system prompt + tools) > usable window" — `inbox/allspark-opencode-issues-compaction-loops-cache-busting-756-byte-prompt.md`
4. **W** — "Extract structured knowledge from source material" — `ops/skills-archive/extract-2026-03-06-pre-upgrade.md`
5. **W** — "Extract structured knowledge from source material" — `.claude/skills/extract/SKILL.md`

