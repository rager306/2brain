# Switching a Hermes session to the Rust adapter

> **Superseded 2026-11-16** by `docs/hermes-2brain.md`. This document describes the switch from the
> Python bridge to `arsumbris-rs`; both are gone now — the client spawns `/usr/local/bin/2brain`.


Written for the operator: what to change, what to ask Hermes, what to check, and how to
take it back. Nothing here restarts the engine or touches the vault's data unless you
ask for the write test.

## What is being compared

Today `~/.hermes/config.yaml` points at the Python bridge:

```yaml
  arsumbris:
    type: stdio
    command: python3
    env:
      AU_MCP_SOCKET: /root/.arsumbris/au-mcp/run/adcfe8680a485529.sock
      AU_MCP_NAME: arsumbris
    args:
      - /root/arsumbris/au-mcp-bridge/bridge.py
```

The Rust adapter speaks to the engine's own socket, so it needs no Node daemon:

```yaml
  arsumbris-rs:
    type: stdio
    command: /root/arsumbris/au-mcp-rs/target/release/au-mcp-rs
    args:
      - /root/.arsumbris/au-engine/run/adcfe8680a485529.sock
```

**Add it, do not replace `arsumbris`.** Both can be mounted in one session, which is
what makes the comparison a comparison.

## Step 0 — the read-only A/B, before any session

```sh
cd /root/arsumbris/au-mcp-rs
python3 scripts/live_parity.py
```

Measured on 2026-11-15 against the served vault:

```text
fact              adapter  bridge  verdict
members                 7       7  identical
top_level_dirs          9       9  identical
types                  95      95  identical
hubs                   20      20  identical
diagnostics             0       0  identical
PASS: 5 facts agree between the live bridge and the Rust adapter
```

It reads the live engine socket and the live bridge and prints one verdict per read.
Read-only by construction: no `mutate` frame, no `--allow-mutations`, no git on the
vault, no signal to any daemon. Exit 0 means every compared read agreed.

If this fails, stop here and send me the output: the rest of the procedure assumes the
two stacks already agree on reads.

## Step 1 — mount the adapter read-only

Add the `arsumbris-rs` block above under `mcp_servers:` in `~/.hermes/config.yaml`,
then start a NEW Hermes session (config is read at session start; an existing session
will not pick it up).

Success looks like: the tool list contains `mcp__arsumbris-rs__au_overview`,
`au_search_tools`, `au_content` — 35 tools, and **no** `au_write_file`,
`au_batch`, `au_create_file`. A read-only launch cannot advertise what it cannot do.

## Step 2 — give it a real read task

> Use only the `arsumbris-rs` tools, not `arsumbris`. Show me the workspace members and
> the five notes with the most references. Then read the top one and give me its
> frontmatter and its headings.

What to watch for:

- it starts with `au_overview`, not with a guess;
- `au_search_tools` appears if it is unsure which tool fits — that is the discovery
  path working, not a detour;
- `au_content` answers with a `hash` and a `commit` (the bridge answers with a text
  blob; if you see the hash, you are on the Rust adapter);
- no tool call fails with `unknown tool` or `policy/…`.

Send me the session transcript if anything looks wrong. The most useful failure is
"it wanted X and no tool gave it X".

## Step 3 — the write test, on a throwaway path

This is the only step that writes to the vault, and it is reversible.

Add `--allow-mutations` to the adapter's `args`, start a new session, and ask:

> Using only `arsumbris-rs`: create `inbox/adapter-write-test.md` with type `note`,
> a title, and one line of text. Then read it back and tell me its hash.

Checks, in order of importance:

1. the write is one engine commit (`git -C /root/vault log -1 --stat` shows exactly
   one file);
2. `au_files` or `au_content` reads it back;
3. **a second write to the same path without a fresh read is refused** with
   `policy/prior_read` — that refusal is the gate working, and it is the thing the
   Node stack does not do;
4. if the reply is lost (it should not be), the answer is `outcome_unknown` and the
   tool to use is `au_reconcile`, never a retry.

Clean up with `git -C /root/vault rm inbox/adapter-write-test.md` and commit, or ask
the adapter: `au_delete_file` needs a fresh `au_content` read of that path first.

## Step 4 — what to send me

Any of these, whichever you get first:

- the session transcript (`~/.hermes/…`) if a tool call failed;
- the output of `python3 scripts/live_parity.py` when you are done;
- the exact prompt you used and what Hermes answered instead of what you expected.

## Taking it back

Remove the `arsumbris-rs` block from `~/.hermes/config.yaml` and start a new session.
Nothing else changed: the engine, the vault and the Node stack were never touched, so
removing the block restores exactly the previous state.

## What this does not prove

- It does not prove the two stacks answer identically for everything — only the five
  facts `live_parity.py` compares (members, top-level dirs, the full type list, the top
  20 hubs, and the diagnostics list). It exits 2 rather than 0 when a fact could not be
  compared, so "could not check" never reads as a pass.
- It does not exercise resource subscriptions (there are none; `au_watch` is the change
  signal) or the plugin loader (this vault declares no plugins).