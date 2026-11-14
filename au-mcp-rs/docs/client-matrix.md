# Client compatibility matrix

Measured by `scripts/client-matrix.sh` on 2026-11-13, on this host, against a
disposable two-file repository and a private engine socket. The script uses a
throwaway `HOME`, so no user or project configuration is read or written.

| client | version | what was exercised | result |
|---|---|---|---|
| Claude Code | 2.1.316 | `claude mcp add --scope local`, then `claude mcp list` health check | connected: the client completed `initialize` and `tools/list` |
| Codex CLI | 0.164.0 | `codex mcp add`, then `codex mcp list` | registered and listed; this client offers no non-interactive health check, so the handshake was **not** exercised |
| Cursor agent | 2026.10.22-27dcf5f | configuration only (no `mcp` subcommand; it reads `.cursor/mcp.json`) | **not exercised**: needs an API key and a model turn |

## What the matrix does and does not prove

- Claude Code's health check is a real client handshake: it spawns the adapter,
  completes `initialize`, and reads the tool list. A protocol or lifecycle
  regression breaks it.
- A registered-and-listed Codex entry proves the config shape the client accepts
  (stdio command plus args), not that a session succeeds.
- No client was run through a model turn, so nothing here exercises tool-call
  arguments chosen by a model, approval prompts, or client-side timeouts.
- Only the `2025-06-18` revision is reached by this script, because that is what
  these clients request. Other claimed revisions are covered by
  `tests/modern_stdio.rs`, not by a real client.

## Re-running

```sh
cd au-mcp-rs
AU_ENGINE_BIN=../au-engine/target/debug/au ./scripts/client-matrix.sh
```

The script exits non-zero when an installed client that offers a health check
reports anything other than a connected server. Clients that are not installed
are reported as such and do not fail the run.

## Re-run 2026-11-14: fork release binaries

Run again on this host with the fork release builds instead of the debug builds:

```sh
cd au-mcp-rs
AU_ENGINE_BIN=/root/arsumbris/au-engine/target/release/au \
AU_MCP_RS_BIN=/root/arsumbris/au-mcp-rs/target/release/au-mcp-rs \
bash scripts/client-matrix.sh
```

The run exited 1. All three clients are installed. The table records only what
each client reported in this run.

| client | version | what was exercised | result this run |
|---|---|---|---|
| Claude Code | 2.1.323 | `claude mcp add --scope local`, then `claude mcp list` health check | **not connected**: the health check failed with `ENOENT` on the adapter path it was given, so the client never started the binary |
| Codex CLI | 0.164.0 | `codex mcp add`, then `codex mcp list` | registered and listed; the handshake was **not** exercised (no non-interactive health check) |
| Cursor agent | 2026.10.22-27dcf5f | configuration only (`.cursor/mcp.json`) | **not exercised**: needs an API key and a model turn |

### The failure, verbatim

Full output of the run, including the exit status the harness appended:

```text
engine: /tmp/au-client-matrix-1vQCCf/home/.arsumbris/au-engine/run/afa490f1da63b5bb.sock
adapter: /root/arsumbris/au-mcp-rs/target/release/au-mcp-rs

client         version    result
---------------------------------------------------------------
claude         2.1.323    FAILED: arsumbris-rs: /root/arsumbris/au-mcp-rs//root/arsumbris/au-mcp-rs/target/release/au-mcp-rs /tmp/au-client-matrix-1vQCCf/home/.arsumbris/au-engine/run/afa490f1da63b5bb.sock - ✘ Failed to connect — ENOENT: no such file or directory, posix_spawn '/root/arsumbris/au-mcp-rs//root/arsumbris/au-mcp-rs/target/release/au-mcp-rs'
codex          0.164.0    registered; no non-interactive health check
cursor-agent   2026.10.22-27dcf5f configured via .cursor/mcp.json; needs an API key, not exercised
EXIT:1
```

### What this run verified, and what it did not

- The Claude Code entry was written by `claude mcp add`, and the health check
  then reported a failed connection. The path in the error is the adapter path
  with its parent directory prepended twice:
  `/root/arsumbris/au-mcp-rs//root/arsumbris/...`. The script builds that
  argument as `$OLDPWD/$ADAPTER`, and `AU_MCP_RS_BIN` was supplied as an
  absolute path. No binary exists at the doubled path, so the spawn failed
  before the adapter ran.
- Because the adapter never started, this run says nothing about whether the
  adapter serves Claude Code. It measures a client-side launch failure.
- Codex wrote and listed a config entry. That proves the config shape the
  client accepts (stdio command plus args), not that a session succeeds.
- Cursor agent received no exercise at all. No `.cursor/mcp.json` handshake was
  performed.
- No client completed `initialize` or `tools/list` in this run, and no client
  was driven through a model turn. The 2026-11-13 result above remains the last
  recorded connected health check on this host.

## Re-run 2026-11-14 (second): the ENOENT was ours, and Claude Code connects

The failure above was a bug in `scripts/client-matrix.sh`, not a client or adapter
problem: the script registered `"$OLDPWD/$ADAPTER"` while `AU_MCP_RS_BIN` was
already absolute, so the path doubled (`/root/arsumbris/au-mcp-rs//root/arsumbris/...`).
Fixed by resolving the adapter path to absolute ONCE, before any client config is
written, and passing that path to both `claude mcp add` and `codex mcp add`.

Same command, same binaries, after the fix:

```text
engine: /tmp/au-client-matrix-MNGeir/home/.arsumbris/au-engine/run/af3ace6d988bed39.sock
adapter: /root/arsumbris/au-mcp-rs/target/release/au-mcp-rs

client         version    result
---------------------------------------------------------------
claude         2.1.323    connected (initialize + tools/list)
codex          0.164.0    registered; no non-interactive health check
cursor-agent   2026.10.22-27dcf5f configured via .cursor/mcp.json; needs an API key, not exercised
```

Exit status 0: the only client with a non-interactive health check now reports a
connected server, which means it spawned the adapter and completed `initialize`
plus `tools/list` against the disposable engine. Codex and Cursor remain what the
first run recorded — registered/configured, handshake not exercised without an
interactive session.

## 2026-11-14 (third): a real model turn, and it found the marker

Everything above stops short of the one thing a client is for: a model choosing a
tool. `scripts/model-turn.sh` closes that with one small turn on the operator's own
credentials — deliberately a separate script, because `client-matrix.sh` must stay
free and offline.

The fixture holds `unique-marker-7f3a` in `notes/a.md`, a string no engine knows
except this one, so a correct answer cannot be a lucky guess:

```text
engine:  /tmp/au-fixture-DSYaVF/home/.arsumbris/au-engine/run/123e4ed907267994.sock
adapter: /root/arsumbris/au-mcp-rs/target/release/au-mcp-rs
prompt:  find unique-marker-7f3a with au_grep
PASS: the model called mcp__arsumbris-rs__au_grep
PASS: the marker travelled through the engine (tool result and/or answer)
PASS: the answer names the file that holds the marker
PASS: no permission denial (the allow-list covered the call)
PASS: one model turn completed through the adapter
```

The transcript is the interesting part. The client loaded the tool lazily, then the
model sent exactly the arguments the schema asks for, and the engine's payload came
back as a tool result rather than as prose:

```json
{"type":"tool_use","name":"mcp__arsumbris-rs__au_grep","input":{"query":"unique-marker-7f3a"}}
{"type":"tool_result","content":"{\"grep\":{\"matches\":[{\"column\":10,\"line\":6,
  \"path\":\".../fixture/notes/a.md\",\"repo\":\"fixture\",
  \"text\":\"find me: unique-marker-7f3a\",\"text_truncated\":false}],
  \"skipped_files\":0,\"total\":1,\"truncated\":false}}"}
{"type":"text","text":"`.../fixture/notes/a.md` (line 6): `find me: unique-marker-7f3a`"}
```

Three facts this settles: the tool description is good enough for a model to pick
the right tool from 35, the input schema's `query` name is what a model actually
supplies, and a payload shape with `matches`/`total`/`truncated` survives the MCP
boundary intact. The turn cost $0.042 and took 12 s.

Both scripts now share the fixture through `scripts/lib/disposable-fixture.sh`, so a
client check and a model-turn check cannot disagree about what they ran against.
