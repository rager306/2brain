#!/usr/bin/env bash
# A disposable workspace plus a daemon on it, for the verification scripts.
#
# Sourced, never executed. Every caller gets the same shape so a client check and a
# model-turn check cannot disagree about what they ran against:
#
#   source scripts/lib/disposable-fixture.sh
#   prepare_fixture            # sets $root $repo $socket $daemon_pid, installs the trap
#   ...run the check...
#
# The fixture is three notes and two types, one note carrying a marker no engine in
# the world would answer by accident (`unique-marker-7f3a`), so a check can prove an
# answer came from THIS engine rather than from the model's memory.
#
# `$root/home` becomes HOME for the daemon, so the socket lands under it and no
# production socket or user config is touched.

prepare_fixture() {
  root=$(mktemp -d /tmp/au-fixture-XXXXXX)
  export HOME=$root/home
  export XDG_CONFIG_HOME=$root/config
  mkdir -p "$HOME" "$XDG_CONFIG_HOME"

  trap cleanup_fixture EXIT

  repo=$root/fixture
  mkdir -p "$repo/.arsumbris" "$repo/type" "$repo/notes"
  printf 'name: fixture\n' >"$repo/.arsumbris/repo.yaml"
  printf 'fields:\n  title: String\n' >"$repo/type/note.type.yaml"
  printf 'fields:\n  title: String\n  body: String\n' >"$repo/type/task.type.yaml"
  printf -- '---\ntype: note\ntitle: marker\n---\n\nfind me: unique-marker-7f3a\n' \
    >"$repo/notes/a.md"
  printf -- '---\ntype: note\ntitle: second\n---\n\nplain\n' >"$repo/notes/b.md"
  printf -- '---\ntype: task\ntitle: third\n---\n\nplain\n' >"$repo/notes/c.md"
  git -C "$repo" init -q
  git -C "$repo" add -A
  git -C "$repo" -c user.name=fixture -c user.email=fixture@example.invalid \
    -c commit.gpgsign=false commit -q -m fixture

  "$ENGINE" daemon start "$repo" >/dev/null 2>&1 &
  daemon_pid=$!

  socket=
  for _ in $(seq 1 200); do
    socket=$(ls "$HOME"/.arsumbris/au-engine/run/*.sock 2>/dev/null | head -1 || true)
    [[ -n $socket ]] && break
    sleep 0.1
  done
  if [[ -z $socket ]]; then
    echo "the disposable engine never became ready" >&2
    exit 1
  fi
}

cleanup_fixture() {
  if [[ -n ${daemon_pid:-} ]]; then
    kill "$daemon_pid" 2>/dev/null || true
    wait "$daemon_pid" 2>/dev/null || true
  fi
  rm -rf "$root"
}
