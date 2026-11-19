#!/usr/bin/env bash
# The one failure the surface must never get wrong: a write whose reply was lost.
#
# The engine applies the mutation and the adapter never hears the answer. Retrying
# would apply it twice; doing nothing leaves the operator unsure whether a note was
# written. The contract is `outcome_unknown` plus `tb_reconcile`, and this script
# proves it against a REAL engine rather than a fake socket:
#
#   1. start a disposable engine and a proxy in front of it that forwards the write
#      to the engine and then drops the reply
#   2. read the file, then write it through the proxy  -> outcome_unknown?
#   3. ask `tb_reconcile` for the same path            -> applied, with the commit
#   4. check the engine's own git log and the file on disk, and that exactly one
#      commit was made: a retry would show two
#
# Uses the adapter's stdio protocol directly, so no model or client is involved: this
# is about the engine's durability and the adapter's state machine.
set -euo pipefail

# shellcheck source=scripts/lib/target-dir.sh
source "$(dirname "$0")/lib/target-dir.sh"
target_dir=$(cargo_target_dir)

ENGINE=${AU_ENGINE_BIN:-$target_dir/debug/au}
ADAPTER=${AU_MCP_RS_BIN:-$target_dir/debug/2brain}

[[ -x $ENGINE ]] || { echo "missing engine binary: $ENGINE (set AU_ENGINE_BIN)" >&2; exit 1; }
[[ -x $ADAPTER ]] || { echo "missing adapter: $ADAPTER (set AU_MCP_RS_BIN)" >&2; exit 1; }
ADAPTER=$(cd "$(dirname "$ADAPTER")" && pwd)/$(basename "$ADAPTER")

# shellcheck source=scripts/lib/disposable-fixture.sh
source "$(dirname "$0")/lib/disposable-fixture.sh"
prepare_fixture

export AU_PROXY_DEBUG=${AU_PROXY_DEBUG:-}
proxy_sock=$root/proxy.sock
python3 "$(dirname "$0")/lost_reply_proxy.py" --engine "$socket" --listen "$proxy_sock" \
  --drop mutate:write_file --drop-replies 1 >"$root/proxy.log" 2>&1 &
proxy_pid=$!
# Extend the fixture's trap rather than replacing it: bash keeps only the last EXIT
# trap, and the first one is what kills the daemon and removes the temp root.
trap 'kill "$proxy_pid" 2>/dev/null || true; cleanup_fixture' EXIT
for _ in $(seq 1 100); do [[ -S $proxy_sock ]] && break; sleep 0.05; done
[[ -S $proxy_sock ]] || { echo "the proxy never listened" >&2; exit 1; }

echo "engine: $socket"
echo "proxy:  $proxy_sock (drops one reply)"

python3 - "$ADAPTER" "$proxy_sock" "$repo" "$(dirname "$0")/lib" <<'PY'
import json, subprocess, sys

adapter, proxy, repo, lib = sys.argv[1:5]
sys.path.insert(0, lib)
from mcp_stdio import StdioMcp, tool_payload, tool_text

target = f"{repo}/notes/a.md"
before = subprocess.run(["git", "-C", repo, "rev-list", "--count", "HEAD"],
                        capture_output=True, text=True).stdout.strip()

client = StdioMcp([adapter, proxy, "--allow-mutations"], name="lost-reply").start()

read = client.call("tb_content", {"repo": "fixture", "path": "notes/a.md"})
payload = tool_payload(read)
content, hash_ = payload["content"]["text"], payload["content"]["hash"]

write = client.call("tb_write_file",
                    {"repo": "fixture", "path": "notes/a.md",
                     "content": content.replace("unique-marker-7f3a", "unique-marker-7f3a once"),
                     "expected_hash": hash_})
text = tool_text(write)
print("write reply:", text[:200])

after_read = client.call("tb_content", {"repo": "fixture", "path": "notes/a.md"})
current_hash = tool_payload(after_read)["content"]["hash"]
reconcile = client.call("tb_reconcile", {"repo": "fixture", "path": "notes/a.md",
                                         "expected_hash": current_hash})
report = tool_payload(reconcile)
print("reconcile:  ", json.dumps(report)[:300])
head = subprocess.run(["git", "-C", repo, "rev-parse", "HEAD"], capture_output=True, text=True).stdout.strip()
client.close()

after = subprocess.run(["git", "-C", repo, "rev-list", "--count", "HEAD"],
                       capture_output=True, text=True).stdout.strip()
on_disk = open(target).read()
commits = int(after) - int(before)

checks = [
    ("the write reply was not a success", "outcome_unknown" in text),
    ("the adapter did not retry on its own", text.count("outcome_unknown") == 1),
    ("reconcile says the mutation landed", report.get("outcome") == "applied"),
    ("reconcile reports the engine's real HEAD", report.get("current_commit") == head),
    ("the file on disk carries the change", "unique-marker-7f3a once" in on_disk),
    ("exactly one engine commit was made", commits == 1),
]
failed = False
for label, ok in checks:
    print(("PASS: " if ok else "FAIL: ") + label)
    failed |= not ok
print("commits added:", commits)
sys.exit(1 if failed else 0)
PY
# If the proxy never matched the mutation, the scenario silently stops being the lost
# reply and every later check would describe the wrong situation.
if ! grep -q 'dropped the reply to mutate:write_file' "$root/proxy.log"; then
  echo "FAIL: the proxy never dropped a write reply; see $root/proxy.log" >&2
  exit 1
fi
if [[ -n ${AU_PROXY_DEBUG:-} ]]; then
  echo "--- proxy log:"
  cat "$root/proxy.log"
fi
