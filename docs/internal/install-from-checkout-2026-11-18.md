# Install from a clean checkout — 2026-11-18

The question was narrow: `install.sh` was repaired to ask `cargo metadata` for the target directory
instead of guessing `tb-mcp-rs/target`, and no one had run it on a tree that has no build output at all.
Every earlier install test copied binaries or ran in a tree where `target/` already existed, which is why
the guess survived.

## The run

A detached worktree at `b376e86`, the state the restore was asked about:

```sh
git -C /root/2brain worktree add --detach /tmp/install-check HEAD
ls /tmp/install-check/.cargo-target /tmp/install-check/tb-engine/target
# both absent: this is the clean case
cd /tmp/install-check
PREFIX=/tmp/install-prefix CARGO_TARGET_DIR=/root/2brain/.cargo-target ./install.sh
```

Result: `EXIT=0`. Installed: `/tmp/install-prefix/bin/2brain` (`2brain 0.1.0+b376e86e6d31`),
`/tmp/install-prefix/bin/au` (`au 0.0.5`), and five templates under
`/tmp/install-prefix/share/2brain/templates`.

A cold release build of both halves took about 25 minutes. `CARGO_TARGET_DIR` pointed at the monorepo's
shared directory, which is why the log shows the crate graph compiled twice: `tb-engine` and `tb-cli`
each appear for the CLI's feature set and again for the adapter's. That is the known cost of one target
directory with two manifests, recorded in `docs/build-layout.md`, not a failure.

## The acceptance, on an isolated home

```sh
HOME=/tmp/install-home /tmp/install-prefix/bin/2brain init /tmp/install-ws
HOME=/tmp/install-home /tmp/install-prefix/bin/2brain daemon start /tmp/install-ws
HOME=/tmp/install-home /tmp/install-prefix/bin/2brain doctor /tmp/install-ws
HOME=/tmp/install-home python3 /root/2brain/tools/probe/mcp_smoke.py \
    --binary /tmp/install-prefix/bin/2brain \
    --socket /tmp/install-home/.arsumbris/au-engine/run/99f051c55c4a7b28.sock \
    --repo install-ws
```

- `init`: `created workspace "install-ws" … (.2brain/repo.yaml, README.md, git initialised)`.
- `doctor`: **7 ok, 0 warn, no failures** — binary, engine, target, socket, daemon (wire schema 29,
engine version 1), workspace (1 member, no diagnostics), and `tools/list` answering 36 tools.
- `mcp_smoke.py`: 8 checks passed. Initialize; a read through the installed binary; a clean create that
lands;
the hash a read returns; a gated write that lands and answers its new hash; a `require_clean` create
refused with `policy/diagnostics` and the code `type-repo-unknown`.

## What the run found

The socket in that last command is under `~/.arsumbris/au-engine/run`, on a **fresh** home. The state
root's write rule was missing: `state_dir_in` preferred an existing `.2brain`, fell back to an existing
`.arsumbris`, and — when neither existed — fell back to the legacy name, so a new installation was
spelled the old way forever. The reserved-root check had the same gap from the other side: it compared
only `<root>/.arsumbris` against the device root, so with a `.2brain` device root and a `.2brain`
repository identity the newest pairing was the one collision it could not see.

Both are fixed in the same wave as this document, with the engine tests `a_fresh_home_writes_the_new_
state_dir_and_an_old_one_keeps_its_own` and `a_new_spelling_device_root_is_reserved_too`, plus
`socket_is_under_state_run` rewritten to assert the invariant through `state_dir_in` rather than the
spelling. Nothing on this host moves: `/root/.2brain` exists, so both the engine and the adapter keep
resolving it.

## Cleanup the run left

The daemon from the acceptance was stopped with `2brain daemon stop`, and `/tmp/install-check`,
`/tmp/install-prefix`, `/tmp/install-home`, `/tmp/install-ws` were removed. The worktree registration is
gone with them (`git worktree prune`).
