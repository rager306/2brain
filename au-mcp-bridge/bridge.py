#!/usr/bin/env python3
"""MCP stdio bridge to the arsumbris au-mcp daemon (v2).

Speaks standard MCP (JSON-RPC over stdio, as the Hermes MCP client expects)
on this process's stdin/stdout, and translates tool calls to the au-mcp
daemon's native wire protocol on a Unix socket (4-byte big-endian length
prefix + JSON body per frame).

v2 changes (integration hardening):
  - Name round-trip: the daemon registers callables under full manifest ids
    ("mcp.au_overview"); MCP clients forbid "." in tool names, so Hermes
    sanitizes them to "__" at registration. The bridge now advertises SAFE
    names (dots replaced by "__") and maps them BACK to manifest ids on
    tools/call. Bare names (no dot) pass through unchanged.
  - Reconnect: the socket to the daemon is re-established on transport
    failure (daemon restart no longer wedges the bridge; the failing call
    returns a legible isError instead of a broken pipe).
  - Session pinning: tools/list pins ONE daemon session and tools/call
    reuses it (previously every list created a new session).

Config via environment:
  AU_MCP_SOCKET  path to the au-mcp daemon socket (required)
  AU_MCP_NAME    server name reported to the client (default: arsumbris)

The au-mcp socket must already be running:
  au daemon start <workspace>
  node ~/arsumbris/au-mcp/src/cli.ts start <workspace>
"""

import json
import os
import socket
import sys
import threading

SOCKET_PATH = os.environ.get("AU_MCP_SOCKET", "")
SERVER_NAME = os.environ.get("AU_MCP_NAME", "arsumbris")

PROTOCOL_VERSION = "2025-06-18"


def log(msg: str) -> None:
    print(f"[au-bridge] {msg}", file=sys.stderr, flush=True)


def safe_name(manifest_id: str) -> str:
    """MCP-safe tool name: dots are illegal in some clients; Hermes also
    sanitizes non-[a-zA-Z0-9_-] to _. Use '__' for '.' so the mapping is
    injective and reversible."""
    return manifest_id.replace(".", "__")


class AuClient:
    """One daemon connection per bridge process, with reconnect."""

    def __init__(self, socket_path: str):
        self.socket_path = socket_path
        self._lock = threading.Lock()
        self._sock = None
        self._id = 0
        self.session = None

    def _connect(self):
        if self._sock is not None:
            return
        if not self.socket_path:
            raise RuntimeError("AU_MCP_SOCKET is not set")
        s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        s.settimeout(120)
        s.connect(self.socket_path)
        self._sock = s
        opened = self._rpc_locked({"kind": "session-open", "id": self._next_id(),
                                   "info": {"name": SERVER_NAME, "version": "2"}})
        if opened.get("kind") != "opened":
            raise RuntimeError(f"session-open failed: {opened}")
        self.session = opened.get("session")

    def _next_id(self):
        self._id += 1
        return self._id

    def _recv_exact(self, n: int) -> bytes:
        buf = b""
        while len(buf) < n:
            chunk = self._sock.recv(n - len(buf))
            if not chunk:
                raise ConnectionError("au-mcp daemon closed the socket")
            buf += chunk
        return buf

    def _rpc_locked(self, request: dict) -> dict:
        self._connect()
        body = json.dumps(request).encode()
        self._sock.sendall(len(body).to_bytes(4, "big") + body)
        header = self._recv_exact(4)
        length = int.from_bytes(header, "big")
        return json.loads(self._recv_exact(length))

    def rpc(self, request: dict, retry: bool = True) -> dict:
        with self._lock:
            try:
                return self._rpc_locked(request)
            except (OSError, ConnectionError, json.JSONDecodeError) as exc:
                if not retry:
                    raise
                # Stale socket (daemon restart / idle reset): drop and retry once.
                log(f"transport error ({exc}); reconnecting to {self.socket_path}")
                self._drop()
                return self._rpc_locked(request)

    def _drop(self):
        if self._sock is not None:
            try:
                self._sock.close()
            except Exception:
                pass
            self._sock = None

    def close(self):
        with self._lock:
            if self._sock is not None:
                try:
                    self._rpc_locked({"kind": "session-close",
                                      "id": self._next_id(),
                                      "session": self.session or "s1"})
                except Exception:
                    pass
                try:
                    self._sock.close()
                except Exception:
                    pass
                self._sock = None


au = AuClient(SOCKET_PATH)


# --------------------------------------------------------------- MCP surface

def handle_initialize(params: dict) -> dict:
    return {
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": {"tools": {}},
        "serverInfo": {"name": SERVER_NAME, "version": "2.0.0"},
    }


def handle_tools_list(params: dict) -> dict:
    # Pin ONE daemon session for this bridge process; tools/call reuses it so
    # freshness/continuity tracking inside the daemon stays coherent.
    if au.session is None:
        reply = au.rpc({"kind": "list-capabilities", "id": au._next_id(),
                        "session": ""})
        # A session-less advertise is allowed (profile-less launch resolves
        # unrestricted); fall back to opening a real session if the daemon
        # insists on one.
        if reply.get("kind") != "capabilities":
            au._connect()
            reply = au.rpc({"kind": "list-capabilities", "id": au._next_id(),
                            "session": au.session})
    else:
        reply = au.rpc({"kind": "list-capabilities", "id": au._next_id(),
                        "session": au.session})
    tools = []
    for c in reply.get("callables", []):
        if c.get("kind") != "tool":
            continue
        full_id = c["name"]
        tools.append({
            "name": safe_name(full_id),
            "description": (c.get("description") or "") +
                           f" [arsumbris tool, id: {full_id}, access: {c.get('access', '?')}]",
            "inputSchema": c.get("inputSchema") or {"type": "object", "properties": {}},
        })
    return {"tools": tools}


def resolve_tool(name: str) -> str:
    """Map the advertised safe name back to the daemon's full manifest id.
    Exact full-id match wins; then safe-name match; then bare name."""
    if "__" in name or "." in name:
        if name in _advertised:
            return _advertised[name]
    # Bare or unknown: try as-is, then with the mcp. prefix.
    if name in _advertised.values():
        return name
    return f"mcp.{name}"


def handle_tools_call(params: dict) -> dict:
    name = params.get("name", "")
    arguments = params.get("arguments") or {}
    wire_name = resolve_tool(name)
    try:
        reply = au.rpc({"kind": "invoke", "id": au._next_id(),
                        "session": au.session,
                        "tool": wire_name, "input": arguments})
    except Exception as exc:
        return {"content": [{"type": "text", "text": f"au-bridge transport error: {exc}"}],
                "isError": True}
    if reply.get("kind") != "invoked":
        text = reply.get("message") or json.dumps(reply, ensure_ascii=False)
        return {"content": [{"type": "text", "text": f"au-mcp error: {text}"}],
                "isError": True}
    result = reply.get("result")
    if reply.get("isError"):
        text = result if isinstance(result, str) else json.dumps(result, ensure_ascii=False)
        return {"content": [{"type": "text", "text": text}], "isError": True}
    if isinstance(result, str):
        text = result
    else:
        text = json.dumps(result, ensure_ascii=False, indent=1)
    return {"content": [{"type": "text", "text": text}], "isError": False}


# Advertised-name cache: refreshed on every tools/list, consulted by
# resolve_tool so a client call maps back to the exact manifest id.
_advertised: dict = {}


def _refresh_advertised():
    try:
        listing = handle_tools_list({})
        _advertised.clear()
        for t in listing.get("tools", []):
            desc = t.get("description", "")
            # description carries "[arsumbris tool, id: mcp.xxx, ...]"
            marker = "id: "
            if marker in desc:
                full = desc.split(marker, 1)[1].split(",", 1)[0].strip()
                _advertised[t["name"]] = full
            else:
                _advertised[t["name"]] = t["name"]
    except Exception as exc:
        log(f"advertised cache refresh failed: {exc}")


_refresh_advertised()


def dispatch(method: str, params: dict) -> dict:
    table = {
        "initialize": handle_initialize,
        "notifications/initialized": lambda p: {},
        "initialized": lambda p: {},
        "notifications/cancelled": lambda p: {},
        "ping": lambda p: {},
        "tools/list": handle_tools_list,
        "tools/call": handle_tools_call,
    }
    handler = table.get(method)
    if handler is None and method.startswith("notifications/"):
        return {}  # tolerate unknown notifications, never crash the bridge
    if handler is None:
        raise ValueError(f"method not supported: {method}")
    return handler(params or {})


def main():
    log(f"starting, socket={SOCKET_PATH}")
    # Simplest robust framing: line-delimited JSON on stdin (the MCP Python SDK
    # writes newline-delimited JSON-RPC over stdio).
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            msg = json.loads(line)
        except json.JSONDecodeError as exc:
            log(f"bad JSON on stdin: {exc}")
            continue
        method = msg.get("method", "")
        msg_id = msg.get("id")
        is_notification = msg_id is None
        try:
            if method == "tools/list":
                _refresh_advertised()
            result = dispatch(method, msg.get("params") or {})
            if not is_notification:
                print(json.dumps({"jsonrpc": "2.0", "id": msg_id, "result": result}),
                      flush=True)
        except Exception as exc:
            log(f"error handling {method}: {exc}")
            if not is_notification:
                print(json.dumps({
                    "jsonrpc": "2.0", "id": msg_id,
                    "error": {"code": -32603, "message": str(exc)},
                }), flush=True)
    au.close()


if __name__ == "__main__":
    main()
