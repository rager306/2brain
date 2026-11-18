# AGENTS.md — the MCP adapter

The Rust workspace rules, the toolchain, the crate map and the build/test commands are in
[`../tb-engine/AGENTS.md`](../tb-engine/AGENTS.md); this repository's root [`AGENTS.md`](../AGENTS.md) carries the
repository-wide rules. Read both — this file exists so that a session started here finds them.

What this package is: the serving path. `src/main.rs` speaks MCP over stdio and the engine over a Unix socket;
`src/gate.rs` is the mutation gate; `src/schemas.rs` mirrors `WIRE.md`, which is the contract. `tests/` holds the
fake-daemon suite and an opt-in real-engine test. Never point a test at the live vault or the production socket
(root `AGENTS.md`, live-system rules).

Local commands, also listed in the Dev loop of `../tb-engine/AGENTS.md`:

```sh
RUSTUP_HOME=/root/.rustup cargo +1.99.0 fmt --check
RUSTUP_HOME=/root/.rustup cargo +1.99.0 test --locked
RUSTUP_HOME=/root/.rustup cargo +1.99.0 clippy --locked --all-targets --all-features -- -D warnings
```

Design notes and the closed completion record: `README.md`, `docs/roadmap.md`, `docs/tools.md`,
`docs/verification.md`.
