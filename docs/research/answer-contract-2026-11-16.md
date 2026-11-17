# The answer contract, shipped as MCP prompts and workflow resources

Studied 2026-11-16 (`arscontexta-skills-2026-11-16.md`), measured and shipped 2026-11-17.
Adapter lane only: `tb-mcp-rs`. The engine is untouched, and no tool was added.

The strongest finding of the skills study is one sentence: `tb_search` and
`tb_neighborhood` return **inputs to an answer**; nothing in the product is under
contract to **produce one**. The predecessor plugin's `ask` skill enforced a six-part
answer shape (direct answer, cited backing, implications in the reader's vocabulary,
tensions, what to explore next, sources consulted with counts) plus an honesty
standard: say when the corpus lacks coverage, and do not fabricate it.

That contract is text, and MCP has two slots for text that cost zero tool-schema
bytes: `prompts` and `resources`. This document is the record of shipping the
contract through them.

## What shipped

| surface | name | carries |
|---|---|---|
| `prompts/get` | `recall` (arg `question`) | the retrieval workflow: orient, search, widen, read whole, answer in the six-part shape, state plainly when the base does not know |
| `prompts/get` | `intake` (arg `record`) | the write workflow: find the claim first, pick the type honestly, write with a description that says why it matters, link into a family, confirm with `tb_diagnostics` |
| `prompts/get` | `handoff` (arg `state`) | the session-end workflow: decisions, evidence by path, what stays unverified, one concrete next action |
| `resources/read` | `2brain://workflow/answer-shape` | the answer contract itself |
| `resources/read` | `2brain://workflow/write-gates` | what the write gate refuses, stage by stage, and what clears each stage |

One deliberate grounding change against the study's wording: the study says "widen
with `tb_context`" and "read with `tb_read`". Neither tool exists in the 52-tool
catalogue. `recall` names the tools that do exist - `tb_neighborhood`,
`tb_references`, and `tb_follow` for widening, `tb_content` for reading - because a
prompt that names an absent tool misleads the model at runtime.

## The three prompts, verbatim

The texts below are byte-identical to the constants in `tb-mcp-rs/src/contract.rs`
(`RECALL`, `INTAKE`, `HANDOFF`); the argument is substituted at `prompts/get` time,
verified against the served bytes.

### `recall`

```
Answer this question from the workspace: {question}

Work the steps in order. Each step names the tool that does it.

1. Orient on the base before searching. tb_overview says what the workspace holds; tb_members names the members and their roots; tb_types lists the vocabulary the notes use. Skip this only if you already hold that vocabulary from earlier in the session.
2. Search before you read. tb_search ranks notes against the question's words; tb_grep answers "which notes contain this exact phrase" when the words are a quote, a name, or an id. Read the hit descriptions, not only the paths.
3. Widen when the page is ambiguous. tb_neighborhood walks the reference graph around the strongest hit; tb_references lists one file's incoming and outgoing edges; tb_follow resolves a wikilink before you trust it. One widening step is usual; if the note the answer lives in is still not named after two, say so in the answer rather than guessing.
4. Read the note the answer lives in. tb_content on it, whole, before quoting. An answer built from search excerpts alone is an answer about excerpts, not about the note.
5. Answer in the six-part shape (2brain://workflow/answer-shape): the direct answer first; backing cited by member and path, quoting only what you read; implications in the reader's own vocabulary; tensions where notes disagree, never averaged into false balance; what to explore next; and sources consulted with counts.
6. State it explicitly when the base does not know. Name the nearest note you did find and stop there. Do not fabricate coverage, and label an unverified hunch as one.

A deep answer that weaves four notes beats a shallow one that lists ten.
```

### `intake`

```
Record this into the workspace: {record}

1. Check for an existing note on the same claim before writing. tb_search with the claim's own words; tb_grep for the exact phrase; tb_frontmatter on the near hits, where a typed instance carries its claim under `type` and a plain note does not. A duplicate is not an observation: when a note already holds the claim, extend it or link to it, and say so in your answer.
2. Pick the type honestly. tb_types and tb_type give the workspace's vocabulary; tb_candidates names files that already satisfy a type's fields without claiming it. When no type fits, write a plain note and say so - do not bend the claim into the nearest-looking type.
3. Write it. tb_create_file for a new path (it refuses a taken path rather than overwriting); tb_edit_file or tb_write_file for an existing one, which first need a tb_content read of the same path and the hash it returned as expected_hash. The description says why the record matters, not that it exists. Every refusal names its stage; 2brain://workflow/write-gates lists them and what clears each.
4. Link it into a family. Address the family note and its siblings with [[wiki-links]]; tb_follow resolves a link before you write it, tb_references shows the family's edges afterwards. A note nothing links to is an orphan the graph cannot route a reader to.
5. Confirm with tb_diagnostics scoped to the changed file. An empty page is the exit condition; report a diagnostic you cannot clear instead of hiding it.
```

### `handoff`

```
Write the handoff. Where the work stands: {state}

The reader is another agent with none of this session's context, the same tool surface, and one chance to read what you write.

1. What was decided. Each decision in one line a stranger can act on.
2. What the evidence was. The member and path of every note a decision rests on, so the reader can re-open it with tb_content instead of trusting this summary.
3. What is unverified. Open questions, write-gate refusals, outcome_unknown mutations still owed a tb_reconcile, and diagnostics left standing.
4. The next action. One concrete step, with its tool, its member, and its path.

Every line that matters names a file or a tool; a line that names neither is a wish, not a handoff.
```

## The two workflow resources, verbatim

### `2brain://workflow/answer-shape`

```
# Answer shape

The tools return inputs to an answer; nothing here produces one for you. An answer to a question about this workspace carries six parts, in this order.

1. **Direct answer first.** Say what the answer is, not what you searched. "The hub of the intake family is X" - never "I searched and found three notes".
2. **Backing, cited.** Every claim names the note it came from, by member and path, and quotes only what a tb_content read actually returned.
3. **Implications in the reader's vocabulary.** Translate; a reader should not need the graph's type names to act on the answer.
4. **Tensions.** Where notes disagree, present the disagreement. Do not average two positions into a balance neither holds.
5. **What to explore next.** Departure points that exist in the base - a note, a type, a family - not a wish list.
6. **Sources consulted.** How many notes were read, how many candidates were rejected, and why the winner won.

## Honesty

When the workspace lacks coverage, say the workspace lacks coverage, name the nearest adjacent note, and stop. Do not assemble a plausible-looking answer from tangential notes. An unverified hunch is labelled as one.

Read fewer sources more deeply: a shallow answer citing ten notes is worse than a deep answer that weaves four.
```

### `2brain://workflow/write-gates`

```
# Write gates

The server is fail-closed. Without a launch grant every mutating tool is refused before the engine is contacted, and every refusal names its stage as `policy/<stage>`. No later stage runs once an earlier one denies, so the first refusal is the whole diagnosis.

## The grants, decided at launch

- `allow_mutations` - the 16 mutating tools exist at all. Without it they are absent from `tools/list` and a call is refused by launch policy.
- `allow_composition` - `tb_add_dep` and `tb_remove_dep`, on top of mutations: changing what the workspace depends on is a second trust decision.
- `allow_workspace_shape` - `tb_add_workspace_member`, `tb_remove_workspace_member`, and `tb_set_workspace_member_role`, on top of mutations: changing which members are served changes what every later read sees.
- member list - a policy file may name the members a mutation may target.

## A write to an existing file passes, in order

1. `arguments` - an empty `repo` or `path` is refused.
2. `path_scope` - the path must stay inside the member: no absolute paths, no `..`.
3. `member_grant` - "mutations are not allowed in workspace member X": this launch does not hold the grant for that member.
4. `prior_read` - "read this file with tb_content before writing it": a write is based on a read this server served, so no caller writes a file it has not seen.
5. `freshness` - "`expected_hash` does not match the hash returned by the last tb_content read": the file must not have changed since the read the write builds on.
6. `editability` - the engine answers whether it authors this member.
7. `capacity` - one mutation runs at a time.

## A create passes the same order, with absence in place of the read pair

`absence` - the path must not exist: "file already exists; use tb_write_file to change it". A create has no prior text to compare against, so the guarantee is the opposite one: the path must be free.

## After the write

Diagnostics are advisory, and tb_diagnostics scoped to the changed file is how a write is confirmed. A reply lost in flight is `outcome_unknown` and must not be retried blindly: tb_reconcile reports what actually happened. Every mutation decision is recorded to the launch's audit trail.
```

## Byte cost

Measured 2026-11-17 on a debug build of the adapter over stdio: `initialize`
(2025-06-18), then each request, against a fixture engine answering `files` (a full
100-file page) and `members`. Bytes are the raw JSON-RPC reply lines; the two
resource reads are counted as their text payloads.

| surface | bytes | who pays |
|---|---:|---|
| `tools/list`, default launch, 36 tools | 17316 | every client - **unchanged** by this work, pinned byte-for-byte by a fixture |
| `initialize` `capabilities` | +11 (`"prompts":{}`) | every client |
| `prompts/list` | 1038 | clients that ask |
| `prompts/get recall` (full reply) | 2003 | clients that ask |
| `prompts/get intake` (full reply) | 1818 | clients that ask |
| `prompts/get handoff` (full reply) | 1055 | clients that ask |
| workflow head entries in `resources/list` | 337 (of a 10874-byte first page) | clients that ask |
| `resources/read answer-shape` | 1287 | clients that ask |
| `resources/read write-gates` | 2223 | clients that ask |

The comparison the study asked for: the tool surface costs 17316 bytes on every
session before any call is made, and the study measured it at ~4.9k tokens in the
concise profile. The whole answer contract costs 1038 bytes to discover and about
7.6 KB to fetch in full - paid only by a client that wants it, and never multiplied
per session the way tool definitions are. A client that asks for nothing pays
exactly 11 bytes.

## Capability behaviour

`initialize` advertises exactly `{"prompts":{},"resources":{},"tools":{}}` - the three
capabilities the server can serve, and nothing else. The rule the implementation
follows, and the tests pin:

- A capability is advertised **because and only because the code to serve it is
  compiled in**. The prompt and workflow text is a static module (`contract.rs`);
  there is no launch flag under which the server cannot answer it, so there is no
  launch flag that hides it. (Mutations have a grant; reading a static string does
  not. `intake` describes a workflow the launch may refuse at the gate, and the
  text says so - hiding `recall` and `handoff` because writes are ungranted would
  hide guidance the launch can still use.)
- Nothing is claimed that is not served: no `prompts.listChanged` (the catalogue is
  static for the process's lifetime), no `resources.subscribe` (change notification
  is `tb_watch`'s job, as before).
- `prompts/list` and `resources/list` require `initialize`, exactly like
  `tools/list`; the inline lifecycle (`2026-07-28`) is unaffected.
- A client that ignores the two surfaces sees a byte-identical `tools/list`
  (fixture-pinned) and a capabilities object one key longer. Nothing else on its
  path changes.

## What a client that ignores them loses

Nothing breaks. The tools still return inputs; the client's model still assembles
answers. What it loses is the contract:

- the six-part answer shape, most importantly *direct answer first* and the
  sources-consulted footer with counts - the two parts the study found missing;
- the honesty standard as an explicit, servable norm: say the base lacks coverage,
  name the nearest note, stop - instead of assembling a plausible-looking pack from
  tangential notes;
- the write-gate vocabulary: a caller who hits `policy/prior_read` or
  `policy/freshness` can read *why* the gate exists and what clears each stage,
  instead of inferring it from refusal strings;
- the handoff discipline: decisions, evidence by path, what stays unverified, one
  next action - written for a reader with none of the session's context.

## Where the text lives

`src/contract.rs` is a new leaf module: the three workflow templates, the two
resource texts, and `INSTRUCTIONS` (the `initialize` instructions string - the same
genus of client-facing static text). The question the task asked - does the text
belong in the existing `resources.rs`? - checks out as **no**: `resources.rs` owns
the URI scheme, percent-encoding, and the member table for engine-backed files,
where every read is an engine round trip guarded by a hash. The contract text has
no engine, no IO, and no member; its failure mode (a missing entry) is a defect the
tests catch, not a read error. Different data source, different failure mode,
different module.

The `lib.rs` production-line ratchet moved **down**: 4556 -> 4546 (measured after
rustfmt). The feature's wiring - two handlers, the workflow branch of
`resources/read`, the workflow head of `resources/list`, `enable_prompts`, minus
the `INSTRUCTIONS` lines - cost 52 lines, paid for by moving the task-word ranking
(`search_tools`, `tokenize`) into `describe.rs`, where the one-line cut already
lived. `tests/architecture.rs` records the number, the two new module edges
(`lib -> contract`, `describe -> capability`), and `contract`'s leaf status.

## Tests

`tests/answer_contract.rs` (9 tests) pins the contract end to end:

- `initialize` advertises exactly `{prompts, resources, tools}`, and no
  `listChanged` is claimed;
- `prompts/list` names `recall`, `intake`, `handoff`, each with exactly one
  required argument;
- `prompts/get` for each prompt returns non-empty user-role messages with the
  argument substituted (no placeholder survives) and names the real tools that do
  the work;
- a missing argument and an unknown prompt are refusals that name the way out;
- `resources/read` serves both workflow texts - and the legacy `arsumbris://`
  scheme - without the engine; an unknown or bare workflow URI is refused without
  an engine round trip;
- `resources/list` carries the two workflow entries ahead of the catalogue, keeps
  the member URI shape behind them, and still hands back a cursor on a full page;
- `tools/list` bytes equal the checked-in fixture
  (`tests/fixtures/tools-list-concise.json`) captured from the pre-contract
  surface; the test names the regeneration procedure for an intentional change.

`tests/resources.rs`'s catalogue test now asserts the workflow head before the
fixture's member URIs.

## Evidence, and what was not measured

Measured on 2026-11-17, debug build of `tb-mcp-rs`, over stdio: the byte table
above, the byte-identity of `tools/list` against the pre-change binary's capture,
and the substituted prompt text checked against the template arithmetic
(len(served) = len(template) - len("{question}") + len(argument), 1717 = 1682 - 10 + 45)
to prove the document's verbatim texts are the served bytes. Gates at the time of
writing: `cargo test --offline` 256 passed across 46 binaries (was 239/45),
`clippy --all-targets --all-features -D warnings` clean, `fmt --check` clean.

Not measured, stated plainly: the live production socket and the live vault were
never touched (the fake-daemon fixture and the dead-socket discipline in the tests
are the whole exercise); the opt-in real-engine test remains opt-in and was not
run here; token equivalence of the byte counts is taken from the study's 4.9k
measurement, not re-measured. Whether clients actually *follow* the contract after
fetching it is a behavioural question this change cannot answer from bytes - the
next evidence would be an answer audit against the six parts.
