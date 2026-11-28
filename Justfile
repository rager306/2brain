export CARGO_BUILD_JOBS := "4"
export NEXTEST_TEST_THREADS := "4"
export COLUMNS := "120"

default: gate-fast

# Fast crate-scoped inner edit loop (R3: use before full workspace gate)
check crate="two-brain":
    cargo check -p {{crate}} --all-targets

# Fast crate-scoped test loop (optional nextest filter substring)
test crate="tb-app" filter="":
    @if [ -n "{{filter}}" ]; then \
        cargo nextest run -p {{crate}} -E 'test({{filter}})'; \
    else \
        cargo nextest run -p {{crate}}; \
    fi

# Fast local check (<10s)
gate-fast:
    cargo fmt --all -- --check
    python3 scripts/check_layer_deps.py
    cargo clippy --workspace --all-targets -- -D warnings

# Full Tier-1 verification gate (run once at commit/milestone boundaries)
gate: gate-fast
    cargo nextest run --workspace
    cargo deny --offline -L error check licenses bans sources

# Miri UB check on pure domain crates (0 I/O)
miri crate="tb-contracts":
    KACHE_DISABLED=1 cargo +nightly-2026-08-21 miri test -p {{crate}}

# Release build of the single `2brain` binary
build-release:
    cargo build --release -p two-brain --bin 2brain
