# Work per read, measured at three corpus sizes

Entry 1 of [[kutha-ideas-2026-11-16]] proposes asserting a *visit bound* on every read
primitive — documents inspected, posting blocks decoded — and testing it at three corpus
sizes, "because latency on a thousand notes hides a linear scan that stops hiding at a
hundred thousand". Its own acceptance criterion: **a slope above about 1.2 in
visited-documents-per-query against note count is a defect**.

This note records what the counters are, the probe that exercises them, the numbers at
256 / 512 / 1 066 notes, and the verdict per verb. The code is observation only: no read
consults a counter, and every answer in the comparison below is byte-identical to the
answer the same binary gave before the counters existed.

## What was counted

`verb_metrics.rs` already kept, per verb, a request count and a latency histogram, served
by `graph_health` with `verbs: true`. Each row gains two counters:

| field | unit |
|---|---|
| `documents_visited` | document-visit STEPS: a document counted once per pass that examines it |
| `posting_blocks_decoded` | posting blocks decoded: one per term list a scoring or adjacency pass walks |

A step, not a distinct document, on purpose: deduplicating would need a set per request,
and the quantity the bound is about is "how much of the corpus does one request walk", not
"how many distinct notes does it name". A two-term `search` that scores one note twice
contributes two.

**Cost.** The accumulator is a thread-local `Cell` tally, flushed into the per-verb atomics
once, after the request; a walk in a loop pays a plain cell add, not an atomic on a shared
cache line. Where a batch size is already known — a posting list's length — the walk notes
it once, so a scoring loop over 600 postings costs one tally add. A read that opens no
scope pays nothing and reports nothing; a nested scope counts for its own verb and leaves
the enclosing tally intact.

**Instrumented paths** (the ones the entry names, plus the two composed reads that reuse
them): `search` (posting decode, adjacency merge), `grep` (the byte scan), `references_in`
(the backlink lookup), `neighborhood` (the graph walk), `hubs` (the backlink fold),
`graph_shape` / `graph_health` (the closure and its SCC pass), and `overview`, which
composes the hub ranking and the graph shape.

The wire documentation is `tb-engine/crates/tb-engine/WIRE.md`, `graph_health` §`verbs`;
the change is additive and the golden `graph_health` capture is taken without `verbs`, so
no capture changed.

## The probe

`tools/probe/visit_bound.py`.

- **Copies, never the live vault.** It runs `cp -a /root/vault` once, derives the
  truncations from that copy, and starts one private engine per corpus: own `HOME`, own
  socket under it, `au daemon start` on the copy. It refuses a `--work` directory that
  overlaps the vault, sends no mutation verb, and stops only the daemons it started. The
  production daemon is never touched.
- **Nested truncation.** Notes are ordered by a bit-reversal permutation of the sorted
  relative paths, and the n-note corpus is the first n of that order. Prefixes therefore
  nest — the 256-note corpus is inside the 512-note one, both inside the full copy — so a
  difference between sizes is the corpus growing, never the corpus changing shape. Type
  definitions (`.type.yaml`) and the repo manifest are never removed; only `.md` notes are.
- **A fixed 20-query text set**, chosen to span document-frequency bands (0.3 % to 61 % of
  notes): ten single terms, six two-term ORs, two three-term ORs, one `error->fix`
  conjunction and one explicit AND. The same set runs at every size, with `limit: 10` — a
  page. Measured document frequency per query is flat across the three sizes (for example
  `engine` 0.12 / 0.12 / 0.12 of notes, `agent` 0.65 / 0.62 / 0.61), which is what makes
  the series comparable.
- **Per-request deltas.** For each query the probe reads `graph_health`'s counter for the
  verb, issues the query, and reads the counter again; the difference is that request's
  visits.
- One fixed seed path per size for `references_in` and the two `neighborhood` depths, chosen
  from the smallest corpus (the note the others link to most) so it exists at every size.

## The numbers

Three corpora derived from one copy of the live vault: 256, 512 and 1 066 notes (the count
of `.md` files in the copy on 2026-11-16; the entry's own table counts 1 032 that carry
frontmatter, and the difference is notes with no frontmatter block, which `grep` scans and
`search` does not index).

Two slopes are reported, because they answer different questions:

- **linear slope** — least squares of visits against note count, in visits per note. This is
  the entry's criterion: above about 1.2 is a defect.
- **log-log slope** — the elasticity `d ln(visits) / d ln(notes)`: 1.0 means "grows in
  proportion to the corpus", below 1 means "grows slower than the corpus". It is the honest
  shape for a count that starts from a fixed overhead, and it separates "linear" from
  "worse than linear".

Mean documents visited per query, and both slopes:

| verb | 256 notes | 512 notes | 1 066 notes | linear slope | log-log | R² | the answer it returned |
|---|---:|---:|---:|---:|---:|---:|---|
| `search` | 92.2 | 196.1 | 401.9 | 0.38 | 1.00 | 1.00 | 23 / 49 / 100 matches |
| `grep` | 405.0 | 661.0 | 1 215.0 | 1.00 | 1.00 | 1.00 | 189 / 280 / 569 matches |
| `references_in` | 18.0 | 30.0 | 59.0 | 0.05 | 0.86 | 1.00 | 18 / 30 / 59 edges |
| `neighborhood` | 4.0 | 7.0 | 12.0 | 0.01 | 0.86 | 1.00 | 8 / 20 / 61 nodes |
| `hubs` | 94.0 | 304.0 | 836.0 | 0.92 | 1.53 | 1.00 | 20 / 20 / 20 rows |

The two reads whose answer IS the corpus, measured as the control at the other extreme —
their visit count is nodes + edges of the in-scope graph, checked exactly against
`graph_shape`'s own `node_count` and `edge_count` at the full size (1 214 + 4 697 = 5 911):

| verb | 256 notes | 512 notes | 1 066 notes | linear slope | what it visited |
|---|---:|---:|---:|---:|---|
| `graph_shape` | 604 | 1 589 | 5 911 | 6.75 | nodes + edges (404+200 / 660+929 / 1 214+4 697) |
| `overview` | 698 | 1 893 | 6 747 | 7.68 | the hub fold plus the graph shape |

**The closure rows are not comparable across sizes, and their slope is an artifact.** A
truncated corpus loses edges faster than nodes: a wikilink survives only when BOTH of its
endpoints do, so edge density per node falls with the truncation (0.50 → 1.41 → 3.87 edges
per node). The growth is in the graph, not in the read: the read visits each node and each
edge once, which the exact check above confirms. The verdict below therefore rests on the
first table.

### `search` varies with the query, as BM25 says it must

Visits are the sum of the query terms' document frequencies, plus the adjacency merge's
steps for a multi-term query. A rare term is cheap at any corpus size; a common one is not.
Per-query linear slopes for the same 20 queries:

| query | 256 | 512 | 1 066 | slope |
|---|---:|---:|---:|---:|
| `wikilink` | 2 | 2 | 3 | 0.00 |
| `mutation` | 3 | 9 | 18 | 0.02 |
| `daemon` | 9 | 22 | 45 | 0.04 |
| `page` | 11 | 21 | 34 | 0.03 |
| `note` | 13 | 22 | 51 | 0.05 |
| `engine` | 24 | 49 | 113 | 0.11 |
| `index` | 39 | 78 | 145 | 0.13 |
| `decision` | 46 | 97 | 197 | 0.19 |
| `graph` | 61 | 138 | 296 | 0.29 |
| `agent` | 143 | 285 | 590 | 0.55 |
| `daemon status` | 81 | 169 | 351 | 0.33 |
| `bm25 ranking` | 16 | 59 | 131 | 0.14 |
| `search index` | 156 | 328 | 633 | 0.58 |
| `pattern decision` | 216 | 430 | 864 | 0.80 |
| `error fix` | 64 | 135 | 270 | 0.25 |
| `graph type` | 154 | 346 | 768 | 0.76 |
| `search index cache` | 269 | 596 | 1 164 | 1.09 |
| `knowledge graph type` | 346 | 737 | 1 589 | **1.54** |
| `error->fix` (exact) | 34 | 71 | 142 | 0.13 |
| `search index` (AND) | 156 | 328 | 633 | 0.58 |

The second counter is the sharper one, and it is flat: **posting blocks decoded does not move
with the corpus at all** — 1 for a one-term query, 4 for a two-term query (2 scoring blocks
and 2 for the adjacency merge), 9 for three terms, 2 for the `error->fix` conjunction, at
256, 512 and 1 066 notes alike. The number of term lists a query touches is the query's own
width. A block count that grew with the note count would mean the engine was decoding
postings the query never named; it does not.

The other four verbs do not vary by query. Every `grep` query visits the whole catalogue —
405 / 661 / 1 215, exactly the note count plus the 149 catalogued type-defs and assets at
every size — and returns matches from a 10-row page whatever `limit` says. `references_in`
visits its inbound edges (18 / 30 / 59, the payload length exactly). `neighborhood` expands
the seed (1) plus its fan-out at depth 2 (7 / 13 / 23 of the reachable set, with `max_nodes`
at 200 never approached). `hubs` folds the backlink index once (94 / 304 / 836 candidates
for 20 rows).

## Verdict

**No read in the five does work that grows faster than the set it must examine.** Read the
linear slope for what the entry asks, and the log-log for shape: `search` and `grep` are
linear in their candidate set, `references_in` and `neighborhood` are flat or nearly so, and
`hubs` is the one that grows slightly faster than the corpus over this range.

- **Worst slope: `hubs`, linear 0.92 and log-log 1.53.** It folds the entire backlink index —
  one entry per referenced file — then sorts, to return 20 rows and no total. The referenced
  fraction climbs 0.37 → 0.59 → 0.78 of the notes, so the asymptotic slope is 1.0: in a
  connected corpus almost every note is referenced by something. This is the one read of the
  five that touches most of the corpus for an answer that cannot require it. The fix, if it
  ever earns one, is a ranking maintained at build time (the incremental patch already knows
  which edges moved), not a cheaper scan. At 1 000 notes the fold is microseconds; the
  counter makes the linearity visible before it costs anything.
- **`grep` at exactly 1.00** is a byte scan over the catalogue: 1 215 files opened to return
  11 matches for `wikilink`. The work is what the answer costs, because the read reports an
  exact whole-corpus `total` beside the page. Bounding it means changing that contract (an
  early-exit mode that stops after `offset+limit` matches and says `truncated: true` or
  `total: null`). A wire decision, not a counter.
- **`search` at 0.38 on the query set** is exhaustive scoring, and its per-query slopes run
  from 0.00 (`wikilink`, three notes hold the term) to **1.54** (`knowledge graph type`). The
  1.54 is above the entry's 1.2 line and is not a defect: a k-term OR visits the sum of k
  document frequencies, bounded by k·n. The bound worth asserting is that one — visits ≤
  Σdf(term), with k as the width of the query and not a constant — and the per-query table
  above is that bound made visible.
- **`references_in` (0.05) and `neighborhood` (0.01)** are answer-bounded and measurably so:
  the backlink lookup visits exactly the edges it returns, and the walk visits the nodes it
  expands. The property the entry wanted to exist is real, and it is where the counters put
  it — not where a slope threshold alone would look.

**Nothing here needs a fix rather than a counter.** Each linear read is linear for a stated
reason, and each reason is visible in the answer it returns. What the counters buy is the
ability to notice when that stops being true — a `search` above Σdf(term), or a `hubs` above
1.0 — and both are one `graph_health` read away.

## Reproducing it

```sh
cd /root/2brain/tb-engine
RUSTUP_HOME=/root/.rustup cargo +1.99.0 build --release -p tb-cli --offline
cd /root/2brain
python3 tools/probe/visit_bound.py --json /tmp/visit-bound/rows.json
```

The probe prints the two tables above; `--json` writes the raw per-query rows. It takes a
few minutes: three cold engine builds, three daemons started and stopped, and roughly 140
measured requests. `--keep` reuses the copies under `/tmp/visit-bound` on a re-run.

## Equivalence: the counters changed no answer

Two binaries were compared on one frozen copy of the live vault (copied to
`/tmp/visit-bound/corpus`, own `HOME`, one daemon at a time): the engine at `HEAD` without
the counters, and the same tree with them. Seventeen reads were captured from each —
`overview`, `hubs`, `diagnostics`, `graph_health`, `graph_shape`, `files`, `neighborhood`
at depth 1 and 2, `references_in`, `references_out`, `link_graph`, `type_graph`, three
`search` shapes, `grep`, `type_closure` — normalized the way `engine_probe.py` normalizes
(workspace root to `$VAULT`, observed `version` dropped, keys sorted) and compared with
`diff -ru`. The result was empty: same scores, same ordering, same payloads. The visit
counters appear only under `verbs: true`, which no golden capture and no client sets.

## What this does not measure

- **Traffic.** The five verbs were chosen by code path, not by usage: the served binary on
  this host predates the `verbs` field, so the engine could not rank its catalog by
  request count. A read that is rarely used and linear matters less than one that is used
  constantly; this note cannot tell them apart.
- **Concurrency.** The counters are process-global and a request's tally is thread-local, so
  a per-request delta is exact only while one client reads the engine. The probe is
  single-client.
- **Cost in nanoseconds.** The counters say what a read walked, not what that walk cost.
  The latency histogram beside them says that, and its buckets are octaves.
- **The corpus as a whole.** Two hundred and fifty-six and 512 notes are truncations, not a
  different knowledge base. A link survives only when both endpoints do, so a truncated copy
  is sparser than the full vault and the graph reads' counts are not comparable across the
  three sizes (see the closure table above). The text and backlink reads do not depend on
  density and are comparable.
