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
