# Deploy, 2026-11-17: the migration-window advisory

Fourth swap of the day, and the smallest: one new diagnostic, no shape change.

## What was served before

| half | identity |
|---|---|
| engine | `/root/.2brain/bin/au`, sha256 `8736c9fc…` (release 2's schema flip without the advisory) |
| adapter | `/usr/local/bin/2brain`, `0.1.0+020d0de64469` (untouched, as in the two swaps before it) |
| vault | `/root/vault` at `1bf9c92`, 1 040 notes, 0 diagnostics |

## Artifacts

- Candidate: `/tmp/s1/au-r2c`, sha256 `c9df1cbd…`, built from `fa45472`.
- Rollback: `/root/backups/au-20261117-pre-window`, sha256 `8736c9fc…`.

## Why this swap needed less ceremony than the last three

The change is additive in the strict sense: a `hint` appears only on a claim written in the release-1
spelling. Every read's shape and every value is unchanged, and the vault holds no such claim, so the
expected diff is **zero**.

That expectation was checked twice before installing — on the candidate's own vault copy (0
diagnostics) and by the fact that `git diff` of the scenarios shows the fixtures already canonical —
and once after: the live socket answers **0 diagnostics**, and the eight reads I spot-checked
(`overview`, `hubs`, `graph_health`, `files`, `search`, `instances_of` on the LEGACY name, `imports`,
`members`) all answer with no error frame.

## Swap

copied the served binary to `au-20261117-pre-window`; installed atomically; `systemctl restart
2brain-engine`; waited for `ready=true`; live diagnostics 0.

## Rollback

```sh
install -m 0755 /root/backups/au-20261117-pre-window /root/.2brain/bin/au.new
mv -f /root/.2brain/bin/au.new /root/.2brain/bin/au
systemctl restart 2brain-engine
```

## What a reader gains

A client that lists diagnostics now sees, for any tree that still writes `au.engine.*`, one hint per
claim with the canonical spelling in its fix. That is the whole point of the swap: the window is open
by design, and it is no longer silent.
