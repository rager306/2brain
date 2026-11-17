# Surfacing the graph diagnostics

The engine now answers questions about the **shape** of the knowledge base, not only its content:
fragility (bridges, articulation points), weave (triangles, transitivity), and the existing cycles,
sources, dead ends and centrality. This page says where those numbers are read and what they are for.
It is design, not operations: a second machine would serve the same fields.

## The read

`tb_graph_health` (MCP) / `graph_health` (wire) carries, on the in-scope workspace:

| field | meaning | how to read it |
|---|---|---|
| `bridges: [{a, b}]` | an undirected edge whose removal disconnects its ends | the two notes depend on each other with no second path; a link-pass candidate |
| `articulation_points: [{path, in, out, total}]` | a node whose removal disconnects at least two neighbours | a structural bottleneck; a map to split or a hub to relieve |
| `clustering: {triangles, transitivity}` | triangle count and closed-over-all triples on the same projection | how woven the corpus is; chains score near zero |
| `counts.{bridges, articulation_points}` | full-corpus counts, independent of the list cap | the number a card or a census tracks |
| existing `cycles`, `sources`, `dead_ends`, `isolated`, `central`, `broken_links` | unchanged | unchanged |

Projection rules, so two readers cannot disagree: edges are the resolved in-scope references with
direction dropped; parallel edges collapse for the structure but a pair joined more than once is never
a bridge; self-loops are ignored; a node with no edges is never an articulation point. `schema_version`
is 29 and the change is additive.

## Three surfaces, in order of who pays

1. **An agent's read.** One `tb_graph_health` call answers "what is fragile here". The long description
   names the fields so `tools/list`, `tb_search_tools` and `tb_describe_tool` can route to it; the
   concise profile keeps the first sentence only, so the payload does the talking.
2. **The card.** Add to `tools/cards/metrics.json`, each with its command and date:
   `graph.bridges`, `graph.articulation_points`, `graph.triangles`, `graph.transitivity`,
   `graph.isolated`, `graph.zero_inbound`. `render.py --check` then keeps the README honest. A weekly
   delta is the point: transitivity and zero-inbound are the two numbers an intake pass is supposed to
   move.
3. **The evidence.** `docs/research/sparx-2026-11-17.md` holds the definition, the source of the idea
   and the first measurement (37 bridges, 57 articulation points, 3 123 triangles, 0.1816 on a 1 228
   note snapshot). A later change cites the new number beside the old one.

## A demo that fits in one terminal

```sh
2brain mcp my-workspace      # any MCP client, then:
# tb_graph_health {"limit": 20}
```

The answer names the fragile joints and the weave in one object. For a human demo, show three lines:
the counts, the first two bridges, and the top articulation points — a map with 167 references and a
pattern note attached by a single inbound link are the two stories the numbers tell.

## What an intake pass does with it

- a note in `bridges` is attached by one thread: give it a second link, or decide it is a leaf;
- an articulation point that is a `type: moc` map is a bottleneck: split it, or link its members
  directly, then re-read the count;
- transitivity and triangles are the outcome metrics: if the link pass works, both rise, and isolated
  and zero-inbound fall. The engine's number is the one to track; a regex over bodies is not.

## What this surface does not do

- It is not a ranking input and not a write gate: an SCC is not a short-cycle policy, and none of
  these numbers authorizes a mutation.
- It does not replace `graph_shape` or `link_graph`; those stay the raw views.
- It does not detect a *semantic* hub: a map and a definition look alike to a degree count; the
  articulation list is a starting point for a human, not a verdict.
