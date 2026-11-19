# tb-mcp-rs (the `2brain` adapter)

The MCP stdio server for the 2brain engine, and **the only serving path**: installed as
`/usr/local/bin/2brain` by `scripts/install.sh`.

**One binary**: `2brain`. There is no separate server binary any more — `2brain mcp <target>` is the
server, and `2brain mcp --help|--info|--version` answers with the server's own contract, which is why
those flags pass through the dispatcher unchanged.

The package, crate and directory are `tb-mcp-rs` / `tb_mcp_rs` since 2026-11-16 (`2b_` is not a legal Rust
prefix; see `../docs/naming.md`), and the binary is `2brain`. It reads a workspace through the engine's Unix
socket, and can write only when the launch grants it.

- **33 reads** (29 engine, 4 derived and observability), **2 discovery** tools, **1 watch**, and **16
  mutating** tools that appear only with an explicit grant (three grants in total). 52 with every
  grant, 36 with none.
- **Results are the engine's own object**, under the read's key: `tb_overview`
  answers `{"overview": ...}`. Nothing is re-shaped on the way out.
- **Three surfaces**: the tools above; MCP **resources** at `2brain://<repo>/<path>` (the legacy `arsumbris://` is still accepted on parse)
  for files a client wants to hold and re-read (`resources/list` pages the
  catalogue, `resources/read` returns the engine's text), plus two static
  workflow resources at `2brain://workflow/answer-shape` and
  `2brain://workflow/write-gates`; and MCP **prompts** — three workflows
  (`recall`, `intake`, `handoff`) that carry the answer contract
  (`docs/research/answer-contract-2026-11-16.md` in the monorepo) as text.
  The prompt and workflow text is static: it answers without the engine, and
  `tools/list` is byte-identical whether or not a client ever asks for the
  other two surfaces (`tests/answer_contract.rs` pins the bytes).
  No resource subscriptions: `tb_watch` carries change notification.
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

One binary is built from this package:

| binary | role | size |
|---|---|---|
| `2brain` | **one binary for both halves**: `daemon`, `open`, `members` (the engine), `mcp`, `info` (this server), and `init`, `doctor` (first run) | 23 MB |

`2brain` delegates the engine subcommands to `au_cli` and the MCP subcommand
to this crate, so the two halves cannot drift.

```sh
RUSTUP_HOME=/root/.rustup cargo +1.99.0 build --release --offline
./target/release/2brain --help
./target/release/2brain mcp ~/.arsumbris/au-engine/run/<hash>.sock
./target/release/2brain mcp --help         # the MCP server's own contract
./target/release/2brain mcp --version
./target/release/2brain mcp --info         # MCP surface, no engine needed
./target/release/2brain mcp /root/vault    # serve (a socket path works too)
./target/release/2brain info /root/vault   # surface + a probe of the live engine
./target/release/2brain init ./my-workspace  # a workspace the engine opens, and the next command
./target/release/2brain doctor ./my-workspace # ok/warn/fail per check; exit 0 only when nothing failed
```

`--info` is the self-description: server name and version, both protocol
families, the tool surface (52 registered, 36 advertised without a grant), the
result shape, the available profiles, the launch policy, and — when a socket is
reachable — `ready`, the engine's wire schema, its version, and the workspace
size. A bad option exits with status 2 and points at `--help`.

The release binary is a 6 MB ELF executable, dynamically linked only to `libc`,
`libm`, and `libgcc_s`. It needs a running engine daemon and that daemon's
socket; nothing else.

## Version and build identity

`--version` prints the package version plus the git commit the binary was built
from, as semver build metadata:

```
2brain 0.1.0+74e88f7e3688
2brain 0.1.0+<sha>                  # `.dirty` = uncommitted changes in this crate
```

`--info` adds the target triple and profile, and the same string is reported to
clients as `serverInfo.version`, so a bug report names an exact source. The
adapter's own identity is separate from the engine's: the engine version rides
`_meta["io.2brain/engineVersion"]` on every result.

The build identity comes from `build.rs` (`git rev-parse --short=12 HEAD`, plus a
dirty check scoped to this crate). A tarball build without `.git` reports
`unknown`. Rebuilds are triggered by `.git/HEAD`, the index, the reflog, and
packed refs — HEAD alone does not change on a commit, and watching only HEAD left
a rebuilt binary reporting the previous commit. `TB_BUILD_COMMIT` overrides the
commit for reproducible packaging.

## Launch policy

```sh
2brain mcp <socket>                                 # read-only
2brain mcp <socket> --allow-mutations               # explicit grant
2brain mcp <socket> --policy /etc/au-policy.json \
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
- `allow_composition` (default `false`) — a second grant for the two dependency
  verbs (`tb_add_dep`, `tb_remove_dep`). They author `.arsumbris/repo.yaml`, so a
  launch may trust writes to notes and still refuse to reshape the workspace.
  `--allow-composition` on the command line implies `--allow-mutations`.
- `allow_workspace_shape` (default `false`) — a third grant for the three
  member verbs (`tb_add_workspace_member`, `tb_remove_workspace_member`,
  `tb_set_workspace_member_role`). They change *which* members are served, so a
  mistake here alters every later read, not one note. `--allow-workspace-shape`
  implies `--allow-mutations`; the grants are independent of one another, so a
  launch can reshape members without touching dependencies or vice versa.
- `members` — exact allowlist of members a mutation may target; anything else is
  refused before the engine is contacted.
- `audit` — `"stderr"` (default), `{"file": "..."}` or `"disabled"`; `"disabled"`
  is rejected while mutations are allowed.
- A malformed or unreadable policy file aborts startup instead of falling back to
  a permissive default.

Every mutation decision is one JSON line with `principal`, `tool`, `member`,
`outcome` (`applied`, `rejected`, `outcome_unknown`, `denied:<stage>`), and a
timestamp — plus `seq`, `prev`, and `hash`. The records form a SHA-256 chain: each
`hash` covers the record without it, and each `prev` is the previous record's
hash, so editing a line breaks its own hash and deleting one breaks the next
line's link. A reader verifies with no server code: parse the line, drop `hash`,
serialize again, and compare. Appending to a file an earlier process wrote starts
a new chain at `seq` 1, so pin the file per launch or archive it between runs.

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
| `prior_read` | no `tb_content` read of this exact path is recorded |
| `freshness` | the recorded hash differs from `expected_hash` |
| `editability` | the engine reports the member as consumed |
| `capacity` | another mutation holds the single slot |

The slot is taken last, so a permit is never held while an engine read settles a
file. `tb_create_file` runs the same chain with `absence` in place of
`prior_read` + `freshness`: a create has no text to compare against, so its
promise is that the path is free.

A lost reply is `outcome_unknown`, never a retry: `tb_reconcile` says whether the
mutation landed and re-arms the guard. See [`docs/tools.md`](docs/tools.md).

## Agent-facing cost

Two profiles and three payload modes, measured on the repo build, 2026-11-16:

| setting | tools/list | tokens (chars/4) |
|---|---|---|
| `--tools concise` (default), 36 tools in a read-only launch | 19 404 bytes | ≈4 851 |
| `--tools full`, 36 tools in a read-only launch | 59 127 bytes | ≈14 782 |
| `--tools concise` (default), 52 tools with all grants | 29 612 bytes | ≈7 403 |
| `--tools full`, 52 tools with all grants | 93 549 bytes | ≈23 387 |
| `--payload structured` | halves the payload a client reads when it uses `structuredContent` | |

The concise profile is the default. It keeps every tool name, every parameter
name, type, enum, bound, default, and the required set; it shortens each
description to its first sentence and drops the output schema. What a client
loses by default is per-parameter semantics and the output envelope — one
`tb_describe_tool` call per tool returns the complete definition (measured
describe payloads are byte-identical under both profiles). A 28-scenario
routing test (26 answerable + 2 traps), two judge agents per arm, showed no
wrong-tool or schema-invalid-argument difference between the arms; the trade
and its evidence are in
[`../docs/research/tool-surface-tokens-2026-11-16.md`](../docs/research/tool-surface-tokens-2026-11-16.md).
`tb_search_tools` and `tb_describe_tool` serve the detail a concise profile
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
- Every path-taking tool rejects absolute paths and `..`, and resolves the path
  against the member's declared root before the engine is contacted.

These bound one adapter process; they are not a request rate limit and not a
substitute for process isolation with untrusted clients.

## Tests

```sh
RUSTUP_HOME=/root/.rustup cargo +1.99.0 test --offline          # 229 tests, 2 ignored
RUSTUP_HOME=/root/.rustup cargo +1.99.0 clippy --offline --locked --all-targets --all-features -- -D warnings
```

`tests/create_race.rs` runs in the default suite and is the one test that starts **two**
adapter processes: both create the same path against one private engine, twenty rounds, and
the assertions are one success, one `policy/absence` refusal, and the winner's bytes on disk.

The ignored real-engine test starts its own daemon on a synthetic repository with
a temporary `HOME`:

```sh
cd ../tb-engine && RUSTUP_HOME=/root/.rustup cargo +1.99.0 build -p tb-cli --bin au
cd ../tb-mcp-rs
RUSTUP_HOME=/root/.rustup TB_ENGINE_BIN=../tb-engine/target/debug/au \
  cargo +1.99.0 test --offline --test real_engine -- --ignored
```

Captured contracts:

- `tests/fixtures/mutations/` — one captured answer per mutating tool (16), because a
  validating client rejects an answer the declared schema does not describe, after the
  mutation has landed.
- `tests/fixtures/synthetic-reads/` — 32 captured reads, one per engine read tool, from a real engine on a
  disposable repository, paths as `$FIXTURE`, commit ids as `$COMMIT`; the ignored
  test re-verifies and `tests/fixture_schema.rs` checks the corpus without an
  engine.
- `tests/golden_replay.rs` — production-captured payloads from
  `tb-engine/tests-golden` replayed unchanged.

This suite does not establish full MCP conformance or production readiness.

## Verification scripts

| script | what it measures |
|---|---|
| `scripts/client-matrix.sh` | which installed MCP clients accept and connect the server ([result](docs/client-matrix.md)) |
| `scripts/model-turn.sh` | that a model, given the tool list, chooses a tool and gets the engine's answer back — costs one model turn on your credentials ([result](docs/client-matrix.md#2026-11-14-third-a-real-model-turn-and-it-found-the-marker)) |
| `scripts/mutation-turn.sh` | that a model edits a file through the gate (read → hash → write → one engine commit) and that a read-only launch cannot write at all — two model turns |
| `scripts/lost-reply.sh` | that a write whose reply was dropped becomes `outcome_unknown`, not a retry, and that `tb_reconcile` resolves it against the engine's real history — no model, no fake socket |
| `scripts/saga-and-rollback.sh` | that a refused `tb_batch` leaves no trace, and that `tb_rollback_preview` reports a plan without touching the tree |
| `scripts/codex-turn.sh` | the same read round trip as `model-turn.sh` through a second client implementation, Codex — one model turn |
| `scripts/live_parity.py` | that a live front end and this adapter answer the same facts on the served vault, read-only and re-runnable (its original bridge half is gone; it now compares two adapters) |
| `scripts/strict-client.py` | that a client which validates `structuredContent` against `outputSchema` accepts a mutation answer — the check a live session failed on 2026-11-15 |
| `scripts/batch-turn.sh` | that a model reaches for `tb_batch` instead of two single writes when the task spans files — one model turn |
| `scripts/rollback-turn.sh` | that a model plans a revert from the commit `tb_content` reports, and reads the plan — one model turn |
| `scripts/bridge_parity.py` | shared facts between the retired Python bridge and this adapter — historical, kept as the record behind [docs/bridge-parity.md](docs/bridge-parity.md) |
| `scripts/rollback_rehearsal.py` | that reverting to read-only loses nothing ([result](docs/rollback-rehearsal.md)) |

## Skill

`skills/2brain/SKILL.md` is an agent skill for working a workspace: surface
choice, tool ladder, envelope, read-then-write loop, and mutation recovery.
`references/tool-map.md` is generated from the running binary and loads only when
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
| [`docs/live-comparison.md`](docs/live-comparison.md) | A/B against the Python bridge on one workspace (historical: the bridge is deleted) |
| [`docs/client-matrix.md`](docs/client-matrix.md) | measured client compatibility |
| [`docs/bridge-parity.md`](docs/bridge-parity.md) | parity scope and the surface split (historical) |
| [`docs/rollback-rehearsal.md`](docs/rollback-rehearsal.md) | fallback-path rehearsal |

Rust 1.99 or newer is required.
