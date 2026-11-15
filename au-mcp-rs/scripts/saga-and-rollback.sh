#!/usr/bin/env bash
# Two promises that only a real engine can keep: an all-or-nothing batch, and a
# rollback plan that changes nothing.
#
# Scenario 1 — a batch that must not half-apply. Three file operations go to the
# engine as one saga; the third carries a stale hash, so the engine rejects the set.
# "Rejected" is only worth anything if the first two really did not land, so the
# check is the working tree and the commit count, not the error text.
#
# Scenario 2 — a rollback preview that must stay a plan. After a real write, ask what
# reverting the PARENT commit would do: it must name the path, say whether the
# revert restores or deletes it, and leave the tree and HEAD untouched.
set -euo pipefail

ENGINE=${AU_ENGINE_BIN:-../au-engine/target/debug/au}
ADAPTER=${AU_MCP_RS_BIN:-target/debug/au-mcp-rs}

[[ -x $ENGINE ]] || { echo "missing engine binary: $ENGINE (set AU_ENGINE_BIN)" >&2; exit 1; }
[[ -x $ADAPTER ]] || { echo "missing adapter: $ADAPTER (set AU_MCP_RS_BIN)" >&2; exit 1; }
ADAPTER=$(cd "$(dirname "$ADAPTER")" && pwd)/$(basename "$ADAPTER")

# shellcheck source=scripts/lib/disposable-fixture.sh
source "$(dirname "$0")/lib/disposable-fixture.sh"
prepare_fixture

echo "engine:  $socket"
echo "adapter: $ADAPTER"

python3 - "$ADAPTER" "$socket" "$repo" "$(dirname "$0")/lib" <<'PY'
import json, subprocess, sys

adapter, socket_path, repo, lib = sys.argv[1:5]
sys.path.insert(0, lib)
from mcp_stdio import StdioMcp, is_error, tool_text

client = StdioMcp([adapter, socket_path, "--allow-mutations", "--allow-composition"],
                  name="saga").start()

def call(name, args):
    reply = client.call(name, args)
    return tool_text(reply), is_error(reply)

def git(*args):
    return subprocess.run(["git", "-C", repo, *args], capture_output=True, text=True).stdout.strip()

def read(path):
    payload, _ = call("au_content", {"repo": "fixture", "path": path})
    return json.loads(payload)["content"]

a, b, c = read("notes/a.md"), read("notes/b.md"), read("notes/c.md")
before_commit = git("rev-parse", "HEAD")
before_tree = git("status", "--porcelain")

ops = [
    {"op": "write_file", "path": "notes/a.md", "content": a["text"] + "\nfirst\n",
     "expected_hash": a["hash"]},
    {"op": "write_file", "path": "notes/b.md", "content": b["text"] + "\nsecond\n",
     "expected_hash": b["hash"]},
    # The third asks the engine to replace text that is not there. Every hash is
    # correct, so this is not the gate refusing up front: the engine applies the
    # first two, hits a miss on the third, and must undo the set.
    {"op": "edit_file", "path": "notes/c.md", "old_string": "text-that-is-not-there",
     "new_string": "x", "expected_hash": c["hash"]},
]
payload, _ = call("au_batch", {"repo": "fixture", "ops": ops})
print("batch reply:", payload[:220])

after_commit = git("rev-parse", "HEAD")
after_tree = git("status", "--porcelain")
a_text = open(f"{repo}/notes/a.md").read()

checks = [
    # Any error would satisfy "refused"; the property under test is that the ENGINE
    # rejected a mid-saga operation with every gate satisfied, so the message must be
    # the engine's own miss and not a client-side refusal.
    ("the engine refused the batch on the third op", "old_string not found" in payload),
    ("no commit was made", after_commit == before_commit),
    ("the first operation did not land", "first" not in a_text),
    ("the working tree is unchanged", after_tree == before_tree),
]

# Scenario 2: a real write, then a preview of reverting THAT commit.
payload, _ = call("au_write_file", {"repo": "fixture", "path": "notes/a.md",
                                    "content": a["text"] + "\nwritten\n",
                                    "expected_hash": a["hash"]})
written = git("rev-parse", "HEAD")
plan_payload, _ = call("au_rollback_preview", {"repo": "fixture", "commit": written})
plan = json.loads(plan_payload)
plan = plan.get("rollback_preview", plan)
print("rollback plan:", json.dumps(plan)[:260])

paths = {entry["path"].split("notes/")[-1]: entry for entry in plan.get("paths", [])}
checks += [
    ("the write landed as one commit", written != before_commit),
    ("the preview names the changed path", "a.md" in paths),
    ("the preview says it would restore", paths.get("a.md", {}).get("action") == "restore"),
    ("the preview reports the path safe", plan.get("safe") is True),
    ("the preview changed nothing", git("rev-parse", "HEAD") == written
     and "written" in open(f"{repo}/notes/a.md").read()),
]

client.close()
failed = False
for label, ok in checks:
    print(("PASS: " if ok else "FAIL: ") + label)
    failed |= not ok
sys.exit(1 if failed else 0)
PY
