# Progressive disclosure and token economy for MCP tools

What the sources actually say, what a Rust MCP server can do with it, and what
this server now does. Measured numbers are from this repository, 2026-11-13.

## Sources

- Anthropic, *Code execution with MCP*: presenting tools as code lets an agent
  read only the definitions it needs; a Google-Drive-to-Salesforce flow dropped
  from 150 000 to 2 000 tokens (−98.7%). Two named costs: definitions loaded up
  front, and intermediate results passing through the context.
- Anthropic, *Tool search tool*: `defer_loading: true` on tool definitions plus a
  search tool; matches come back as `tool_reference` blocks (5 by default),
  regex or BM25 variants; reported 85% context preservation and accuracy gains
  on MCP evaluations (Opus 4 49%→74%, Opus 4.5 79.5%→88.1%).
- MCP specification 2025-11-25, *Tools*: `tools/list` takes a `cursor` and returns
  `nextCursor`; `notifications/tools/list_changed` announces catalogue changes;
  `outputSchema` plus `structuredContent` are the typed-result pair, and a tool
  returning structured content SHOULD also serialize it in a text block.
- `modelcontextprotocol` discussion #2036 (tool bloat): server-side meta-tools
  (`search_tools`/`load_domain`) plus `listChanged` work, but retrieval can go
  stale mid-conversation, so recently used tools are pinned for a few turns;
  namespaced names are recommended; and "most of the token bloat comes from
  `inputSchema`, not tool names".
- The New Stack, *10 strategies to reduce MCP token bloat*: separate discovery
  from execution, build tool hierarchies, keep parameter descriptions to one
  sentence, prefer enums over prose, and drop rarely used optional parameters.

## The four independent costs

1. **Catalog cost** — every tool definition in context, paid once per session.
2. **Intermediate cost** — tool results flowing back through the model.
3. **Round-trip cost** — extra calls a vague description forces.
4. **Error cost** — a failure that arrives without a fix instruction costs a
   whole extra turn.

Techniques below attack exactly one of these; a server can only influence 1, 3,
4 fully, and 2 partially.

## Techniques, with their semantics

### 1. Ladder of detail (server-side)

Three depths for the same catalogue: `name` (≈5–10 tokens), `name + one line`,
`full definition`. The agent climbs only as far as it needs.

Semantics that matter: the ladder must be **stable** (same names at every
depth), **complete** (nothing hidden by the ladder itself), and **cheap at the
top** (the search tool must be the smallest definition in the list).

Algorithm used here: tokenize the query and each tool into lowercase
alphanumeric words (underscores and dots split, so `au_content` matches
"content"); score a term 3 in the name, 2 in a parameter name, 1 in the
description, plus 1 for a prefix match; keep positive scores, sort by score then
name for determinism, take `limit` (default 5, max 20). BM25 is unnecessary at
this catalogue size; what matters is determinism and that the first hit is right.

### 2. Schema minimization (server-side)

The largest single cost is `inputSchema`. What to keep: property names, types,
`required`, `enum`, bounds. What to cut in the cheap profile: `description` on
every node, `$schema`, `title`, and `outputSchema` (recoverable through the
ladder). What never to cut: validation-relevant constraints, because a schema
that lies about its own rules causes cost 3 and 4.

### 3. Deferred loading (host-side, not the server's decision)

`defer_loading` and `tool_reference` live in the client/API. A server cooperates
by making that decision possible: stable names, descriptions that discriminate
(no two tools described as "manage documents"), and no server-side state tied to
"was this definition loaded".

### 4. Pagination and budgets (server-side)

`tools/list` supports a cursor for a reason; a server with hundreds of tools
should page it rather than emit one 100k-token blob. For results, the rule is the
same as for errors: **never silently truncate**. A truncated payload carries
`truncated: true` and a continuation (`next_offset`/cursor), so the agent can
choose to pay for the rest.

### 5. Result shaping (server-side, partial)

Projections (`summary: true`, `fields`, `limit`) let the agent ask for less.
Aggregations computed server-side replace "fetch 10 000 rows, filter in the
model". Both are ordinary arguments, not new tools.

### 6. Code execution / programmatic calls (host-side)

When the host runs agent-written code against tool endpoints, intermediate
results stay in the sandbox. A server helps by returning stable, machine-shaped
JSON (`structuredContent`), documenting idempotency, and never requiring a
model turn between two dependent reads.

### 7. Catalogue freshness

`notifications/tools/list_changed` exists so a client can refresh. If the
catalogue is dynamic, the server must send it; and a client that trims the
catalogue should pin recently used tools for a few turns, or retrieval staleness
forces a re-fetch loop.

### 8. Error semantics

A failure should carry: what failed, which input caused it, and the next action.
The cheapest error is one the agent can fix without another round trip. Machine
codes (`policy/prior_read`) cost a few tokens and save a turn.

### 9. Caching and consistency signals

A payload that names its version (`_meta` engine version, content hash) lets a
client cache safely and detect drift without re-reading. Free at the token level.

### 10. Long-running work

The 2025-11-25 spec has task-augmented execution (`execution.taskSupport`). For a
server whose reads can approach a client timeout, tasks beat "poll this other
tool" loops.

## What this repository now does

| technique | state |
|---|---|
| ladder of detail | `au_search_tools` with `detail: name \| summary \| schema` |
| full definition on demand | `au_describe_tool` |
| concise profile | `--tools concise`: one-line descriptions, no `outputSchema`, no parameter descriptions or presentation keys |
| typed results | `outputSchema` + `structuredContent` on every tool (full profile) |
| annotations | `readOnlyHint` / `destructiveHint` / `idempotentHint` on every tool |
| result metadata | `_meta["io.arsumbris/engineVersion"]` |
| stable names, deterministic order | yes; search sorts by score then name |
| `tools/list` cursor pagination | not used: 18 tools fit one page |
| `listChanged` | not used: the catalogue is fixed at launch |
| task-augmented execution | not used: reads finish in milliseconds to seconds |
| result budgets for unpaged reads | **open**: `au_content`, `au_type_tree`, `au_instances_of` can still be large |

## Measured effect of the concise profile

Live workspace, release build, raw stdio:

| profile | tools | tools/list | tokens (chars/4) | first `search` | first `describe` |
|---|---|---|---|---|---|
| `full` | 18 | 20 636 chars | ≈5 159 | 876 chars | 2 298 chars |
| `concise` | 18 | 8 718 chars | ≈2 179 | 876 chars | 2 298 chars |

−58% at discovery. The break-even rule: describing one tool costs ≈2 300 chars,
so the concise profile pays as soon as fewer than ~5 definitions are needed, and
pays more and more as the catalogue grows. With 18 tools it is a modest win for
narrow tasks and a loss for a session that needs everything; with 200 tools it is
the difference between a usable agent and a blown context.

For reference, the existing Python bridge advertises 49 tools in 42 625 chars
(≈10 656 tokens) with no annotations and no output schemas.
