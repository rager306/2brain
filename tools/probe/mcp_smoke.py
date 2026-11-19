#!/usr/bin/env python3
"""Acceptance smoke for an installed 2brain: MCP reads and both write paths, over stdio.

    python3 tools/probe/mcp_smoke.py --binary /usr/local/bin/2brain \
        --socket <state root>/engine/run/<hash>.sock --repo <member>

Speaks MCP the way a client does: `initialize`, then `tools/call`. It checks a read, a clean
create that must land, the hash a read returns, a gated write that must land, and a create with
`require_clean` that must be refused with `policy/diagnostics` naming its code. Exit code 1 on
the first failed check, so it can be the last line of an install script.

Run 2026-11-18 against a binary installed from a clean checkout: 8 checks, all passed
(`docs/internal/install-from-checkout-2026-11-18.md`).
"""
from __future__ import annotations

import argparse
import json
import subprocess
import sys
from pathlib import Path


class Server:
    def __init__(self, argv: list[str]):
        self.process = subprocess.Popen(
            argv, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True
        )
        self.next_id = 0

    def send(self, message: dict) -> None:
        assert self.process.stdin and self.process.stdout
        self.process.stdin.write(json.dumps(message) + "\n")
        self.process.stdin.flush()

    def await_id(self, wanted: int, limit: int = 50) -> dict:
        assert self.process.stdout
        for _ in range(limit):
            line = self.process.stdout.readline()
            if not line:
                break
            frame = json.loads(line)
            if frame.get("id") == wanted:
                return frame
        raise SystemExit(f"no reply for id {wanted}")

    def call(self, method: str, params: dict) -> dict:
        self.next_id += 1
        self.send({"jsonrpc": "2.0", "id": self.next_id, "method": method, "params": params})
        return self.await_id(self.next_id)

    def tool(self, name: str, arguments: dict) -> dict:
        reply = self.call("tools/call", {"name": name, "arguments": arguments})
        if "error" in reply:
            raise SystemExit(f"{name}: JSON-RPC error {reply['error']}")
        return reply["result"]

    def close(self) -> None:
        assert self.process.stdin
        self.process.stdin.close()
        self.process.wait(timeout=10)


def payload(result: dict) -> dict:
    text = result["content"][0]["text"]
    return json.loads(text)


def check(condition: bool, message: str) -> None:
    if not condition:
        raise SystemExit(f"FAIL {message}")
    print(f"ok   {message}")


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--socket", type=Path, required=True)
    parser.add_argument("--repo", required=True)
    parser.add_argument("--note", default="smoke.md")
    args = parser.parse_args(argv)

    server = Server(
        [str(args.binary), "mcp", str(args.socket), "--allow-mutations", "--payload", "text"]
    )
    init = server.call(
        "initialize",
        {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": {"name": "mcp-smoke", "version": "0"},
        },
    )
    check("result" in init, "initialize answers")
    server.send({"jsonrpc": "2.0", "method": "notifications/initialized", "params": {}})

    overview = payload(server.tool("tb_overview", {}))
    check("overview" in overview, "a read answers through the installed binary")

    good = f"---\ntype: note\ndescription: written by mcp_smoke\n---\n"
    created = payload(
        server.tool("tb_create_file", {"repo": args.repo, "path": args.note, "content": good})
    )
    check(created.get("path", "").endswith(args.note), "a clean create lands")

    read = payload(server.tool("tb_content", {"repo": args.repo, "path": args.note}))
    content = read["content"]
    check(content["hash"] != "", "a read returns the hash a write needs")

    rewritten = good + "second line\n"
    wrote = payload(
        server.tool(
            "tb_write_file",
            {
                "repo": args.repo,
                "path": args.note,
                "content": rewritten,
                "expected_hash": content["hash"],
            },
        )
    )
    check(wrote.get("hash") != "", "a gated write lands and answers its new hash")

    refused = server.tool(
        "tb_create_file",
        {
            "repo": args.repo,
            "path": "refused.md",
            "content": "---\ntype: note::ghost\ndescription: must not land\n---\n",
            "require_clean": True,
        },
    )
    check(refused.get("isError") is True, "require_clean refuses through the installed binary")
    text = refused["content"][0]["text"]
    check("policy/diagnostics" in text, "the refusal names its stage")
    check("type-repo-unknown" in text, "the refusal names the code")

    server.close()
    print("smoke passed")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
