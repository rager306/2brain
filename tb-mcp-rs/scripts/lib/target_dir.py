"""The monorepo's shared cargo target directory (`.cargo/config.toml`).

Every crate here shares one target directory, so a per-crate path such as
`tb-engine/target/release/tb-engine` does not exist on a clean checkout. Asking cargo is
the only answer that honours both `CARGO_TARGET_DIR` and the config file.
"""

from __future__ import annotations

import json
import subprocess
from pathlib import Path

CRATE = Path(__file__).resolve().parents[2]
REPO = CRATE.parent


def cargo_target_dir() -> Path:
    """Ask cargo where it writes; fall back to the repo-local directory it names."""
    try:
        answer = subprocess.run(
            ["cargo", "metadata", "--format-version", "1", "--no-deps"],
            cwd=CRATE, capture_output=True, text=True, check=True)
        return Path(json.loads(answer.stdout)["target_directory"])
    except (OSError, subprocess.CalledProcessError, KeyError, json.JSONDecodeError):
        return REPO / ".cargo-target"
