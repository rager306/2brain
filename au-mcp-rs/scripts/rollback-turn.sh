#!/usr/bin/env bash
# Does a model plan a revert before doing it?
#
# `saga-and-rollback.sh` proves `au_rollback_preview` answers correctly when a Python
# client calls it with a commit id. This asks the harder question: a model has no git
# tool here, so it must learn the commit from `au_content`, pass it to the preview, and
# read the plan — the exact chain a careful operator follows before undoing a write.
#
# Costs one model turn on YOUR credentials.
set -euo pipefail

ENGINE=${AU_ENGINE_BIN:-../au-engine/target/debug/au}
ADAPTER=${AU_MCP_RS_BIN:-target/debug/au-mcp-rs}
TURN_HOME=${AU_TURN_HOME:-$HOME}

[[ -x $ENGINE ]] || { echo "missing engine binary: $ENGINE" >&2; exit 1; }
[[ -x $ADAPTER ]] || { echo "missing adapter: $ADAPTER" >&2; exit 1; }
command -v claude >/dev/null || { echo "claude is not installed" >&2; exit 1; }
ADAPTER=$(cd "$(dirname "$ADAPTER")" && pwd)/$(basename "$ADAPTER")

# shellcheck source=scripts/lib/disposable-fixture.sh
source "$(dirname "$0")/lib/disposable-fixture.sh"
prepare_fixture

config=$root/turn-mcp.json
printf '{"mcpServers":{"2brain":{"command":"%s","args":["%s","--allow-mutations"]}}}\n' \
  "$ADAPTER" "$socket" >"$config"

echo "engine:  $socket"
echo "adapter: $ADAPTER"

# A preview only has a `restore` to report when the commit it previews changed a file.
# The fixture starts at its initial commit, so make one real write first — as a Python
# client, before the model is involved, so the turn tests the model and not the setup.
python3 - "$ADAPTER" "$socket" "$repo" <<'PY'
import json, subprocess, sys

adapter, socket_path, repo = sys.argv[1:4]
proc = subprocess.Popen([adapter, socket_path, "--allow-mutations"],
                        stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                        stderr=subprocess.DEVNULL, text=True)

def rpc(id_, method, params):
    proc.stdin.write(json.dumps({"jsonrpc": "2.0", "id": id_, "method": method,
                                 "params": params}) + "\n")
    proc.stdin.flush()
    return json.loads(proc.stdout.readline())

def call(id_, name, args):
    reply = rpc(id_, "tools/call", {"name": name, "arguments": args})
    return json.loads(reply["result"]["content"][0]["text"])

rpc(1, "initialize", {"protocolVersion": "2025-06-18", "capabilities": {},
                      "clientInfo": {"name": "rollback-setup", "version": "1"}})
proc.stdin.write(json.dumps({"jsonrpc": "2.0", "method": "notifications/initialized"}) + "\n")
proc.stdin.flush()
read = call(2, "au_content", {"repo": "fixture", "path": "notes/a.md"})
body = read["content"]["text"] + "\nprepare the revert\n"
call(3, "au_write_file", {"repo": "fixture", "path": "notes/a.md",
                          "content": body, "expected_hash": read["content"]["hash"]})
proc.terminate()
PY

before=$(git -C "$repo" rev-list --count HEAD)
echo "prompt:  plan a revert of the last write"

transcript=$(cd "$repo" && HOME="$TURN_HOME" claude -p \
'Using the 2brain MCP server only: read notes/a.md with au_content and note the
`commit` it reports. Then call au_rollback_preview for that commit. Do not change
anything else. Finish with one line: the files the revert would touch, and whether the
preview says it is safe.' \
  --mcp-config "$config" \
  --allowedTools 'mcp__2brain__au_content,mcp__2brain__au_rollback_preview' \
  --output-format stream-json --verbose 2>&1 || true)

status=0
fail() { printf 'FAIL: %s\n' "$1" >&2; status=1; }
pass() { printf 'PASS: %s\n' "$1"; }

calls=$(grep -o '"name":"mcp__2brain__[a-z_]*"' <<<"$transcript" | sed 's/.*__//;s/"//' | tr '\n' ' ')
echo "tool calls: ${calls:-none}"

grep -q '"name":"mcp__2brain__au_content"' <<<"$transcript" \
  && pass "the model read the file" || fail "no au_content call"
grep -q '"name":"mcp__2brain__au_rollback_preview"' <<<"$transcript" \
  && pass "the model asked for a rollback preview" || fail "no au_rollback_preview call"
# The commit must be the one au_content reported, not something invented: the answer
# is only meaningful if the model took the id from the engine.
read_commit=$(grep -oE '"commit":"[0-9a-f]{40}"' <<<"$transcript" | head -1 | cut -d'"' -f4 || true)
preview_arg=$(grep -oE '"name":"mcp__2brain__au_rollback_preview","input":\{"repo":"[^"]*","commit":"[0-9a-f]+"' <<<"$transcript" | grep -oE '[0-9a-f]{40}' | head -1 || true)
if [[ -n $read_commit && $preview_arg == "$read_commit" ]]; then
  pass "the preview used the commit au_content reported"
else
  fail "the preview argument (${preview_arg:-none}) is not the commit the read reported (${read_commit:-none})"
fi
# The plan must come back with the path and a verdict, not just arrive.
grep -E '"type":"tool_result"' <<<"$transcript" | grep -q '"action":"restore"' \
  && pass "the engine's plan names a restore" || fail "no preview plan in a tool result"
grep -E '"type":"tool_result"' <<<"$transcript" | grep -q '"safe":true' \
  && pass "the engine reports the revert as safe" || fail "the plan is not safe or absent"
# A preview is a READ: the fixture must be exactly where it was.
[[ $(git -C "$repo" rev-list --count HEAD) -eq $before ]] \
  && pass "the preview made no commit" || fail "the preview created a commit"

printf '%s\n' "$transcript" >"$root/transcript.jsonl"
final=$(python3 "$(dirname "$0")/lib/last_answer.py" --client claude "$root/transcript.jsonl" 2>/dev/null || true)
grep -q 'a.md' <<<"$final" && pass "the final answer names the file the revert would touch" \
  || fail "the final answer does not name a.md (got: ${final:0:200})"

[[ ${AU_TURN_TRANSCRIPT:-} ]] && printf '%s\n' "$transcript" >"$AU_TURN_TRANSCRIPT"
echo "---"
if [[ $status -eq 0 ]]; then
  echo "PASS: a model planned a revert from the engine's own history"
else
  echo "FAIL: the rollback preview was not exercised as intended" >&2
fi
exit $status
