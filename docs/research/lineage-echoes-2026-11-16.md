# Lineage echoes: 36 notes are 17 observations, and 13 of the 17 echo groups carry no kinship signal

Measured 2026-11-16 against `/root/vault` (944 notes: 384 in `knowledge/`, 560 in `inbox/`), read-only.
The question is the multi-agent failure `crates/ruvector-agent-memory` implements lineage arbitration for:
N notes that repeat one source counting as N independent observations. When several notes declare one
source, how many independent observations does the corpus actually hold?

The short answer: **17 echo clusters, 36 notes, 17 observations — a 2.1x over-count — and only 4 of the 17
clusters declare their kinship.** All 4 declarations are the `family:` field. No note in the vault has
`supersedes:` or `derived_from:` (the only relative field anywhere is one `generated_from:` in a hygiene
note). The remaining 13 clusters are invisible to the graph's own signals, and 6 of those 13 are invisible
even to source-URL grouping, because 13 of their 14 notes carry no `source_url`.

## What was measured

| | knowledge | inbox | total |
|---|---:|---:|---:|
| notes | 384 | 560 | 944 |
| with `source_url` | 269 | 551 | 820 |
| distinct normalized sources | 116 | 451 | 565 |
| sources with 2+ notes | 32 | 66 | 100 |
| notes inside those | 187 | 168 | 355 |

Two of the 100 groups span `inbox/` and `knowledge/`; the rest are single-scope. The two columns overlap by
those 2 cross-scope sources, so the total is 565, not 567.

A source identity is the URL with the fragment removed, the trailing slash removed, `www.` removed, the host
lower-cased, tracking query parameters dropped, and three known equivalences applied: `arxiv.org/abs|pdf|html/<id>`
collapses to `arxiv:<id>`, `docs.google.com/document/d/<id>` to `gdoc:<id>`, and `github.com/<owner>/<repo>` to
`gh:<owner>/<repo>` (case-folded). Every note in a multi-note group was read through `tb_references` for both
directions. The 6 cross-URL clusters were found by a title-and-description similarity sweep over all 384
knowledge notes, then judged by reading the bodies; they are a screened set, not an exhaustive one.

Two of the 11 same-URL echo pairs (`Agent Plasticity` and `Raw-RAG`) are same-URL only under the arXiv
`abs|pdf|html` equivalence; with strict URL equality they would join the cross-URL set. That is the point
of normalizing rather than comparing strings.

## The corpus already names this failure

`inbox/memory-echo-repeated-write-back-gives-old-claims-retrieval-votes-and-fpa-stops-it-without-deleting-history.md`
imports the Memory Echo paper (arXiv 2610.17763) and its FPA mechanism: one representative per source family,
a provenance gate, and an authority-guided answer. `inbox/arsumbris-improvement-backlog-from-the-memory-echo-wave.md`
proposes Stage-1 of FPA for `au-engine` ranking and says `family:` is already on 263 source-derived notes.

The doctrine is present. The measurement below is of the assumption underneath it: that `family:` is a
reliable source key, and that the failure is visible at retrieval time.

## The numbers

The 100 same-source multi-note groups partition into four shapes:

| shape | groups | what it is |
|---|---:|---|
| duplicate extraction (echo) | 11 | two notes re-extract one source and assert the same claims |
| source note + derived pattern | 31 | one `extract`/`insight` and one `pattern` note; usually the same evidence at a different abstraction, but the label is structural, not a claim judgment |
| provenance collision | 3 | the URL belongs to one note and was copied onto an unrelated synthesis or log, or is a bare site root shared by two documents |
| facets only | 55 | different claims from one source; legitimately N |

The 11 same-URL echoes hold 22 notes. Six further echo clusters hold 14 notes; they do not appear in this
partition because their members have different or absent `source_url` values.

Internal links are common in the groups that are not echoes: 24 of the 32 knowledge-only groups and 48 of
the 66 inbox-only groups have at least one `tb_references` edge, and 18 and 25 respectively have a mutual
one. The echo clusters are the exception: 5 of 17 have any directional link and none is mutual.

**36 notes collapse to 17 observations.** The over-count is 2.1x. All 36 carry `status: current`, so a
status/provenance gate cannot demote any of them.

## Three-way classification, with evidence

### A. Echo: same claim restated

**Entity resolution — a triple, and a contradiction.** Three knowledge notes, all `status: current`, all
saying the same thing:

- `entity-resolution-accuracy-below-85-percent-corrupts-graph.md` — "Entity resolution below 85% accuracy causes cascading errors; Dr. Smith the doctor merges with Dr. Smith the movie character"
- `entity-resolution-accuracy-below-90-percent-corrupts-knowledge-graph-reasoning-chains.md` — "…errors compound with every traversal hop and must exceed 90% before enabling graph retrieval" (source: `senzing.com`)
- `entity-resolution-below-85-percent-cascades-into-graph-corruption.md` — "Entity Resolution ниже 85% приводит к каскадным ошибкам при обходе графа — Dr. Smith doctor и Dr. Smith movie character становятся одним узлом"

The first sentence makes the same claim in all three ("Entity resolution (ER) is the single hardest problem in
knowledge graph construction"). The threshold is not: two notes say 85%, one says 90%. Neither note links to
the other, and only one has a `source_url`. This is the worst shape: one observation counted three times,
with a number that disagrees with itself and no way for a reader to see the disagreement.

**OO-Harness — a same-URL pair.** `inbox/oo-harness-evolving-harness-classes-oop.md` (2026-10-26) and
`inbox/oo-harness-object-oriented-harness-evolution.md` (2026-10-22), both `source_url:
https://arxiv.org/abs/2610.28380`, both `family: f/arxiv-2610.28380`. First sentences: "Харнес = исполняемый
код (промпты, тулы, control flow) вокруг замороженной LLM" and "Харнесс = исполняемый код вокруг модели
(промпты, тулы, context management, control flow)". Same paper, same claim, same family; one links to the
other in one direction, so the pair is at least visible. It is still two notes for one observation.

**JevBench — two snapshots of one board, both current.** `inbox/jevbench-independent-board.md` (2026-10-17)
says "42 системы, четыре оси"; `inbox/jevbench-baseline-v1-7-22.md` (2026-10-29) says "316 систем, метод v1.7".
Same URL, same board, same four axes. The later note supersedes the earlier one's numbers, but there is no
`supersedes:` and no link; both are `status: current`. A reader who finds the 42-system note has no signal
that 316 systems is the current count.

The remaining same-URL echoes are the same shape: `agent-beacon` (1,961 vs 2,590 stars, two extracts),
`Agent Plasticity` (two extracts, one `abs` and one `html` URL), `JevAgentHarness` (insight + extract),
`Raw-RAG` (two extracts), `awesome-reverse-engineered` (two extracts; one counts 644 stars and 16,814 projects, the other says "16k+ projects" and adds the nightly pipeline), `mirror`
(188 vs 201 stars), `open-ontologies` (1.4k vs 2.2k stars), and `Meta JiTTesting` (two extracts ten days
apart). In every case the body is a paraphrase, not a copy.

**The cross-URL echoes are the dangerous half**, because no source-URL mechanism can see them. All six are in
`knowledge/`, and 13 of their 14 notes have no `source_url` — but every one carries a free-text `provenance:`
of the form "Extracted from ZeroClaw `.planning/research/<FILE>.md` on 2026-02-2x". The two extraction waves
are 2026-02-21/22 and 2026-02-27. Five of the six clusters re-extract the same file (`PITFALLS.md`,
`SUMMARY.md`, `MNEMOSYNE_ANALYSIS.md`, `SECURITY_MODEL.md`); the dual-stream pair draws on `SUMMARY.md` +
`PITFALLS.md` against `ARCHITECTURE.md` for the same pattern. The source identity is recorded; nothing reads
it as a key. That is also why the entity-resolution contradiction exists: `PITFALLS.md` was re-extracted and
the second note wrote 85% where the first wrote 90%, with no source change.

| claim | notes | evidence |
|---|---|---|
| entity resolution | 3 | quoted above; 85% vs 90% |
| fact-extraction precedes graph | 3 | "fact extraction → semantic memory → entity memory → knowledge graph" repeated verbatim in all three descriptions |
| dual-stream ingestion | 2 | both say fast sync path (<100ms) + async consolidation; one names MAGMA, one does not |
| Mnemosyne hybrid weights | 2 | both say FTS5 20% + graph 10% + vector 70% for sub-2ms on 1,000 memories |
| defense-in-depth | 2 | both list policy, auth, secrets, sandboxing, audit, supply chain |
| ZeroClaw stack choice | 2 | both choose hnsw_rs + tantivy + petgraph + fastembed; one adds the RuVector rejection |

### B. Different facets: legitimately N

These are the groups that look like echoes to a lexical detector and are not:

- `agents-of-chaos-*` — 16 notes from arXiv 2602.20021, one per failure mode (guilt trip, identity spoofing,
  indirect injection, silent censorship, …). Each title is a different proposition.
- `claude-flow-*` — 13 notes from one repository, one per subsystem (15-agent mesh, 171 MCP tools, hybrid
  memory, three consensus protocols, …).
- `lavrov-ch*` — 13 chapter notes from one textbook, one per chapter.
- `hermes-stories-*` — 15 category inventories from one page, each a different category count.
- `gdoc:1RWwW4aB…` — 14 notes from one Google Doc, each a different ontology/verification claim.

The control is `https://blog.rust-lang.org/releases`: an `extract` of the 15 releases and a `moc` that maps
them by usefulness. The two notes are mutually linked, share the source, and make different claims (what
changed vs which changes matter for this stack). That is the shape the corpus should produce for a source
with more than one facet, and it is why this pair is **not** counted as an echo.

### C. Same URL, unrelated content: provenance bugs

- `arxiv:2611.05953` is a survey on graph-mediated LLM systems. The same URL is on
  `inbox/news-corpus-synthesis-2026-11-segment.md`, a weekly synthesis of four unrelated convergences that
  cites the survey as one of several inputs. One `source_url` cannot represent a multi-source synthesis.
  (The other two members of that group are a legitimate survey-extract + pattern pair; the collision is the
  synthesis note only.)
- `gh:badtheorylabs/interference-search` is on `inbox/frontier-orchestrator-phase-one-closed-with-verified-live-run.md`,
  a project log about a local orchestrator, not an extraction of that repository.
- `https://hotpath.rs` is a site root on two unrelated documents: an `au-engine` build profiling and a
  `hotpath-rs` library analysis. 21 `http(s)` source URLs in the corpus point at a bare site root; 3 of
  those roots carry more than one note.

These are not echoes, but they corrupt any source-level count: they make one source look like it supports a
claim it never made.

## What the graph already declares

For each of the 17 echo clusters, I checked the three kinship signals named in the task: shared `family:`,
mutual `tb_references` edges, and a shared `supersedes:`/`derived_from:`.

| signal | echo clusters declaring it |
|---|---:|
| shared `family:` | 4 / 17 |
| mutual wikilink | 0 / 17 |
| `supersedes:` / `derived_from:` | 0 / 17 (the fields do not exist anywhere in the vault) |
| **any of the above** | **4 / 17 (24%)** |
| any one-directional link | 5 / 17 |

The 4 family declarations are `agent-beacon`, `Agent Plasticity` and `OO-Harness` (all inbox same-URL pairs)
and `Mnemis dual-route` (the knowledge same-URL pair). The other 7 same-URL echo clusters do **not** share a family, even
though they share an exact `source_url`:

- `f/arxiv-decision-models` vs `f/arxiv-2610.21412`
- `f/benchmarkheaven` vs `f/benchmark-heaven` (a hyphen)
- `f/gh-fabio-rovai` vs `f/gh-fabio-rovai-open-ontologies`
- `f/reflection-beam` vs `f/gh-reflection-oss-mirror`
- `f/meta-engineering` vs `f/agent-architectures`
- `f/arxiv-2610.30755` vs `f/raw-rag-baseline`
- `f/gh-alexgaoth-awesome-reverse-engineered` vs `f/re-tooling-index`

`family:` is therefore not a canonical source key. Across the whole corpus, 43 family values span more than
one source identity and 46 source identities carry more than one family. In `inbox/`, `f/patterns` alone
spans 36 different sources and `f/agent-architectures` spans 20; using them for FPA would merge unrelated
pages into one observation. In `knowledge/` the field is much cleaner — 0 identities carry two families —
but 8 families still span more than one source identity (`f/docs-typesafe-ai` covers 13 pages,
`f/reddit-com` covers 9 posts), and 41 of the 269 source-bearing notes have no family at all (15 of the 16
`hermes-stories` notes among them). All 115 notes without a `source_url` also have no family.

`provenance:` is the exception the graph does not use. Five of the six cross-URL clusters name the same
`.planning/research/*.md` file there, and 900 of the 944 notes carry a `source_url` or a `provenance` string.
The raw material for a source key is present on 95% of the corpus; it is free text, and `tb_references` does
not return it, so the graph's declared kinship is still 4/17.

Counting a shared provenance file as a source declaration raises the number to 9/17: the 4 family cases plus
the 5 cross-URL clusters that name the same planning file. But **none of the 17 has a claim key.** Even the 9
can answer only "these notes share a source", not "these notes assert the same claim" — and the 55 facet
groups are the proof that a shared source is not a shared claim. The corpus is one computed field away from
source-lineage and two fields away from claim-lineage.

That last set is where the cross-URL echoes live: **13 of the 14 notes in the six cross-URL clusters have no
`source_url` and no `family`.** FPA Stage 1 on `family:` therefore reads them as six sets of independent
notes; a source key derived from `provenance:` would see five of the six. The remaining one (dual-stream)
shares the planning directory and the pattern, not the file. That is the mechanism's blind spot as it is
currently built, and it is exactly where the corpus's duplicate density is highest: 13 of 115 no-source
knowledge notes are in an echo cluster (11.3%), against 22 of 355 source-bearing notes in multi-note groups
(6.2%).

Two more facts matter for a serving-time fix:

1. **All 36 echo notes are `status: current`.** The provenance gate has nothing to demote.
2. **Lexical dedup finds none of them.** Over the 21 true echo pairs, 5-gram Jaccard on the first 4,000
   characters has mean 0.021, maximum 0.140, and **zero pairs above 0.30**. This extends the finding in
   `docs/research/self-retrieval-2026-11-16.md` that description similarity is not a duplicate detector:
   body similarity is not one either, because the re-extractions are paraphrases in another language or at
   another length.

## What a mechanism would need

Three facts, not a design:

1. **A canonical `source_key` per note, computed, not authored.** Normalize `source_url`, and fall back to
the file path in `provenance:` (the `.planning/research/<FILE>.md` pattern here). The rules this measurement
used: fragment, trailing slash, `www.`, tracking query, arXiv `abs|pdf|html`, Google Doc id, GitHub
owner/repo; for provenance, the first `*.md` path. Return the key with the note. This is derivable from what
900 of 944 notes already carry; it needs no migration and no author discipline. It replaces `family:` as the
grouping key, and `family:` can then be checked against it rather than trusted.
2. **A `claim_key` that is independent of the source.** A source key alone over-collapses in the other
direction: one `.planning/research/SUMMARY.md` yields the fact-chain, dual-stream and stack claims, and one
paper yields the 16 `agents-of-chaos` claims. The six cross-URL clusters need claim grouping because their
notes share a source that also contains other claims. A claim key cannot be the raw title (the titles differ
in length and language) and cannot be lexical similarity (0 pairs above 0.30). The cheapest honest version
is an author-declared `claim:` slug on notes that assert a finding, plus a review step that proposes slugs
for existing clusters. Without this fact, a source key would count 16 legitimate `agents-of-chaos` facets as
one observation while still missing the dual-stream pair.
3. **A per-claim independent-source count, with null treated as unknown, not as one.** For a set of notes
   asserting a claim, return `{notes: N, sources: M, unknown_source: K}`. `M` counts distinct `source_key`
   values (URL or provenance path); `K` counts notes with neither. The reader's question is "how many
   independent observations", and the honest answer for the cross-URL clusters is "1 claim, 1 source" for the
   five that share a provenance file and "1 claim, 2 files, same pattern" for dual-stream — not 2 or 3 notes.

**The smallest thing that ships first** is a `lineage` block in the existing `tb_references` payload:

```json
"lineage": {
  "source_key": "arxiv:2602.15313",
  "source_notes": 4,
  "claim_key": "mnemis-dual-route",
  "claim_notes": 2,
  "independent_sources": 1
}
```

That is one added key on a read that already returns the note's edges; it makes the 11 same-URL echoes
visible and makes the 7 inconsistent-family cases irrelevant. The `claim_key` half is the second ship, and
it is the one that covers the 6 cross-URL clusters. Per `docs/research/ruvector-crates-2026-11-16.md` §8,
the parent-field experiment is already specified; the answer from this corpus is that **0 notes carry
`supersedes:` or `derived_from:`** (the only relative field anywhere is one `generated_from:` in a hygiene
note), so the first deliverable is the computed key, not a request for authors to populate one.

What the mechanism should **not** do is delete the echoes. The existing backlog is right on this point:
collapse at retrieval, keep the history. A lineage read that reports "N notes, M sources" (in
`tb_references` first, a `tb_lineage` call later) is enough
for a context pack to stop counting one source as N; deletion would destroy the audit trail that shows the
re-extraction happened.

## Limits

- Claim identity is a judgment, not a measurement. I read every member of every echo cluster and quoted the
  first sentence and description; a different judge could move a borderline pair (the Meta pair, the
  awesome-reverse-engineered pair) between "echo" and "facets".
- The six cross-URL clusters were found by title/description screening, not exhaustive claim clustering.
  There may be more among the 115 no-source knowledge notes; the 11.3% density is a lower bound.
- Singleton sources (465 of 565) were not screened for cross-source echoes. The measurement covers the 355
  notes that share a source with at least one other note, plus the knowledge title/description sweep.
- `tb_references` resolves in-vault wikilinks only. A kinship declared outside the vault would not appear.
- Read-only: no file in `/root/vault` was changed.

## Appendix: the 17 echo clusters

| # | group | n | scope | shared family | mutual link | notes |
|---|---|---:|---|---:|---:|---|
| 1 | `gh:asymptote-labs/agent-beacon` | 2 | inbox | yes | no | `agent-beacon-cross-harness-memory`, `beacon-cross-harness-session-memory` |
| 2 | `arxiv:2610.08902` | 2 | inbox | yes | no | `agent-plasticity-...`, `measuring-how-agents-learn-...` |
| 3 | `arxiv:2610.21412` | 2 | inbox | no | no | `allspark-jev-agent-harness-...`, `jevagentharness-decision-models-...` |
| 4 | `arxiv:2610.28380` | 2 | inbox | yes | no | `oo-harness-evolving-harness-classes-oop`, `oo-harness-object-oriented-harness-evolution` |
| 5 | `arxiv:2610.30755` | 2 | inbox | no | no | `raw-rag-beats-agentic-memory-baseline`, `raw-rag-beats-agentic-memory` |
| 6 | `gh:alexgaoth/awesome-reverse-engineered` | 2 | inbox | no | no | `awesome-reverse-engineered-catalog-automation`, `awesome-reverse-engineered-index` |
| 7 | `gh:reflection-oss/mirror` | 2 | inbox | no | no | `mirror-harness-vs-allspark`, `mirror-reflection-coding-harness-patterns` |
| 8 | `gh:fabio-rovai/open-ontologies` | 2 | inbox | no | no | `open-ontologies-proof-carrying-rust-engine`, `open-ontologies-vs-arsumbris` |
| 9 | `https://benchmarkheaven.com/jev-models/api` | 2 | inbox | no | no | `jevbench-baseline-v1-7-22`, `jevbench-independent-board` |
| 10 | `https://engineering.fb.com/.../jittesting` | 2 | inbox | no | no | `meta-catching-jittests-production`, `meta-jitesting-human-on-the-loop` |
| 11 | `arxiv:2602.15313` | 2 | knowledge | yes | no | `dual-route-memory-...`, `mnemis-dual-route-memory-...` |
| 12 | claim: entity resolution | 3 | knowledge | no | no | `entity-resolution-accuracy-below-85-...`, `...-below-90-...`, `entity-resolution-below-85-...` |
| 13 | claim: fact-extraction order | 3 | knowledge | no | no | `fact-extraction-must-precede-...`, `zeroclaw-dependency-chain-...`, `zeroclaw-phase-ordering-...` |
| 14 | claim: dual-stream ingestion | 2 | knowledge | no | no | `dual-stream-ingestion-separates-...`, `zeroclaw-architecture-follows-dual-stream-...` |
| 15 | claim: Mnemosyne hybrid weights | 2 | knowledge | no | no | `mnemosyne-hybrid-search-three-component`, `mnemosyne-hybrid-search-weights-...` |
| 16 | claim: defense-in-depth | 2 | knowledge | no | no | `zeroclaw-defense-in-depth-six-independent-...`, `zeroclaw-defense-in-depth-six-layers` |
| 17 | claim: ZeroClaw stack choice | 2 | knowledge | no | no | `zeroclaw-stack-hnsw-rs-...`, `zeroclaw-stack-validated-...` |

**The number for the card: 13.** Thirteen of the 17 echo clusters carry no kinship signal at all; the
corpus counts 36 notes where it has 17 independent observations, and the 6 clusters that source-URL grouping
cannot even see hold 14 of the 36 notes (13 of them with no `source_url`; 5 of the 6 share a provenance
file, so a provenance-aware key would surface them without a new field).
