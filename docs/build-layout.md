# Build layout and disk budget

Measured on 2026-11-15, `/root/2brain`, toolchain 1.99.0. This file exists because
the build directory grew to 8.8 GiB and the reasons were not obvious from any manifest:
two profile mismatches between the workspaces, and incremental state that the compiler
cache can never serve.

## One target directory, two workspaces

`au-engine` and `au-mcp-rs` are separate Cargo workspaces over one dependency graph
(`au-mcp-rs` → `au-cli` → `au-engine`). `.cargo/config.toml` points both at
`/root/2brain/.cargo-target`, and `tb-engine/target` / `tb-mcp-rs/target` are
symlinks to it, so documented paths keep working. Before that unification the same
graph was compiled twice: 10.6 GiB.

A shared directory also means **the two manifests must agree on profile settings**.
Any difference changes the fingerprint and compiles the same crate twice into the same
directory — the most expensive duplicate there is.

## The profile budget

| setting | value | why |
|---|---|---|
| `debug` (dev, test) | `line-tables-only` | full DWARF was the largest single cost (~2 GiB). Backtraces and panic messages keep file:line; step-through variable inspection does not |
| `incremental` (dev, test) | `false`, workspace-wide | an incremental unit is never cacheable, so the compiler cache cannot serve it. Its redirected `incremental.kache-auto` directory had grown to 2.5 GiB |
| `release` (engine) | `opt-level = 3`, `lto = "thin"`, `codegen-units = 1` | release builds run the verification scripts; built rarely, run often |

Before these, `au-engine` had `line-tables-only` and `au-mcp-rs` did not, and
incremental was off only for dependencies (`[profile.dev.package."*"]`).

## Before and after

`cargo clean`, then one rebuild of everything the gates use: engine release, adapter
release, adapter tests, engine `--lib` and `--test it`.

| | before | after |
|---|---|---|
| `debug` profile total | 8.0 GiB | 3.1 GiB |
| `.cargo-target` total | 8.8 GiB | **4.0 GiB** |
| `debug/deps` | 4.7 GiB | 2.6 GiB |
| `debug/incremental.kache-auto` | 2.5 GiB | 4 KiB |
| `debug/build` | 209 MiB | 168 MiB |
| `release` | 784 MiB | 900 MiB |

The release directory grew by 116 MiB because the adapter's release binary is now
built in the same directory as the engine's; the debug half paid for it several times
over. Nothing the gates need was dropped: after the rebuild, `cargo test --offline`
runs 173 tests green, and both release binaries print their `+<commit>` build id.

`AGENTS.md` carries the short version of this, because that is the file a session
reads first.

## The compiler cache is a separate budget

`kache` holds 13.1 GiB of blobs against a 12 GiB limit, pruned automatically to it
(`Pruned 22.3 GiB automatically` over its lifetime). Its hit rate was 19.6% over the
last 24 h on 3,694 crate builds; incremental state is the main reason it was lower
before, because an incremental unit can never be cached. Treat the cache as a
different budget from the target directory: `cargo clean` reclaims artifacts,
`kache clean --cache` reclaims blobs, and neither touches the other.

## Re-measuring and reclaiming

```sh
du -sh .cargo-target .cargo-target/debug .cargo-target/release
du -sh .cargo-target/debug/* | sort -h | tail -6
kache stats            # cache size, hit rate, time saved
```

In order of what is safe to lose: `cargo clean` (all artifacts, keeps the cache),
`kache clean --cache` (the compile cache), `kache clean --units --dry-run` (obsolete
artifacts inside live targets, needs a receipt from `KACHE_TARGET_LIVENESS=1`).
Do not delete `.cargo-target` itself: `target-dir` is absolute, so a missing directory
only costs a rebuild, but both symlinks would dangle until it exists again.
