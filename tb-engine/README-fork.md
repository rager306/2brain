# au-engine fork notes

This is the fork integration branch of the 2brain engine (`fork/main`, created 2026-11-12
from the pristine 0.0.5-era local work at `44b32f4`), not upstream `main`
(`release 0.0.2-alpha`, frozen). It carries fork-local work on profiling and the mutation
path; the type system and vault format remain compatible, and wire shapes follow
`crates/tb-engine/WIRE.md`.

## Test baseline

`tests-known-environment.txt` lists every test that fails on this host, with a reason class
and the first line of its panic. Four fail today, all for one reason: **the suite runs as
root**, so `chmod 000`/`chmod 555` cannot make a fixture unreadable, undeletable, or
unmovable, and the two failure-injection tests never see their failure. An unprivileged
runner passes them.

A second host artifact used to account for most of the red: an empty **`/tmp/.git`**
directory, created 2026-10-18. `discover_gitdir()` walks up from every
`tempfile::tempdir()` fixture, found it, and treated the fixture as a git work tree while git
itself answered "not a git repository" — which turned 40 mutation tests into failures that
looked like engine bugs. It was removed on 2026-11-14; the baseline went from 43 failures to
4. If the count jumps back to ~43, check for that directory first:

```sh
test -d /tmp/.git && echo "the fixture-poisoning directory is back: rmdir /tmp/.git"
```

Only a NEW name is a regression; a missing name means the host changed.

The baseline is also **provable as environmental** without creating a user:
`scripts/run-known-root-tests.sh` builds the test binaries, copies them somewhere
`nobody` can execute them, and re-runs exactly the names recorded in the file as
uid 65534. It passes — which is the statement "the code is fine, the runner is
privileged", made checkable in one command:

```sh
cd au-engine && bash scripts/run-known-root-tests.sh   # exit 0 = only root, not code
```

Compare a fresh run against the recorded baseline, from `tb-engine/` (export
`RUSTUP_HOME=/root/.rustup` first when `cargo +1.99.0` cannot find the toolchain):

```sh
fail() { t=$1; shift; cargo +1.99.0 test -p tb-engine --offline "$@" 2>&1 \
  | sed -n '/^failures:$/,$p' | sed -n 's/^    \([A-Za-z_][A-Za-z0-9_:]*\)$/\1/p' \
  | sed "s/^/$t::/"; }
{ fail lib --lib; fail it --test it; } | sort > /tmp/engine-fail-now
awk -F '\t' '!/^#/ && NF >= 2 {print $1}' tests-known-environment.txt \
  | sort > /tmp/engine-fail-known
diff /tmp/engine-fail-known /tmp/engine-fail-now
```

`diff` printing test names means the baseline moved; no output means the known set is
unchanged. The file's header records the expected `fmt`/`clippy` state.

`cargo fmt --check` is clean as of 2026-11-14 (`cargo fmt` over 131 files, formatting
only); the engine's test counts did not move. `clippy -D warnings` is NOT a gate for
the engine: upstream style lints remain in `au-core` and the crates clippy reaches
after it, so a red clippy says nothing about a fork change. The gate is per crate —
`cargo clippy -p tb-engine -p tb-cli --all-targets -- -D warnings` — plus the whole
`au-mcp-rs` workspace, which is clean.
