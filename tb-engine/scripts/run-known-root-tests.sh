#!/usr/bin/env bash
# Run the tests known to fail ONLY because the suite runs as root.
#
# tests-known-environment.txt records them with a reason class. Every one of them
# injects a failure through file permissions (chmod 000/555), which cannot block
# uid 0: the fixture stays readable, deletable, and movable, so the failure under
# test never happens. Run as an unprivileged user, they pass.
#
# This script proves that, without asking anyone to install a user or a sandbox:
# it builds the test binaries as the current user, copies them into a world-
# readable directory, and re-runs exactly the recorded names as `nobody`.
#
# Exit status 0 means every recorded test passed as an unprivileged user, i.e. the
# baseline is environmental and the code is fine.
set -euo pipefail

cd "$(dirname "$0")/.."
RUSTUP_HOME=${RUSTUP_HOME:-/root/.rustup}
CARGO="cargo +1.99.0"

LIST=tests-known-environment.txt
[[ -f $LIST ]] || { echo "missing $LIST" >&2; exit 1; }

lib_names=$(awk -F '\t' '$1 ~ /^lib::/ {sub(/^lib::/, "", $1); print $1}' "$LIST")
it_names=$(awk -F '\t' '$1 ~ /^it::/ {sub(/^it::/, "", $1); print $1}' "$LIST")

if [[ -z $lib_names && -z $it_names ]]; then
  echo "the recorded baseline is empty: every test already passes here"
  exit 0
fi

echo "building the test binaries"
RUSTUP_HOME=$RUSTUP_HOME $CARGO test --offline --lib --no-run >/dev/null
RUSTUP_HOME=$RUSTUP_HOME $CARGO test --offline --test it --no-run >/dev/null

target_dir=$(RUSTUP_HOME=$RUSTUP_HOME $CARGO metadata --format-version 1 --no-deps \
  | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')

# The crate is `tb-engine`, so its test binary is `tb_engine-<hash>`. This looked for
# `tb_engine-*` and matched nothing but stale `.d` files, failing before it ran a test.
lib_bin=$(ls -t "$target_dir"/debug/deps/tb_engine-* 2>/dev/null | grep -vE '\.(d|txt)$' | head -1)
it_bin=$(ls -t "$target_dir"/debug/deps/it-* 2>/dev/null | grep -vE '\.(d|txt)$' | head -1)
[[ -x $lib_bin && -x $it_bin ]] || { echo "test binaries not found under $target_dir" >&2; exit 1; }

# `nobody` cannot traverse /root on this host, so the binaries are re-run from
# world-readable scratch space; the tests themselves only use temp directories.
scratch=$(mktemp -d /tmp/tb-root-tests-XXXXXX)
chmod 755 "$scratch"
trap 'rm -rf "$scratch"' EXIT
cp "$lib_bin" "$it_bin" "$scratch"/

run_as_nobody() {
  local bin=$1; shift
  setpriv --reuid=65534 --regid=65534 --clear-groups \
    env HOME="$scratch" TMPDIR=/tmp "$bin" --exact "$@"
}

status=0
if [[ -n $lib_names ]]; then
  echo "lib, as nobody:"
  # shellcheck disable=SC2086
  run_as_nobody "$scratch/$(basename "$lib_bin")" $lib_names || status=1
fi
if [[ -n $it_names ]]; then
  echo "it, as nobody:"
  # shellcheck disable=SC2086
  run_as_nobody "$scratch/$(basename "$it_bin")" $it_names || status=1
fi

if [[ $status -eq 0 ]]; then
  echo "PASS: every recorded failure is environmental (root cannot be blocked by chmod)"
else
  echo "FAIL: a recorded test failed even unprivileged — that is a real regression" >&2
fi
exit $status
