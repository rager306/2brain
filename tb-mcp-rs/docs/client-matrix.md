# Client compatibility matrix

> **Historical record, 2026-11-14.** The client matrix was run against the adapter under the server id
> `arsumbris-rs`; the product name is `2brain` since 2026-11-16, and tool names now register as
> `2brain__au_*`. The transcripts are kept verbatim — they are the evidence, not a how-to.

Measured by `scripts/client-matrix.sh` on 2026-11-13, on this host, against a
disposable two-file repository and a private engine socket. The script uses a
throwaway `HOME`, so no user or project configuration is read or written.

| client | version | what was exercised | result |
|---|---|---|---|
| Claude Code | 2.1.316 | `claude mcp add --scope local`, then `claude mcp list` health check | connected: the client completed `initialize` and `tools/list` |
| Codex CLI | 0.164.0 | `codex mcp add`, then `codex mcp list` | registered and listed; this client offers no non-interactive health check, so the handshake was **not** exercised |
| Cursor agent | 2026.10.22-27dcf5f | configuration only (no `mcp` subcommand; it reads `.cursor/mcp.json`) | **not exercised**: needs an API key and a model turn |

## What the matrix does and does not prove

- Claude Code's health check is a real client handshake: it spawns the adapter,
  completes `initialize`, and reads the tool list. A protocol or lifecycle
  regression breaks it.
- A registered-and-listed Codex entry proves the config shape the client accepts
  (stdio command plus args), not that a session succeeds.
- No client was run through a model turn, so nothing here exercises tool-call
  arguments chosen by a model, approval prompts, or client-side timeouts.
- Only the `2025-06-18` revision is reached by this script, because that is what
  these clients request. Other claimed revisions are covered by
  `tests/modern_stdio.rs`, not by a real client.

## Re-running

```sh
cd au-mcp-rs
AU_ENGINE_BIN=../tb-engine/target/debug/au ./scripts/client-matrix.sh
```

The script exits non-zero when an installed client that offers a health check
reports anything other than a connected server. Clients that are not installed
are reported as such and do not fail the run.

## Re-run 2026-11-14: fork release binaries

Run again on this host with the fork release builds instead of the debug builds:

```sh
cd au-mcp-rs
AU_ENGINE_BIN=/root/arsumbris/tb-engine/target/release/au \
AU_MCP_RS_BIN=/root/arsumbris/tb-mcp-rs/target/release/tb-mcp-rs \
bash scripts/client-matrix.sh
```

The run exited 1. All three clients are installed. The table records only what
each client reported in this run.

| client | version | what was exercised | result this run |
|---|---|---|---|
| Claude Code | 2.1.323 | `claude mcp add --scope local`, then `claude mcp list` health check | **not connected**: the health check failed with `ENOENT` on the adapter path it was given, so the client never started the binary |
| Codex CLI | 0.164.0 | `codex mcp add`, then `codex mcp list` | registered and listed; the handshake was **not** exercised (no non-interactive health check) |
| Cursor agent | 2026.10.22-27dcf5f | configuration only (`.cursor/mcp.json`) | **not exercised**: needs an API key and a model turn |

### The failure, verbatim

Full output of the run, including the exit status the harness appended:

```text
engine: /tmp/au-client-matrix-1vQCCf/home/.arsumbris/au-engine/run/afa490f1da63b5bb.sock
adapter: /root/arsumbris/tb-mcp-rs/target/release/tb-mcp-rs

client         version    result
---------------------------------------------------------------
claude         2.1.323    FAILED: arsumbris-rs: /root/arsumbris/tb-mcp-rs//root/arsumbris/tb-mcp-rs/target/release/tb-mcp-rs /tmp/au-client-matrix-1vQCCf/home/.arsumbris/au-engine/run/afa490f1da63b5bb.sock - ✘ Failed to connect — ENOENT: no such file or directory, posix_spawn '/root/arsumbris/tb-mcp-rs//root/arsumbris/tb-mcp-rs/target/release/tb-mcp-rs'
codex          0.164.0    registered; no non-interactive health check
cursor-agent   2026.10.22-27dcf5f configured via .cursor/mcp.json; needs an API key, not exercised
EXIT:1
```

### What this run verified, and what it did not

- The Claude Code entry was written by `claude mcp add`, and the health check
  then reported a failed connection. The path in the error is the adapter path
  with its parent directory prepended twice:
  `/root/arsumbris/tb-mcp-rs//root/arsumbris/...`. The script builds that
  argument as `$OLDPWD/$ADAPTER`, and `AU_MCP_RS_BIN` was supplied as an
  absolute path. No binary exists at the doubled path, so the spawn failed
  before the adapter ran.
- Because the adapter never started, this run says nothing about whether the
  adapter serves Claude Code. It measures a client-side launch failure.
- Codex wrote and listed a config entry. That proves the config shape the
  client accepts (stdio command plus args), not that a session succeeds.
- Cursor agent received no exercise at all. No `.cursor/mcp.json` handshake was
  performed.
- No client completed `initialize` or `tools/list` in this run, and no client
  was driven through a model turn. The 2026-11-13 result above remains the last
  recorded connected health check on this host.

## Re-run 2026-11-14 (second): the ENOENT was ours, and Claude Code connects

The failure above was a bug in `scripts/client-matrix.sh`, not a client or adapter
problem: the script registered `"$OLDPWD/$ADAPTER"` while `AU_MCP_RS_BIN` was
already absolute, so the path doubled (`/root/arsumbris/tb-mcp-rs//root/arsumbris/...`).
Fixed by resolving the adapter path to absolute ONCE, before any client config is
written, and passing that path to both `claude mcp add` and `codex mcp add`.

Same command, same binaries, after the fix:

```text
engine: /tmp/au-client-matrix-MNGeir/home/.arsumbris/au-engine/run/af3ace6d988bed39.sock
adapter: /root/arsumbris/tb-mcp-rs/target/release/tb-mcp-rs

client         version    result
---------------------------------------------------------------
claude         2.1.323    connected (initialize + tools/list)
codex          0.164.0    registered; no non-interactive health check
cursor-agent   2026.10.22-27dcf5f configured via .cursor/mcp.json; needs an API key, not exercised
```

Exit status 0: the only client with a non-interactive health check now reports a
connected server, which means it spawned the adapter and completed `initialize`
plus `tools/list` against the disposable engine. Codex and Cursor remain what the
first run recorded — registered/configured, handshake not exercised without an
interactive session.

## 2026-11-14 (third): a real model turn, and it found the marker

Everything above stops short of the one thing a client is for: a model choosing a
tool. `scripts/model-turn.sh` closes that with one small turn on the operator's own
credentials — deliberately a separate script, because `client-matrix.sh` must stay
free and offline.

The fixture holds `unique-marker-7f3a` in `notes/a.md`, a string no engine knows
except this one, so a correct answer cannot be a lucky guess:

```text
engine:  /tmp/au-fixture-DSYaVF/home/.arsumbris/au-engine/run/123e4ed907267994.sock
adapter: /root/arsumbris/tb-mcp-rs/target/release/tb-mcp-rs
prompt:  find unique-marker-7f3a with au_grep
PASS: the model called mcp__arsumbris-rs__au_grep
PASS: the marker travelled through the engine (tool result and/or answer)
PASS: the answer names the file that holds the marker
PASS: no permission denial (the allow-list covered the call)
PASS: one model turn completed through the adapter
```

The transcript is the interesting part. The client loaded the tool lazily, then the
model sent exactly the arguments the schema asks for, and the engine's payload came
back as a tool result rather than as prose:

```json
{"type":"tool_use","name":"mcp__arsumbris-rs__au_grep","input":{"query":"unique-marker-7f3a"}}
{"type":"tool_result","content":"{\"grep\":{\"matches\":[{\"column\":10,\"line\":6,
  \"path\":\".../fixture/notes/a.md\",\"repo\":\"fixture\",
  \"text\":\"find me: unique-marker-7f3a\",\"text_truncated\":false}],
  \"skipped_files\":0,\"total\":1,\"truncated\":false}}"}
{"type":"text","text":"`.../fixture/notes/a.md` (line 6): `find me: unique-marker-7f3a`"}
```

Three facts this settles: the tool description is good enough for a model to pick
the right tool from 35, the input schema's `query` name is what a model actually
supplies, and a payload shape with `matches`/`total`/`truncated` survives the MCP
boundary intact. The turn cost $0.042 and took 12 s.

Both scripts now share the fixture through `scripts/lib/disposable-fixture.sh`, so a
client check and a model-turn check cannot disagree about what they ran against.

## 2026-11-14 (fourth): the mutation path, through a model

`scripts/mutation-turn.sh` asks for the half that decides whether a note-editing
grant is safe: the model must read first, carry the hash it read into the write, and
the write must land as one engine commit.

```text
=== scenario 1: granted
PASS: the model read before writing
PASS: the model called au_write_file
PASS: the file on disk carries the new line
PASS: the write is an engine commit (1 new)
PASS: no permission denial

=== scenario 2: read-only, the same request
PASS: au_write_file was never callable in a read-only launch
PASS: notes/b.md is untouched
```

Two details worth keeping. The first run FAILED on "a permission denial appeared",
and the denial was `au_overview`: the model oriented itself before editing, and the
allow-list named only `au_content` and `au_write_file`. The server was right and the
script was wrong — a check that forbids orientation tests the allow-list, not the
gate. `au_overview` is now in the list, and scenario 2 confirms the opposite
property: without `--allow-mutations` the write tool is absent from `tools/list`, so
a model cannot call what the server never advertised.

## 2026-11-14 (fifth): Codex completes a turn too, and reads the skill

One client working proves the server matches that client. Two clients with different
implementations prove it matches the protocol. `scripts/codex-turn.sh` runs the same
marker task through `codex exec` (0.169.0) with its own `CODEX_HOME`, so the user's
`config.toml` is untouched and only `auth.json` is copied in.

```text
PASS: the model used au_grep
PASS: the marker travelled through the engine
PASS: the answer names the file that holds the marker
PASS: one Codex model turn completed through the adapter
```

Codex's own event log shows the same shape from the other side — one `mcp_tool_call`
carrying exactly the arguments the schema asks for, and the engine's payload returned
as the tool result:

```json
{"type":"item.completed","item":{"type":"mcp_tool_call","server":"arsumbris-rs",
 "tool":"au_grep","arguments":{"query":"unique-marker-7f3a"},
 "result":{"content":[{"type":"text","text":"{\"grep\":{\"matches\":[{...\
  "\"path\":\".../notes/a.md\",\"text\":\"find me: unique-marker-7f3a\"...}"}]}"}]}}}
```

Two things this run established that the Claude Code run could not. First, Codex
reads the installed skill before acting (`cat /root/.agents/skills/arsumbris/SKILL.md`
is the first command in its transcript), so the skill is live for more than one
client. Second, the client's own sandbox does not interfere with the adapter: Codex
spawns it as an MCP server outside the model's sandbox.

What it does not establish: the protocol revision Codex negotiates. Its `--json`
event stream does not report `protocolVersion`, so that line stays unverified rather
than guessed.

## 2026-11-14 (sixth): a model recovers from a lost reply — and found a lie in the tool

`scripts/recovery-turn.sh` puts the model in the failure `lost-reply.sh` proves
mechanically: the write goes through, the reply is dropped, and the tool answer says
the outcome is unknown.

```text
tool calls: au_members au_content au_write_file au_reconcile au_content
PASS: the model sent a write
PASS: the model asked au_reconcile what happened
PASS: the model did not retry the write
PASS: exactly one engine commit exists
PASS: the file carries the appended line
```

It did not retry — the whole point — and it reconciled instead. But the transcript
shows something the checks missed, and it is the most useful finding of the day:

```json
{"name":"mcp__arsumbris-rs__au_reconcile",
 "input":{"repo":"fixture","path":"notes/a.md","expected_hash":"null"}}
{"content":"{\"current_commit\":\"75780668...\",\"current_hash\":\"eb4d60ba...\",
  \"outcome\":\"not_applied\"}"}
```

The model did not know the digest the write would have produced — it cannot know it —
so it sent the string `"null"`. The server compared strings, found no match, and
answered **`not_applied` with full confidence, about a write that had landed**. The
model only recovered because its next move was to re-read the file.

That is a design flaw, not a model mistake: `au_reconcile` accepted any non-empty
string as a digest, and a wrong expectation produces a confident wrong answer. Fixed
in `src/lib.rs`: `expected_hash` must now be the 16-hex-character digest `au_content`
returns, and anything else is refused before the engine is contacted, with a message
naming the two real options (`expect_absent: true` for a delete, or `au_content` to
compare the text). `tests/reconcile.rs` pins six malformed values, including the
literal `"null"`.

A second run, after the fix, took a different wrong turn and got a useful answer for
it: the model called `au_reconcile` with **no** digest at all, was told that it must
give one, and then did the right thing — `au_content`, compare the text. That refusal
used to end at "give exactly one of a nonempty 'expected_hash' or 'expect_absent'",
which names the rule but not the way out, so it now says the rest: if the digest is
unknown, read the file and compare the text, because reconcile can only compare
against an expectation.

### The revision Codex actually negotiates, read off the wire

`scripts/stdio_tee.py` sits between a client and the server and records both
directions, which is the only way to answer "which MCP revision does this client
ask for" — neither side prints it. Codex 0.169.0:

```text
protocol: client asked 2025-06-18, server answered 2025-06-18
```

```json
{"dir":"c2s","line":"{\"jsonrpc\":\"2.0\",\"id\":0,\"method\":\"initialize\",
  \"params\":{\"protocolVersion\":\"2025-06-18\",...}}"}
{"dir":"s2c","line":"{\"jsonrpc\":\"2.0\",\"id\":0,\"result\":{\"protocolVersion\":\"2025-06-18\",...}}"}
```

So `2026-07-28` remains reached only by our own tests: the installed clients ask for
`2025-06-18` and the adapter answers the same revision. That is a fact about the
clients on this host, not about the server's support — and it is now measured rather
than assumed.

One trap worth recording: the tee must be registered by ABSOLUTE path. A client spawns
its MCP servers with its own working directory, so a relative `scripts/stdio_tee.py`
does not resolve, and the failure surfaces as `MCP startup failed ... initialize
response: connection closed` — which looks like a server bug and is a path bug.

## 2026-11-14 (seventh): a model picks the transactional tool when the task needs one

Two files that must change together, and `batch-turn.sh` asks for exactly that. The
failure this guards against is not a wrong answer but a *reachable wrong shape*: two
separate `au_write_file` calls, which leave half a change behind if the second fails.

```text
tool calls: au_overview au_content au_content au_batch au_content au_content
PASS: the model used au_batch
PASS: the model did not fall back to two single writes
PASS: both files carry the line
PASS: the batch made exactly one engine commit
```

It read both files first (two `au_content` calls, one per path — the gate demands it),
then sent one batch carrying each file's own digest:

```json
{"name":"mcp__arsumbris-rs__au_batch","input":{"repo":"fixture","ops":[
  {"op":"write_file","path":"notes/a.md","expected_hash":"587400e00990872a","content":"..."},
  {"op":"write_file","path":"notes/b.md","expected_hash":"3ba03c987069bc6d","content":"..."}]}}
```

Then it read both files again — verifying its own work, which the prompt did not ask
for. One engine commit for the pair, which is what "once per touched member" means
in practice.

## 2026-11-15 (eighth): a model plans a revert from the engine's own history

`rollback-turn.sh` gives the model no git: the only way to name a commit is the one
`au_content` reports, and the only way to see what a revert would do is
`au_rollback_preview`.

```text
tool calls: au_content au_rollback_preview
PASS: the model read the file
PASS: the model asked for a rollback preview
PASS: the preview used the commit au_content reported
PASS: the engine's plan names a restore
PASS: the engine reports the revert as safe
PASS: the preview made no commit
PASS: the final answer names the file the revert would touch
```

Two things this caught while being written, both worth keeping:

- The first run previewed the fixture's ROOT commit, because the scenario never made a
  write, so every path came back `delete` and there was nothing to restore. The script
  now performs one real write as a Python client before the model's turn: the turn
  tests the model, not the setup.
- "The plan names a restore" is checked in the tool RESULT, not in the transcript, for
  the same reason the marker is: the model's own arguments and prose repeat words like
  `restore`, so a whole-transcript grep proves nothing about what the engine said.
