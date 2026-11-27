export CARGO_BUILD_JOBS := "4"
export NEXTEST_TEST_THREADS := "4"
export COLUMNS := "120"

default: gate-fast

# Fast local check (<10s)
gate-fast:
    cargo fmt --all -- --check
    python3 scripts/check_layer_deps.py
    cargo clippy --workspace --all-targets -- -D warnings

# Full Tier-1 verification gate
gate: gate-fast
    cargo nextest run --workspace
    cargo deny --offline check licenses bans sources

# Miri UB check on pure domain crates (0 I/O)
miri crate="tb-contracts":
    KACHE_DISABLED=1 cargo +nightly-2026-08-21 miri test -p {{crate}}

# Release build of the single `2brain` binary
build-release:
    cargo build --release -p two-brain --bin 2brain
