# Graph-engine assessment: is migrating to Samyama Graph (or another) justified?

Status: assessment, 2026-11-15. No migration was performed and no code changed. Numbers that
decide the question were measured on the live engine, on `/root/samyama-graph` (v1.10.0) and in
a survey of the candidate field; every claim has a command, a file:line, a note, or a source.

## 1. The framing that decides most of it

The arsumbris engine is **not a database**. No SQLite, no embedded store, no `petgraph`. Its
dependencies are visible in `au-engine/Cargo.toml`: `serde`, `serde_json`, `regex`, `memchr`,
`saphyr` (YAML), `notify` (file watching), `fs4` (file locks), `rpds` (persistent immutable
maps), `tokio`, `tracing`. The graph lives in memory as persistent maps
(`rpds::RedBlackTreeMapSync` in `crates/au-engine/src/ir.rs:42` and
`crates/au-references/src/lib.rs:35`), derived from markdown on load and kept current by a
watcher plus an incremental rebuild.

The store is the filesystem plus git. That is not an accident of immaturity; it is what makes
"one commit = one logical mutation" true, and it is why `au_reconcile` can read a git HEAD.

So the question is not "swap SQLite for a graph DB". It is: **should the graph stop being a
derived in-memory index and become a database in its own right?** Everything below turns on
that, and the answer is no — with measured reasons.

## 2. Measured state (2026-11-15)

### 2.1 Scale of the live vault

| quantity | value | how |
|---|---:|---|
| markdown files | 1 036 | `find /root/vault -name '*.md' -not -path '*/.git/*'` |
| markdown text | 4.25 MB | sum of file sizes (the 49 MB working tree is mostly `.git`) |
| wikilinks | 5 046 | `grep -ro '\[\[[^]]*\]\]' --include=*.md` |
| out-degree | median 3, p95 12, max 78 | `au_overview.graph_shape.degree` |
| strongest hub (in-degree) | 140 | same |
| components | 213 | same |
| diagnostics | 0 | same |

### 2.2 Latency of the read verbs a graph DB would touch

Release binary, live socket, 5 calls each, median:

| verb | median | payload | what it does |
|---|---:|---:|---|
| `au_hubs` | 9.8 ms | 3.3 KiB | in-degree ranking over the catalogue |
| `au_graph_health` | 20.5 ms | 2.7 KiB | builds adjacency for the scope, iterative Tarjan SCC (`serve.rs:14369`), union-find, centrality (`serve.rs:14202`) |
| `au_diagnostics` | 4.1 ms | 0.0 KiB | cached diagnostic set |
| `au_overview` | 16.7 ms | 8.1 KiB | counts, degree histograms, components |
| `au_grep` | **132.8 ms** | 11.1 KiB | literal full scan: reads every file, lowercases every line (`serve.rs:13522`) |

The pattern is unambiguous: **whole-graph analytics already cost 20 ms at 1k nodes, and the one
slow verb is a text scan.** A graph database improves neither at this size, and does nothing at
all for the text scan.

## 3. The candidates, measured (not marketed)

| candidate | embedded | store | query | verdict |
|---|---|---|---|---|
| **samyama-graph** 1.10.0 | yes (`GraphStore` + `QueryEngine`, sync API) | RocksDB, HNSW | OpenCypher | real engine, but **crates.io `samyama` = 1.14.0, 25 downloads, one published version**, repo 187★ with 233 open issues; its own SKILL.md lists stubs (SPARQL executor returns empty, `RdfsReasoner::materialize()` is a stub, default embedder is mock); the SCORECARD.json its README cites returns 404; the billion-edge claim is a self-published preprint |
| **Kuzu** | yes | columnar | Cypher | **dead: repository archived 2025-10-10** (Apple acquired the company) |
| **CozoDB** | yes | RocksDB/SQLite | Datalog | **abandoned** (last release 2023-12, dbdb.io marks it abandoned) |
| **Oxigraph** 0.5.13 | yes | RocksDB | SPARQL 1.1 | maintained, 107 revdeps — but RDF triples, not our typed markdown; docs.rs: "query evaluation has not been optimized yet" |
| **SurrealDB** 4.0.1 | nominally | SurrealKV/RocksDB | SurrealQL | BSL 1.1, pulls a full server stack |
| **Memgraph / Neo4j** | no | — | Cypher | separate process / JVM — not candidates for a single-binary daemon |
| **HelixDB** | no (LMDB locks) | LMDB | HelixQL | separate process |
| **RuVector** | claimed | own | own | no `ruvector` crate on crates.io, only satellites |
| **`petgraph` / `hnsw_rs` / `tantivy` / SQLite FTS5** | libraries | in-memory / one file | none | the component stack |

Measurement that decides the "when": on a reproduced benchmark at **100 000 nodes / 500 000
edges**, power-law, a SQLite recursive CTE from a degree-1 node costs 2.9 µs for one hop and
40.7 µs for two — indistinguishable from a graph engine (8.0 / 38.7 µs). From a hub with 11 460
incoming edges the same queries cost 49.9 ms and 305 ms, while the graph engine answers in 8.0
and 38.7 µs. At depth 10 the gap is 390×; at depth 50, 2 819×. **The threshold is hubs × depth,
not graph-ness** — and our hubs have 140 edges with traversal depth 1–3.

Samyama's own benchmarks are LDBC SF1 = 3.7 M nodes / 21 M edges — about 3 600× our node count;
its docs mark SF10 unverified and name deep traversal (IC1/IC3/IC5/IC6/IC9) as an open
optimization area.

### 3.5 Same-name traps, checked

Two different things answer to the name "rust-graph", and neither is a candidate:

| name | what it is | why not |
|---|---|---|
| `deargen/rust-graph` (GitHub, PyPI) | a **Python package** wrapping one Rust function, `all_pairs_dijkstra_path_length`, networkx-compatible; Rust core behind `maturin`; Apache-2.0; 8★; created 2024-06-28; one contributor; 0 open issues; reports 12–23× faster than networkx | it is not a Rust crate — a Rust daemon cannot depend on a wheel; and the one algorithm (all-pairs shortest paths) is not one we need over 1 036 unweighted nodes |
| `rust-graph` on crates.io | v0.0.3, **6 554 downloads, last published 2015-12-11**, keywords graph/algorithm/mst/dfs | dead for a decade |

If weighted shortest paths are ever needed, the honest options are `petgraph` 0.8.3 (616 M
downloads) or ~60 lines over a `BinaryHeap` inside the existing in-memory graph — which is the
same conclusion as everywhere else here: **add algorithms, not a store.**

## 4. Capability by capability: what would we actually gain?

| capability | do we need it now? | evidence | cheaper alternative |
|---|---|---|---|
| OpenCypher ad-hoc composition instead of 51 fixed verbs | **the only genuinely new product capability** | today composing two reads means two calls | a declarative traversal object (start set, edge filter, depth, projection) covers the common cases; a Cypher subset stays possible with no new store |
| CSR analytics (PageRank, WCC/SCC, BFS, CDLP) | no | SCC + union-find + centrality already ship in 20 ms; SCC also exists in `au-core` (`closure_id.rs`, `graph.rs`) | extend the existing pass; 200–400 lines or `petgraph` |
| HNSW vector search | maybe, for retrieval quality | vault pattern: filter with a structured query, then vector inside the subgraph | `hnsw_rs` (1.4 M downloads) or `usearch` as a **derived sidecar**, rebuilt from git |
| BM25 / full-text | **yes — the actual gap** | 133 ms byte scan; backlog P1 asks for bm25 with type-aware freshness | SQLite FTS5 `bm25()` or `tantivy` (22 M downloads) |
| MVCC snapshots | no | `Arc<Inner>` swap is already a snapshot; one writer | real MVCC pays only with concurrent writers |
| Raft / clustering | no | single host, single writer | — |
| RDF/SPARQL interop | no | our format is typed markdown; the type system is ours | lossy mapping, no consumer |
| bitemporal edges | not yet | 35 vault mentions, no engine requirement | two timestamps on the existing edge record |

### 4.5 What our own vault already decided

Read through `arsumbris-rs` while preparing this:

- `knowledge/zeroclaw-stack-hnsw-rs-plus-tantivy-plus-petgraph-plus-fastembed.md`
  (**type: decision**, `reversibility: High — all behind Cargo feature flags`) recommends
  exactly the component stack: `hnsw_rs` + `tantivy` + `petgraph` + `fastembed`, and lists what
  not to take — RuVector (168 downloads, single author), Raphtory (GPL), Raft (YAGNI for
  single-agent deployments). That is this document's recommendation, recorded earlier and with
  a rationale field.
- `inbox/graphicle-rust-code-graph-agent-memory.md` is the product-shaped precedent: a code
  graph + agent memory shipped as **one Rust binary over one SQLite file**, with memory anchored
  to symbols so it goes stale exactly when that code changes, no model calls inside tools, and
  LoCoMo-10 87.7 at ~12 model calls versus Mem0's 332. Its topic list already points at
  `[[arsumbris-fork-backlog]]`.
- `inbox/allspark-applicability-analysis-15-research-sources-full-disposition.md` carries the
  honesty sentence worth adopting verbatim: *"graph is a SQLite index, not a graph database"*.
- `inbox/lance-csr-cypher-over-lance-csr-index.md` and
  `inbox/ldbc-snb-interactive-benchmark-anatomy.md` supply the benchmark hygiene that made §3
  possible: golden answers per query (LDBC IC1-14), CSR hop = lookup instead of join.

## 5. What the migration would cost

1. **A second source of truth.** Files and git are authority. A graph DB makes the graph
   authoritative too, so every mutation becomes a two-phase write (git + RocksDB) with a crash
   window that does not exist today: now a mutation either commits or it does not, and
   `au_reconcile` reads the git HEAD. A DB also needs an idempotent replay keyed by
   `Mutation-Id` to close that window — new machinery to protect the guarantee we already have.
2. **Wire and contract churn.** 47 of 82 engine verbs re-expressed; `wire.rs` (5 758 lines),
   `WIRE.md`, 4 golden captures, 32 read fixtures, 16 mutation fixtures, 51 adapter tools, the
   seven policy stages and freshness/hash semantics all move.
3. **Operational surface.** RocksDB brings a WAL, compaction, its own durability policy
   (samyama: strict/background/raft-default), backup/restore and a second recovery path — a
   third durability model next to git and the filesystem, and unpredicted p99 inside a
   single-binary daemon.
4. **The typed contract is ours, not the store's.** `au-type-system` validates frontmatter and
   cross-note contracts; a property graph is schema-less, so the type layer would sit on top of
   a store that cannot enforce it. Spans, block ids and diagnostics would have to round-trip
   through a model that does not know about them.
5. **Dependency quality.** One published crate version with 25 downloads is the wrong class of
   dependency for a project whose vault format is a contract; Oxigraph, the mature option,
   migrates its on-disk format between minor versions.
6. **Nothing that is slow gets fixed.** The 133 ms grep is unaffected; the parse cost (~60% of a
   cold build, per `knowledge/au-engine-build-profiling-2026-10.md`) is unaffected.

## 6. Effect estimate

- **Latency:** no measurable gain. Current graph verbs are 10–20 ms at 1 036 nodes; a query
  planner, a wire hop and a RocksDB read cannot beat an in-memory adjacency walk at that size,
  and the measured gap only opens at hubs × depth we do not have.
- **Capability:** three real gains — ad-hoc queries, vector search, richer analytics — each
  obtainable with a library, not a store (§4).
- **Risk:** negative. The one guarantee the product sells (files are the truth, one commit is
  one mutation) is the one a graph store weakens.

**Verdict: do not migrate.** The engine is not storage-bound at 1 036 notes and 5 046 links; it
is text-search-bound. Spending weeks to move the graph into a database would leave the measured
bottleneck exactly where it is.

## 7. What to do instead, in order

1. **Text index — fixes the measured bottleneck.** SQLite FTS5 `bm25()` (one file, one writer,
   WAL) or `tantivy`; kept as a derived artefact, rebuilt from files. `au_grep` stays as the
   literal escape hatch with its current contract. Target: single-digit ms for ranked search,
   backlog P1 delivered.
2. **Declarative traversal object** (start set, edge filter, depth, projection, limit) behind
   one read verb — the 80% of Cypher agents actually ask for, with fail-closed typing intact.
3. **Vector sidecar only if retrieval quality is the goal**, rebuildable, never authority;
   filter first (type/topic), then vector inside the subgraph.
4. **Analytics stay in-memory**: extend `graph_health`'s pass (PageRank, WCC, CDLP) instead of
   adopting a store to get them.
5. **Adopt the honesty sentence** for documentation: *the graph is an index over files, not a
   database*.

## 8. Measured triggers that would reopen the question

| trigger | threshold | why it changes the answer |
|---|---|---|
| graph size | > 10⁵ nodes or > 10⁶ edges | rebuild and whole-graph passes stop being free |
| traversal shape | p95 > 50 ms at depth 2 from hubs with degree ≫ 100 | the measured hub × depth gap starts to matter (at 100k/500k edges: 305 ms for a 2-hop CTE versus 38.7 µs) |
| query composition | > 30% of read traffic is "call two verbs and join" | ad-hoc queries become a product need, not a convenience |
| writers | more than one concurrent writer, or a second host | MVCC/Raft stop being irrelevant |
| temporal queries | "what did this note say on date X" over the graph | needs bitemporal edges with an index we do not have |

## 9. If it is ever done, the only safe shape

Make the graph store a **derived index, not an authority**: written through from the file/git
mutation path, rebuildable from scratch, verified by rebuilding and comparing digests. Then a
store crash is a cache miss, not data loss — exactly the discipline that keeps the current
in-memory graph safe. Moving the *authority* into a graph DB is a different, much larger project
and should be rejected on its own merits.

## 10. Sources

Measured here: `au-engine/Cargo.toml`; `crates/au-engine/src/ir.rs:42`;
`crates/au-references/src/lib.rs:35`; `crates/au-engine/src/serve.rs:13522` (grep scan),
`:14202`, `:14369` (graph health, Tarjan); live scale and latencies (§2); `/root/samyama-graph`
(Cargo.toml, `docs/BENCHMARKS.md`, `docs/ADR`, test and LOC counts).

Candidate survey (crates.io, repos, dbdb.io, vendor changelogs), 2026-11-15: samyama
(github.com/samyama-ai/samyama-graph, crates.io/crates/samyama, arXiv 2603.08036), kuzu
(archived), cozo (abandoned), oxigraph 0.5.13, surrealdb 4.0.1 (BSL), memgraph (BSL),
helix-db, ruvector, `petgraph` / `hnsw_rs` / `usearch` / `tantivy` / `rusqlite`. The
SQLite-CTE-versus-graph-engine benchmark (100k/500k) and the "node/edge model is a performance
tax" preprint (arXiv 2609.01525) are the two numbers that set §8's thresholds.

Vault, read through `arsumbris-rs`: `knowledge/zeroclaw-stack-hnsw-rs-plus-tantivy-plus-petgraph-plus-fastembed.md`,
`knowledge/hybrid-search-combines-cypher-filtering-with-hnsw-in-subgraph.md`,
`knowledge/mnemosyne-hybrid-search-three-component.md`,
`inbox/graphicle-rust-code-graph-agent-memory.md`,
`inbox/lance-csr-cypher-over-lance-csr-index.md`,
`inbox/ldbc-snb-interactive-benchmark-anatomy.md`,
`inbox/allspark-applicability-analysis-15-research-sources-full-disposition.md`,
`inbox/open-ontologies-vs-arsumbris.md`, `knowledge/au-engine-build-profiling-2026-10.md`,
`inbox/arsumbris-fork-backlog.md`.
