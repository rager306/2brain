# The MCP↔engine path, measured — what to delete now that there is no TypeScript

Measured 2026-11-16 against the live daemon and the installed `/usr/local/bin/2brain`.

## The numbers

| path | per call | note |
|---|---:|---|
| raw engine frame, persistent connection | 1.24 ms | 500 calls, one socket |
| raw engine frame, connect per call | 1.14 ms | 500 calls, a socket each |
| through the adapter (`tb_content`) | 2.80 ms | 200 calls, includes the Python MCP client |

Two conclusions that constrain the design:

1. **A fresh connection per call costs nothing measurable.** The daemon's "fork per connection" leaves no
   children behind and reconnect measures *faster* than persistent (1.14 vs 1.24 ms — inside noise). A
   connection pool is therefore not an optimization; it is a new failure mode (a dead pooled socket after a
   daemon restart) for no gain.
2. **Going in-process is a loss, not a shortcut.** The daemon holds the built IR at **72.6 MiB** RSS for
   1 036 notes; a per-session stdio adapter would duplicate that in every client process, and — worse — would
   become a second writer of a vault whose contract is "the engine is the authority". The socket is not
   ceremony; it is the seam that keeps one writer.

So the ~1.7 ms the adapter adds is not a transport problem. It is the MCP layer doing its job (schema,
policy, audit) plus the client's own stdio round trip.

## What is genuinely legacy, and worth removing

| item | why it exists | what deleting it buys |
|---|---|---|
| `src/wire.rs`'s `match verb { "overview" => payload.is_object(), … }` (20 arms) | the TS bridge needed a shape check per verb; the check was hand-maintained | it is a **bug class**, not a guard: when `search` landed the arm was missing and every call failed with "invalid read result payload" while every test stayed green. A typed deserialize per verb, or a fixture-driven check derived from `tests/fixtures/synthetic-reads/`, removes the hand-written list |
| `ReadResult { version, payload: Value }` | the payload was forwarded between two languages | with one language the contract can be per-verb Rust structs; the compiler then enforces the shape the fixtures already pin |
| `preview_mutation` / `reconcile` as separate tools | the bridge could not express the read-then-write CAS | **keep** — they are the recovery path for `outcome_unknown`, and the gate depends on them |
| `tb_search_tools` / `tb_describe_tool` | discovery over 52 tools | **keep** — they save a model round trip |
| per-tool Rust arms in `lib.rs` (52 tools, ~1 700 lines) | each tool has a schema and a description | not removable without losing the typed contract; but the *bodies* are 3–8 lines each and already thin |

## The three refactors that are actually worth doing

1. **Per-verb typed payloads** (highest value): define `struct OverviewPayload`, `struct SearchPayload`, … for
   the hot reads (`overview`, `content`, `files`, `search`, `diagnostics`, `hubs`, `graph_health`), deserialize
   in `wire.rs`, and delete the arm-match. The fixture set already contains one payload per read, so the
   structs are validated by data that exists. Cost: a day; benefit: the `search` class of defect becomes a
   compile error.
2. **One source for the verb list**: `wire.rs`, `capability.rs`, the router in `lib.rs`, and
   `tests/fixtures/synthetic-reads/` all enumerate the verbs. A single `const VERBS: [(&str, Verb); N]`
   (name, capability, payload struct) would let the compiler check all four, instead of four tests
   noticing drift.
3. **`--payload structured` as the default**: the adapter currently ships each answer twice (text and
   `structuredContent`). Measured payload for `tb_files` (500 rows) is large; a client that validates
   `structuredContent` (Hermes does) reads one copy and the text copy is dead weight. This is a config
   change with a client-compat question — measure the byte reduction first.

## What NOT to do

- **Do not put the engine inside the adapter.** 72.6 MiB per session and a second writer.
- **Do not pool connections.** No measured win, one new failure mode.
- **Do not collapse the 52 tools into a generic `au_call`.** The typed surface is what makes the tool
  catalogue usable by a model; a passthrough would move schema errors from compile time to runtime.
- **Do not delete the policy layer.** It is the only thing standing between a model and an unwritten,
  unreconciled vault.

## How to verify each step

`tools/ranking/eval.py` for anything touching search; `scripts/live_parity.py` for the adapter's shape;
`scripts/strict-client.py` for the schema contract; and the fixture replay tests, which are the reason the
`search` bug was caught late rather than never — they need a `tb_search` case that actually calls the tool,
which is now added.
