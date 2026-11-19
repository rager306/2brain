# CODING_STANDARDS.md

Rules code in this repository follows. Every change is reviewed against this
file, including by `ce-code-review`. Each rule states a defect class seen in
this repository's fix history; the source commits name the incident that
earned the rule.

## Engine state and state layout

- The name and home of an engine state directory are defined in one place. A
  rename or relocation updates every check that validates layout, and during a
  migration window a check accepts both the old and the new spelling instead
  of assuming one. (`efd67a4`, `ca4846d`, `df53e9b`)

## Asking the engine vs. scraping text

- Code that needs engine state or a count asks the engine, not a regex or text
  scan over files. An approximation by scraping has already overstated a count
  by a large multiple in this repository. (`003ee6f`, `cab17b4`)
- A metric measures exactly one defined thing. A counter that mixes two kinds
  of the same underlying unit (for example, link kinds under one isolation
  count) is wrong by construction, even when it looks plausible. (`cab17b4`)

## Ranking and search

- A quality floor must not hide results the ranker found. When a term barely
  matches, the floor gives way; the floor's behavior is part of the wire
  contract and is observable, not an internal tuning detail. (`006ac0b`)
- Text normalization (plural forms and the like) lives in the dictionary and
  costs one lookup; it does not become per-query machinery. (`c3c7ee4`)

## Wire and schema contracts

- A field that can be absent is declared nullable before a client can meet it,
  and the page that ends a pagination sequence has a test. (`c34879c`)
- Descriptions pinned by a gate — schema comments, one-line tool descriptions —
  change in the same commit as the code they describe. When a description must
  be shortened, it is cut on a clause boundary, not at a character count.
  (`6be27e5`, `fb5b973`)

## Tests

- A test that exercises the real engine stays real. When its fixture no longer
  runs, the test is fixed or deleted; a dead test that still passes is a
  defect, not a safety net. (`c5afba6`)
- A fixture reaps everything it spawned, including on the path where its trap
  cannot run. (`ea164f9`)

## Portability

- Configuration and CI code work from any checkout location. An absolute path
  that assumes one specific checkout directory is a defect; use paths relative
  to the repo or the configured location. (`5fd9d39`)

## Audit and ledgers

- An audit or ledger record identifies the actor precisely — which tool ran —
  not a generic label. (`ffe59d7`)

## Maintaining this file

A defect class enters this file only when it appeared in a review finding the
author fixed, a fix commit, a cleanup that removed it across the codebase, or
a solution record, and no automated check covers it. State the class as a
condition in a few sentences that stay true without naming volatile numbers or
symbols, and group rules by area. When a change establishes or retires a rule,
update this file in the same change.
