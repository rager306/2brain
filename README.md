# 2brain

**2brain** is a Rust knowledge engine over a typed graph of notes, the MCP surface agents talk to, and a
judgement layer for the small decisions. One repository, one serving binary, one language on the request
path.

```sh
/usr/local/bin/2brain mcp /root/vault --allow-mutations     # the MCP server a client spawns
/usr/local/bin/2brain info /root/vault                      # surface + a probe of the live engine
/root/.arsumbris/bin/au daemon status /root/vault           # the engine daemon
```

## Lineage

2brain is a fork of the **2brain** project — a typed knowledge-graph engine over markdown workspaces
with `[[wiki-links]]`, `^block-ids` and frontmatter type contracts: <https://github.com/arsumbris>.
That project stopped receiving changes after `0.0.2-alpha` (last commit 2026-09-25); its `0.0.5`-era
work is preserved here as the tag `archive/upstream-0.0.5` (commit `44b32f4`). Everything since —
the Rust MCP serving path, the BM25 index, the fail-closed write gate, the retirement of the
TypeScript and Python layers — is 2brain's own.

## Layout

| path | role |
|---|---|
| `tb-engine/` | the engine: parser, type graph, diagnostics, daemon, wire, the BM25 index (crates `tb-cli`, `tb-engine`, `tb-core`, `tb-parser`, `tb-grammar`, `tb-references`, `tb-diagnostics`, `tb-testkit`) |
| `tb-mcp-rs/` | **the serving path**: the MCP adapter behind `/usr/local/bin/2brain` (`scripts/install.sh` builds and installs it) |
| `tb-mcp-sdk/` | the `mcp.*` type vocabulary that content notes are written in (`mcp.skill`, `mcp.inject`, …) |
| `tb-defaults/` | workspace bundles: `minimal`, `blank`, `studio`, `host-bundle`, `mcp-bundle` |
| `tb-type-system/`, `tb-base-types/` | the type-system contract and the base ontology |
| `tb-ingest/`, `tb-govern/` | ingest contracts, workspace composition |
| `tb-rules/`, `tb-skills/`, `tb-agent-guides/`, `tb-writing-style/`, `tb-competency/` | content: rules, skills, guides, style, competency |
| `tb-weave/`, `tb-tree-research/` | domain knowledge repositories |
| `upstream/` | the upstream project's own documentation, kept verbatim as an archive |
| `docs/` | the trajectory: architecture, verification index, swap runbooks, rebrand plan |
| `tools/` | tooling that never runs on the request path |

The engine's own file format still uses the historical `au.engine.*` schema names and `.arsumbris/`
identity directories; that is a format, not a brand, and moving it is a migration with its own gate
(`docs/rebrand-plan.md`, tier C).

## Gates

```sh
cd tb-engine && RUSTUP_HOME=/root/.rustup cargo +1.99.0 test --offline --lib
cd tb-mcp-rs && RUSTUP_HOME=/root/.rustup cargo +1.99.0 test --offline
```

`docs/verification.md` is the evidence index: every property this repository claims, with the command
that re-measures it and the date it was last measured.
