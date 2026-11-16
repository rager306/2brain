# arscontexta methodology corpus → what 2brain dropped (2026-11-16)

Source: `/root/vendor-source/arscontexta/methodology/` — 249 notes, one flat directory, ~3.1 MB, READ-ONLY.
Method: all 249 titles read; ~45 notes read in depth, selected for bearing on a Rust engine with an MCP
surface (retrieval, description quality, write gates/automation safety, graph traversal, schema/types,
measurement). Nothing was run against `/root/vault` (1 032 notes) — the engine is busy; all measurements
below are specified, not executed. Corpus claims are cited by path; our choices are marked **design**.

The corpus is the claim-based research vault behind the arscontexta vault generator. Frontmatter shows it
is itself partly synthesized from outside sources (Cornell, Systems Theory, Evergreen traditions, "Rata"
papers, Heinrich's vibe-notetaking), with a large "Original" stratum from its own rethink sessions.

---

## 1. Cluster map of the 249 titles

The corpus clusters itself via a `topics:` frontmatter field pointing at 14 topic hubs plus `index.md`.
Primary-topic partition (first listed topic), hubs counted separately:

| Cluster (hub file) | Count | One-line summary | Engine-relevant examples |
|---|---|---|---|
| agent-cognition (`agent-cognition.md`) | 57 | LLM/agent cognition: hooks-as-habits, sessions, memory/identity, traversal | hooks×14, memory-identity×13, sessions/context×8, traversal×7, platform×5, epistemics/verification×4, trust×2, federated/coherence×3, MOC-attention×1 |
| design-dimensions (`design-dimensions.md`) | 34 | configuration space: 8 dimensions, 4 abstraction layers, derivation vs templating, platform tiers | eight configuration dimensions; no wrong patches |
| processing-workflows (`processing-workflows.md`) | 32 | capture → processing pipelines; generation effect; WIP limits; four-phase skeleton | generation effect gate; temporal separation |
| graph-structure (`graph-structure.md`) | 29 | wiki links, topology, MOCs, centrality, heterarchy vs hierarchy | wiki links implement GraphRAG; small-world topology |
| maintenance-patterns (`maintenance-patterns.md`) | 27 | gardening, loops at timescales, fix-vs-report, reconciliation, automation retirement | automated detection vs remediation; fix-versus-report |
| note-design (`note-design.md`) | 20 | atomicity, claim titles, descriptions, schema fields, decontextualization | descriptions are retrieval filters; title as claim |
| discovery-retrieval (`discovery-retrieval.md`) | 17 | BM25 behavior, description quality, verification loops, scaling regimes, staleness | BM25 fails on full descriptions; retrieval verification loop |
| domain-compositions (`domain-compositions.md`) | 12 | 12 reference domains (academic, trading, therapy, legal, student, …) used as derivation targets | novel domains derive by mapping |
| method singletons (six how-to notes) | 6 | onboarding balance; automatic memory; multi-domain compose; schema zones; vulnerability matrix; vocabulary six levels | enforce schema with graduated strictness |
| hubs / index (no `topics` field) | 15 | `index.md` + 14 topic MOC files | — |
| **Total** | **249** | | |

Sub-clusters inside agent-cognition (57): hooks-automation 14, memory-identity 13, sessions/context 8,
traversal 7, platform 5, epistemics 4, federated/coherence/data-exit 3, trust 2, MOC-attention 1.

---

## 2. Claims testable on our corpus (`/root/vault`, 1 032 notes) — measurements specified, not run

| # | Claim (source) | Prediction on /root/vault | Exact measurement to run when engine is idle |
|---|---|---|---|
| T1 | Full-prose queries dilute BM25; 5/5-prediction descriptions can return zero hits (`methodology/BM25 retrieval fails on full-length descriptions….md`) | A material fraction (corpus implies >20%) of the 1 032 descriptions fail as BM25 queries (self not in top 10); condensed rare-term queries recover most | For every note: query 2brain BM25 with the note's full `description:` text; record self-rank (hit@5, hit@10, miss). Repeat with the query condensed to its 3–5 lowest-document-frequency terms. Report the two miss rates and the recovery rate. Read-only, ~2 064 queries |
| T2 | Description quality is two channels: human-scanning vs keyword (`methodology/description quality for humans diverges….md`) | The notes that pass a scanning test are over-represented among T1 misses | Join T1 results with description length/connective density (count of "because/which/so that/not/just"-class tokens per description). Expect positive correlation between connective density and BM25 miss rate |
| T3 | Descriptions that are too similar cause retrieval confusion; threshold 0.8 flag / 0.6 review (`methodology/distinctiveness scoring treats description quality as measurable.md`) | Some note pairs are near-interchangeable under BM25; eval confusion axis should show wrong-note-rank-1 incidents | Pairwise similarity over all 1 032 descriptions (BM25 or embedding cosine); flag pairs >0.8; for each flagged pair run both descriptions as queries and count wrong-note-rank-1 outcomes. Feeds the eval's confusion axis |
| T4 | Wiki-link graphs must be small-world: hubs + dense locals, short paths (`methodology/small-world topology requires hubs and dense local links.md`) | The 1 032-note link graph has heavy-tailed degree, high clustering, mean path length ≪ linear | Parse all `[[…]]` edges into a graph; compute degree distribution (Gini/power-law fit), clustering coefficient, average shortest path on the largest component; compare with an Erdős–Rényi baseline of same n,m |
| T5 | Weak flag ≈ corpus "maturity field": immature notes deserve different retrieval treatment (`methodology/maturity field enables agent context prioritization.md`) | weak-flagged notes have lower in-degree, shorter bodies, younger git mtime than the rest | Split notes by `weak` flag; Mann-Whitney on in-degree, body length, recency. If no separation, the weak flag is not proxying maturity (**design** question, corpus-derived hypothesis) |
| T6 | Dangling links are demand signals, not errors (`methodology/dangling links reveal which notes want to exist.md`) | Dangling target frequencies are Zipfian; top-20 targets name real missing concepts | Extract all `[[targets]]` that resolve to nothing; count frequencies; manually inspect top 20 for "deserves a note" precision |
| T7 | Stale links silently break navigation (`methodology/tag rot applies to wiki links because titles serve as both identifier and display text.md`) | Some link targets match no current title after renames | Set difference: all link targets vs all note titles (exact + normalized). Report broken-link rate per family |
| T8 | Past ~500 notes, search-only navigation degrades; local structure (links/expansion) becomes load-bearing (`methodology/navigation infrastructure passes through distinct scaling regimes….md`) | At 1 032 notes (Regime 2), BM25@5 on topical questions underperforms BM25 + 1-hop link expansion | 20 topical questions; run BM25@5 vs BM25-seeded 1-hop expansion ranking; compare nDCG@5 on the existing judgement set. Decides whether transfer #8 is worth building |
| T9 | Every new note compounds value via traversal paths (`methodology/each new note compounds value by creating traversal paths.md`) | Reachability (fraction of graph within k hops) grows superlinearly in note count | Snapshot the graph at n = 250/500/750/1032 by git history; compute pairwise reachability at k=2,3. Confirms/quietly kills the "graph compounds" intuition on OUR corpus |
| T10 | Idiosyncratic vocabulary is distinctive for humans but sparse for BM25 (`methodology/narrow folksonomy optimizes for single-operator retrieval….md`) | Notes whose descriptions lean on rare domain terms are easier to condense-retrieve (interacts with T1) | From T1 outputs, bucket descriptions by mean term document frequency; report miss rate per bucket |

All ten are read-only. T1–T3 reuse 2brain's own BM25 index; T4–T7, T9 need only the markdown tree; T8, T10 reuse T1's outputs.

---

## 3. Claims that contradict our design

**design** marks our current choice. Each entry: the corpus claim, its evidence, and whether it argues we are wrong.

### 3.1 vs BM25 over body + description (**design**)
- `methodology/BM25 retrieval fails on full-length descriptions because query term dilution reduces match scores.md` — mechanism: IDF dilution by connective prose; observed case: a well-written description returned **zero** BM25 results for its own note, condensed query ranked #1.
- `methodology/description quality for humans diverges from description quality for keyword search.md` — descriptions optimized for agent scanning (the heuristic→mechanism→implication formula, `methodology/good descriptions layer heuristic then mechanism then implication.md`) are structurally BM25-hostile. Two channels, opposite optimization targets.
- The source system demoted BM25 entirely: its fallback chain is **query (LLM rerank) → vsearch (embeddings) → search (BM25) → grep**, and it notes BM25 became primary only during qmd outages (in the description-divergence note). They treat BM25 as the degraded fallback, not the primary channel.
- Mitigating: 2brain's rare-term floor escape mirrors the corpus's prescribed diagnostic ("condense to 3–5 key terms before declaring failure"). The corpus contradiction is not fatal — but it says the escape must be **automatic and measured** (T1–T3), and that our eval set is blind to this failure mode if its 160 questions are all short queries. **Verdict: the strongest contradiction in the corpus. Keep BM25, but the eval must include a long-query axis and the condensation escape must be engine-side, not operator-side.**

### 3.2 vs one fail-closed write gate (**design**)
- `methodology/schema enforcement via validation agents enables soft consistency.md` — hard enforcement is comply-or-fail; soft enforcement (warn, don't block) preserves creation flow and still trends toward compliance. Argues our gate rejects too much at write time.
- `methodology/schema evolution follows observe-then-formalize not design-then-enforce.md` — upfront schema design fails; five drift signals (placeholder stuffing, dead enums, …) should drive formalization. Argues fixed schemas enforced fail-closed calcify wrong shapes.
- Counter-evidence inside the same corpus: `methodology/enforce schema with graduated strictness across capture processing and query zones.md` prescribes **soft at capture, hard at pipeline boundaries** — which is close to a fail-closed gate at the write boundary; and `methodology/over-automation corrupts quality when hooks encode judgment rather than verification.md` argues the danger is judgment-encoding automation, not structural verification — our gate checks structure/types, which is exactly the safe class. **Verdict: partial contradiction. The defensible position is zoned strictness: fail-closed on deterministic structural checks (what we do), warn-and-accept on anything requiring reading content. Also adopt their gate for our future auto-fix features: four conditions (determinism, reversibility, low cost-if-wrong, accumulated trust) from `methodology/the fix-versus-report decision….md` before any tool mutates content.**

### 3.3 vs no LLM on the serving path (**design**)
- `methodology/confidence thresholds gate automated action between the mechanical and judgment zones.md` — the source system treats LLM-reranked retrieval as its highest-confidence mode; keyword search as lowest. Serving-path retrieval without any LLM caps confidence at the lexical tier.
- `methodology/wiki links implement GraphRAG without the infrastructure.md` + `methodology/MOC construction forces synthesis….md` — the corpus's answer to semantic retrieval is curated structure (links, MOCs, claim titles) doing work that embeddings would otherwise do. If we drop both embeddings and curation, we must show BM25+structure reaches comparable nDCG (that is exactly T8).
- Counter-evidence: the same notes document qmd's unreliability (stale indices, segfaults) and conclude the LLM/embedding path is the fragile one; deterministic BM25 is what survives outages. **Verdict: mixed. The corpus argues the no-LLM path is right for reliability and wrong for ceiling. It does not force an LLM onto the serving path; it forces offline curation/structure (transfers 8, 12) or measured evidence that lexical+graph suffices.**

### 3.4 vs typed notes (**design**)
- `methodology/schema evolution follows observe-then-formalize not design-then-enforce.md` and `methodology/schema fields should use domain-native vocabulary not abstract terminology.md` argue against rich upfront type systems and for field vocabularies that match the writer's language.
- Counter-evidence: `methodology/type field enables structured queries without folder hierarchies.md` — a `type` axis is exactly what makes structured queries possible without folders; `methodology/metadata reduces entropy enabling precision over recall.md` is the information-theoretic case for typed frontmatter. **Verdict: not a contradiction of minimal typing (our `kind` field); a contradiction of type proliferation. The corpus's five drift signals should be monitored; if placeholder stuffing or dead enums appear on /root/vault, prune types rather than add.**

### 3.5 vs families in a flat tree (**design**)
- `methodology/associative ontologies beat hierarchical taxonomies because heterarchy adapts while hierarchy brittles.md` — the closest direct attack: hierarchy forces the placement decision at capture time (maximum ignorance), loses all but one classification facet (`methodology/faceted classification treats notes as multi-dimensional objects rather than folder contents.md`), and brittles under reorganization. The corpus's own practice is flat files + links + local (non-global) MOC hierarchies.
- `methodology/federated wiki pattern enables multi-agent divergence as feature not bug.md` — single canonical families discard interpretive diversity; productive vs noise divergence is tested empirically via backlink neighborhoods, not resolved administratively.
- Counter-evidence inside the same corpus: `methodology/navigational vertigo emerges in pure association systems without local hierarchy.md` says pure association leaves unlinked neighbors unreachable and demands **local** hierarchy — a family is exactly a local hierarchy. `methodology/stale navigation actively misleads because agents trust curated maps completely.md` warns any curated grouping (families included) rots silently and misleads agents, who never suspect staleness. **Verdict: the corpus argues against trees as global taxonomies, not against local grouping; families survive if they are few, derived from link structure, and allowed to split/merge on evidence (transfer 9), not if they are a fixed filing system. The federation claim is the one we should consciously reject: 2brain wants canonical state; divergence belongs in git branches, not in the serving tree (design).**

---

## 4. Twelve ranked transfers (value per unit of risk, best first)

| # | Transfer | Claim source | 2brain subsystem | Cost | Risk | Experiment (gates adoption) |
|---|---|---|---|---|---|---|
| 1 | **Engine-side query condensation**: when a query exceeds ~8 terms or returns zero/weak hits, auto-retry with top-3-5 rarest terms (IDF from our own index) before declaring miss; expose both result sets | BM25 dilution note (§3.1) | search | S | low | T1: measure dilution rate first; then A/B condensation on the same queries, expect recovery of most misses |
| 2 | **Long-query axis in the eval set**: add n≥20 questions phrased as full descriptions/prose to the 160-question set; nDCG@5 reported per length class (MDE recalculated per axis) | description-divergence note (§3.1) | evaluation | S | low | the axis itself; run at next eval window |
| 3 | **Distinctiveness scoring as offline tool + confusion axis**: pairwise description similarity, 3-tier thresholds (auto-flag >0.8, review 0.6–0.8, ignore below); surface in MCP as read-only `descriptions/conflicts`-style tool or eval report | distinctiveness note (T3) | eval / tools | M | low | T3 measurement; adopt if flagged pairs produce wrong-note-rank-1 incidents |
| 4 | **Read-only graph analytics tools**: dangling links, orphans, bridge notes (betweenness), small-world health — aggressive read-only detection is always safe per the corpus's read/write asymmetry | automated-detection note; dangling links note; betweenness note | MCP tools (read-only) | M | low | T4, T6, T7 run on /root/vault; keep all report-only, write path untouched |
| 5 | **Freshness stamping**: index stores per-note content hash + git commit; every MCP result carries `as_of` so agents can distrust stale answers (stale-navigation claim: agents never suspect staleness) | stale-navigation note; live-index note | engine/index | S | low | ingest a write, confirm stale results are flagged within one commit |
| 6 | **Zoned write strictness**: gate stays fail-closed on deterministic structural checks; add warn-and-accept tier for checks that require reading content; log warnings to the same audit surface | graduated-strictness note; soft-consistency note (§3.2) | write gate | M | medium | dry-run classifier over recent gate rejections: what fraction were judgment-flavored vs structural |
| 7 | **Weak flag → 3-level maturity semantics** (seedling/developing/evergreen or weak/ok/mature): retrieval may prefer mature notes when budgets are tight; weak remains the floor | maturity-field note (T5) | schema/ranking | M | medium | T5 correlation first; adopt ranking use only if weak separates degree/recency |
| 8 | **1-hop link expansion ranking (spreading activation, lite)**: BM25 seeds, expand via wiki-links with decay; a second ranking mode behind the same tool, not a new surface | spreading-activation note; GraphRAG note (§3.3) | ranking | L | medium | T8 decides; build only if lexical-only nDCG@5 underperforms expansion at 1 032 notes |
| 9 | **Family split/merge on evidence**: periodic report-only pass comparing family membership to detected link communities; humans/agents decide splits from the report | community-detection/MOC note; reconciliation note | maintenance (offline) | M | low | run once on /root/vault; adopt if reported splits match family pain (oversized families) |
| 10 | **Facet histogram in search responses**: return topic/kind/family counts for the candidate set so a session can pivot (berrypicking checkpoints) without re-scanning | queries-evolve note; metadata-entropy note | MCP search response | M | low | measure pivot value in eval transcripts; adopt if reassessment changes the query in ≥ a third of hard questions |
| 11 | **Typed-edge vocabulary, gated**: pre-measure the prose around wiki-links for connective tokens (since/because/contradicts); only if the signal is dense, add optional link relation field — corpus's observe-then-formalize applied to ourselves | propositional-link-semantics note; schema-evolution note | schema | L | high | the pre-measurement (count connective-marked links); otherwise do not build |
| 12 | **Automation retirement policy**: any future auto-fix feature must publish FP/TP counts; retire when FP>TP or zero catches — a standing policy, not code | automation-retirement note; fix-versus-report note (§3.2) | policy / gate | S | low | audit sheet per auto-fix; first application: gate warning tier from #6 |

Deliberately **not** transferred, with reasons: voice capture + inbox pipelines (serving-path machinery, out of engine scope); per-domain vault templates (derivation output, not engine); MOC curation as a workflow (needs an LLM writer; replaced by #8/#9 evidence-driven structure); hooks-as-shell-scripts (subsumed by the engine's gate — but the determinism boundary itself is adopted inside #6/#12); federated divergence (rejected in §3.5, design).

---

## Appendix: three propositions to test first, and the most threatening claim

1. **T1 — BM25 description-dilution rate on our 1 032 descriptions** (cheapest, attacks the core primitive, uses only our own index).
2. **T8 — lexical-only vs 1-hop expansion at Regime-2 scale** (decides the corpus's biggest structural bet: that past 500 notes, links carry the navigation that search cannot).
3. **T5 — weak flag vs degree/recency separation** (validates or kills our own maturity heuristic before ranking code leans on it).

**Most threatening claim:** `methodology/BM25 retrieval fails on full-length descriptions because query term dilution reduces match scores.md`, backed by `description quality for humans diverges from description quality for keyword search.md` — the source system measured its own well-formed descriptions returning zero BM25 hits, identified the IDF mechanism, and demoted BM25 to last-resort fallback. 2brain put that exact primitive at the center of its serving path. The claim is testable in an afternoon (T1) and, if confirmed at rate, forces either the condensation escape (#1), the long-query eval axis (#2), or both.
