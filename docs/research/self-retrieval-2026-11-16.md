# Does every note answer to its own description? Yes — 1037 of 1037 — and the four exceptions are duplicates

A note's `description:` is the sentence its author wrote to say what it is about. If a query made of that sentence
cannot find the note, the index or the ranking is at fault rather than the reader. So the cheapest invariant in the
whole knowledge base is `search(description(note)) -> note`.

Measured 2026-11-16 through the served MCP server on `/root/vault`: every one of the 1 037 notes whose `description`
is longer than 30 characters, queried by the first 200 characters of that description, `limit: 5`.

| result | notes |
|---|---:|
| the note at rank 1 | 1 033 |
| at rank 2 or 3 | 4 |
| not in the top five | **0** |

The four that were not first are one shape, repeated: an archived copy outranking the live one.

| query note | what came back |
|---|---|
| `.claude/skills/connect/SKILL.md` | `ops/skills-archive/connect-2026-03-06-pre-upgrade.md`, then the note |
| `.claude/skills/extract/SKILL.md` | `ops/skills-archive/extract-2026-03-06-pre-upgrade.md` |
| `.claude/skills/next/SKILL.md` | `ops/skills-archive/next-2026-03-06-pre-upgrade.md` |
| `ops/skills-archive/refresh-2026-03-06-pre-upgrade.md` | the live `refresh/SKILL.md` (the pair inverted) |

## What the number does and does not say

It says the index is consistent with its corpus: a rebuild that broke, a tokenizer that dropped a field, or a
catalogue that lost a note would all move this below 1 037, so it is a cheap tripwire worth keeping.

It does **not** say descriptions are good queries. They are close paraphrases of the note's own body — the extraction
pipeline writes them that way — so a hit at rank 1 is partly a tautology. The measurement that would say something
about findability is a query written by a *reader* who has not read the note, and that is a task set
(`docs/research/task-set.md`), not a corpus operation.

## Description similarity is not a duplicate detector

Jaccard over description tokens, 1 030 notes with at least 8 tokens: **74 pairs at 0.75 or higher**. Four are exact
(1.0) — the archive/live pairs above. The rest are template families: `hermes-stories-*` writes the same paragraph
with one field changed, and their descriptions differ by about as much. A similarity threshold of 0.75 would flag a
legitimate family as duplication and miss a real duplicate whose description was rewritten. Duplicate detection needs
the body and the links, which is what `tools/kb/connect.py` reads.

The archive directory is the actionable half: those four notes sit in the vault, index beside live content, and win.
Either they carry a lifecycle state the ranking respects, or the archive does not belong in the served tree. That is
a corpus decision for the intake lane, not a search change.
