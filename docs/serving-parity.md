# Serving parity: what the Node daemon and the bridge give, and what the Rust adapter does not

Measured 2026-11-16 on the live socket and in the source. This is the gate for `docs/detach-and-consolidate.md` Y1/Y2:
it lists what the retirement would remove, and it leads with the check that fails today.

## 0. The first check fails, and it is not about parity

Hermes launches `/root/arsumbris/au-mcp-rs/target/release/au-mcp-rs`
(`~/.hermes/config.yaml:588`). **That file does not exist.** The path is right — `au-mcp-rs/target` is
a symlink to the shared `.cargo-target` (`/root/arsumbris/.cargo/config.toml`), and both names resolve
to the same missing release binary. Both live adapter processes run from a **deleted inode**
(`/proc/1194089/exe -> /root/arsumbris/.cargo-target/release/au-mcp-rs (deleted)`), so they work only
until the next connection: a new Hermes session cannot start `arsumbris-rs` at all.

One fault, and it is not a config fault: the release binary has to exist, and be kept. A debug build
survived in `au-mcp-rs/target/debug/`, so this was a release build that was never made here — the
adapter ran from a binary later deleted by a cleanup.

This is the reason the retirement checklist is ordered the way it is: an unlaunchable Rust server is
not a parity question, it is an outage waiting for the next session.

## 1. The Node daemon is not an MCP server

It serves its own wire (`au-mcp/src/wire/server.ts`, 18 request kinds in `daemon/daemon.ts:618-1041`);
the MCP surface is produced by `au-mcp-bridge/bridge.py`, a translator (MCP JSON-RPC over stdio ↔
4-byte-length-framed daemon wire, `bridge.py:82-97`) that also renames tool ids
(`mcp.write_file` → `mcp__write_file`, `:46-50`, reverse map `:178-187`), pins one daemon session
(`:148-163`) and reconnects once (`:99-109`).

Live surface on the running socket: **49 tools, 0 resources, 0 prompts**. Definitions:
`au-mcp-core/type/mcp.tool.*.type.yaml` (49).

| group | count | what it is |
|---|---:|---|
| engine reads | 30 | one engine read each; `au_type_system` is a two-read composition (`engine-reads.ts:280`) |
| file + mutation | 13 | writes go through the engine mutation channel, never the filesystem (`file-tools.ts:184-570`) |
| host relay | 4 | relay to an `au-host` socket; **no such socket exists on this host**, so an agent gets a connect error |
| stubs / fs | 3 | `bash` never executes — it returns guidance (`file-tools.ts:76-86`); `glob`/`grep_files` are `node:fs` + ripgrep over the workspace |

Hooks: 6 (`mcp.hook` plugins: `read-guard`, `tool-precondition`, `nativeToolRedirect`, `repo-overview`,
`requiredScope-check`, `instance-count-notice`). **None of them runs under the bridge** — it never sends
`mediate`/`observe`, so floors never fire and session-start injects are computed then discarded
(`daemon.ts:701-722`; `bridge.py:241-256` has no `session-start-context`).

Resource URI scheme: **none**. The Rust adapter has `arsumbris://<member>/<path>`
(`au-mcp-rs/src/resources.rs`), which is a gap in the other direction.

## 2. Gaps in the Rust adapter, by class

| class | gaps | would an agent notice? |
|---|---|---|
| read | the 4 host relays (dead: no socket), `~glob`, `~grep_files`, `au_type_system` | reads: `au_grep` is literal text over *catalogued* files, so regex/mtime ordering is lost — noticeable only for files outside the vault; the composition is rebuildable from `au_instances_of` + `au_content` |
| hook | all 6 | no: they are invisible on the bridge path too, so Hermes never had them. The Rust write gate is **stricter** (`policy.rs` PriorRead/Freshness) while the Node path via the bridge has no read-guard at all |
| subscription | one held `changes` subscription maintaining per-session read-views (`broker.ts:160-180`, `freshness.ts:41-60`) | no: unusable by Hermes (no read-view on the bridge path); `au_watch` is a ≤30 s long-poll and not equivalent, and not needed |
| lifecycle | sessions, dormancy, retention, mediation, trace, turn-end, bindings, profile allowlists, gate-side input validation, stamp injection, served-files contract, crash recovery, skill/inject materialization (`~/.arsumbris/au-mcp/gen/`, 3 skills present), macOS approval dialog (inert on Linux), dynamic TS plugin loading (`discovery.ts`) | no: invisible to Hermes. Dynamic plugin loading is the genuinely unportable piece, and it is measured unused (`docs/phase-d.md`: the vault declares no plugins) |
| stub | `bash` | no: it never executed |

So the parity gap is **smaller than the surface count suggests**: 8 of 49 tools have no counterpart,
of which 4 are dead relays and 1 is an inert stub.

## 3. The ordered retirement gate

0. **Build the adapter release binary and point the config at it** — fails today, §0.
1. **Verify the mutation reply schema on the rebuilt binary.** The 2026-11-15 live run had every
   successful mutation rejected client-side (`commits` object vs array;
   `/root/hermes-runs/2026-11-15/transcript.txt`). The source now declares an object
   (`schemas.rs:382-395`); a fixture mutation turn against the rebuilt binary is the proof.
2. **Decide the 8 Node-only tools** and record the decision: 4 dead relays, 1 stub, 2 filesystem reads,
   1 composition.
3. **Grants.** The config passes only `--allow-mutations`; `au_add_dep`/`au_remove_dep` need
   `--allow-composition` and the three `*_workspace_member*` tools need `--allow-workspace-shape`
   (`policy.rs`). Node exposed all five ungated, so this must be decided *before* the bridge goes.
4. **Update Hermes text that pins Node tool names**: `~/.hermes/skills/knowledge-graphs/arsumbris-vault/SKILL.md`,
   `.../scripts/bridge_probe.py`, `~/.hermes/skills/research/jev/SKILL.md:94`,
   `~/.hermes/skills/arsumbris/SKILL.md`. Signature deltas to state there:
   `au_validate{typeName,value}` → `au_validate_value{type_name,value}`; absolute `file_path` →
   `{member, path}`; `au_grep` literal, not regex.
5. **Remove `mcp_servers.arsumbris`** (`config.yaml:576-583`); confirm a session sees only
   `mcp__arsumbris_rs__*` and completes one read and one write turn.
6. **Only now** stop `arsumbris-mcp.service`, delete `au-mcp-bridge/bridge.py` and `~/.arsumbris/au-mcp/`.
7. **Then** the code retirement (`au-mcp`, `au-mcp-core` runtime, `au-mcp-sdk` runtime, `au-type-codegen`,
   both adapters, `au-engine-sdk`) — subject to `docs/repo-inventory.md`'s contract/code split.

Rollback for steps 5–6 is a config edit plus `systemctl start`; nothing is deleted that git cannot
restore, and the vault is not touched.

## 4. What the retirement removes

~4.1 MiB of TypeScript, 8 lockfiles, and the only runtime consumer of `au-type-codegen`
(`discovery.ts:19`). No third-party `node_modules` anywhere: the repo-root `node_modules/` is 8 KiB of
`@arsumbris/*` symlinks, and the daemon runs on Node builtins plus workspace links.

## 5. Not determined

Full write-path parity for the 16 mutations on a fixture vault — that is step 1's job, and it is the
one place where "the source declares it" is not evidence.
