# Engine swap — 2026-11-14 13:07 +07

The production daemon at `/root/vault` now runs the fork build instead of the
October bundle binary. This is the record of how that was decided, what was
checked, and how to undo it.

## Before

| fact | value |
|---|---|
binary | `/root/.arsumbris/bin/au`, 13 586 208 bytes, dated 2026-10-11 |
sha256 | `e0293cfdf45b462943bb00ce7e820a24e3704a60ad29dbdfe4ae8120fcab9a85` |
version | `au 0.0.5` (no commit marker) |
daemon pid | 498980, started 2026-11-13 06:27 by `arsumbris-engine.service` |
Node MCP layer | `arsumbris-mcp.service`, pid 1336489 |

## After

| fact | value |
|---|---|
binary | same path, 13 624 840 bytes |
sha256 | `97826aaee81e5cd3f43b164ff4ba567753b435dc54cce719e8d65d4458a6c5f7` |
built from | monorepo `5625aff` (engine `crates/au-cli` release build) |
daemon pid | 3133009, started 2026-11-14 13:07 |
rollback artifact | `/root/backups/au-pre-swap-20261114-1307` |

## What was proven before the swap

The jump is not "one added read": the served binary predates the fork's own
engine commits, so the comparison had to cover every read it answers, not just
the new verb.

1. **Full engine suite**: 675 pass, 4 fail. The four failures are environmental
   and pre-existing — verified by running the same four tests at the parent
   commit `509697d` in a separate worktree, where they fail identically
   (`an_unreadable_file_rebuilds_once_then_settles` needs a non-root user,
   `malformed_dotgit_file_*` and the two `saga_tests` guards need a temp dir
   outside any git discovery).
2. **Twelve reads compared byte-for-byte** between the live daemon and the new
   build on a frozen copy of the vault working tree, with paths normalized and
   the volatile `version` field dropped: `overview`, `hubs` (own and all),
   `diagnostics`, `members`, `types`, `files`, `imports`, `top_level_dirs`,
   `graph_shape`, `lifecycle`, `ignores`. **Zero differences.**
3. The copy had to keep the folder name `vault`: at `/tmp/vault-swap` the new
   build correctly reported one extra drift diagnostic
   (`repo-folder-name-mismatch`), which is the engine working, not a regression —
   the comparison at `/tmp/swaproot/vault` is the one that reads zero.
4. **Mutation path on the copy**: create, write with `expected_hash`, `grep`,
   delete — all four answered as specified, commits landed in the copy's git.

## What was checked after the swap

- the twelve reads re-run against the live socket: still zero differences from
  the pre-swap baseline;
- `au_grep` answers on the live vault through the engine socket and through the
  MCP adapter;
- the legacy path works end to end: Python bridge → Node daemon → new engine
  answered `mcp__au_overview` with `diagnostic_counts.total = 0`;
- `/root/vault` is untouched: same HEAD `66c9cf8`, same five dirty files as
  before the swap (none of them ours);
- `au-health.sh` reports `engine=1 mcp=1 action=ok`;
- `au daemon status /root/vault` is green **when run with `HOME=/root`**. From a
  shell whose `HOME` is somewhere else it looks for the socket under that home
  and reports "no daemon running": the health cron runs with `HOME=/root`, so
  this is a trap for humans, not a fault.

## Rollback

```sh
install -m 0755 /root/backups/au-pre-swap-20261114-1307 /root/.arsumbris/bin/au
systemctl restart arsumbris-engine
HOME=/root /root/.arsumbris/bin/au daemon status /root/vault
```

The Node layer does not need a restart: it reconnects per request (verified by
the bridge read above, which ran while the old daemon was already gone). Schema
version stayed 29 on both sides, so no client had to be reconfigured.
