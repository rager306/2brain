#!/usr/bin/env bash
# One real model turn through a SECOND client, to check the server is not
# Claude-Code-shaped.
#
# `model-turn.sh` proves the round trip through Claude Code. A second client is worth
# the turn because it exercises a different MCP client implementation: a different
# protocol revision negotiation, a different lazy-tool-loading strategy, and a
# different way of passing tool results back to the model. A server that only works
# with one client is a server with an accidental contract.
#
# The fixture carries `unique-marker-7f3a` in one note; the model must find it with
# `au_grep` and name the file. Codex runs with its own CODEX_HOME so the user's
# config.toml is untouched; only `auth.json` is copied in, because a fresh home has
# no credentials.
#
# Costs one small Codex turn on YOUR credentials.
set -euo pipefail

ENGINE=${AU_ENGINE_BIN:-../au-engine/target/debug/au}
ADAPTER=${AU_MCP_RS_BIN:-target/debug/au-mcp-rs}
TURN_HOME=${AU_TURN_HOME:-$HOME}

[[ -x $ENGINE ]] || { echo "missing engine binary: $ENGINE (set AU_ENGINE_BIN)" >&2; exit 1; }
[[ -x $ADAPTER ]] || { echo "missing adapter: $ADAPTER (set AU_MCP_RS_BIN)" >&2; exit 1; }
command -v codex >/dev/null || { echo "codex is not installed" >&2; exit 1; }
ADAPTER=$(cd "$(dirname "$ADAPTER")" && pwd)/$(basename "$ADAPTER")

# shellcheck source=scripts/lib/disposable-fixture.sh
source "$(dirname "$0")/lib/disposable-fixture.sh"
prepare_fixture

codex_home=$root/codex-home
mkdir -p "$codex_home"
if [[ -f $TURN_HOME/.codex/auth.json ]]; then
  cp "$TURN_HOME/.codex/auth.json" "$codex_home/auth.json"
else
  echo "no $TURN_HOME/.codex/auth.json: codex has no credentials, skipping" >&2
  exit 2
fi

CODEX_HOME=$codex_home codex mcp add arsumbris-rs -- "$ADAPTER" "$socket" >/dev/null 2>&1 || {
  echo "codex refused the MCP registration" >&2; exit 1; }

echo "engine:  $socket"
echo "adapter: $ADAPTER"
echo "client:  codex $(codex --version 2>/dev/null | awk '{print $2}')"

transcript=$(cd "$repo" && HOME="$TURN_HOME" CODEX_HOME=$codex_home codex exec \
  --json --skip-git-repo-check -C "$repo" \
  'Use the arsumbris-rs MCP server: call au_grep to search the workspace for
unique-marker-7f3a. Then answer with one line: the path of the file that contains it
and the text of that line.' 2>&1 || true)

status=0
fail() { printf 'FAIL: %s\n' "$1" >&2; status=1; }
pass() { printf 'PASS: %s\n' "$1"; }

if grep -q 'au_grep' <<<"$transcript"; then pass "the model used au_grep"; else fail "no au_grep call"; fi
if grep -q 'unique-marker-7f3a' <<<"$transcript"; then pass "the marker travelled through the engine"; else fail "the marker never came back"; fi
if grep -q 'notes/a.md' <<<"$transcript"; then pass "the answer names the file that holds the marker"; else fail "the answer does not name notes/a.md"; fi

# Which MCP revision this client negotiated: the adapter prints it in initialize only,
# so read it from the client's own log if it exposes one, and say "unknown" otherwise
# rather than guessing.
if grep -qo 'protocolVersion[^,]*' <<<"$transcript"; then
  echo "negotiated: $(grep -o 'protocolVersion[^,]*' <<<"$transcript" | head -1)"
fi

[[ ${AU_TURN_TRANSCRIPT:-} ]] && printf '%s\n' "$transcript" >"$AU_TURN_TRANSCRIPT"
echo "---"
if [[ $status -eq 0 ]]; then
  echo "PASS: one Codex model turn completed through the adapter"
else
  echo "FAIL: the Codex turn did not complete as expected" >&2
fi
exit $status
