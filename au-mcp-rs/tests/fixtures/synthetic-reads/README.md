# Synthetic read fixtures

One file per advertised read tool, captured by calling that tool through the MCP
layer against a real engine on a disposable two-member repository. Each file is
`{ "tool", "arguments", "payload" }`; paths under the temporary fixture root are
recorded as `$FIXTURE`, so the corpus carries no machine paths.

Both members are single-commit git work trees, which is what the engine requires
for mutation and what makes `git.root` deterministic instead of depending on
whatever directory encloses the temporary parent.

## Regenerate

```sh
RUSTUP_HOME=/root/.rustup cargo +1.99.0 build --manifest-path ../au-engine/Cargo.toml -p au-cli --bin au
CAPTURE_FIXTURES=1 RUSTUP_HOME=/root/.rustup AU_ENGINE_BIN=../au-engine/target/debug/au \
  cargo +1.99.0 test --offline --test real_engine -- --ignored --test-threads=1
```

## Verify

```sh
RUSTUP_HOME=/root/.rustup AU_ENGINE_BIN=../au-engine/target/debug/au \
  cargo +1.99.0 test --offline --test real_engine -- --ignored
```

The ignored test fails when a read's payload drifts from the recording, so an
intentional change regenerates the affected files in the same commit. The
non-ignored `tests/fixture_schema.rs` keeps the corpus honest without an engine:
every fixture parses, names its own tool, carries the documented payload key, and
leaks no absolute temporary path.

These fixtures were captured on a synthetic repository, not the production vault;
the production payloads in `au-engine/tests-golden` remain the contract for
`au_overview`, `au_hubs`, and `au_diagnostics`.
