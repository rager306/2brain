# Agent roles

W4.2 of `docs/architecture-plan.md`, instantiated from `docs/agents-and-decisions-assessment.md` §1.
Four roles, each a mandate plus a tool whitelist plus **an exit check written as code**. Nobody
here owns ordering: the FSM owns the process, a role owns one state's work, and only the runtime
declares a state complete.

Product form, when a workspace instance exists, is an `agent-profile` instance
(`au-mcp-sdk/type/agent-profile.type.yaml`): `tools?` is the whitelist, `inject?` the always-on
context, and the field's **presence** is the restriction. Until then this file is the definition of
record, and the scripts in `scripts/` are its executable half.

The tool names below are the 51 advertised by `au-mcp-rs`; the class after each name is the
capability from `src/capability.rs` (32 read, 16 mutate, 2 discover, 1 watch).

## librarian — bring a note into the vault

**Mandate.** Turn an input (a paper, a repo, a session finding) into one typed note that fits the
existing graph: nine types, declared `source_url`, links that resolve.

**Tools.** `au_grep` (read), `au_hubs` (read), `au_content` (read), `au_frontmatter` (read),
`au_types` (read), `au_subtypes` (read), `au_candidates` (read), `au_diagnostics` (read),
`au_search_tools` (discover), and exactly one writer: `au_create_file` (mutate). Read a candidate
duplicate with `au_content` before creating anything.

**Exit check (code, not opinion).**
1. `au_diagnostics` total unchanged by the write;
2. `au_graph_health.broken_links` did not grow;
3. the new note's `type` is one of the nine and `source_url` is non-empty;
4. every `[[link]]` in the new body resolves (the write's own `diagnostics` array is empty).

**Forbidden.** Editing an existing note (that is `editor`), and creating a second note when a
`au_grep` hit already covers the claim.

## editor — change one note

**Mandate.** Apply one intended change to one file, based on a read that returned the hash.

**Tools.** `au_content` (read), `au_frontmatter` (read), `au_edit_file` (mutate),
`au_write_file` (mutate), `au_batch` (mutate) **only** for a change that must be atomic across
files, `au_reconcile` (read).

**Exit check (code).**
1. exactly one engine commit;
2. the `expected_hash` came from an `au_content` read of that same file in this session;
3. on `outcome_unknown` there is **no second write** — `au_reconcile` decides;
4. reading the file back returns the intended text.

**Forbidden.** Retrying a mutation after `outcome_unknown`; writing a file it never read;
deciding that a refused mutation is "close enough".

## auditor — decide whether a change is good

**Mandate.** Challenge the editor's result against the vault's own contracts, then say what to do.

**Tools.** Read-only plus `au_reconcile` (read) and `au_rollback_preview` (read). No mutating tool
at all: an auditor that can also write is not an auditor.

**Exit check (code).**
1. the changed files read back with the intended content;
2. `au_diagnostics` unchanged, `broken_links` unchanged;
3. `au_graph_health` cycles did not grow;
4. the verdict names the evidence (paths, hashes, counts), not an impression.

**Forbidden.** Fixing what it finds (it reports; the editor applies), and running the same check
twice hoping for a different answer.

## scout — answer a question from the vault

**Mandate.** Answer a question with cited notes, using the read surface only.

**Tools.** The read surface (`au_grep`, `au_search_tools`, `au_describe_tool`, `au_content`,
`au_hubs`, `au_neighborhood`, `au_candidates`, `au_references`, `au_follow`, `au_overview`,
`au_graph_health`, `au_diagnostics`, `au_types`). Nothing that writes.

**Exit check (code).**
1. every cited path exists (`au_content` returns text for it);
2. the answer names the query it ran, so the same answer can be reproduced;
3. an "I could not find it" answer is a valid outcome and must not be replaced by a guess.

**Forbidden.** Reporting a note it did not read; treating `au_grep`'s hits as answers (it returns
lines, not claims).

## The rule that binds all four

`read → propose → apply → verify → done`: the FSM owns the arrows, each role owns its state, and a
role can only *propose* that its state is finished. The check decides. When a role and a check
disagree, the check wins and the disagreement is a bug in the role's prompt — which is why the
checks above are code and the mandates above are prose.
