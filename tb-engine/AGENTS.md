# AGENTS.md — the Rust engine and the MCP adapter

This file applies to the Rust workspace under `tb-engine/`. The repository root's [`AGENTS.md`](../AGENTS.md)
carries the rules that apply everywhere: the layer split, the live-system rules, the gates, and how documentation
is organised. A session whose working directory is inside `tb-engine/` reads this file in addition to the root
one; **the closest file wins**. The adapter package has its own pointer at [`../tb-mcp-rs/AGENTS.md`](../tb-mcp-rs/AGENTS.md),
and the commands for both halves are in the Dev loop below.

## Workspace and Rust environment

- Use **Rust 1.99.0 and edition 2024** for Rust work. On this host the complete
  toolchain is under `/root/.rustup`; set `RUSTUP_HOME=/root/.rustup` and use
  `cargo +1.99.0`. The default rustup home has an incomplete 1.99.0 installation.
  Do not silently switch to 1.99.1 or another toolchain if 1.99.0 is unavailable;
  report the blocker. The `tb-engine` workspace manifest currently declares
  `rust-version = "1.85"` (a minimum), while `rust-toolchain.toml` pins 1.99.0;
  the user-required development toolchain remains 1.99.0.
- Root `.graphignore` excludes local agent tooling, build output, dependencies,
  and caches from the native graph index. Source, tests, scenarios, and contract
  fixtures remain in scope. Individual sibling indexes may need their own rules.

### `tb-engine` crates

All paths below are under `tb-engine/crates/`; the workspace is defined in
`tb-engine/Cargo.toml`.

| Crate | Purpose |
|---|---|
| `tb-cli` | the `au` executable, CLI commands, and daemon management |
| `tb-engine` | Held knowledge-base state, build orchestration, daemon reads, wire protocol, and mutations |
| `tb-core` | Type graph, closure and resolution, validation, and candidate analysis |
| `tb-parser` | Filesystem input, frontmatter, Markdown, YAML and source-span parsing |
| `tb-grammar` | Parser for type-field shape expressions |
| `tb-references` | Wikilink parsing, file indexing, and reference resolution |
| `tb-diagnostics` | Structured diagnostic codes, spans, and suggested fixes |
| `tb-testkit` | Property generators and adversarial fixtures for tests |

The engine binary is still named **`au`** (the unit and the scripts name it; it is
a device layout, not a repository directory — `docs/naming.md`).

`tb-mcp-rs/` is a separate Cargo package, not a `tb-engine` workspace member.
Its `src/main.rs` uses `rmcp` over stdio and the engine's Unix socket; `tests/`
contains fake-daemon tests and an opt-in real-engine test on a temporary fixture.
See `tb-mcp-rs/README.md` and `tb-engine/doc/plans/implementation-plan.md` for
its current limitations and plan. Do not assume bridge, policy, or client parity.


## Dev loop

```sh
cd tb-engine
RUSTUP_HOME=/root/.rustup cargo +1.99.0 build
RUSTUP_HOME=/root/.rustup cargo +1.99.0 test -p <crate>  # smallest sufficient scope
```

Build layout: every crate in the monorepo shares ONE target directory,
`/root/2brain/.cargo-target` (set in `.cargo/config.toml`), with
`tb-engine/target` and `tb-mcp-rs/target` as symlinks to it so documented paths keep
working. Before this, `tb-mcp-rs/target` recompiled the whole engine dependency graph:
10.6 GiB for one graph.

The profile settings are a budget, not a preference, and both manifests must agree —
they share the target directory, so a mismatch compiles the same crates twice:

| setting | value | why |
|---|---|---|
| `debug` (dev and test) | `line-tables-only` | full DWARF was the largest single cost (~2 GiB). Backtraces and panic messages keep file:line; step-through variable inspection does not |
| `incremental` (dev and test) | `false`, workspace-wide | an incremental unit is never cacheable, so it cannot be served by `kache` (the global `rustc-wrapper` here). Its redirected `incremental.kache-auto` directory had grown to 2.5 GiB on its own |
| `release` | `opt-level = 3`, `lto = "thin"`, `codegen-units = 1` (engine) | release builds run the verification scripts; they are built rarely and run often |

Measured on 2026-11-15, after `cargo clean` and one rebuild of everything the gates
need (engine release, adapter release, adapter tests, engine `--lib` and `--test it`):
the target directory went from 8.8 GiB (debug 8.0 + release 0.8) to **4.0 GiB**
(debug 3.1 + release 0.9) with the suites that build needs still green — the current
counts, their commands and their dates live under `gates` in `tools/cards/metrics.json`;
`docs/build-layout.md` records the breakdown and how to re-measure it.

Reclaiming space, in order of what is safe to lose: `cargo clean` (all artifacts, keeps
the cache), `kache clean --cache` (the compile cache), and `du -sh .cargo-target/*` to
see where it went. Do not delete `.cargo-target` itself — `target-dir` is absolute and
a missing path only costs a rebuild, but the symlinks in both crates would dangle.

Two traps worth knowing: a stale `cargo test` from an earlier session can hold the target
lock for days and make every build look slow (`pgrep -af "cargo test"`), and an empty
`/tmp/.git` makes every `tempfile()` fixture look like a git tree, which turns ~40 engine
tests red (see `tb-engine/README-fork.md`).

For the standalone MCP prototype, run from `tb-mcp-rs/`:

```sh
RUSTUP_HOME=/root/.rustup cargo +1.99.0 fmt --check
RUSTUP_HOME=/root/.rustup cargo +1.99.0 test --locked
RUSTUP_HOME=/root/.rustup cargo +1.99.0 clippy --locked --all-targets --all-features -- -D warnings
```

The opt-in real-engine test and private fixture setup are documented in its
README. It requires a separately built `au` binary and must never use the live
vault or production socket.

Golden wire contract: `tb-engine/tests-golden/engine/*.json` hold real engine
reads captured with `tb-engine/scripts/engine_probe.py` (read-only). After
engine changes, responses must be identical unless the diff is intentional —
intentional diffs regenerate the fixture in the same commit, with the reason in
the message. The top-level `tests-golden/*.json` are the 2026-11-12 pre-fork
captures in the retired daemon's shapes; they are history, not a contract.

Gates: `cargo fmt --check` is clean for both halves and is a real gate, and so is
`cargo clippy --all-targets -- -D warnings` — `.github/workflows/ci.yml` runs both
in the adapter job and in the engine job.
