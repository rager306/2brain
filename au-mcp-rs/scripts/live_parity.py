#!/usr/bin/env python3
"""Compare the two live stacks on the production vault, read-only.

`bridge_parity.py` proves the bridge and the adapter agree on a disposable fixture.
This runs the same comparison against the vault that is actually served, which is the
only evidence that matters before a client is switched.

Read-only by construction: both clients are started WITHOUT `--allow-mutations`, so
the adapter cannot advertise a mutation tool and the engine never receives a `mutate`
frame; no signal is sent to any daemon, and nothing in the vault is written.

    python3 scripts/live_parity.py                      # auto-detect both sockets
    python3 scripts/live_parity.py --socket /path.sock --daemon-socket /path.sock
    python3 scripts/live_parity.py --json               # machine-readable

Exit 0 when every compared read agrees, 1 when one differs, 2 when the comparison
could not be made (a socket missing, a client refusing the handshake) — a comparison
that did not happen is not a pass.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent / "lib"))
from mcp_stdio import StdioMcp, StdioMcpError, tool_payload  # noqa: E402

BUNDLE = Path(__file__).resolve().parent.parent.parent
ADAPTER = os.environ.get(
    "AU_MCP_RS_BIN", str(Path(__file__).resolve().parent.parent / "target/release/au-mcp-rs"))
BRIDGE = BUNDLE / "au-mcp-bridge" / "bridge.py"

# Each row: a name, the adapter tool and arguments, the bridge tool and arguments, and
# how to reduce both answers to the fact that must match. The bridge answers the bare
# list or object; the adapter wraps it under the read's key, which is the only
# difference these reducers have to absorb.
FACTS = [
    ("members", "au_members", {}, "mcp__au_members", {}, "repo"),
    ("top_level_dirs", "au_top_level_dirs", {}, "mcp__au_top_level_dirs", {}, "repo:name"),
    ("types", "au_types", {"limit": 500}, "mcp__au_types", {"limit": 500}, "name:repo:hash"),
    ("hubs", "au_hubs", {"limit": 20}, "mcp__au_hubs", {"limit": 20}, "path:refs_total"),
    ("diagnostics", "au_diagnostics", {"limit": 200}, "mcp__au_diagnostics",
     {"limit": 200}, "path:code:severity"),
]


def newest(directory: Path) -> Path | None:
    if not directory.is_dir():
        return None
    sockets = sorted(directory.glob("*.sock"), key=lambda p: p.stat().st_mtime, reverse=True)
    return sockets[0] if sockets else None


def find(kind: str, override: str | None) -> Path | None:
    """The newest socket of `kind`, whichever home the caller runs under.

    The daemons are started with HOME=/root by their units, while an agent shell may
    have a different HOME; searching a couple of roots is cheaper than making the
    operator pass four paths, and `--socket`/`--daemon-socket` still win.
    """
    if override:
        return Path(override)
    sub = {"engine": ".arsumbris/au-engine/run", "daemon": ".arsumbris/au-mcp/run"}[kind]
    for home in (Path.home(), Path("/root")):
        found = newest(home / sub)
        if found is not None:
            return found
    return None


def rows(payload) -> list[dict]:
    """The list a tool answered with, whether wrapped or bare."""
    if isinstance(payload, list):
        return payload
    if isinstance(payload, dict):
        for key in ("members", "top_level_dirs", "types", "hubs", "diagnostics"):
            if isinstance(payload.get(key), list):
                return payload[key]
    raise ValueError(f"no list in the answer: {json.dumps(payload)[:200]}")


def reduce_rows(payload, shape: str) -> list[str]:
    keys = shape.split(":")
    out = []
    for row in rows(payload):
        parts = []
        for key in keys:
            if key == "repo:name":       # name and repo are separate keys here
                parts.append(str(row.get("repo")))
                parts.append(str(row.get("name")))
            else:
                parts.append(str(row.get(key)))
        out.append(":".join(parts))
    return out


def first_difference(left: list[str], right: list[str]) -> str:
    only_left = sorted(set(left) - set(right))
    only_right = sorted(set(right) - set(left))
    if only_left:
        return f"only in the adapter: {only_left[:3]}"
    if only_right:
        return f"only in the bridge: {only_right[:3]}"
    if left != right:
        return "same rows, different order"
    return ""


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--socket", help="engine socket (default: newest under ~/.arsumbris/au-engine/run)")
    parser.add_argument("--daemon-socket", help="Node daemon socket for the bridge (default: newest under ~/.arsumbris/au-mcp/run)")
    parser.add_argument("--json", action="store_true", help="machine-readable result")
    args = parser.parse_args()

    engine_socket = find("engine", args.socket)
    daemon_socket = find("daemon", args.daemon_socket)
    if engine_socket is None or not Path(engine_socket).exists():
        print("no engine socket: the adapter has nothing to read", file=sys.stderr)
        return 2
    if daemon_socket is None or not Path(daemon_socket).exists():
        print("no daemon socket: the bridge has nothing to read", file=sys.stderr)
        return 2
    if not Path(ADAPTER).exists():
        print(f"missing adapter at {ADAPTER}", file=sys.stderr)
        return 2
    if not BRIDGE.exists():
        print(f"missing bridge at {BRIDGE}", file=sys.stderr)
        return 2

    bridge_env = dict(os.environ, AU_MCP_SOCKET=str(daemon_socket), AU_MCP_NAME="arsumbris")
    results = []
    status = 0
    try:
        with StdioMcp([str(ADAPTER), str(engine_socket)], name="adapter") as adapter, \
             StdioMcp(["python3", str(BRIDGE)], name="bridge", env=bridge_env) as bridge:
            adapter_tools = {t["name"] for t in adapter.request("tools/list", {})["result"]["tools"]}
            mutations = {"au_write_file", "au_edit_file", "au_delete_file", "au_batch",
                         "au_create_file", "au_rename", "au_rename_type"}
            if adapter_tools & mutations:
                results.append({"fact": "read-only launch", "agree": False,
                                "detail": f"a mutation tool is advertised: {sorted(adapter_tools & mutations)}"})
                status = 1

            for name, a_tool, a_args, b_tool, b_args, shape in FACTS:
                reducer = (lambda payload, s=shape: reduce_rows(payload, s))
                try:
                    a_payload = tool_payload(adapter.call(a_tool, a_args))
                    b_payload = tool_payload(bridge.call(b_tool, b_args))
                except (StdioMcpError, ValueError) as error:
                    results.append({"fact": name, "agree": None, "detail": f"could not compare: {error}"})
                    status = max(status, 2)
                    continue
                a_rows, b_rows = reducer(a_payload), reducer(b_payload)
                agree = a_rows == b_rows
                results.append({
                    "fact": name,
                    "agree": agree,
                    "adapter": len(a_rows),
                    "bridge": len(b_rows),
                    "detail": "" if agree else first_difference(a_rows, b_rows),
                })
                if not agree:
                    status = 1
    except StdioMcpError as error:
        print(f"a client could not start: {error}", file=sys.stderr)
        return 2

    if args.json:
        print(json.dumps({"engine_socket": str(engine_socket), "daemon_socket": str(daemon_socket),
                          "results": results, "status": status}, indent=1))
        return status

    print(f"engine: {engine_socket}")
    print(f"daemon: {daemon_socket}")
    print()
    print(f"{'fact':<16} {'adapter':>8} {'bridge':>7}  verdict")
    print("-" * 66)
    for row in results:
        if row.get("agree") is None:
            verdict = row["detail"]
        elif row["agree"]:
            verdict = "identical"
        else:
            verdict = f"DIFFERS: {row['detail']}"
        counts = f"{row.get('adapter', '-'):>8} {row.get('bridge', '-'):>7}"
        print(f"{row['fact']:<16} {counts}  {verdict}")
    print()
    compared = [r for r in results if r.get("agree") is not None]
    if status == 0:
        print(f"PASS: {len(compared)} facts agree between the live bridge and the Rust adapter")
    elif status == 2:
        print("INCOMPLETE: at least one fact could not be compared; that is not a pass", file=sys.stderr)
    else:
        print("FAIL: the two stacks disagree; do not switch a client yet", file=sys.stderr)
    return status


if __name__ == "__main__":
    sys.exit(main())
