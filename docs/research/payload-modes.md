# Result payload modes, measured on the live adapter

Measured 2026-11-16 against the production vault (`/root/vault`, HEAD `b7ef429b`) served
by the live engine, read-only calls only. Adapter: `/usr/local/bin/2brain`
`0.1.0+87ea9851279d`, sha256 `fb9469d2…`, release build of `tb-mcp-rs`.

## The three modes

`--payload both|text|structured` decides what one tool call's result carries.
`PayloadMode` is `tb-mcp-rs/src/lib.rs` (enum near `lib.rs:569`), the flag is parsed in
`parse_args` (`lib.rs:4364`), and **the default is `both`**:

| mode | the result carries |
|---|---|
| `both` (default) | a `content[0].text` block holding the payload JSON, **and** `structuredContent` |
| `text` | the text block only |
| `structured` | `structuredContent` only |

`--info` repeats it as `profiles --tools full|concise, --payload both|text|structured`.

## Method

The strict client `tb-mcp-rs/scripts/lib/mcp_stdio.py`, subclassed with one change:
`_read` is copied verbatim and records `len(raw) + 1` for every newline-terminated
frame. That is the real response frame on the wire, framing included, request excluded.
A fresh server process per mode, the same calls in each; the suite ran twice,
byte-identical, on two builds of the same source.

## Bytes per call

| call (size of the answer) | `both` | `text` | `structured` |
|---|---:|---:|---:|
| `tb_content` `ops/tasks.md` (383 B answer) | 934 | 528 | **498** |
| `tb_files` `limit=500` (75 803 B answer) | 159 746 | 83 922 | **75 916** |
| `tb_search` `limit=50` (5 660 B answer) | 11 870 | 6 189 | **5 773** |
| 36 read calls (every read tool, one session) | 22 023 267 | 11 640 203 | **10 386 147** |
| `initialize` / `tools/list`, mode-independent | 937 / 54 661 | 937 / 54 661 | 937 / 54 661 |

`both` is exactly the `text` frame plus the `structured` frame minus 92 B of envelope
shared between the two: one frame, two copies. The text copy is a second encoding, so
its quotes are escaped — for `tb_files` it is 8 006 B larger than the structured copy of
the same payload. `tb_files` therefore costs **2.10×** in `both` what it costs in
`structured`. The 36-call total is dominated by two unbounded reads (`tb_instances`
18.9 MB, `tb_candidates` 3.1 MB in `both`): it says what a chatty session pays.

`--tools concise` (orthogonal) cuts `tools/list` from 54 661 B to 17 923 B.

## Is `structuredContent` alone enough for a schema-validating client?

- `tools/list` advertises 36 read tools without a grant and 47 with `--allow-mutations`;
  **all 47 declare an `outputSchema`** (47/47).
- 36/36 read tools returned `structuredContent`, and each payload validated against its
  declared schema with `Draft202012Validator` (jsonschema 4.10.3).
- On every success the text block parses to exactly `structuredContent` (36/36,
  JSON-equal). **No tool carries data only in text on success.**

So a client that validates schemas loses nothing from a structured-only result on the
success path.

## Where text is still the only copy

- **Errors.** `isError: true` results carry a text block and no `structuredContent`, in
  *every* mode — verified in `structured` mode too: `tb_content {repo:"vault"}` misses
  `path` and answers with the deserialization message as text, `isError: true`, no
  structured content. A structured default therefore does not lose error text (the
  block is still there), but a client reading only `structuredContent` sees no message,
  and an error cannot be validated against `outputSchema`.
- rmcp's own argument-validation failures have the same shape and the same conclusion.

## Conclusion

A `structured` default would cut response bytes **52.8%** on the read suite (46.7% on the
short read, 52.5% on `tb_files`) — and 9.5% even against `text`, because the text copy
pays JSON escaping. It breaks nothing on the success path measured here; it only breaks
clients that read `content[0].text` and ignore `structuredContent`, which would then get
an empty `content` array on success. Errors are unaffected: they are text-only today and
stay text-only. If that compatibility risk is too high, `text` is the half-measure
(47.1% saved, old clients keep working) at the cost of escaping and a re-parse per call.

## Commands

```
# Adapter on the live socket, one process per mode:
/usr/local/bin/2brain mcp /root/.arsumbris/au-engine/run/adcfe8680a485529.sock \
    [--payload both|text|structured]
#   initialize (2025-06-18) -> tools/list -> tools/call
#     tb_content {repo:"vault", path:"ops/tasks.md"}
#     tb_files   {repo:"vault", limit:500}
#     tb_search  {query:"arsumbris", limit:50}
# then the same 36 read-tool calls in every mode

# Read-only launch used for all measurements (never --allow-mutations):
/usr/local/bin/2brain mcp /root/.arsumbris/au-engine/run/adcfe8680a485529.sock

# NOTE: `2brain mcp /root/vault` derives the socket from $HOME. In this session
# HOME=/root/.allspark-home, where no socket exists, so the entry form failed with
# "connecting to engine: No such file or directory". The socket path was passed
# explicitly instead; the vault and daemon are the same ones.
```

## What was decided, and the one config change

The global default stays **`both`**. It is the only mode that serves a client which reads `content[0].text`
and a client which validates `structuredContent`, and the adapter does not know which one is on the other end
of stdio. Changing the default to `structured` would silently hand an empty `content` array to any text-only
client, and that failure looks like "the tool returned nothing", not like a mode mismatch.

The one real client here is known, so the choice is made where the client is configured rather than for
everyone. Hermes reads `content[0].text` — `grep -rI structuredContent /root/hermes-agent` finds it only in
the vendored MCP SDK, never in Hermes' own code — so its entry asks for text only:

```yaml
  2brain:
    command: /usr/local/bin/2brain
    args: [mcp, /root/vault, --allow-mutations, --payload, text]
```

Verified on the live socket: the same three reads answer with JSON-equal text and no `structuredContent`
block, at 47.1% fewer bytes (22.0 MB → 11.6 MB over the read suite). The previous config is backed up at
`/root/backups/hermes-config-20261116-011954.yaml`.

Revisit when a client that validates schemas is connected: that one wants `--payload structured`, and the
52.8% saving is only real for it.

## Correction, same day: the client *does* validate schemas

This section's original claim — "Hermes reads `content[0].text`, so text-only is safe for it" — was wrong, and
the logs prove it. With `--payload text` in the Hermes entry, six tool calls failed in 46 seconds:

```
Tool tb_overview has an output schema but did not return structured content
```

The check is not in Hermes' own code, which is why searching Hermes' source found nothing; it is in the MCP
client layer the server is consumed through, and it enforces the spec rule that a tool declaring an
`outputSchema` must return `structuredContent`. Text-only therefore breaks every read tool, not just the ones
that care about shapes, and the client's circuit breaker turns the third failure into "server unreachable".

The entry is back on the default (`both`) and the three calls were re-checked live. The measurement in the
table above stands — the bytes are real — but the compatibility conclusion does not: **`text` is only safe
for a client that ignores schemas, and this one does not**. The full account is in
`docs/research/hermes-log-findings.md`.
