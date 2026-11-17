# The alias class: measured 2026-11-17, not adopted

**Question.** The task set's open lane C4 asks one thing: how large is the class of terms that
name the same concept, so that a query on one surface cannot reach the notes that state the other.
The answer decides whether the engine grows an alias mechanism at all.

**Method.** `tools/kb/alias_audit.py` reads every note and pairs an acronym with a multi-word
expansion whose initials spell it, in the same note. Two surfaces are the same concept when the
corpus itself puts them together; the tool does not claim more. For each pair it reports the note
sets and their divergence, `1 - |acronym ∩ expansion| / |acronym ∪ expansion|`: a pair whose
surfaces reach the same notes costs nothing, and a pair that reaches different notes is the class
a search can miss.

```sh
python3 tools/kb/alias_audit.py --selftest        # the tool's own fixture
python3 tools/kb/alias_audit.py --top 15          # the vault at /root/vault
```

**Numbers (1 064 notes, 2026-11-17).**

| measure | value |
|---|---|
| distinct acronym-shaped tokens | 2 943 |
| of those, in two or more notes | 1 222 |
| pairs with a same-note expansion | 285 |
| pairs with divergence ≥ 0.5 | 272 |

The ranking is by how many notes state the expansion, so the top of the list is the class worth
reading, not the class that is real. Of the top fifteen, nine read as real pairs:

| acronym | expansion | acronym notes | expansion notes | shared | divergence |
|---|---|---|---|---|---|
| `CC` | claude code | 9 | 90 | 2 | 0.98 |
| `KG` | knowledge graph | 20 | 54 | 4 | 0.94 |
| `AS` | ai systems | 8 | 45 | 2 | 0.96 |
| `US` | user stories | 12 | 39 | 1 | 0.98 |
| `ER` | entity resolution | 13 | 32 | 12 | 0.64 |
| `MT` | multi turn | 4 | 31 | 1 | 0.97 |
| `SoT` | source of truth | 15 | 24 | 4 | 0.89 |
| `IO` | i o | 10 | 24 | 4 | 0.87 |
| `RSI` | recursive self improvement | 37 | 23 | 13 | 0.72 |

The other six are initial collisions: `TE` → "type extract", `ES` → "extract status", `GC` →
"github com", `IT` → "is the", `PR` → "provenance readme", `ID` → "in depth". The tool names
candidates; only a reader decides.

**What this means.** The class is real — a query on `SoT` reaches 15 notes while 20 notes state
only "source of truth", and the two sets share four — but the automatic pairing is at best
six-in-ten precise on the frequency-ranked head, and the full list would need a human pass over
285 rows. That is a content task, not an engine one, and it is the reason no alias mechanism is
adopted now:

- **no `aliases:` field.** A third indexed field changes `matched`, the `min_should_match` floor
  and the candidate set; the same trap that closed the title-as-field route (`docs/research/task-set.md` §4).
- **no query expansion yet.** Expanding a query before search is the cheap, reversible shape, but it
  changes the candidate set for the expanded query, so it needs its own task and its own gate rather
  than a patch to the ranker.

**Next step, registered.** Add one task to the task set: the query `SoT` must reach the notes that
state only "source of truth" (`event-log-as-source-of-truth-agent-memory` among them). Until that
task exists and fails, the alias class is a measured model, not a defect. If it fails, the fix is a
build-time alias map applied at query time, with the map's rows taken from this tool's candidate
list after a human pass.

**Not measured here.** Russian inflection: the paraphrase task `агент не вызывает доступные
скиллы` returns nothing at all while `скиллы` alone returns 57 notes, so the missing surface is a
morphological ending, not an acronym. That is the larger class, and it is a separate decision
(a stemmer or a prefix match changes the index, and the index is where the floor lives).
