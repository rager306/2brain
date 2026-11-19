#!/usr/bin/env bash
# The monorepo's shared cargo target directory (`.cargo/config.toml`).
#
# A per-crate `target/` is not where cargo writes: every crate here shares one
# dependency graph and one directory, so a default such as
# `../tb-engine/target/debug/au` names a path that does not exist on a clean
# checkout. Asking cargo is the only answer that honours both `CARGO_TARGET_DIR`
# and the config file, which is also what the root `install.sh` does.
#
# Sourced, never executed:
#
#   source scripts/lib/target-dir.sh
#   ENGINE=${AU_ENGINE_BIN:-$(cargo_target_dir)/debug/au}
cargo_target_dir() {
  local crate answer
  crate=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
  answer=$(cd "$crate" && cargo metadata --format-version 1 --no-deps 2>/dev/null |
    python3 -c 'import json,sys;print(json.load(sys.stdin)["target_directory"])' 2>/dev/null) || answer=
  printf '%s\n' "${answer:-$(cd "$crate/.." && pwd)/.cargo-target}"
}
