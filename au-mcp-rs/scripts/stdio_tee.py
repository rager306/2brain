#!/usr/bin/env python3
"""Record an MCP stdio conversation while staying in the middle of it.

Some questions cannot be answered from either side's output: which protocol revision
did this client actually request, and which did the server answer? The client's log
may not say, and the server has no reason to print it.

This wrapper sits between them and copies every line both ways to a JSONL file:

    {"dir":"c2s","line":"{\"jsonrpc\":\"2.0\",...}"}

    scripts/stdio_tee.py --record /tmp/mcp.jsonl -- <server command> [args...]

It is transparent: bytes are forwarded unmodified as soon as they are complete lines,
and the exit status is the server's. Point a client's MCP command at it instead of at
the server, then read the record.
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import threading

MAX_UNRECORDED = 4096


def pump(src, dst, log, direction: str, lock: threading.Lock) -> None:
    """Copy complete lines from src to dst, recording each one."""
    while True:
        line = src.readline()
        if not line:
            try:
                dst.close()
            except Exception:  # noqa: BLE001 - a closed pipe is the normal end
                pass
            return
        try:
            dst.write(line)
            dst.flush()
        except Exception:  # noqa: BLE001 - the peer went away; stop pumping
            return
        if log is not None and len(line) <= MAX_UNRECORDED * 1024:
            with lock:
                log.write(json.dumps({"dir": direction, "line": line.rstrip("\n")}) + "\n")
                log.flush()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--record", help="JSONL file to append the conversation to")
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()

    command = args.command
    if command and command[0] == "--":
        command = command[1:]
    if not command:
        print("stdio_tee: no server command given", file=sys.stderr)
        return 2

    log = open(args.record, "a", encoding="utf-8") if args.record else None
    lock = threading.Lock()

    child = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                             stderr=sys.stderr, text=True, bufsize=1)
    # The harness speaks to us on stdin/stdout, text mode, line buffered.
    to_child = threading.Thread(target=pump, args=(sys.stdin, child.stdin, log, "c2s", lock),
                                daemon=True)
    from_child = threading.Thread(target=pump, args=(child.stdout, sys.stdout, log, "s2c", lock),
                                  daemon=True)
    to_child.start()
    from_child.start()
    status = child.wait()
    from_child.join(timeout=1)
    if log is not None:
        log.close()
    return status


if __name__ == "__main__":
    sys.exit(main())
