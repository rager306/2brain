# Judgement harness (W4.1 phase 1)

Offline measurement of a `systemone` judge against 50 hand-labelled vault notes.
Three bounded questions per note: `stale_description` (noul: does the frontmatter
description still describe the body?), `type` (choice over the nine note types)
and `duplicate` (noul: is this the same claim as a named candidate?).
`labelled.json` holds each note's path, declared type, description, a 400-800
character body excerpt, the three hand labels with evidence, and the candidate
text used by the duplicate question.

## What it does not do

- It never writes to the vault and reads nothing but `labelled.json`.
- It is not in the serving path: no MCP tool, no daemon, no read budget.
- It makes no vendor call without `SUBQ_API_KEY` (or an explicit
  `JUDGEMENT_ENDPOINT` / `--endpoint`); without either it exits 3 on purpose.
- It does not prove the labels are complete: `duplicate_of: null` means none was named.

## Run

```
python3 audit.py --dry-run          # first request body + cost estimate, sends nothing
SUBQ_API_KEY=... python3 audit.py --threshold 0.7
JUDGEMENT_ENDPOINT=http://127.0.0.1:8080 python3 audit.py   # local judge, no key
```

Exit codes: `0` completed (including `--dry-run`), `2` `labelled.json` invalid,
`3` no key and no endpoint. `report.json` holds one row per question: answer,
probability, threshold, verdict, gold and matched. `--price-per-mtok` (default 0.04).

## Reading the precision table

`decided` counts answers outside the uncertainty band (noul: p >= t is yes,
p <= 1-t is no, between is `uncertain`); `uncertain` is reported separately and
never scored. `precision = correct / decided`; `errors` counts failed requests,
which stay in `report.json`. Type labels are the vault's declared `type`, so the
fixture measures recovery of the vault's classification, not a re-typing. For a
`null` duplicate label the noul compares against a deterministic different-type
note (`duplicate_candidate`): the gold answer is "no", so that row measures
false positives, not duplicate recall.
