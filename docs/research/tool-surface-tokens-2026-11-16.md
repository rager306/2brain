# Tool surface tokens: can the concise profile be the default?

Measured and decided 2026-11-16. Adapter lane only (`tb-mcp-rs`); no engine
edits, no live-vault writes. Everything here was captured from the repository
build (`cargo +1.99.0 build --offline`) speaking raw MCP stdio to
`target/debug/2brain mcp <dead-socket>`: `tools/list` and `tb_describe_tool`
never touch the engine, so a dead socket path is a faithful harness. (The
installed `/usr/local/bin/2brain` takes `mcp <vault>` and derives the socket
through `$HOME`; the repo build takes the socket path directly.)

## 1. The byte table

Read-only launch, 36 tools (what an ungranted client sees):

| profile | tools/list total | tool descriptions | input schemas | output schemas |
|---|---|---|---|---|
| `--tools full` | **59 127 bytes** | 12 620 | 21 466 | 18 933 |
| `--tools concise` | **19 404 bytes** | 3 176 | 10 783 | 0 (dropped) |

Saved: **39 723 bytes ≈ 9 931 tokens (chars/4), 67.3 %** per session, for a
flag that already shipped. The concise figure matches the earlier
measurement byte for byte (19 404); the full figure is 280 bytes above the
58 847 measured earlier the same day on a build one commit older — the
catalogue moved under us (descriptions are edited regularly); the concise
surface is stable because it keeps only first sentences.

With every grant (52 tools): 93 549 bytes full → 29 612 concise (−68.4 %).

## 2. What concise drops, exactly

Per tool, three cuts. Nothing else changes — same 36 (or 52) names, same
order, same annotations.

1. **Description → first sentence** (≤140 chars). 12 620 → 3 176 bytes.
   Two tools already fit (`tb_top_level_dirs`, `tb_type_tree`); 34 lose text.
   The deepest losses: `tb_search` (1 493 → 60), `tb_grep` (629 → 65),
   `tb_types` (703 → 75), `tb_reconcile` (567 → 63), `tb_graph_health`
   (532 → 140), `tb_content` (535 → 140), `tb_hubs` (498 → 27).
2. **Input schema: descriptions and presentation keys stripped**
   (21 466 → 10 783). Exactly two key kinds disappear, measured over every
   node of every schema: `description` × 116 (property and `$defs` docs) and
   `$schema` × 36. `title` and `examples` never existed in these schemas
   (schemars emits neither here), so nothing else was lost.
3. **Output schema dropped** (18 933 bytes). The result envelope key is
   still named by every tool description's verb and by `docs/tools.md`; the
   exact shape comes back from `tb_describe_tool`.

What survives in the concise schema, verified key by key against the full
payload: every property name, `type`, `required` (identical in all 36),
`enum`/`oneOf`/`const` inside `$defs` (e.g. `MutationOp`'s
`write_file|edit_file|delete_file`), `minLength`/`minimum`/`maximum` bounds,
all 26 `default` values (byte-identical set), `additionalProperties: false`,
`anyOf` nullability, `$ref`/`$defs`. That is the whole validation contract:
a client that fills arguments against the concise schema cannot build a call
the server would reject on shape.

**Truncation wart (pre-existing, now default-visible):** when the first
sentence is longer than 140 chars the concise description is cut mid-sentence.
Eight tools are affected: `tb_candidates` ("…and the fields that"),
`tb_children`, `tb_content`, `tb_follow` (cut inside a backtick enumeration),
`tb_graph_health`, `tb_references`, `tb_rollback_preview`,
`tb_semantic_tokens`. Cutting at a clause/word boundary instead of a raw char
count would fix this without changing the economics; not done in this change.

### Load-bearing vs decorative

Load-bearing — the dropped text changes what a careful caller does at call
time or how it reads the answer:

| where | what is lost | failure mode |
|---|---|---|
| `tb_reconcile` | "give the 16-hex digest the interrupted write should have produced, or `expect_absent`; if unknown, read with `tb_content` and compare" | schema-valid call with neither `expected_hash` nor `expect_absent` is refused at runtime by a self-explanatory error: one lost turn, no silent wrong answer (observed in the test below) |
| `tb_search` | `exact`/`min_should_match` semantics, `weak` hits, "a literal phrase is `tb_grep`'s job" | silently different ranking/interpretation; routing to `tb_grep` itself survives via `tb_grep`'s own one-liner |
| `tb_watch` | "a timeout is an ordinary answer (`timed_out`), not an error" | misread as failure; costs a turn |
| `tb_neighborhood` | "past depth 1 the engine requires an edge-kind filter" | cross-field constraint no schema can carry; loud engine error |
| `tb_follow` | `origin_path` scoping ("the file it appears in, which scopes a bare target") | silently different resolution scope |
| scope defaults (`own` vs `all`) on `tb_search`/`tb_grep`/`tb_files`/`tb_diagnostics`/`tb_hubs`/`tb_subtypes` | the prose spelling out the default breadth | answer covers a different member set than the caller assumed; the `default` value itself survives in the schema |
| `tb_types`/`tb_members`/`tb_subtypes` | "unknown member returns null, distinct from empty array" | interpretation of an empty-looking answer |

Decorative — dropped with no call-time effect: the repeated paging prose
(limits survive as `minimum`/`maximum`), "not an authorization boundary"
disclaimers, "pages are not a snapshot" caveats, and the per-tool warnings
about revealing workspace content.

Everything in the load-bearing list is one `tb_describe_tool` call away, and
that call is profile-independent — verified, not assumed:

**`tb_describe_tool` returns the FULL definition under the concise default.**
Every describe payload was captured for all 36 read-only tools and all 52
granted tools, under both profiles, and compared byte-for-byte: **zero
differences**. Each payload carries the long description, the complete input
schema with per-parameter descriptions, and the output schema. Parameter is
`name`. Cost: 0.8–3.8 kB per tool described.

## 3. Does it change behaviour? A routing-and-use test

Method: **agent test, not mechanical** — with one labelled caveat. Four judge
subagents (same model, `zai/glm-5.3-flash`, held constant across arms; two
replicates per arm) each received ONLY its arm's `tools/list` payload — the
59 127-byte full surface or the 19 404-byte concise one — plus the same 28
scenarios: 26 answerable tasks (two of them exercising the discovery
ladder), and 2 traps that no read-only tool can do ("change the status
field…", "delete the note…"). Each judge picked the tool by name and filled
arguments, once, from the surface alone. Scoring was mechanical against
ground truth: exact tool match, every required argument present with a valid
primitive/enum value, no unknown properties (the schemas reject extras).

| arm | decisions (26 answerable × 2 judges) | wrong tool | schema-invalid arguments | traps correctly refused |
|---|---|---|---|---|
| full | 52 | **0 (0 %)** | **0 (0 %)** | 2/2 |
| concise | 52 | **0 (0 %)** | **0 (0 %)** | 2/2 |

No judge in either arm ever picked a hidden mutation tool for the traps; both
answered `null`.

The one observed difference, and it is in argument SEMANTICS, not shape: on
the `tb_reconcile` task both full-arm judges supplied a plausible 16-hex
`expected_hash` (the long description explains why), and both concise judges
omitted it — schema-valid, but the server refuses the call at runtime with an
error that states the fix ("give exactly one of `expected_hash` or
`expect_absent`… if you do not know the digest, read the file with
`tb_content` and compare"). Price under the concise default: one self-curing
round trip, 2/52 concise decisions vs 0/52 full. This is the honest cost of
the default; every other task filled identically or with legal variations.

Caveats, labelled:

- **Task-design artefact (symmetric):** all four judges — both arms —
  answered the "which tool validates a value against a type? give its name"
  task with `tb_validate_value` directly instead of the discovery ladder
  (`tb_search_tools`). The arms are comparable (identical behaviour), but the
  test therefore never exercised `tb_search_tools`; scoring accepted the
  direct answer. The discovery helpers' own findability under concise is
  pinned by `tests/discovery.rs`, not by this judge panel.
- Judges ran inside the same agent harness (their system prompts carry other
  tools); contamination is identical across arms, so the comparison holds.
- 26 scenarios × 2 judges cannot resolve a difference smaller than ~4 %; a
  zero-vs-zero result supports "no degradation at this sample", not proof of
  equality.

The mechanical layer underneath (not a substitute for the agent test, but a
hard fact): the concise schema preserves the complete argument contract —
required sets, types, enums, bounds, defaults — as listed in §2, so no
schema-rejected call is newly possible.

## 4. Decision

**The concise profile is the default since this change; `--tools full` is the
opt-in.** The 67 % surface cut showed no wrong-tool or wrong-argument
difference in the agent test; the only observed regression is one
self-explanatory retry on `tb_reconcile`-style tasks, and every dropped byte
is recoverable per tool via `tb_describe_tool` (measured identical across
profiles). `tb_search_tools`/`tb_describe_tool` stay on every surface.

Changed for the flip:

- `tb-mcp-rs/src/lib.rs`: `parse_args` default `ToolProfile::Full` →
  `ToolProfile::Concise`; help text "(default concise)". Line-neutral:
  `src/lib.rs` stays at 4 564 production lines against a 4 564 budget
  (`tests/architecture.rs`) — the ratchet was not raised.
- `tests/discovery.rs`: full-profile tests pass `--tools full` explicitly;
  new `the_default_profile_is_concise` (no-flag `tools/list` byte-equals
  `--tools concise`) and `describe_returns_the_full_definition_under_the_concise_default`
  (describe byte-equal across profiles, long description and parameter
  descriptions present).
- `tests/architecture.rs`, `tests/schema_contract.rs` (the test's own local
  launch client), `tests/schema_types.rs`,
  `tests/stdio.rs`: launches that assert output schemas or descriptions now
  ask for `--tools full` explicitly, with the reason in a comment.
- `tb-mcp-rs/README.md` (Agent-facing cost: fresh byte table, the trade),
  `docs/connecting.md` (flag table defaults, measured cost),
  `docs/progressive-disclosure.md` (decision section),
  `skills/2brain/SKILL.md`, `skills/2brain/references/tool-map.md`.

**`WIRE.md` deviation, flagged:** the task named `WIRE.md`, but
`tb-engine/crates/tb-engine/WIRE.md` is the daemon-socket contract — the
engine tree is another lane's right now, and `tools/list` (adapter → client
over MCP stdio) is not covered by that document at all. The trade is therefore
stated in the adapter's own client-facing docs (README, connecting.md,
progressive-disclosure.md) as above. If a one-line cross-reference in WIRE.md
is still wanted, it is a doc-only follow-up on the engine lane.

Gates after the change (and the lesson that produced them): `cargo fmt
--check` exit 0; `cargo clippy --locked --offline --all-targets --all-features
-- -D warnings` exit 0; `cargo +1.99.0 test --offline` exit 0 — 45 binaries,
239 tests passed (the 237 before plus the two new default/describe pins).
An earlier gate read relied on `cmd | tail`, whose exit status is tail's, not
the tool's; it hid a compile error in `tests/schema_contract.rs` until a run
with a real exit code exposed it. Gate exits are captured directly, never
through a pipe.

## 5. Rollback

Runtime, no rebuild: launch with `--tools full` — every client sees the old
surface again.

Code, one flag: in `tb-mcp-rs/src/lib.rs`, `parse_args`, restore
`let mut profile = ToolProfile::Concise;` to `ToolProfile::Full;` (and the
help-text word, cosmetically). Two tests must follow: delete or invert
`discovery.rs::the_default_profile_is_concise`, and
`describe_returns_the_full_definition_under_the_concise_default` still passes
(describe is profile-independent). Nothing else is coupled to the default.
