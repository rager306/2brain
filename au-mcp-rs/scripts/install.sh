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
# The crate's `target/` is a symlink into the shared `.cargo-target`
# (`.cargo/config.toml`), so this path is where cargo actually writes without
# asking cargo metadata — which needs the toolchain environment this script sets
# up only for `build`.
target=${CARGO_TARGET_DIR:-$here/target}

cd "$here"
RUSTUP_HOME=${RUSTUP_HOME:-$HOME/.rustup} cargo +1.99.0 build --release --bin 2brain

install -d "$prefix/bin"
install -m 0755 "$target/release/2brain" "$prefix/bin/2brain"
echo "installed $prefix/bin/2brain"
"$prefix/bin/2brain" --version
sha256sum "$prefix/bin/2brain"
