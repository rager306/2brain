# The context pack: what an agent needs, and why it is a contract rather than a tool

Two independent readers of this repository asked for the same thing in the same week: not a better rank list, but an
answer that says **what it is made of**. The external review put it as four questions; the second agent in the
cross-agent run put it as "not an empty answer but here are the nearest notes and here is what none of them
establishes". This note fixes the fields, the composition that produces them today, and the measurement that would
justify a dedicated tool.

## The four questions

1. Why is this fragment included?
2. Which source and version backs it?
3. What is missing from the answer?
4. Does anything conflict, or is anything superseded?

A rank list answers none of them. `tb_search` gives path, score, matched terms and now `weak`; a caller that needs
the rest has to know to ask for it, and today that knowledge lives in whoever wrote the caller.

## The pack

| field | where it comes from today | what it is for |
|---|---|---|
| `workspace`, `member` | `tb_overview`, `tb_resolve_member` | the area the answer claims to cover |
| `engine_version` | `_meta."io.2brain/engineVersion"` | so a reader can tell two answers apart |
| `query`, `intent` | the caller | what was asked, in the caller's words |
| `fragments[]` | `tb_search` then `tb_content` per hit | the evidence |
| `fragments[].path`, `hash`, `commit` | `tb_content` | a fragment a reader can re-read and verify |
| `fragments[].provenance` | the pack: `search-excerpt` or `content` | which of the fields below a reader can trust |
| `fragments[].excerpt` | `tb_search` `excerpt`, else `tb_content` `max_chars` | enough to judge, not the whole note |
| `fragments[].reason` | `tb_search` score, terms, `weak`; `tb_references` for link-reached fragments | why it is here, in the engine's own words |
| `fragments[].status` | the note's frontmatter `status` | current, proposed, superseded — the difference between a decision and a rumour |
| `missing` | the caller, or a `tb_grep` for the terms that found nothing | what the base does not establish |
| `conflicts` | two fragments whose `status`/`claim` disagree on one topic | the reader's judgement, surfaced rather than resolved |
| `budget` | the caller: fragments × excerpt + overhead | what was spent, so the next call is a decision |

## The composition as it stands

```
tb_search  {"query": ..., "limit": 8}          # candidates, with matched_terms, weak and excerpt
tb_content {"path": hit, "max_chars": 1200}    # only for a hit the search did not quote
tb_references {"path": ..., "direction": ...}  # only for fragments reached by following a link
tb_diagnostics                                 # only when a fragment claims to be current
```

Five calls for one answer, four once the search quotes the fragment itself. The pieces are all there; what is
missing is that a caller has to invent the assembly, and two callers will invent two different assemblies — which is
the failure the ecosystem reports (one client, two bases, answers nobody can compare).

## Why not a tool yet

The trajectory's rule holds: a new tool has to measurably cut calls and bytes. Two numbers decide it:

- **calls per answer**: if the composition is five calls and a `tb_context` is one, that is a 5× reduction in round
  trips, and on a local socket that is not the cost — the cost is the model reading five payloads instead of one;
- **bytes per answer**: the composition's `tb_content` windows are where the token cost lives, so a pack that chooses
  its own windows has to be *smaller* than a caller choosing them, or it is a convenience rather than an improvement.

The measurement: ten tasks from `tools/ranking/tasks.json`, the same question asked both ways, counting calls, bytes,
and whether the pack contained the minimal evidence. A pack that loses evidence to save bytes is rejected.

## What this fixes in the vault, not only in the client

The cross-agent run showed the deeper half: a pack can only report `status` if the notes carry one, and it can only
report conflicts if two notes disagree *visibly*. The vault today is 99% `status: current`, so a pack built on it
would report "no conflicts" and be technically correct and practically useless. That is why the lifecycle vocabulary
and the `proposed → accepted` rule are part of this work rather than a separate hygiene chore.

## The composition, measured

`tools/kb/context_pack.py` builds the pack above from the existing tools and counts what it costs. On the twelve
tasks of `tools/ranking/tasks.json`, live vault, 2026-11-16:

| fragments read | excerpt chars | calls per answer | bytes per answer | tasks whose evidence appears |
|---:|---:|---:|---:|---:|
| 3 | 1 200 | 4 | 8.7 KB | 7 / 9 answerable |
| 5 | 1 200 | 6 | 15.2 KB | 7 / 9 |
| 8 | 600 | 9 | 13.3 KB | 7 / 9 |
| 8 | 1 200 | 9 | 24.2 KB | 7 / 9 |

**Three fragments find exactly the evidence eight do.** Reading eight documents per answer — the naive composition —
costs 2.25× the calls and 2.8× the bytes for the same binary hit rate. That is the number a `tb_context` has to beat:
its budget must be at most four calls and about nine kilobytes per answer, or it is a convenience wrapper with a
worse price.

The binary "evidence appears" is a weak judge on its own: a multi-note question needs *supporting* fragments, not one
mention. The harness now reports `evidence_paths / expected_paths` per task, and at three fragments every answerable
task scores 1/1 except the ambiguous one (0/3 — a query with three legitimate answers cannot be covered by a
three-fragment pack that ranks one note first). A pack that finds the note but not its corroboration does not count
as a pass.

### The excerpt experiment: one call instead of four

A `tb_search` match now carries `excerpt` — up to 240 characters of the note's body around the first query term,
one line, cut on word boundaries, `…` where an edge was cut — and the page states `excerpt_chars`. The pack reads
that field and skips the `tb_content` call for the fragment. Same twelve tasks, same three fragments, same
live-vault content: a `cp -a` snapshot of `/root/vault` served by temporary engines on both arms, so before and
after see one corpus. Measured 2026-11-16:

| arm | calls per answer | bytes per answer | tasks whose evidence appears | corroboration at 1.0 |
|---|---:|---:|---:|---:|
| deployed engine and adapter: `tb_content` per fragment (the table above) | 4.00 | 8.86 KB | 7 / 9 | 7 / 9 |
| this build, `--no-excerpt` control: the excerpt arrives and is ignored | 4.00 | 10.82 KB | 7 / 9 | 7 / 9 |
| this build, excerpt used | **1.00** | **2.44 KB** | 7 / 9 | 7 / 9 |

**Calls fall 4× and bytes 3.6× with the evidence unchanged.** No answerable task lost its note and no multi-note
task lost its corroboration: the same fragments arrive either way, because the excerpt changes what a match carries,
not what the ranking returns.

The control is a control, not a second baseline. It receives the search answer's excerpts and throws them away,
which is exactly where its extra 1.96 KB comes from — three excerpts are about 2 KB, which is the treatment's whole
answer. It exists so that the last two rows are one binary and one socket: the difference between them is the pack's
use of a field, and nothing else. The first row is the state the table above was measured in: on the snapshot it
reproduces that table's 4 calls and 7/9, at 8.86 KB where the live vault measured 8.7 KB — the corpus moved, not
the composition.

Where the bytes went: before, one `tb_content` per fragment bought 1 200 characters of prose plus `hash`, `commit`
and `status`; after, the search answer's excerpt is 240 characters of prose and nothing else. The largest of the
twelve answers fell from 14.6 KB (control) and 11.7 KB (deployed) to 3.5 KB.

A smaller fragment is also a less certain one, so the pack labels it: `provenance` is `search-excerpt` or `content`.
A `search-excerpt` fragment has no `hash`, `commit`, `about` or `status` — the search answer holds no frontmatter —
so a reader that needs to re-read the note, pin it to a commit, or weigh its lifecycle state asks `tb_content` for
that fragment. Naming the provenance is the point: a fragment whose fields are silently `null` is worse than one
that says what it is. The same check is the fallback: a fragment whose excerpt the engine did not send takes the
`tb_content` path, so the pack still works against an engine from before the field existed.

Byte convention: `bytes` is `len(json.dumps(payload))` over each call's payload, `ensure_ascii` default, so a
Cyrillic character counts as its `\uXXXX` escape — an artifact of the counter, applied to both arms alike.

Reproduce both rows with one engine: `python3 tools/kb/context_pack.py --tasks --fragments 3` uses the excerpt,
`--no-excerpt` ignores it and takes the `tb_content` path. Point either at a particular engine with `--socket`
and `--vault`; the numbers above come from two temporary engines over a copy of `/root/vault`, so no daemon was
restarted and the served vault was never written.

### What the pack already says that a rank list cannot

- `status` and `about` come from a fragment's frontmatter when its `provenance` is `content`, so a reader sees
  "proposed" or "superseded" beside the text instead of trusting it — and a `search-excerpt` fragment says so
  rather than presenting absent fields as unknown;
- `weak` is carried per fragment rather than averaged away, so a page of common-word matches is visible as such;
- `hash` and `commit` come from the same read that produced the excerpt when that read happened, so a fragment can
  be re-read and verified, and a fragment that skipped that read is labelled `search-excerpt`;
- `missing` and `conflicts` remain the caller's work — this composition does not guess them, and an empty list means
  "nobody looked", not "nothing is missing". That is the honest state and the reason the contract is worth writing
  down before a tool hides it.
