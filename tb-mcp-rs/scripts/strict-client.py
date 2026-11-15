#!/usr/bin/env python3
"""Prove that a client which VALIDATES `structuredContent` accepts a mutation answer.

Our Rust tests compare payload keys against the declared schema. A client does something
stricter: it validates the whole answer against `outputSchema` and refuses the call when
they disagree. That is how the `commits` bug surfaced — a live Hermes session wrote a
file, the engine applied it, and the answer came back as "Invalid structured content".

This runs the same check with the same SDK the clients use, against a disposable engine,
so it needs no user session:

    /root/.allspark-home/.prime/agent/kernel-venv/bin/python scripts/strict-client.py

Exit 0 means a mutation answer passed client-side validation. Always disposable: the
fixture engine is a temp directory, never the served vault.
"""

from __future__ import annotations

import asyncio
import glob
import json
import os
import subprocess
import sys
import tempfile
import time
from pathlib import Path

ENGINE = os.environ.get("AU_ENGINE_BIN", "/root/2brain/tb-engine/target/release/au")
ADAPTER = os.environ.get(
    "AU_MCP_RS_BIN", str(Path(__file__).resolve().parent.parent / "target/release/2brain"))


def disposable() -> tuple[str, dict, str]:
    root = tempfile.mkdtemp(prefix="au-strict-")
    home, repo = os.path.join(root, "home"), os.path.join(root, "repo")
    for directory in (home, os.path.join(repo, ".arsumbris"), os.path.join(repo, "notes")):
        os.makedirs(directory, exist_ok=True)
    Path(repo, ".arsumbris/repo.yaml").write_text("name: fixture\n")
    Path(repo, "notes/a.md").write_text("---\ntype: note\ntitle: t\n---\n\nplain\n")
    git = ["git", "-c", "user.name=f", "-c", "user.email=f@x", "-c", "commit.gpgsign=false"]
    subprocess.run(["git", "init", "-q"], cwd=repo, check=True)
    subprocess.run(["git", "add", "-A"], cwd=repo, check=True)
    subprocess.run([*git, "commit", "-q", "-m", "fixture"], cwd=repo, check=True)
    return repo, dict(os.environ, HOME=home), home


async def main() -> int:
    from mcp import ClientSession, StdioServerParameters
    from mcp.client.stdio import stdio_client

    repo, env, home = disposable()
    daemon = subprocess.Popen([ENGINE, "daemon", "start", repo], env=env,
                              stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        socket = None
        for _ in range(100):
            found = glob.glob(home + "/.arsumbris/au-engine/run/*.sock")
            if found:
                socket = found[0]
                break
            time.sleep(0.1)
        if socket is None:
            print("FAIL: the disposable engine never became ready", file=sys.stderr)
            return 2

        params = StdioServerParameters(command=ADAPTER, args=["mcp", socket, "--allow-mutations"])
        failures = 0
        async with stdio_client(params) as (read, write), ClientSession(read, write) as session:
            await session.initialize()
            read_answer = await session.call_tool(
                "au_content", {"repo": "fixture", "path": "notes/a.md"})
            body = json.loads(read_answer.content[0].text)["content"]
            write_args = {"repo": "fixture", "path": "notes/a.md",
                          "content": body["text"] + "changed\n",
                          "expected_hash": body["hash"]}
            try:
                answer = await session.call_tool("au_write_file", write_args)
            except Exception as error:  # noqa: BLE001 - the failure IS the result
                print(f"FAIL: the client rejected the mutation answer: {type(error).__name__}: {error}")
                return 1

            # Reaching here means the SDK validated the answer against `outputSchema`.
            payload = (getattr(answer, "structuredContent", None)
                       or getattr(answer, "structured_content", None) or {})
            commits = payload.get("commits")
            print("PASS: the client accepted a mutation answer")
            print("  keys:", ", ".join(sorted(payload)))
            print(f"  commits: {type(commits).__name__} {commits}")
            if not isinstance(commits, dict):
                print("FAIL: commits is not an object; the schema and the engine disagree")
                failures += 1
            if not payload.get("hash"):
                print("FAIL: the answer carries no content hash")
                failures += 1

            # A delete answers `hash: null`, and its schema must say so: declaring a
            # plain string broke delete at a validating client right after the write
            # path was fixed. The write invalidated the read, so delete needs a fresh
            # one — that is the guard working, not an obstacle to route around.
            fresh = await session.call_tool(
                "au_content", {"repo": "fixture", "path": "notes/a.md"})
            fresh_hash = json.loads(fresh.content[0].text)["content"]["hash"]
            try:
                delete_answer = await session.call_tool(
                    "au_delete_file",
                    {"repo": "fixture", "path": "notes/a.md", "expected_hash": fresh_hash})
            except Exception as error:  # noqa: BLE001 - the failure IS the result
                print(f"FAIL: the client rejected the delete answer: {type(error).__name__}: {error}")
                return 1
            deleted = (getattr(delete_answer, "structuredContent", None)
                       or getattr(delete_answer, "structured_content", None) or {})
            if getattr(delete_answer, "isError", False):
                print(f"FAIL: the delete was refused: {delete_answer.content[0].text[:200]}")
                return 1
            print("PASS: the client accepted a delete answer")
            print("  keys:", ", ".join(sorted(deleted)))
            print("  hash:", repr(deleted.get("hash")))
            if deleted.get("hash") is not None:
                print("FAIL: a deleted file should report hash: null")
                failures += 1
        return 1 if failures else 0
    finally:
        daemon.terminate()
        daemon.wait(timeout=10)


if __name__ == "__main__":
    sys.exit(asyncio.run(main()))
