# au-mcp-rs

Experimental MCP stdio server for an existing arsumbris engine daemon. Reads are
always available; mutations are off unless granted at launch (see below). It
does not start a daemon or derive a default socket. Supply an explicit Unix
socket path as the sole command-line argument:

```sh
cargo run -- /path/to/engine.sock
```

### Launch policy

Mutations are **off unless granted at launch**. Without a grant the server is
read-only: `au_write_file`, `au_edit_file`, and `au_delete_file` are absent from
`tools/list`, and calling one is refused before any engine connection.

```sh
au-mcp-rs /path/to/engine.sock                                  # read-only
au-mcp-rs /path/to/engine.sock --allow-mutations                # explicit grant
au-mcp-rs /path/to/engine.sock --policy /etc/au-policy.json \
          --principal "hermes"                                  # scoped grant + audit
```

A policy file is JSON with `deny_unknown_fields`; a malformed or unreadable file
aborts startup instead of falling back to a permissive default:

```json
{
  "allow_mutations": true,
  "members": ["notes", "journal"],
  "audit": "stderr"
}
```

- `allow_mutations` (default `false`) — whether the mutating tools exist at all.
- `members` (default: every editable member) — exact allowlist of members a
  mutation may target. Anything else is refused before the engine is contacted.
- `audit` — `"stderr"` (default), `{"file": "/var/log/au-mcp.jsonl"}`, or
  `"disabled"`. `"disabled"` is rejected while mutations are allowed.

Every mutation decision is audited as one JSON line with `principal`, `tool`,
`member`, `outcome` (`applied`, `rejected`, `outcome_unknown`, `denied`,
`denied:member`) and a timestamp.

The mutation gate is an ordered chain; a refusal names its stage both in the
tool error (`policy/<stage>: ...`) and in the audit (`denied:<stage>`):

| stage | refuses when |
|---|---|
| `arguments` | `repo`, `path`, or `expected_hash` is empty |
| `path_scope` | the path is absolute, contains `..` or NUL |
| `member_grant` | the member is not in the policy allowlist |
| `prior_read` | no `au_content` read of this exact path is recorded |
| `freshness` | the recorded hash differs from `expected_hash` |
| `capacity` | another mutation holds the single slot |
| `editability` | the engine reports the member as consumed |

Earlier stages win, so a refusal is deterministic and cheaper: a path escape is
reported without needing a prior read.

**Identity is not authenticated.** `--principal` and the client's `initialize`
name only label audit records; a stdio client controls its own process and can
claim any name. The real boundary remains the OS user that owns the engine
socket.

The selected daemon and its socket are a trust boundary: the engine wire has no
authentication. The server has no per-client authorization; the launcher must
restrict who can use it. A single server process permits at most eight concurrent
engine reads. Further reads fail immediately with a tool error and may be retried
later. The slot is released on completion, failure, timeout, or cancellation;
there is no waiting queue. This limits concurrent adapter reads, not the total
request rate or work across processes. The engine may continue work after an
adapter timeout or cancellation, so it is not a strict limit on engine activity.
The stdio transport is bounded. At most four responses may be queued for
writing; while that queue is full the server stops reading requests, so the
client's own pipe buffer provides backpressure instead of unbounded growth. A
client that does not drain stdout for five seconds while the queue stays full
is treated as gone: the server closes the connection instead of buffering
more. Shutdown waits at most two seconds for queued output, then abandons it and
exits; a client must not close stdin while it still needs queued responses.
These are bounds on one adapter process, not a request rate limit. The server is
still not suitable for untrusted clients without process and resource isolation.
For an in-flight read, an MCP
cancellation notification stops the adapter's wait and closes its engine socket.
Cancellation is best effort: the engine may continue processing after the socket
closes, and a response may already have been sent when cancellation arrives.
This behavior does not imply rollback or apply to future mutations. The server
checks wire schema version 29, frame size, response
type, correlation id, readiness, and the result envelope. A read has a 30-second
deadline for connect, request, and response; a timeout returns a tool error
without retrying. It accepts no generic wire requests and exposes only
`au_overview`, `au_top_level_dirs`, `au_type`, `au_type_tree`, `au_types`,
`au_hubs`, `au_diagnostics`, `au_subtypes`, `au_members`, `au_imports`,
`au_files`, `au_content`, `au_instances_of`, `au_validate_value`,
`au_preview_mutation`, and `au_reconcile`, all annotated read-only. `au_write_file`,
`au_edit_file`, and `au_delete_file` are the mutating tools; they are annotated
destructive and not read-only. Only these tools and `au_content` send a
client-supplied path to the engine, and
only after rejecting absolute paths and `..` and joining it to a member's
declared root. All tools except `au_type`, `au_subtypes`, `au_content`,
`au_write_file`, `au_edit_file`, and `au_delete_file` accept an empty, null, or
omitted argument object.
`au_type` requires a nonempty `name` and optionally accepts a nonempty `repo`;
a supplied empty `repo` is rejected instead of widening the lookup to the
workspace, even if `name` is qualified. A nonempty `repo` does not override an
already-qualified `name`. A missing type or member returns `null`. This differs
from the old bridge, which ignores an empty `repo`. All tools reject unknown fields. Tool
responses contain the entire engine result envelope as JSON text; errors are
MCP tool errors. The server checks only the top-level payload type, not every
field in the engine response. `au_overview` can expose the full dependency
member topology, including absolute paths, rather than only own-scoped data.
`au_type_tree` includes mounted dependencies and built-in types. Its adjacency
is matched by name, not a complete qualified identity. It is unpaged: a response
exceeding the 16 MiB wire limit fails instead of being truncated. These tool
names do not promise compatibility with the old bridge.

`au_types` returns the engine's `{ "types": [...] | null }` envelope from one
read, not the old bridge's count and paging envelope. It defaults to summary
entries, `limit: 50`, and `offset: 0`; limits outside 1–500 are rejected.
`summary: false` returns full definitions. An unknown `repo`, including the
empty string, yields `null`; a known member can return an empty array. Omitted
or null `repo` selects the whole workspace, including dependencies. For a page
shorter than the limit, enumeration is complete; an exactly full final page
requires another request. Pages are not a snapshot: changes between reads can
cause duplicates or omissions. No total is computed.

`au_hubs` returns ranked targets with inbound structural, navigational, and
combined edge counts. It accepts an optional nonempty `repo`, optional `scope`
(`own` or `all`), `limit` (default 20, range 1–500), and `offset` (default 0).
When scope is omitted, engine defaults to `own`, or `all` when `repo` is set.
An unknown repo returns an empty list. Scope and repo filter targets only: counts
include all incoming edges, including sources outside the selected scope. The
result can expose paths and topology and is not an authorization boundary. An
explicit page limit bounds the response count, not ranking work in the engine.
Offset pages are not a snapshot; changes may cause duplicates or omissions.

`au_diagnostics` lists diagnostics with optional exact `repo`, `severity`,
`code`, and `scope` filters, combined with AND. `repo` and `code` must be
nonempty when supplied. Severity is `error`, `drift`, `warning`, or `hint`;
scope is `own` or `all`. With no scope, the engine defaults to `own`, or to
`all` when `repo` is supplied. An empty result does not imply other scopes have
no diagnostics. Pages use `limit` (default 50, range 1–500) and `offset`
(default 0); an exactly full page requires another request to detect the end.
Pages are separate reads, not a snapshot, so concurrent changes can cause
omissions or duplicates. A page limit bounds the number of diagnostics, not
engine work or response bytes; oversized frames fail rather than being
truncated. Client-supplied `path` and `path_prefix` are not accepted. Diagnostic
messages, spans, related locations, and suggested fixes may expose paths and
other workspace data. Suggested mutation actions are data, not permission to
execute them; no mutation is exposed. This tool is not an authorization boundary.

`au_files` lists catalogued files, including assets and dependencies. Optional
`repo` must be nonempty; `scope: own | all` defaults to `all` and composes with
`repo`. The default `limit` is 50, accepted range 1–500, with `offset` default
0. Paths may be absolute, and stems are not unique. Results are ordered by
path components, not ordinary string order. Pages are separate reads, not a
snapshot: changes may cause omissions or duplicates. A full final page requires
another request to detect the end. Page size bounds entries, not response bytes;
oversized frames fail. This is not an authorization boundary.

`au_content` reads one member's file text. Both `repo` and `path` are
required; `path` is relative to that member's declared root, and absolute
paths, `..` components, and NUL are rejected before any engine connection. The
adapter resolves the member root with one `members` read, then performs one
`content` read for the joined path. The result is `{ content: { text, hash,
commit } | null }`; `null` means the file is unreadable. `hash` is the value a
later write can guard on, and `text` and `hash` come from the same read. `commit`
is the owning repo's HEAD, or null outside a Git work tree. The returned text is
workspace content and may be sensitive. A workspace member without a local root
(for example a disabled member) is a tool error.

`au_preview_mutation` returns the engine's would-be product for a
`write_file`, `edit_file`, or `delete_file`: target path, would-be hash,
identities, diagnostics, and blast radius. It writes nothing, so it needs no
`expected_hash`, but the member must still be editable. Use it before a
mutation to inspect consequences.

`au_instances_of` lists the instances matching a type. A bare name matches every
identity of that name across the workspace; `name::repo` selects one. Optional
`origins` restricts to `file`, `nested`, or `meta` sites; `instance` and `body`
splice each match's resolved view or markdown body. The read is unpaged, so an
oversized frame fails rather than being truncated.

`au_validate_value` validates a JSON value against a type without touching
disk, returning one verdict per identity the name denotes with diagnostics and
advisory undeclared fields. An unknown type yields a single verdict with a null
identity carrying `unknown-type-claim`, never an empty array, so a caller folding
diagnostics cannot mistake an unvalidated value for a valid one. Spans index a
synthesized document and do not map back to the input.

The read corpus in `tests/fixtures/synthetic-reads/` records every advertised
read's payload from a real engine on a disposable two-member repository, with
paths normalized to `$FIXTURE` and git object names to `$COMMIT`. The ignored real-engine test regenerates or
verifies it, and `tests/fixture_schema.rs` keeps it honest without an engine.

`au_reconcile` resolves an interrupted mutation. Give the hash the write should
have produced, or `expect_absent: true` for a delete; it answers `applied` or
`not_applied` plus the file's current hash and commit. When the file exists with
a different hash, the answer re-arms the read-before-write guard with that hash,
so a deliberate retry needs no extra read. It never writes.

`au_write_file`, `au_edit_file`, and `au_delete_file` share one mutation
policy. Each requires a successful
`au_content` read of the same `repo` and `path` in the same server process, and
`expected_hash` must equal the hash that read returned; a missing or stale read
is rejected before any engine connection. The member must be `editable`; a
consumed member is refused before the mutation. One mutation runs at a time.
An engine rejection is definitive and nothing was written. A timeout, connection
failure, or malformed reply after the request is sent is reported as
`outcome_unknown`: the write may have landed, so the caller must read the file
again instead of retrying. A successful or unknown write discards the recorded
read. The MCP cancellation notification does not abort a mutation, because an
interrupted write has an unknown outcome; the bounded exchange is allowed to
finish. The engine still enforces its clean-at-HEAD precondition, editability,
and path guards. `au_edit_file` additionally requires `old_string` to match
exactly once unless `replace_all` is true; `au_delete_file` requires the file
to exist. The engine does not take `expected_hash` for `edit_file`, so there the
hash is only the adapter's staleness check.

`au_imports` lists the engine's resolved cross-repo type uses as an
`imports` array. Optional `scope: own | all` defaults to `all`; `own` filters
the importing members and excludes the automatic engine builtin fold, not
external owners of imports made by own members. Field-shape references,
unresolved imports, and dependencies with no type use are not included. This
is not a list of all declared dependencies. Results are unpaged and may reveal
repo and type names; oversized engine frames fail rather than being truncated.
The tool is not an authorization boundary.

`au_members` lists workspace members or selects an exact nonempty `repo`;
unknown repos return an empty array. It is unpaged and includes dependencies
and disabled members. Entries can contain absolute roots, role, local and
editable flags, and Git topology. These are engine facts, not authorization or
permission to write. An oversized response fails at the 16 MiB frame limit.

`au_subtypes` requires a nonempty `base` and accepts `scope: own | all`
(default `all`), which filters the members holding the returned subtypes. A
bare base matches by name across the workspace; `name::repo` selects that
member's base identity. Unknown bases return an empty array. Each matching
local definition is returned separately: identical definitions in different
members can share a hash and must not be deduplicated by hash when member data
matters. The engine's echoed base and full result envelope are preserved. This
read is unpaged; a response over the 16 MiB frame limit fails. Results can
include absolute paths and workspace content and are not an authorization
boundary.

Successful read results include `_meta["io.arsumbris/engineVersion"]`, a decimal
string copied from the same engine response. It is diagnostic only. The engine
version is not a globally unique snapshot ID: it can reset when the daemon
restarts, and this server opens a new connection for each read. Equal values do
not establish that pages came from the same snapshot. Metadata may be dropped
by clients; error results do not invent a version. The existing JSON text content
is unchanged.

This is a new interface, not a drop-in replacement for the Python bridge. It has
no mutation, daemon startup, subscriptions, search, or content read. It has not
been tested against a production daemon or vault. Run tests with `cargo test`;
they use a temporary fake daemon and exercise initialization for 2024-11-05,
2025-03-26, 2025-06-18 and 2025-11-25, not actual client applications. The
server requires a completed legacy `initialize` handshake for `tools/list`
and `tools/call`; self-contained request metadata does not replace it. A second
`initialize` in the same session is rejected and does not replace peer state.
The SDK still allows calls after the initialize response and before the client's
`notifications/initialized` notification. This is not an authorization boundary.
The server does not claim support for the 2026-07-28 protocol revision. In a separate
experiment with that revision enabled, rmcp 3.9 returned success for a first
`ping` carrying an unsupported version instead of error `-32022`; modern
protocol tests remain ignored pending resolution. The server is not a replacement
for the existing bridge. Rust 1.99 or newer is required.

An opt-in test runs against a separately built engine and a synthetic temporary
repository. It starts and tears down only its own daemon, using a temporary HOME:

```sh
cd ../au-engine
RUSTUP_HOME=/root/.rustup cargo +1.99.0 build -p au-cli --bin au
cd ../au-mcp-rs
RUSTUP_HOME=/root/.rustup AU_ENGINE_BIN=../au-engine/target/debug/au cargo +1.99.0 test --locked --test real_engine -- --ignored
```

This test does not establish full MCP conformance or production readiness.

## Client matrix

`scripts/client-matrix.sh` registers the adapter in the MCP clients installed on
this host and reports what each one actually verified; `docs/client-matrix.md`
records the measured result and the gaps. It runs in a throwaway `HOME` against a
disposable repository, so it touches neither user configuration nor the
production vault.

## Rollback rehearsal

`scripts/rollback_rehearsal.py` proves the fallback path: a granted adapter's
write is an ordinary engine commit, the adapter leaves no files behind, a fresh
process still reads the write, and starting the same binary without a grant is
the whole rollback. See `docs/rollback-rehearsal.md`.

## Bridge parity

`scripts/bridge_parity.py` runs the Python bridge, the Node daemon, and this
adapter against one disposable workspace and compares the facts both surfaces
state. `docs/bridge-parity.md` records the result and the surface differences a
cutover must decide on. The daemon core compiles in no tools: its surface comes
from `mcp.tool` plugins discovered in the served workspace, so the script mounts
`au-mcp-core` as a dependency.
