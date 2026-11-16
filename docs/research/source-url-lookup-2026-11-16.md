# Source-identity lookup: a pasted URL finds the note that carries it

Measured 2026-11-16, engine lane. One read changed: `search`. The defect is row
`z08` of the 160-question set (`docs/research/question-set-baseline-2026-11-16.md`):
a query that is itself a URL — `https://arxiv.org/abs/2602.20021` — returned 109
documents ranked over the words inside the URL, and none of the 16 notes whose
`source_url:` frontmatter names exactly that address reached the page of ten. The
index holds body and description; it holds no URL field, so a URL query matched
prose that never contains the URL. The provenance-echo work hit the same gap from
the other side: the identity is recorded in the note and nothing served reads it
(`docs/research/lineage-echoes-2026-11-16.md`).

## The decision: a lookup, not a field

Two designs were on the table, and the acceptance criteria decided, not taste:

**`source_url` as a third indexed field** (`Bm25Index<2>` → `<3>`): touches
`default_weights`, `indexed_fields`, the `SearchIndex` type and about fifteen
typed test helpers in `search.rs` — roughly 45 changed lines. It also changes
every score in the corpus: URL text tokenizes into terms the prose vocabulary
already holds (`arxiv`, `org`, `abs`, id digits), so document frequencies shift,
idf shifts, and every note containing those tokens moves. That fails two
acceptance criteria by construction: the golden `tb_search` capture's scores
drift (the diff would be new scores, not a new field), and the 159 other rows
would be re-rolled on an unmeasurable perturbation. Cheaper in lines, unaffordable
in contract.

**The lookup** (shipped): when the query normalizes to a source identity, one
pass over the catalogue's `source_url` values answers before any ranking. The
scan is bounded by the catalogue the index was built from (1 032 documents on the
measured vault), compares normalized strings, reads no disk, and cannot touch a
single non-URL query — the ranking path is untouched, so every other row keeps
its rank by construction, not by hope. New code instead of changed code, measured with `git diff --numstat` plus the new module: `source_url.rs` 157 lines of code + 136 lines of normalization tests (10 of them), 88 insertions / 20 deletions in `serve.rs` (the merge, the `matched_source` / `MatchedSource` wire fields and their docs), 168 in the `it` suite (the fixture + 4 integration tests), 1 in `lib.rs`, 3 in `WIRE.md` — 260 insertions / 22 deletions across 4 files plus the module.

## The normalization (one test per rule, `src/source_url.rs`)

A source identity is the provenance key the echo work already groups notes by —
the same rules, so a lookup agrees with the grouping without a third definition:

1. trim surrounding whitespace.
2. scheme `http://` / `https://` required and dropped, case-insensitive on the
   paste (anything else — `ftp://`, a bare host, a path — names no source, and
   the query falls through to the ranking unchanged).
3. fragment dropped (`#...`).
4. query string dropped (`?...`): the parameters a paste carries are tracking,
   not identity. The earlier of fragment and query string ends the URL.
5. host lower-cased, one leading `www.` dropped.
6. trailing slashes dropped. Ports are NOT normalized (the provenance rules do
   not mention them); the path's case is kept (hosts fold, paths do not).
7. three known equivalences, host-scoped:
   - `arxiv.org/abs|pdf|html/<id>` → `arxiv:<id>` — the rule the provenance work
     uses; `.../abs/2602.20021` and `.../pdf/2602.20021v2` meet on `arxiv:<id>`.
   - `docs.google.com/document/d/<id>` → `gdoc:<id>` (deeper path ignored).
   - `github.com/<owner>/<repo>` → `gh:<owner>/<repo>`, case-folded, deeper path
     (`/tree/main`) ignored; one segment names a user, not a repo, and
     collapses to nothing.
8. otherwise the identity is `host/path`; a bare host is its own identity (the
   provenance pass counts a bare site root as a source).

Both sides normalize identically — the pasted query and the note's authored
`source_url` — which is the whole trick: `https://arxiv.org/pdf/2602.20021v2` in
a note and `http://www.arxiv.org/abs/2602.20021v2#conclusion` pasted by a caller
are one identity.

## The wire shape (additive, no `schema_version` bump)

`search.matches[].matched_source` — present ONLY on a hit found by source
identity, absent on every ranked match, so a caller tells an exact lookup from a
scored guess without comparing paths. It carries `source_url` (the note's value
as authored) and `identity` (what both URLs normalized to). A lookup hit carries
no ranking answer: `score` 0.0, `matched_terms` 0, `weak` false — the no-match
values, so no existing field changes meaning. The ranking still runs over the
whole in-scope set and follows the looked-up hits with the paths the lookup named
removed, so a note appears once; `total` counts lookup plus ranked; paging
crosses the boundary on the unchanged `offset` / `next_offset` rules. The
lookup's candidates pass the same `repo` / `scope` predicate the ranking's mask
uses, so a scoped read cannot return an out-of-scope note by identity.
`exact: true` does not suppress the lookup — it is a ranking argument, and the
lookup is not a ranking. A query that is not a URL, or a URL no `source_url`
holds, answers exactly the ranking it would have answered before: an unknown URL
cannot come back worse than the unranked scan. `WIRE.md` carries the same note.

## Measured: `z08`, before and after

GOLDEN: `scripts/engine_probe.py --verb tb_search` captured against the HEAD daemon and against this change's daemon, each serving its own `cp -a` of the same frozen working tree, normalize to byte-identical files (17 115 bytes both, `diff` clean). The capture test (`the_search_capture_matches_the_engine`) passes without re-blessing. 

METHOD: `cp -a` copy of the working vault at `/tmp/url-lookup/vault`, its own
`HOME` and socket (`/tmp/url-lookup/home`), the same installed adapter, the same
runner: `tools/ranking/run_questions.py --scope own --limit 10`. The daemon was
rebuilt from the same tree between the two runs; the only delta is this change.

| | before | after |
|---|---|---|
| `z08` query total | 109 | 125 (16 by identity + 109 ranked; the 16 were never in the ranking — the URL appears only in frontmatter, never as prose) |
| `z08` page head | `inbox/folio-context-folding.md`, `knowledge/xml-jsonl-vs-json-ld-2026.md`, ... | `knowledge/agents-of-chaos-circular-verification`, `knowledge/agents-of-chaos-disproportionate-response`, `knowledge/agents-of-chaos-emergent-safety-coordination`, ... — the lookup block leads; every one of the 16 carries `matched_source: { identity: "arxiv:2602.20021", source_url: ... }` |
| `z08` expect in page | no (rank null) | **yes, rank 4 (`knowledge/agents-of-chaos-forwarding-reframes-bypass.md`, the row's first `expect`; its sibling `guilt-trip-attack` sits at rank 5)** |

The full 160-row diff (`before.json` / `after.json`): exactly ONE row moved — `z08` (rank null → 4, total 109 → 125). The other 159 rows are identical in rank, total, and full page: no score moved anywhere, because no non-URL query reaches the lookup. Class summary: silly hit@5 12 → 13, "not on the page" 22 → 21, hit@1 unchanged at 75, MRR 0.636 → 0.638 — the whole delta is this row.

## Coverage limits

- A URL in the BODY but not in `source_url:` finds nothing by identity — the
  lookup reads the field the corpus already declares, not prose. `grep` remains
  the literal tool for body text.
- A note with no `source_url` cannot be found by one — the field is the
  contract. On the measured copy: 1 066 markdown files, 884 carry a
  `source_url:` field, 764 of those name an http(s) URL, and no identity is
  invented for the rest.
- The whole query string is dropped, so the 4 vault URLs whose query string
  carries real identity (three YouTube `watch?v=<id>` plus one more) collapse
  into one per-host identity: a pasted video URL returns the site's notes as a
  family, not the one video. The provenance rules drop tracking parameters
  wholesale; the corpus measured has exactly this cost, and per-site parameter
  rules are where a future fix would live, not here.
- The `source_url` extraction covers top-level frontmatter of typed instances
  and frontmatter notes — the two shapes the corpus uses. A nested record's
  `source_url` field is not an identity.
- Unicode URLs: the host folds ASCII-case; percent-encoding is not decoded. The
  corpus's URLs are ASCII and unencoded; a paste that differs by encoding falls
  through to the ranking, unchanged from today.
- Scope: `repo` / `scope` filtering of identity hits is wired through the shared
  predicate but has no dedicated integration test — the vault run exercises
  `repo: vault, scope: own`, and the predicate is the one `grep` and the ranking
  already answer through.
- The golden `tb_search` capture is unchanged (the capture test is green); the
  corpus it pins holds no URL query, and none of this code is on its path.

## Pre-existing test failures seen on this host, not caused by this change

Reproduced on a clean `HEAD` worktree without the diff:
`engine::tests::an_unreadable_file_rebuilds_once_then_settles` (the fixture
requires a file root cannot read — the test's own panic says "skipped as
root?"), `delete_dir::a_delete_failing_mid_way_leaves_the_folder_and_its_skeleton_untouched`,
`move_dir::a_move_failing_mid_way_leaves_both_trees_and_the_skeleton_untouched`,
`mutate::cross_repo_rename_failure_compensates_every_repo`. Environment-shaped,
owned by whoever works the mutation lane next; they fail identically with and
without this change.

Gates run with this change: `cargo fmt --check` clean (engine), `cargo clippy
--all-targets --all-features -- -D warnings` clean, `cargo test -p tb-engine`:
740 lib green (1 pre-existing root-environment failure above), 734 `it` green
(3 pre-existing failures above, byte-identical to a clean-HEAD worktree run).
