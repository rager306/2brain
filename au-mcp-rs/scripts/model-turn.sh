#!/usr/bin/env bash
# One real model turn through the adapter: the check our own tests cannot make.
#
# Every other verification here asks whether the server speaks MCP correctly. This
# one asks whether a model, given the tool list, chooses a tool, supplies arguments,
# and gets the engine's answer back — the step where a broken description, a wrong
# input schema, or a client-side permission model would still show up as "everything
# passes".
#
# The fixture carries `unique-marker-7f3a` in one note. The model is asked to find
# it with `au_grep` and name the file. Nothing but this engine knows that string, so
# a correct answer proves the round trip rather than a lucky guess.
#
# Costs one small model turn on YOUR credentials, which is why it is a separate
# script and not part of `client-matrix.sh`: that one must stay free and offline.
#
# Requires AU_ENGINE_BIN and AU_MCP_RS_BIN, and `claude` with a working login.
set -euo pipefail

ENGINE=${AU_ENGINE_BIN:-../au-engine/target/debug/au}
ADAPTER=${AU_MCP_RS_BIN:-target/debug/au-mcp-rs}
TURN_HOME=${AU_TURN_HOME:-$HOME}

if [[ ! -x $ENGINE ]]; then
  echo "missing engine binary: $ENGINE (set AU_ENGINE_BIN)" >&2
  exit 1
fi
if [[ ! -x $ADAPTER ]]; then
  echo "missing adapter: $ADAPTER (set AU_MCP_RS_BIN)" >&2
  exit 1
fi
command -v claude >/dev/null || { echo "claude is not installed" >&2; exit 1; }

ADAPTER=$(cd "$(dirname "$ADAPTER")" && pwd)/$(basename "$ADAPTER")

# shellcheck source=scripts/lib/disposable-fixture.sh
source "$(dirname "$0")/lib/disposable-fixture.sh"
prepare_fixture

config=$root/turn-mcp.json
printf '{"mcpServers":{"arsumbris-rs":{"command":"%s","args":["%s"]}}}\n' \
  "$ADAPTER" "$socket" >"$config"

echo "engine:  $socket"
echo "adapter: $ADAPTER"
echo "prompt:  find unique-marker-7f3a with au_grep"

# The daemon's HOME is the throwaway one; the model's turn needs the real one for
# its credentials. `--mcp-config` keeps the registration out of any config file the
# user owns.
transcript=$(cd "$repo" && HOME="$TURN_HOME" claude -p \
  'Use the arsumbris-rs MCP server: call au_grep to search the workspace for
unique-marker-7f3a. Then answer with one line: the path of the file that contains it
and the text of that line.' \
  --mcp-config "$config" \
  --allowedTools 'mcp__arsumbris-rs__au_grep' \
  --output-format stream-json --verbose 2>&1 || true)
AU_ANSWER_SOURCE=$root/transcript.jsonl
printf '%s\n' "$transcript" >"$AU_ANSWER_SOURCE"

status=0
fail() { printf 'FAIL: %s\n' "$1" >&2; status=1; }

if grep -q '"name":"mcp__arsumbris-rs__au_grep"' <<<"$transcript"; then
  echo "PASS: the model called mcp__arsumbris-rs__au_grep"
else
  fail "no au_grep tool call in the transcript"
fi

# The engine's own answer must come back in a TOOL RESULT. Grepping the whole
# transcript for the marker would pass on the model's own `au_grep` arguments, before
# any result exists: the first occurrence in a real transcript is
# `"input":{"query":"unique-marker-7f3a"}`. Only the tool_result lines count.
if grep -E '"type":"tool_result"' <<<"$transcript" | grep -q 'unique-marker-7f3a'; then
  echo "PASS: the engine returned the marker inside a tool result"
else
  fail "no tool result carried the marker"
fi

# Same reasoning as the marker: the path appears in the model's own arguments and in
# the tool result. The claim under test is that the ANSWER names the file, so the
# final assistant text is the only place to look.
final=$(python3 "$(dirname "$0")/lib/last_answer.py" --client claude "$AU_ANSWER_SOURCE" 2>/dev/null || true)
if grep -q 'notes/a.md' <<<"$final"; then
  echo "PASS: the final answer names the file that holds the marker"
else
  fail "the final answer does not name notes/a.md (got: ${final:0:200})"
fi

if grep -q '"permission_denials":\[[^]]' <<<"$transcript"; then
  fail "the client recorded a permission denial: $(grep -o '"permission_denials":\[[^]]*\]' <<<"$transcript" | head -1)"
else
  echo "PASS: no permission denial (the allow-list covered the call)"
fi

if [[ ${AU_TURN_TRANSCRIPT:-} ]]; then
  printf '%s\n' "$transcript" >"$AU_TURN_TRANSCRIPT"
  echo "transcript: $AU_TURN_TRANSCRIPT"
fi

echo "---"
if [[ $status -eq 0 ]]; then
  echo "PASS: one model turn completed through the adapter"
else
  echo "FAIL: the turn did not complete as expected" >&2
fi
exit $status
