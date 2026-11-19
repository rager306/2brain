#!/usr/bin/env bash
# Register the Rust MCP adapter in whichever MCP clients are installed and
# report what each client actually verified. Everything runs in a throwaway
# HOME against a disposable repository: no production socket, no user config.
#
# AU_ENGINE_BIN and AU_MCP_RS_BIN override the built binaries; the defaults are
# the debug builds in the shared `.cargo-target` (`.cargo/config.toml`). Exit
# status is non-zero when an installed client that offers a non-interactive
# health check reports anything other than a connected server.
set -euo pipefail

# shellcheck source=scripts/lib/target-dir.sh
source "$(dirname "$0")/lib/target-dir.sh"
target_dir=$(cargo_target_dir)

ENGINE=${AU_ENGINE_BIN:-$target_dir/debug/au}
ADAPTER=${AU_MCP_RS_BIN:-$target_dir/debug/2brain}

if [[ ! -x $ENGINE ]]; then
  echo "missing engine binary: $ENGINE (set AU_ENGINE_BIN)" >&2
  exit 1
fi
if [[ ! -x $ADAPTER ]]; then
  echo "building the adapter with the pinned toolchain"
  RUSTUP_HOME=${RUSTUP_HOME:-/root/.rustup} cargo +1.99.0 build --offline >/dev/null
fi

# shellcheck source=scripts/lib/disposable-fixture.sh
source "$(dirname "$0")/lib/disposable-fixture.sh"
prepare_fixture
echo "engine: $socket"
# Resolve ONCE to an absolute path: the client configs below are written from
# another directory, and a `$PWD/` prefix on an already-absolute argument
# produces a doubled path that the client reports as ENOENT.
ADAPTER=$(cd "$(dirname "$ADAPTER")" && pwd)/$(basename "$ADAPTER")
echo "adapter: $ADAPTER"
echo

status=0
verified=0
report() { printf '%-14s %-10s %s\n' "$1" "$2" "$3"; }

echo "client         version    result"
echo "---------------------------------------------------------------"

if command -v claude >/dev/null; then
  version=$(claude --version 2>/dev/null | head -1 | awk '{print $1}')
  (cd "$repo" && claude mcp add --scope local 2brain -- "$ADAPTER" mcp "$socket" >/dev/null 2>&1) || true
  # The row, not the whole table: another connected server must not make this one
  # look healthy. `mcp list` prints one line per server, naming it.
  listed=$(cd "$repo" && timeout 60 claude mcp list 2>&1 || true)
  row=$(grep -m1 '2brain' <<<"$listed" || true)
  if grep -q '✔ Connected' <<<"$row"; then
    report claude "$version" "connected (initialize + tools/list)"
    verified=1
  else
    report claude "$version" "FAILED: ${row:-not listed}"
    status=1
  fi
else
  report claude - "not installed"
  verified=0
fi

if command -v codex >/dev/null; then
  version=$(codex --version 2>/dev/null | head -1 | awk '{print $2}')
  codex mcp add 2brain -- "$ADAPTER" mcp "$socket" >/dev/null 2>&1 || true
  listed=$(codex mcp list 2>/dev/null || true)
  if grep -q '^2brain' <<<"$listed"; then
    report codex "$version" "registered; no non-interactive health check"
  else
    report codex "$version" "FAILED: not listed"
    status=1
  fi
else
  report codex - "not installed"
fi

if command -v cursor-agent >/dev/null; then
  # Nothing is written and nothing is verified here: the config shape is known, the
  # client is not exercised, and saying "configured" would claim otherwise.
  report cursor-agent "$(timeout 20 cursor-agent --version 2>/dev/null | head -1)" "NOT exercised: no API key on this host"
else
  report cursor-agent - "not installed"
fi

if [[ $verified -eq 0 ]]; then
  echo "no client completed a health check: this run verified nothing" >&2
  status=1
fi
exit $status
