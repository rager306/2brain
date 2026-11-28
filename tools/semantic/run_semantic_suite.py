#!/usr/bin/env python3
"""End-to-end runner: isolated black-box capture (`capture.py`) -> offline scoring (`scorer.py`)."""

from __future__ import annotations

import argparse
import subprocess
import sys
import tempfile
from pathlib import Path


def main() -> int:
    parser = argparse.ArgumentParser(description="Run 2brain independent semantic acceptance suite")
    parser.add_argument(
        "--bin",
        type=Path,
        default=Path("target/release/2brain"),
        help="Path to compiled 2brain binary (default: target/release/2brain)",
    )
    parser.add_argument(
        "--out",
        type=Path,
        help="Optional directory to retain frozen capture bundle and scorecard",
    )
    args = parser.parse_args()

    here = Path(__file__).resolve().parent
    binary = args.bin.resolve()
    if not binary.exists():
        print(f"error: 2brain binary not found at {binary}", file=sys.stderr)
        return 2

    if args.out:
        out_dir = args.out.resolve()
        out_dir.mkdir(parents=True, exist_ok=True)
        return _run(here, binary, out_dir)

    with tempfile.TemporaryDirectory(prefix="2brain-semantic-run-") as td:
        return _run(here, binary, Path(td))


def _run(here: Path, binary: Path, out_dir: Path) -> int:
    cap = subprocess.run(
        [sys.executable, str(here / "capture.py"), "--bin", str(binary), "--out", str(out_dir)],
        check=False,
    )
    if cap.returncode != 0:
        return cap.returncode

    score = subprocess.run(
        [
            sys.executable,
            str(here / "scorer.py"),
            "--observations",
            str(out_dir / "observations.json"),
            "--manifest",
            str(out_dir / "run_manifest.json"),
            "--out",
            str(out_dir / "scorecard.json"),
        ],
        check=False,
    )
    return score.returncode


if __name__ == "__main__":
    raise SystemExit(main())
