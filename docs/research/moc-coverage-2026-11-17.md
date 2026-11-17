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

**Next, in order of payload.** `agent-security` (21 of 21), `rust-ecosystem-kb-map` (16 of 18),
`rust-ai-systems` (15 of 31), `self-improvement` (12 of 15), `search-patterns` (10 of 11),
`zeroclaw-decisions` (20 of 66). The first five are small enough to finish in one pass each; the last
is a judgement pass. A note whose placement is unclear gets no link.

The gate stays `moc_links.py --check` over the maps touched, not the vault: the remaining 232 include
maps whose gap is deliberate (a hub that should link only the notes it narrates).
