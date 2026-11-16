# Deploy checklist: the 2026-11-16 batch

Four changes are in the repository and **not** in the served binary. They land together, so the window is one swap,
not four.

## What changes when the new binary runs

| change | wire | what a client sees |
|---|---|---|
| `create_file` mutation | additive, `schema_version` stays 29 | `tb_create_file` no longer races: two clients creating one path get one success and one `policy/absence` refusal |
| `scope` default `own` on `search` and `grep` | same fields, new default | a default query answers from the workspace, not from dependency repositories; `scope: "all"` restores the old behaviour |
| `weak` on every search match | additive field | a hit whose every matched term is common in the corpus says so |
| `rename_type` preview | additive previewable op | `tb_rename_type` refuses a cascade that would write members outside the grant |

Only the second is a behaviour change for an existing caller. `--tools full` clients see the new description text;
`--tools concise` clients see one line and lose the sentence about the default, so a `doctor` run is worth doing after.

## The swap

1. **Capture** the four golden reads against the running daemon:
   `tb_overview`, `tb_hubs`, `tb_graph_health`, `tb_search` (the last one is the one that will differ: `weak` is a
   new key, so expect an intentional diff there and nowhere else).
2. **Record the served version**: `2brain --version`, and the engine's own build id.
3. **Build** both halves in release, from a clean tree at the release commit.
4. **Stop, install, start** by `docs/internal/engine-swap-*.md` — one window, adapter and daemon together, because a
   new adapter against an old engine is the combination that produced the "declared but not served" divergence
   recorded in the vault.
5. **Re-capture** and diff: three of four reads identical, `tb_search` identical except the new `weak` key on each
   match.
6. **Verify the surface**: `tb_describe_tool tb_search` now declares `scope` default `own`; a default query for a
   term that occurs only in a dependency repo returns nothing; `tb_create_file` twice on one path refuses the second.
7. **Flip the vault notes**: `search-scope-defaults-to-the-own-workspace` and
   `search-weak-flag-is-a-signal-not-a-verdict` move from `proposed` to `accepted`, and
   `2brain-serving-state-lags-the-repository` gets a `superseded` status with the date. That is the rule the
   cross-agent run produced; skipping it is what put the vault ahead of production in the first place.

## Rollback

Keep the previous `au` and `2brain` binaries beside the new ones and the vault at its commit before the swap. A read
that differs unintentionally means the swap is wrong, not the capture: revert, then investigate.
