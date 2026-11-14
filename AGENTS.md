# AGENTS.md — arsumbris fork runbook

You are working in the arsumbris fork workspace. Development happens HERE
(allspark coding sessions run with this directory as the workdir); arsumbris
code is never vendored into allspark/prime-agent.

## What this is

A closed-distribution bundle of sibling repos implementing the arsumbris
typed knowledge-graph engine (typed markdown + `[[wiki-links]]` + `^block-ids`,
frontmatter contracts, MCP serving). Upstream: https://github.com/arsumbris
(engine last upstream commit 2026-09-25, `release 0.0.2-alpha`; upstream
development has stalled). The local bundle is NEWER than the last release:
local au-engine carries `0.0.5`-era unreleased work on
`feat/hotpath-profiling-and-memchr-wikilinks` (commit `44b32f4`).

## Layout

| path | role |
|---|---|
| `au-engine/` | Rust engine Cargo workspace; primary fork target. `fork/main` is the fork integration branch; `feat/hotpath-*` is pristine local upstream work |
| `au-mcp-rs/` | Experimental standalone Rust MCP stdio server; fixed read-only engine operations, explicit socket, not a production replacement |
| `au-engine-sdk/` | TypeScript client and wire helpers for the Rust engine |
| `au-mcp-bridge/` | Python stdio MCP bridge Hermes talks to (`~/.hermes/config.yaml` → `mcp_servers.arsumbris`). Own git now (first snapshot committed) |
| `au-type-system/` | the type-system contract. **Do not break**: vault format compatibility depends on it |
| `au-mcp/`, `au-mcp-sdk/`, `au-mcp-core/` | Existing MCP daemon, SDK contracts, tools, and hooks |
| `au-mcp-adapter-cc/`, `au-mcp-adapter-codex/` | Client-specific adapters |
| `au-ingest/`, `au-govern/` | ingest pipeline, workspace composition (`workspace.yaml` members) |
| `au-skills/`, `au-agent-guides/`, `au-rules/`, `au-writing-style/`, `au-competency/`, `au-defaults/` | content/knowledge repos |

## Workspace and Rust environment

- This directory is ONE Git repository (`github.com/rager306/arsumbris`,
  private). Every `au-*` directory inside it is a plain directory whose history
  was imported with `git subtree`; there are no nested repositories and no
  submodules. Run `git status` and `git log` once, at the monorepo root, and
  scope pathspecs when you mean one project (`git log -- au-engine`).
- The pre-monorepo sibling checkouts under `/root/arsumbris/` are untouched and
  remain readable references. Do not edit them expecting changes to appear
  here; treat this repository as the working tree.
- Use **Rust 1.99.0 and edition 2024** for Rust work. On this host the complete
  toolchain is under `/root/.rustup`; set `RUSTUP_HOME=/root/.rustup` and use
  `cargo +1.99.0`. The default rustup home has an incomplete 1.99.0 installation.
  Do not silently switch to 1.99.1 or another toolchain if 1.99.0 is unavailable;
  report the blocker. The `au-engine` workspace manifest currently declares
  `rust-version = "1.85"` (a minimum), while `rust-toolchain.toml` pins 1.99.0;
  the user-required development toolchain remains 1.99.0.
- Root `.graphignore` excludes local agent tooling, build output, dependencies,
  and caches from the native graph index. Source, tests, scenarios, and contract
  fixtures remain in scope. Individual sibling indexes may need their own rules.

### `au-engine` crates

All paths below are under `au-engine/crates/`; the workspace is defined in
`au-engine/Cargo.toml`.

| Crate | Purpose |
|---|---|
| `au-cli` | `au` executable, CLI commands, and daemon management |
| `au-engine` | Held knowledge-base state, build orchestration, daemon reads, wire protocol, and mutations |
| `au-core` | Type graph, closure and resolution, validation, and candidate analysis |
| `au-parser` | Filesystem input, frontmatter, Markdown, YAML and source-span parsing |
| `au-grammar` | Parser for type-field shape expressions |
| `au-references` | Wikilink parsing, file indexing, and reference resolution |
| `au-diagnostics` | Structured diagnostic codes, spans, and suggested fixes |
| `au-testkit` | Property generators and adversarial fixtures for tests |

`au-mcp-rs/` is a separate Cargo package, not an `au-engine` workspace member.
Its `src/main.rs` uses `rmcp` over stdio and the engine's Unix socket; `tests/`
contains fake-daemon tests and an opt-in real-engine test on a temporary fixture.
See `au-mcp-rs/README.md` and `au-engine/doc/adr/0001-implementation-plan.md` for
its current limitations and plan. Do not assume bridge, policy, or client parity.

## Live-system rules (read before touching anything)

1. This bundle SERVES the production vault at `/root/vault` (hundreds of
   live notes, every Hermes session reads/writes it through the daemon +
   bridge). The vault is a git repo — data is protected by history — but
   an engine bug can still degrade serving.
2. Never point tests at the live vault. Use a fixture copy:
   `git -C /root/vault worktree add /tmp/vault-fixture` or `cp -r`.
3. Never kill or restart the engine daemon / bridge processes while Hermes
   sessions are live. Socket: `~/.arsumbris/au-engine/run/*.sock`.
4. Engine swaps to the live daemon require the golden wire contract to pass
   (below) — no exceptions. A swap is a read comparison on a frozen copy of the
   vault working tree first, then the install, then the same reads against the
   live socket; `docs/engine-swap-2026-11-14.md` is a worked example, with the
   rollback artifact named in it. The served engine is now the fork build from
   `5625aff`, not the October bundle binary.

## Dev loop

```sh
cd au-engine
RUSTUP_HOME=/root/.rustup cargo +1.99.0 build
RUSTUP_HOME=/root/.rustup cargo +1.99.0 test -p <crate>  # smallest sufficient scope
```

Build layout (2026-11-14): every crate in the monorepo shares ONE target directory,
`/root/arsumbris/.cargo-target` (set in `.cargo/config.toml`), with
`au-engine/target` and `au-mcp-rs/target` as symlinks to it so documented paths keep
working. Before this, `au-mcp-rs/target` recompiled the whole engine dependency graph:
10.6 GiB of artifacts for one graph, now about 7 GiB. Dependencies are built without
incremental state (`[profile.dev.package."*"]` in both manifests) so the compiler cache
(`kache`, a global `rustc-wrapper` here) can serve them; our own crates keep it.

Two traps worth knowing: a stale `cargo test` from an earlier session can hold the target
lock for days and make every build look slow (`pgrep -af "cargo test"`), and an empty
`/tmp/.git` makes every `tempfile()` fixture look like a git tree, which turns ~40 engine
tests red (see `au-engine/README-fork.md`).

For the standalone MCP prototype, run from `au-mcp-rs/`:

```sh
RUSTUP_HOME=/root/.rustup cargo +1.99.0 fmt --check
RUSTUP_HOME=/root/.rustup cargo +1.99.0 test --locked
RUSTUP_HOME=/root/.rustup cargo +1.99.0 clippy --locked --all-targets --all-features -- -D warnings
```

The opt-in real-engine test and private fixture setup are documented in its
README. It requires a separately built `au` binary and must never use the live
vault or production socket.

Golden wire contract: `au-engine/tests-golden/engine/*.json` hold real engine
reads captured with `au-engine/scripts/engine_probe.py` (read-only). After
engine changes, responses must be identical unless the diff is intentional —
intentional diffs regenerate the fixture in the same commit, with the reason in
the message. The top-level `tests-golden/*.json` are the 2026-11-12 pre-fork
captures in the retired daemon's shapes; they are history, not a contract.

## Branch model (au-engine)

- `remotes/origin/main` = upstream `release 0.0.2-alpha` (frozen; stalled upstream)
- `feat/hotpath-profiling-and-memchr-wikilinks` = pristine local upstream work
  (`0.0.5`-era, commit `44b32f4`) — never rebase or amend
- `fork/main` = fork integration branch (created 2026-11-12 from `44b32f4`)
- Feature branches: `fork/<topic>`, conventional commits, English docs

## Improvement backlog

`/root/vault/inbox/arsumbris-fork-backlog.md` — P0 (FTS5 `MATCH` date-filter
quirk, ingest validation of broken `[[wiki-links]]`, stale-description
detector), P1 (bm25() ranking with type-aware freshness — durable types do
not decay, transient types half-life; enrichment columns keywords/questions
from a cheap LLM; `error→fix` atoms), P2 deferred (scheduled connectors,
dashboard). Backlog notes cite their source studies.

## Language

- Communicate with the user in Russian, including progress updates, reports,
  installation results, and explanations of errors. Translate user-facing
  output from tools or instructions written in another language into Russian.
- Write code identifiers, comments, documentation, configuration prose, and
  commit messages in English, unless the user explicitly requests otherwise.
  When adapting existing non-English text for these artifacts, translate it to
  English.
- Preserve exact commands, paths, identifiers, protocol values, and error text
  when precision matters; provide a Russian explanation or translation alongside.

## Working approach

- Act proactively toward the current repository implementation goal. Identify
  dependencies, risks, and the next useful steps; resolve issues within the
  requested scope instead of stopping at the first superficial fix. Report
  assumptions, blockers, and meaningful progress. Ask before expanding scope
  or taking consequential or irreversible actions.
- Actively research current best practices, including relevant 2026 guidance,
  when they can inform a design or implementation. Prefer authoritative,
  verifiable sources; check publication dates and applicability. Adapt guidance
  to this codebase and its contracts rather than adopting trends uncritically.
- Implement solutions that are clear, efficient, and elegant for the actual
  requirements. Prefer the simplest design that preserves behavior and safety;
  avoid unnecessary abstractions and unmeasured optimization.
- Verify changes with tests proportionate to their risk. Cover gaps in logic and
  semantics, operation sequences and state transitions, cascading effects,
  failure paths, and regressions where relevant. Test observable behavior and
  contracts, not implementation details alone; state any coverage limits.
- Follow the live-system rules and existing compatibility contracts throughout.
  Proactivity does not authorize production changes, destructive actions, or
  changes outside the user's requested scope.

## Discipline

- No git identity in-repo; global config only.
- Docs and commits in English.
- `au-type-system` is a contract, not a playground: type changes require a
  migration note and vault-rollback reasoning first.
- When a change alters wire shapes, bump/annotate per `au-engine/crates/au-engine/WIRE.md`.
