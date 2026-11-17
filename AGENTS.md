# AGENTS.md — 2brain runbook

You are working in the 2brain workspace (`/root/2brain`). Development happens HERE
(allspark coding sessions run with this directory as the workdir); product code is never vendored
into allspark/prime-agent.

## What this is

**2brain** is a Rust implementation of a typed knowledge graph (typed markdown + `[[wiki-links]]` +
`^block-ids`, frontmatter contracts, MCP serving) with a judgement layer kept outside the serving path.
It is a fork of the `arsumbris` project (<https://github.com/arsumbris>), which stalled after
`0.0.2-alpha`; the `0.0.5`-era upstream work is preserved as the tag `archive/upstream-0.0.5`
(commit `44b32f4`). The only place the upstream name belongs now is the lineage paragraph in
`README.md`; the engine's file format still spells `au.engine.*` and `.arsumbris/`, which is a format
waiting for its migration (`docs/rebrand-plan.md`, tier C).

## Layout

| path | role |
|---|---|
| `tb-engine/` | Rust engine Cargo workspace — the authoritative writer, serving `/root/vault` through `2brain-engine.service` |
| `tb-mcp-rs/` | **The serving path**: the Rust MCP adapter behind `/usr/local/bin/2brain` (52 tools, fail-closed gate, `scripts/install.sh` builds and installs it) |
| ~~`au-mcp-bridge/`~~ | **retired 2026-11-16** — the Python stdio bridge Hermes used to speak; removed with the Node daemon when Hermes moved to `/usr/local/bin/2brain` (`docs/internal/hermes-2brain.md`) |
| `tb-type-system/` | the type-system contract. **Do not break**: vault format compatibility depends on it |
| `tb-mcp-sdk/` | the `mcp.*` vocabulary: 17 type definitions (`mcp.tool`, `mcp.skill`, `mcp.inject`, `agent-profile`) that content notes are written in. Its TypeScript was deleted 2026-11-16 |
| `tb-ingest/`, `tb-govern/` | ingest pipeline, workspace composition (`workspace.yaml` members) |
| `tb-skills/`, `tb-agent-guides/`, `tb-rules/`, `tb-writing-style/`, `tb-competency/`, `tb-defaults/` | content/knowledge repos |

## Branch model

There is **one branch**: `main`, tracking `origin` (`github.com/rager306/2brain`). Development
happens on `main` or on short-lived feature branches that merge back into it.

**Upstream is detached (2026-11-16).** The upstream project stalled; its work lives here as
imported history, not as a branch to merge from. Merges from upstream are not planned. Where a
document needs to cite upstream state, it cites a hash or an archive tag:

| what upstream was | how it is cited |
|---|---|
| engine `0.0.5`-era work, including the hotpath profiling and memchr wikilink scan | commit `44b32f4`, tagged `archive/upstream-0.0.5` |
| release `0.0.2-alpha` | whichever commit the release pointed at, tagged `archive/upstream-0.0.2-alpha` when identified |
| the sibling repositories the bundle was assembled from | the `Add '<dir>/' from commit '<sha>'` merge commits in history; their content now lives in the `tb-*` directories, and their `remote:` anchors were removed when the tree became one repository |

Older revisions of this section named `remotes/origin/main`, `feat/hotpath-profiling-and-memchr-wikilinks`
and `fork/main`. None of those branches exist; the names are kept here only so an old document that
mentions them can be read.

## Code, tooling, and models: three layers, one rule each

Every file here belongs to exactly one of three layers, and the layer decides the language
and the review bar. Name the layer before adding a file.

| layer | where | language | rule |
|---|---|---|---|
| **Product code** | `tb-engine/`, `tb-mcp-rs/`, `tb-ingest/`, `tb-govern/` | **Rust only** | it runs in the serving path; nothing here may import, spawn, or require a script, a notebook, or a prototype |
| **Tooling** | `scripts/`, `tools/`, test support | shell, Python, or Rust — whatever fits | it may drive the product, never be driven by it; it exits honestly (non-zero on failure) and prints what it measured; it is never on the request path |
| **Models** | `tools/*`, spikes, a section of `docs/` | any language | allowed, and often the cheapest way to settle a design question — but it is labelled a model, it is never cited as shipped behaviour, and it names where the real implementation would live |

Rules that make the split hold:

- **A model that something other than a human reads has graduated.** Either rewrite it in
the product's language or move it into tooling with tests. Nothing sits in between.
- **Tooling may be replaced without touching the serving path.** If changing `scripts/`
forces a change in `tb-mcp-rs/`, the boundary was crossed in the wrong direction.
- **No Rust crate depends on a script or an interpreter.** A `python3` invocation inside
Rust is a design smell; a `python3` invocation inside a shell script is fine.
- ~~The one serving-path exception is `au-mcp-bridge/`~~ (Python). **Removed 2026-11-16**: the serving
  path is Rust end to end — `/usr/local/bin/2brain` (adapter) → engine socket. There is no exception left.
It is legacy scheduled for retirement by `docs/phase-d.md`, not a precedent to copy.
- **Every model states its evidence:** what it measured, on what corpus, and what it did not
measure. `docs/internal/w4.1-judgement-run-2026-11-16.md` is the shape to copy.

Worked examples, so the rule is not abstract:

| what | layer | why there |
|---|---|---|
| `tb-mcp-rs/src/gate.rs` — the mutation gate's state machine | product | the serving path walks it on every write |
| `tb-mcp-rs/scripts/lib/procedure.py` — two procedures as states, with `--selftest` | tooling | its only consumer is `mutation-turn.sh`, a verification script; no Rust file imports it |
| the Python BM25 prototype behind `docs/architecture-plan.md` T1 | model | it produced numbers and nothing else; the index that shipped is Rust (`tb-engine/crates/tb-engine/src/search.rs`) |

If the procedure runtime ever gets a **product** consumer — the server itself running an
agent procedure — it moves to `tb-mcp-rs/src/procedure.rs` and gains a tool; until then its
home is `scripts/`.

## Workspace and Rust environment

- This directory is ONE Git repository (`github.com/rager306/2brain`, public
  since 2026-11-16). Every `tb-*` directory inside it is a plain directory whose
  history was imported with `git subtree`; there are no nested repositories and
  no submodules. Run `git status` and `git log` once, at the monorepo root, and
  scope pathspecs when you mean one project (`git log -- tb-engine`).
- The pre-monorepo sibling checkouts (the old `/root/arsumbris` layout) are
  untouched and remain readable references. Do not edit them expecting changes
  to appear here; treat this repository as the working tree.
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
See `tb-mcp-rs/README.md` and `tb-engine/doc/adr/0001-implementation-plan.md` for
its current limitations and plan. Do not assume bridge, policy, or client parity.

## Live-system rules (read before touching anything)

1. This bundle SERVES the production vault at `/root/vault` (hundreds of
   live notes; every agent session reads/writes it through the daemon and the
   MCP server). The vault is a git repo — data is protected by history — but
   an engine bug can still degrade serving.
2. Never point tests at the live vault. Use a fixture copy:
   `git -C /root/vault worktree add /tmp/vault-fixture` or `cp -r`.
3. Never kill or restart the engine daemon or an MCP server process while agent
   sessions are live. Socket: `~/.2brain/au-engine/run/*.sock` (`~/.arsumbris` is a symlink).
4. Engine swaps to the live daemon require the golden wire contract to pass
   (below) — no exceptions. A swap is a read comparison on a frozen copy of the
   vault working tree first, then the install, then the same reads against the
   live socket; `docs/internal/engine-swap-2026-11-14.md` is a worked example, with the
   rollback artifact named in it. The served build is whatever the last swap
   record in `docs/internal/engine-swap-*.md` names; `2brain --version` and the
   daemon's own build id are authoritative, not this file.

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
(debug 3.1 + release 0.9) with `cargo test --offline` still 170 green;
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

## Docs: public design vs \`docs/internal/\`

Two kinds of document live in \`docs/\`, and the split is a rule, not a habit:

- **design** — \`docs/*.md\` and \`docs/research/*.md\`. Claims about the system that survive a different
  machine: architecture, naming, the trajectory, measurements, the evidence index.
- **operations** — \`docs/internal/\`. Every document that names this host: \`/root/vault\`, a systemd unit, a
  socket path, the Hermes config, a backup directory, or the run log of one operator's evening.

If a document you are writing names a host path, a unit, or a client configuration, it goes to
\`docs/internal/\`. \`docs/internal/README.md\` holds the table and the reasoning. Internal documents link to the
design documents they serve; design documents must not need an internal one to be understood, because the
repository is public and the internal set is the part a second machine would rewrite first.

## Discipline

- No git identity in-repo; global config only.
- Docs and commits in English.
- `tb-type-system` is a contract, not a playground: type changes require a
  migration note and vault-rollback reasoning first.
- When a change alters wire shapes, bump/annotate per `tb-engine/crates/tb-engine/WIRE.md`.
