# Graph, statistical and mathematical algorithms for 2brain

Scale measured 2026-11-16 on the live vault: **1 036 markdown notes**, 1 185 graph *nodes*, ~4 606 edges,
5 046 wikilinks, 0 diagnostics. The engine already ships BM25 (`tb-engine/src/search.rs`) over the
catalogue and a typed graph (`au_references`, `au_neighborhood`, `au_follow`, `au_hubs`,
`au_graph_health`). Everything below is judged by: does it answer a question someone in this system
actually asks, and can it run without a model.

## Ranking

| algorithm | problem | input | cost at our scale | where it runs |
|---|---|---|---|---|
| BM25 field weights (**shipped**) | relevance | catalogue text | 0.16–1.03 ms per query, 4.7 KiB/doc | in-engine, incremental |
| personalized PageRank / random walk with restart | "related notes", not just matching ones | link graph + a seed set | O(E) per iteration; 4.6k edges × 20 iterations ≈ 92k ops ≈ sub-millisecond | on request, or cached per rebuild |
| graph diffusion (heat kernel) | soft relatedness that decays with distance | adjacency + seed | same order; needs a truncated series | cached |
| HITS (hubs/authorities) | distinguishes "index notes" from "sources" | link graph | same as PageRank | per rebuild |

**Type-aware freshness** fits as a **multiplicative prior**, not a decay of the index: `score × w(status)`
with `current 1.0, aging 0.7, stale 0.3, superseded 0.05`, and a family multiplier where `pattern` /
`decision` / `extract` are durable (no time decay) and inbox/session notes are transient (half-life ~120
days). This is a one-line change to the scoring loop and is testable against a labelled query set; it is
**not** a re-index.

## Clustering and community

| algorithm | use | cost | verdict |
|---|---|---|---|
| label propagation | topic clusters, orphan detection | O(E) per pass, ~5 passes | worth it as a **background** job: the output is a "these 12 notes are one topic" list a human can reject |
| Louvain / Leiden | tighter communities, modularity score | O(E log N) | overkill at 4.6k edges; label propagation gets the same answer here |
| connected components | "is this note reachable from the MOC at all" | O(E) | cheap and immediately actionable as a diagnostic |
| betweenness centrality | "bridge notes" whose loss disconnects a topic | O(VE) ≈ 5.5M — fine | good *monthly* report, bad per-request |

The actionability test matters more than the algorithm: a cluster with no consumer is a number nobody
reads. The consumer here is the compaction/consolidation pass (see the FSM document), which proposes
merges inside a cluster and asks the judge to confirm each pair.

## Deduplication

Ordered by cost, each stage only seeing what the previous one kept:

1. **Exact**: normalized-body hash — O(N).
2. **Shingle Jaccard / MinHash (LSH)**: bands at ~0.5 similarity. At 1 036 notes all-pairs Jaccard is
   536k comparisons of small sets — milliseconds in Rust, seconds in Python. This is the prefilter that
   should keep the judge's pair set small: measured judge precision/recall 1.000/0.889 means it is right
   when asked, so the only cost to cut is *how often it is asked*.
3. **SimHash**: cheaper, worse on short notes; not needed here.
4. **Embeddings**: the only stage that sees paraphrase across different wording — and the one we would
   have to host. Not before stages 1–3 show a residual miss rate.

## Temporal and topological analysis

- **Link decay / hub rot**: for each hub (`au_hubs`), the age of its outbound targets. A hub whose targets
  are mostly `stale` is itself suspect. Pure arithmetic over `au_hubs` + frontmatter — a cheap background
  diagnostic.
- **Cycles**: already reported (`graph_health.cycles`, 17→21 observed during the corpus drift). Their
  *value* is as a consolidation signal: a cycle inside one type family is a merge candidate.
- **Orphan detection**: notes with no inbound and no outbound edges, excluded by type (`manual` and `moc`
  may legitimately stand alone). O(N).
- **Temporal drift**: git history gives created/modified; a note whose body changed but whose description
  did not is a staleness candidate — deterministic and exact, and it is the *right* prefilter for the
  judge's staleness question.

## Contradiction and consistency

Checkable without a model: frontmatter contract violations (the engine's diagnostics), mutually exclusive
field pairs (a `decision` with both `adopted` and `rejected`), duplicate ids, links to retyped notes,
claims whose `target` no longer resolves. All of these are **engine diagnostics** and already exist or
belong there.

Needs a model: two notes asserting incompatible things in prose. The graph narrows the pairs (same topic,
same type, overlapping entities), the judge decides, and the output is a **proposal**, never an edit.

## Background vs on-request

| job | trigger | budget | invalidation |
|---|---|---|---|
| BM25 index | every rebuild | incremental, ms | file hash |
| PageRank / diffusion seed caches | every rebuild | ms | link graph version |
| components, orphans, cycles | every rebuild | ms | link graph version |
| label propagation clusters | nightly | seconds | graph version + 24 h |
| shingle/MinHash prefilter | nightly | seconds | body hash |
| judged duplicate pairs (≤100 per call) | nightly, after the prefilter | ~10 s + judge | pair set hash |
| staleness candidates (git-based) | nightly | seconds | note hash |
| betweenness report | weekly | seconds | graph version |
| judged contradiction pairs | weekly | ~10 s | pair set hash |

## What not to build

- **A second store.** Any index that is not derived-and-discardable becomes a second source of truth; the
  vault's git history is the source, and the engine's IR is a cache.
- **Embeddings before prefiltering.** The deterministic stages are cheap and exact; a vector store adds a
  dependency whose benefit is unmeasured here.
- **Per-request heavy graph queries.** Betweenness on a user's search path would make latency a function of
  corpus size for no gain.
- **Clusters without a consumer.** If no pass consumes the clusters to propose a merge, the clustering is
  decoration; the FSM's CONSOLIDATE state is the consumer that justifies it.

## Estimates flagged as estimates

Per-iteration PageRank on 4.6k edges is sub-millisecond *in Rust*; the same in Python is ~10 ms. The
nightly jobs above are bounded by I/O (reading 1 036 files, ~5.4 MiB) rather than by the algorithms.
