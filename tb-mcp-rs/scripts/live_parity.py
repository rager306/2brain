#!/usr/bin/env python3
"""Compare the installed 2brain binary with the one this tree builds, on the served vault.

What this proves now: the binary a client launches is the binary the tree builds. Before the
TypeScript layer was deleted (2026-11-16) this script compared the Python bridge with the adapter;
both bridge and daemon are gone, so the second process is the freshly built `au-mcp-rs` binary and
the comparison answers a different question — "is what is installed still what the source says".

Read-only by construction: both servers are started WITHOUT `--allow-mutations`, so neither can
advertise a mutation tool and the engine never receives a `mutate` frame; no signal is sent to any
daemon, and nothing in the vault is written.

    python3 scripts/live_parity.py                       # auto-detect the engine socket
    python3 scripts/live_parity.py --socket /path.sock
    python3 scripts/live_parity.py --json                # machine-readable

Exit 0 when every compared read agrees, 1 when one differs, 2 when the comparison could not be made
(a socket or binary missing, a server refusing the handshake) — a comparison that did not happen is
not a pass.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent / "lib"))
from mcp_stdio import StdioMcp, StdioMcpError, newest_socket, tool_payload  # noqa: E402

CRATE = Path(__file__).resolve().parent.parent
# Both are the unified binary (`2brain mcp <target>`), so the argv is identical and the only difference
# between the two processes is which build of the same code runs.
BUILT = os.environ.get("AU_2BRAIN_BUILT", str(CRATE / "target/release/2brain"))
INSTALLED = os.environ.get("AU_2BRAIN_BIN", "/usr/local/bin/2brain")

# Each row: a name, the tool, its arguments, and how to reduce the answer to the facts that must
# match. The two processes run the same code, so a difference here means the installed binary is
# older (or newer) than the tree — which is exactly the failure this script caught in practice.
FACTS = [
    ("members", "tb_members", {}, "repo"),
    ("top_level_dirs", "tb_top_level_dirs", {}, "repo:name"),
    ("types", "tb_types", {"limit": 500}, "name:repo:hash"),
    ("hubs", "tb_hubs", {"limit": 20}, "path:refs_total"),
    ("diagnostics", "tb_diagnostics", {"limit": 200}, "path:code:severity"),
    ("search", "tb_search", {"query": "wikilink", "limit": 10}, "path"),
]


def rows(payload) -> list[dict]:
    """The list a tool answered with, whether wrapped or bare."""
    if isinstance(payload, list):
        return payload
    if isinstance(payload, dict):
        for key in ("members", "top_level_dirs", "types", "hubs", "diagnostics", "matches", "search"):
            value = payload.get(key)
            if isinstance(value, list):
                return value
            if isinstance(value, dict) and isinstance(value.get("matches"), list):
                return value["matches"]
    raise ValueError(f"no list in the answer: {json.dumps(payload)[:200]}")


def reduce_rows(payload, shape: str) -> list[str]:
    keys = shape.split(":")
    out = []
    for row in rows(payload):
        parts = []
        for key in keys:
            if key == "repo:name":
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
        return f"only in the installed binary: {only_left[:3]}"
    if only_right:
        return f"only in the built binary: {only_right[:3]}"
    if left != right:
        return "same rows, different order"
    return ""


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--socket",
        help="engine socket (default: the newest under the engine's run directory, "
        "which is <state dir>/au-engine/run for whichever spelling the engine uses)",
    )
    parser.add_argument("--json", action="store_true", help="machine-readable result")
    args = parser.parse_args()

    # The run directory is found, not spelled: the engine names its state root, and a script
    # that joins a literal name stops finding the daemon the day that rule changes.
    if args.socket:
        socket: Path | None = Path(args.socket)
    else:
        found = newest_socket(Path.home())
        socket = Path(found) if found else None
    if socket is None or not socket.exists():
        print("no engine socket: nothing to read", file=sys.stderr)
        return 2
    for label, binary in (("installed", INSTALLED), ("built", BUILT)):
        if not Path(binary).exists():
            print(f"missing {label} binary at {binary}", file=sys.stderr)
            return 2

    results = []
    status = 0
    mutations = {"tb_write_file", "tb_edit_file", "tb_delete_file", "tb_batch",
                 "tb_create_file", "tb_rename", "tb_rename_type"}
    try:
        with StdioMcp([INSTALLED, "mcp", str(socket)], name="installed") as left, \
             StdioMcp([BUILT, "mcp", str(socket)], name="built") as right:
            for server, label in ((left, "installed"), (right, "built")):
                tools = {t["name"] for t in server.request("tools/list", {})["result"]["tools"]}
                if tools & mutations:
                    results.append({"fact": "read-only launch", "agree": False,
                                    "detail": f"{label}: a mutation tool is advertised: "
                                              f"{sorted(tools & mutations)}"})
                    status = 1
            for name, tool, call_args, shape in FACTS:
                def ask(server):
                    """Rows on success, or the error text — an identical refusal is agreement.

                    This matters for a verb the engine does not know yet: a daemon that predates a
                    read refuses it for both processes, and the honest answer is "both refused the
                    same way", not "the comparison failed".
                    """
                    try:
                        reply = server.call(tool, call_args)
                    except StdioMcpError as error:
                        return ("failed", str(error))
                    try:
                        return ("rows", reduce_rows(tool_payload(reply), shape))
                    except ValueError:
                        # An error answer arrives as text, so `tool_payload` cannot parse it; the
                        # text itself is the comparable fact.
                        try:
                            return ("refused", reply["result"]["content"][0]["text"])
                        except (KeyError, IndexError, TypeError):
                            return ("refused", json.dumps(reply)[:400])

                kind_a, a = ask(left)
                kind_b, b = ask(right)
                if kind_a == "rows" and kind_b == "rows":
                    difference = first_difference(a, b)
                    results.append({"fact": name, "agree": not difference,
                                    "detail": difference or f"{len(a)} rows"})
                    if difference:
                        status = 1
                elif kind_a == kind_b == "refused":
                    same = a == b
                    results.append({"fact": name, "agree": same,
                                    "detail": "both refused: the engine predates this verb"
                                              if same else "both refused, differently"})
                    if not same:
                        status = 1
                else:
                    results.append({"fact": name, "agree": False,
                                    "detail": f"installed={kind_a}, built={kind_b}: {a if kind_a != 'rows' else b}"})
                    status = 1
    except StdioMcpError as error:
        print(f"a server refused the handshake: {error}", file=sys.stderr)
        return 2

    if args.json:
        print(json.dumps({"socket": str(socket), "results": results, "status": status}, indent=2))
    else:
        for row in results:
            mark = "identical" if row["agree"] else "DIFFERENT"
            print(f"{row['fact']:<16} {mark:<10} {row['detail']}")
        print("PASS" if status == 0 else "FAIL")
    return status


if __name__ == "__main__":
    sys.exit(main())
