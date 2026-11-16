# Where the knowledge base actually grows, measured

`tools/kb/census.py` on the live vault, 2026-11-16:

| measure | value |
|---|---|
| notes | 1024 |
| of them in `inbox/` | 557 |
| typed | 995 (97%) |
| described | 1011 (98%) |
| sourced | 888 (86%) |
| links (body only) | 3112, median 1 per note |
| notes with no *body* link | 424 (416 of them described) |
| graph | 1205 nodes, 4637 edges, 204 isolated, 0 broken links |

The capture side is healthy: typing, descriptions and sources are near-complete, and the graph has no broken links.
**The bottleneck is intake, not capture.** 557 notes sit in `inbox/` and 347 of the 424 unlinked notes are inbox
notes: material arrives, gets a description, and never joins the graph.

## What `connect.py` proposes, and how good it is

416 described, unlinked notes each get 3 neighbours from the server's own search over the note's description. Every
one produced candidates; 321 of 416 put a note from the same top-level directory first.

Judged by hand on the first two dozen:

- **Parallel families work perfectly.** The eight `hermes-stories-*` inventories link to each other on the first
  candidate, every time; the `omp-*` insights find the architecture note; the law-as-code note finds
  `law-as-code-topic-map`.
- **Everything else is topical noise.** `prime-typesafe-jev-comparison-insight` proposes a GPUI note, a Rust-parity
  note and a prime-agent note; only the third is a neighbour a person would write.

So the tool is a **proposal list with a usable minority**, not an auto-linker. That is the right shape: a person
accepts the family link in a second, and the noise costs a glance. Auto-applying would need a threshold the current
ranking cannot supply, because the score does not separate "same family" from "shares rare words" — the same
weakness the durability probe found in the other direction.

## The intake policy this implies

1. **A note joins the graph when it enters `knowledge/`, not when it enters the vault.** The inbox is a queue, and
   a queue that only grows is where a knowledge base dies.
2. **A note promoted out of `inbox/` gets a link to at least one existing note.** Not a tag, not a topic word — a
   wikilink, because that is what search and traversal both use.
3. **Families are linked as families.** The parallel-inventory case is the strongest evidence the corpus has: eight
   notes that only matter together, currently unlinked to each other.
4. **Weekly, not nightly.** The census is cheap and the numbers move slowly; a weekly delta is a signal, a nightly
   one is noise.

The measurable target: the unlinked share and the inbox depth fall while the note count stays flat. If the count
must rise, it should rise in `knowledge/` with a source and a link already attached.

## The lifecycle field carries no signal yet

`status:` across the vault, 2026-11-16:

| value | count |
|---|---|
| `current` | 993 |
| `pending` | 7 |
| `active` | 3 |
| `aging` | 1 |
| `acknowledged` | 1 |
| — | 3 rows that are documentation, not values (`current \| pending \| aging \| …`) |

So 99.3% of the corpus says `current`, and the five other values look like three different people's vocabularies
rather than one enum. Any "supersede the old decision" flow built on this field would be reading a constant.

That is the same lesson as the search sets: **a field that is never allowed to be false measures nothing.** Before
the lifecycle becomes load-bearing, it needs a small closed vocabulary (`current` / `superseded` / `proposed` /
`retired`), a rule for who may write it, and at least one real `superseded` note per superseded topic. Until then
the honest statement is that 2brain stores statuses, not that it tracks currency.

## A measured delta, hours later

The census run twice on the same day, which is the smallest useful experiment it can do:

| | 2026-11-16 morning | 2026-11-16 evening | delta |
|---|---:|---:|---:|
| notes | 1024 | 1032 | +8 |
| links | 3112 | 3120 | +8 |
| notes with no outbound link | 424 | 429 | **+5** |

Eight notes arrived and five of them joined the unlinked set. That is the thesis of this note, measured rather than
argued: the inbox fills faster than the graph, and a note nobody linked to is a note the next agent will not reach by
following anything.

It also fixes the weekly target in units: **the unlinked count is the number to move**, not the note count. A week
that adds ten notes and links nine of them is a better week than one that adds two and links neither.

## The metric was counting one kind of link, and it overstated isolation fivefold

Re-run a few hours later with the same code, then with a correction. The vault's convention is to put *what this note
is about* in the frontmatter as wikilinks:

```yaml
topics:
  - "[[hermes-user-stories]]"
```

The engine counts those as field references (`kind: field`, `slot: topics`), and they are what makes the graph
traverse. The census counted only body links, so it called a connected note isolated — and so did `connect.py`, which
proposed neighbours for notes that already had them.

Corrected numbers, same corpus, 2026-11-16:

| measure | value |
|---|---:|
| links, body + `topics:` | 4886 |
| median links per note | 3 |
| notes with no link of any kind | **76** |
| notes with no *body* link | 429 |
| notes with no `topics:` link | 165 |
| isolated in the graph (no edge either way) | 204 |

So the honest statement is: **76 notes are unlinked in the sense the convention uses, and 204 are isolated in the
graph's sense (nothing points at them and they point nowhere).** The 424 figure was an artifact of one regex, and it
made the base look five times lonelier than it is.

Two things follow, and the first is about instruments rather than content:

- **A link metric has to say which link it counts.** `census.py` now reports body, `topics:` and total separately and
  prints the bare counts for each kind; `connect.py` no longer proposes a link for a note that already has one.
- The intake target is still real but smaller: 76 unlinked notes, 165 without topics, and 204 isolated — the last is
  the one that matters, because a note nobody links to is invisible to traversal even if it links outward.

The morning delta (+8 notes, +5 with no body link) stays true as a body-link delta. It just is not the headline it
looked like.

## Correction two: the regex also missed `topics: ["[[x]]"]`

The same day, a second reader of the vault found the second spelling. `topics:` is written two ways —

```yaml
topics: ["[[ingar30-reviewer-economics-paper-panel]]"]     # inline array, most of f/patterns
topics:
  - "[[hermes-user-stories]]"                              # block list
```

— and the census matched only the block list. So "76 with no link of any kind" was still a statement about a regex.
Asking the engine instead (`tb_references` for each of the 76) gives the number that can be defended:

| question | answer | who can answer it |
|---|---:|---|
| notes with no body link | 429 | a regex |
| notes with no `topics:` link | 165 | a regex |
| notes with no reference either way | **13** | the engine |
| notes with **nothing pointing at them** | **43** | the engine |
| `graph_health` "isolated" | 204 | the engine, a wider definition |

The fourth row is the one the intake work should move. A note with one outbound link and nothing inbound is reachable
by nobody who does not already hold its path: 36 of the 43 are `f/patterns` notes whose only edge is the extract they
came from. That is the shape of the hole, and it is an in-degree problem — which the outbound metric never measured.

`census.py` now prints the regex view and the engine view side by side, and the engine's numbers are the ones to
quote. Two corrections in one day, both from the same root: **a metric computed over text files is a statement about
the file format, and a metric computed by the engine is a statement about the graph.** The tool now says which is
which, and that is the part worth keeping.

The lesson generalises past this repository: when two measurements of one thing disagree, the one that resolves the
data (the engine) wins over the one that pattern-matches it (the regex) — and the disagreement is the finding, not the
noise.
