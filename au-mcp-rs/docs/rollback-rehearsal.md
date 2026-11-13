# Rollback rehearsal

`scripts/rollback_rehearsal.py` rehearses the direction that matters if the Rust
adapter misbehaves: stop using it and fall back to the read-only path. Run on
2026-11-13 against a disposable repository and a private engine; it passed.

```sh
cd au-mcp-rs
AU_ENGINE_BIN=../au-engine/target/debug/au python3 scripts/rollback_rehearsal.py
```

## What it proves

1. **A granted adapter writes through the engine.** The rehearsal reads the file
   with `au_content`, then writes it with the hash that read returned.
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
  was lost is covered by `tests/faults.rs`, and recovery by `au_reconcile` plus
  `tests/reconcile.rs`.
- It writes to a disposable repository only. No production socket, daemon, or
  vault is touched, and no maintenance window is implied.
