#!/usr/bin/env python3
"""The strict MCP stdio client the verification scripts share.

Each rule here is the strictest of the four copies it replaces: handshake first; every
read bounded by a timeout, so a silent server raises instead of hanging; a reply whose
id does not match the request is an error; on EOF the error carries the child's stderr
tail; close() terminates the child; and a message is read to its newline, because a
frame over a pipe can arrive in pieces.
"""

from __future__ import annotations

import contextlib
import glob
import json
import os
import select
import subprocess
import threading
import time
from collections import deque


class StdioMcpError(RuntimeError):
    """A session that could not produce the answer its request asked for."""


STATE_DIR_NAMES = (".2brain",)


def state_run_dir(home: str | os.PathLike[str]) -> "os.PathLike[str] | None":
    """The engine's socket directory under a `$HOME`, whichever spelling the engine used.

    The state root is `$2BRAIN_HOME` else `<home>/.2brain` (`repo::state_dir_in`). A script
    that joins a literal name asserts that decision instead of finding the daemon; this asks
    the filesystem instead, so a configured `$2BRAIN_HOME` is found too.
    """
    for name in STATE_DIR_NAMES:
        run = os.path.join(os.fspath(home), name, "engine", "run")
        if os.path.isdir(run):
            return run
    return None


def newest_socket(home: str | os.PathLike[str]) -> "str | None":
    """The newest `.sock` in the engine's run directory under `home`, or None."""
    run = state_run_dir(home)
    if run is None:
        return None
    candidates = glob.glob(os.path.join(run, "*.sock"))
    if not candidates:
        return None
    return max(candidates, key=os.path.getmtime)


def is_error(reply: dict) -> bool:
    """True when the server answered with a JSON-RPC error or `isError`."""
    return "error" in reply or bool((reply.get("result") or {}).get("isError"))


def tool_text(reply: dict) -> str:
    """The first content block's text. A transport error is not an answer."""
    content = (reply.get("result") or {}).get("content") or []
    if "error" in reply or not content:
        raise StdioMcpError(f"the server returned no answer: {json.dumps(reply)[:800]}")
    return content[0]["text"]


def tool_payload(reply: dict) -> dict:
    """The JSON a tool put in its first content block."""
    return json.loads(tool_text(reply))


class StdioMcp:
    """One MCP stdio session with a child process."""

    def __init__(self, command, *, name="stdio-client", timeout=30.0, env=None) -> None:
        self.command, self.name, self.timeout, self.env = list(command), name, timeout, env
        self.process, self._reader, self._next_id = None, None, 0
        self._pending = bytearray()  # bytes read, not yet terminated by a newline
        self._stderr = deque(maxlen=40)

    def start(self) -> "StdioMcp":
        self.process = subprocess.Popen(
            self.command, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=subprocess.PIPE, env=self.env)
        # stderr is drained, not discarded: its tail is the only evidence an EOF leaves
        # behind, and an undrained pipe can deadlock the child we are asking to prove.
        self._reader = threading.Thread(
            target=self._read_stderr, args=(self.process,), daemon=True)
        self._reader.start()
        try:
            reply = self.request("initialize", {
                "protocolVersion": "2025-06-18", "capabilities": {},
                "clientInfo": {"name": self.name, "version": "1"}})
        except BaseException:  # a child we spawned is ours to stop, whatever went wrong
            self.close()
            raise
        if "result" not in reply:
            self.close()
            raise StdioMcpError(f"{self.name} refused initialize: {json.dumps(reply)[:800]}")
        self._write({"jsonrpc": "2.0", "method": "notifications/initialized"})
        return self

    def close(self) -> None:
        process, self.process = self.process, None
        if process is None:
            return
        with contextlib.suppress(OSError):
            process.stdin.close()  # EOF is the polite shutdown
        process.terminate()
        try:
            process.wait(timeout=2)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()

    def __enter__(self) -> "StdioMcp":
        return self.start()

    def __exit__(self, *_exc_info: object) -> None:
        self.close()

    def request(self, method: str, params: dict | None = None, *,
                timeout: float | None = None) -> dict:
        """Send one request; return the reply that carries its id."""
        self._next_id += 1
        self._write({"jsonrpc": "2.0", "id": self._next_id, "method": method,
                     "params": params or {}})
        deadline = time.monotonic() + (self.timeout if timeout is None else timeout)
        while True:
            reply = self._read(deadline, method)
            if "id" not in reply:
                continue  # a notification: it answers nothing
            if reply["id"] != self._next_id:
                raise StdioMcpError(
                    f"{self.name} answered {method} (id {self._next_id}) with id "
                    f"{reply['id']!r}: {json.dumps(reply)[:800]}")
            return reply

    def call(self, tool: str, arguments: dict | None = None, *,
             timeout: float | None = None) -> dict:
        """tools/call, as the whole reply: a refusal is an answer too."""
        return self.request("tools/call", {"name": tool, "arguments": arguments or {}},
                            timeout=timeout)

    def _write(self, message: dict) -> None:
        try:
            self.process.stdin.write((json.dumps(message) + "\n").encode())
            self.process.stdin.flush()
        except (AttributeError, OSError) as error:
            raise StdioMcpError(f"{self.name} closed its input ({error}){self._tail()}") from None

    def _read(self, deadline: float, method: str) -> dict:
        while b"\n" not in self._pending:
            remaining = deadline - time.monotonic()
            if not (remaining > 0 and select.select([self.process.stdout], [], [], remaining)[0]):
                raise StdioMcpError(f"{self.name} did not answer {method} in time{self._tail()}")
            if not (chunk := os.read(self.process.stdout.fileno(), 65536)):
                raise StdioMcpError(f"{self.name} closed stdout answering {method}{self._tail()}")
            self._pending += chunk
        raw, _, rest = self._pending.partition(b"\n")
        self._pending = bytearray(rest)
        try:
            return json.loads(raw.decode("utf-8", "replace"))
        except ValueError as error:
            raise StdioMcpError(
                f"{self.name} sent a line that is not JSON ({error}): {raw[:400]!r}") from None

    def _read_stderr(self, process: subprocess.Popen) -> None:
        for line in process.stderr or ():
            self._stderr.append(line.decode("utf-8", "replace").rstrip())

    def _tail(self) -> str:
        if self._reader is not None:
            self._reader.join(timeout=0.2)
        tail = "\n".join(self._stderr)
        return f"; stderr tail:\n{tail}" if tail else " (no stderr)"
