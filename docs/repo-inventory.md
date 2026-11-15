# Repository inventory: what each directory is, and what happens to it

Measured 2026-11-16 from `git ls-files` (tracked files only; `node_modules`, `target`, `.pyc` are not
counted). This is the map the consolidation works from: every directory has one **layer**
(`docs/detach-and-consolidate.md` §2, `AGENTS.md`) and one **fate**.

## Layers, restated in one line each

- **Code** — runs in the serving path; Rust only, by the rule adopted 2026-11-16.
- **Contract** — type definitions and frontmatter rules; not code, and changing one is a migration.
- **Tooling** — drives the product, never driven by it; any language.
- **Model** — produced numbers for a decision; never cited as shipped behaviour.
- **Content** — knowledge that the engine serves; not a program.
- **Docs** — prose about the above.

## The table

| directory | files | language | layer | serving today | fate |
|---|---:|---|---|---|---|
| `au-engine` | 1 869 | Rust (180 src files, 157 k LOC) + yaml fixtures | code | **yes** — `arsumbris-engine.service` | keep; primary target for every architectural feature |
| `au-mcp-rs` | 140 | Rust (51 src/test files, 20 k LOC) + 11 shell + 10 py in `scripts/` | code + tooling | **yes** — `arsumbris-rs` MCP server | keep; its `scripts/` stays tooling |
| ~~`au-mcp`~~ | 0 | — | — | **no** | **deleted 2026-11-16** with its unit; the Node daemon had no consumer left |
| ~~`au-mcp-core`~~ | 0 | — | — | no | **deleted 2026-11-16**: 55 `mcp.tool.*`/`mcp.hook.*` definitions for a tool surface that no longer exists |
| ~~`au-mcp-adapter-codex`~~ | 0 | — | — | no | **deleted 2026-11-16**: no consumer |
| ~~`au-mcp-adapter-cc`~~ | 0 | — | — | no | **deleted 2026-11-16**: no consumer |
| `au-engine-sdk` | 45 | TypeScript (36, 14 k LOC) | client library | no | keep only if an external JS consumer exists; otherwise retire |
| `au-mcp-sdk` | 20 | 17 yaml type defs + docs | **contract** | no | **kept as vocabulary**; its TypeScript (`src/`, `tests/`, `skills/`, package, lockfile) was deleted 2026-11-16 — `au-weave` declares `type: mcp.skill::au-mcp-sdk` in six files and the vault's `mcp-bundle` resolves through it |
| `au-type-codegen` | 36 | TypeScript (30, 3.5 k LOC) | tooling | no | keep as tooling; a Rust port is possible but not scheduled |
| ~~`au-mcp-bridge`~~ | 0 | — | — | **no** | **retired 2026-11-16** (Y1): `bridge.py`, its `.bak` and the directory are gone; `~/.arsumbris/au-mcp/` deleted |
| `au-ingest` | 38 | yaml types + 1 py fixture generator | contract + tooling | no | keep the contract; the generator is tooling |
| `au-govern` | 19 | yaml types + markdown | contract | no | keep |
| `au-type-system` | 68 | yaml + markdown | contract | no | keep; **not** a migration-free zone |
| `au-base-types` | 12 | yaml types | contract | no | keep |
| `au-agent-guides`, `au-rules`, `au-writing-style`, `au-competency`, `au-defaults`, `au-skills`, `au-weave`, `au-tree-research` | 203 | markdown + yaml | content / contract | no | keep |
| `arsumbris` | 11 | markdown (upstream docs project: README, INSTALL, au-host.md, banner) | docs (upstream) | no | keep as archived upstream documentation; it is the only surviving description of the `au-host` launcher |
| `tools` | 4 | Python (2) + md | tooling | no | keep |
| `docs` | 13 | markdown | docs | no | keep |

## What the inventory makes obvious

1. **The serving-path zoo is two artifacts**, not seven: `au-mcp` (Node) and `au-mcp-bridge/bridge.py`
   (Python). Everything else non-Rust is a contract, a client library, a generator, tooling, or docs.
2. **The Python surface is 18 tracked files**, of which **one** was on the serving path — and that one
   is gone (2026-11-16). The "39 py files" in a raw count included `.pyc` and untracked leftovers;
   `bridge.py.v1.bak` went with the bridge.
3. **`au-mcp-sdk` is the contract that survived the deletion**: its 17 type definitions are the
   language that `au-weave` and `au-rules` notes are written in. Deleting the code removed an
   implementation; deleting the types would have invalidated content.
4. **Two TS packages are contracts, not code**: `au-mcp-core` (57 type definitions) and `au-mcp-sdk`
   (19). Deleting them with the daemon would delete the language the ontology work is written in. The
   split inside each package — `type/` versus runtime — is what the retirement must respect.
5. **`au-engine-sdk` is the only substantial client library** (14 k LOC). Its fate is a product
   question: do we support a JS client, or is the Rust adapter the only supported surface?
6. **`arsumbris/` is upstream's documentation project**, imported whole. It describes the `au-host`
   launcher, which nothing in this repository implements — keep it as an archival reference, and do not
   mistake it for a component.
## Follow-ups this inventory creates

| item | why | gate |
|---|---|---|
| delete `au-engine-sdk` and `au-type-codegen`, or give them a consumer | the last TypeScript in the tree, with no JS consumer left after the MCP deletion | a consumer, or a deletion in one commit |
| check whether `au-host` exists anywhere | measured: `arsumbris/README.md:51` documents it and the deleted SDK named it, so it is a *referenced* launcher with no implementation here | a `grep` says "referenced, not implemented"; anyone relying on it must be told |
