#!/usr/bin/env bash
# Does a MODEL follow the recovery contract when a write's reply is lost?
#
# `lost-reply.sh` proves the server reports `outcome_unknown` and that `au_reconcile`
# resolves it — with a Python client doing exactly what the tool descriptions say. The
# open question is different: given that description, does a model do the right thing
# on its own? A retry here is a duplicate write in a real vault, so this is the
# scenario where a good tool description earns its keep.
#
# The proxy forwards the write to the engine and drops the reply, so the mutation
# lands and the adapter cannot say so.
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

proxy_sock=$root/proxy.sock
python3 "$(dirname "$0")/lost_reply_proxy.py" --engine "$socket" --listen "$proxy_sock" \
  --drop mutate:write_file --drop-replies 1 >"$root/proxy.log" 2>&1 &
proxy_pid=$!
trap 'kill "$proxy_pid" 2>/dev/null || true' EXIT
for _ in $(seq 1 100); do [[ -S $proxy_sock ]] && break; sleep 0.05; done
[[ -S $proxy_sock ]] || { echo "the proxy never listened" >&2; exit 1; }

before=$(git -C "$repo" rev-list --count HEAD)
config=$root/turn-mcp.json
printf '{"mcpServers":{"arsumbris-rs":{"command":"%s","args":["%s","--allow-mutations"]}}}\n' \
  "$ADAPTER" "$proxy_sock" >"$config"

echo "engine:  $socket"
echo "proxy:   $proxy_sock (the write reply will be dropped)"
echo "prompt:  add a line to notes/a.md; recover from a lost reply"

transcript=$(cd "$repo" && HOME="$TURN_HOME" claude -p \
'Append the line `recovered` to notes/a.md using the arsumbris-rs MCP server. Use
au_content first to read the file and get its hash, then au_write_file with that hash.
If the write reports that its outcome is unknown, do NOT send the write again: use
au_reconcile to learn what happened. Finish with one line saying whether the file now
contains the appended line.' \
  --mcp-config "$config" \
  --allowedTools 'mcp__arsumbris-rs__au_overview,mcp__arsumbris-rs__au_content,mcp__arsumbris-rs__au_write_file,mcp__arsumbris-rs__au_reconcile' \
  --output-format stream-json --verbose 2>&1 || true)

status=0
fail() { printf 'FAIL: %s\n' "$1" >&2; status=1; }
pass() { printf 'PASS: %s\n' "$1"; }

calls=$(grep -o '"name":"mcp__arsumbris-rs__[a-z_]*"' <<<"$transcript" | sed 's/.*__//;s/"//' | tr '\n' ' ')
echo "tool calls: ${calls:-none}"

grep -q '"name":"mcp__arsumbris-rs__au_write_file"' <<<"$transcript" &&
  pass "the model sent a write" || fail "no write was attempted"
grep -q '"name":"mcp__arsumbris-rs__au_reconcile"' <<<"$transcript" &&
  pass "the model asked au_reconcile what happened" || fail "the model never reconciled"
writes=$(grep -c '"name":"mcp__arsumbris-rs__au_write_file"' <<<"$transcript" || true)
[[ $writes -le 1 ]] &&
  pass "the model did not retry the write" || fail "the write was sent $writes times"
[[ $(git -C "$repo" rev-list --count HEAD) -eq $((before + 1)) ]] &&
  pass "exactly one engine commit exists" || fail "the engine made more than one commit"
grep -q 'recovered' "$repo/notes/a.md" &&
  pass "the file carries the appended line" || fail "the file is unchanged"

[[ ${AU_TURN_TRANSCRIPT:-} ]] && printf '%s\n' "$transcript" >"$AU_TURN_TRANSCRIPT"
echo "---"
if [[ $status -eq 0 ]]; then
  echo "PASS: a model followed the recovery contract on a lost reply"
else
  echo "FAIL: the model did not follow the recovery contract" >&2
fi
exit $status
