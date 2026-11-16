# Rollback rehearsal

`scripts/rollback_rehearsal.py` rehearses the direction that matters if the Rust
adapter misbehaves: stop using it and fall back to the read-only path. Run on
2026-11-13 against a disposable repository and a private engine; it passed.

```sh
cd au-mcp-rs
AU_ENGINE_BIN=../tb-engine/target/debug/au python3 scripts/rollback_rehearsal.py
```

## What it proves

1. **A granted adapter writes through the engine.** The rehearsal reads the file
   with `tb_content`, then writes it with the hash that read returned.
2. **The write is an engine commit, not adapter state.** The fixture gains
   exactly one commit and `git status --porcelain` is empty afterwards.
3. **The adapter leaves no files behind.** The file list, excluding `.git` and
   the engine's own `.arsumbris/au-engine` runtime directory, is unchanged.
4. **Nothing is lost when the adapter stops.** A fresh adapter process, with no
   inherited state, reads the written text.
5. **Rolling back to read-only is a launch flag, not a migration.** The same
   binary started without `--allow-mutations` still reads the file and refuses
   the write at the protocol layer.

## What it does not prove

- It does not exercise the existing Python bridge or the Node `au-mcp` daemon,
  which speak to a different socket. Parity with them is separate work.
- It does not rehearse a rollback of a negative outcome: a mutation whose reply
  was lost is covered by `tests/faults.rs`, and recovery by `tb_reconcile` plus
  `tests/reconcile.rs`.
- It writes to a disposable repository only. No production socket, daemon, or
  vault is touched, and no maintenance window is implied.

## Re-run 2026-11-14: fork release binaries

Rehearsed again on 2026-11-14 with the fork release binaries, against a new
disposable repository and a private engine socket under a throwaway `HOME`:

```sh
cd au-mcp-rs
AU_ENGINE_BIN=/root/2brain/tb-engine/target/release/au \
AU_MCP_RS_BIN=/root/2brain/tb-mcp-rs/target/release/tb-mcp-rs \
python3 scripts/rollback_rehearsal.py
```

The script exited 0. Each of the three properties it exists to check passed.
The quoted lines are its own:

1. **The adapter keeps no state on disk.** **PASS.**
   `ok: a granted adapter writes through the engine`, then
   `ok: the write is an engine commit and the tree is clean` and
   `ok: the adapter left no files behind`. The fixture gained exactly one
   commit, `git status --porcelain` was empty, and the file list was unchanged.
2. **Stopping the adapter loses nothing.** **PASS.**
   `ok: a fresh process reads the written file`. The writing process was
   killed, and a new adapter process read back the written text.
3. **Rolling back to read-only is a launch flag, not a migration.** **PASS.**
   `ok: rolling back to read-only is a launch flag: the write is refused`. The
   same binary started without `--allow-mutations` read the file and refused
   `tb_write_file` at the protocol layer.

Its closing line was
`PASS: rollback to read-only loses nothing and needs no migration`, followed by
no failure output.

What this run does not add to the section above: it still does not exercise the
Python bridge or the Node `au-mcp` daemon, does not lose a mutation reply, and
touches no production socket or vault.
