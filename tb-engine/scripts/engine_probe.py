#!/usr/bin/env python3
"""Capture engine-level golden reads from a running 2brain daemon.

Read-only by construction: it speaks the engine wire and sends only `read`
requests. Nothing is written to the vault, no mutation verb is ever sent, and the
socket is opened per request.

Why this exists next to `tests-golden/*.json`: those three files were captured
through the Python bridge on the production stack before the fork's first engine
change, so their shapes are the Node daemon's projections (`summary`, `severity`,
a bare hub array). The daemon is being retired; a contract that outlives it has to
be phrased in the engine's own answers, which is what this captures.

    ./tb-engine/scripts/engine_probe.py \
        --socket ~/.arsumbris/au-engine/run/<hash>.sock \
        --out tb-engine/tests-golden/engine

Every response is normalized so a re-capture on a clean tree diffs only when the
engine changed: the workspace root becomes `$VAULT`, the observed `version` is
dropped (it counts rebuilds), and JSON keys are sorted.
"""

from __future__ import annotations

import argparse
import json
import socket
import struct
import sys
from pathlib import Path

# One frame may not exceed this, matching the engine's own bound.
MAX_FRAME = 16 * 1024 * 1024

# The reads captured by default: the shape of the workspace, its navigation
# crossroads, its problem list, the graph's hygiene summary, and a ranked search
# answer. The graph_health limit matches `tests-golden/README.md`.
DEFAULT_VERBS = {
    "tb_overview": {"read": "overview", "id": 1},
    "tb_hubs": {"read": "hubs", "id": 1, "limit": 20, "offset": 0},
    "tb_diagnostics": {"read": "diagnostics", "id": 1, "limit": 50, "offset": 0},
    "tb_graph_health": {"read": "graph_health", "id": 1, "limit": 20},
    # A ranked read: `matches` carries a BM25 score and the distinct-term count,
    # and `next_offset` is null on the last page. The scores are only comparable
    # against the corpus captured here, so re-capture against a frozen vault.
    "tb_search": {"read": "search", "id": 1, "query": "engine", "limit": 20, "offset": 0},
}


def exchange(socket_path: Path, request: dict) -> dict:
    body = json.dumps(request).encode()
    if len(body) > MAX_FRAME:
        raise SystemExit(f"request frame too large: {len(body)} bytes")
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as sock:
        sock.settimeout(120)
        sock.connect(str(socket_path))
        sock.sendall(struct.pack(">I", len(body)) + body)

        def read_exactly(count: int) -> bytes:
            chunks = []
            remaining = count
            while remaining:
                chunk = sock.recv(remaining)
                if not chunk:
                    raise SystemExit("engine closed the connection mid-frame")
                chunks.append(chunk)
                remaining -= len(chunk)
            return b"".join(chunks)

        (length,) = struct.unpack(">I", read_exactly(4))
        if length > MAX_FRAME:
            raise SystemExit(f"response frame too large: {length} bytes")
        return json.loads(read_exactly(length))


def normalize(value, root: str):
    if isinstance(value, dict):
        return {k: normalize(v, root) for k, v in sorted(value.items()) if k != "version"}
    if isinstance(value, list):
        return [normalize(v, root) for v in value]
    if isinstance(value, str) and root and root in value:
        return value.replace(root, "$VAULT")
    return value


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--socket", required=True, type=Path)
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument(
        "--root",
        default=None,
        help="workspace root to replace with $VAULT (default: /root/vault)",
    )
    parser.add_argument(
        "--verb",
        action="append",
        choices=sorted(DEFAULT_VERBS),
        help="capture only these verbs (repeatable)",
    )
    args = parser.parse_args()

    root = args.root if args.root is not None else "/root/vault"
    verbs = args.verb or sorted(DEFAULT_VERBS)
    args.out.mkdir(parents=True, exist_ok=True)

    captured = 0
    for name in verbs:
        response = exchange(args.socket, DEFAULT_VERBS[name])
        if response.get("type") != "response" or not response.get("ready"):
            raise SystemExit(f"{name}: engine answered {response.get('type')!r}, not a ready response")
        payload = normalize(response.get("result"), root)
        path = args.out / f"{name}.json"
        path.write_text(json.dumps(payload, indent=1, sort_keys=True) + "\n")
        print(f"{name}: {path} ({path.stat().st_size} bytes)")
        captured += 1
    print(f"captured {captured} reads into {args.out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
