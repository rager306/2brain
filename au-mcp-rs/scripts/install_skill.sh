#!/usr/bin/env bash
# Install the arsumbris skill into the agent skills directory.
#
# The repository copy is the source of truth; the installed directory is a build
# artefact. A symlink is not used: the skill loader skips symlinked entries.
set -euo pipefail

source_dir=$(cd "$(dirname "$0")/../skills/arsumbris" && pwd)
target_root=${AGENT_SKILLS_DIR:-$HOME/.agents/skills}
target=$target_root/arsumbris

if [[ -L $target ]]; then
  rm "$target"
fi
mkdir -p "$target"
rm -rf "${target:?}/"*
cp -r "$source_dir/." "$target/"

echo "installed $target"
ls "$target"
