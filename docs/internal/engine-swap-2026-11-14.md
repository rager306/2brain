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
built from | monorepo `5625aff` (engine `crates/tb-cli` release build) |
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
- `tb_grep` answers on the live vault through the engine socket and through the
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


## Second swap, same day — the `batch` verb

`grep` was live at 13:07; `batch` (engine verb + `tb_batch`) landed at `59e94db`
afterwards, so the served binary was one feature behind again. Same protocol, with
one correction learned from the first round.

| fact | value |
|---|---|
before | `/root/.arsumbris/bin/au`, sha256 `97826aae…` (the 13:07 swap) |
after | sha256 `95a55cae9709bde6f45bb6cbdbbc2325e8ce70f08840d630b036ddd9904ee8d5` |
built from | monorepo `59e94db` |
rollback artifact | `/root/backups/au-20261114-post-grep` |

**The correction:** the first attempt at this comparison compared a fresh copy
against a baseline captured twenty minutes earlier, and four graph reads
(`overview`, `hubs`, `hubs_all`, `graph_shape`) differed. They differed because the
digest loop had written three notes at 13:24 — the live index was stale against
the copy, not the build against the build. The vault is a living corpus: capture
the baseline and make the copy back to back, then compare. Done that way, all
twelve reads are identical again.

Verified live after the restart: the same twelve reads unchanged from the fresh
baseline, `grep` answering, and `{"mutate":"batch","ops":[]}` answering
`` `ops` is empty `` — which proves the daemon serves the verb, where an older
engine says `unknown variant`. The bridge path (`mcp__au_overview`) answers with
`diagnostic_counts.total = 0`, `/root/vault` keeps HEAD `00e0743` and its five
dirty files, and `au-health.sh` reports `engine=1 mcp=1 action=ok`.


## Third swap — the `graph_health` read

| fact | value |
|---|---|
before | sha256 `95a55cae…` (the `batch` swap) |
after | sha256 `98ea88e3d2d3a8943625f3f52ca857582f16dc27da7dfecc3b9df895bca4fb26` |
built from | monorepo `877168c` |
rollback artifact | `/root/backups/au-20261114-post-batch` |

Same protocol, and this time the baseline and the copy were taken back to back as
the second swap taught. Twelve reads: zero differences. `graph_health` on the copy
and then on the live daemon answers the same counts — 1156 nodes, 4565 edges, 211
components, 17 cycles over 664 nodes, 19 dead ends, 178 sources, 202 isolated
files, 0 broken links — and the MCP adapter returns the identical report.

The vault kept HEAD `00e0743` and its five dirty files; `au-health.sh` reports
`engine=1 mcp=1 action=ok`.


## Fourth swap — the `rollback_preview` read

| fact | value |
|---|---|
before | sha256 `98ea88e3…` (the `graph_health` swap) |
after | sha256 `b7e48c82c7ffe891272a9c32822354e065333ca48e21c0ff5344c94f491d5991` |
built from | monorepo `015aeda` |
rollback artifact | `/root/backups/au-20261114-post-health` |

Twelve reads, zero differences; the new verb previewed the copy's HEAD commit as
one clean path and then the live HEAD the same way. Vault stayed at `00e0743` with
its five dirty files, `au-health.sh` green.

Engine regression evidence for this one is a set comparison rather than a count:
the `it` suite fails **39** tests on this host both at the parent commit `332b6d3`
and after the change, with an identical name set — root user and git discovery in
temp dirs, not the engine. Passes went 668 → 671 with the three new tests.
