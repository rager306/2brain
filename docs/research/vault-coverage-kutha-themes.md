# Vault coverage of the Kutha ADR themes

Audit of what `/root/vault` already knows about the ideas a sister project (`/root/kutha-graph`, ADR-000–094) is researching, and how the vault's search behaves while answering.

- **Workspace:** `/root/vault`, served over MCP stdio: `HOME=/root /usr/local/bin/2brain mcp /root/vault`.
- **Engine:** `2brain 0.1.0+be4a56aaaa6a` (binary mtime 2026-11-16 13:46), read tools only.
- **Date:** 2026-11-16.
- **Corpus at audit time:** 1,065 markdown files — 384 in `knowledge/`, 559 in `inbox/`, 63 in `f/`, 59 elsewhere. Graph: 1,213 nodes, 4,689 edges, 0 broken wikilinks, 54 topic maps, 204 isolated nodes.
- **Method:** 62 `tb_search` calls (61 distinct query strings; one repeated with `scope=own`) and 23 `tb_grep` literals (full ledger in the appendix). Every path and score below came from a result page. `/root/kutha-graph/docs/ADR` filenames were read for vocabulary only; it was never searched.
- **Read-only:** sha256 over all 1,065 markdown files is identical before and after the audit. The only file written is this report.

**Counts: 4 covered, 9 touched, 2 absent** (15 themes; the legal/scientific row is split in the verdict, legal = touched, scientific = absent).

## 1. Theme table

| # | Theme | Verdict | Primary query and its top three (`score`) |
|---|---|---|---|
| T1 | Event log as source of truth | **covered** | 'event log as source of truth' → 330 hits — `knowledge/kutha-temporal-graphs.md` 75.24; `knowledge/surrealdb-agent-memory-vs-kutha.md` 69.74; `knowledge/kutha-graph-project-snapshot.md` 60.58 |
| T2 | Bitemporal facts and invalidation | **covered** | 'valid time transaction time half-open intervals' → 66 hits — `knowledge/bitemporal-valid-transaction-time.md` 169.12; `knowledge/surrealdb-agent-memory-temporal-api-fields.md` 61.07; `knowledge/kutha-temporal-graphs.md` 38.68 |
| T3 | Snapshots and tiered time-travel | **touched** | 'snapshots and tiered time-travel' → 111 hits — `knowledge/ruvector-temporal-tensor-compresses-vectors-3x-to-10x-via-hot-warm-cold-access-pattern-tiers.md` 37.30; `knowledge/ruvector-raft-implements-paper-complete-consensus-with-consistent-hashing-for-distributed-deployment.md` 34.65; `knowledge/surrealdb-stack-map.md` 27.09 |
| T4 | Cascade budgets and receipts | **touched** | 'cascade budgets and receipts' → 54 hits — `knowledge/the-cascade-pattern-cheapest-first-escalate-only-when-needed-governs-every-resource-decision-in-ruvnet.md` 30.88; `f/patterns/pattern-budgets-as-trades.md` 27.39; `f/patterns/pattern-evidence-gates-process-vs-completion.md` 27.11 |
| T5 | Hybrid retrieval composition | **covered** | 'hybrid retrieval composition lexical plus graph expansion' → 156 hits — `knowledge/zeroclaw-retrieval-uses-mnemis-dual-route-system-1-vector-plus-system-2-graph.md` 59.48; `knowledge/mnemosyne-hybrid-search-three-component.md` 46.80; `f/patterns/pattern-static-embedding-hybrid-search.md` 41.65 |
| T6 | Temporal "as of" queries | **covered** | 'temporal queries as of date' → 267 hits — `knowledge/bitemporal-valid-transaction-time.md` 44.60; `knowledge/kutha-temporal-graphs.md` 42.37; `knowledge/premature-graphrag-builds-graph-before-validating-need.md` 38.08 |
| T7 | Fork and diff of a knowledge base | **touched** | 'fork and diff knowledge base' → 217 hits — `inbox/pgx-patterns-for-kutha-graph.md` 53.82; `knowledge/sona-combines-micro-lora-ewc-plus-plus-and-reasoningbank-for-sub-millisecond-adaptive-learning.md` 30.23; `self/methodology.md` 25.48 |
| T8 | Strict replay | **touched** | 'strict replay' → 3 hits — `knowledge/event-log-as-source-of-truth-agent-memory.md` 39.42; `inbox/pgx-patterns-for-kutha-graph.md` 14.17; `knowledge/pahf-framework-for-continual-agent-personalization.md` 8.78 |
| T9 | Gated regimes / loops | **touched** | 'gated regime loop' → 3 hits — `inbox/matsakis-agentic-links-deepdive.md` 18.36; `inbox/rem-experience-memory-lifecycle.md` 8.87; `knowledge/hermes-stories-marketing.md` 6.92 |
| T10 | Meta-prompt dictionaries | **touched** | 'meta-prompt dictionaries' → 12 hits — `knowledge/agents-of-chaos-prompt-injection-structural.md` 22.44; `inbox/meta-privacylib-data-access-boundary.md` 20.94; `knowledge/hermes-stories-meta-ecosystem.md` 20.51 |
| T11 | Enrichment packs | **absent** | 'enrichment pack' → 0 hits; nearest 'enrichment' → `knowledge/amem-zettelkasten-retroactive-evolution.md` 25.34; `knowledge/memory-evolution-background-jobs.md` 25.08; `inbox/arsumbris-fork-backlog.md` 23.33 |
| T12 | Portable capsules | **touched** | 'portable capsules' → 0 hits; the idea as 'RVF container' → `knowledge/ruvector-rvf-container-self-bootstrapping-design.md` 62.88; `knowledge/rvf-container-is-self-bootstrapping-embedding-vectors-index-wasm-microkernel-and-linux-unikernel.md` 61.13 |
| T13 | Legal / scientific reference packs | **touched / absent** | 'legal reference pack' → 18 hits — `knowledge/legal-text-analytics.md` 43.65; `knowledge/legal-text-analytics-lli-moc.md` 42.86; `inbox/github-rubicon-git-storage.md` 40.83 |
| T14 | Oracle matrices for tests | **touched** | 'oracle matrix tests' → 5 hits — `inbox/insight-verifiable-artifact-beats-green-gates.md` 29.96; `inbox/matsakis-agentic-links-deepdive.md` 16.68; `inbox/egm-vs-superpowers-vs-compound-engineering.md` 7.77 |
| T15 | ABAC slices | **absent** | 'ABAC attribute based access control slices' → 34 hits — `knowledge/ruvector-temporal-tensor-compresses-vectors-3x-to-10x-via-hot-warm-cold-access-pattern-tiers.md` 38.83; `inbox/chime-attribute-before-memorize-separate-plan-and-execution-banks.md` 31.14; `inbox/perfr-c2rust-performance-regressions.md` 29.76 |

Reading the verdicts:

- **covered** — a note really addresses the idea, not merely the words; the note's body was opened and read.
- **touched** — the idea appears as a clause, a table row, or a status line inside another subject, or the field's word maps to a different subject in this corpus (the `regime` case).
- **absent** — no note addresses it; literal search also finds no phrase.

## 2. Theme detail

### T1 — Event log as source of truth — **covered**

`tb_search "event log as source of truth"` → 330 hits: `knowledge/kutha-temporal-graphs.md` 75.24, `knowledge/surrealdb-agent-memory-vs-kutha.md` 69.74, `knowledge/kutha-graph-project-snapshot.md` 60.58. The dedicated atomic note exists but lands at **rank 8** (36.63, 6/6 terms): `knowledge/event-log-as-source-of-truth-agent-memory.md` — "лог первичен; граф, память, эмбеддинги — проекции/optional views". `tb_search "append-only event log fold projection"` → 105 hits, same top three (snapshot 95.74, surrealdb 89.54, hub 79.38). Covered; the ranking miss is D7.

### T2 — Bitemporal facts and invalidation — **covered**

`tb_search "valid time transaction time half-open intervals"` → 66 hits: `knowledge/bitemporal-valid-transaction-time.md` **169.12** (highest score in the audit), `knowledge/surrealdb-agent-memory-temporal-api-fields.md` 61.07, `knowledge/kutha-temporal-graphs.md` 38.68. `tb_search "bitemporal facts invalidation"` → 6 hits: `knowledge/graphiti-bitemporal-fact-edges-contrast.md` 27.60, `knowledge/temporal-kg-white-space.md` 15.91, hub 15.19. The note covers valid × transaction time, half-open intervals, `as_of_tt`, and the correction-vs-evolution split; `knowledge/zeroclaw-bi-temporal-edge-model-tracks-validity-and-ingestion-separately.md` adds the four-timestamp edge model. Covered.

### T3 — Snapshots and tiered time-travel — **touched**

`tb_search "snapshots and tiered time-travel"` → 111 hits, none about storage snapshots: `knowledge/ruvector-temporal-tensor-compresses-vectors-3x-to-10x-via-hot-warm-cold-access-pattern-tiers.md` 37.30, `knowledge/ruvector-raft-implements-paper-complete-consensus-with-consistent-hashing-for-distributed-deployment.md` 34.65, `knowledge/surrealdb-stack-map.md` 27.09. `tb_search "snapshot tiers vacuum"` → **0 hits**; `tb_search "vacuum"` → **0 hits** (the only occurrence in the corpus is a broken token, "блокиVacuum/WAL-рост", at column 423 of line 57 of `knowledge/rust-top20-directions.md`). What exists is the snapshot *as a query point*: `knowledge/bitemporal-valid-transaction-time.md` — "Снимок графа — `G(t, tt)`, каждый оператор принимает `as_of_tt`" — and `knowledge/surrealdb-stack-map.md` line 31, "VERSION clause = store-level time travel". No note on snapshot tiers, vacuum, or tiered retention. Touched.

### T4 — Cascade budgets and receipts — **touched**

`tb_search "cascade budgets and receipts"` → 54 hits, and the top hit is a different cascade: `knowledge/the-cascade-pattern-cheapest-first-escalate-only-when-needed-governs-every-resource-decision-in-ruvnet.md` 30.88 (RuVNet resource decisions), then `f/patterns/pattern-budgets-as-trades.md` 27.39 and `f/patterns/pattern-evidence-gates-process-vs-completion.md` 27.11. `tb_search "cascade budget receipt"` → 4 hits, all incidental (top `inbox/icm-agent-memory-architecture-patterns.md` 13.13). The receipt half is real and reusable: `f/patterns/pattern-verification-receipt-commits.md` (`tb_grep "receipt"` → 49 lines, this file first: commit message as receipt, differential test, explicit *Not-verified* section), and `knowledge/kutha-graph-project-snapshot.md` names "checkpoint receipt (M154)". The budget half is `pattern-budgets-as-trades`. No cascade-budget note (ADR-014 has no analogue). Touched.

### T5 — Hybrid retrieval composition (lexical + graph expansion) — **covered**

`tb_search "hybrid retrieval composition lexical plus graph expansion"` → 156 hits: `knowledge/zeroclaw-retrieval-uses-mnemis-dual-route-system-1-vector-plus-system-2-graph.md` 59.48, `knowledge/mnemosyne-hybrid-search-three-component.md` 46.80, `f/patterns/pattern-static-embedding-hybrid-search.md` 41.65; also `knowledge/hybrid-search-fusion-tuning-risk.md` 34.86 (typed `error`, quantifies the 70/20/10 vector/FTS/graph weights) and `knowledge/amem-search-agentic-graph-traversal.md` 24.85 (vector search then one-hop BFS over links). Three distinct angles, not three paraphrases. Covered. The `ruvector hybrid search` query is noisier (D4).

### T6 — Temporal queries ("as of") — **covered**

`tb_search "temporal queries as of date"` → 267 hits: `knowledge/bitemporal-valid-transaction-time.md` 44.60, hub 42.37, `knowledge/premature-graphrag-builds-graph-before-validating-need.md` 38.08. `tb_search "as of query cypher temporal"` → 300 hits: `knowledge/kutha-graph-project-snapshot.md` 52.41, `knowledge/surrealdb-agent-memory-vs-kutha.md` 50.12, bitemporal note 44.60. The snapshot names the exact surface: `as_of` / `as_known_on` / `live_at`; `knowledge/surrealdb-agent-memory-temporal-api-fields.md` names the `asOf` field. Covered. The phrase itself cannot be enforced (D2).

### T7 — Fork and diff of a knowledge base — **touched**

`tb_search "fork and diff knowledge base"` → 217 hits: `inbox/pgx-patterns-for-kutha-graph.md` 53.82, `knowledge/sona-combines-micro-lora-ewc-plus-plus-and-reasoningbank-for-sub-millisecond-adaptive-learning.md` 30.23, `self/methodology.md` 25.48 (the vault's own extract/diff workflow). `tb_search "fork at event shared prefix structural diff"` → 89 hits: pgx 54.31, `knowledge/event-log-as-source-of-truth-agent-memory.md` 47.86 (6/6 terms), snapshot 36.52. Substance is three one-line mentions: snapshot "`fork_at` fail-closed (M155), fact-set `diff_folds` (M156)"; the event-log note "fork-at-event с shared prefix + структурным diff"; the pgx extract treats COW branching as a measured fork cost. No note explains the design. Touched.

### T8 — Strict replay — **touched**

`tb_search "strict replay"` → only **3 hits**: `knowledge/event-log-as-source-of-truth-agent-memory.md` 39.42, `inbox/pgx-patterns-for-kutha-graph.md` 14.17, `knowledge/pahf-framework-for-continual-agent-personalization.md` 8.78. `tb_search "deterministic replay from event log"` → 109 hits: `knowledge/surrealdb-agent-memory-vs-kutha.md` 73.33, snapshot 66.84, `inbox/vercel-durable-harness.md` 56.13. The idea is present only as clauses — "strict vs permissive replay" in the event-log note, and in `knowledge/droppable-materializations-pattern.md`: "durable log гарантирует replay до последнего кванта" and "тот же лог → та же проекция". Touched, thin.

### T9 — Gated regimes / loops — **touched**

`tb_search "gated regime loop"` → 3 hits, all wrong: `inbox/matsakis-agentic-links-deepdive.md` 18.36, `inbox/rem-experience-memory-lifecycle.md` 8.87, `knowledge/hermes-stories-marketing.md` 6.92. `tb_grep "regime"` → 9 lines, every one KARLBench's *reasoning regimes* (`knowledge/karlbench-six-grounded-reasoning-regimes.md`), a different subject. The gate-before-continue idea does exist under other names: `f/patterns/pattern-evidence-gates-process-vs-completion.md` ("a stage finishing is not a stage passing"), `f/patterns/pattern-impact-gated-skill-consolidation.md`, `knowledge/model-checking-as-ci-gate.md`. `tb_search "gate before loop escalation"` → 78 hits, top `knowledge/zeroclaw-agent-loop-enforces-max-10-tool-iterations-and-auto-trims-history-at-50-messages.md` 27.43. Touched: gates yes, a gated loop no.

### T10 — Meta-prompt dictionaries — **touched**

`tb_search "meta-prompt dictionaries"` → 12 hits; four of the top five are unrelated (`knowledge/agents-of-chaos-prompt-injection-structural.md` 22.44, `inbox/meta-privacylib-data-access-boundary.md` 20.94, `knowledge/hermes-stories-meta-ecosystem.md` 20.51, `inbox/harness2-contrastive-propose-probe-compose-evolves-harnesses-without-verifiers.md` 17.91), and the note that addresses it is fifth: `knowledge/dict-first-agent-interface-pattern.md` 13.94. `tb_grep "meta-prompt"` → 2 lines, both inside that note ("6 dictionaries, meta-prompt@version"); the note body describes "pack типизированных словарей". `knowledge/kutha-graph-project-snapshot.md` line 75 names "ADR-050 six dictionaries". Touched: the mechanism, not the name.

### T11 — Enrichment packs — **absent**

`tb_search "enrichment pack"` → **0 hits**. `tb_search "enrichment packs"` → **0 hits**. `tb_search "genai enrichment pack optional"` → 4 hits, none on the idea: `knowledge/amem-zettelkasten-retroactive-evolution.md` 25.96, `inbox/neo4j-ontology-graph-specification-open-ai-first.md` 20.10, snapshot 12.45, `.claude/skills/ralph/SKILL.md` 4.69. `tb_search "enrichment"` → 25 hits at 1 term; the knowledge hits (`amem-zettelkasten-retroactive-evolution`, `memory-evolution-background-jobs`) are about memory evolution, not an optional enrichment pack. `tb_grep "enrichment"` → 164 lines, 73 in `.claude/skills`, 63 in `ops/`. Absent.

### T12 — Portable capsules — **touched** (the idea exists under another name)

`tb_search "portable capsules"` → **0 hits**; `"portable capsule"` → 0; `"capsules"` → 0. `tb_search "capsule"` → 2 hits, both a different sense: `inbox/statcraft-safe-rust-spss-alternative.md` 35.89 (CapsuleCraft, `.capsule/` with 341 RFC 5322 `.eml` files) and `inbox/evolver-gene-experience-framework.md` 12.37 (Evolver's Gene/Capsule/AntiGene). The portable-unit idea itself is documented as an **RVF container**: `tb_search "RVF container"` → 15 hits, `knowledge/ruvector-rvf-container-self-bootstrapping-design.md` 62.88 and `knowledge/rvf-container-is-self-bootstrapping-embedding-vectors-index-wasm-microkernel-and-linux-unikernel.md` 61.13 — one file, 23 segment types, embedded WASM, COW branching. `tb_search "portable"` → 8 hits with the RVF note at rank 1 (40.08). The concept is covered; the field's word is not. See D6 and D11.

### T13 — Legal / scientific reference packs — **legal touched, scientific absent**

`tb_search "legal reference pack"` → 18 hits: `knowledge/legal-text-analytics.md` 43.65, `knowledge/legal-text-analytics-lli-moc.md` 42.86, `inbox/github-rubicon-git-storage.md` 40.83 (wrong), `knowledge/legal-norm-ontology-classification.md` 37.38. The legal domain is deep — the MOC links LLI's 21 tasks and 130+ resources, jurisdiction dataset maps, `implicit-legal-reference-extraction`, `legal-qa-complexity-boundary`, `pullenti-decree-model-bridge` — but there is no "reference pack" as a packaged artifact; the snapshot only says "ADR-090 ontology / legal pack | STATE names legal pack". `tb_search "scientific archive pack evidence graph"` → 125 hits, none on it: `inbox/jev-system1-system2-four-beats-video-haystack.md` 48.22, `.claude/skills/review/SKILL.md` 45.79, `inbox/github-rubicon-git-storage.md` 31.90. `tb_grep "scientific"` → 10 lines, mostly the review skill. `tb_grep "reference pack"` → 0. Scientific archive pack: absent.

### T14 — Oracle matrices for tests — **touched**

`tb_search "oracle matrix tests"` → 5 hits, none the oracle note: `inbox/insight-verifiable-artifact-beats-green-gates.md` 29.96, `inbox/matsakis-agentic-links-deepdive.md` 16.68, `inbox/egm-vs-superpowers-vs-compound-engineering.md` 7.77. `tb_grep "oracle matrix"` → 0. `tb_grep "oracle"` → 83 lines, first file `f/patterns/pattern-oracle-drift-ci.md`: differential runs against a live reference, and the explicit warning "не подменять oracle снапшотами его выдачи". That is the oracle idea; the matrix (ADR-023) is not there. The note is excluded by `min_should_match` (D3): with `min_should_match=1` the same query returns 84 hits and the note is **rank 3** (25.62). Touched.

### T15 — ABAC slices — **absent**

`tb_search "ABAC attribute based access control slices"` → 34 hits, all irrelevant: `knowledge/ruvector-temporal-tensor-compresses-vectors-3x-to-10x-via-hot-warm-cold-access-pattern-tiers.md` 38.83 (2/6 terms — "access pattern tiers"), `inbox/chime-attribute-before-memorize-separate-plan-and-execution-banks.md` 31.14, `inbox/perfr-c2rust-performance-regressions.md` 29.76. The nearest security note is at rank 15 (`knowledge/zeroclaw-defense-in-depth-six-layers.md` 9.59). `tb_grep "ABAC"` → 2 lines, both incidental (`inbox/kidehen-context-engines-voice.md` line 29; `inbox/pgx-patterns-for-kutha-graph.md` line 47, a licence note). `tb_grep "capability security"` → 0. `tb_search "tenant isolation multi-tenant"` → 28 hits (`inbox/pingcap-tidb-breach-claim.md` 46.57, `inbox/bashkit-virtual-bash-sandbox.md` 42.01, `knowledge/ruvector-memory-pool-provides-paged-allocation-lru-eviction-and-multi-tenant-isolation.md` 34.23): tenant isolation exists, ABAC does not. Absent.

## 3. Diagnosis, ordered by what it costs a user

Each item states the query, what I expected, what happened, and the smallest change that fixes it. Line numbers are in `/root/2brain/tb-engine/crates/tb-engine/src/search.rs` and `/root/2brain/tb-mcp-rs/src/wire.rs`.

### D1 — A Russian question cannot reach an English note; an English question reaches a Russian note only through its title

**Expected:** the vault is bilingual, so a question in either language should find a note written in the other.

**Observed:** `tb_search "битемпоральная модель ребра отслеживает валидность и время загрузки отдельно"` → 346 hits; the top three are `inbox/pattern-blocker-rubric-grading.md` 32.10, `inbox/claims-slot-graph-balance-gate-for-proofgate.md` 28.80, `inbox/hotreload-in-rust-dynamic-libraries.md` 20.19. The English note that answers it word for word — `knowledge/zeroclaw-bi-temporal-edge-model-tracks-validity-and-ingestion-separately.md` (read: "every edge carries four timestamps… facts are never deleted, only invalidated") — does not appear in 100 results. Only its Russian sibling ranks (`knowledge/bitemporal-valid-transaction-time.md` 20.95, rank 3). The reverse direction is asymmetric but survives: `tb_search "event log as source of truth"` reaches the Russian-body note `knowledge/event-log-as-source-of-truth-agent-memory.md` at rank 8 (36.63, 6/6 terms) because its **title** is English; its description is Russian ("append-only лог первичен").

**Why lexical search cannot fix it:** there is no shared token, and the corpus mixes languages inside notes ("лог первичен; граф…" next to English technical terms). This is the one place where a synonym/translation layer is genuinely needed.

**Smallest fix — corpus, not ranker:** make `description:` English on every note. The description field is weighted **3×** the body (`DESCRIPTION_WEIGHT: f64 = 3.0`, search.rs). The event-log note would rank first instead of eighth if its description said "event log is the source of truth; graph and memory are deterministic projections". A second option, engine-side and cheap: index the frontmatter `title`/`description` in a translated form when a note declares one. A vector index is not required to fix this; a bilingual description convention is.

### D2 — `exact` is an AND of terms, not a phrase; hyphenated tokens are split

**Expected:** from the tool's own description — "Use it for punctuation or a two-word phrase" — `exact` should find the phrase.

**Observed:** `tb_search "as of" exact=true` → **219 hits**, i.e. all 219 notes that contain the two words anywhere, adjacent or a page apart; the bitemporal note is first only because it repeats them (12.59). There is no adjacency requirement, so a phrase cannot be asked for. Hyphens are split too: `tb_search "meta-prompt" exact=true` → 12 hits with `matched_terms=2` for a one-token query, which is the tell that "meta-prompt" became `meta AND prompt`; the only note that contains the literal token (`knowledge/dict-first-agent-interface-pattern.md`; `tb_grep "meta-prompt"` → 2 lines, both there) ranks **5th** at 7.66 behind `knowledge/agents-of-chaos-prompt-injection-structural.md` 16.03. `tb_search "attribute-based access control"` → top hit `knowledge/ruvector-temporal-tensor-compresses-vectors-3x-to-10x-via-hot-warm-cold-access-pattern-tiers.md` 42.07 ("access pattern"), because two of the four split words are enough.

**Smallest fix:** the index already stores positions (the proximity pass reads them, `PROXIMITY_WINDOW = 5`). Add `phrase: bool` that requires consecutive positions for a quoted query, and keep the hyphenated token as well as its parts. No new index.

### D3 — `min_should_match` hides a note that answers, when the body is in the other language

**Expected:** `tb_search "oracle matrix tests"` should surface `f/patterns/pattern-oracle-drift-ci.md` — a note about differential testing against a live oracle.

**Observed:** 5 hits, none of them the note; it is **absent from 100 results** (verified by `rank_of`). `tb_grep "oracle"` proves it exists and is the first hit of 83. Its body says "корпус тестов", so English `tests` does not match; `matrix` is absent; only `oracle` matches → 1 term, and the default for a three-word query is 2. With `min_should_match=1` the same query returns 84 hits and the note is **rank 3** (25.62). The same mechanism removes `knowledge/event-log-as-source-of-truth-agent-memory.md` from some English queries where only its title matches.

**Smallest fix:** the engine already computes `best_idf` per hit. Let one **rare** term satisfy the floor: `matched_terms >= min_should_match || best_idf >= RARE_IDF`. A term that appears in ≤13% of the corpus is evidence even when it is the only one. Second choice, less principled: auto-retry with `msm-1` when a page returns fewer hits than `msm` and say so in the payload.

### D4 — Near-duplicate notes answer one question with three variants, and two of them disagree

**Expected:** one hit per claim.

**Observed:** the vault holds three entity-resolution notes: `knowledge/entity-resolution-accuracy-below-85-percent-corrupts-graph.md` (18.37, rank 18 of `tb_search "graphiti temporal facts"`), `knowledge/entity-resolution-below-85-percent-cascades-into-graph-corruption.md` (12.81, rank 19), and `knowledge/entity-resolution-accuracy-below-90-percent-corrupts-knowledge-graph-reasoning-chains.md` (`created: 2026-02-21`, five days before the second; not in the page). The first two say 85%, the third says 90% — same claim, two numbers. Two `zeroclaw-defense-in-depth-*` notes repeat the same six layers. A second flavour is same-family crowding: `tb_search "mnemosyne hybrid search"` ranks `knowledge/mnemosyne-hybrid-search-three-component.md` 70.11 first and `knowledge/mnemosyne-hybrid-search-weights-fts5-20-percent-plus-graph-10-percent-plus-vector-70-percent-for-sub-2ms-latency.md` 64.62 third — the same 70/20/10 fact twice. `tb_search "ruvector hybrid search"` puts three ruvector product notes in ranks 3–5 (`ruvector-graph-supports-n-ary-hypergraphs` 45.31, `ruvector-architecture` 44.34, `ruvector-hnsw-achieves-52k-ops-sec-with-sub-millisecond-latency` 44.04), none about hybrid search.

**Verdict:** the corpus is at fault, not the ranker. `tb_search "graphiti temporal facts"` is doing its job; it returns three notes because three exist. **Fix:** a duplicate pass at `/connect` (title-token Jaccard ≥ 0.8, or the existing content hash plus claim extraction) and merge the 85/90 pair against its source. In search, collapse near-identical titles into one hit with an `also:` list, so a page is evidence rather than variants.

### D5 — `min_should_match=2` is too lenient for long queries: 2 of 9 words is not a question

**Expected:** a long, specific question should narrow.

**Observed:** `tb_search "ABAC attribute based access control slices"` (6 terms) → 34 hits, top `knowledge/ruvector-temporal-tensor-…-access-pattern-tiers.md` 38.83 with 2/6 terms; the actual security note is at rank 15. The 9-term Russian query in D1 → 346 hits, top 32.10 with 3/9. The floor is fixed at 2 for every query longer than two terms, so adding words adds noise instead of narrowing. `matched_terms` is on the wire, so the reader can see it — but only after the page is already wrong.

**Smallest fix:** raise the floor with query length, e.g. `min(3, n)` for n > 2, and drop the lowest-idf terms above ~6 terms. Keep D3's rare-term override so the stricter floor does not repeat D3. This is the same knob as D3 pulled in the other direction; the pair needs one rule, not two defaults.

### D6 — Plurals are separate tokens: "portable capsules" returns nothing, "RVF container" returns the answer

**Expected:** `capsules` should find `capsule`.

**Observed:** `tb_search "portable capsules"` → **0**; `"portable capsule"` → **0**; `"capsules"` → **0**; `"capsule"` → 2. `tb_search "portable"` → 8 hits, and the two RVF notes are **rank 1** (40.08) and rank 5 (13.75). `"enrichment packs"` → 0 while `"packs"` → 8 and `"pack"` → 20. Whatever the corpus does or does not contain, the engine does not fold `-s`, so a user's plural silently empties the page.

**Smallest fix:** suffix folding at index and query time (`s`, `es`, and `-ies → -y`), or at minimum a query-time retry with the singular. Cheapest single win in this audit; no ranking change.

### D7 — The hub outranks the atomic note that answers the question

**Expected:** asking a note's own title finds that note first.

**Observed:** `tb_search "event log as source of truth"` → `knowledge/kutha-temporal-graphs.md` 75.24 first (a topic map that repeats the phrase in its description and body) and `knowledge/event-log-as-source-of-truth-agent-memory.md` **8th** at 36.63, with 6/6 terms matched. The title is indexed; the body is Russian, so the atomic note matches once per term, while the hub matches several times and its description is weighted 3×.

**Fix:** a title-match boost (BM25F on the H1 field, or `score *= 1 + 0.2·title_terms/n`) and a down-weight for `type: moc` when a non-MOC note matched the same terms. The hub is a legitimate second hit — it is the map — but it should not be the first answer to "what is the event-log rule".

### D8 — The documented `weak` flag never appears, and the running binary is older than the source

**Expected:** the task asked for a query where `weak: true` appeared and whether it was correct.

**Observed:** it never appears, on any of 62 searches. `tb_describe_tool tb_search` contains no `weak` string, and every match object has exactly `path`, `score`, `matched_terms`. The engine computes the flag (`Hit::weak`, `best_idf < RARE_IDF` where `RARE_IDF = 2.0`, search.rs:156–166), and the tool description at `/root/2brain/tb-mcp-rs/src/lib.rs:1091` promises it ("Each match carries `weak`: true when every term it matched is common in the corpus"), but the wire struct `SearchMatch` in `wire.rs` has only `path`, `score`, `matched_terms`, `excerpt` — no `weak`. So the signal is unreachable even at HEAD. The live binary is older still: HEAD's description also promises `excerpt`/`excerpt_chars`, and the running binary sends neither.

**Smallest fix:** add `weak: Option<bool>` to `SearchMatch` and emit it; then rebuild. Until then, the honest proxy is the absolute score — the pages in D5 (top 38.83 at 2/6 terms, and the Russian query's top 32.10 at 3/9) are exactly the ones the flag exists to label.

### D9 — `scope` defaults to `all`, so a vault question returns the harness's own docs

**Expected:** a question about the vault should search the vault.

**Observed:** the live schema says `"Select own or all members (default all)"`. `tb_search "oracle tests reference implementation differential"` returned `/root/2brain/tb-mcp-sdk/README.md` at **rank 4** (19.94); with `scope="own"` it disappears and the total drops 31 → 30. It is rare in search (1 of 798 returned hits across every recorded page, 0.13%) but common in grep: `tb_grep "gate"` returns four `/root/2brain/**/*.type.yaml` files in the first six lines.

**Fix:** default `scope=own` for `tb_search` and `tb_grep` (HEAD's description already claims own; the binary's does not — the same version drift as D8), and let `scope=all` be explicit.

### D10 — `tb_grep` returns one row per line, so one file can fill a page

**Observed:** `tb_grep "enrichment"` (limit 20) → 19 of 20 rows are `.claude/skills/extract/SKILL.md` (total 164); `tb_grep "snapshot"` → 38 of 61 lines in `inbox/`; `tb_grep "capsule"` → 4 lines in 3 files. There is no per-file grouping and no regex, by design.

**Fix:** `per_file: bool` (default true, one row per path with a count), which is what a coverage question actually wants. Regex is a separate, larger decision; `tb_grep`'s literal contract is defensible.

### D11 — The vault stores the finding under its own name, not the field's name

**Observed, four times:** `capsule` → the idea is `RVF container` (T12; the plural miss is D6); `meta-prompt dictionaries` → `dict-first` (T10); `gated regime loop` → `evidence gates` / `impact-gated` (T9); `enrichment pack` → `memory evolution` (T11). In each case a note exists, or nearly exists, and the query in the requester's vocabulary misses it. `tb_grep` cannot bridge this either: the words are simply not in the files.

**Verdict:** this is a corpus problem and the fix is a naming convention, not an index. Add an `aliases:` list to frontmatter, indexed with the description at the same 3× weight, and have `/connect` write the field's synonyms when a note is created from an outside source ("RVF container (portable capsule)"). A vector index would also bridge these, but it is not the smallest change and it would not fix D1's directionality, D2's phrases, D4's duplicates, or D6's plurals — four of the eleven defects are morphology or corpus, not semantics.

## 4. What the vault is good at

The failures above are real, but this is not a thin base. The things that work here work because the vault has been curated by a person who reads the field, and the numbers show it.

- **Temporal knowledge graphs are covered at four independent depths.** `tb_search "valid time transaction time half-open intervals"` → `knowledge/bitemporal-valid-transaction-time.md` at **169.12**, the highest score in the audit. Around it: `graphiti-bitemporal-fact-edges-contrast` (form right, core wrong), `assert-retract-correct-write-ops` (three write ops, no physical delete), `isolation-typed-contradiction-operators`, `temporal-kg-white-space` (nine under-served capabilities), `zeroclaw-bi-temporal-edge-model-…` (four timestamps), and the project snapshot with `as_of` / `as_known_on` / `live_at`. A reader asking "how should bitemporal facts work" gets a real answer, not a definition.
- **The graph holds together.** `tb_graph_health` on the vault: 1,213 nodes, 4,689 edges, **0 broken wikilinks**, 19 dead ends, 54 topic maps. Hubs are real: `knowledge/zeroclaw-decisions.md` 140 inbound, `knowledge/index.md` 131, `knowledge/memory-systems.md` 131, `knowledge/kutha-temporal-graphs.md` 36 inbound / 27 outbound. `tb_references(repo="vault", path="knowledge/kutha-temporal-graphs.md")` returns the 12 themed notes directly, which is a working route around every lexical failure in section 3.
- **Provenance is on almost every note**, so a claim can be checked instead of trusted: `source_url`, `provenance`, `family`, `confidence` are filled on the kutha notes (`arXiv 2607.10265`, `arXiv 2605.21997`, TGMS deep-read). That is why this audit can call a theme "covered" without guessing whether the note is a summary of a summary.
- **Legal is a genuine sub-field, not a tag.** `knowledge/legal-text-analytics.md` is a hub over LLI's 21 tasks and 130+ resources; below it `legal-text-analytics-lli-moc`, `legal-datasets-jurisdiction-map`, `legal-norm-ontology-classification`, `implicit-legal-reference-extraction`, `legal-qa-complexity-boundary`, `document-comparison-vs-semantic-matching`. For a legal-temporal wedge this is the strongest asset in the base.
- **The pattern library answers the methodology half of the theme list** even where the ADR names are absent: `f/patterns/pattern-verification-receipt-commits.md` (receipt per change), `pattern-evidence-gates-process-vs-completion.md` (a gate before continuing), `pattern-oracle-drift-ci.md` (differential oracle), `pattern-budgets-as-trades.md`, `pattern-impact-gated-skill-consolidation.md`, `pattern-cache-aligned-context-packs.md`, `pattern-static-embedding-hybrid-search.md`. These carry `applies_to:` lists, so they are meant to be reused, not just read.
- **When the query uses the field's own phrase, the ranker is decisive.** `"worst-case optimal join"` 145.78 on `knowledge/wcoj-leapfrog-triejoin-pattern.md`; `"valid time transaction time half-open intervals"` 169.12; `"RVF container"` 62.88 and 61.13 on the two right notes; `"hybrid retrieval composition…"` 59.48. Latency is 0–6 ms per query. The ranker is not the problem in most failures — the words are.
- **The vault knows what it does not know.** `temporal-kg-white-space` lists nine unmet capabilities; `graphiti-bitemporal-fact-edges-contrast` states the contrast rather than the similarity; `premature-graphrag-builds-graph-before-validating-need` is an argument *against* building. That is the opposite of collector's-fallacy material and it is rare.

One caveat about the corpus, not the search: 559 of 1,065 markdown files sit in `inbox/`, i.e. more than half the base is unprocessed capture. The three duplicate notes in D4 were all created in `knowledge/` within six days of each other, which is where the duplicate pass is supposed to catch them. The base is good at depth per subject and weak at consolidation across subjects.

## 5. Appendix — query ledger

Every `tb_search` below was run against `/root/vault`; `total` is the engine's hit count, `shown` how many the page returned. Scores in the report are from these pages. The `scope=own` variant is listed separately because it is the D9 evidence; the D3 `min_should_match=1` run is quoted in section 3 and in the snippet below.

| # | query | args | total | shown |
|---|---|---|---|---|
| 1 | `event log as source of truth` | — | 330 | 20 |
| 2 | `append-only event log fold projection` | — | 105 | 20 |
| 3 | `bitemporal facts invalidation` | — | 6 | 6 |
| 4 | `valid time transaction time half-open intervals` | — | 66 | 20 |
| 5 | `snapshots and tiered time-travel` | — | 111 | 20 |
| 6 | `snapshot tiers vacuum` | — | 0 | 0 |
| 7 | `cascade budgets and receipts` | — | 54 | 20 |
| 8 | `cascade budget receipt` | — | 4 | 4 |
| 9 | `hybrid retrieval composition lexical plus graph expansion` | — | 156 | 20 |
| 10 | `temporal queries as of date` | — | 267 | 20 |
| 11 | `as of query cypher temporal` | — | 300 | 20 |
| 12 | `fork and diff knowledge base` | — | 217 | 20 |
| 13 | `fork at event shared prefix structural diff` | — | 89 | 20 |
| 14 | `strict replay` | — | 3 | 3 |
| 15 | `deterministic replay from event log` | — | 109 | 20 |
| 16 | `gated regime loop` | — | 3 | 3 |
| 17 | `gate before loop escalation` | — | 78 | 20 |
| 18 | `meta-prompt dictionaries` | — | 12 | 12 |
| 19 | `enrichment packs` | — | 0 | 0 |
| 20 | `genai enrichment pack optional` | — | 4 | 4 |
| 21 | `portable capsules` | — | 0 | 0 |
| 22 | `legal reference pack` | — | 18 | 18 |
| 23 | `scientific archive pack evidence graph` | — | 125 | 20 |
| 24 | `oracle matrix tests` | — | 5 | 5 |
| 25 | `ABAC attribute based access control slices` | — | 34 | 20 |
| 26 | `enrichment pack` | — | 0 | 0 |
| 27 | `portable capsule` | — | 0 | 0 |
| 28 | `capsules` | — | 0 | 0 |
| 29 | `pack` | — | 20 | 20 |
| 30 | `capsule` | — | 2 | 2 |
| 31 | `evolver` | — | 5 | 5 |
| 32 | `statcraft` | — | 2 | 2 |
| 33 | `enrichments` | — | 5 | 5 |
| 34 | `packs` | — | 8 | 8 |
| 35 | `oracle tests reference implementation differential` | — | 31 | 20 |
| 36 | `snapshot isolation MVCC` | — | 5 | 5 |
| 37 | `snapshot and vacuum storage` | — | 76 | 20 |
| 38 | `versioned snapshot tiered storage` | — | 14 | 14 |
| 39 | `fork branch copy-on-write knowledge base` | — | 221 | 20 |
| 40 | `structural diff between two versions` | — | 44 | 20 |
| 41 | `diff two snapshots` | — | 4 | 4 |
| 42 | `claude flow architecture` | — | 116 | 20 |
| 43 | `ruvector hybrid search` | — | 72 | 20 |
| 44 | `битемпоральная модель ребра отслеживает валидность и время загрузки отдельно` | — | 346 | 20 |
| 45 | `attribute-based access control` | — | 34 | 20 |
| 46 | `worst-case optimal join` | — | 9 | 9 |
| 47 | `wcoj` | — | 8 | 8 |
| 48 | `as of` | exact=True | 219 | 20 |
| 49 | `meta-prompt` | exact=True | 12 | 12 |
| 50 | `оценка качества поиска` | — | 18 | 18 |
| 51 | `oracle tests reference implementation differential` | scope=own | 30 | 20 |
| 52 | `RVF container` | — | 15 | 15 |
| 53 | `portable` | — | 8 | 8 |
| 54 | `plugin lifecycle` | — | 4 | 4 |
| 55 | `pack lifecycle` | — | 3 | 3 |
| 56 | `enrichment` | — | 25 | 20 |
| 57 | `optional pack` | — | 1 | 1 |
| 58 | `tenant isolation multi-tenant` | — | 28 | 20 |
| 59 | `row level security policy filter` | — | 57 | 20 |
| 60 | `loop termination gate budget` | — | 56 | 20 |
| 61 | `mnemosyne hybrid search` | — | 54 | 20 |
| 62 | `graphiti temporal facts` | — | 27 | 20 |

| # | grep literal | lines |
|---|---|---|
| 1 | `vacuum` | 1 |
| 2 | `meta-prompt` | 2 |
| 3 | `dictionar` | 5 |
| 4 | `regime` | 9 |
| 5 | `gated loop` | 0 |
| 6 | `capsule` | 4 |
| 7 | `oracle matrix` | 0 |
| 8 | `enrichment` | 164 |
| 9 | `receipt` | 49 |
| 10 | `oracle` | 83 |
| 11 | `ABAC` | 2 |
| 12 | `capability security` | 0 |
| 13 | `materialization` | 20 |
| 14 | `time travel` | 2 |
| 15 | `капсул` | 9 |
| 16 | `реплик` | 33 |
| 17 | `MVCC` | 17 |
| 18 | `quantum receipt` | 0 |
| 19 | `cascade` | 146 |
| 20 | `gate` | 699 |
| 21 | `gated` | 77 |
| 22 | `reference pack` | 0 |
| 23 | `scientific` | 10 |

## 6. Reproduce

```python
import sys; sys.path.insert(0, '/root/2brain/tb-mcp-rs/scripts/lib')
from mcp_stdio import StdioMcp, tool_text
import json
m = StdioMcp(['/usr/local/bin/2brain','mcp','/root/vault'], env={'HOME':'/root'}).start()
def search(q, **kw):
    r = json.loads(tool_text(m.call('tb_search', {'query': q, **kw})))
    print(r['search']['total'], [(x['path'], round(x['score'],2)) for x in r['search']['matches'][:3]])
def grep(q):
    r = json.loads(tool_text(m.call('tb_grep', {'query': q})))
    print(r['grep']['total'], [x['path'] for x in r['grep']['matches'][:3]])
```

The D3 and D2 recoveries are one parameter away: `search('oracle matrix tests', min_should_match=1)` and `search('meta-prompt', exact=True)`; the D7 comparison needs a 100-hit page to see rank 8.
