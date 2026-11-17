# One finding recorded twice: 7 duplicate pairs merged, measured 2026-11-17

**Question.** Repeated ingest passes over the same source can record one finding as two notes. Each
duplicate inflates corpus statistics and splits inbound links between two copies of the same claim.

**What was found.** Two passes over `ZeroClaw .planning/research/PITFALLS.md` and six other
documents — one on 2026-02-22 writing **Russian**, one on 2026-02-27 writing **English** — produced a
second note per finding. Seven pairs were confirmed by reading both, not by a score:

| finding | RU note (2026-02-22) | EN note (2026-02-27) |
|---|---|---|
| Zettelkasten retroactive evolution | `amem-zettelkasten-retroactive-evolution` | `a-mem-uses-zettelkasten-method-...` |
| entity resolution below 85% | `entity-resolution-below-85-percent-cascades-...` | `entity-resolution-accuracy-below-85-percent-...` |
| six security layers | `zeroclaw-defense-in-depth-six-layers` | `zeroclaw-defense-in-depth-six-independent-...` |
| Mnemosyne hybrid weights | `mnemosyne-hybrid-search-three-component` | `mnemosyne-hybrid-search-weights-fts5-20-percent-...` |
| Mnemosyne four agents | `mnemosyne-four-agent-orchestration-ractor` | `mnemosyne-four-agent-framework-orchestrator-...` |
| embedding drift | `embedding-drift-silently-degrades-...` | `embedding-drift-silent-retrieval-killer` |
| no forgetting mechanism | `no-forgetting-mechanism-causes-self-degradation-...` | `no-forgetting-mechanism-anti-pattern` |

**What was done.** For each pair the winner is the note with more inbound links, breaking ties on
body length. Three losers held a fact the winner lacked and that fact was carried over first:

- `0600` key-file permissions and `256-bit` bearer tokens (security layers);
- `715 ms` measured latency and the fusion-in-code detail (Mnemosyne hybrid);
- the top-5 neighbour lookup and `should_evolve` decision (Zettelkasten).

Then every wikilink to a loser was rewritten to the winner (29 files), and the seven loser files
were deleted. Vault commit is in the same change.

**Numbers.** Notes 1 047 → 1 040. Edges 4 895 → 4 856: the deletion removes each loser's own
outbound links, and the rewrite keeps every inbound one pointing at the survivor. Components 206,
isolated 197 → **196** (one loser was an island), no-inbound 324, diagnostics **0**.

**The tool, and the trap it fell into twice.** `tools/kb/duplicate_findings.py` (`--selftest`)
ranks pairs by token-set Jaccard over note bodies. Its first version reported **8 pairs that were
not duplicates**: a `claim` and the `insight` its `evidence:` field names restate one finding on
purpose — that is the vault's claim structure, not a defect — and they score 0.41–0.48, above the
0.40 line. The filter now excludes any pair where one note's `evidence:` cites the other.

The second bug was the filter reading the wrong field (the description rather than the file text),
which silently disabled it; the doc records that because the first "fixed" run still reported all
eight. A detector is only trustworthy after it stops flagging the cases a human has already
explained.

**What remains.** 69 candidate pairs at the default 0.30 threshold, none above 0.47, and the top of
the list is legitimate structure:

- `insight` / `claim` / `decision` triplets about one measurement (the concise tool surface) —
  three notes with three roles;
- `connect-2026-03-06-pre-upgrade` / `refresh-...` — two different skills, whose skill files share a
  template;
- `codeactinstruct-00N` — the queue items themselves, which are numbered copies by design.

**Next step, registered.** A `superseded` field on the type system would let a loser stay readable
instead of being deleted; the merge here was chosen because the vault is a git repository and the
deletion is one `git revert` away. If a second pass finds more pairs, the field is worth the
migration.
