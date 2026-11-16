# A cross-agent handoff, run on the live vault (2026-11-16)

The Stage 4 exit criterion is "the two-agent scenario passes on the live vault, with the transcript kept as
evidence". This is that run, and it passed on two counts while failing on a third — which is why it is written down.

## The arrangement

1. **Agent A (this session)** wrote three notes into `/root/vault` through the MCP server, each carrying its source
   and a status: the `scope` default decision, the `weak`-flag negative result, and — after the finding below — a note
   that the served binary lags the repository. The engine committed each write itself.
2. **Agent B** was spawned with the vault as its working directory and one instruction: find where the 2brain
   search/ranking work stands and what the next step is. It was told **not** to read `/root/2brain`, not to ask
   questions, and to report what it could not find.
3. Nothing else was shared: no summary, no file names, no hints.

## What it found

It retrieved both notes from the vault alone, stated each decision, quoted the evidence as the vault gives it
(`scope`: the dependency README that answered an unanswerable question; `weak`: 0/3 on the absent class, and why
rarity is not relevance), and named the next step — the `context` contract — from the note that states it.

Its retrieval was not clean: `2brain search ranking` ranked four unrelated notes above both, `флаг weak` found the
right note first, and the English `weak flag` missed it entirely (the note body is Russian). A person reading the
rank list would have found the notes in the top five; a strict hit@1 test would have failed. That is the honest
measurement of a lexical engine over a bilingual corpus, and it argues for the task set rather than against the notes.

## What it found that we had wrong

**The vault described source, not production.** Agent B read `tb_describe_tool tb_search`, saw `scope` declared with
default `all`, and got the dependency README back on a default query — so the decision recorded as landed was not
observable in the served binary. It reported this as "either the change is absent from this build or the note
overstates", which is exactly right and is precisely the kind of thing the author does not notice.

Two causes, both ours:

- the live daemon runs a binary built before the change, and nothing in the vault said so;
- agent A wrote a decision as if it were in force. The rule that follows is now in the vault note: **a decision
  recorded before it is deployed is `status: proposed`, flipped to `accepted` in the same commit as the deploy.**

It also found the notes isolated — no inbound or outbound wikilinks — which violated the intake policy written the
same day. Both now link to each other and to the state note, and `tb_references` reports 3 in / 3 out.

## What this establishes

- **The handoff works without shared context.** A second agent reconstructed decisions, evidence and next step from
  the vault, and it did so in one pass.
- **The vault can catch its author.** The finding above is not something a self-review would have produced: it
  required a reader who took the notes literally and then checked them against the running system.
- **The gap that remains is a state record, not more notes.** "What is deployed, what is proposed, what is
  stale" is a property nobody writes down unless a type asks for it — which is why the project-memory template
  carries `status` and why the live vault needs the same discipline.

## Reproduce

```
HOME=/root /usr/local/bin/2brain mcp /root/vault     # agent B's only input
tb_search {"query": "флаг weak"}
tb_describe_tool tb_search                            # the declaration that gave the game away
```
