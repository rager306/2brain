#!/usr/bin/env python3
"""A ranker that is worse by construction: every `tb_search` page comes back reversed.

    BRAIN_BIN=tools/ranking/degraded_ranker.py python3 tools/ranking/tasks_eval.py --json \
        --socket <candidate socket> --vault <vault copy>

It is a stdio MCP server that runs `2brain mcp <target>` as a child (`$BRAIN_REAL_ADAPTER`, default
`/usr/local/bin/2brain`) and forwards every message unchanged except one: in a `tb_search` reply the
`matches` array is reversed. Each match keeps its own score, matched-term count and `weak` flag, so
the page still names the same notes with the same evidence; only the order is wrong.

It exists for one purpose: the control arm for `bootstrap.py`. A statistic that cannot call this
pair worse is not measuring anything. It is not a serving path, nothing imports it, and it should
only be pointed at a candidate socket whose answers do not matter.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys


def reverse_page(reply: dict) -> dict:
    """Reverse the match order in both places a reply carries it: structured content and text block."""
    result = reply.get("result")
    if not isinstance(result, dict):
        return reply
    structured = (result.get("structuredContent") or {}).get("search")
    if isinstance(structured, dict) and isinstance(structured.get("matches"), list):
        structured["matches"] = list(reversed(structured["matches"]))
    for block in result.get("content") or []:
        if not isinstance(block, dict) or block.get("type") != "text":
            continue
        try:
            payload = json.loads(block.get("text", ""))
        except ValueError:
            continue
        search = payload.get("search") if isinstance(payload, dict) else None
        if isinstance(search, dict) and isinstance(search.get("matches"), list):
            search["matches"] = list(reversed(search["matches"]))
            block["text"] = json.dumps(payload, ensure_ascii=False)
    return reply


def main() -> int:
    adapter = os.environ.get("BRAIN_REAL_ADAPTER", "/usr/local/bin/2brain")
    child = subprocess.Popen(
        [adapter, *sys.argv[1:]], stdin=subprocess.PIPE, stdout=subprocess.PIPE)
    try:
        for line in iter(sys.stdin.buffer.readline, b""):
            if not line.strip():
                continue
            child.stdin.write(line)
            child.stdin.flush()
            try:
                request = json.loads(line)
            except ValueError:
                request = {}
            if "id" not in request:
                continue  # a notification is never answered
            method = request.get("method")
            tool = (request.get("params") or {}).get("name")
            while True:
                answer = child.stdout.readline()
                if not answer:
                    return 1
                try:
                    reply = json.loads(answer)
                except ValueError:
                    sys.stdout.buffer.write(answer)
                    sys.stdout.buffer.flush()
                    continue
                if reply.get("id") != request["id"]:
                    sys.stdout.buffer.write(answer)
                    sys.stdout.buffer.flush()
                    continue
                if method == "tools/call" and tool == "tb_search":
                    reply = reverse_page(reply)
                sys.stdout.buffer.write((json.dumps(reply, ensure_ascii=False) + "\n").encode())
                sys.stdout.buffer.flush()
                break
    finally:
        child.terminate()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
