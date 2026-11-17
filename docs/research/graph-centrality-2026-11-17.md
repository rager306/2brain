# Structural centrality against degree: what the export says

Measured 2026-11-17 on the live workspace (`3 members`, 0 diagnostics) by exporting `link_graph` once
(1 227 nodes, 3 958 unique edges after collapsing repeated `from`/`to` pairs) and running
`tools/graph_centrality.py` against that frozen export. Offline tooling: no socket is read by the
analysis itself, nothing is written, and no ranking is changed.

## The three lists, top ten

| rank | by degree (`graph_health.central`) | PageRank | betweenness |
|---|---|---|---|
| 1 | zeroclaw-decisions | jev | agentic-architecture |
| 2 | index | memory-systems | index |
| 3 | memory-systems | inbox/agent-plasticity… | memory-systems |
| 4 | agentic-architecture | index | inbox/pattern-preregistered-quality-gates |
| 5 | jev | inbox/gdpevo-rule-hybridization… | jev |
| 6 | rust-ai-systems | zeroclaw-decisions | inbox/jevact-ibm-runtime-decision-gate |
| 7 | ruvector-architecture | agentic-architecture | inbox/qwen-office-2-team-agent |
| 8 | agentic-flow-architecture | inbox/xferagent-reconcile… | inbox/github-rubicon-git-storage |
| 9 | tool-use | rust-ecosystem-kb-map | rust-ai-systems |
| 10 | hermes-user-stories | inbox/recursive-self-improvement-survey… | inbox/google-agents-cli-eval-loop |

Overlap with the degree list at ten: **5** for PageRank, **5** for betweenness.

## What each adds, and what it does not

- **PageRank** half duplicates degree and half promotes *inbox notes* (`agent-plasticity`, `gdpevo`,
  `xferagent-reconcile`, `recursive-self-improvement-survey`). Those are pointed at by notes that are
  themselves central — a citation-importance signal, not a shape signal. For a ranking change that
  would have to be measured against the task set; for a shape diagnostic it says little.
- **Betweenness** is the one list degree cannot produce: `pattern-preregistered-quality-gates`,
  `jevact-ibm-runtime-decision-gate`, `qwen-office-2-team-agent`, `github-rubicon-git-storage`,
  `google-agents-cli-eval-loop` are connectors between parts of the base, and several have modest
  degree. That is a different question from "which note is a map", and it is the list an intake pass
  could act on ("this note holds two families together").
- The two maps that lead both lists (`agentic-architecture`, `memory-systems`, `jev`, `index`) are the
  same nodes `graph_health.articulation_points` already flags; the genuinely new information is the
  lower part of the betweenness list, not its top.

## Verdict

**Keep it offline.** Betweenness is worth a read *if* a consumer appears: it is O(V·E) and would add a
measurable cost to every `graph_health` call, while the articulation-point list already answers
"bottleneck" for the top of the graph. PageRank is a ranking signal, and the ranking already has a
measured harness; it should enter through that harness or not at all.

Boundaries, stated: the export collapses direction and edge kinds, repeats are deduplicated, and no
weights exist; none of these numbers is a write gate or a claim about semantic importance. The tool
does not read the live workspace itself, and the comparison used one frozen snapshot.

## Reproduce

```sh
# one export, read-only (engine_probe speaks the engine wire; link_graph is a read)
python3 tb-engine/scripts/engine_probe.py --socket ~/.2brain/au-engine/run/<hash>.sock \
    --out /tmp/export --verb link_graph
# convert the payload to {"nodes", "edges"} and run the tool
python3 tools/graph_centrality.py /tmp/live-graph.json --top 20
```
