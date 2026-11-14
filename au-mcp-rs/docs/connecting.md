# Connecting an MCP client

The server speaks MCP over stdio and needs two things: its own path, and the
path of the **engine socket** it reads. It never starts a daemon and never
derives a default socket.

## 1. Build

```sh
cd au-mcp-rs
RUSTUP_HOME=/root/.rustup cargo +1.99.0 build --release --offline
# target/release/arsumbris  — engine daemon + MCP server in one binary
# target/release/au-mcp-rs  — the MCP server alone
```

Use `arsumbris mcp <socket>` everywhere below; `au-mcp-rs <socket>` is the same
server with the same flags, kept so an existing configuration keeps working.

The result is one self-contained executable (6 MB, dynamically linked only to
`libc`, `libm`, `libgcc_s`). It has no runtime, interpreter, or package
dependencies; the engine daemon is a separate process it talks to over a Unix
socket.

## 2. Find the engine socket

```sh
ls ~/.arsumbris/au-engine/run/*.sock
```

There is one socket per served workspace, named by a hash of the workspace path.
When several exist, the bridge's socket name under `~/.arsumbris/au-mcp/run/`
matches the engine socket for the same workspace, so comparing names pairs them.

Check it answers before wiring a client:

```sh
python3 - <<'PY'
import json, socket, struct
path = "/root/.arsumbris/au-engine/run/adcfe8680a485529.sock"
with socket.socket(socket.AF_UNIX) as s:
    s.connect(path)
    body = json.dumps({"read": "overview", "id": 1}).encode()
    s.sendall(struct.pack(">I", len(body)) + body)
    n = struct.unpack(">I", s.recv(4))[0]
    reply = json.loads(s.recv(n))
print(reply["ready"], reply["schema_version"])
PY
```

`ready: true` and `schema_version: 29` mean this build can serve it.

## 2b. Check what the binary says about itself

```sh
arsumbris --help                       # both halves and their subcommands
arsumbris --version                    # version + build commit
arsumbris info                         # MCP surface and launch defaults, no engine
arsumbris info <engine.sock>           # the same, plus ready/schema/version/workspace
arsumbris daemon status <entry>        # engine side, same as `au daemon status`
au-mcp-rs --help / --version / --info  # the MCP half alone
```

`--info` never needs a running daemon; with a socket it adds the probe result and
still exits 0 when the engine is unreachable, so it is safe in a health script.

## 3. Register the server

### allspark (Prime Agent)

`~/.allspark-home/.prime/agent/settings.json`, under `mcpServers`:

```json
"arsumbris-rs": {
  "type": "stdio",
  "command": "/root/arsumbris/au-mcp-rs/target/release/arsumbris",
  "args": ["mcp", "/root/.arsumbris/au-engine/run/adcfe8680a485529.sock"]
}
```

The host reads `mcpServers` at session start, so a new entry appears in a new
session. A running session may pick it up later, but do not rely on it: verify
with `mcp.list_connections()` and `mcp.list_tools("arsumbris-rs")`.

### Claude Code

```sh
claude mcp add --scope local arsumbris-rs -- \
  /root/arsumbris/au-mcp-rs/target/release/arsumbris mcp \
  /root/.arsumbris/au-engine/run/adcfe8680a485529.sock
claude mcp list          # expect: arsumbris-rs ... ✔ Connected
```

### Codex CLI

```sh
codex mcp add arsumbris-rs -- \
  /root/arsumbris/au-mcp-rs/target/release/arsumbris mcp \
  /root/.arsumbris/au-engine/run/adcfe8680a485529.sock
codex mcp list           # expect: arsumbris-rs  ...  enabled
```

Codex has no non-interactive health check, so `list` proves the configuration
shape, not the handshake.

### Any client that takes JSON

```json
{
  "mcpServers": {
    "arsumbris-rs": {
      "command": "/root/arsumbris/au-mcp-rs/target/release/arsumbris",
      "args": ["mcp", "/root/.arsumbris/au-engine/run/adcfe8680a485529.sock"]
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
"args": ["mcp", "/root/.arsumbris/au-engine/run/adcfe8680a485529.sock",
         "--policy", "/etc/au-mcp-policy.json",
         "--principal", "allspark"]
```

`--principal` labels audit records; it is not authentication. The real boundary
is the OS user that owns the engine socket.

## 5. Tuning for agents

| flag | effect |
|---|---|
| `--tools concise` | one line per tool, no parameter descriptions or output schemas; full definitions via `au_search_tools` / `au_describe_tool` |
| `--tools full` (default) | every definition up front |
| `--payload both` (default) | text block and `structuredContent`, as the specification recommends |
| `--payload structured` | no text block, for clients that read `structuredContent` and would otherwise pay twice |
| `--payload text` | no `structuredContent` |

Measured discovery cost on the live workspace: 74 850 chars in `full` and
26 323 in `concise` for the 51-tool surface with all grants; a read-only launch
advertises 35 tools at 50 019 and 16 881 chars (see
`docs/progressive-disclosure.md`).

## 6. Troubleshooting

| symptom | cause and fix |
|---|---|
| `-32600 mutations are disabled by this server's launch policy` | the launch has no grant; restart the server with `--allow-mutations` or a policy file |
| `-32602 tool not found` | the name is misspelled or hidden by policy; call `tools/list` |
| `policy/prior_read: read this file with au_content before writing it` | mutations need a fresh read of the same path in the same process |
| `outcome_unknown: ...` | the reply was lost; the write may have landed — call `au_reconcile`, never retry blindly |
| `initialize required` (JSON-RPC error) | a legacy session must complete `initialize`; a request declaring `2026-07-28` in `_meta` does not |
| every read fails with `connecting to engine` | wrong socket path, or the daemon is down; check with the probe above |
| `unsupported or missing engine schema_version (expected 29)` | the engine and this build disagree on the wire; rebuild against the served schema |
| the server is listed but a client shows no tools | for the Node daemon that means no `mcp.tool` plugin is mounted in the workspace; for this adapter, check `tools/list` directly |
| `engine error: malformed read: unknown variant <verb>` | the daemon is older than this build: it does not serve that read yet. Version skew is visible in the answer instead of a silent empty result. `au_grep` in particular needs an engine built at or after `5625aff`; run a current daemon, or keep using the reads the running engine serves |
