# au-mcp-rs

An MCP stdio server for the arsumbris engine. It reads a workspace through the
engine's Unix socket, and can write only when the launch grants it.

- **21 engine reads**, 2 discovery helpers, and 3 mutating tools that appear only
  with an explicit grant.
- **Results are the engine's own object**, under the read's key: `au_overview`
  answers `{"overview": ...}`. Nothing is re-shaped on the way out.
- **Read-only by default.** Without a grant the mutating tools are absent from
  `tools/list` and refused before the engine is contacted.
- **Typed answers**: every tool declares an `outputSchema`, every result carries
  `structuredContent`, and `initialize` returns instructions.
- **Single binary**: no runtime, interpreter, or package dependencies. It talks
  to a daemon it does not start.

Connecting a client, including the socket probe and per-client commands, is in
[`docs/connecting.md`](docs/connecting.md). The tool-by-tool reference is in
[`docs/tools.md`](docs/tools.md).

## Build and inspect

Two binaries are built from this package:

| binary | role | size |
|---|---|---|
| `arsumbris` | **one binary for both halves**: `daemon`, `open`, `members` (the engine) and `mcp`, `info` (this server) | 21 MB |
| `au-mcp-rs` | the MCP server alone, for an existing configuration that names it | 5 MB |

`arsumbris` delegates the engine subcommands to `au_cli` and the MCP subcommand
to this crate, so the two halves cannot drift from their standalone binaries.

```sh
RUSTUP_HOME=/root/.rustup cargo +1.99.0 build --release --offline
./target/release/arsumbris --help
./target/release/arsumbris mcp ~/.arsumbris/au-engine/run/<hash>.sock
./target/release/au-mcp-rs --help          # the MCP half alone
./target/release/au-mcp-rs --version
./target/release/au-mcp-rs --info          # MCP surface, no engine needed
./target/release/au-mcp-rs --info ~/.arsumbris/au-engine/run/<hash>.sock   # + probe
./target/release/au-mcp-rs ~/.arsumbris/au-engine/run/<hash>.sock          # serve
```

`--info` is the self-description: server name and version, both protocol
families, the tool surface (21 registered, 18 advertised without a grant), the
result shape, the available profiles, the launch policy, and — when a socket is
reachable — `ready`, the engine's wire schema, its version, and the workspace
size. A bad option exits with status 2 and points at `--help`.

The release binary is a 5 MB ELF executable, dynamically linked only to `libc`,
`libm`, and `libgcc_s`. It needs a running engine daemon and that daemon's
socket; nothing else.

## Launch policy

```sh
au-mcp-rs <socket>                                     # read-only
au-mcp-rs <socket> --allow-mutations                    # explicit grant
au-mcp-rs <socket> --policy /etc/au-policy.json \
          --principal "allspark"                        # scoped grant + audit
```

```json
{
  "allow_mutations": true,
  "members": ["vault"],
  "audit": "stderr"
}
```

- `allow_mutations` (default `false`) — whether the mutating tools exist at all.
- `members` — exact allowlist of members a mutation may target; anything else is
  refused before the engine is contacted.
- `audit` — `"stderr"` (default), `{"file": "..."}` or `"disabled"`; `"disabled"`
  is rejected while mutations are allowed.
- A malformed or unreadable policy file aborts startup instead of falling back to
  a permissive default.

Every mutation decision is one JSON line with `principal`, `tool`, `member`,
`outcome` (`applied`, `rejected`, `outcome_unknown`, `denied:<stage>`), and a
timestamp.

**Identity is not authentication.** `--principal` and the client's `initialize`
name only label audit records; a stdio client controls its own process. The real
boundary is the OS user that owns the socket.

## The mutation gate

An ordered chain; a refusal names its stage in the tool error
(`policy/<stage>: ...`) and in the audit:

| stage | refuses when |
|---|---|
| `arguments` | `repo`, `path`, or `expected_hash` is empty |
| `path_scope` | the path is absolute, contains `..` or NUL |
| `member_grant` | the member is not in the policy allowlist |
| `prior_read` | no `au_content` read of this exact path is recorded |
| `freshness` | the recorded hash differs from `expected_hash` |
| `capacity` | another mutation holds the single slot |
| `editability` | the engine reports the member as consumed |

A lost reply is `outcome_unknown`, never a retry: `au_reconcile` says whether the
mutation landed and re-arms the guard. See [`docs/tools.md`](docs/tools.md).

## Agent-facing cost

Two profiles and three payload modes, measured on the live workspace:

| setting | tools/list | tokens (chars/4) |
|---|---|---|
| `--tools full` (default) | 20 636 chars | ≈5 159 |
| `--tools concise` | 8 718 chars | ≈2 179 |
| `--payload structured` | halves the payload a client reads when it uses `structuredContent` | |

`au_search_tools` and `au_describe_tool` serve the detail a concise profile
withholds. The reasoning, sources, and break-even rule are in
[`docs/progressive-disclosure.md`](docs/progressive-disclosure.md).

## Protocol

Four legacy revisions and `2026-07-28`. A legacy session must complete
`initialize`; a modern request declares its version in `_meta` and needs none —
`server/discover`, `tools/list`, and `tools/call` work as a first message. A
second `initialize` in one session is rejected. A pre-lifecycle `ping` that
declares a version is answered by the transport: `-32022` when unsupported,
`-32601` for `2026-07-28`, which removed `ping`.

## Bounds

- At most 8 concurrent engine reads; further reads fail fast, no queue.
- One mutation at a time.
- A read has a 30 s deadline for connect, request and response.
- The stdio transport queues at most 4 responses, stops reading while full, and
  closes a connection that stalls for 5 s; shutdown waits 2 s for queued output.
- Wire checks: schema version 29, frame size (16 MiB), response type, correlation
  id, readiness, result envelope. Oversized frames fail rather than truncate.
- Only `au_content`, `au_write_file`, `au_edit_file`, and `au_delete_file` send a
  client path to the engine, after rejecting absolute paths and `..` and joining
  it to the member's declared root.

These bound one adapter process; they are not a request rate limit and not a
substitute for process isolation with untrusted clients.

## Tests

```sh
RUSTUP_HOME=/root/.rustup cargo +1.99.0 test --offline          # 108 tests
RUSTUP_HOME=/root/.rustup cargo +1.99.0 clippy --offline --locked --all-targets --all-features -- -D warnings
```

The ignored real-engine test starts its own daemon on a synthetic repository with
a temporary `HOME`:

```sh
cd ../au-engine && RUSTUP_HOME=/root/.rustup cargo +1.99.0 build -p au-cli --bin au
cd ../au-mcp-rs
RUSTUP_HOME=/root/.rustup AU_ENGINE_BIN=../au-engine/target/debug/au \
  cargo +1.99.0 test --offline --test real_engine -- --ignored
```

Captured contracts:

- `tests/fixtures/synthetic-reads/` — every advertised read, captured from a real
  engine on a disposable repository, paths as `$FIXTURE`, commit ids as
  `$COMMIT`; the ignored test re-verifies and `tests/fixture_schema.rs` checks the
  corpus without an engine.
- `tests/golden_replay.rs` — production-captured payloads from
  `au-engine/tests-golden` replayed unchanged.

This suite does not establish full MCP conformance or production readiness.

## Verification scripts

| script | what it measures |
|---|---|
| `scripts/client-matrix.sh` | which installed MCP clients accept and connect the server ([result](docs/client-matrix.md)) |
| `scripts/bridge_parity.py` | shared facts between the Python bridge and this adapter ([result](docs/bridge-parity.md)) |
| `scripts/rollback_rehearsal.py` | that reverting to read-only loses nothing ([result](docs/rollback-rehearsal.md)) |

## Skill

`skills/arsumbris/SKILL.md` is an agent skill for working a workspace: surface
choice, tool ladder, envelope, read-then-write loop, and mutation recovery.
`references/tool-map.md` holds the adapter-to-bridge mapping and loads only when
needed.

```sh
./scripts/install_skill.sh     # AGENT_SKILLS_DIR defaults to ~/.agents/skills
```

The skill loader does not follow symlinks, and a running host caches its skill
catalogue at start.

## Documentation

| file | content |
|---|---|
| [`docs/connecting.md`](docs/connecting.md) | socket discovery, probe, per-client registration, flags, troubleshooting |
| [`docs/tools.md`](docs/tools.md) | every tool, its arguments, and its failure modes |
| [`docs/runtime-map.md`](docs/runtime-map.md) | every module in the bundle, what the Rust binary covers, and the phase plan for the rest |
| [`docs/roadmap.md`](docs/roadmap.md) | the phase checklist with current status |
| [`docs/agent-interface.md`](docs/agent-interface.md) | measured first-call and per-call cost, best-practice checklist, open gaps |
| [`docs/progressive-disclosure.md`](docs/progressive-disclosure.md) | sources and technique catalogue for tool token economy |
| [`docs/live-comparison.md`](docs/live-comparison.md) | A/B against the Python bridge on one workspace |
| [`docs/client-matrix.md`](docs/client-matrix.md) | measured client compatibility |
| [`docs/bridge-parity.md`](docs/bridge-parity.md) | parity scope and the surface split |
| [`docs/rollback-rehearsal.md`](docs/rollback-rehearsal.md) | fallback-path rehearsal |

Rust 1.99 or newer is required.
