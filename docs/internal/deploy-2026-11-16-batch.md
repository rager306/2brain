# Deploy checklist: the 2026-11-16 batch

Six changes are in the repository and **not** in the served binary. They land together, so the window is one swap,
not six.

## What changes when the new binary runs

| change | wire | what a client sees |
|---|---|---|
| `create_file` mutation | additive, `schema_version` stays 29 | `tb_create_file` no longer races: two clients creating one path get one success and one `policy/absence` refusal |
| `scope` default `own` on `search` and `grep` | same fields, new default | a default query answers from the workspace, not from dependency repositories; `scope: "all"` restores the old behaviour |
| `weak` on every search match | additive field | a hit whose every matched term is common in the corpus says so |
| `rename_type` preview | additive previewable op | `tb_rename_type` refuses a cascade that would write members outside the grant |
| `excerpt` on every search match | additive field, `excerpt_chars` per page | a hit can be judged without `tb_content`; the context pack drops from 4.00 calls / 8.86 KB to 1.00 call / 2.44 KB per answer |
| a word form of an unknown query term is looked up | none — a query resolves differently | `capsules` finds `capsule`: plural mismatch stops costing a whole query |

The second is a behaviour change for an existing caller, and the last is a ranking change under a query that
previously returned nothing, so neither is invisible. `--tools full` clients see the new description text;
`--tools concise` clients see one line and lose the sentence about the default, so a `doctor` run is worth doing after.

Commits behind the batch: `1b601b0` (create_file), `b37c156` (scope), `b59f595` (weak), `6c080ad` (rename preview),
`25ce096` (excerpt), `c3c7ee4` (word form). The golden `tb_search` capture in `tests-golden/engine/tb_search.json`
was re-blessed twice — once for `weak`, once for `excerpt`/`excerpt_chars` — so it must be regenerated from the
release commit, not carried over.

## The swap

1. **Capture** the golden reads against the running daemon: `tb_overview`, `tb_hubs`, `tb_graph_health`, `tb_search`.
   The served binary predates `weak` and `search` excerpting, so `tb_search` is expected to differ in **three** keys —
   `weak` per match, `excerpt` per match, `excerpt_chars` per page — and in nothing else. A difference in a score, a
   path, `total` or paging is a failed capture, not an intentional diff.
2. **Record the served version**: `2brain --version`, and the engine's own build id.
3. **Build** both halves in release, from a clean tree at the release commit.
4. **Stop, install, start** by `docs/internal/engine-swap-*.md` — one window, adapter and daemon together, because a
   new adapter against an old engine is the combination that produced the "declared but not served" divergence
   recorded in the vault, and an old adapter against a new engine silently drops `weak` and `excerpt` from anything
   it rebuilds.
5. **Re-capture** and diff: three of four reads identical, `tb_search` identical except the three keys above.
6. **Verify the surface**, one check per row of the table:
   - `tb_describe_tool tb_search` declares `scope` default `own` and describes `excerpt`;
   - a default query for a term that occurs only in a dependency repo returns nothing;
   - `tb_create_file` twice on one path refuses the second with `policy/absence`;
   - `tb_search "capsules"` on a workspace holding `capsule` returns the note (the word-form lookup);
   - one `tb_search` match carries an `excerpt` whose text is in the note's body, not its frontmatter.
7. **Flip the vault notes**: `search-scope-defaults-to-the-own-workspace` and
   `search-weak-flag-is-a-signal-not-a-verdict` move from `proposed` to `accepted`, and
   `2brain-serving-state-lags-the-repository` gets a `superseded` status with the date. That is the rule the
   cross-agent run produced; skipping it is what put the vault ahead of production in the first place.

## Rollback

Keep the previous `au` and `2brain` binaries beside the new ones and the vault at its commit before the swap. A read
that differs unintentionally means the swap is wrong, not the capture: revert, then investigate.

The batch touches search behaviour twice (the scope default and the word form), so the rollback test is a query that
answers differently: `tb_search "capsules"` must return nothing under the old binary and the note under the new one.
