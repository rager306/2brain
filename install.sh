#!/usr/bin/env bash
# Build both halves of 2brain and put them where a client can find them.
#
#   ./install.sh                          # /usr/local/bin, needs a writable prefix
#   PREFIX=$HOME/.local ./install.sh      # a user install, no root
#
# What it installs, and where:
#
#   $PREFIX/bin/2brain        the MCP server, the engine daemon, `init` and `doctor`
#   $PREFIX/bin/au            the engine binary, or $HOME/.2brain/bin/au when
#                             $PREFIX/bin cannot take it. `2brain daemon` does not
#                             need it; `au members`, `au migrate` and `au open` do.
#   $PREFIX/share/2brain/templates/
#                             the starters `2brain init --template <name>` copies
#
# It does not restart, signal or inspect a daemon: a running one keeps serving the
# binary it started with, and an install that replaced a file under a live daemon
# must say so rather than act on it. Stop and start the daemon yourself.
set -euo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
prefix=${PREFIX:-/usr/local}

die() {
  printf 'install.sh: %s\n' "$*" >&2
  exit 1
}

# The runbook pins RUSTUP_HOME=/root/.rustup. Honour it when the caller set one,
# and otherwise prefer that home: an agent shell whose HOME is elsewhere would
# silently compile the same tree with a second toolchain root, starving the shared
# target lock and mixing caches.
if [[ -z ${RUSTUP_HOME:-} ]]; then
  if [[ -d /root/.rustup ]]; then
    export RUSTUP_HOME=/root/.rustup
  else
    export RUSTUP_HOME="$HOME/.rustup"
  fi
fi
command -v cargo >/dev/null 2>&1 || die "cargo is not on PATH; install Rust 1.99.0 from https://rustup.rs"

# The pinned toolchain, named explicitly so a host with several installed cannot
# silently build with another edition's compiler.
cargo_command=(cargo +1.99.0)

# One target directory for the whole monorepo (`.cargo/config.toml`); both crates'
# `target/` are symlinks into it, so these two paths are the same build.
engine_target=$here/tb-engine/target/release
server_target=$here/tb-mcp-rs/target/release

printf 'install.sh: building 2brain and the engine (release)\n'
(cd "$here/tb-engine" && "${cargo_command[@]}" build --release -p tb-cli --bin au)
(cd "$here/tb-mcp-rs" && "${cargo_command[@]}" build --release --bin 2brain)

for binary in "$server_target/2brain" "$engine_target/au"; do
  [[ -x $binary ]] || die "the build did not produce $binary"
done

bin_dir=$prefix/bin
mkdir -p "$bin_dir" 2>/dev/null ||
  die "cannot create $bin_dir; run as root, or pass PREFIX=\$HOME/.local"

install -m 0755 -t "$bin_dir" "$server_target/2brain" 2>/dev/null ||
  die "cannot write $bin_dir/2brain; run as root, or pass PREFIX=\$HOME/.local"

engine_bin=$bin_dir/au
if ! install -m 0755 -t "$bin_dir" "$engine_target/au" 2>/dev/null; then
  engine_bin=$HOME/.2brain/bin/au
  mkdir -p "$HOME/.2brain/bin"
  install -m 0755 -t "$HOME/.2brain/bin" "$engine_target/au"
  printf 'install.sh: kept the engine at %s, because %s refused it\n' "$engine_bin" "$bin_dir"
fi

# The templates an installed `2brain init --template <name>` reads; a checkout
# would have found them in `tb-defaults/`, which a deployed binary has no path to.
templates=$prefix/share/2brain/templates
if [[ -d $here/tb-defaults ]] && mkdir -p "$templates" 2>/dev/null; then
  for starter in "$here"/tb-defaults/*/; do
    cp -R "$starter" "$templates/"
  done
else
  printf 'install.sh: no tb-defaults/ beside this script, so --template starts nothing but an empty workspace\n'
fi

printf '\ninstall.sh: installed %s\n' "$bin_dir/2brain"
"$bin_dir/2brain" --version
printf 'install.sh: installed %s\n' "$engine_bin"

case ":$PATH:" in
  *":$bin_dir:"*) ;;
  *) printf 'install.sh: add %s to PATH\n' "$bin_dir" ;;
esac
case ":$PATH:" in
  *":$HOME/.2brain/bin:"*) ;;
  *) [[ $engine_bin == "$HOME/.2brain/bin/au" ]] && printf 'install.sh: add %s to PATH\n' "$HOME/.2brain/bin" ;;
esac

printf '\ninstall.sh: next two commands\n'
printf '  2brain init my-workspace\n'
printf '  2brain doctor my-workspace\n'
