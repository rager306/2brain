# Topic maps link half their members: 372 of 732 declarations, measured 2026-11-17

**Question.** A note declares its topic with `topics: [[some-map]]`; the map is the page a reader opens
to navigate. Does the map's body link the notes that name it?

**The graph is not the problem.** `references_in` answers the reverse direction exactly: for
`knowledge/agentic-architecture.md` the engine returns 126 references, 67 of them
`surface: frontmatter`, `slot: topics`. The edge exists. What may not exist is a *reader's* path from
the map to the note.

**Method.** `tools/kb/moc_links.py` (`--selftest`) reads the catalogued corpus, finds every
`type: moc` instance, collects the notes whose frontmatter names it, and subtracts the ones its
BODY links. A stem naming two files is reported separately rather than counted as a gap, because a
bare wikilink to it would be ambiguous and the engine already warns about that.

```sh
python3 tools/kb/moc_links.py            # report
python3 tools/kb/moc_links.py --json     # per-map rows
python3 tools/kb/moc_links.py --check    # gate: non-zero when any map has a gap
```

**Numbers (1 047 notes, 52 maps, 2026-11-17).**

| measure | value |
|---|---|
| declarations (map named by a note) | 732 |
| declarations the map body does not link | **372** |
| maps with a gap | **31 of 52** |

The seven maps that carry most of it:

| map | unlinked / namers |
|---|---|
| `index` | 63 / 63 |
| `agentic-architecture` | 51 / 66 |
| `memory-systems` | 50 / 71 |
| `jev` | 39 / 61 |
| `tool-use` | 32 / 42 |
| `agent-security` | 21 / 21 |
| `zeroclaw-decisions` | 20 / 66 |

**What was rejected, and why.** The first version of this tool generated a `## Named by` section into
every map: a marker block listing the notes that name it. Two problems, both fatal:

1. **It is a second source of truth.** The list already exists as the notes' `topics:` field; a
   generated copy adds machinery whose only job is to keep two copies in step, and the tool then
   needs a `--check` gate to detect its own drift. The engine already serves the index —
   `references_in` — so the copy buys nothing a read does not.
2. **It moves the graph to fix a reading problem.** The write pass added 372 links and pushed
   `graph_shape` from 4 754 to 5 126 edges without a single note gaining a claim.

It was written, measured, and reverted (vault `c85ec01`, unchanged). This tool is the report.

**What the numbers mean.** `index` and `agent-security` are complete gaps: every note that names
them is invisible from the map. The four largest maps are between a fifth and half covered. That is
a content decision — which of 372 notes belong under a human-curated heading — not a mechanical
one, and the alternative to a link is not "nothing": `references_in` gives any client the exact
reverse list, one read.

**Pass 1, done 2026-11-17 (vault `620ffba`).** The three largest maps now link every note that names
them — `agentic-architecture` 52 → **0 of 66**, `memory-systems` 50 → **0 of 71**, `jev` 39 → **0 of 61**
— with each note placed under an existing heading rather than appended in one list. The vault total
fell 372 → **232** unlinked, maps with a gap 31 → **28**, edges 4 754 → **4 895**, components
207 → 206, diagnostics 0.

Two cases the pass had to handle rather than skip:

- `codeactinstruct-enables-agent-training-without-general-capability-degradation` names **two** archive
  files, so a bare link is ambiguous and the engine says so (`navigational-target-ambiguous`); the
  map now carries the path-qualified form;
- `jev-applications` was already listed twice under `jev`'s Related Topic Maps, so the generated row
  would have been a third — the map keeps its two.

**Pass 2, done 2026-11-17 (vault `3172c2d`).** Five more maps link every note that names them:
`agent-security` 21 → **0 of 21**, `rust-ai-systems` 15 → **0 of 31**, `rust-ecosystem-kb-map`
16 → **0 of 18**, `self-improvement` 12 → **0 of 15**, `search-patterns` 10 → **0 of 11**. The vault
total fell 232 → **155**, maps with a gap 28 → **23**, diagnostics 0.

Two placements worth naming, because both are structure rather than a list:

- `search-patterns` gained a **Claims** section: four `claim` notes name it as their topic and state
  the decision a measured insight below supports, so the map shows finding and decision together
  instead of leaving the claim unreachable;
- `agent-security` and `self-improvement` each gained a **Harness Research (allspark/prime-agent)**
  section, which is where the twelve allspark notes that name them actually belong.

**What remains: 155 declarations across 23 maps, and the head is now a decision, not a gap.**

| map | unlinked / namers | why it may stay |
|---|---|---|
| `index` | 63 / 63 | the entry map: a curated front page, not an exhaustive index. Making it complete would make it useless. |
| `tool-use` | 32 / 42 | a real gap; the next pass. |
| `zeroclaw-decisions` | 17 / 59 | a decision log; the missing ones are recent ingest the log has not absorbed. |
| the other 20 | ≤ 5 each | mostly one or two notes each, below the cost of a judgement pass. |

**The rule the two passes earned.** A note earns a link when the map has a heading whose name is true
of it. When it does not, the choice is to add a heading that names the reason — which is what the two
harness sections are — or to leave the note unlinked. What is never acceptable is a link with no
heading, and it is why the generated-section version was rejected: it produced exactly that, 372 times.

**Next.** `tool-use` is the only remaining map with a large gap, and it is a genuine content pass.
`index` should be decided rather than measured: either it is a curated front page (and 63/63 is
correct), or it is meant to be the full map and its job should move to a generated view the engine
already serves (`references_in`).
