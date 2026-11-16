# The 160-question set: how it was built, what it measures, and what it already shows

`tools/ranking/tasks.json` holds eighteen hand-written tasks. At that size one flipped task is 5.6 points and the paired
bootstrap cannot separate one flip from none (`task-set.md`, *What a paired bootstrap can and cannot say*). This set is
the larger instrument: **160 rows**, each a question a reader who has **not** read the note could ask, labelled with the
minimal evidence, the quoted sentence that answers it, and the measured share of the query's words that the note does not
contain.

The file is `tools/ranking/questions.json`. It is a question set, not a score: every row carries `measured` — what the
served engine did on 2026-11-16 — so a later run can be diffed row by row.

## What "not tautological" means here, and how it is checked

The failure this set exists to avoid: a question generated from a note's title or first sentence, which the note then
"answers" because the question quotes it. Three defences:

1. **`why_not_tautology` per row** names the query words that do not occur in the expected note, with the note's own
   wording beside them.
2. **`absent_terms_share` per row** is the measured number: content terms only (English and Russian stoplist), a term
   counts as present if either side is a prefix of the other at five characters, repeated terms counted once. It is a
   number, not a judgement, and it is in the file so a reader can disagree with it.
3. **The exact class is exempt, and says so.** An identifier lookup is the thing being measured: `CapsuleCraft`,
   `Oxetene`, `SteelDB` and the other twelve each occur in **exactly one** of the 1 066 served markdown files (checked by
   `df` over the whole vault, not per directory). A term with df = 1 cannot be "matched because the question quotes the
   note" — it resolves a name. Their absent share is 0.00 by construction, and the class is reported that way.

Two more rules were applied while writing:

- a question must contain at least one word a reader would plausibly know (the topic), so the note has a lexical path
  to itself; the rest of the wording must be foreign. A question with no lexical hook is unfair, not hard.
- a question of twelve words or fewer whose words all occur verbatim in the note was rewritten. That is what removed the
  first drafts aimed at the `sliding-window-rate-limiter`, `release-profile` and `claude-flow-security` notes from the
  paraphrase class; the first went to the cross-lingual class, the second to a multi-hop pair, the third out of the set
  because every honest question about it quoted `444` and `bcrypt`.

## Distribution

| class | n | mean absent share | in top 10 | in top 20 |
|---|---:|---:|---:|---:|
| exact identifier | 15 | 0.00 | 15 | 15 |
| paraphrase | 35 | **0.52** | 30 | 32 |
| multi-hop | 20 | 0.54 | 19 | 19 |
| cross-lingual | 16 | 0.62 | 15 | 15 |
| ambiguous | 15 | 0.06 | 15 | 15 |
| absent | 15 | — (no expected note) | 0 (by design) | 0 |
| structural | 12 | 0.44 | 9 | 10 |
| silly / noise | 19 | 0.28 | 10 | 10 |
| regression | 13 | 0.48 | 7 | 7 |
| **total** | **160** | | | |

Reading the column that matters for the tautology bar: paraphrase **0.52** including the six rows that hold vocabulary
constant on purpose (they isolate another axis — see the pairs below), **0.580** excluding them. Cross-lingual is 0.62
because the two languages share almost no tokens. The low figures are honest: *ambiguous* is 0.06 because an ambiguous
query is usually one shared word (`capsule`, `tantivy`, `lease`), and *silly* is 0.28 because a fragment or a path
either quotes the note or quotes nothing.

## Axes: the set is a factorial, not a list

Every row carries `axes`, nine values:

| axis | values | distribution |
|---|---|---|
| language | en / ru / mixed | 103 / 38 / 19 |
| length | short / medium / long | 62 / 89 / 9 |
| form | question / keyword / silly / regression | 92 / 36 / 19 / 13 |
| hop | single / multi / none | 125 / 20 / 15 |
| lifecycle | any / current / proposed / superseded | 34 / 125 / 1 / **0** |
| type | any / a note type | 44 any, 48 pattern, 42 insight, 20 extract, 3 error, 2 claim, 1 decision |
| scope | own / all | 159 / 1 |
| temporality | none / recent / as-of | 156 / 0 / 4 |
| entity | concept / artifact / error / decision | 141 / 15 / 3 / 1 |

`superseded` is empty because the corpus has **no note with `status: superseded`** (measured: 997 current, 5 pending, 1
active, 1 acknowledged, 1 aging). That is a fact about the vault, not an omission here; the axis value exists so the set
can carry such a row the day the vault has one.

### The ten one-axis pairs

Each pair asks the same question and differs in exactly one axis, so a verdict can be attributed to that axis and not to
a different topic.

| pair | axis it isolates | ranks on the served engine (2026-11-16) |
|---|---|---|
| `z01` agnet memory retrievel ↔ `z02` agent memory retrieval | spelling | — / — (both miss: the typo is not the cause) |
| `z14` bitemporl edge model ↔ `z15` bitemporal edge model | spelling | 1 / 1 |
| `g01` capsules ↔ `g02` capsule | word form (plural) | **— / 1** (one-sided) |
| `g04` oracle matrix tests (default) ↔ `g05` same, `min_should_match: 1` | floor | **— / 3** (one-sided) |
| `g06` stars query, `scope: own` ↔ `g07` same, `scope: all` | scope | — / — (both return a vault page; the pair is inconclusive on the served engine) |
| `x01` how many tests does the security module pass ↔ `x02` сколько тестов проходит модуль безопасности | language | **1 / —** (one-sided) |
| `x03` oracle drift in ci ↔ `x04` the 17-word version | length | **1 / —** (one-sided) |
| `x05` hierarchical memory graph ↔ `x06` how does a hierarchical graph make agent state easier to manage | form | 4 / 12 |
| `x07` the oracle drift pattern ↔ `z07` f/patterns/pattern-oracle-drift-ci.md | entity (concept vs artifact) | 1 / 1 |
| `x08` what does CodeActInstruct change about training (current) ↔ `x09` same, proposed | lifecycle | 8 / 7 |

**Three pairs where the current index fails on one side only:**

1. **language, `x01` / `x02`.** The English question finds the note at rank 1; the Russian one does not enter the top 20.
   The note is English and the question shares no token with it — the bilingual gap `task-set.md` §5 names.
2. **length, `x03` / `x04`.** Three keywords find the pattern at rank 1; the same question in seventeen words does not.
   The long form adds eleven words that the corpus has everywhere, and BM25 rewards the notes that hold more of them.
3. **word form, `g01` / `g02`.** `capsule` returns its two notes; `capsules` returns **zero** hits — the served binary
   predates `c3c7ee4`, and the pair is the one row that says so in a single number.

## What the run already shows

These are measurements on the served engine, 2026-11-16, `scope: own`, limit 20. They are not the set's purpose — the
set is — but a 160-row instrument that found nothing would be a suspicious instrument.

1. **The served binary is behind the repository, and the regression rows prove it.** `capsules` → 0 hits (word-form
   retry absent); `oracle matrix tests` at the default floor → 5 notes, the oracle-drift pattern absent (rare-term
   escape absent); matches carry `matched_terms`, `path`, `score` and **no** `weak` and **no** `excerpt`. The vault
   already documents this (`knowledge/2brain-serving-state-lags-the-repository.md`); the regression class turns the
   documentation into a check that fails.
2. **Agent-tooling files inside the vault outrank notes.** `/root/vault/.claude/skills/*/SKILL.md` (16 files) and
   `ops/skills-archive/*.md` (4) occupy a top-3 slot in **25 of 160 rows** and 34 of 436 top-3 slots. They are long,
   generic, and hold many query words. Any long natural-language question pays for them.
3. **Near-duplicate notes split the answer.** `entity resolution accuracy threshold` returns three notes with two
   different numbers (85% and 90%); `injection prevention` returns prompt injection and command injection in one page;
   `tantivy` returns the stack note and the library note. The ambiguous class exists to hold these, and it is the only
   class where every row is reachable — because one of the siblings always wins.
4. **A `?` can zero a query.** `oracle matrix?` returns **0** hits while `oracle matrix tests` returns 5. A single
   Russian word the corpus does use in an inflected form (`битемпоральность`) also returns 0.
5. **Structural questions are not search questions.** `which notes sit in the patterns directory` and `which queued
   notes are still pending` are answerable by `tb_children` and by the frontmatter, and not by `tb_search` at all. They
   are in the set because a caller will ask them this way; a `context`-shaped tool should route them, and the search
   tool should not pretend.

## The five hardest questions for the current index

Chosen from rows that are well formed, have a lexical path, and still miss or land low.

| id | query | why it is hard |
|---|---|---|
| `x02` | сколько тестов проходит модуль безопасности | **Bilingual with no shared token.** The note is English (`444 passing tests`, `8 components`); the question is Russian and every content word is absent from the note. Lexical search cannot cross this, and no single fix in the ranker can either — it needs a description or alias field on the note. |
| `p16` | what share of the blended score comes from learned vectors versus keyword ranking | **A near-duplicate wins.** The Mnemosyne note gives 70% vector / 20% FTS / 10% graph, but `hybrid-search-fusion-tuning-risk` and `mnemosyne-hybrid-search-weights-ft` cover the same words; the note never enters the top 20 while the query's own subject words (`blended`, `share`, `versus`) match nobody. |
| `p13` | why does the index behind semantic lookup need to sit entirely in main memory | **A short English note loses to tooling boilerplate.** The HNSW-bloat note states it in one sentence (600MB+ at 100K vectors, RAM-resident), but the query's words are generic and the `.claude/skills/*` pages hold more of them. This is the row that names the boilerplate problem in one miss. |
| `m09` | which three retrieval channels are blended, and which tables store the experience | **Two notes, one page.** The first note is reachable alone (rank 2 for `Mnemosyne 70/20/10`); the second never is. A multi-hop row fails as soon as one of its two notes is out of the page, and single-document scoring has no mechanism to look for the second. |
| `s09` | which queued notes are still pending | **The answer is frontmatter, not prose.** Five `ops/queue/codeactinstruct-00x` notes carry `status: pending`; no sentence says so. Search returns the claims' text (they match on `CodeActInstruct`), never the queue as a set. |

## The regression class: what it pins, and what it found

| id | what it pins | tag | served engine |
|---|---|---|---|
| `g01` | a plural finds the singular | `c3c7ee4` | **fail** — 0 hits |
| `g02` | the singular is unchanged (control) | `c3c7ee4` | pass — 2 notes |
| `g03` | `exact` is an AND, not a phrase | `exact-and` | pass — 218 candidates, note first |
| `g04` | the rare-term escape at the default floor | `006ac0b` | **fail** — 5 hits, note absent |
| `g05` | `min_should_match: 1` reaches rank 3 of 83 | `006ac0b` | pass |
| `g06` | `scope: own` does not answer from a dependency | `scope-own` | pass (no dependency path in the page) |
| `g07` | `scope: all` may (inverse) | `scope-own` | inconclusive — both scopes returned a vault page |
| `g08` | the `weak` flag marks a common-word page | `weak-flag` | **fail** — no `weak` field on the wire |
| `g09` | the atomic note outranks the map | `D7-hub-vs-atomic` (**open**) | **fail** — rank 8, map first |
| `g10` | a Russian question reaches its English note | `D1-bilingual` (**open**) | **fail** — not in the top 20 (measured outside the top 100 in `task-set.md`) |
| `g13` | `tb_grep` and `tb_search` stay different tools | `exact-and` | pass — 10 literal matches vs 218 AND matches |
| `g14` | every match carries `matched_terms` and `total` | `wire-shape` | pass |
| `g15` | every match carries a non-empty `excerpt` | `wire-shape` | **fail** — `excerpt` absent from the wire |

Six failures out of thirteen, and five of the six are the documented lag between repository and served binary rather than
a new defect. The two open ones (`D7`, `D1`) fail for the reasons already published.

## Reproduction

```sh
python3 - <<'PY'
import json
d = json.load(open("tools/ranking/questions.json"))
print(len(d["rows"]), d["counts"], d["class_stats"])
PY
```

The rows are plain `{query, args}` pairs; anything that speaks `tb_search` can score them. `min_total` is used only where
a flag changes the candidate set (`g03`), `args` only where a flag is the subject (`g03`, `g05`, `g07`), and `should` /
`must_not` only on the rows whose pass rule is not a single rank (silly, absent).

## What this set does not measure

- **It is not a sample.** The rows are chosen, not drawn; per-class means describe these rows, not the vault.
- **`absent` has no clean judge.** The wire carries no "I do not know"; a row passes only when the page is honestly
  weak, and that is a judgement over a page, not a number.
- **Ranks are a snapshot of a lagging binary.** They are stored in the file so the next run can be diffed; they are not
  a target for the served engine to hit.
- **The Russia/English direction is under-represented.** Ten questions go Russian → English note and six go English →
  Russian note; the second direction is easier for this corpus (Russian notes keep English technical nouns), and the
  set says so by counting them separately.
