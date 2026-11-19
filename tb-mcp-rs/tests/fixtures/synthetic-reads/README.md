# Synthetic read fixtures

One file per engine read tool (32, one per engine read tool among the 35 a read-only launch advertises;
the rest are derived reads with no wire verb), captured by calling that tool through the MCP
layer against a real engine on a disposable two-member repository. Each file is
`{ "tool", "arguments", "payload" }`; paths under the temporary fixture root are
recorded as `$FIXTURE`, and git object names as `$COMMIT`, so the corpus carries
neither machine paths nor capture-specific commit ids.

Both members are single-commit git work trees, which is what the engine requires
for mutation and what makes `git.root` deterministic instead of depending on
whatever directory encloses the temporary parent.

## Regenerate

```sh
RUSTUP_HOME=/root/.rustup cargo +1.99.0 build --manifest-path ../tb-engine/Cargo.toml -p tb-cli --bin au
CAPTURE_FIXTURES=1 RUSTUP_HOME=/root/.rustup TB_ENGINE_BIN=../tb-engine/target/debug/au \
  cargo +1.99.0 test --offline --test real_engine -- --ignored --test-threads=1
```

## Verify

```sh
RUSTUP_HOME=/root/.rustup TB_ENGINE_BIN=../tb-engine/target/debug/au \
  cargo +1.99.0 test --offline --test real_engine -- --ignored
```

The ignored test fails when a read's payload drifts from the recording, so an
intentional change regenerates the affected files in the same commit. The
non-ignored `tests/fixture_schema.rs` keeps the corpus honest without an engine:
every fixture parses, names its own tool, carries the documented payload key, and
leaks no absolute temporary path.

These fixtures were captured on a synthetic repository, not the production vault;
the production payloads in `tb-engine/tests-golden` remain the contract for
`tb_overview`, `tb_hubs`, and `tb_diagnostics`.
