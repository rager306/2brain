#!/usr/bin/env bash
# Register the Rust MCP adapter in whichever MCP clients are installed and
# report what each client actually verified. Everything runs in a throwaway
# HOME against a disposable repository: no production socket, no user config.
#
# Requires AU_ENGINE_BIN pointing at a built `au` binary. Exit status is
# non-zero when an installed client that offers a non-interactive health check
# reports anything other than a connected server.
set -euo pipefail

ENGINE=${AU_ENGINE_BIN:-../au-engine/target/debug/au}
ADAPTER=${AU_MCP_RS_BIN:-target/debug/au-mcp-rs}

if [[ ! -x $ENGINE ]]; then
  echo "missing engine binary: $ENGINE (set AU_ENGINE_BIN)" >&2
  exit 1
fi
if [[ ! -x $ADAPTER ]]; then
  echo "building the adapter with the pinned toolchain"
  RUSTUP_HOME=${RUSTUP_HOME:-/root/.rustup} cargo +1.99.0 build --offline >/dev/null
fi

root=$(mktemp -d /tmp/au-client-matrix-XXXXXX)
export HOME=$root/home
export XDG_CONFIG_HOME=$root/config
mkdir -p "$HOME" "$XDG_CONFIG_HOME"

cleanup() {
  if [[ -n ${daemon_pid:-} ]]; then
    kill "$daemon_pid" 2>/dev/null || true
    wait "$daemon_pid" 2>/dev/null || true
  fi
  rm -rf "$root"
}
trap cleanup EXIT

repo=$root/fixture
mkdir -p "$repo/.arsumbris" "$repo/type" "$repo/notes"
printf 'name: fixture\n' >"$repo/.arsumbris/repo.yaml"
printf 'fields:\n  title: String\n' >"$repo/type/note.type.yaml"
printf -- '---\ntype: note\ntitle: matrix\n---\n' >"$repo/notes/a.md"
git -C "$repo" init -q
git -C "$repo" add -A
git -C "$repo" -c user.name=matrix -c user.email=matrix@example.invalid \
  -c commit.gpgsign=false commit -q -m fixture

"$ENGINE" daemon start "$repo" >/dev/null 2>&1 &
daemon_pid=$!
socket=
for _ in $(seq 1 200); do
  socket=$(ls "$HOME"/.arsumbris/au-engine/run/*.sock 2>/dev/null | head -1 || true)
  [[ -n $socket ]] && break
  sleep 0.1
done
if [[ -z $socket ]]; then
  echo "the disposable engine never became ready" >&2
  exit 1
fi
echo "engine: $socket"
# Resolve ONCE to an absolute path: the client configs below are written from
# another directory, and a `$PWD/` prefix on an already-absolute argument
# produces a doubled path that the client reports as ENOENT.
ADAPTER=$(cd "$(dirname "$ADAPTER")" && pwd)/$(basename "$ADAPTER")
echo "adapter: $ADAPTER"
echo

status=0
report() { printf '%-14s %-10s %s\n' "$1" "$2" "$3"; }

echo "client         version    result"
echo "---------------------------------------------------------------"

if command -v claude >/dev/null; then
  version=$(claude --version 2>/dev/null | head -1 | awk '{print $1}')
  (cd "$repo" && claude mcp add --scope local arsumbris-rs -- "$ADAPTER" "$socket" >/dev/null 2>&1) || true
  listed=$(cd "$repo" && claude mcp list 2>&1 || true)
  if grep -q '✔ Connected' <<<"$listed"; then
    report claude "$version" "connected (initialize + tools/list)"
  else
    report claude "$version" "FAILED: $(grep -m1 arsumbris-rs <<<"$listed" || echo 'not listed')"
    status=1
  fi
else
  report claude - "not installed"
fi

if command -v codex >/dev/null; then
  version=$(codex --version 2>/dev/null | head -1 | awk '{print $2}')
  codex mcp add arsumbris-rs -- "$ADAPTER" "$socket" >/dev/null 2>&1 || true
  listed=$(codex mcp list 2>/dev/null || true)
  if grep -q '^arsumbris-rs' <<<"$listed"; then
    report codex "$version" "registered; no non-interactive health check"
  else
    report codex "$version" "FAILED: not listed"
    status=1
  fi
else
  report codex - "not installed"
fi

if command -v cursor-agent >/dev/null; then
  report cursor-agent "$(cursor-agent --version 2>/dev/null | head -1)" "configured via .cursor/mcp.json; needs an API key, not exercised"
else
  report cursor-agent - "not installed"
fi

exit $status
