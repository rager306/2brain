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
| `fragments[].excerpt` | `tb_content` `max_chars` | enough to judge, not the whole note |
| `fragments[].reason` | `tb_search` score, terms, `weak`; `tb_references` for link-reached fragments | why it is here, in the engine's own words |
| `fragments[].status` | the note's frontmatter `status` | current, proposed, superseded — the difference between a decision and a rumour |
| `missing` | the caller, or a `tb_grep` for the terms that found nothing | what the base does not establish |
| `conflicts` | two fragments whose `status`/`claim` disagree on one topic | the reader's judgement, surfaced rather than resolved |
| `budget` | the caller: fragments × excerpt + overhead | what was spent, so the next call is a decision |

## The composition as it stands

```
tb_search  {"query": ..., "limit": 8}          # candidates, with matched_terms and weak
tb_content {"path": hit, "max_chars": 1200}    # per chosen hit: text window, hash, commit
tb_references {"path": ..., "direction": ...}  # only for fragments reached by following a link
tb_diagnostics                                 # only when a fragment claims to be current
```

Five calls for one answer. The pieces are all there; what is missing is that a caller has to invent the assembly,
and two callers will invent two different assemblies — which is the failure the ecosystem reports (one client, two
bases, answers nobody can compare).

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

### What the pack already says that a rank list cannot

- `status` and `about` come from each fragment's frontmatter, so a reader sees "proposed" or "superseded" beside
  the text instead of trusting it;
- `weak` is carried per fragment rather than averaged away, so a page of common-word matches is visible as such;
- `hash` and `commit` come from the same read that produced the excerpt, so a fragment can be re-read and verified;
- `missing` and `conflicts` remain the caller's work — this composition does not guess them, and an empty list means
  "nobody looked", not "nothing is missing". That is the honest state and the reason the contract is worth writing
  down before a tool hides it.
