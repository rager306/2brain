# Implementation plan: Rust-native, agent-neutral MCP serving

This plan accompanies [ADR-0001](0001-rust-native-agent-mcp.md). The ADR is
proposed; this document authorizes no live deployment or vault migration.

## Scope and milestones

1. **Inventory and safety baseline.** Classify existing tool and hook definitions
   and distinguish source presence from actual mounted use. Record behavior,
   policy, client dependencies, and intentional compatibility differences.
   Preserve existing uncommitted work. The default rustup home has an incomplete
   1.99.0 toolchain; this host also has a working 1.99.0 under
   `RUSTUP_HOME=/root/.rustup`. Use an explicitly selected, complete toolchain
   and record the environment used for verification.
2. **Read-only vertical slice.** Build an isolated Rust MCP stdio prototype in
   the sibling `au-mcp-rs/` directory, without a production writer or an automatic
   connection to the live vault. Its initial API is provisional. Connect to an
   explicitly selected engine socket and implement only proven engine reads.
   Repository ownership and long-term workspace layout remain open decisions.
3. **Protocol and fixture tests.** Exercise real MCP framing and a fake engine,
   including partial/oversized frames, protocol errors, incompatible engine wire,
   empty or failed responses, no exposed mutations, and clean stdout. Compare
   selected read behavior against a real engine on a disposable fixture; test
   each claimed MCP revision and client rather than inferring compatibility.
4. **Policies and mutations.** Specify identity, policy ordering, read-before-write,
   fresh state, dirty-path handling, mutation outcome uncertainty and recovery
   before implementing writes. Verify failed or absent critical policy cannot
   silently admit a write. Do not blindly replay a mutation after lost response.

   **Implemented so far.** Mutations are off unless granted at launch
   (`--allow-mutations`, or a policy file with `allow_mutations`); the mutating
   tools are then absent from `tools/list` and refused before any engine
   connection. A policy file can restrict mutations to an exact member allowlist
   and routes one audit line per decision to stderr or a file; a malformed
   policy aborts startup. Read-before-write, the `expected_hash` staleness check,
   the editable-member check, single-mutation serialization, `outcome_unknown`
   without retry, and a non-mutating `preview_mutation` are in place. Identity is
   a label only: a stdio client controls its own process, so `--principal` and
   the client's `initialize` name are not authentication.
5. **Cutover decision.** Complete the expanded golden and differential contracts,
   fault tests, client compatibility matrix, and a rollback rehearsal. Quiesce or
   fence writers during a separately approved maintenance window. No dual writes
   to production and no daemon restart while live sessions are active.

## Immediate acceptance checks

- The MCP process exposes a fixed allowlist of audited engine operations only;
  mutation requests and arbitrary engine verbs are never forwarded, including
  for unadvertised MCP names. In particular, engine `read: shutdown` stops the
  daemon; the `read` field and broker permissions alone do not establish safety.
  Some existing host tools can change UI even with a read-only broker.
- Production sockets and `/root/vault` are not used by tests. Fake socket paths
  belong to test-created temporary directories; any real engine serves an
  independently created disposable fixture.
- The server validates engine frame length, JSON response type and declared wire
  version, and returns an actionable error rather than fabricated success.
- Captured output contains only MCP messages on stdout; diagnostics use stderr.
- Client compatibility, current engine golden coverage, and toolchain version
  are reported accurately; a passing mock is not a production release gate.
- Stdio serving must be tested with a client that stops reading stdout while
  continuing to send requests, and on stdin EOF with stdout blocked. The current
  prototype uses a bounded standard-IO transport: at most four queued responses,
  the reader stops while the queue is full, a five-second stall closes the
  connection, and shutdown abandons queued output after two seconds. Both cases
  are covered by tests. This bounds one adapter process; it is not a rate limit
  and does not protect against many adapter processes or a hostile client that
  keeps the connection busy within those bounds.

## Open decisions

- Exact tool surface and compatibility names, grounded in existing consumers.
- Explicit application identity and authorization across heterogeneous MCP
  clients, particularly where no durable authenticated session is available.
- Capability registration and migration of existing JS plugin metadata without
  creating an unsynchronized second source of truth.
- Long-term Rust repository and workspace topology, release/version policy, and
  extent of non-Rust development and CI tooling.
