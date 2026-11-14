# Verification index

This file indexes the artifacts in this repository that assert something about
the server's behaviour. Each row names the property, the files that prove it, the
command that re-runs or regenerates it, and the last measured result with its
date.

Counts taken from the code on 2026-11-14, `au-mcp-rs` HEAD `d09d480`. The release
binary used for the surface measurements was built from `1bcc08e` and reports
`au-mcp-rs 0.1.0+1bcc08ed8ee9`.

- **51 tools**: 35 advertised without a grant (29 engine reads plus 6 derived
  reads), 16 mutating behind three independent grants.
- **167 registered tests**: 165 run by default, 2 `#[ignore]`d because they need
  a built `au`.
- **29** synthetic read fixtures, **3** production golden payloads.
- **4** known engine test failures on this host, all needing a non-root runner.

Commands below run from `/root/arsumbris/au-mcp-rs` with
`RUSTUP_HOME=/root/.rustup`.

## 1. What is verified, and by what

| property | artifact | how to run it | last measured | date |
|---|---|---|---|---|
| mutation gate ordering and refusals | `src/policy.rs` (`Stage`: arguments -> path scope -> member grant -> prior read -> freshness or absence -> capacity -> editability); `tests/policy.rs` (6), `tests/write_file.rs` (7), `tests/create_file.rs` (3), `tests/structural.rs` (6), `tests/composition.rs` (6), `tests/batch.rs` (3) | `cargo +1.99.0 test --offline --test policy --test write_file --test create_file --test structural --test composition --test batch` | 31/31 pass; a launch without grants advertises 35 tools and refuses a mutation before the engine is contacted; a malformed policy stops the process before it serves | 2026-11-14 |
| `outcome_unknown` is never retried | `tests/faults.rs` (3), `tests/write_file_unknown.rs` (3), `tests/reconcile.rs` (4); the tool descriptions in `src/lib.rs` | `cargo +1.99.0 test --offline --test faults --test write_file_unknown --test reconcile` | 10/10 pass; a lost mutation reply is `outcome_unknown`, an engine rejection is not, and `au_reconcile` re-arms the read-before-write guard | 2026-11-14 |
| bounded stdio, in-flight cap, cancellation | `src/bounded_stdio.rs` (4 queued responses, 5 s outbound wait, 2 s close wait, 16 MiB inbound line); `MAX_IN_FLIGHT_READS = 8` in `src/lib.rs`; `tests/stdio_boundaries.rs` (8), `tests/cancellation.rs` (3), `tests/concurrent_reads.rs` (2) | `cargo +1.99.0 test --offline --test stdio_boundaries --test cancellation --test concurrent_reads` | 13/13 pass; a client that stops draining stdout is closed instead of buffered, EOF terminates within the bound, a cancelled read frees its slot and does not disturb another read | 2026-11-14 |
| protocol versions, including `2026-07-28` | `SUPPORTED_PROTOCOL_VERSIONS` in `src/lib.rs` (five revisions); `tests/modern_stdio.rs` (4), `tests/stdio.rs` (2), the lifecycle cases in `tests/stdio_boundaries.rs`; `--info` | `cargo +1.99.0 test --offline --test modern_stdio --test stdio --test stdio_boundaries`; `./target/release/au-mcp-rs --info` | `modern_stdio` 4/4 and `stdio` 2/2 pass; inline `_meta` metadata cannot bypass the legacy handshake; `--info` prints both families; `2026-07-28` is reached by tests only | 2026-11-14 |
| response budgets and paging | `tests/budgets.rs` (4); `au_content {max_chars}` and `au_instances_of {limit, offset}` in `src/lib.rs`; `docs/progressive-disclosure.md` | `cargo +1.99.0 test --offline --test budgets` | 4/4 pass; `au_content` returns an explicit window plus `truncated`/`total_chars`/`next_offset` and its hash still covers the whole file; `au_instances_of` reports `total` and `next_offset`; payload modes select the representation | 2026-11-14 |
| discovery cost and the detail ladder | `tests/discovery.rs` (4); `au_search_tools`/`au_describe_tool`; `--tools full` and `--tools concise`; `docs/progressive-disclosure.md` | `cargo +1.99.0 test --offline --test discovery`; measure a `tools/list` reply per profile | 4/4 pass; 51 tools: `tools/list` is **74 850** chars in `full` and **26 323** in `concise` (−65%; chars/4 is about 18 713 and 6 581); a read-only launch advertises 35 tools at 50 019 and 16 881 | 2026-11-14 |
| wire shapes vs the engine (synthetic fixtures) | `tests/fixtures/synthetic-reads/` (29 JSON, one per engine read), `tests/fixture_schema.rs` (2), `tests/real_engine.rs::synthetic_read_fixtures_match_the_engine` | verify: `AU_ENGINE_BIN=../au-engine/target/debug/au cargo +1.99.0 test --offline --test real_engine -- --ignored --test-threads=1`; regenerate after an intentional change: the same command with `CAPTURE_FIXTURES=1` | 29 fixtures parse, each names an advertised read and carries its documented payload key; the ignored real-engine run that replays them passes 2/2 in 3.15 s | 2026-11-14 |
| schema vs captured payloads | `tests/schema_contract.rs` (1); `src/schemas.rs` | `cargo +1.99.0 test --offline --test schema_contract` | 1/1 pass; the first run found two real drifts (`au_subtypes` declared only `subtypes`; `au_reconcile` declared an envelope key the engine never sends), both fixed in `1bcc08e` | 2026-11-14 |
| production golden replay | `tests/golden_replay.rs` (1); `/root/arsumbris/au-engine/tests-golden/*.json` (3 payloads: `au_overview`, `au_hubs`, `au_diagnostics`) | `cargo +1.99.0 test --offline --test golden_replay` | 1/1 pass; the adapter re-emits the captured payloads unchanged under the read's key. These three are the 2026-11-12 captures in the retired daemon's shapes and cannot be regenerated; they are history, not a contract | 2026-11-14 |
| the engine-shaped golden set is usable as evidence | `au-engine/tests-golden/engine/` (4 captures: `overview`, `hubs`, `diagnostics`, `graph_health`); `au-engine/crates/au-engine/tests/it/golden_captures.rs` (3); `au-engine/scripts/engine_probe.py` | `cargo +1.99.0 test --offline -p au-engine --test it golden_captures`; re-capture with `au-engine/scripts/engine_probe.py --socket <sock> --out au-engine/tests-golden/engine` | 3/3 pass: every capture parses, names a served read, carries that read's payload with the right JSON kind, holds no absolute workspace path and no rebuild counter, and the set on disk is exactly the recorded one. Two consecutive captures are byte-identical | 2026-11-14 |
| every read's wire request and payload, pinned without an engine | `tests/type_read.rs` (19), `tests/hubs_read.rs` (5), `tests/links.rs` (6), `tests/scan.rs` (8), `tests/types_read.rs` (3), `tests/type_tree.rs` (4), `tests/graph_health_read.rs` (2), `tests/grep_read.rs` (2), `tests/rollback_preview.rs` (2) | `cargo +1.99.0 test --offline --test type_read --test hubs_read --test links --test scan --test types_read --test type_tree --test graph_health_read --test grep_read --test rollback_preview` | 51/51 pass against a fake engine socket; exact request frames, payload pass-through, and recovery after engine errors and malformed frames | 2026-11-14 |
| MCP reads against a real engine | `tests/real_engine.rs` (2 `#[ignore]`d tests) | `AU_ENGINE_BIN=../au-engine/target/debug/au cargo +1.99.0 test --offline --test real_engine -- --ignored --test-threads=1` | 2/2 pass in 3.15 s on a disposable two-member repository; every advertised read replays and matches the fixture | 2026-11-14 |
| parity with the Python bridge | `scripts/bridge_parity.py`; `docs/bridge-parity.md` | `AU_ENGINE_BIN=../au-engine/target/debug/au python3 scripts/bridge_parity.py` (needs `bun` for the Node daemon) | PASS: 19 bridge tools vs 35 adapter tools in the fixture (a fixture artifact), one comparable pair (`mcp__au_diagnostics`), 3 identical findings | 2026-11-14 |
| live read A/B against the bridge | `docs/live-comparison.md`; no script | not scripted: call each read through both servers against `/root/vault` and diff after removing the bridge's projection envelope | 11 reads byte-identical across 95 types, 200 files, 100 hubs, 62 subtypes, 27 imports; `au_diagnostics` agreed by being empty on both sides | 2026-11-13 |
| client compatibility | `scripts/client-matrix.sh`; `docs/client-matrix.md` | `AU_ENGINE_BIN=/root/arsumbris/au-engine/target/release/au AU_MCP_RS_BIN=/root/arsumbris/au-mcp-rs/target/release/au-mcp-rs bash scripts/client-matrix.sh` (needs `claude`, `codex`, `cursor-agent`) | exit 0: Claude Code 2.1.323 connected (`initialize` + `tools/list`); Codex 0.164.0 registered, handshake not exercised; Cursor configured, not exercised | 2026-11-14 |
| a model actually chooses a tool | `scripts/model-turn.sh`; `scripts/lib/disposable-fixture.sh`; `docs/client-matrix.md` | `AU_ENGINE_BIN=... AU_MCP_RS_BIN=... AU_TURN_TRANSCRIPT=/tmp/t.jsonl bash scripts/model-turn.sh` (costs one model turn on the operator's credentials) | exit 0: the model called `mcp__arsumbris-rs__au_grep` with `{"query":"unique-marker-7f3a"}`, the engine's payload came back as a tool result (line 6, column 10, `total: 1`), the answer named `notes/a.md`, no permission denial; $0.042, 12 s | 2026-11-14 |
| a **second client** completes a model turn | `scripts/codex-turn.sh`; `docs/client-matrix.md` | `AU_ENGINE_BIN=... AU_MCP_RS_BIN=... AU_TURN_HOME=/root bash scripts/codex-turn.sh` (one Codex turn on the operator's credentials) | exit 0: Codex 0.169.0 called `au_grep {query: unique-marker-7f3a}`, received the engine payload as an `mcp_tool_call` result, and answered with `notes/a.md`; its first action was reading the installed `arsumbris` skill | 2026-11-14 |
| a model follows the recovery contract on a lost reply | `scripts/recovery-turn.sh`; `docs/client-matrix.md` | `AU_ENGINE_BIN=... AU_MCP_RS_BIN=... AU_TURN_HOME=/root bash scripts/recovery-turn.sh` (one model turn) | 5/5 pass: the model read, wrote once, called `au_reconcile`, did not retry, one engine commit, the line present. The transcript also exposed `au_reconcile` answering `not_applied` to the string `"null"`, now refused as a non-digest; a second run called reconcile with no digest and got a refusal that names the alternative | 2026-11-14 |
| a model edits, and the read-only launch cannot | `scripts/mutation-turn.sh` (two scenarios); `docs/client-matrix.md` | `AU_ENGINE_BIN=... AU_MCP_RS_BIN=... bash scripts/mutation-turn.sh` (two model turns on the operator's credentials; `AU_TURN_ONLY=1` runs the granted one) | PASS both: granted — the model called `au_content`, then `au_write_file` with the hash it read, the file on disk changed, the write landed as one new engine commit, no permission denial; read-only — `au_write_file` was never callable and the target file stayed untouched | 2026-11-14 |
| a lost mutation reply against a **real** engine | `scripts/lost-reply.sh`; `scripts/lost_reply_proxy.py`; `tests/faults.rs` (fake engine) | `AU_ENGINE_BIN=... AU_MCP_RS_BIN=... bash scripts/lost-reply.sh` | PASS: the write reached the engine and the reply was dropped; the adapter answered `outcome_unknown` once and did not retry; `au_reconcile` answered `applied` with the engine's real `HEAD`; the file changed; exactly one commit was added | 2026-11-14 |
| a batch that must not half-apply, and a rollback plan that changes nothing | `scripts/saga-and-rollback.sh`; `tests/batch.rs`, `tests/rollback_preview.rs` (fake engine) | `AU_ENGINE_BIN=... AU_MCP_RS_BIN=... bash scripts/saga-and-rollback.sh` | 9/9 pass against a real engine: a three-op `au_batch` whose third op misses its `old_string` is refused with `old_string not found`, no commit is made, the first two ops left no trace, the tree is unchanged; then a real write lands, and `au_rollback_preview` on that commit reports `notes/a.md` `action: restore`, `state: clean`, `safe: true`, with HEAD and the file untouched | 2026-11-14 |
| rollback rehearsal | `scripts/rollback_rehearsal.py`; `docs/rollback-rehearsal.md` | `AU_ENGINE_BIN=/root/arsumbris/au-engine/target/release/au AU_MCP_RS_BIN=/root/arsumbris/au-mcp-rs/target/release/au-mcp-rs python3 scripts/rollback_rehearsal.py` | exit 0: a granted write is one engine commit with a clean tree and no stray files, a fresh process reads the written text, and the same binary without `--allow-mutations` refuses the write | 2026-11-14 |
| live engine swap | `docs/engine-swap-2026-11-14.md`; no script | procedure: build `au-cli` release, freeze a vault copy and capture the baseline back to back, compare twelve reads, install, re-read the live socket, keep the previous binary | four swaps on the day; latest `b7e48c82...` from `015aeda`; twelve reads showed zero differences; `graph_health` gave 1156 nodes, 4565 edges, 211 components, 17 cycles over 664 nodes, 19 dead ends, 178 sources, 202 isolated files, 0 broken links; schema 29 on both sides | 2026-11-14 |
| engine environment baseline | `au-engine/tests-known-environment.txt` (4 named failures); `au-engine/README-fork.md`; `au-engine/scripts/run-known-root-tests.sh` proves they are root-only | `cd ../au-engine && cargo +1.99.0 test --offline --lib`; `cargo +1.99.0 test --offline --test it`; `bash scripts/run-known-root-tests.sh` | `--lib` 678 passed, 1 failed, 1 ignored; `--test it` 710 passed, 3 failed, 1 ignored; the four recorded names pass 4/4 as uid 65534; `fmt --check` exits 0 since 2026-11-14 and `clippy --workspace --all-targets` reports 85 warnings | 2026-11-14 |
| build identity and the unified binary | `tests/cli.rs` (10), `tests/version_metadata.rs` (2); `build.rs` | `cargo +1.99.0 test --offline --test cli --test version_metadata`; `--version`; `--info` | 12/12 pass; `--version` prints `au-mcp-rs 0.1.0+1bcc08ed8ee9`; `--info` reports 51 registered, 35 advertised, wire schema 29, 16 MiB frames, 30 s read deadline, 8 in-flight reads; `docs/roadmap.md` now states the same numbers (`16de348`, 165 tests, 2 ignored) | 2026-11-14 |
| MCP resources and change notification | `tests/resources.rs` (4), `tests/watch.rs` (3); `src/resources.rs` | `cargo +1.99.0 test --offline --test resources --test watch` | 7/7 pass; `arsumbris://<repo>/<path>` resolves and a foreign URI is refused before the engine; `au_watch` returns a delta and treats a timeout as an answer; no resource subscriptions are advertised | 2026-11-14 |

The documents agree with the code on 51 tools since 2026-11-14: `README.md` said
"5 derived reads" (which summed to 50 against its own 29 + 16), and a documentation
audit found it; the surface itself never changed.

## 2. What is NOT verified here

- **Four engine tests need a non-root runner.** They inject failure through
  `chmod`, which cannot block uid 0. `au-engine/scripts/run-known-root-tests.sh`
  re-runs exactly the recorded names as `nobody` and passes, so the failures are
  environmental — but the default `cargo test` run on this host still reports them,
  and nothing here makes an unprivileged runner the default.
- **Engine lints are red at HEAD; formatting is not.** `cargo fmt --check -p
  au-engine` exits 0 since 2026-11-14 (`cargo fmt` over 131 files, formatting only;
  `--lib` 678/1 and `--test it` 710/3 unchanged). `clippy --workspace --all-targets`
  reports 85 warnings — au-core 43, au-engine 38, au-testkit 4 — so `-D warnings`
  fails; all of them predate the fork, and the earlier "one clippy error" in the
  baseline was an artifact of clippy stopping at the first crate that fails. The
  engine's gate is its test suite plus the wire contract. The Rust MCP half is clean
  (`fmt --check` and `clippy -D warnings` both exit 0 at `d09d480`).
- **Two clients complete model turns, one does not.** Claude Code does (read, write,
  read-only refusal) and Codex does (read); Cursor is not exercised at all, because
  it needs an API key this host does not have. Client-side approval
  prompts and timeouts are still untouched, because both scripts allow-list the tools
  they ask for.
- **Compensation and rollback are proven by script, not by a model.** The engine's
  saga undo and the rollback plan hold against a real engine, but no model has chosen
  `au_batch` or `au_rollback_preview` yet.
- **The mutation turn is one scenario, not a campaign.** It proves the granted write
  path and the read-only refusal. It does not reach `outcome_unknown`, a lost reply,
  a compensating batch, or a rollback — those are covered by tests against a fake
  engine, not by a model.
- **`2026-07-28` is exercised only by our own tests.** Read off the wire with
  `scripts/stdio_tee.py`, Codex 0.169.0 asks for `2025-06-18` and the adapter answers
  `2025-06-18`; no installed client has requested the newer revision.
- **Bridge parity is one comparable read on a synthetic workspace.** The stronger
  eleven-read A/B was a manual session on 2026-11-13, not a repeatable command.
  Live `au_diagnostics` agreed by being empty on both sides. The live bridge tool
  count is stated as 49 in `docs/live-comparison.md` and as 44 in
  `docs/bridge-parity.md`; neither matches the 19 the disposable fixture
  discovers.
- **Fixtures are synthetic and the golden set is narrow.** The 29 fixtures come
  from a disposable two-member repository. `au-engine/tests-golden` holds 3
  payloads and has no `au_files` capture. Its README names `bridge_probe.py` as
  the capture client, but that file does not exist in this repository, so the
  golden payloads cannot be regenerated here.
- **Mutations are not exercised end to end except in the rehearsal.** Every gate
  test drives a fake engine socket. Only `scripts/rollback_rehearsal.py` writes
  through a real engine, and it does not lose a reply (that is `tests/faults.rs`)
  and touches no production socket.
- **The live swap has no script.** Its evidence is a hand-written record;
  reproducing the twelve-read comparison needs a frozen vault copy and a
  maintenance decision. It is the only artifact that touches `/root/vault` and its
  socket.
- **No performance data.** Discovery cost is a static byte count. There is no
  latency, throughput, or memory measurement, and the 8 in-flight cap is tested
  against a fake engine only.
- **No CI.** The repository carries no workflow configuration. Every artifact here
  is run by hand, so nothing enforces that it ran.

## 3. Regenerating everything

```sh
cd /root/arsumbris/au-mcp-rs
export RUSTUP_HOME=/root/.rustup

# 1. Static gates. No engine, no network once the dependency cache is warm.
cargo +1.99.0 fmt --check
cargo +1.99.0 clippy --offline --locked --all-targets --all-features -- -D warnings

# 2. The default suite: 165 tests pass, 2 ignored, no engine needed.
cargo +1.99.0 test --offline

# 3. Build the engine binary the later steps need. Uses kache and the shared
#    target directory; a cold dependency cache wants the network (drop --offline).
cargo +1.99.0 build --offline --manifest-path ../au-engine/Cargo.toml -p au-cli --bin au

# 4. Real-engine equivalence and fixture verification, on disposable temp repos.
AU_ENGINE_BIN=../au-engine/target/debug/au \
  cargo +1.99.0 test --offline --test real_engine -- --ignored --test-threads=1

# 5. Regenerate the synthetic corpus after an intentional wire change, then
#    verify it. This is the only step that writes tracked files.
CAPTURE_FIXTURES=1 AU_ENGINE_BIN=../au-engine/target/debug/au \
  cargo +1.99.0 test --offline --test real_engine -- --ignored --test-threads=1
AU_ENGINE_BIN=../au-engine/target/debug/au \
  cargo +1.99.0 test --offline --test real_engine -- --ignored

# 6. Bridge parity. Needs bun and the Node workspace; throwaway HOME.
AU_ENGINE_BIN=../au-engine/target/debug/au python3 scripts/bridge_parity.py

# 7. Client matrix. Needs claude, codex, and cursor-agent installed; throwaway HOME.
AU_ENGINE_BIN=/root/arsumbris/au-engine/target/release/au \
AU_MCP_RS_BIN=/root/arsumbris/au-mcp-rs/target/release/au-mcp-rs \
  bash scripts/client-matrix.sh

# 8. Rollback rehearsal. Disposable repository, private engine socket.
AU_ENGINE_BIN=/root/arsumbris/au-engine/target/release/au \
AU_MCP_RS_BIN=/root/arsumbris/au-mcp-rs/target/release/au-mcp-rs \
  python3 scripts/rollback_rehearsal.py

# 9. Engine baseline. Slow; expect the 4 recorded failures.
cd ../au-engine
cargo +1.99.0 test --offline --lib
cargo +1.99.0 test --offline --test it

# 10. Prove the 4 failures are the privileged runner, not the code.
bash scripts/run-known-root-tests.sh
```

What each step needs:

- **A built `au` engine binary**: steps 4, 5, 6, 7, 8. Step 3 builds it.
- **`bun` and the Node workspace**: step 6 only.
- **Installed client CLIs**: step 7 only. The script reports absent clients without
  failing.
- **`setpriv` and uid 65534**: step 10 only.
- **`kache` and a warm crate cache**: every cargo step. `--offline` refuses a
  network fetch, so a cold cache fails at step 2 rather than downloading.
- **The production vault or its socket**: none of the steps. They use temp
  directories, throwaway `HOME` values, and private sockets. The live swap in
  section 1 is the only procedure that touches production, and it is not part of
  this block.

The evidence documents (`docs/bridge-parity.md`, `docs/client-matrix.md`,
`docs/rollback-rehearsal.md`, `docs/live-comparison.md`, and
`docs/engine-swap-2026-11-14.md`) are written from these outputs by hand. No
command regenerates them.
