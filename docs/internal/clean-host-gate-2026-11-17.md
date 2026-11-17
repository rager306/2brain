# Clean-host gate, 2026-11-17: two binaries and a vault copy on bkk-edge01

The release-hygiene lane (H) asks one question: does the system run on a machine that has nothing
but the artifacts? This is that machine — `bkk-edge01` (43.152.237.209, `aleksei`, x86_64 Linux,
2 cores, 1 GB RAM), with **no Rust, no cargo, no sqlite3, no repository clone**.

## What was placed there

| artifact | identity |
|---|---|
| adapter | `~/gate/bin/2brain`, sha256 `17c36204…`, `0.1.0+020d0de64469` |
| engine | `~/gate/bin/au`, sha256 `a6f9a3f4…`, `0.0.5` |
| workspace | `~/gate/vault`, rsync of `/root/vault` without `.git` (8.4 MB) |
| home | `~/gate/home` — deliberately empty, so the daemon creates its state there |

The engines talks to the adapter by socket resolved from `HOME`; nothing in the binaries refers to
`/root`, and nothing was compiled on the host.

## Result

`2brain doctor ~/gate/vault` → **exit 0, 6 ok, 1 warn**:

```
ok    binary     2brain 0.1.0+020d0de64469 — this build is what runs
ok    engine     au 0.0.5 at /home/aleksei/gate/bin/au
ok    target     workspace /home/aleksei/gate/vault
ok    socket     /home/aleksei/gate/home/.arsumbris/au-engine/run/5d905eea5e63531b.sock
ok    daemon     answering, wire schema 29, engine version 1
warn  workspace  1 member, 1 diagnostic — call `tb_diagnostics` through an MCP client for the file and the fix
ok    mcp        36 tools advertised with no mutation grant — the MCP server starts and answers tools/list
```

The one diagnostic is `peer-unmounted`: the copied `.2brain/repo.yaml` declares the `mcp-bundle`
member, which is not mounted on this host. That is the correct answer — the workspace really is
missing a peer here — and it is a warning, not a failure.

Read and write, through the engine wire on that host:

- `search "SoT"` → 15 documents, the same count as the live vault;
- `write_file gate-check.md` → `reflected: true`, `diagnostics: []`, hash `d2809dc3…`;
- `search "one write from a clean host"` → the file is in the page.

## Two findings

1. **`install.sh` is not the install path on a machine without Rust.** It runs
   `cargo +1.99.0 build --release`, so a host that has no toolchain cannot use it. What worked is
   copying the built binaries; release hygiene therefore needs either a published artifact or an
   install mode that accepts one. This pass did not add it.
2. **A fresh `HOME` still creates `~/.arsumbris/au-engine/`, by design.** The vault's identity was
   migrated to `.2brain` (`a8aba4b`), but writers still create the legacy spelling:
   `tb-parser::identity::CREATE_IDENTITY_DIR` is `LEGACY_IDENTITY_DIR`, stated so a previous-release
   binary reads what this one writes. Release 2 flips that one constant (tier C step 7); the gate
   confirms the constant is the only thing deciding it, and that no other code path invents the
   name. `doctor` printing the legacy socket path is therefore correct, not drift.

The test daemon was stopped; `~/gate` remains as the evidence (binaries, vault copy, the written
note).

## What this does and does not prove

It proves the two binaries are self-contained: no toolchain, no repository, no build cache, a
different absolute path, and a different user. It does not prove a packaged install (no tarball, no
checksum file, no service unit), and it does not exercise the adapter's mutation gate with a
policy file — the write above went through the engine wire, while the adapter answered
`tools/list` and reported 36 tools with no mutation grant.
