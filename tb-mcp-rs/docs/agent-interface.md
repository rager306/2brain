# Agent interface: first call and subsequent calls

Measured 2026-11-14 on the production workspace, raw stdio, release build.
"First call" means what a client sees before it picks a tool; "subsequent calls"
means what one tool invocation costs and returns.

## First call

| what | before | after |
|---|---|---|
| `initialize` latency | 14 ms | 10 ms |
| `instructions` in the initialize result | absent | present: purpose, envelope contract, suggested flow, unpaged reads, mutation policy and `outcome_unknown` |
| capabilities | tools | tools + resources (list, read, templates; no subscribe) |
| `tools/list` size | 15 874 chars (~4.0k tokens), 16 tools | `50 019` chars (~12.5k tokens), `35 read-only` tools; the growth is `outputSchema` plus the later reads |
| tools with `annotations` | 16/16 | 35/35 |
| tools with `outputSchema` | 0/16 | 35/35 |
| schema strictness | `additionalProperties: false`, `minLength`, `limit` bounds | unchanged |

`outputSchema` is part of that growth: it is paid once per session and buys a
declared answer shape for every tool, so a client parses the result instead of
guessing.

For comparison, the existing bridge advertises 49 tools in 42 625 chars
(≈10 656 tokens) with no annotations and no output schemas, and `initialize`
takes 66 ms.

## Subsequent calls

| what | measurement |
|---|---|
| `tb_overview`, first answer | 7 466 chars (~1.9k tokens), 17–29 ms |
| `structuredContent` | present on every result, equal to the text payload |
| engine version metadata | `_meta["io.arsumbris/engineVersion"]` on every result |
| `tb_files` limit 500 | 65 806 chars (~16.5k tokens), 16 ms |
| `tb_type_tree` | 13 467 chars (~3.4k tokens), 14 ms |
| `tb_types` limit 500 | 15 228 chars (~3.8k tokens), 6 ms |

Errors, by case:

| case | answer |
|---|---|
| unknown tool | JSON-RPC `-32602 tool not found` |
| mutation disabled by policy | JSON-RPC `-32600 mutations are disabled by this server's launch policy` |
| absolute path | tool error: `'path' must be relative to the member root` |
| `..` in path | tool error: `'path' must not contain '..'` |
| unknown member | tool error: `unknown workspace member: <name>` |
| stale hash, no read | tool error: `policy/prior_read: read this file with tb_content before writing it` |
| lost mutation reply | tool error: `outcome_unknown: … read the file again before retrying` |

## Where it stands against agent-friendly practice

Met:

- **one discovery call, cheap**: 12.5k tokens, 35 read-only tools;
- **self-description**: an `instructions` block plus per-tool descriptions that
  state defaults, limits, and failure behaviour;
- **closed schemas**: `additionalProperties: false`, `minLength`, bounded
  `limit`, enums where the value set is fixed;
- **typed answers**: `outputSchema` and `structuredContent` carry the same
  payload the text does;
- **safe defaults**: reads only unless the operator grants mutations; mutating
  tools are absent from `tools/list` and refused before the engine is contacted;
- **retry safety**: the read-before-write hash guard, `outcome_unknown` instead of
  a blind retry, and `tb_reconcile` to settle the outcome;
- **caching signals**: `_meta` engine version on every result;
- **deterministic order and no timestamps** in payloads.

One measured lesson: a client validates `structuredContent` against
`outputSchema`, so a field the adapter adds but does not declare is a protocol
error at the client. Paging metadata (`total`, `offset`, `next_offset`) is
declared in every envelope schema for that reason.

Not met, in priority order:

1. **Large unpaged answers.** `tb_content` and `tb_instances_of` now take
   explicit budgets, and every windowed answer says what was withheld.
   `tb_type_tree` and `tb_validate_value` remain unbounded; a caller can only
   avoid them or accept the size.
2. **Argument errors** now name the tool, list its required fields, and point at
   `tb_describe_tool`; the underlying text still comes from the SDK.
3. **Tool errors carry no machine-readable code.** The text embeds
   `policy/<stage>:` and `outcome_unknown:`, which is parseable but not a field.
4. **No progress notifications** for reads that approach the 30 s deadline.
5. **`ping` and discovery are the only pre-tool calls**; nothing reports the
   engine's schema version or degraded state at connect time, so a mismatched
   engine is discovered on the first read rather than at startup.
