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
# `tb_grep` and name the file. Codex runs with its own CODEX_HOME so the user's
# config.toml is untouched; only `auth.json` is copied in, because a fresh home has
# no credentials.
#
# Costs one small Codex turn on YOUR credentials.
set -euo pipefail

ENGINE=${AU_ENGINE_BIN:-../tb-engine/target/debug/au}
ADAPTER=${AU_MCP_RS_BIN:-target/debug/2brain}
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

# The client's own log does not report which MCP revision it negotiated, so the server
# command goes through a tee that records both directions when asked. That turns "the
# client speaks some revision" into a fact read off the wire.
tee=$root/mcp-wire.jsonl
if [[ -n ${AU_TEE_TRANSCRIPT:-} ]]; then
  # Absolute, because the client spawns the server with its own working directory.
  tee_script=$(cd "$(dirname "$0")" && pwd)/stdio_tee.py
  server_cmd=(python3 "$tee_script" --record "$tee" -- "$ADAPTER" mcp "$socket")
else
  server_cmd=("$ADAPTER" mcp "$socket")
fi
CODEX_HOME=$codex_home codex mcp add 2brain -- "${server_cmd[@]}" >/dev/null 2>&1 || {
  echo "codex refused the MCP registration" >&2; exit 1; }

echo "engine:  $socket"
echo "adapter: $ADAPTER"
echo "client:  codex $(codex --version 2>/dev/null | awk '{print $2}')"

transcript=$(cd "$repo" && HOME="$TURN_HOME" CODEX_HOME=$codex_home codex exec \
  --json --skip-git-repo-check -C "$repo" \
  'Use the 2brain MCP server: call tb_grep to search the workspace for
unique-marker-7f3a. Then answer with one line: the path of the file that contains it
and the text of that line.' 2>&1 || true)

status=0
fail() { printf 'FAIL: %s\n' "$1" >&2; status=1; }
pass() { printf 'PASS: %s\n' "$1"; }

# Exact tool name, not a bare substring: "tb_grep" also appears in prose and in error
# text, and the two turn scripts used to disagree about what "used the tool" means.
grep -q '"type":"mcp_tool_call"' <<<"$transcript" && grep -q '"tool":"tb_grep"' <<<"$transcript" \
  && pass "the model called tb_grep" || fail "no tb_grep tool call"

# The marker must come back inside the tool RESULT: the model's own arguments carry
# it before any result exists, so a whole-transcript grep proves nothing.
# `item.started` also carries the tool name with `"result":null`, so the line must be a
# COMPLETED call whose result object holds the marker.
grep -E '"type":"item.completed".*"type":"mcp_tool_call".*"result": *\{[^}]*unique-marker-7f3a' <<<"$transcript" \
  && pass "the engine returned the marker inside a completed tool result" \
  || fail "no completed tool result carried the marker"

printf '%s\n' "$transcript" >"$root/transcript.jsonl"
final=$(python3 "$(dirname "$0")/lib/last_answer.py" --client codex "$root/transcript.jsonl" 2>/dev/null || true)
grep -q 'notes/a.md' <<<"$final" \
  && pass "the final answer names the file that holds the marker" \
  || fail "the final answer does not name notes/a.md (got: ${final:0:200})"

# Which MCP revision this client negotiated, read off the wire when the tee is on.
if [[ -n ${AU_TEE_TRANSCRIPT:-} && -s $tee ]]; then
  cp "$tee" "$AU_TEE_TRANSCRIPT"
  requested=$(python3 - "$tee" <<'PY'
import json, sys
for line in open(sys.argv[1]):
    record = json.loads(line)
    if record["dir"] == "c2s" and '"initialize"' in record["line"]:
        print(json.loads(record["line"])["params"]["protocolVersion"])
        break
PY
)
  answered=$(python3 - "$tee" <<'PY'
import json, sys
for line in open(sys.argv[1]):
    record = json.loads(line)
    if record["dir"] == "s2c" and '"protocolVersion"' in record["line"]:
        try:
            print(json.loads(record["line"])["result"]["protocolVersion"])
        except Exception:
            print("?")
        break
PY
)
  echo "protocol: client asked $requested, server answered ${answered:-?}"
fi

[[ ${AU_TURN_TRANSCRIPT:-} ]] && printf '%s\n' "$transcript" >"$AU_TURN_TRANSCRIPT"
echo "---"
if [[ $status -eq 0 ]]; then
  echo "PASS: one Codex model turn completed through the adapter"
else
  echo "FAIL: the Codex turn did not complete as expected" >&2
fi
exit $status
