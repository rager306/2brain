---

> **Note 2026-11-16.** This ADR's plan was executed and then finished off: the TypeScript MCP layer
> (`au-mcp`, `au-mcp-core`, both adapters, the SDK's code, the codegen) and the Python bridge are
> deleted, and the served path is `/usr/local/bin/2brain`. The tree it describes no longer exists.
id: ADR-0001
title: Rust-native, agent-neutral MCP serving for Arsumbris
status: Proposed
lifecycle: "[proposed]"
date: 2026-11-12
supersedes: none
related: []
---

# ADR-0001: Rust-native, agent-neutral MCP serving for Arsumbris

## Status

**Proposed [proposed].** The desired direction is a Rust-native production
serving path, not an adapter dedicated to Hermes. The architecture, migration
and release gates below are proposals; none is evidence that an implementation
exists or is safe to deploy. Acceptance requires review by the project owner.
This cross-repository decision is recorded in `au-engine` provisionally because
it is the fork integration repository; ownership of the eventual MCP server
repository remains open. [proposed]

Lifecycle tags in this record are `[validated]` for reproducible measurements,
`[bounded]` for existing behavior within a stated scope, `[smoke]` for a limited
check, `[proposed]` for design, and `[deferred]` for decisions postponed.

## Context

Arsumbris serves a typed graph over files, including Markdown, YAML, wikilinks
and block IDs. The engine is a Rust workspace, while the current agent path
includes a Python stdio bridge, a TypeScript/Node MCP kernel, and TypeScript or
JavaScript plugin implementations. The engine wire contract is documented in
[`WIRE.md`](../../crates/tb-engine/WIRE.md). [bounded]

The production vault is shared with live agent sessions. An engine or writer
defect can affect real data. Tests must use isolated fixtures, and the live
daemon and bridge must not be restarted while sessions are active. [bounded]

The `au-mcp-core` source tree contains 49 tool definitions and six hook
definitions. This is an inventory of definitions, not evidence that every tool
is mounted or used in the target workspace. A full replacement must account
for session, profile, governance and plugin behavior, not just tool names.
[bounded]

Prior log and source investigation identified transport failures, invalid tool
arguments, dirty-file conflicts, and risks around bridge reconnect, session
handling and retries. The bridge/session mismatch and possible duplicate writes
on a lost response are source-level findings; production occurrence and runtime
version equivalence have not been established. [bounded]

The existing golden files cover three tool responses, not the complete MCP,
policy or mutation contract. They must not be presented as full parity proof.
See [`tests-golden/README.md`](../../tests-golden/README.md). [bounded]

## Decision

### Target and scope

Build an **agent-neutral MCP server in Rust**, backed by the Rust engine, with
no required Python or Node/TypeScript runtime in the production path from an
MCP client to the vault. Hermes is one supported client, not the architecture's
owner. Preserve the vault and type-system contracts unless a separate reviewed
migration and rollback plan explicitly changes them. [proposed]

Keep the engine as a separate process and its existing wire as the initial
boundary. The MCP server delegates mutations to the engine writer; it must not
implement a competing filesystem/Git writer. This is ownership of this server's
write path, not a claim that external editors or processes cannot modify vault
files. Any process consolidation requires its own evidence and decision.
[proposed]

Start with local **stdio** MCP. Add Streamable HTTP only when a concrete use
case, authentication and authorization design, client compatibility matrix,
and transport-specific security tests justify it. Localhost alone is not an
authentication boundary. Use a maintained MCP SDK after validating its exact
version, supported protocol revisions and lifecycle behavior against real
clients. [proposed]

Keep MCP protocol lifecycle separate from Arsumbris application identity,
authorization and read state. Support an explicitly tested set of MCP versions
and clients; neither an SDK claim nor a newer protocol version proves that a
particular client and application session interoperate. [proposed]

### Capabilities and source of truth

Inventory existing tool and hook definitions and actual mounted use before
choosing a first release. Classify each as port, redesign, defer or remove;
record observable schema and behavior differences. Expose a focused base
catalog and offer additional capabilities deliberately, rather than assuming
that all existing definitions must be ported or shown to every client.
[proposed]

The existing graph type definitions and metadata are contracts to assess, not
an automatic license to change them. In particular, current plugin runtime
metadata can name JavaScript entries. Decide how Rust capability registration
relates to these definitions and any external plugins before changing their
format; avoid two unsynchronized authorities for schemas and policy. Any format
change needs its own versioning, migration and rollback reasoning. [proposed]

Implement required first-party tools and policy in Rust. Do not require a JS
plugin host as a permanent dependency. Dynamic native ABI, WASM or subprocess
extension mechanisms are deferred until an actual extension requirement and a
security/compatibility contract justify one. [proposed] [deferred]

### Safety invariants

Enforce permissions at invocation, not only in discovery. An unknown or stale
application identity must not silently acquire session-less privileges. If a
required safety policy is unavailable, refuse the affected mutation rather
than silently bypassing it. Define the scope of any read-only degradation
explicitly. [proposed]

Preserve or intentionally revise the relevant existing policy semantics with
tests: profile visibility, mediation ordering, read-view versus served-view,
freshness, continuity and trusted stamps. In the existing TypeScript kernel,
`mediate` is a separate request from `invoke`; a caller must not assume that
`invoke` alone runs mediators. Source behavior must be characterized on an
isolated fixture before claiming parity or correcting a suspected gap.
[bounded] [proposed]

After a transport failure or cancellation following mutation submission, report
an **unknown outcome** when the outcome cannot be established. Do not blindly
retry. An MCP request ID or an adapter-only journal does not provide exactly-once
execution; stronger idempotency requires a separately designed and tested
protocol involving the authoritative writer and its crash recovery. [proposed]

Retain the engine's distinct preconditions: `write_file` has an optional
`expected_hash`, while `edit_file` uses an exact-string precondition. A policy
requiring a stronger guard must provide and verify it at the appropriate layer;
it must not claim that all mutations have equivalent CAS. `reflected: false`
means the mutation landed but the held view has not caught up; do not retry it.
Multi-repo mutation is a saga, not an ACID transaction. [bounded] [proposed]

Return actionable, structured errors and bounded results with explicit scope,
truncation and continuation where applicable. Treat note contents as untrusted
data, not executable instructions. A successful structural diagnostic is not a
claim that the knowledge itself is true or semantically complete. [proposed]

## Alternatives considered

1. **Repair the existing Python/TypeScript stack only.** This can mitigate
   immediate defects but does not meet the Rust-native production objective.
   It may remain a temporary baseline. [deferred]
2. **Replace only the Python bridge with Rust.** Lower migration risk, but it
   leaves Node and the existing plugin runtime in the production path. It may
   be used as a disposable migration step if evidence requires it, not as the
   target. [deferred]
3. **Rewrite MCP and engine as one new monolith immediately.** It removes a
   process boundary but expands the blast radius, duplicates proven engine
   behavior and risks vault compatibility. No evidence currently justifies it.
   [deferred]
4. **Direct read-only MCP over the existing engine, then migrate governance
   and writes.** Chosen direction for the initial vertical slice; read-only does
   not imply policy parity or production readiness. [proposed]

## Migration and release gates

1. **Inventory.** Record actual clients, deployed versions, mounted tools,
   plugins and policies. Preserve existing local changes; do not infer runtime
   behavior from source versions alone. Restore the broken local pinned Rust
   toolchain before any build. [proposed]
2. **Contract harness.** Capture sanitized representative schemas, responses,
   errors, policy decisions and effects. Create two isolated copies of one
   consistent fixture; never run tests against the live vault. Existing golden
   fixtures are a baseline, not the whole contract. [proposed]
3. **Read-only vertical slice.** Implement MCP stdio over a fixture engine for
   overview, targeted reads and navigation. Check at least two different real
   clients, protocol versions, schemas, bounded results and failure behavior.
   [proposed]
4. **Identity and policy.** Characterize and test actual mediation, session,
   profile and freshness semantics. Agree on authorization when a client does
   not provide a reliable application identity. Keep writes disabled until
   required safeguards are demonstrated. [proposed]
5. **Mutations and extensions.** Port prioritized operations and first-party
   capabilities. Test stale reads, external edits, dirty paths, concurrent
   sessions, interrupted writes, lost responses, saga compensation, ignored
   files and off-git cases where applicable. Compare independent fixture copies;
   never mirror writes to production. [proposed]
6. **Promotion.** Run applicable MCP conformance tests and real client tests,
   then engine golden wire checks and the expanded behavioral suite. Review all
   intentional differences. Plan a maintenance window with no live sessions,
   fence or quiesce writes, rehearse rollback, and only then switch clients.
   Preserve or reconcile writes accepted after cutover during rollback. [proposed]

The golden wire contract remains mandatory before any live engine swap; this
ADR neither waives it nor authorizes a restart. MCP replacement also requires
the broader behavioral gates above. [bounded] [proposed]

## Consequences

**Positive:** one implementation language in the serving path, a client-neutral
MCP surface, and an opportunity to make failure and authorization behavior
explicit. [proposed]

**Costs and risks:** behavior must be reconstructed and proved across multiple
repositories; existing external JS/TS plugins may not work without migration;
the new server can regress security even if read-only JSON matches; broader
client support increases testing cost; moving boundaries does not itself fix the
daemon, watchdog or client behavior. [proposed]

The objective covers the production serving path. Whether auxiliary converters,
client adapters, test infrastructure and build tools must also be Rust-only is a
separate scope decision; MCP conformance tools may themselves require other
runtimes in development or CI. [proposed]

## Open questions

- Repository ownership and release topology: a new crate in this workspace or
  a sibling Rust repository? Decide after a read-only spike and dependency review.
  [proposed]
- Which exact tools, hooks, clients and MCP protocol revisions are mandatory
  for the first supported release? [proposed]
- How is application identity established and read-before-write enforced for
  clients that cannot provide a stable authenticated session? [proposed]
- Which existing plugin metadata is a compatibility contract, and what migration
  path would let Rust implementations replace current JS entries? [proposed]
- What exact engine and writer changes, if any, are required for recoverable
  mutation IDs or durable deduplication? Do not claim exactly-once before proof.
  [proposed]
- Which operational metrics, compatibility differences and rollback criteria
  are required before production promotion? [proposed]

## References

- Engine wire: [`crates/tb-engine/WIRE.md`](../../crates/tb-engine/WIRE.md).
- Existing golden coverage: [`tests-golden/README.md`](../../tests-golden/README.md).
- Local sibling source for current behavior: `../au-mcp/src/daemon/daemon.ts`,
  `../au-mcp/src/daemon/session.ts`, `../au-mcp-core/`, `../au-mcp-bridge/`.
  These are separate repositories and versions must be pinned during migration.
- [MCP tools, protocol revision 2026-07-28](https://modelcontextprotocol.io/specification/2026-07-28/server/tools).
- [MCP transports, protocol revision 2026-07-28](https://modelcontextprotocol.io/specification/2026-07-28/basic/transports).
- [Official MCP Rust SDK](https://github.com/modelcontextprotocol/rust-sdk).
- [MCP conformance test framework](https://github.com/modelcontextprotocol/conformance).
