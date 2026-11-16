---
type: tb.engine.readme::tb-engine
tldr: Six record kinds that let one agent continue the work another agent started, without re-reading the repository.
---

# Repo Overview

## What this is

A place to write down what an agent would otherwise re-derive every session: what we decided and on what evidence,
what constrains us, what we tried and what happened, and where the work stands when the session ends.

Six types, each one answerable to a question the next agent actually asks:

| type | the question it answers |
|---|---|
| `decision` | what did we choose, why, and with what evidence |
| `constraint` | what may we not do, and until when |
| `evidence` | what did we observe, and where does it come from |
| `experiment` | what did we try, and what happened |
| `handoff` | where does the work stand, and what is next |
| `error-fix` | what broke, and what fixed it |

## How to use this

Write a record when the session ends or when a fact becomes load-bearing, not when it is merely interesting. Each
record names its `source_url` or `provenance`; a record without one is an opinion, and the `status` field is what
tells the next reader whether it still holds.

A `handoff` is the one to write first: `task`, `next_step`, `decided`, `open_questions`. Everything else is
retrievable; the handoff is what an agent reads before anything else.

## How to extend this

Add a type when a question recurs, not when a case appears once. Type changes are a contract: keep the field names
stable, and write a `decision` record when you rename or remove one, so old notes can be read by their own rules.
