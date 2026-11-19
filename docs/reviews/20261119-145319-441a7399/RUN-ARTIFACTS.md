# Run artifacts — 20261119-145319-441a7399

Повторное полное внешнее review всего дерева 2brain (`ce-code-review`, full spine) относительно корневого коммита `7ee72b1`; HEAD `da676f6` + незакоммиченные правки `.gitignore`/`AGENTS.md`. Результат: 39/39 primary findings подтверждены валидатором (19 P1, 19 P2, 1 P2 anchor 75), verdict «Not ready».

## Состав каталога

- `report.md` — финальный отчёт (человекочитаемый).
- `metadata.json`, `stages.jsonl` — статус, стоимость стадий, сводка запуска.
- `synthesized-findings.json` — итоговый набор после дедупликации 60 -> 39 (18 дубликатов объединено; 1 исключён как предпочтение; #36/#41 перенесены в testing gaps).
- `validator-input.json`, `validator-input-rerun.json`, `validator-verdicts.json`, `validator-verdicts-rerun.json`, `validator-outcome.json` — валидация в два прохода: первый подтвердил #1-3 и ложно остановился по бюджету (~30% лимита); второй завершил инспекцию оставшихся 36. Итог 39/39 confirmed, покрытие 1:1.
- `raw-returns.json` — компактные возвраты 11 ревьюеров (60 кандидатов).
- `correctness.json`, `project-standards.json`, `testing.json`, `maintainability.json`, `agent-native.json`, `security.json`, `performance.json`, `api-contract.json`, `data-migration.json`, `reliability.json`, `adversarial.json` — полные артефакты ревьюеров с evidence.
- `finish-input.json` — передача фактов от orchestrator к листьям.
- `scope-signals.json` — детерминированные scope-факты (size_band large, hard_block_full).

## Не копировалось

- `full.diff` (16 MB), `fixwave.diff` (4.6 MB), `files.txt` — воспроизводимо через git (`git diff 7ee72b1f879b09bec1c06bf7a42834f2d673261f`; fixwave: `git diff c5afba6..da676f6`).
- Scratch-каталог запуска: `/tmp/compound-engineering-0/ce-code-review/20261119-145319-441a7399/`.

Report-only запуск: исправления не применялись. Все 39 находок owner `downstream-resolver`. Кросс-модельный adversarial pass не выполнялся (host family unattested, skip до egress — ничего не покидало машину); линзу покрыл локальный adversarial-ревьюер. Предыдущий ран: `docs/reviews/20261119-024022-72fef655/` (утро того же дня, base `7ee72b1`, head `c5afba6`).
