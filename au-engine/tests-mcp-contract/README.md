# Read-only MCP contract test plan

This directory is reserved for an isolated end-to-end contract suite for the
new read-only MCP vertical. The executable interface, tool catalog, schemas,
and result mapping are not yet agreed. Do not infer them from this plan or
freeze the behavior of an older bridge as the new API. Replace the pending
items below with explicit decisions before implementing assertions for them.

## Decisions required before implementation

- How to launch the MCP process and inject the engine endpoint, environment,
  and fixture location; how startup failures are reported.
- Supported MCP protocol versions, including which 2025 and 2026 revisions are
  supported if any, and the behavior for an unsupported version. There is no
  assumption here that a particular 2026 revision exists or is supported.
- The exact read-only tool names, input schemas, engine reads, output mapping,
  and error mapping, including unavailable/not-ready and not-found cases.
- The agreed compatibility boundary with the existing bridge and golden
  captures, including any intentional output changes.
- How the test fixture is created and how the engine reaches a ready state.

## Isolation and safety

- Never use `/root/vault`, its socket, its daemon, or its bridge in tests.
  Do not copy data from the live vault into test fixtures. Do not restart,
  stop, or reconfigure production processes.
- Each test owns a fresh temporary directory, `HOME`, engine socket and child
  processes. Pass paths and environment explicitly; reject configuration that
  resolves to a production path or socket. Run tests in parallel to expose
  accidental sharing. Teardown must stop and reap only processes started by
  that test and remove only its own temporary files.
- Use a synthetic repo with a valid manifest, a minimal note type, two notes
  and a link between them. Add controlled missing-target and invalid-input
  cases where needed. Pin fixture content in this directory or generate it
  deterministically; never use live data as expected results.
- Where a test needs to change fixture files, use a separate fixture writer
  against the test-owned repo. The writer must not use the MCP read-only
  endpoint, the production daemon, or another test's directory. Wait for an
  explicit ready/change observation before checking post-write results; do not
  rely on fixed sleeps. Snapshot fixture files before and after read-only calls
  and assert the calls themselves did not change them. Exclude test-owned
  engine runtime artifacts only by an explicit, documented boundary.

## Test layers and acceptance criteria

### MCP stdio and lifecycle

- Launch the actual MCP process, send JSON-RPC messages over stdin and parse
  responses from stdout. Stdout contains protocol messages only; diagnostics
  go to stderr. Requests receive exactly one response with the matching ID;
  notifications receive none. Exercise multiple requests in one session.
- For every supported protocol revision, test its specified lifecycle,
  version and capability negotiation, tool discovery, and a read call. Test
  legacy initialization and initialized notifications only for revisions that
  define them. Test the agreed response to unsupported versions, malformed
  JSON-RPC messages, missing parameters, unknown methods, and invalid lifecycle
  sequences. Pin assertions to the corresponding published MCP specification.
- Check clean shutdown on stdin EOF and prompt cleanup of the test-owned
  process. A transport failure must not contaminate stdout or hang the runner.

### Fake-engine boundary

- Start a test-owned fake engine at the injected endpoint. Implement only the
  required engine protocol, record each request, and return controlled frames.
  Use it to distinguish the MCP contract from the daemon wire. The daemon wire
  is documented in `../crates/au-engine/WIRE.md`: length-prefixed JSON over a
  Unix socket, with tagged responses and a `schema_version`.
- For each agreed tool, verify the exact permitted read request and arguments,
  correlation, and translation of ready results. Exercise null payloads,
  not-ready responses, engine errors, disconnects and malformed replies. Assert
  the agreed MCP error representation and that no engine response is reported
  as a successful read when it is an error.
- Record all outbound requests and assert that every request matches an
  explicit allowlist of audited engine operations. Never forward engine `mutate`,
  `subscribe`, or arbitrary `read` verbs: notably `read: shutdown` stops the
  daemon. Test guessed mutation names, shutdown, unknown tools, invalid
  arguments and disallowed fields directly, not only their absence from
  discovery. Verify no forbidden request reaches the fake engine.
- Verify that the advertised tool allowlist and input schemas match the agreed
  contract, and that each advertised tool has at least one successful call.
  Test filtering, pagination and missing-resource behavior for the tools that
  actually expose those features; assert exact semantics rather than accepting
  any non-error result.

### Fixture parity and regression

- Run the actual MCP process and a test-owned engine over the same synthetic
  repo. After explicit readiness, compare each agreed tool's result to an
  independent direct daemon read of the same fixture, applying only the
  documented MCP mapping. Include links, missing targets, empty results and
  error cases. Verify outputs after a fixture-writer change as well as before
  it, using the agreed freshness contract.
- Keep deterministic expected outputs for the synthetic fixture where useful.
  Normalize only documented nondeterministic values such as temporary root
  paths; never normalize away semantic fields or errors. Compare paths and
  ordering according to the agreed contract, not an invented guarantee.
- `../tests-golden/` contains existing production-captured bridge results, not
  daemon frames or MCP JSON-RPC envelopes. Its values and absolute paths are
  not synthetic fixture expectations. If bridge compatibility is agreed,
  test the applicable output structure separately and document intentional
  differences; do not contact the live vault or silently rewrite those files.

### Security and read-only enforcement

- Beyond discovery, verify that guessed write, delete, mutation and subscription
  calls fail without reaching the engine. Check malformed input and path
  traversal or out-of-scope paths against the agreed access policy; verify no
  fixture-external file is read or changed when the policy forbids it.
- Test that an inaccessible or missing engine endpoint fails explicitly and
  cannot cause fallback to the user's production socket or home directory.
  Inspect recorded fake-engine requests and fixture snapshots to establish the
  boundary, rather than relying solely on reported success or failure.

## Completion gate

The suite must run from a documented command without changes to existing Cargo
manifests. It must pass against the agreed implementation, fail when a tested
read mapping or read-only boundary is deliberately broken, and leave no
test-owned processes or files behind. Record the supported protocol revisions,
decisions above and any deliberate compatibility changes alongside the suite.
