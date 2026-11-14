#!/usr/bin/env python3
"""A socket that answers one request and then hangs up, to produce a lost reply.

The dangerous engine state is not "the write failed" but "the write landed and we do
not know it": the adapter reports `outcome_unknown`, and the operator's only correct
move is `au_reconcile`, never a retry. Reproducing that needs a real engine — a fake
one proves the adapter's state machine, not the engine's durability.

This proxy sits between the adapter and the engine:

    adapter -> proxy -> engine

Every frame is forwarded to the engine, so the engine really applies the mutation.
The reply to the request that matches `--drop-tool` is then dropped and the
connection closed, so the adapter sees EOF exactly where a crashed engine would leave
it. Every other request, including the `initialize` and the later `au_reconcile`, is
a plain pass-through.

    ./scripts/lost_reply_proxy.py --engine <engine.sock> --listen <proxy.sock> \
        --drop mutate:write_file --drop-replies 1
"""

from __future__ import annotations

import argparse
import json
import os
import socket
import struct
import sys
import threading
from pathlib import Path

MAX_FRAME = 16 * 1024 * 1024


def read_frame(sock: socket.socket) -> bytes | None:
    header = b""
    while len(header) < 4:
        chunk = sock.recv(4 - len(header))
        if not chunk:
            return None
        header += chunk
    (length,) = struct.unpack(">I", header)
    if length > MAX_FRAME:
        raise SystemExit(f"frame too large: {length}")
    body = b""
    while len(body) < length:
        chunk = sock.recv(length - len(body))
        if not chunk:
            return None
        body += chunk
    return struct.pack(">I", length) + body


def verb_of(frame: bytes) -> str | None:
    """The engine verb a frame carries, as `mutate:<name>` or `read:<name>`.

    The adapter speaks the ENGINE wire on this socket, not MCP: frames look like
    `{"id":1,"mutate":"write_file",...}` or `{"id":1,"read":"content",...}`. Matching
    on the engine verb is what makes "drop the reply to the write" precise.
    """
    try:
        message = json.loads(frame[4:])
    except Exception:  # noqa: BLE001 - a malformed frame simply names no verb
        return None
    for kind in ("mutate", "read", "resolve", "subscribe"):
        if kind in message:
            return f"{kind}:{message[kind]}"
    return None


def serve_one(client: socket.socket, engine_path: str, drop: str | None) -> bool:
    """Forward one request; return True when its reply was deliberately dropped."""
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as engine:
        engine.settimeout(120)
        engine.connect(engine_path)
        request = read_frame(client)
        if request is None:
            return False
        engine.sendall(request)
        reply = read_frame(engine)
        if reply is None:
            return False
        seen = verb_of(request)
        if os.environ.get("AU_PROXY_DEBUG"):
            print(f"verb: {seen}", file=sys.stderr, flush=True)
        if drop is not None and seen == drop:
            print(f"dropped the reply to {drop}", file=sys.stderr, flush=True)
            return True
        client.sendall(reply)
        return False


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--engine", required=True)
    parser.add_argument("--listen", required=True)
    parser.add_argument(
        "--drop",
        default="mutate:write_file",
        help="drop the reply to frames carrying this engine verb (default: mutate:write_file)",
    )
    parser.add_argument(
        "--drop-replies",
        type=int,
        default=1,
        help="drop this many matching replies before passing everything through",
    )
    args = parser.parse_args()

    listen = Path(args.listen)
    if listen.exists():
        listen.unlink()
    listen.parent.mkdir(parents=True, exist_ok=True)

    server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    server.bind(str(listen))
    server.listen(8)
    os.chmod(listen, 0o600)

    remaining = args.drop_replies
    print(f"proxy {listen} -> {args.engine} (dropping {remaining} reply/replies to {args.drop})",
          flush=True)

    def handle(client: socket.socket) -> None:
        nonlocal remaining
        with client:
            drop_to = args.drop if remaining > 0 else None
            try:
                if serve_one(client, args.engine, drop_to):
                    remaining -= 1
            except Exception as error:  # noqa: BLE001 - a proxy must not die on one bad client
                print(f"connection failed: {error}", file=sys.stderr, flush=True)

    while True:
        client, _ = server.accept()
        threading.Thread(target=handle, args=(client,), daemon=True).start()


if __name__ == "__main__":
    sys.exit(main())
