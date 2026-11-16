#!/usr/bin/env python3
"""Run a candidate engine on a COPY of the vault, so a ranking change can be measured without a swap.

The live daemon serves `/root/vault` with the installed binary. Measuring a change to `search.rs` there would
mean swapping production first, which the runbook forbids outside a window. This starts a second engine, built
from the working tree, over a copy of the vault, with its own HOME and its own socket.

    tools/ranking/candidate_engine.py --start          # prints the socket path, leaves it running
    tools/ranking/candidate_engine.py --socket         # print the socket of a running one
    tools/ranking/candidate_engine.py --stop

Three details the rehearsal learned the hard way (`docs/engine-swap-2026-11-16.md`):
- members resolve through `$HOME/.2brain/au-engine/config/repos.yaml`, so the copy gets its own HOME and a
  registry copy with **only** the `vault` entry re-pointed at the copy — leaving it at `/root/vault` would make
  the candidate index the live vault;
- the socket name derives from the canonical path, so a copy at a different path gets a different socket name;
- `search` and `grep` now default to `scope: own` (2026-11-16), so a comparison no longer has to pin it — but a
  comparison against an OLDER binary still does, so the harness results state which binary answered.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
VAULT = Path("/root/vault")
RUN_DIR = Path("/tmp/candidate-engine")
CANDIDATE = ROOT / "tb-engine/target/release/au"
INSTALLED = Path("/root/.2brain/bin/au")
STATE = Path("/root/.2brain/au-engine")

sys.path.insert(0, str(ROOT / "tb-mcp-rs/scripts/lib"))


def engine_binary() -> Path:
    return CANDIDATE if CANDIDATE.exists() else INSTALLED


def socket_path(home: Path, vault: Path) -> Path:
    """The socket the daemon will create: its own naming rule, asked of the binary."""
    probe = subprocess.run(
        [str(engine_binary()), "info", str(vault)],
        capture_output=True,
        text=True,
        env={**os.environ, "HOME": str(home)},
        timeout=60,
    )
    for line in probe.stdout.splitlines():
        if ".sock" in line:
            return Path(line.split()[-1])
    raise SystemExit(f"could not read the socket path from `au info`: {probe.stdout}{probe.stderr}")


def start() -> int:
    if (RUN_DIR / "socket").exists():
        print(f"already running: {(RUN_DIR / 'socket').read_text().strip()}")
        return 0
    if RUN_DIR.exists():
        shutil.rmtree(RUN_DIR)
    home = RUN_DIR / "home"
    vault = RUN_DIR / "vault"
    home.mkdir(parents=True)
    shutil.copytree(VAULT, vault, symlinks=True)

    # The registry, with the one entry this copy owns re-pointed at the copy.
    config = home / ".2brain/au-engine/config"
    config.mkdir(parents=True)
    for name in ("repos.yaml", "workspaces.yaml"):
        source = STATE / "config" / name
        if not source.exists():
            continue
        lines = source.read_text().splitlines()
        rewritten = []
        for line in lines:
            if "path:" in line and str(VAULT) in line:
                line = line.replace(str(VAULT), str(vault))
            rewritten.append(line)
        if name == "workspaces.yaml":
            rewritten = [line.replace(str(VAULT), str(vault)) for line in rewritten]
        (config / name).write_text("\n".join(rewritten) + "\n")

    log = RUN_DIR / "daemon.log"
    with log.open("w") as sink:
        subprocess.Popen(
            [str(engine_binary()), "daemon", "start", str(vault)],
            stdout=sink,
            stderr=sink,
            env={**os.environ, "HOME": str(home)},
            start_new_session=True,
        )
    for _ in range(60):
        found = list((home / ".2brain/au-engine/run").glob("*.sock"))
        if found:
            (RUN_DIR / "socket").write_text(str(found[0]) + "\n")
            print(str(found[0]))
            return 0
        time.sleep(1)
    print("the candidate engine did not open a socket; see", log)
    return 1


def stop() -> int:
    socket_file = RUN_DIR / "socket"
    if socket_file.exists():
        socket_file.unlink()
    subprocess.run(["pkill", "-f", f"daemon start {RUN_DIR}/vault"], capture_output=True)
    if RUN_DIR.exists():
        shutil.rmtree(RUN_DIR, ignore_errors=True)
    print("stopped")
    return 0


def current() -> int:
    socket_file = RUN_DIR / "socket"
    if not socket_file.exists():
        print("not running")
        return 1
    print(socket_file.read_text().strip())
    return 0


def main() -> int:
    parser = argparse.ArgumentParser()
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument("--start", action="store_true")
    group.add_argument("--stop", action="store_true")
    group.add_argument("--socket", action="store_true")
    args = parser.parse_args()
    if args.start:
        return start()
    if args.stop:
        return stop()
    return current()


if __name__ == "__main__":
    raise SystemExit(main())
