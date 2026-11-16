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
