#!/usr/bin/env python3
"""Rehearse rolling back from the Rust adapter to the read-only path.

The rehearsal proves the properties a rollback depends on:

1. The adapter keeps no state on disk: everything it writes goes through the
   engine, which commits it, and the fixture's git status stays clean.
2. Stopping the adapter loses nothing: a fresh process reads the written file.
3. Rolling back to read-only is a launch flag, not a migration: the same binary
   without a grant reads the file and refuses to write.

Run with a built `au` engine and adapter:

    AU_ENGINE_BIN=../tb-engine/target/debug/au python3 scripts/rollback_rehearsal.py

Exit status is non-zero on the first failed expectation.
"""

import os
import shutil
import socket
import subprocess
import sys
import tempfile
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent / "lib"))
from mcp_stdio import StdioMcp, is_error, tool_payload

ENGINE = os.environ.get("AU_ENGINE_BIN", "../tb-engine/target/debug/au")
ADAPTER = os.environ.get("AU_MCP_RS_BIN", "target/debug/tb-mcp-rs")
TIMEOUT = 20.0


def adapter(socket_path: Path, *extra: str, name: str) -> StdioMcp:
    """A started stdio session with the adapter."""
    return StdioMcp([ADAPTER, str(socket_path), *extra], name=name, timeout=TIMEOUT).start()


def fail(message: str) -> "None":
    print(f"FAIL: {message}", file=sys.stderr)
    sys.exit(1)


def step(message: str) -> None:
    print(f"ok: {message}")


def main() -> int:
    if not os.access(ENGINE, os.X_OK):
        fail(f"missing engine binary: {ENGINE} (set AU_ENGINE_BIN)")
    if not os.access(ADAPTER, os.X_OK):
        fail(f"missing adapter binary: {ADAPTER} (set AU_MCP_RS_BIN)")

    root = Path(tempfile.mkdtemp(prefix="au-rollback-"))
    home = root / "home"
    repo = root / "fixture"
    home.mkdir()
    for directory in (".arsumbris", "type", "notes"):
        (repo / directory).mkdir(parents=True)
    (repo / ".arsumbris/repo.yaml").write_text("name: fixture\n")
    (repo / "type/note.type.yaml").write_text("fields:\n  title: String\n")
    original = "---\ntype: note\ntitle: before\n---\n"
    (repo / "notes/a.md").write_text(original)
    git = ["git", "-C", str(repo), "-c", "user.name=rehearsal",
           "-c", "user.email=rehearsal@example.invalid", "-c", "commit.gpgsign=false"]
    subprocess.run(["git", "-C", str(repo), "init", "-q"], check=True)
    subprocess.run(["git", "-C", str(repo), "add", "-A"], check=True)
    subprocess.run([*git, "commit", "-q", "-m", "fixture"], check=True)
    def fixture_files() -> "list[Path]":
        """Everything the repository holds, excluding git and engine runtime state."""
        return sorted(
            path.relative_to(repo)
            for path in repo.rglob("*")
            if ".git" not in path.parts and "au-engine" not in path.parts
        )

    before_files = fixture_files()

    env = dict(os.environ, HOME=str(home))
    daemon = subprocess.Popen(
        [ENGINE, "daemon", "start", str(repo)],
        env=env,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    try:
        run_dir = home / ".arsumbris/tb-engine/run"
        deadline = time.monotonic() + TIMEOUT
        socket_path = None
        while time.monotonic() < deadline:
            for candidate in run_dir.glob("*.sock") if run_dir.exists() else []:
                ready = socket.socket(socket.AF_UNIX)
                try:
                    ready.connect(str(candidate))
                    socket_path = candidate
                except OSError:
                    pass
                finally:
                    ready.close()
                if socket_path:
                    break
            if socket_path:
                break
            time.sleep(0.05)
        if socket_path is None:
            fail("the disposable engine never became ready")

        # 1. A granted adapter writes through the engine.
        writer = adapter(socket_path, "--allow-mutations", name="rollback-writer")
        read = tool_payload(writer.call("au_content", {"repo": "fixture", "path": "notes/a.md"}))
        assert read["content"]["hash"], read
        written = "---\ntype: note\ntitle: after\n---\n"
        result = writer.call(
            "au_write_file",
            {
                "repo": "fixture",
                "path": "notes/a.md",
                "content": written,
                "expected_hash": read["content"]["hash"],
            },
        )
        if is_error(result):
            fail(f"the write was refused: {result}")
        step("a granted adapter writes through the engine")
        writer.close()

        # 2. The write is an engine commit, not adapter state.
        log = subprocess.run([*git, "log", "--oneline"], capture_output=True, text=True, check=True)
        if len(log.stdout.strip().splitlines()) != 2:
            fail(f"expected the engine to add one commit, saw: {log.stdout}")
        status = subprocess.run(
            ["git", "-C", str(repo), "status", "--porcelain"], capture_output=True, text=True, check=True
        )
        if status.stdout.strip():
            fail(f"the fixture is not clean after the write: {status.stdout}")
        step("the write is an engine commit and the tree is clean")

        # 3. No adapter artefacts on disk.
        after_files = fixture_files()
        if after_files != before_files:
            fail(f"the adapter left files behind: {set(after_files) - set(before_files)}")
        step("the adapter left no files behind")

        # 4. A fresh read-only process still reads the write and refuses to write.
        reader = adapter(socket_path, name="rollback-reader")
        again = tool_payload(reader.call("au_content", {"repo": "fixture", "path": "notes/a.md"}))
        if again["content"]["text"] != written:
            fail(f"a fresh adapter read {again['content']['text']!r}")
        step("a fresh process reads the written file")
        refused = reader.call(
            "au_write_file",
            {
                "repo": "fixture",
                "path": "notes/a.md",
                "content": "x",
                "expected_hash": again["content"]["hash"],
            },
        )
        # A read-only adapter refuses at the protocol layer, before the tool runs.
        if not is_error(refused):
            fail(f"a read-only adapter accepted a write: {refused}")
        step("rolling back to read-only is a launch flag: the write is refused")
        reader.close()

        print("PASS: rollback to read-only loses nothing and needs no migration")
        return 0
    finally:
        daemon.terminate()
        try:
            daemon.wait(timeout=5)
        except subprocess.TimeoutExpired:
            daemon.kill()
        shutil.rmtree(root, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main())
