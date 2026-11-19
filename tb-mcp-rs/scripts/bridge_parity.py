#!/usr/bin/env python3
"""RETIRED 2026-11-16: the Python bridge and the Node daemon this compared are deleted.

Kept as the record behind `docs/bridge-parity.md`. The live equivalent that still runs is
`live_parity.py`, which compares the installed `2brain` binary with the one this tree builds.
"""

import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent / "lib"))
from mcp_stdio import StdioMcp, is_error, tool_payload

ENGINE = os.environ.get("TB_ENGINE_BIN", "../tb-engine/target/debug/tb-engine")
ADAPTER = os.environ.get("TB_MCP_BIN", "target/debug/tb-mcp-rs")
BRIDGE = os.environ.get(
    "TB_BRIDGE", str(Path(__file__).resolve().parents[2] / "tb-mcp-bridge/bridge.py")
)
BUNDLE = Path(__file__).resolve().parents[2]
TIMEOUT = 60.0

# The daemon tool that states the same fact as a Rust tool, and how to project
# each onto the comparable fact set.
SHARED = {
    "mcp__au_diagnostics": ("tb_diagnostics", "diagnostics"),
}


def tools(client: StdioMcp) -> "list[str]":
    """The tool names a server reports."""
    reply = client.request("tools/list", {})
    return sorted(tool["name"] for tool in reply["result"]["tools"])


def engine_read(socket: Path, request: dict) -> dict:
    import socket as socket_module
    import struct

    def read_exactly(connection, count: int) -> bytes:
        # A socket may deliver a frame in pieces; one recv is not the frame.
        chunks, remaining = [], count
        while remaining:
            chunk = connection.recv(remaining)
            if not chunk:
                raise SystemExit(f"the engine closed the socket after {count - remaining} of {count} bytes")
            chunks.append(chunk)
            remaining -= len(chunk)
        return b"".join(chunks)

    with socket_module.socket(socket_module.AF_UNIX) as connection:
        connection.settimeout(TIMEOUT)
        connection.connect(str(socket))
        body = json.dumps(request).encode()
        connection.sendall(struct.pack(">I", len(body)) + body)
        (length,) = struct.unpack(">I", read_exactly(connection, 4))
        return json.loads(read_exactly(connection, length))


def commit(directory: Path) -> None:
    run = ["git", "-C", str(directory), "-c", "user.name=parity",
           "-c", "user.email=parity@example.invalid", "-c", "commit.gpgsign=false"]
    subprocess.run(["git", "-C", str(directory), "init", "-q"], check=True)
    subprocess.run(["git", "-C", str(directory), "add", "-A"], check=True)
    subprocess.run([*run, "commit", "-q", "-m", "parity"], check=True)


def build_workspace(root: Path) -> Path:
    """A fixture workspace that mounts the bundled plugin package as a dependency."""
    for name in ("tb-mcp-core", "tb-mcp-sdk"):
        target = root / name
        shutil.copytree(BUNDLE / name, target, ignore=shutil.ignore_patterns(".git", "node_modules"))
        commit(target)
    repo = root / "fixture"
    (repo / ".2brain").mkdir(parents=True)
    (repo / "notes").mkdir()
    (repo / ".2brain/repo.yaml").write_text(
        "name: fixture\ndeps:\n  - name: tb-mcp-core\n  - name: tb-mcp-sdk\n"
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

    root = Path(tempfile.mkdtemp(prefix="tb-parity-"))
    home = root / "home"
    home.mkdir()
    repo = build_workspace(root)
    env = dict(os.environ, HOME=str(home))
    processes: list[subprocess.Popen] = []
    clients: list[StdioMcp] = []
    try:
        engine = subprocess.Popen(
            [ENGINE, "daemon", "start", str(repo)],
            env=env, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        processes.append(engine)
        engine_socket = wait_for(home / ".2brain/tb-engine/run", time.monotonic() + TIMEOUT)
        if engine_socket is None:
            raise SystemExit("the disposable engine never became ready")

        daemon = subprocess.Popen(
            ["bun", "run", "src/cli.ts", "start", str(repo)],
            cwd=str(BUNDLE / "tb-mcp"), env=env,
            stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        processes.append(daemon)
        daemon_socket = wait_for(home / ".2brain/tb-mcp/run", time.monotonic() + TIMEOUT)
        if daemon_socket is None:
            raise SystemExit("the Node daemon never became ready")

        bridge = StdioMcp([sys.executable, BRIDGE], name="bridge", timeout=TIMEOUT,
                          env=dict(env, TB_MCP_SOCKET=str(daemon_socket)))
        rust = StdioMcp([ADAPTER, str(engine_socket)], name="adapter", timeout=TIMEOUT)
        clients.extend((bridge, rust))
        bridge.start()
        rust.start()
        bridge_tools = set(tools(bridge))
        rust_tools = set(tools(rust))

        print(f"bridge tools: {len(bridge_tools)}")
        print(f"rust tools:   {len(rust_tools)}")
        shared = sorted(set(SHARED) & bridge_tools)
        rust_only = sorted(rust_tools - {rust_name for _, (rust_name, _) in SHARED.items()})
        daemon_only = sorted(bridge_tools - set(SHARED))
        print(f"\ncomparable pairs: {shared or 'none'}")
        # A run that compares nothing proves nothing, and used to print PASS: the
        # compared pair is the only fact this script establishes.
        if not shared:
            raise SystemExit(
                "no comparable pair: the daemon no longer exposes any tool in SHARED, "
                "so this run would prove nothing"
            )
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
                reply = bridge.call(bridge_name, {"severity": severity, "limit": 500})
                if not is_error(reply):
                    bridge_facts |= diagnostics_facts(tool_payload(reply))
            from_rust = tool_payload(rust.call(rust_name, {"limit": 500}))
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
        # A stdio client owns a child process; an exception between the two closes
        # used to leave the adapter and the bridge running.
        for client in clients:
            try:
                client.close()
            except Exception:
                pass
        for process in processes:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
        shutil.rmtree(root, ignore_errors=True)


if __name__ == "__main__":
    # The packages this needs (tb-mcp-core, tb-mcp-sdk code, tb-mcp-bridge) were deleted on
    # 2026-11-16. Fail with the reason instead of a traceback from a missing path.
    missing = [p for p in (BRIDGE, BUNDLE / "tb-mcp-core") if not Path(p).exists()]
    if missing:
        print("retired: the Python bridge / Node workspace is gone (" +
              ", ".join(str(m) for m in missing) + "); see live_parity.py", file=sys.stderr)
        sys.exit(2)
    sys.exit(main())
