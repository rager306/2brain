# Connecting an MCP client

The server speaks MCP over stdio and needs one thing: a **target** — a workspace path, which it resolves to
that workspace's engine socket, or a `.sock` path given as-is. It never starts a daemon; `2brain daemon start`
does that, and `2brain doctor` says whether it answered.

## 1. Build, or install

```sh
./install.sh                    # from the repository root: both binaries, starters, PATH hint
# or, in this crate alone:
cargo +1.99.0 build --release   # target/release/2brain
```

One binary serves both halves: `2brain mcp <target>` is the MCP server, `2brain daemon` the engine,
`2brain init` makes a workspace, `2brain doctor` tells you whether the whole thing works. There is no
second executable to keep in step.

## 2. Find the engine socket — or let the server find it

```sh
2brain doctor <workspace>       # prints the socket it resolves, whether the daemon answers, and what to do
```

The rule it applies, in order: an explicit `.sock` path is used as given; a workspace is resolved to the
socket its canonical path hashes to; `$2BRAIN_SOCKET` wins when set. One socket per served workspace, under
`~/.2brain/au-engine/run/` (or `~/.arsumbris/au-engine/run/` on a machine that has not created the new
directory).

## 2b. Check what the binary says about itself

```sh
2brain --help                       # both halves and their subcommands
2brain --version                    # version + build commit
2brain info                         # MCP surface and launch defaults, no engine
2brain info <engine.sock>           # the same, plus ready/schema/version/workspace
2brain daemon status <entry>        # engine side, same as `au daemon status`
2brain --help / --version / --info  # the MCP half alone
```

`--info` never needs a running daemon; with a socket it adds the probe result and
still exits 0 when the engine is unreachable, so it is safe in a health script.

## 3. Register the server

### allspark (Prime Agent)

`~/.allspark-home/.prime/agent/settings.json`, under `mcpServers`:

```json
"2brain": {
  "type": "stdio",
  "command": "/usr/local/bin/2brain",
  "args": ["mcp", "/root/.2brain/au-engine/run/adcfe8680a485529.sock"]
}
```

The host reads `mcpServers` at session start, so a new entry appears in a new
session. A running session may pick it up later, but do not rely on it: verify
with `mcp.list_connections()` and `mcp.list_tools("2brain")`.

### Claude Code

```sh
claude mcp add --scope local 2brain -- \
  /usr/local/bin/2brain mcp \
  /root/.2brain/au-engine/run/adcfe8680a485529.sock
claude mcp list          # expect: 2brain ... ✔ Connected
```

### Codex CLI

```sh
codex mcp add 2brain -- \
  /usr/local/bin/2brain mcp \
  /root/.2brain/au-engine/run/adcfe8680a485529.sock
codex mcp list           # expect: 2brain  ...  enabled
```

Codex has no non-interactive health check, so `list` proves the configuration
shape, not the handshake.

### Any client that takes JSON

```json
{
  "mcpServers": {
    "2brain": {
      "command": "/usr/local/bin/2brain",
      "args": ["mcp", "/root/.2brain/au-engine/run/adcfe8680a485529.sock"]
    }
  }
}
```

## 4. Read-only by default

Without `--allow-mutations` the 16 mutating tools are absent from `tools/list`
and refused before the engine is contacted. `allow_composition` and
`allow_workspace_shape` are separate grants, each requiring `allow_mutations`
and independent of the other. To grant writes, pass a policy file and keep the
allowlist narrow:

```json
{
  "allow_mutations": true,
  "members": ["vault"],
  "audit": "stderr"
}
```

```json
"args": ["mcp", "/root/.2brain/au-engine/run/adcfe8680a485529.sock",
         "--policy", "/etc/2brain-policy.json",
         "--principal", "allspark"]
```

`--principal` labels audit records; it is not authentication. The real boundary
is the OS user that owns the engine socket.

## 5. Tuning for agents

| flag | effect |
|---|---|
| `--tools concise` (default) | one line per tool, no parameter descriptions or output schemas; full definitions via `tb_search_tools` / `tb_describe_tool` |
| `--tools full` | every definition up front |
| `--payload both` (default) | text block and `structuredContent`, as the specification recommends |
| `--payload structured` | no text block, for clients that read `structuredContent` and would otherwise pay twice |
| `--payload text` | no `structuredContent` |

Measured discovery cost on the repo build (2026-11-16): 19 404 bytes in the
default `concise` and 59 127 in `full` for the 36-tool read-only surface; with
all grants, 52 tools cost 29 612 (`concise`) or 93 549 bytes (`full`) — see
[`../../docs/research/tool-surface-tokens-2026-11-16.md`](../../docs/research/tool-surface-tokens-2026-11-16.md)
for what the default drops and what a `tb_describe_tool` call gives back.

## 6. Troubleshooting

| symptom | cause and fix |
|---|---|
| `-32600 mutations are disabled by this server's launch policy` | the launch has no grant; restart the server with `--allow-mutations` or a policy file |
| `-32602 tool not found` | the name is misspelled or hidden by policy; call `tools/list` |
| `policy/prior_read: read this file with tb_content before writing it` | mutations need a fresh read of the same path in the same process |
| `outcome_unknown: ...` | the reply was lost; the write may have landed — call `tb_reconcile`, never retry blindly |
| `initialize required` (JSON-RPC error) | a legacy session must complete `initialize`; a request declaring `2026-07-28` in `_meta` does not |
| every read fails with `connecting to engine` | wrong socket path, or the daemon is down; check with the probe above. The first such failure also prints, once on stderr, the socket the launch tried, the state root that path came from, and the next step |
| `unsupported or missing engine schema_version (expected 29)` | the engine and this build disagree on the wire; rebuild against the served schema |
| the server is listed but a client shows no tools | for the Node daemon that means no `mcp.tool` plugin is mounted in the workspace; for this adapter, check `tools/list` directly |
| `engine error: malformed read: unknown variant <verb>` | the daemon is older than this build: it does not serve that read yet. Version skew is visible in the answer instead of a silent empty result. `tb_grep` in particular needs an engine built at or after `5625aff`; run a current daemon, or keep using the reads the running engine serves |
