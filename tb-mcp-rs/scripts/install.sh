#!/usr/bin/env bash
# Build and install the 2brain binary.
#
# The binary Hermes launches must exist at a stable path and must not be the
# build tree: a `cargo clean`, a deleted target directory, or a rebuilt release
# profile leaves the configured path pointing at nothing, and the next MCP
# connection fails with no other symptom. This script is that deployment step.
#
#   ./scripts/install.sh                  # build, install to /usr/local/bin/2brain
#   PREFIX=/root/.local ./scripts/install.sh
#
# The default is /usr/local because `HOME` differs by caller here (`/root` for the
# service, `/root/.allspark-home` for agent sessions), so `$HOME/.local` would put
# the binary in a place the client that launches it cannot see.
#
# Prints the identity a reader can compare against `docs/verification.md`.
set -euo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
prefix=${PREFIX:-/usr/local}

cd "$here"
# The runbook pins RUSTUP_HOME=/root/.rustup (`AGENTS.md`). Honour it when the
# caller set it, and otherwise prefer that home: an agent shell whose HOME is
# /root/.allspark-home would silently compile the same tree with a second
# toolchain root, which both starves the shared target lock and mixes caches.
if [[ -z ${RUSTUP_HOME:-} ]]; then
  export RUSTUP_HOME=$([[ -d /root/.rustup ]] && echo /root/.rustup || echo "$HOME/.rustup")
fi

# One target directory for the whole monorepo (`.cargo/config.toml`), and the only
# honest way to learn which one is to ask cargo: `tb-mcp-rs/target` is not a symlink
# and does not exist on a clean checkout, so a hard-coded `$here/target` made the
# install fail after a successful build. `CARGO_TARGET_DIR` and the config are both
# honoured because the answer comes from cargo itself, as in the root `install.sh`.
target=$(cargo +1.99.0 metadata --no-deps --format-version 1 2>/dev/null |
  sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')
[[ -n $target ]] || target=$(cd "$here/.." && pwd)/.cargo-target

cargo +1.99.0 build --release --bin 2brain

install -d "$prefix/bin"
install -m 0755 "$target/release/2brain" "$prefix/bin/2brain"
echo "installed $prefix/bin/2brain"
"$prefix/bin/2brain" --version
sha256sum "$prefix/bin/2brain"
