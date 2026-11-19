# Run artifacts — 20261119-024022-72fef655

Полное внешнее ревьюирование дерева 2brain (`ce-code-review`, depth full). База — корневой коммит `7ee72b1`; ревизия `c5afba6`. Результат: 40 подтвержденных валидатором находок (22 P1, 18 P2), verdict «Not ready».

## Состав каталога

- `report.md` — финальный отчет (человекочитаемый).
- `metadata.json`, `stages.jsonl` — статус, стоимость стадий, сводка запуска.
- `synthesized-findings.json` — итоговый набор находок после дедупликации 57 -> 40, с гидратацией доказательств.
- `validated-findings.json`, `validator-verdicts.json`, `validator-outcome.json`, `validator-input.json` — вход и вердикты независимого валидатора (40/40 confirmed).
- `raw-returns.json` — компактные возвраты 10 ревьюеров + adversarial fallback + fast-pass.
- `<persona>.json` (11 файлов) — полные артефакты ревьюеров с evidence.
- `finish-input.json` — передача фактов от orchestrator к листьям (scope, coverage, peer).

## Не копировалось

- `full.diff` (15 MB) — воспроизводимо: `git diff 7ee72b1f879b09bec1c06bf7a42834f2d673261f`.
- `files.txt` — воспроизводимо: `git ls-files`.
- Scratch-каталог запуска: `/tmp/compound-engineering-0/ce-code-review/20261119-024022-72fef655/`.

Отчет-only запуск: исправления не применялись. Все находки owner `downstream-resolver`, требуют отдельного решения.
