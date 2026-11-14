#!/usr/bin/env bash
# Does a model choose `au_batch` when the task spans files, and does it tell the truth
# about the result?
#
# `saga-and-rollback.sh` proves the engine's all-or-nothing behaviour mechanically.
# This asks the other question: given two files that must change together, does the
# model reach for the one tool that promises that, rather than two separate writes?
# Two separate writes are the failure mode that matters — they leave a half-applied
# change behind when the second one fails.
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

before=$(git -C "$repo" rev-list --count HEAD)
config=$root/turn-mcp.json
printf '{"mcpServers":{"arsumbris-rs":{"command":"%s","args":["%s","--allow-mutations"]}}}\n' \
  "$ADAPTER" "$socket" >"$config"

echo "engine:  $socket"
echo "adapter: $ADAPTER"
echo "prompt:  one change across notes/a.md and notes/b.md"

transcript=$(cd "$repo" && HOME="$TURN_HOME" claude -p \
'Both notes/a.md and notes/b.md must gain the line `batched` at the end, and the two
changes must land together or not at all. Use the arsumbris-rs MCP server: read each
file with au_content to get its hash, then apply both changes with a single au_batch
call (one write_file operation per file, each with its own hash). Finish with one line
saying whether the batch applied.' \
  --mcp-config "$config" \
  --allowedTools 'mcp__arsumbris-rs__au_overview,mcp__arsumbris-rs__au_content,mcp__arsumbris-rs__au_batch,mcp__arsumbris-rs__au_write_file' \
  --output-format stream-json --verbose 2>&1 || true)

status=0
fail() { printf 'FAIL: %s\n' "$1" >&2; status=1; }
pass() { printf 'PASS: %s\n' "$1"; }

calls=$(grep -o '"name":"mcp__arsumbris-rs__[a-z_]*"' <<<"$transcript" | sed 's/.*__//;s/"//' | tr '\n' ' ')
echo "tool calls: ${calls:-none}"

batches=$(grep -c '"name":"mcp__arsumbris-rs__au_batch"' <<<"$transcript" || true)
writes=$(grep -c '"name":"mcp__arsumbris-rs__au_write_file"' <<<"$transcript" || true)
[[ $batches -ge 1 ]] && pass "the model used au_batch" || fail "no au_batch call"
[[ $writes -eq 0 ]] && pass "the model did not fall back to two single writes" \
  || fail "the model issued $writes single writes instead of one batch"

grep -q 'batched' "$repo/notes/a.md" && grep -q 'batched' "$repo/notes/b.md" \
  && pass "both files carry the line" || fail "a file is unchanged"
[[ $(git -C "$repo" rev-list --count HEAD) -eq $((before + 1)) ]] \
  && pass "the batch made exactly one engine commit" || fail "the engine made more than one commit"

[[ ${AU_TURN_TRANSCRIPT:-} ]] && printf '%s\n' "$transcript" >"$AU_TURN_TRANSCRIPT"
echo "---"
if [[ $status -eq 0 ]]; then
  echo "PASS: a model chose the all-or-nothing path"
else
  echo "FAIL: the model did not use the transactional path" >&2
fi
exit $status
