# Candidate-set first: what a scoped read pays for the notes it discards

Entry 1 of [[ruvector-adr-2026-11-16]] asks 2brain to restrict a read's candidate set before
scoring, and cites ruvector's ADR-268 for the ordering: 57 µs against 450 µs at 12.5 %
selectivity, same answers. Our own `search.rs` stated the defect plainly — "the whole answer
comes back because scope (`repo` / `scope`) is a query-side filter this index knows nothing
about: the caller filters, then totals and pages over what it kept" — but nothing measured
what that costs here.

This note is that measurement, the change it justified, and the evidence that the answers did
not move. Both halves are in the tree: `tools/probe/candidate_set.py` measures, and
`search.rs` / `serve.rs` now hand the ranking a candidate mask instead of filtering its
result.

**Verdict: ship it — but not for the reason the entry gives.** On the workspace this host
actually serves, restricting the candidate set removes **0.09 %** of the ranking's work: the
vault is 1 215 of 1 237 catalogued files, so there is almost nothing out of scope to skip. The
mask pays anyway, and pays well, because it is the only way to delete the post-ranking filter,
whose cost is not a scoring cost at all: it resolves the owning repo **per hit**, at 0.5–1.8 µs
a hit, which is 9 % of a scoped read at three members and 28 % at nine. The entry's own
mechanism then adds a real 8.7 % cut in scored documents once a workspace has several mounted
members. Details and the crossings are in [The verdict](#the-verdict).

## What was measured, and on what

Two corpora, both frozen copies of the served vault (`cp -a`; the live vault is only read).
The snapshot is 2026-11-16, 1 066 notes, and it matters that it is frozen: the vault was being
written by another agent during this work, and the first before/after comparison was invalid
until the corpus was pinned.

| corpus | members | catalogued files | in scope at `own` | notes |
|---|---|---:|---:|---|
| **A — as served** | 3: `vault` (entry), `mcp-bundle`, `au-mcp-sdk` | 1 237 | 1 215 (**98.2 %**) | the real workspace on this host |
| **B — widened** | 9: A plus `au-type-system`, `au-agent-guides`, `au-weave`, `au-skills`, `au-writing-style`, `au-base-types` | 1 469 | 1 215 (**82.7 %**) | **a model**, not a served workspace: A's frozen copy with six more `deps:` in its `repo.yaml` |

The brief for this work said the vault has seven members. It has three: `au members` resolves
`vault → mcp-bundle → au-mcp-sdk` and stops. Corpus B exists because that number is the one the
entry's mechanism depends on, and a decision that rests on "several mounted members" should say
what happens at several. B is labelled a model wherever it appears, and nothing below cites it
as shipped behaviour.

Twenty queries — `visit_bound.py`'s fixed set, so both probes answer over one query set — at
`scope: own` and `scope: all`. `own` is not a contrived arm: it is `search`'s own default.

## The probe

`tools/probe/candidate_set.py`, same style and same helpers as `visit_bound.py` (it imports
them; there is one probe style, not two).

```sh
python3 tools/probe/candidate_set.py                          # the table, on stdout
python3 tools/probe/candidate_set.py --against /tmp/au-old     # paired: two binaries, one corpus
```

Per query and scope it records:

- **`documents_visited`** and **`posting_blocks_decoded`** for the `search` verb, from
  `graph_health { verbs: true }` sampled either side of one request — the engine's own count of
  what the request walked, not an estimate;
- the answer's **`total`**;
- **wall time**, client side, as the median and minimum of N rounds, with the two scopes (or the
  two binaries) alternating inside each round;
- with `--capture DIR`, **every answered page** of every request, paged to the end and
  normalized (`$VAULT` for the workspace root, sorted keys), which is what makes "the answers
  are identical" a diff rather than an opinion.

Safety, by construction: the live vault is only ever `cp -a`-ed; each engine runs on a copy
under `--work` with its own `HOME` and a registry whose `vault` entry is re-pointed at the copy;
no mutation verb is sent; only the daemons the probe started are stopped, through their own
socket. `--work` may not overlap the live vault.

Timing on this host is not quiet: other agents run builds and engines on the same 12 cores. The
paired mode exists for that — two engines on identical copies answering in alternating order —
and the `all` scope is a control whose code path did not change, so it measures the residual
noise of the method. It reads ±4 %.

## Before: the hole, in numbers

The same 20 queries, one binary, both scopes. Documents visited are **identical** at both
scopes, which is the defect stated as a number: the read does the same work whether it may
return 2 521 documents or 2 514.

| corpus | documents visited `own` | documents visited `all` | answers `own` / `all` | out of scope |
|---|---:|---:|---:|---:|
| A | 8 048 | 8 048 (1.00×) | 2 514 / 2 521 | 7 hits, **0.3 %** |
| B | 9 468 | 9 468 (1.00×) | 2 649 / 3 108 | 459 hits, **14.8 %** |

The second defect is not in the entry, and it is the bigger one. `search_view` filtered the
finished ranking with `in_read_scope`, which resolves a hit's owning repo — an ancestor walk
over the repo index, per hit. Its cost shows up as the `own` − `all` gap inside one binary, for
the *same* ranking work:

| corpus | `own` − `all` gap, old binary | gap after the change | closed = the filter's cost | hits filtered | per hit |
|---|---:|---:|---:|---:|---:|
| A | 4 998 µs | 3 691 µs | **1 307 µs** | 2 521 | 0.5 µs |
| B | 9 557 µs | 3 963 µs | **5 594 µs** | 3 108 | 1.8 µs |

So a scoped read spent 9 % (A) and 28 % (B) of its wall time deciding which of the notes it had
just ranked it was allowed to return.

The third column is why the cost is read as the gap's CLOSING rather than its size. The new
binary's gap is not zero even though `own` now does strictly less work than `all`, so part of
any `own` − `all` gap is an artifact of the probe: inside a round, `own` always precedes `all`
for the same engine, and the second request reuses a warmed allocator. That artifact is the
same in both binaries, so differencing removes it; the two closings (1 307 and 5 594 µs) also
match the measured `own` improvements (1 131 and 5 941 µs) within this host's noise.

## The change

`search.rs` — a candidate mask the scoring loop consults:

- `DocMask<'a>`: `None` means every document (an unscoped read), `Some(&[bool])` indexes one
  flag per document id. One branch per posting, and a posting the mask rejects is neither
  scored nor accumulated.
- `Bm25Index::search` / `exact` take the mask; the posting walk counts the postings it
  **admits** (`admitted += 1` per scored posting, one integer add) and the adjacency pass skips
  the per-document position decode for a document it cannot answer. `exact` filters while it
  decodes each term's block.
- **The corpus statistics are untouched.** A term's idf still comes from the posting list's
  whole length, and the average field lengths still come from every live document. That is the
  proof that the mask cannot move a score, and it is why the answer set is identical rather
  than merely similar.
- `SearchIndex::candidate_mask(verdicts)`: paints one flag per document id from the repo roots,
  in root-depth order, so a deeper root's verdict replaces a shallower one's — the same rule
  `RepoMap::repo_of` applies. One predicate call per **repo** instead of one per **document**,
  which is what makes the mask affordable: the predicate's own cost is the ancestor walk.
- Two tests: a masked ranking equals the full ranking filtered afterwards, score for score
  (`a_candidate_mask_answers_what_the_filter_left_of_the_full_ranking`), and the mask equals
  the per-document predicate over nested roots, a shared root, and an unowned path
  (`a_candidate_mask_is_the_per_document_filter_over_the_paths`).

`serve.rs` — `search_view` builds the mask when a filter is engaged (`scope_engaged`, one
spelling of that condition, which `in_read_scope` now also uses) and **deletes the
post-ranking filter**. The predicate is the same function, asked once per repo; there is no
second implementation of "in scope".

Size, from `git diff --numstat`: 151 added lines in `search.rs` (about 50 the `DocMask` type and
its rationale, 40 the mask builder, the rest the two walks, the call sites and comments) and 49
in `serve.rs`. The test module's 167 added lines are mostly rustfmt re-wrapping existing call
sites to take the mask, plus the two new tests. No persisted state, no new field on the index,
no schema change.

**Why the mask cannot go stale**, which is the risk the entry names ("a stale bitmap after a
rename or a move makes a note silently unsearchable"): it is not stored. It is derived per
request from `kb.repos` and the index's own path table, both of which the request already
holds, so it cannot outlive the state it describes. The cost of deriving it is a `Vec<bool>` of
one flag per indexed document plus one binary search per repo — about a thousand flags and 3 or
9 searches here — and it does not surface in the comparison below: the `all` control, which
builds no mask, moves by less than the effect being measured.

## After: the same 20 queries

Paired mode, frozen corpus, `before` = the binary at `HEAD`, `after` = this change. Times are
client-side wall time for a one-match page; the ranking work does not depend on the page size.
Per-query numbers on this host are noisy (p25–p75 spans of ±50 % are common), so the aggregate
is the claim and the control is the error bar.

### Corpus A — the workspace as served (3 members, 98.2 % own)

| query | visits `own` before → after | visits `all` | µs `own` before → after | µs `all` before → after | answers `own` / `all` |
|---|---:|---:|---:|---:|---:|
| `wikilink` | 3 → 3 | 3 | 548 → 563 | 562 → 715 | 3 / 3 |
| `mutation` | 18 → 18 | 18 | 709 → 639 | 484 → 410 | 18 / 18 |
| `daemon` | 46 → 45 | 46 | 687 → 603 | 418 → 426 | 45 / 46 |
| `page` | 34 → 34 | 34 | 672 → 808 | 489 → 412 | 34 / 34 |
| `note` | 52 → 51 | 52 | 378 → 330 | 284 → 238 | 51 / 52 |
| `engine` | 114 → 113 | 114 | 527 → 541 | 300 → 304 | 113 / 114 |
| `index` | 145 → 145 | 145 | 574 → 545 | 433 → 611 | 145 / 145 |
| `decision` | 197 → 197 | 197 | 846 → 716 | 419 → 398 | 197 / 197 |
| `graph` | 296 → 296 | 296 | 920 → 612 | 411 → 388 | 296 / 296 |
| `agent` | 591 → 590 | 591 | 1 240 → 873 | 373 → 619 | 590 / 591 |
| `daemon status` | 353 → 352 | 353 | 819 → 658 | 386 → 499 | 179 / 180 |
| `bm25 ranking` | 131 → 131 | 131 | 838 → 812 | 698 → 545 | 65 / 65 |
| `search index` | 633 → 633 | 633 | 703 → 591 | 530 → 518 | 64 / 64 |
| `pattern decision` | 864 → 864 | 864 | 753 → 602 | 710 → 416 | 75 / 75 |
| `error fix` | 270 → 270 | 270 | 1 036 → 1 248 | 800 → 1 099 | 129 / 129 |
| `graph type` | 770 → 769 | 770 | 878 → 741 | 556 → 428 | 113 / 114 |
| `search index cache` | 1 164 → 1 164 | 1 164 | 580 → 488 | 308 → 339 | 138 / 138 |
| `knowledge graph type` | 1 592 → 1 591 | 1 592 | 778 → 777 | 712 → 455 | 182 / 183 |
| `error->fix exact` | 142 → 142 | 142 | 620 → 663 | 446 → 450 | 13 / 13 |
| `search index AND` | 633 → 633 | 633 | 636 → 799 | 425 → 649 | 64 / 64 |

| | before | after | ratio |
|---|---:|---:|---:|
| documents visited at `own` | 8 048 | 8 041 | **0.999×** |
| documents visited at `all` | 8 048 | 8 048 | 1.000× |
| wall time at `own`, sum of medians | 14 741 µs | 13 610 µs | **0.923×** |
| wall time at `own`, sum of minima | 7 812 µs | 6 284 µs | **0.804×** |
| wall time at `all`, sum of medians (control) | 9 743 µs | 9 919 µs | 1.018× |
| wall time at `all`, sum of minima (control) | 5 115 µs | 5 316 µs | 1.039× |
| answer totals at `own` / `all` | 2 514 / 2 521 | 2 514 / 2 521 | identical |

Paired per query: the median of the `after/before` ratio is **0.890** at `own` (14 of 20
queries faster, 16 of 20 by the minimum) and **0.994** at `all` (10 of 20 — the control says
"unchanged").

### Corpus B — the 9-member model (82.7 % own)

| query | visits `own` before → after | visits `all` | µs `own` before → after | µs `all` before → after | answers `own` / `all` |
|---|---:|---:|---:|---:|---:|
| `wikilink` | 20 → 3 | 20 | 625 → 679 | 487 → 547 | 3 / 20 |
| `mutation` | 30 → 18 | 30 | 455 → 484 | 273 → 333 | 18 / 30 |
| `daemon` | 53 → 45 | 53 | 437 → 317 | 291 → 234 | 45 / 53 |
| `page` | 38 → 34 | 38 | 462 → 542 | 258 → 343 | 34 / 38 |
| `note` | 79 → 51 | 79 | 784 → 761 | 530 → 544 | 51 / 79 |
| `engine` | 175 → 113 | 175 | 897 → 788 | 350 → 488 | 113 / 175 |
| `index` | 155 → 145 | 155 | 919 → 706 | 404 → 365 | 145 / 155 |
| `decision` | 215 → 197 | 215 | 1 345 → 710 | 532 → 410 | 197 / 215 |
| `graph` | 352 → 296 | 352 | 1 066 → 514 | 340 → 448 | 296 / 352 |
| `agent` | 628 → 590 | 628 | 2 193 → 737 | 681 → 507 | 590 / 628 |
| `daemon status` | 383 → 367 | 383 | 1 216 → 839 | 923 → 602 | 179 / 195 |
| `bm25 ranking` | 133 → 132 | 133 | 1 019 → 656 | 458 → 414 | 65 / 66 |
| `search index` | 671 → 652 | 671 | 1 016 → 974 | 607 → 753 | 145 / 155 |
| `pattern decision` | 915 → 889 | 915 | 899 → 581 | 422 → 465 | 75 / 76 |
| `error fix` | 370 → 319 | 370 | 1 530 → 946 | 1 053 → 1 047 | 129 / 178 |
| `graph type` | 1 055 → 889 | 1 055 | 1 104 → 728 | 638 → 547 | 50 / 95 |
| `search index cache` | 1 245 → 1 217 | 1 245 | 1 101 → 621 | 547 → 386 | 215 / 231 |
| `knowledge graph type` | 2 087 → 1 893 | 2 087 | 1 245 → 727 | 462 → 466 | 141 / 197 |
| `error->fix exact` | 193 → 142 | 193 | 786 → 956 | 635 → 604 | 13 / 15 |
| `search index AND` | 671 → 652 | 671 | 653 → 547 | 302 → 345 | 145 / 155 |

| | before | after | ratio |
|---|---:|---:|---:|
| documents visited at `own` | 9 468 | 8 644 | **0.913×** |
| documents visited at `all` | 9 468 | 9 468 | 1.000× |
| wall time at `own`, sum of medians | 19 751 µs | 13 810 µs | **0.699×** |
| wall time at `own`, sum of minima | 11 554 µs | 7 203 µs | **0.623×** |
| wall time at `all`, sum of medians (control) | 10 194 µs | 9 847 µs | 0.966× |
| wall time at `all`, sum of minima (control) | 5 620 µs | 5 180 µs | 0.922× |
| answer totals at `own` / `all` | 2 649 / 3 108 | 2 649 / 3 108 | identical |

Paired per query: median ratio **0.707** at `own` (16 of 20 faster, 17 of 20 by the minimum),
**1.000** at `all`.

Note what did *not* fall: `posting_blocks_decoded` is 58 per query set in every arm. The mask
stops a document being scored; it does not stop the posting block being decoded, because the
document ids are delta-encoded inside it and the only way to learn which documents it holds is
to walk it. So the visit count falls by the admitted share and the decode count does not, and
the wall-clock saving is what is left after that.

## Equivalence evidence

This is a performance change, so a payload difference would be a defect. The evidence, in order
of strength:

1. **Every answered page, both binaries, byte for byte.** `--capture` pages each of the 20
   queries at both scopes to the end (`limit: 100`, following `next_offset`), plus two
   `repo`-pinned requests, and normalizes paths and key order. 42 files per binary per corpus.
   The two directories compare equal in every run: `diff -r` on the sequential pair against the
   frozen corpus, and the probe's own byte comparison (which exits non-zero when it fails) in
   both paired runs, corpus A and corpus B. The captured answers carry `total`, every match's
   `path`, `score` (an f64, as text), `matched_terms`, `weak` and `excerpt`, so identical
   captures are identical scores and identical order, not just identical counts.
2. **Answer totals per scope**, in both tables above: unchanged in every arm.
3. **Every OTHER read verb that filters, swept the same way.** The change is confined to
   `search`, so the rest must be untouched, and they are: against the same frozen corpus, one
   daemon per binary, 15 requests returned byte-identical payloads — `grep`, `files`,
   `top_level_dirs`, `hubs`, `diagnostics` and `type_counts` at both scopes, plus `members`,
   `references_in` and `neighborhood`. `engine_probe.py`'s own five reads (`overview`, `hubs`,
   `diagnostics`, `graph_health`, `search`) are identical too.
4. **`--test it golden_captures`**: 3 tests, green, and no file under `tb-engine/tests-golden/`
   changed in this work (`git status`), so the captured wire answers are still the ones the
   contract holds.
5. **Two unit tests** state the property where it can be re-run cheaply: a masked ranking
   equals the full ranking filtered afterwards (`Hit`s compared whole, score included), and a
   masked conjunction equals the filtered conjunction.
6. **The reason it must hold**: every corpus-wide quantity a score reads (per-term document
   frequency, per-field average length) is computed over the whole index in both arms, and a
   document's score depends on nothing else. The mask only decides which documents get scored.

Gates: engine lib 730 passed (the one failure is the known root-only
`an_unreadable_file_rebuilds_once_then_settles`), `--test it golden_captures` green, clippy
`--workspace --all-targets --all-features -D warnings` clean, `cargo fmt --check` clean.

The whole `it` suite is 729 passed, 4 failed. Three are the known root-only permission
fixtures (`delete_dir`, `move_dir`, the cross-repo rename compensation). The fourth is a
PRE-EXISTING failure this work did not cause and did not fix:
`search_read::three_distinct_terms_need_two_of_them_by_default` asserts `total: 1` for
`zeta eta theta`, but `theta` is in one document of that fixture, so its idf is 2.30 and the
rarity escape that commit `006ac0b` added admits `only-theta.md` — exactly as the escape's own
rule says it must. Run against `HEAD` with this change stashed, the payload is byte-identical
(the same two matches, the same scores). The fix belongs to that rule's lane, not here: the
test's premise — "the default floor is two of three" — now needs a term that is not rare.

## The verdict

**The mask ships. The entry's number is real but it is not the number that pays here.**

Attribution, from the tables above:

| what the change removes | corpus A (3 members) | corpus B (9 members) |
|---|---:|---:|
| scoring of out-of-scope documents | 7 of 8 048 visits (**0.09 %**) | 824 of 9 468 visits (**8.7 %**) |
| the post-ranking filter's per-hit repo resolution | ~1.3 ms of 14.7 ms (**9 %**) | ~5.6 ms of 19.8 ms (**28 %**) |
| measured result at `own` | 0.92× median, 0.80× minimum | 0.70× median, 0.62× minimum |
| control at `all` | 1.02× | 0.97× |

Three consequences, in the order a reader should weigh them:

1. **On this host, restricting the candidate set would not pay for itself.** Against a
   workspace where 98.2 % of the catalogued files are the caller's own, the mask skips 7
   postings in 8 048. Judged on the entry's own metric — the visits ratio — the honest answer
   is no: there is nothing there to win.
2. **The change still pays, because it deletes work that should never have existed.** The
   post-ranking filter resolved a hit's owning repo per hit, 0.5–1.8 µs each, and no mask can be
   avoided while that filter exists: the scope has to be applied somewhere, and applying it to
   the ranking instead of to its output is the only way to stop paying per hit. That is 9 % of
   a scoped read at three members and 28 % at nine, and it is the same predicate either way.
3. **The entry's mechanism starts to matter at several members.** With 14.8 % of the answer out
   of scope (B), the mask also removes 8.7 % of the scored postings, and the two effects
   compound: 0.70× median at `own` with the control at 1.00×. This is the workspace the entry
   describes, and it is not the one this host serves.

The crossing point, stated so the next reader need not re-measure it: the mask is worth
building the moment a workspace has **one** dependency member, because the per-hit filter it
replaces is paid on every hit regardless of selectivity; the candidate restriction itself needs
a dependency share of roughly a fifth of the answer (B: 14.8 % of hits, 8.7 % of postings)
before it is visible against that.

What would have made this a "no": if the post-ranking filter had been cheap (a `bool` per
document rather than a repo resolution per hit), corpus A would have shown 0.09 % of work saved
and no wall-clock effect, and the answer would have been "not worth 150 lines". The measurement
that decided it was not the one the entry proposed.

### The risk that remains

- **A false negative is the failure mode, and it is invisible to ranking metrics.** The mask and
  `in_read_scope` are one predicate, but the mask is applied per repo root rather than per path.
  The ordering precondition (root depth, then name — `RepoMap::repo_of`'s own rule) is a
  `debug_assert` plus a unit test, and the capture diff is what proves it on the real corpus.
  `grep` and `files` still call `in_read_scope` once per catalogued path, which is affordable
  because both walk the catalogue anyway; a fourth facet (`type:`, `status:`, `freshness:`)
  that copies the mask's shape must come with its own capture diff, for the same reason.
- **A facet is not free.** The mask is a document-id set, and today the only thing that can
  produce one cheaply is repo membership, because repo roots are path prefixes and document ids
  are path-ordered. `type:` or `status:` would need the index to carry that attribute per
  document; without it, the same mask would cost one catalogue lookup per document, which is the
  per-document cost this change exists to remove.
- **The counters' meaning moved for scoped reads**, so `documents_visited` for a `search` is now
  "postings the scope admitted" rather than "postings decoded". That is documented in `WIRE.md`,
  and `posting_blocks_decoded` is the figure that still reports the decode.
- **Nothing was measured on a cold cache**, on a corpus larger than ~1 500 files, or under
  concurrent writes; all three are larger effects than the one measured here.

## Reproduce

```sh
cd /root/2brain
# the engine binary the probe starts; `--binary` overrides it
(cd tb-engine && RUSTUP_HOME=/root/.rustup cargo +1.99.0 build --release -p tb-cli --offline)

# one binary, both scopes: the table above, frozen-copy safe
python3 tools/probe/candidate_set.py --live /tmp/frozen/vault --work /tmp/candidate-set \
        --json /tmp/rows.json --capture /tmp/capture

# before/after: two binaries, two identical copies of one frozen corpus, alternating
git -C /root/2brain stash push -- tb-engine/crates/tb-engine/src/search.rs \
                                   tb-engine/crates/tb-engine/src/serve.rs
(cd tb-engine && RUSTUP_HOME=/root/.rustup cargo +1.99.0 build --release -p tb-cli --offline)
cp .cargo-target/release/au /tmp/au-old
git -C /root/2brain stash pop
(cd tb-engine && RUSTUP_HOME=/root/.rustup cargo +1.99.0 build --release -p tb-cli --offline)
cp .cargo-target/release/au /tmp/au-new
python3 tools/probe/candidate_set.py --against /tmp/au-old --binary /tmp/au-new \
        --live /tmp/frozen/vault --work /tmp/pair-a --rounds 15
```

The probe prints the equivalence verdict itself (`The two capture directories are
byte-identical: the answers are the same.`) and exits non-zero when they differ, so a CI step
can hold it.
