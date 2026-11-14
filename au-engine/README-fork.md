# au-engine fork notes

This is the fork integration branch of the arsumbris engine (`fork/main`, created 2026-11-12
from the pristine 0.0.5-era local work at `44b32f4`), not upstream `main`
(`release 0.0.2-alpha`, frozen). It carries fork-local work on profiling and the mutation
path; the type system and vault format remain compatible, and wire shapes follow
`crates/au-engine/WIRE.md`.

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

Compare a fresh run against the recorded baseline, from `au-engine/` (export
`RUSTUP_HOME=/root/.rustup` first when `cargo +1.99.0` cannot find the toolchain):

```sh
fail() { t=$1; shift; cargo +1.99.0 test -p au-engine --offline "$@" 2>&1 \
  | sed -n '/^failures:$/,$p' | sed -n 's/^    \([A-Za-z_][A-Za-z0-9_:]*\)$/\1/p' \
  | sed "s/^/$t::/"; }
{ fail lib --lib; fail it --test it; } | sort > /tmp/engine-fail-now
awk -F '\t' '!/^#/ && NF >= 2 {print $1}' tests-known-environment.txt \
  | sort > /tmp/engine-fail-known
diff /tmp/engine-fail-known /tmp/engine-fail-now
```

`diff` printing test names means the baseline moved; no output means the known set is
unchanged. The file's trailing block also records the expected `fmt`/`clippy` failures.
