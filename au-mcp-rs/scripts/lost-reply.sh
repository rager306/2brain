#!/usr/bin/env bash
# The one failure the surface must never get wrong: a write whose reply was lost.
#
# The engine applies the mutation and the adapter never hears the answer. Retrying
# would apply it twice; doing nothing leaves the operator unsure whether a note was
# written. The contract is `outcome_unknown` plus `au_reconcile`, and this script
# proves it against a REAL engine rather than a fake socket:
#
#   1. start a disposable engine and a proxy in front of it that forwards the write
#      to the engine and then drops the reply
#   2. read the file, then write it through the proxy  -> outcome_unknown?
#   3. ask `au_reconcile` for the same path            -> applied, with the commit
#   4. check the engine's own git log and the file on disk, and that exactly one
#      commit was made: a retry would show two
#
# Uses the adapter's stdio protocol directly, so no model or client is involved: this
# is about the engine's durability and the adapter's state machine.
set -euo pipefail

ENGINE=${AU_ENGINE_BIN:-../au-engine/target/debug/au}
ADAPTER=${AU_MCP_RS_BIN:-target/debug/au-mcp-rs}

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
trap 'kill "$proxy_pid" 2>/dev/null || true' EXIT
for _ in $(seq 1 100); do [[ -S $proxy_sock ]] && break; sleep 0.05; done
[[ -S $proxy_sock ]] || { echo "the proxy never listened" >&2; exit 1; }

echo "engine: $socket"
echo "proxy:  $proxy_sock (drops one reply)"

python3 - "$ADAPTER" "$proxy_sock" "$repo" <<'PY'
import json, subprocess, sys

adapter, proxy, repo = sys.argv[1], sys.argv[2], sys.argv[3]
target = f"{repo}/notes/a.md"
before = subprocess.run(["git", "-C", repo, "rev-list", "--count", "HEAD"],
                        capture_output=True, text=True).stdout.strip()

proc = subprocess.Popen([adapter, proxy, "--allow-mutations"],
                        stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                        stderr=subprocess.PIPE, text=True)

def rpc(id_, method, params):
    proc.stdin.write(json.dumps({"jsonrpc": "2.0", "id": id_, "method": method,
                                 "params": params}) + "\n")
    proc.stdin.flush()
    line = proc.stdout.readline()
    if not line:
        raise SystemExit(f"adapter closed the stream answering {method}")
    return json.loads(line)

rpc(1, "initialize", {"protocolVersion": "2025-06-18", "capabilities": {},
                      "clientInfo": {"name": "lost-reply", "version": "1"}})
proc.stdin.write(json.dumps({"jsonrpc": "2.0", "method": "notifications/initialized"}) + "\n")
proc.stdin.flush()

def call(id_, name, args):
    return rpc(id_, "tools/call", {"name": name, "arguments": args})

read = call(2, "au_content", {"repo": "fixture", "path": "notes/a.md"})
if "error" in read or "content" not in read.get("result", {}):
    raise SystemExit(f"au_content answered: {json.dumps(read)[:600]}")
payload = json.loads(read["result"]["content"][0]["text"])
content, hash_ = payload["content"]["text"], payload["content"]["hash"]

write = call(3, "au_write_file", {"repo": "fixture", "path": "notes/a.md",
                                  "content": content.replace("unique-marker-7f3a", "unique-marker-7f3a once"),
                                  "expected_hash": hash_})
text = write["result"]["content"][0]["text"]
print("write reply:", text[:200])

after_read = call(4, "au_content", {"repo": "fixture", "path": "notes/a.md"})
after_payload = json.loads(after_read["result"]["content"][0]["text"])
current_hash = after_payload["content"]["hash"]
reconcile = call(5, "au_reconcile", {"repo": "fixture", "path": "notes/a.md",
                                     "expected_hash": current_hash})
report = json.loads(reconcile["result"]["content"][0]["text"])
print("reconcile:  ", json.dumps(report)[:300])
head = subprocess.run(["git", "-C", repo, "rev-parse", "HEAD"], capture_output=True, text=True).stdout.strip()
proc.terminate()

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
if [[ -n ${AU_PROXY_DEBUG:-} ]]; then
  echo "--- proxy log:"
  cat "$root/proxy.log"
fi
