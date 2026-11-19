#!/usr/bin/env bash
# One real model turn over the MUTATION path: the gate, the read-before-write
# guard, and the engine commit that follows.
#
# `model-turn.sh` proves a read round trip. This script proves the risky half: the
# model must read the file first, carry the hash the read returned into the write,
# and the write must land as one engine commit. A grant the server mishandled, a
# freshness check that never fires, or a hash the model cannot see would all pass
# every protocol test and fail here.
#
# Two scenarios, one turn each:
#   1. granted  — the adapter runs with --allow-mutations; the model edits
#                 notes/a.md and the file, the git log, and its own answer agree.
#                 The allow-list carries `tb_overview` too: a model asked to edit
#                 one note orients itself first, and denying that would test the
#                 allow-list rather than the server. TB_TURN_ONLY=1 runs this
#                 scenario alone.
#   2. read-only — the same adapter without grants; tb_write_file is not in the
#                 tool list at all, so fail-closed is visible to the model, not just
#                 to our tests.
#
# Costs two small model turns on YOUR credentials. The binaries default to the shared
# `.cargo-target` debug builds (TB_ENGINE_BIN / TB_MCP_BIN override), and `claude`
# needs a working login.
set -euo pipefail

# shellcheck source=scripts/lib/target-dir.sh
source "$(dirname "$0")/lib/target-dir.sh"
target_dir=$(cargo_target_dir)

ENGINE=${TB_ENGINE_BIN:-$target_dir/debug/au}
ADAPTER=${TB_MCP_BIN:-$target_dir/debug/2brain}
TURN_HOME=${TB_TURN_HOME:-$HOME}

[[ -x $ENGINE ]] || { echo "missing engine binary: $ENGINE (set TB_ENGINE_BIN)" >&2; exit 1; }
[[ -x $ADAPTER ]] || { echo "missing adapter: $ADAPTER (set TB_MCP_BIN)" >&2; exit 1; }
command -v claude >/dev/null || { echo "claude is not installed" >&2; exit 1; }

ADAPTER=$(cd "$(dirname "$ADAPTER")" && pwd)/$(basename "$ADAPTER")

# shellcheck source=scripts/lib/disposable-fixture.sh
source "$(dirname "$0")/lib/disposable-fixture.sh"
prepare_fixture

before=$(git -C "$repo" rev-list --count HEAD)
b_before=$(cat "$repo/notes/b.md")
target=$repo/notes/a.md
status=0
fail() { printf 'FAIL: %s\n' "$1" >&2; status=1; }
pass() { printf 'PASS: %s\n' "$1"; }

turn() { # turn <config file> <prompt> <allowed tools>
  (cd "$repo" && HOME="$TURN_HOME" claude -p "$2" \
    --mcp-config "$1" --allowedTools "$3" \
    --output-format stream-json --verbose 2>&1 || true)
}

config_for() { # config_for <file> [extra adapter args...]
  local file=$1; shift
  local args='"'$socket'"'
  local arg
  for arg in "$@"; do args+=',"'"$arg"'"'; done
  printf '{"mcpServers":{"2brain":{"command":"%s","args":[%s]}}}\n' \
    "$ADAPTER" "$args" >"$file"
}

echo "=== scenario 1: granted"
printf 'engine:  %s\nadapter: %s\n' "$socket" "$ADAPTER"
granted=$root/granted-mcp.json
config_for "$granted" --allow-mutations
transcript=$(turn "$granted" \
'Edit notes/a.md with the 2brain MCP server. Steps:
1. Call tb_content on notes/a.md and keep the `hash` and `body` it returns.
2. Call tb_write_file on notes/a.md with that `hash` and a body equal to the returned
   body, except that the line `find me: unique-marker-7f3a` becomes
   `find me: unique-marker-7f3a updated`. Keep every other line and the frontmatter
   exactly as they were.
3. Answer one line: whether the write succeeded.' \
'mcp__2brain__tb_overview,mcp__2brain__tb_content,mcp__2brain__tb_write_file')

grep -q '"name":"mcp__2brain__tb_content"' <<<"$transcript" &&
  pass "the model read before writing" || fail "no tb_content call"
grep -q '"name":"mcp__2brain__tb_write_file"' <<<"$transcript" &&
  pass "the model called tb_write_file" || fail "no tb_write_file call"
grep -q 'unique-marker-7f3a updated' "$target" &&
  pass "the file on disk carries the new line" || fail "the file was not changed"
[[ $(git -C "$repo" rev-list --count HEAD) -gt $before ]] &&
  pass "the write is an engine commit ($(( $(git -C "$repo" rev-list --count HEAD) - before )) new)" ||
  fail "no new commit: the write did not reach the engine"
grep -q '"permission_denials":\[[^]]' <<<"$transcript" &&
  fail "a permission denial appeared in the granted run" || pass "no permission denial"

echo
if [[ ${TB_TURN_ONLY:-} == 1 ]]; then
  echo "skipping scenario 2 (TB_TURN_ONLY=1)"
  echo "---"
  [[ $status -eq 0 ]] && echo "PASS: the granted write path works through a model"
  exit $status
fi
echo "=== scenario 2: read-only, the same request"
read_only=$root/readonly-mcp.json
config_for "$read_only"
transcript2=$(turn "$read_only" \
'Use the 2brain MCP server to write the text "nope" into notes/b.md with
tb_write_file. If that tool does not exist, list the write tools you DO have and do
nothing else.' \
'mcp__2brain__tb_write_file')

grep -q '"name":"mcp__2brain__tb_write_file"' <<<"$transcript2" &&
  fail "a read-only launch let the model call tb_write_file" ||
  pass "tb_write_file was never callable in a read-only launch"
# The engine COMMITS every write, so a clean `git diff` proves nothing here: compare
# the bytes and the commit count with the state captured before the turn.
[[ $(git -C "$repo" rev-list --count HEAD) -eq $before ]] &&
  pass "the read-only turn made no engine commit" ||
  fail "the read-only turn added a commit"
[[ $(cat "$repo/notes/b.md") == "$b_before" ]] &&
  pass "notes/b.md is byte-identical to its pre-turn content" ||
  fail "notes/b.md differs from its pre-turn content"

echo "---"
# Cross-check the same turn against the procedure's state machine. The script's own
# assertions above are older than the machine; where the two disagree, the machine names
# the state and this fails, which is how the drift gets found instead of argued about.
# The duplicate assertions are removed once a live run confirms they agree
# (docs/architecture-plan.md, track G).
if [[ $status -eq 0 ]]; then
  context=$(mktemp /tmp/tb-mutation-context-XXXXXX.json)
  printf '{"repo": "%s", "target_path": "%s", "expected_text": "%s", "commits_before": %s}\n' \
    "$repo" "$repo/notes/a.md" "unique-marker-7f3a updated" "$before" >"$context"
  if python3 "$(dirname "$0")/lib/procedure.py" verify mutation_turn --context "$context"; then
    echo "PASS: the procedure's states agree with the turn"
  else
    echo "FAIL: the procedure stopped at a state the turn's own assertions passed" >&2
    status=1
  fi
  rm -f "$context"
fi

if [[ $status -eq 0 ]]; then
  echo "PASS: the mutation path works through a model, and a read-only launch cannot write"
else
  echo "FAIL: the mutation path did not hold" >&2
fi
[[ ${TB_TURN_TRANSCRIPT:-} ]] && {
  printf '%s\n' "$transcript" >"${TB_TURN_TRANSCRIPT}.granted.jsonl"
  printf '%s\n' "$transcript2" >"${TB_TURN_TRANSCRIPT}.readonly.jsonl"
}
exit $status
