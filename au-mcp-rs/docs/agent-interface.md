# Agent interface: first call and subsequent calls

Measured 2026-11-13 on the production workspace, raw stdio, release build.
"First call" means what a client sees before it picks a tool; "subsequent calls"
means what one tool invocation costs and returns.

## First call

| what | before | after |
|---|---|---|
| `initialize` latency | 14 ms | 10 ms |
| `instructions` in the initialize result | absent | present: purpose, envelope contract, suggested flow, unpaged reads, mutation policy and `outcome_unknown` |
| `tools/list` size | 15 874 chars (~4.0k tokens) | 18 598 chars (~4.6k tokens), the growth being `outputSchema` |
| tools with `annotations` | 16/16 | 16/16 |
| tools with `outputSchema` | 0/16 | 16/16 |
| schema strictness | `additionalProperties: false`, `minLength`, `limit` bounds | unchanged |

Adding `outputSchema` costs ~0.6k tokens once per session and buys a declared
answer shape for every tool, so a client parses the result instead of guessing.

For comparison, the existing bridge advertises 49 tools in 42 625 chars
(~10.7k tokens) with no annotations and no output schemas, and `initialize` takes
66 ms.

## Subsequent calls

| what | measurement |
|---|---|
| `au_overview`, first answer | 7 466 chars (~1.9k tokens), 17–29 ms |
| `structuredContent` | present on every result, equal to the text payload |
| engine version metadata | `_meta["io.arsumbris/engineVersion"]` on every result |
| `au_files` limit 500 | 65 806 chars (~16.5k tokens), 16 ms |
| `au_type_tree` | 13 467 chars (~3.4k tokens), 14 ms |
| `au_types` limit 500 | 15 228 chars (~3.8k tokens), 6 ms |

Errors, by case:

| case | answer |
|---|---|
| unknown tool | JSON-RPC `-32602 tool not found` |
| mutation disabled by policy | JSON-RPC `-32600 mutations are disabled by this server's launch policy` |
| absolute path | tool error: `'path' must be relative to the member root` |
| `..` in path | tool error: `'path' must not contain '..'` |
| unknown member | tool error: `unknown workspace member: <name>` |
| stale hash, no read | tool error: `policy/prior_read: read this file with au_content before writing it` |
| lost mutation reply | tool error: `outcome_unknown: … read the file again before retrying` |

## Where it stands against agent-friendly practice

Met:

- **one discovery call, cheap**: 4.6k tokens, 1 ms, 16 tools;
- **self-description**: an `instructions` block plus per-tool descriptions that
  state defaults, limits, and failure behaviour;
- **closed schemas**: `additionalProperties: false`, `minLength`, bounded
  `limit`, enums where the value set is fixed;
- **typed answers**: `outputSchema` and `structuredContent` carry the same
  payload the text does;
- **safe defaults**: reads only unless the operator grants mutations; mutating
  tools are absent from `tools/list` and refused before the engine is contacted;
- **retry safety**: the read-before-write hash guard, `outcome_unknown` instead of
  a blind retry, and `au_reconcile` to settle the outcome;
- **caching signals**: `_meta` engine version on every result;
- **deterministic order and no timestamps** in payloads.

Not met, in priority order:

1. **Large unpaged answers.** `au_instances_of`, `au_content`, and
   `au_type_tree` can return tens of thousands of tokens with no truncation flag
   and no budget argument. `au_files` is bounded by `limit`, those three are not.
2. **Argument errors are the SDK's serde text** (`failed to deserialize
   parameters: missing field \`name\``): it names the missing field but not the
   tool or the accepted shape. A client must consult `inputSchema`.
3. **Tool errors carry no machine-readable code.** The text embeds
   `policy/<stage>:` and `outcome_unknown:`, which is parseable but not a field.
4. **No progress notifications** for reads that approach the 30 s deadline.
5. **`ping` and discovery are the only pre-tool calls**; nothing reports the
   engine's schema version or degraded state at connect time, so a mismatched
   engine is discovered on the first read rather than at startup.
