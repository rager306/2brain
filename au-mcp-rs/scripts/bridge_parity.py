#!/usr/bin/env python3
"""Compare the Rust adapter's surface with the existing Python bridge.

The bridge speaks to the Node `au-mcp` daemon, which projects its own tool shapes
(a diagnostics tool returns a summary and paging on top of the engine read), so a
byte-for-byte comparison would be wrong. This report compares what a cutover
actually depends on:

- the two tool surfaces, classified as shared facts, daemon-only, or rust-only;
- the facts both surfaces can state: diagnostic codes, severities, and spans.

It runs against a disposable workspace that mounts the bundled plugin package
(`au-mcp-core`), because the daemon core compiles in no tools: its surface comes
from `mcp.tool` plugins discovered in the served workspace.

    AU_ENGINE_BIN=../au-engine/target/debug/au python3 scripts/bridge_parity.py
"""

import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

ENGINE = os.environ.get("AU_ENGINE_BIN", "../au-engine/target/debug/au")
ADAPTER = os.environ.get("AU_MCP_RS_BIN", "target/debug/au-mcp-rs")
BRIDGE = os.environ.get(
    "AU_BRIDGE", str(Path(__file__).resolve().parents[2] / "au-mcp-bridge/bridge.py")
)
BUNDLE = Path(__file__).resolve().parents[2]
TIMEOUT = 60.0

# The daemon tool that states the same fact as a Rust tool, and how to project
# each onto the comparable fact set.
SHARED = {
    "mcp__au_diagnostics": ("au_diagnostics", "diagnostics"),
}


class Stdio:
    """Minimal MCP stdio client."""

    def __init__(self, command: list[str], env: dict | None = None) -> None:
        self.process = subprocess.Popen(
            command,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            bufsize=1,
            env=env,
        )
        reply = self.request(
            1,
            "initialize",
            {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "bridge-parity", "version": "1"},
            },
        )
        assert "result" in reply, reply
        self.send({"jsonrpc": "2.0", "method": "notifications/initialized"})

    def send(self, message: dict) -> None:
        assert self.process.stdin is not None
        self.process.stdin.write(json.dumps(message) + "\n")
        self.process.stdin.flush()

    def request(self, request_id: int, method: str, params: dict) -> dict:
        self.send({"jsonrpc": "2.0", "id": request_id, "method": method, "params": params})
        assert self.process.stdout is not None
        line = self.process.stdout.readline()
        if not line:
            stderr = self.process.stderr.read() if self.process.stderr else ""
            raise SystemExit(f"stdio server closed its output: {stderr[-2000:]}")
        return json.loads(line)

    def tools(self) -> "list[str]":
        reply = self.request(2, "tools/list", {})
        return sorted(tool["name"] for tool in reply["result"]["tools"])

    def call(self, request_id: int, name: str, arguments: dict) -> dict:
        reply = self.request(
            request_id, "tools/call", {"name": name, "arguments": arguments}
        )
        result = reply.get("result") or {}
        if result.get("isError"):
            return {"error": result["content"][0]["text"]}
        return json.loads(result["content"][0]["text"])

    def close(self) -> None:
        self.process.kill()
        self.process.wait()


def engine_read(socket: Path, request: dict) -> dict:
    import socket as socket_module
    import struct

    with socket_module.socket(socket_module.AF_UNIX) as connection:
        connection.settimeout(TIMEOUT)
        connection.connect(str(socket))
        body = json.dumps(request).encode()
        connection.sendall(struct.pack(">I", len(body)) + body)
        (length,) = struct.unpack(">I", connection.recv(4))
        return json.loads(connection.recv(length))


def commit(directory: Path) -> None:
    run = ["git", "-C", str(directory), "-c", "user.name=parity",
           "-c", "user.email=parity@example.invalid", "-c", "commit.gpgsign=false"]
    subprocess.run(["git", "-C", str(directory), "init", "-q"], check=True)
    subprocess.run(["git", "-C", str(directory), "add", "-A"], check=True)
    subprocess.run([*run, "commit", "-q", "-m", "parity"], check=True)


def build_workspace(root: Path) -> Path:
    """A fixture workspace that mounts the bundled plugin package as a dependency."""
    for name in ("au-mcp-core", "au-mcp-sdk"):
        target = root / name
        shutil.copytree(BUNDLE / name, target, ignore=shutil.ignore_patterns(".git", "node_modules"))
        commit(target)
    repo = root / "fixture"
    (repo / ".arsumbris").mkdir(parents=True)
    (repo / "notes").mkdir()
    (repo / ".arsumbris/repo.yaml").write_text(
        "name: fixture\ndeps:\n  - name: au-mcp-core\n  - name: au-mcp-sdk\n"
    )
    (repo / "notes/a.md").write_text("---\ntype: note\n---\n")
    (repo / "type").mkdir()
    (repo / "type/note.type.yaml").write_text("fields:\n  title: String\n")
    commit(repo)
    return repo


def wait_for(directory: Path, deadline: float) -> Path | None:
    while time.monotonic() < deadline:
        for candidate in directory.glob("*.sock") if directory.exists() else []:
            return candidate
        time.sleep(0.05)
    return None


def diagnostics_facts(payload: dict) -> "set[tuple]":
    """The comparable part: code, severity, and where each finding points."""
    facts = set()
    for diagnostic in payload.get("diagnostics") or []:
        span = diagnostic.get("span") or {}
        line_col = span.get("line_col") or {}
        start = line_col.get("start") or {}
        facts.add(
            (
                diagnostic.get("code"),
                diagnostic.get("severity"),
                Path(span.get("file", "")).name,
                start.get("line"),
            )
        )
    return facts


def main() -> int:
    for path, label in ((ENGINE, "engine"), (ADAPTER, "adapter")):
        if not os.access(path, os.X_OK):
            raise SystemExit(f"missing {label} binary at {path}")
    if shutil.which("bun") is None:
        raise SystemExit("bun is required to run the Node daemon")

    root = Path(tempfile.mkdtemp(prefix="au-parity-"))
    home = root / "home"
    home.mkdir()
    repo = build_workspace(root)
    env = dict(os.environ, HOME=str(home))
    processes = []
    try:
        engine = subprocess.Popen(
            [ENGINE, "daemon", "start", str(repo)],
            env=env, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        processes.append(engine)
        engine_socket = wait_for(home / ".arsumbris/au-engine/run", time.monotonic() + TIMEOUT)
        if engine_socket is None:
            raise SystemExit("the disposable engine never became ready")

        daemon = subprocess.Popen(
            ["bun", "run", "src/cli.ts", "start", str(repo)],
            cwd=str(BUNDLE / "au-mcp"), env=env,
            stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        processes.append(daemon)
        daemon_socket = wait_for(home / ".arsumbris/au-mcp/run", time.monotonic() + TIMEOUT)
        if daemon_socket is None:
            raise SystemExit("the Node daemon never became ready")

        bridge = Stdio([sys.executable, BRIDGE], env=dict(env, AU_MCP_SOCKET=str(daemon_socket)))
        rust = Stdio([ADAPTER, str(engine_socket)])
        try:
            bridge_tools = set(bridge.tools())
            rust_tools = set(rust.tools())
        finally:
            pass

        print(f"bridge tools: {len(bridge_tools)}")
        print(f"rust tools:   {len(rust_tools)}")
        shared = sorted(set(SHARED) & bridge_tools)
        rust_only = sorted(rust_tools - {rust_name for _, (rust_name, _) in SHARED.items()})
        daemon_only = sorted(bridge_tools - set(SHARED))
        print(f"\ncomparable pairs: {shared or 'none'}")
        if daemon_only:
            print(f"daemon-only: {', '.join(daemon_only)}")
        if rust_only:
            print(f"rust-only:   {', '.join(rust_only)}")

        status = 0
        for bridge_name in shared:
            rust_name, fact = SHARED[bridge_name]
            # The daemon's diagnostics tool filters by severity and defaults to
            # one value, so the comparable fact set is the union over the
            # severities the engine can emit.
            bridge_facts = set()
            for severity in ("error", "drift", "warning", "hint"):
                answer = bridge.call(10, bridge_name, {"severity": severity, "limit": 500})
                if "error" not in answer:
                    bridge_facts |= diagnostics_facts(answer)
            from_rust = rust.call(11, rust_name, {"limit": 500})
            from_engine = engine_read(engine_socket, {"read": fact, "id": 1})["result"]
            rust_facts = diagnostics_facts(from_rust)
            engine_facts = diagnostics_facts(from_engine)
            if not (bridge_facts == rust_facts == engine_facts):
                status = 1
                print(f"MISMATCH {bridge_name} vs {rust_name}:")
                print(f"  bridge: {sorted(bridge_facts)}")
                print(f"  rust:   {sorted(rust_facts)}")
                print(f"  engine: {sorted(engine_facts)}")
            else:
                print(
                    f"MATCH {bridge_name} <-> {rust_name}: "
                    f"{len(engine_facts)} identical findings (code, severity, file, line), "
                    "daemon unioned over its severity filter"
                )
        bridge.close()
        rust.close()
        if status == 0:
            print("\nPASS: the compared facts agree across the bridge, the adapter, and the engine")
        return status
    finally:
        for process in processes:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
        shutil.rmtree(root, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main())
