# Retrieval spike (R1): a bundled engine against the current one

Status: measured, 2026-11-15. Evidence track R1 of `docs/architecture-plan.md`, commissioned by
`docs/graph-engine-assessment.md` §7 step 5. **No repository code was changed to run it**; the
corpus lives under `/tmp`, the vault was read-only, and nothing was written back.

## 1. What was measured, and against what

| piece | value |
|---|---|
| engine under test | HelixDB `ghcr.io/helixdb/helixdb:v0.0.16`, local server, `127.0.0.1:18080`, data dir on local disk |
| corpus | `/root/vault` exported to 1 036 nodes (path, type, status, description, body ≤ 12 k chars) and **2 730 `LINKS` edges** |
| edge rule | a `[[slug]]` resolves when exactly one note in the vault has that basename; 140 slugs stayed unresolved (263 mentions) |
| baseline | the live engine through `arsumbris-rs` (release binary), same session, same host |
| comparison shape | three queries, medians of five runs, warm process |

Corpus caveat, stated up front: the engine reports 5 046 wikilink occurrences and 0 broken links,
my exporter resolves **2 730 unique pairs** because its resolver is deliberately simple (unique
basename, no type/path rules). The two graphs are therefore comparable in shape but not identical;
every number below is quoted against the engine on its own graph and against HelixDB on the
exported one.

## 2. Results

| # | query | HelixDB | current engine | note |
|---|---|---|---:|---|
| Q1 | top-20 in-degree nodes | **103.5 ms** (min 88.4) | **9.8 ms** (`au_hubs`) | the engine ranks from an in-memory `backlinks` map; HelixDB scans relationships |
| Q2a | 2-hop, undirected, explicit two `MATCH`es, type filter, **count** | **54.7 ms** (min 47.3), 160 nodes | — | returns a count, not a subgraph |
| Q2b | 2-hop, directed out, type filter, **count** | **30.4 ms** (min 20.9), 70 nodes | — | |
| Q2c | 2-hop, both directions, `au_neighborhood` **materialising 253 nodes** | — | **102.2 ms** (min 89.2), 238 KiB payload | not the same output as Q2a/b |
| Q2v | 2-hop as a variable-length pattern `*1..2` | **unsupported** | n/a | `unsupported_feature: variable_length_pattern`, phase `compile` |
| Q3a | BM25 over bodies, k = 10 | **24.2 ms** (min 20.2), ranked | 132.8 ms (`au_grep`, literal scan, **unranked**) | 5.5× faster and ranked |
| Q3b | BM25 **within** a type prefilter (`type == "pattern"`, k = 5) | **28.2 ms** (min 25.5), membership correct | no equivalent | this is the genuine capability gap |

Two numbers matter and two do not. **The text half is a real win** (24 ms ranked versus 133 ms
scanning 5.4 MiB of markdown) and it is exactly the bottleneck the assessment measured. **The
traversal half is not a win**: the engine's in-memory ranking is 10× faster than HelixDB's
relationship scan, and HelixDB's 2-hop number is a count, which is cheaper work than returning a
253-node subgraph.

## 3. Cost of getting there

| step | measurement |
|---|---|
| pull image | ~1 min, 402 MB-class image (unmeasured size here) |
| node load | 1 036 nodes in 10 `UNWIND` chunks of 100 — completed, duration not captured |
| edge load, naive | `UNWIND` + two `MATCH`es per row: rejected with `429 resource_limit` / `memory_limit` — the nested scan exceeded the **256 MiB per-query budget** |
| edge load, working | one `UNWIND` per 25 rows with per-row `WHERE` lookups: 2 700 edges in **~70 s** (~39 edges/s) |
| text index build | accepted asynchronously, searchable after **~6 s** |
| documented limits | 16 MiB request, 256 MiB query memory, 30 s query deadline; `429` mixes three different causes and only some are transient |

That ingest path is a Cypher workaround, not the product's bulk path (their native loader exists),
but it is the honest cost of "just try it for an hour".

## 4. What the Cypher profile does not do

Findings that changed the assessment more than any latency:

- **Variable-length patterns are rejected at compile time.** The query shape a graph database is
  supposed to win at (`-[:LINKS*1..2]-`) is not in the MVP profile; traversal depth must be written
  as repeated fixed hops.
- **Full-text and vector search have no Cypher syntax** — they are native-only operations, so the
  interesting hybrid query leaves Cypher and becomes their operation-tree JSON or an SDK DSL.
- **`429` is not rate limiting.** It carries `resource_limit` with `detail: memory_limit` here, and
  the docs warn that only some `429`s are transient. My first loader retried all of them, which was
  the wrong response; the right one was to reduce the per-query working set.
- **Cypher methods are absent from the released SDKs** (Rust 3.0.0 among them); the documented path
  is HTTP or a repository build, and embedded mode needs `cargo add --git … --features embedded`.

## 5. Verdict against the go rule

The rule in the assessment: HelixDB must beat tantivy-plus-manual-traversal by enough to justify a
git-tag dependency and a second data model.

- **Text: it does not need to win — tantivy already wins.** 24 ms ranked BM25 is achievable with
  `tantivy` (0.27.0, 22 M downloads) or SQLite FTS5 inside the existing binary, with no new store.
- **Traversal: it loses.** 103.5 ms versus 9.8 ms for hub ranking on the same box; the 2-hop
  comparison is not apples-to-apples, and the expressive form that would justify a new query
  language is unsupported.
- **Hybrid: real, but premature.** Prefiltered BM25-in-a-subgraph works and is well designed; it
  is also something we can compose from a text index plus our own traversal, which already exists.

**Decision: do not adopt HelixDB.** Take its two lessons instead:

1. **Filter before ranking, and make membership exact** — their order is traversal → candidate
   membership → ranking → top `k`, with an explicit warning that filtering after search silently
   under-returns. Our `au_search`-style retrieval should copy that order, not the other one.
2. **One index, two columns of truth** — the capability table from `docs/architecture-plan.md`
   P0.1 and the schema table from P1.4 are the same table; a text index belongs to that table, not
   to a new engine.

Reopen when: hybrid retrieval with prefiltering becomes a product requirement **and** an embedded
build exists on crates.io (or a `tantivy`-plus-traversal composition measures >100 ms on the live
vault). Both are checkable in an hour.

## 6. Reproducing

```sh
docker run -d --name helix-spike -p 127.0.0.1:18080:8080 \
  -v /tmp/helix-spike/db:/app/db -e DB_PATH=/app/db ghcr.io/helixdb/helixdb:v0.0.16
python3 /tmp/helix-spike/export.py        # /root/vault -> corpus.json (read-only export)
python3 /tmp/helix-spike/load.py          # nodes; expect 429 on naive edge UNWIND
# edges: 25-row UNWIND with per-row WHERE lookups, see §3
```

The scripts are throwaway artefacts of this session, not repository code; the numbers above are
what they produced on 2026-11-15.
