# Where the knowledge base actually grows, measured

`tools/kb/census.py` on the live vault, 2026-11-16:

| measure | value |
|---|---|
| notes | 1024 |
| of them in `inbox/` | 557 |
| typed | 995 (97%) |
| described | 1011 (98%) |
| sourced | 888 (86%) |
| links | 3112, median 1 per note |
| notes with no outbound link | 424 (416 of them described) |
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
