
# What Hermes' logs say about the 2brain server

Read 2026-11-16 from `/root/.hermes/logs/{errors,agent,mcp-stderr,gateway}.log` (27 MB total): 64 lines mention
2brain across `errors.log` (12), `agent.log` (49) and `mcp-stderr.log` (3). Every finding below is a line in
those files, not a hypothesis.

## 1. Six failures in 46 seconds: `--payload text` breaks a schema-validating client

```
01:52:21 ERROR tools.mcp_tool: MCP tool 2brain/au_overview call failed:
         Tool au_overview has an output schema but did not return structured content
01:52:42 … au_top_level_dirs …
01:53:07 … au_grep …  then: MCP server '2brain' is unreachable after 3 consecutive failures
```

Six calls failed between 01:52:21 and 01:53:07, and the third failure tripped the client's circuit breaker,
which is why the model then saw "server is unreachable" rather than another schema error.

**Cause: my own config change.** `docs/research/payload-modes.md` concluded that Hermes reads only
`content[0].text` because `structuredContent` does not appear in Hermes' own source. That conclusion was
wrong in the way that matters: the check lives in the MCP client layer the server is consumed through, and it
enforces the spec rule that a tool declaring an `outputSchema` must return `structuredContent`. Text-only
therefore fails every read tool that declares a schema — which is all of them.

The config was restored to the default (`both`) at 01:53:48, and the same three calls now answer with
`structuredContent` present and the text block intact (verified against the live socket). The lesson is in
`payload-modes.md`: text-only is safe for a client that ignores schemas, and this client does not ignore them.

## 2. The audit ledger did not name the tool

`mcp-stderr.log` carries 28 `mcp_policy` records with a hash chain — the provenance trail. Seventeen of them
said `"tool":"mutation"` instead of the tool that ran, because the shared `guarded_mutation` helper recorded a
literal. An audit record that says `mutation` cannot answer "which of the six mutating tools was refused",
which is the one question it exists for.

Fixed: `guarded_mutation` takes the tool name, all six call sites pass it, and `tests/policy.rs` now asserts
the ledger names `au_write_file` and `au_delete_file` rather than a placeholder. `lib.rs` pays 25 lines on the
ratchet with the reason recorded next to the constant.

## 3. Policy refusals are frequent but correct

Four `policy/prior_read` refusals and two `policy/absence` refusals in the same window, each naming the stage
and the fix ("read this file with au_content before writing it", "file already exists; use au_write_file").
These are the gate working as designed. They cost a turn each, and the message is what makes that turn cheap,
so no change is warranted — but they are worth counting over a longer window before calling the gate tuned.

## 4. Two argument-shape refusals, both the model guessing

- `au_content` refused `limit` (`additionalProperties`): the tool takes `max_chars`, and the server's own
  instructions say `au_content` is unpaged. The refusal named the accepted fields, which is the contract
  working.
- `au_edit_file` was called without `path` and `expected_hash`; the refusal named both.

Neither is a server defect. The second is a reminder that `expected_hash` is the one argument a model drops
most often, because it is not something the model can invent.

## What was not found

No panic, no crash, no lost-reply, no `outcome_unknown`, and no engine timeout in the window reviewed. The
socket path, the state root and the systemd unit never appear in an error line — tier B and D moved nothing
that a running client noticed.

## The one thing to watch

The client's circuit breaker ("unreachable after 3 consecutive failures") turns a schema mismatch into an
outage-shaped symptom, and the retry is ~59 s. A single bad payload mode therefore looks like a dead server.
That argues for keeping the server's default mode compatible rather than optimal, which is what the config
decision now does.
