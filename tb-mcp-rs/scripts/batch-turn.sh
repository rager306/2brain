#!/usr/bin/env bash
# Does a model choose `tb_batch` when the task spans files, and does it tell the truth
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

# shellcheck source=scripts/lib/target-dir.sh
source "$(dirname "$0")/lib/target-dir.sh"
target_dir=$(cargo_target_dir)

ENGINE=${TB_ENGINE_BIN:-$target_dir/debug/au}
ADAPTER=${TB_MCP_BIN:-$target_dir/debug/2brain}
TURN_HOME=${TB_TURN_HOME:-$HOME}

[[ -x $ENGINE ]] || { echo "missing engine binary: $ENGINE" >&2; exit 1; }
[[ -x $ADAPTER ]] || { echo "missing adapter: $ADAPTER" >&2; exit 1; }
command -v claude >/dev/null || { echo "claude is not installed" >&2; exit 1; }
ADAPTER=$(cd "$(dirname "$ADAPTER")" && pwd)/$(basename "$ADAPTER")

# shellcheck source=scripts/lib/disposable-fixture.sh
source "$(dirname "$0")/lib/disposable-fixture.sh"
prepare_fixture

before=$(git -C "$repo" rev-list --count HEAD)
config=$root/turn-mcp.json
printf '{"mcpServers":{"2brain":{"command":"%s","args":["%s","--allow-mutations"]}}}\n' \
  "$ADAPTER" mcp "$socket" >"$config"

echo "engine:  $socket"
echo "adapter: $ADAPTER"
echo "prompt:  one change across notes/a.md and notes/b.md"

transcript=$(cd "$repo" && HOME="$TURN_HOME" claude -p \
'Both notes/a.md and notes/b.md must gain the line `batched` at the end, and the two
changes must land together or not at all. Use the 2brain MCP server: read each
file with tb_content to get its hash, then apply both changes with a single tb_batch
call (one write_file operation per file, each with its own hash). Finish with one line
saying whether the batch applied.' \
  --mcp-config "$config" \
  --allowedTools 'mcp__2brain__tb_overview,mcp__2brain__tb_content,mcp__2brain__tb_batch,mcp__2brain__tb_write_file' \
  --output-format stream-json --verbose 2>&1 || true)

status=0
fail() { printf 'FAIL: %s\n' "$1" >&2; status=1; }
pass() { printf 'PASS: %s\n' "$1"; }

calls=$(grep -o '"name":"mcp__2brain__[a-z_]*"' <<<"$transcript" | sed 's/.*__//;s/"//' | tr '\n' ' ')
echo "tool calls: ${calls:-none}"

batches=$(grep -o '"name":"mcp__2brain__tb_batch"' <<<"$transcript" | wc -l)
writes=$(grep -o '"name":"mcp__2brain__tb_write_file"' <<<"$transcript" | wc -l)
[[ $batches -ge 1 ]] && pass "the model used tb_batch" || fail "no tb_batch call"
[[ $writes -eq 0 ]] && pass "the model did not fall back to two single writes" \
  || fail "the model issued $writes single writes instead of one batch"

grep -q 'batched' "$repo/notes/a.md" && grep -q 'batched' "$repo/notes/b.md" \
  && pass "both files carry the line" || fail "a file is unchanged"
[[ $(git -C "$repo" rev-list --count HEAD) -eq $((before + 1)) ]] \
  && pass "the batch made exactly one engine commit" || fail "the engine made more than one commit"

[[ ${TB_TURN_TRANSCRIPT:-} ]] && printf '%s\n' "$transcript" >"$TB_TURN_TRANSCRIPT"
echo "---"
if [[ $status -eq 0 ]]; then
  echo "PASS: a model chose the all-or-nothing path"
else
  echo "FAIL: the model did not use the transactional path" >&2
fi
exit $status
