## Code Review Results

**Область:** полный внешний review отслеживаемого дерева 2brain: `tb-engine`, `tb-mcp-rs`, система типов и репозитории контента. База: `7ee72b1f879b09bec1c06bf7a42834f2d673261f`; проверенная ревизия: `c5afba6aa657a63079af6f9eb43cf852842bd25f`, ветка `main`.

**Цель:** проверить production serving path, контракты wire и типов, миграции, безопасность mutation gate, поиск и жизненный цикл daemon с использованием `codebase-memory-mcp` для навигации.

**Режим:** `full`, markdown report-only. Исправления не применялись.

**Ревьюеры:** correctness; project-standards и maintainability для стандартов и структуры всего дерева; testing для adapter и real-engine тестов; agent-native для MCP-инструментов; security для mutation gate, разрешений и socket ownership; performance для поиска, индексов и пагинации; api-contract для внешнего MCP-контракта; data-migration для окна миграции и state layout; reliability для daemon, timeout и recovery; adversarial для риска скрытого повреждения vault при ложном сообщении об успешной мутации.

**Итог:** 40 подтвержденных находок: 22 P1 и 18 P2. Основные риски сосредоточены вокруг потери данных при записи и восстановлении и обхода границ авторизации. Отчет и артефакты передаются для сохранения в `docs/reviews/20261119-024022-72fef655/`.

### P1 -- High

Для всех находок ниже статус валидатора `confirmed`, verification gate разрешен. Если не указано иное, частота возникновения соответствующего условия **не измерялась**: подтверждение относится к поведению, допускаемому кодом, а не к распространенности инцидента.

1. **#1 ` .github/workflows/ci.yml:38` -- CI не запускает real-engine тесты адаптера.** `cargo test` пропускает три `#[ignore]` теста, поэтому интеграция через Unix socket и cross-member grant могут сломаться незамеченными. Собрать актуальный `tb-cli`, установить `AU_ENGINE_BIN`, явно запускать ignored suite и обновить инструкции сборки. Ревьюер: testing; confidence 100. Частота пропущенных регрессий не измерялась.

2. **#2 `install.sh:76` -- fallback установки скрывает старое состояние.** При неудачной установке `au` создание `$HOME/.2brain/bin` заставляет resolver предпочесть новую пустую `.2brain` существующей `.arsumbris`; прежние registry и socket становятся недоступны. Устанавливать fallback вне пространства state root либо определять существующий root до создания каталога. Ревьюер: data-migration; confidence 100. Дополнительный защищенный предмет: `data-loss`.

3. **#3 `tb-engine/crates/tb-cli/src/daemon.rs:507` -- `daemon stop --force` может завершить чужой процесс.** После аварийного выхода и повторного использования PID проверяется только существование номера, после чего отправляются `SIGTERM` и `SIGKILL`; устаревший PID также мешает запуску daemon. Удерживать lifetime OS lock и перед сигналом проверять идентичность и время старта процесса; при невозможности проверки отказывать. Ревьюер: reliability; confidence 100; `protected_subject: concurrency`.

4. **#4 `tb-engine/crates/tb-engine/src/identity_migration.rs:175` -- миграция переписывает пользовательский YAML-текст.** Сканер воспринимает строку `type:` внутри literal scalar или вложенного пользовательского значения как claim и меняет пример в содержимом. Делать splice только по узлам claim, найденным парсером; покрыть literal scalar и вложенные значения. Ревьюеры: correctness, data-migration; confidence 100; `protected_subject: data-loss`.

5. **#5 `tb-engine/crates/tb-engine/src/identity_migration.rs:185` -- миграция пропускает поддерживаемый синтаксис claim.** Кавычки и flow-list могут остаться со старой идентичностью при результате `AlreadyMigrated`; удаление legacy aliases позже нарушит такие файлы. Переписывать все поддерживаемые формы по исходным spans либо явно отказывать на неподдерживаемых. Ревьюер: data-migration; confidence 100; `protected_subject: public-contract`; дополнительный предмет: `data-loss`.

6. **#6 `tb-engine/crates/tb-engine/src/serve.rs:11402` -- отклоненный non-Git batch оставляет частичные записи.** Операции выполняются последовательно, но компенсация и preimage parking исключают non-Git members; ошибка поздней операции оставляет ранние изменения и устаревший индекс. Требовать Git для multi-operation batch либо журналировать и восстанавливать все non-Git пути, включая crash recovery. Ревьюер: correctness; confidence 100; `protected_subject: data-loss`.

7. **#7 `tb-engine/crates/tb-engine/src/serve.rs:13489` -- чтение следует по symlink за пределы member.** Read-only MCP-клиент может получить UTF-8 файл, доступный пользователю daemon, через существующую ссылку наружу: проверка относительного пути не ограничивает последующий `std::fs::read`. Передавать идентичность member в запрос и открывать путь beneath-root с защитой от гонок и escaping symlinks; проверить ссылки на файлы и каталоги через MCP. Ревьюеры: adversarial, security; confidence 100; `protected_subject: injection`; дополнительный предмет: `authorization-authentication`.

8. **#8 `tb-engine/crates/tb-engine/src/serve.rs:13489` -- нечитаемый файл считается успешно удаленным.** Ошибка чтения и невалидный UTF-8 превращаются в `null`, а reconcile после потерянного ответа трактует `null` как доказательство удаления. Возвращать `null` только при подтвержденном `NotFound`, остальные ошибки передавать так, чтобы результат оставался неизвестным. Ревьюер: reliability; confidence 100; `protected_subject: public-contract`.

9. **#9 `tb-engine/crates/tb-engine/src/serve.rs:5021` -- неудачный rollback удаляет единственные preimages.** При ошибке восстановления ignored/untracked файла sentinel сохраняет ошибку, но исходные байты и их mapping удаляются; startup sweep повторяет проблему. Сохранять журнал и preimages до успешного восстановления и исключить failed recovery из sweep. Ревьюеры: adversarial, correctness; confidence 100; `protected_subject: data-loss`.

10. **#10 `tb-engine/crates/tb-engine/src/serve.rs:5204` -- ошибка non-Git записи может оставить усеченный файл.** Прямой overwrite может завершиться ошибкой после truncation, например при заполнении диска; non-Git preimage не сохраняется, хотя вызывающая сторона получает rejection. Журналировать и восстанавливать non-Git preimages либо отклонять такие мутации до появления надежной компенсации. Ревьюер: adversarial; confidence 100; `protected_subject: data-loss`.

11. **#11 `tb-engine/crates/tb-engine/src/serve.rs:5400` -- recovery пропускает legacy saga при двух identity directories.** При сосуществовании `.2brain` и `.arsumbris` обе проверки могут указывать на `.2brain`; незавершенная legacy saga остается невосстановленной, а миграция может продолжиться. Независимо проверять и восстанавливать журналы, preimages и failure sentinels в обоих каталогах. Ревьюер: data-migration; confidence 100; `protected_subject: data-loss`.

12. **#12 `tb-engine/crates/tb-engine/src/serve.rs:9795` -- rollback preview проверяет неверные пути вложенного member.** `diff-tree` выдает имена относительно корня worktree, но проверки выполняются с `git -C` внутри member, повторно добавляя его префикс. Измененный или dirty путь может получить `safe`. Выполнять проверки истории и чистоты от корня worktree и переводить результат в member-relative пути. Ревьюер: data-migration; confidence 100; `protected_subject: public-contract`.

13. **#13 `tb-mcp-rs/scripts/lib/disposable-fixture.sh:53` -- fixture ищет socket только в старом каталоге.** В свежем `HOME` engine выбирает `.2brain`, а fixture и несколько вспомогательных скриптов ищут `.arsumbris`, поэтому проверки совместимости и rehearsal не доходят до своей цели. Задать изолированный `2BRAIN_HOME` внутри fixture и искать socket там. Ревьюеры: maintainability, project-standards, testing; confidence 100. Валидатор подтвердил несовпадение путей на проверенной ревизии.

14. **#14 `tb-mcp-rs/src/bounded_stdio.rs:183` -- внутренние rejection обходят backpressure accounting.** Поток version-rejected ping при остановленном чтении stdout может навсегда заблокировать заполненный канал: локальные ответы отправляются без timed permit, а writer возвращает permits и для них. Учитывать слот каждого внутреннего ответа и ограничить время постановки в очередь через `outbound_wait`. Ревьюеры: correctness, reliability; confidence 100; `protected_subject: concurrency`.

15. **#15 `tb-mcp-rs/src/lib.rs:1670` -- preflight может навсегда занять mutation slot.** Если engine принимает соединение, но не отвечает, untimed preflight чтение удерживает единственный permit, и последующие мутации отклоняются до перезапуска. Применить существующий deadline ко всем preflight чтениям и освобождать permit при timeout. Ревьюер: reliability; confidence 100; `protected_subject: concurrency`.

16. **#16 `tb-mcp-rs/src/lib.rs:1932` -- refactor cascade пишет за пределами member grant.** Разрешенный rename в member A может переписать и закоммитить backlinks в editable member B, исключенном из allowlist: engine проверяет editability, но не получает эту политику. Передавать allowlist для `rename`, `rename_block_id`, `inline` и `promote` и проверять все members фактического saga plan. Ревьюеры: adversarial, security; confidence 100; `protected_subject: authorization-authentication`. Engine-тест `mutate.rs:4935-4997` демонстрирует сам cross-member cascade.

17. **#17 `tb-mcp-rs/src/lib.rs:2442` -- `tb_candidates` по умолчанию возвращает неограниченный результат всех типов.** Paging существует, но не включен по умолчанию, а фильтра по типу нет; на production vault документирован ответ 3.1 MB в `both` mode. Ввести ограниченную страницу и легкий summary по умолчанию, continuation metadata и engine-side фильтр типа с адаптерной схемой и тестами. Ревьюеры: agent-native, api-contract; confidence 100. Валидатор подтвердил default и документированный размер.

18. **#18 `tb-mcp-rs/src/lib.rs:2793` -- reconcile может объявить выполняющуюся мутацию непримененной.** Снимок текущего содержимого не доказывает исторический или окончательный исход: исходная операция еще может завершиться, а retry по новому hash может перезаписать другого автора. Использовать устойчивый известный клиенту mutation ID и конечный статус engine; иначе возвращать `unknown` и не разрешать retry только на основании текущего hash. Ревьюеры: adversarial, correctness, data-migration, reliability; confidence 100; `protected_subject: data-loss`.

19. **#19 `tb-mcp-rs/src/lib.rs:4100` -- structured-only чтение не открывает mutation gate.** В `--payload structured` `tb_content` возвращает hash без text block, но `served_hash` читает только text; последующие read-gated мутации отклоняются как без предварительного чтения. Извлекать hash сначала из `structured_content`, затем из text; проверить read-then-mutate во всех трех режимах. Ревьюеры: agent-native, api-contract; confidence 100; `protected_subject: public-contract`.

20. **#20 `tb-mcp-rs/src/lib.rs:4320` -- content window выдает непригодный `next_offset`.** Схема не принимает offset и каждый запрос начинает с нуля; кроме того, полное содержимое передается до обрезки и может превысить предел frame 16 MiB. Реализовать символьные offset и ограниченные окна на engine wire boundary, сохранив hash всего файла и точный continuation; проверить Unicode и файлы больше frame limit. Ревьюеры: agent-native, api-contract, performance; confidence 100; `protected_subject: public-contract`.

21. **#21 `tb-mcp-rs/src/wire.rs:809` -- вложенный member обходит mutation allowlist.** Клиент с доступом к parent может указать путь во вложенный editable, но не разрешенный member: адаптер проверяет переданное имя parent, а engine выбирает фактического глубокого владельца без allowlist. Передавать политику для каждой мутации и проверять фактического владельца каждой операции, включая batch. Ревьюеры: adversarial, security; confidence 100; `protected_subject: authorization-authentication`.

22. **#22 `tb-engine/crates/tb-engine/src/serve.rs:5633` -- успешная мутация может откатиться после перезапуска.** Ошибка удаления intent marker игнорируется после успешных commits; следующий recovery принимает оставшийся marker за прерванную saga и откатывает уже подтвержденную запись. До подтверждения сохранять завершенное состояние и не удалять preimages, пока на них ссылается действующий marker. Ревьюер: adversarial; confidence 75; `protected_subject: data-loss`.

### P2 -- Moderate

Статус валидатора для всех находок также `confirmed`, verification gate разрешен. Частота возникновения не измерялась, кроме отдельно отмеченных подтверждений.

23. **#23 `tb-engine/crates/tb-cli/src/migrate.rs:127` -- `--all` может молча пропустить registry.** Ошибки чтения и разбора превращаются в пустой список, после чего возможен fallback на текущий каталог с сообщением об успехе. Завершать миграцию ошибкой при нечитаемом или некорректном существующем registry и не подменять пустой набор текущим каталогом. Ревьюер: data-migration; confidence 100; `protected_subject: public-contract`.

24. **#24 `tb-engine/crates/tb-engine/src/engine_schema.rs:258` -- ложное предупреждение для пользовательского namespace.** Поддерживаемый `au.engine.custom` помечается устаревшим, хотя canonicalization и миграция его не меняют. Выдавать предупреждение только если canonicalization действительно меняет claim. Ревьюер: data-migration; confidence 100; `protected_subject: public-contract`.

25. **#25 `tb-engine/crates/tb-engine/src/parse.rs:254` -- диагностика пропускает legacy claims в определениях типов.** `extends`, `sealed` и required-meta канонизируются без legacy hints, поэтому отсутствие предупреждений не доказывает готовность удалить совместимость. Выдавать диагностику по исходным claims до canonicalization. Ревьюер: data-migration; confidence 100; `protected_subject: public-contract`.

26. **#26 `tb-engine/crates/tb-engine/src/search.rs:1256` -- удаленный термин блокирует plural fallback.** Пустая сохраненная запись словаря считается успешным совпадением и мешает найти живую singular/plural форму после инкрементального редактирования. Игнорировать записи без postings при разрешении всех форм и проверить соответствие свежему индексу. Ревьюер: performance; confidence 100.

27. **#27 `tb-engine/crates/tb-engine/src/search.rs:1259` -- формы слов генерируются на каждом dictionary miss.** На поисковом hot path создаются строки и выполняются дополнительные lookup вместо оговоренного одного обращения к словарю. Поддерживать aliases при обновлении индекса, сохранив приоритет точного совпадения и порядок fallback. Ревьюер: project-standards; confidence 100. Влияние на производительность не измерялось.

28. **#28 `tb-engine/crates/tb-engine/src/serve.rs:11497` -- составной batch постоянно сообщает `reflected=false`.** Для двух изменений одного пути catalog не может одновременно содержать два промежуточных hash, поэтому клиент может ждать обновления бесконечно. Сравнивать только конечный hash или отсутствие каждого пути. Ревьюер: correctness; confidence 100; `protected_subject: public-contract`.

29. **#29 `tb-engine/crates/tb-engine/src/serve.rs:9770` -- rollback preview merge commit ложно сообщает `safe`.** Используемый `diff-tree` не возвращает пути merge, хотя revert требует mainline parent и может изменить файлы. Отклонять merge commits либо требовать явный mainline и строить preview относительно него. Ревьюер: data-migration; confidence 100; `protected_subject: public-contract`.

30. **#30 `tb-mcp-rs/scripts/client-matrix.sh:12` -- проверка собирает и ищет бинарник в разных каталогах.** Cargo использует общий `.cargo-target`, но скрипт после сборки продолжает искать `target/debug`; другие fixture scripts также используют старые per-crate пути. Вычислять defaults для `au` и `2brain` из общего target каталога. Ревьюер: maintainability; confidence 100.

31. **#31 `tb-mcp-rs/scripts/install.sh:25` -- installer зависит от неотслеживаемой target symlink.** На чистом checkout сборка кладет бинарник в `.cargo-target`, а установка ищет его в `tb-mcp-rs/target`. Определять `target_directory` через `cargo metadata`, как корневой installer. Ревьюер: reliability; confidence 100. Валидатор подтвердил отсутствие отслеживаемой symlink на проверенной ревизии.

32. **#32 `tb-mcp-rs/scripts/lib/disposable-fixture.sh:80` -- осиротевшие fixture daemons не убираются.** Если runner погиб без EXIT trap, живой daemon и каталог остаются, а следующий запуск пропускает daemon именно из-за существования каталога. Записывать владельца fixture и завершать daemon после смерти runner, затем удалять stale root. Ревьюер: project-standards; confidence 100.

33. **#33 `tb-mcp-rs/scripts/strict-client.py:30` -- проверка привязана к фиксированному checkout.** Без `AU_ENGINE_BIN` скрипт вне `/root/2brain` запускает жестко заданный путь к engine. Вычислять default относительно репозитория, сохранив override. Ревьюер: project-standards; confidence 100.

34. **#34 `tb-mcp-rs/src/cli.rs:479` -- adoption starter переименовывает посторонние members.** Глобальная замена `sample` затрагивает и `sample-extra`, повреждая созданный workspace manifest. Разбирать YAML и заменять только полный scalar собственного имени в edit list; проверить общие префиксы и кавычки. Ревьюер: maintainability; confidence 100; `protected_subject: data-loss`.

35. **#35 `tb-mcp-rs/src/lib.rs:2411` -- `tb_instances` пагинируется после слишком большого engine response.** Даже `limit=1` сначала запрашивает все instances и не сработает, если engine frame превысит 16 MiB. Перенести `limit` и `offset` в engine, возвращая count и continuation без полной сериализации. Документированные 18.9 MB относятся к MCP-ответу в `both` mode, а не к измеренному размеру engine frame; частота превышения frame threshold не измерена. Ревьюер: agent-native; confidence 100; `protected_subject: public-contract`.

36. **#36 `tb-mcp-rs/src/lib.rs:3206` -- file-addressed read занимает два permits.** Вложенный `self.read` получает второй permit, пока первый удерживается; лимит восьми может исчерпаться раньше восьми реальных вызовов. Использовать первый permit на всю операцию и добавить concurrent test с восемью чтениями. Ревьюер: performance; confidence 100; `protected_subject: concurrency`.

37. **#37 `tb-mcp-rs/src/lib.rs:4135` -- full schema несовместима с text-only результатом.** `--tools full --payload text` объявляет `outputSchema`, но не возвращает `structuredContent`; документирован реальный отказ schema-validating клиента. Убирать output schema в text mode либо отклонять сочетание, задокументировать и проверить строгим клиентом. Ревьюер: agent-native; confidence 100; `protected_subject: public-contract`.

38. **#38 `tb-mcp-rs/tests/real_engine.rs:265` -- search fixture не снимается с настоящего engine.** Fixture утверждает, что bodyless note найден, хотя индекс исключает такие заметки; список capture cases не вызывает `tb_search`, поэтому schema tests могут принимать невозможный результат. Добавить capture `tb_search`, переснять fixture и сопоставлять имена всех committed fixtures с capture cases. Ревьюер: testing; confidence 100. Валидатор подтвердил расхождение fixture и кода индекса.

39. **#39 `tb-mcp-rs/tests/real_engine.rs:385` -- real-engine тесты наследуют внешний state root.** При заданном `2BRAIN_HOME` daemon использует внешний каталог, тогда как тест ищет socket только во временном `HOME`; возможны timeout и оставшиеся внешние pid/socket файлы. Удалять override или явно задавать его внутри fixture для всех дочерних процессов. Ревьюеры: project-standards, testing; confidence 100.

40. **#40 `tools/kb/census.py:67` -- census выдает результаты regex за факты графа.** Inline topic arrays могут не учитываться, а ссылки в fenced code могут завышать счетчик; метрики outbound links и заметок без них становятся неверными. Получать graph metrics из engine references, отдельно обозначая синтаксические подсчеты файлов. Ревьюер: project-standards; confidence 100; дополнительный предмет: `public-contract`.

### Actionable Findings

Все 40 находок направлены `downstream-resolver`; для каждой есть `suggested_fix`. Подробности, последствия и доказательства приведены выше. `manual` требует ручного решения или реализации; `gated_auto` допускает автоматизированное исправление с последующей проверкой. Подтверждение validator не является разрешением на применение.

Финальных `triage_groups` в синтезированном артефакте нет; группы не добавлялись при представлении отчета. Начинать следует с границ доступа #7, #16, #21 и сохранности данных #6, #9, #10, #18, #22.

### Coverage

- Область: `standalone base:whole-tree`; база -- корневой commit `7ee72b1`, рассматриваемое изменение -- все дерево: 2641 отслеживаемый файл, 358,927 измененных строк, 149,373 исполняемые строки вне тестов, `size_band large`, `hard_block_full`.
- `REVIEW.md` и `CODING_STANDARDS.md` в рабочем дереве -- новые неотслеживаемые файлы, созданные `ce-setup` за несколько минут до review. На базе `REVIEW.md` отсутствует, поэтому `review_policy` не была подготовлена и собственная версия policy изменения **не применялась** на Stage 1d.
- Из области исключены неотслеживаемые `.compound-engineering/config.example.yaml`, `.compound-engineering/config.yaml`, `CODING_STANDARDS.md`, `REVIEW.md`.
- `full.diff` содержит все дерево относительно корневого commit и расположен в `run_dir/full.diff`; ревьюеры использовали `files.txt` и непосредственно рабочее дерево, допустимое для локальной области.
- Хост `omp` не предоставляет override модели или effort для отдельных запусков: все ревьюеры унаследовали модель сессии; конфигурации `review_subagents` нет, настроенный effort не применялся.
- План не найден: `docs/plans` отсутствует, `plan_source: none`, `requirements_completeness: null`.
- Cross-model pass не проводился: семейство модели, обслуживающей хост, не аттестовано, поскольку файл сессии oh-my-pi не называет first-party модель этого runtime. Worker был остановлен до отправки данных, поэтому проверяемое содержимое не покидало машину. Adversarial lens выполнен как `in-process-fallback`; `cross_model_route codex` не отправлялся, `model_requested` и `effort_requested` не проверены, receipt отсутствует.
- Поля peer receipt неприменимы, поскольку запрос peer не отправлялся. Аттестации независимости in-process fallback нет; его находкам не присваивался бонус за независимое подтверждение.
- Предварительная fast-pass находка P1 о якобы отсутствующей пагинации и фильтре `tb_candidates` частично заменена проверенными находками persona: agent-native P1 `tb-mcp-rs/src/lib.rs:2442` и api-contract P2 `tb-engine/crates/tb-engine/src/serve.rs:2460`. Server-side paging через `summary`, `limit`, `offset` существует; дефект -- неограниченный default и отсутствие фильтра типа. Fast-pass ограничен anchor 50 и дедуплицирован с ними.
- Метки `pre_existing` исправлены: ревьюерам было сказано «clean tree -> pre_existing true», но при базе root commit рассматривается все дерево. Все 40 сохраненных находок переведены в `pre_existing: false` и primary до validation.
- Синтез: 57 исходных заявлений сведены к 40 primary findings; объединено 12 групп дубликатов без повышения confidence по serving family. `pre_existing` находок: 0.
- Confidence-suppressed: 0, включая anchor 50 и 25; mode-aware demotions: 0. Policy не удаляла и не переклассифицировала находки. Некорректных returns и findings: 0; first-evidence backfills: 0. Один некорректный символ кавычки в компактном ответе исправлен по исходному артефакту.
- Validation: один batch из 40 находок; `confirmed: 40`, `rejected: 0`, `unresolved: 0`, `malformed: 0`, `failed: 0`, `shortcut-skipped: 0`. Независимого cross-model peer для shortcut не было. Исходные причины verdict и назначенные validator `protected_subject` сохранены в `validated-findings.json`; дополнительные классификации защищенных предметов отражены в записях выше и в этом артефакте.
- Отдельных residual risks и testing gaps от persona не поступило. Сам отчет содержит конкретные пробелы тестирования, включая #1, #13, #38 и #39. Ни одна находка не была исправлена в этом report-only запуске.
- Временные артефакты запуска: `/tmp/compound-engineering-0/ce-code-review/20261119-024022-72fef655/`. Предусмотренный путь отчета и копий артефактов в репозитории: `docs/reviews/20261119-024022-72fef655/report.md` и соседние файлы; их сохранение выполняет orchestrator.

---

### Verdict

> **Not ready.** Подтверждены 22 открытые P1: в том числе возможные потеря данных, ложный результат recovery и обходы границ доступа. Сначала закрыть #7, #16 и #21, а также согласовать и исправить атомарность записи, recovery и reconciliation в #6, #9, #10, #18 и #22. Затем закрыть остальные P1 и восстановить real-engine проверки в CI. Частота большинства описанных условий не измерялась.

### Actionable recap

Для всех пунктов: `suggested_fix` присутствует, owner `downstream-resolver`; confidence 100, кроме #22 с confidence 75.

- **#7 P1** `tb-engine/crates/tb-engine/src/serve.rs:13489` -- ограничить чтение symlink пределами member; `manual`.
- **#16 P1** `tb-mcp-rs/src/lib.rs:1932` -- применять allowlist ко всем участникам refactor cascade; `manual`.
- **#21 P1** `tb-mcp-rs/src/wire.rs:809` -- проверять allowlist фактического владельца вложенного пути; `manual`.
- **#6 P1** `tb-engine/crates/tb-engine/src/serve.rs:11402` -- обеспечить атомарность non-Git batch; `manual`.
- **#9 P1** `tb-engine/crates/tb-engine/src/serve.rs:5021` -- сохранять preimages после ошибки rollback; `manual`.
- **#10 P1** `tb-engine/crates/tb-engine/src/serve.rs:5204` -- восстанавливать non-Git файл после сбоя записи; `manual`.
- **#18 P1** `tb-mcp-rs/src/lib.rs:2793` -- определять итог мутации по устойчивому ID, а не снимку содержимого; `manual`.
- **#22 P1** `tb-engine/crates/tb-engine/src/serve.rs:5633` -- фиксировать завершение saga до подтверждения; `manual`.
- **#2 P1** `install.sh:76` -- не скрывать legacy state при fallback установке; `gated_auto`.
- **#3 P1** `tb-engine/crates/tb-cli/src/daemon.rs:507` -- проверять владельца PID перед сигналом; `manual`.
- **#4 P1** `tb-engine/crates/tb-engine/src/identity_migration.rs:175` -- не переписывать YAML literal text; `manual`.
- **#5 P1** `tb-engine/crates/tb-engine/src/identity_migration.rs:185` -- мигрировать либо явно отклонять все поддерживаемые формы claim; `manual`.
- **#8 P1** `tb-engine/crates/tb-engine/src/serve.rs:13489` -- отличать `NotFound` от ошибок чтения при reconcile; `manual`.
- **#11 P1** `tb-engine/crates/tb-engine/src/serve.rs:5400` -- восстанавливать saga из обоих identity directories; `manual`.
- **#12 P1** `tb-engine/crates/tb-engine/src/serve.rs:9795` -- проверять rollback paths от корня worktree; `manual`.
- **#13 P1** `tb-mcp-rs/scripts/lib/disposable-fixture.sh:53` -- согласовать fixture socket и state root; `manual`.
- **#14 P1** `tb-mcp-rs/src/bounded_stdio.rs:183` -- включить внутренние rejection в ограниченный backpressure; `gated_auto`.
- **#15 P1** `tb-mcp-rs/src/lib.rs:1670` -- ограничить deadline всех mutation preflight reads; `gated_auto`.
- **#17 P1** `tb-mcp-rs/src/lib.rs:2442` -- ограничить default candidates и добавить фильтр типа; `manual`.
- **#19 P1** `tb-mcp-rs/src/lib.rs:4100` -- записывать hash structured-only чтения в ledger; `gated_auto`.
- **#20 P1** `tb-mcp-rs/src/lib.rs:4320` -- реализовать продолжение bounded content window на engine boundary; `manual`.
- **#1 P1** `.github/workflows/ci.yml:38` -- запускать real-engine adapter suite в CI; `gated_auto`.
- **#23 P2** `tb-engine/crates/tb-cli/src/migrate.rs:127` -- отказывать при ошибке registry для `--all`; `manual`.
- **#24 P2** `tb-engine/crates/tb-engine/src/engine_schema.rs:258` -- не предупреждать о неизменяемом custom claim; `gated_auto`.
- **#25 P2** `tb-engine/crates/tb-engine/src/parse.rs:254` -- диагностировать legacy claims в type definitions; `gated_auto`.
- **#26 P2** `tb-engine/crates/tb-engine/src/search.rs:1256` -- пропускать словарные записи без postings; `gated_auto`.
- **#27 P2** `tb-engine/crates/tb-engine/src/search.rs:1259` -- перенести word-form aliases в индекс; `manual`.
- **#28 P2** `tb-engine/crates/tb-engine/src/serve.rs:11497` -- определять `reflected` по конечному состоянию пути; `gated_auto`.
- **#29 P2** `tb-engine/crates/tb-engine/src/serve.rs:9770` -- отклонять merge preview либо требовать mainline; `gated_auto`.
- **#30 P2** `tb-mcp-rs/scripts/client-matrix.sh:12` -- искать бинарники в общем Cargo target; `gated_auto`.
- **#31 P2** `tb-mcp-rs/scripts/install.sh:25` -- получать target directory через Cargo metadata; `gated_auto`.
- **#32 P2** `tb-mcp-rs/scripts/lib/disposable-fixture.sh:80` -- убирать осиротевшие fixture daemons; `manual`.
- **#33 P2** `tb-mcp-rs/scripts/strict-client.py:30` -- убрать абсолютный checkout path; `gated_auto`.
- **#34 P2** `tb-mcp-rs/src/cli.rs:479` -- заменять только точный self-name в YAML; `manual`.
- **#35 P2** `tb-mcp-rs/src/lib.rs:2411` -- перенести `tb_instances` paging в engine; `manual`.
- **#36 P2** `tb-mcp-rs/src/lib.rs:3206` -- использовать один read permit на операцию; `gated_auto`.
- **#37 P2** `tb-mcp-rs/src/lib.rs:4135` -- согласовать output schema с text payload; `gated_auto`.
- **#38 P2** `tb-mcp-rs/tests/real_engine.rs:265` -- переснять search fixture настоящим engine; `gated_auto`.
- **#39 P2** `tb-mcp-rs/tests/real_engine.rs:385` -- изолировать тесты от внешнего `2BRAIN_HOME`; `gated_auto`.
- **#40 P2** `tools/kb/census.py:67` -- получать outbound graph counts из engine; `manual`.