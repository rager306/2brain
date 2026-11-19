## Code Review Results

**Область:** повторное review всего отслеживаемого дерева относительно корневого коммита `7ee72b1f879b09bec1c06bf7a42834f2d673261f`; ветка `main`, HEAD `da676f62b4ff0aab53808ff4940a4e89da643f95`, включая незакоммиченные изменения `.gitignore` и `AGENTS.md`. В сравнении 2 668 файлов.

**Цель:** проверить engine, MCP-адаптер, миграцию и инструменты после предыдущей волны исправлений и переименования. Формулировка цели выведена из истории изменений.

**Режим:** markdown report-only; исправления не применялись.

**Ревьюеры:** correctness; project-standards для применимых стандартов; testing для CI и real-engine тестов; maintainability для большого изменения и переименования; agent-native и api-contract для MCP; security для grants и containment; performance для поиска и payload; data-migration для переноса состояния; reliability для daemon и recovery; adversarial для возможной потери данных и обхода авторизации.

### Triage Groups

|Группа|Findings|Тип и порядок|Почему|
|---|---|---|---|
|Lifecycle|#1, #11, #13, #22|Decision-gate: определить проверку экземпляра процесса, затем исправить сигнализацию и ограничения ожидания|Нельзя завершать чужой процесс или оставлять интерфейс заблокированным|
|Миграция и состояние|#2, #3, #9, #23, #24, #38, #40|Decision-gate: сначала восстановить старые журналы, затем переносить состояние и проверять inventory|Незавершённую saga нельзя терять при cutover|
|Откат мутаций|#4, #8, #10, #19, #26|Decision-gate: сохранить данные восстановления и определить исход компенсации, затем закрыть non-git atomicity|Ошибки могут оставить частичные изменения или уничтожить резервную копию|
|Авторизация и пути|#5, #6, #17, #20, #29|Apply-queue: привязать проверку прав к фактически открытому пути и владельцу|Границы должны соблюдаться при чтении, создании и записи|
|Reconcile и read gate|#12, #15, #16, #27|Decision-gate: определить достоверный исход операции, затем исправить разрешение повторной записи|Снимок хеша не устанавливает историю операции или видимость содержимого|
|MCP payload|#14, #18, #32, #33, #34, #35|Decision-gate: согласовать bounded paging и continuation, затем схемы и режимы ответа|Клиентам нужны ограниченные и соответствующие контракту ответы|
|Поиск|#25, #28|Apply-queue: исправить разрешение живых терминов, затем построение excerpts|Результаты и контекст совпадений должны быть согласованы|

### P1 -- High

Все пункты ниже имеют `validation_status: confirmed`. Кроме отдельно отмеченного измерения в #14, validator подтвердил механизм по коду, но **частоту возникновения не измерял**. Указанные условия существенны: подтверждение не означает, что дефект уже наблюдался в рабочем vault.

1. **`tb-engine/crates/tb-cli/src/daemon.rs:510` -- `stop --force` может завершить чужой процесс.** После аварии PID-файл остаётся, а повторно выданный PID проверяется только на существование. Нужны lifetime lock и проверка идентичности экземпляра перед каждым сигналом. Ревьюеры: adversarial, reliability; confidence 100.

2. **`tb-engine/crates/tb-cli/src/migrate.rs:70` -- нет рабочего перехода для старых vault.** Команда из runbook не переносит старую identity, а новый parser больше её не читает; документированный вызов также не соответствует `--root`. Нужна поддерживаемая offline-миграция identity и parser-распознанных claims с preflight и rollback либо восстановление команды. Ревьюер: data-migration; confidence 100.

3. **`tb-engine/crates/tb-engine/src/repo.rs:982` -- после переименования пропадают registries устройства.** Новый код ищет `engine/config`, тогда как прежняя установка писала в `au-engine/config`; aliases и зарегистрированные репозитории становятся недоступны. Нужен offline-перенос с проверкой конфликтов и сохранением уже существующего нового состояния. Ревьюер: data-migration; confidence 100.

4. **`tb-engine/crates/tb-engine/src/serve.rs:11520` -- batch не атомарен для non-git members.** Если вторая операция падает, первая может остаться на диске: компенсация и preimages ограничены git members. Запретить такие batch до записи либо журналировать и восстанавливать все non-git пути; добавить тест второй неуспешной операции. Ревьюер: correctness; confidence 100.

5. **`tb-engine/crates/tb-engine/src/serve.rs:13650` -- создание через symlink может выйти за пределы member.** Для ещё не существующего файла canonicalize всего пути не проходит, после чего создание следует за symlink родителя. Проверять существующие предки и обеспечивать containment при самом открытии файла. Ревьюер: security; confidence 100.

6. **`tb-engine/crates/tb-engine/src/serve.rs:15438` -- `tb_grep` раскрывает внешние symlink targets.** Walker каталогизирует ссылки за пределы member, а grep читает их без защиты, действующей для `tb_content`. Исключить либо безопасно разрешать такие ссылки при построении каталога и проверять containment при чтении. Ревьюер: security; confidence 100.

8. **`tb-engine/crates/tb-engine/src/serve.rs:5087` -- неудачный rollback удаляет единственную копию ignored-файла.** При ошибке восстановления parked preimage удаляется, как и при startup sweep после ошибки recovery. Сохранять marker и preimages до успешного восстановления и исключать неудавшиеся saga из sweep. Ревьюеры: adversarial, correctness, reliability; confidence 100.

9. **`tb-engine/crates/tb-engine/src/serve.rs:5645` -- recovery не видит журналы старого runtime.** Прерванная до переименования saga находится в `au-engine/run`, но startup проверяет только `engine/run` и может принять частичное состояние. До миграции и обслуживания восстановить старые и новые журналы либо явно отказать в cutover. Ревьюеры: adversarial, data-migration; confidence 100.

10. **`tb-engine/crates/tb-engine/src/serve.rs:5739` -- подтверждённый commit может откатиться при следующем запуске.** Ошибка удаления marker игнорируется; recovery считает оставшийся marker признаком прерванной операции. До ответа об успехе сохранять устойчивое состояние завершения и проверять удаление marker и durability каталога. Ревьюер: adversarial; confidence 100.

11. **`tb-mcp-rs/src/bounded_stdio.rs:183` -- внутренние rejection обходят backpressure.** Они попадают в очередь без permit, но writer возвращает permit для каждого ответа. Клиент может раздуть число разрешений и накопить задачи; внутренние ответы должны использовать тот же учёт и timeout. Ревьюеры: adversarial, api-contract, performance, reliability; confidence 100.

12. **`tb-mcp-rs/src/lib.rs:1201` -- неполное чтение разрешает полную перезапись.** Хеш записывается до усечения `tb_content`; hash-only reconcile также повторно открывает write gate. Клиент может затереть невиденное содержимое. Разрешать замену только после полного чтения и требовать новое чтение изменённого файла. Ревьюер: agent-native; confidence 100.

13. **`tb-mcp-rs/src/lib.rs:1687` -- зависший preflight удерживает единственный mutation slot.** При неотвечающем engine предварительное чтение `rename_type` не имеет timeout, поэтому последующие мутации блокируются. Применить существующий timeout чтения к lookup и grant preview. Ревьюер: reliability; confidence 100.

14. **`tb-mcp-rs/src/lib.rs:2457` -- `tb_candidates` по умолчанию возвращает весь набор типов.** На измеренном workspace ответ составил 3,1 MB и может исчерпать контекст агента. По умолчанию возвращать ограниченную summary-страницу с continuation и добавить engine-side фильтр имени типа. Ревьюер: agent-native; confidence 100.

15. **`tb-mcp-rs/src/lib.rs:2794` -- reconcile принимает последующее изменение за неуспешную мутацию.** Если неизвестная операция успела выполниться, а затем файл изменил другой writer, несовпадение текущего хеша даёт `not_applied`; повтор может затереть новые данные. Без доказательства истории возвращать неопределённый исход. Частота такого порядка операций не измерялась. Ревьюер: agent-native; confidence 100.

16. **`tb-mcp-rs/src/lib.rs:2806` -- reconcile может завершиться раньше самой мутации.** Timeout клиента не отменяет запрос, ожидающий write lock; независимое чтение видит старые данные и сообщает `not_applied`, хотя операция ещё может выполниться. Использовать engine-side mutation ID или сериализованный barrier и сохранять unknown до установления исхода. Ревьюеры: adversarial, reliability; confidence 100.

17. **`tb-mcp-rs/src/lib.rs:2979` -- создание через parent обходит grant вложенного member.** `prepare_create` не проверяет владельца разрешённого пути так, как это делает `arm_file`; engine может записать в незаданный клиенту nested member. Проверять grant фактического владельца до отправки create. Ревьюеры: adversarial, security; confidence 100.

18. **`tb-mcp-rs/src/lib.rs:4330` -- continuation `tb_content` невозможно использовать.** Ответ содержит `next_offset`, но аргумент offset отсутствует и неизвестные поля отклоняются. Добавить символьный offset и тест последовательных окон, Unicode и полного хеша. Ревьюеры: agent-native, api-contract; confidence 100.

19. **`tb-mcp-rs/src/transport.rs:200` -- неполный rollback выглядит как чистый rejection.** Engine передаёт предупреждение только текстом, adapter оставляет ledger активным, хотя часть изменений могла сохраниться. Передавать машинно читаемый исход и mutation ID, классифицировать как `outcome_unknown` и инвалидировать затронутый ledger. Ревьюер: api-contract; confidence 100.

20. **`tb-mcp-rs/src/wire.rs:866` -- symlink alias обходит grant фактического member.** В non-git parent лексический путь может быть авторизован как parent, а запись последует за ссылкой во вложенный незаданный child. Запретить symlink в mutation target либо авторизовать фактически открываемый target внутри engine. Ревьюер: security; confidence 100.

### P2 -- Moderate

Все пункты ниже имеют `validation_status: confirmed`. Validator не измерял частоту перечисленных условий, кроме отмеченного клиентского свидетельства в #34. В #33 измерен MCP-ответ, а не размер engine frame.

21. **`install.sh:79` -- fallback установки падает с `bad substitution`.** `${2BRAIN_HOME:-...}` недопустим в Bash; adapter уже установлен, а fallback engine нет. Получать необычный ключ окружения безопасным способом. Ревьюеры: adversarial, correctness, data-migration, reliability; confidence 100.

22. **`tb-engine/crates/tb-cli/src/daemon.rs:513` -- force stop сообщает успех без подтверждения остановки.** Ошибки сигналов и финального ожидания игнорируются, PID-файл удаляется. Возвращать ошибку и сохранять запись до подтверждённого завершения процесса. Ревьюер: reliability; confidence 100.

23. **`tb-engine/crates/tb-cli/src/migrate.rs:44` -- identity check не проверяет старые schema claims.** Наличие `.2brain/repo.yaml` даёт успех даже при неразрешаемых старых именах. Проверять parser-распознанные claims, включая quoted и sequence формы, и сообщать их расположение. Ревьюеры: correctness, data-migration; confidence 100.

24. **`tb-engine/crates/tb-cli/src/migrate.rs:93` -- `--all` скрывает непроверенные registries.** Ошибки чтения, YAML и недоступные пути пропускаются, после чего неполный inventory считается проверенным. Возвращать ошибку с путями для существующих, но недоступных или некорректных записей. Ревьюер: data-migration; confidence 100.

25. **`tb-engine/crates/tb-engine/src/search.rs:1256` -- пустой словарный термин скрывает живую словоформу.** После incremental edit точное совпадение с нулевым числом postings мешает fallback; после rebuild ответ отличается. Учитывать только термины с postings и сравнить incremental update с rebuild в тесте. Ревьюеры: adversarial, performance; confidence 100.

26. **`tb-engine/crates/tb-engine/src/serve.rs:11415` -- delete может ложно сообщить `reflected=true`.** При неудачном rebuild старый catalog сохраняется, но отсутствие хеша трактуется как no-op. Отдельно представлять удаление и проверять отсутствие пути в held catalog. Ревьюер: correctness; confidence 100.

27. **`tb-engine/crates/tb-engine/src/serve.rs:13666` -- ошибка чтения выглядит как отсутствие файла.** I/O или UTF-8 ошибка превращается в `null`, после чего reconcile может подтвердить невыполненное удаление. Возвращать ошибки отдельно от подтверждённого `NotFound`. Ревьюер: reliability; confidence 100.

28. **`tb-engine/crates/tb-engine/src/serve.rs:15261` -- excerpt не показывает найденную словоформу.** Ranking разрешает singular/plural fallback, но excerpt ищет исходное слово и может показать начало документа вместо совпадения. Передавать разрешённые индексные термины и покрыть позднее совпадение тестом. Ревьюер: performance; confidence 100.

29. **`tb-engine/crates/tb-engine/src/serve.rs:4925` -- grant проверяется для неизменяемого referrer.** Разрешённый rename может быть отклонён из-за member с путём в `not_written`. Проверять владельцев фактически записываемых путей, сохранив защиту реальных изменений. Ревьюер: security; confidence 100.

30. **`tb-engine/scripts/swap-engine.sh:24` -- swap и restore проверяют старый socket.** Скрипт использует `tb-engine/run` вместо `engine/run`; restore может установить backup и затем ошибочно объявить восстановление неудачным. Получать socket из текущего engine и протестировать оба пути. Ревьюер: maintainability; confidence 100.

31. **`tb-mcp-rs/skills/2brain/references/tool-map.md:99` -- актуальная карта инструментов описывает удалённую совместимость.** Она обещает старый URI и `.arsumbris/repo.yaml`, тогда как adapter использует новый формат. Перегенерировать карту и обновить `docs/naming.md`, сохранив исторические записи. Ревьюер: maintainability; confidence 100.

32. **`tb-mcp-rs/src/lib.rs:2158` -- `tb_instances_of` пагинируется после ограничения frame.** При ответе engine больше 16 MiB даже страницу из одной записи получить нельзя. Передавать pagination в engine и применять до сериализации. Ревьюер: api-contract; confidence 100.

33. **`tb-mcp-rs/src/lib.rs:2426` -- каждая страница `tb_instances` строит и передаёт весь набор.** Это повторяет полный scan и может превысить frame limit. Измеренные 18,9 MB относятся к MCP-ответу в режиме both, а не доказывают превышение engine frame. Пагинировать до projection в engine. Ревьюеры: agent-native, performance; confidence 100.

34. **`tb-mcp-rs/src/lib.rs:4145` -- full profile с text payload нарушает объявленную схему.** `outputSchema` остаётся, но `structuredContent` отсутствует; в репозитории зафиксирован отказ реального schema-validating клиента. Не объявлять схему для text-only либо отклонять несовместимую комбинацию. Ревьюер: agent-native; confidence 100.

35. **`tb-mcp-rs/src/schemas.rs:199` -- `tb_graph_health` с `verbs=true` нарушает output schema.** Engine добавляет верхнеуровневое поле `verbs`, запрещённое схемой. Объявить поле необязательным и проверить реальный ответ валидатором схемы. Ревьюер: api-contract; confidence 100.

37. **`tb-mcp-rs/tests/real_engine.rs:385` -- тесты наследуют внешний `2BRAIN_HOME`.** Daemon запускается вне временного HOME, а тест ожидает socket внутри него и может читать registry пользователя. Явно удалить override или задать тестовый каталог для всех дочерних процессов. Ревьюеры: project-standards, testing; confidence 100.

38. **`tools/kb/legacy_audit.py:171` -- CI не запускает оговорённый rename ratchet.** ADR-0005 требует `legacy_audit.py --check`, но workflow его не вызывает. Добавить команду и регрессионный тест live-name нарушения; фактическое число регрессий не измерялось. Ревьюер: maintainability; confidence 100.

39. **`tools/kb/moc_links.py:73` -- счётчик topic membership включает ссылки из других полей.** Regex также обходит разрешение путей engine, поэтому отчёт и `--check` могут быть неверны. Запрашивать resolved incoming references и фильтровать `topics`; покрыть другие поля и qualified links. Частота в vault не измерялась. Ревьюер: project-standards; confidence 100.

40. **`tools/probe/visit_bound.py:269` -- probes и ranking launcher используют старый runtime layout.** Они не находят socket, а candidate launcher может оставить daemon после timeout; candidate-set также использует старый registry. Выводить пути из настроенного state root, включая `2BRAIN_HOME`, и завершать дочерний процесс при неудачном запуске. Ревьюеры: maintainability, project-standards; confidence 100.

42. **`tb-engine/crates/tb-engine/tests/it/mutate.rs:5097` -- тест rollback ненадёжен под root.** С `CAP_DAC_OVERRIDE` read-only mode bits не гарантируют отказ записи, поэтому ожидаемая ошибка может не произойти. Заменить chmod на детерминированное внедрение ошибки, сохранив assertions компенсации. Ревьюер: testing; confidence 75.

### Actionable Findings

Все 39 находок выше направлены `downstream-resolver`; для каждой есть `suggested_fix`, требуется проверка после исправления. `manual` означает необходимость определить решение или затронуть контракт; `gated_auto` обозначает предложенное локальное изменение, но не даёт разрешения применять его без решения пользователя.

- **Manual:** #1, #2, #3, #8, #9, #10, #11, #14, #15, #16, #17, #19, #21, #25, #32, #33, #39.
- **Gated auto:** #4, #5, #6, #12, #13, #18, #20, #22, #23, #24, #26, #27, #28, #29, #30, #31, #34, #35, #37, #38, #40, #42.

### Пробелы тестирования

- **#36 `tb-mcp-rs/tests/real_engine.rs:265` -- `tb_search` не воспроизводится против real engine.** Добавить аргументы существующей fixture в `fixture_cases` и сверять payload с текущим engine. Confidence 100; advisory, validator batch не охватывал этот пункт.
- **#41 `tb-engine/crates/tb-engine/src/serve.rs:6997` -- нет end-to-end denial cases для grant checks `promote`, `inline`, `rename_block_id`.** Добавить multi-member тесты с ограниченными и полными grants, проверкой неизменённых файлов при отказе и MCP propagation. Confidence 75; advisory, validator batch не охватывал этот пункт.

### Coverage

- Индекс `codebase-memory-mcp` для `root-2brain` перестроен перед review: 23 475 узлов и 90 157 рёбер; ревьюерам поручена навигация через него, где инструмент доступен, с прямым чтением как fallback.
- Полный review выбран по размеру: 369 636 изменённых строк, из них 148 659 исполняемых нетестовых; 79 795 строк не классифицированы как исполняемые. Это не гарантия исчерпывающей проверки каждого пути.
- Получены результаты всех 11 ревьюеров; failed и bound-exceeded: 0. Все использовали модель текущей сессии; отдельную модель или effort для них host не задавал.
- adversarial lens: in-process fallback (cross-model peer not run: host serving family unattested — the worker skipped before any egress because the oh-my-pi session file names no first-party default model for this runtime; nothing was sent to any provider; route codex was resolved and sanctioned, editorial default model at high effort requested). The in-process fallback shares the session model, so its findings carry no independence attestation and no agreement-promotion bonus.
- Validator: **39 confirmed, 0 rejected, 0 unresolved, 0 malformed, 0 failed, 0 shortcut-skipped**. Первый проход подтвердил #1-3 и преждевременно остановился с ошибочным указанием исчерпания бюджета; второй проверил оставшиеся 36. Подтверждение не даёт разрешения применять исправления.
- При независимой классификации protected subjects дополнительно учтены #1 как вопрос идентичности процесса и #31 как публичная совместимость; verdict в обоих случаях `confirmed`, маршрут и уровень не изменены.
- Из 60 исходных кандидатов 18 объединены как дубликаты; один пункт о длине модуля исключён как предпочтение, #36 и #41 перенесены в тестовые пробелы. Подавлено по confidence: 0; других soft-bucket demotions: 0.
- `REVIEW.md` не применён как review policy, поскольку в base его нет. Из diff исключены untracked `.codebase-memory/`, `.compound-engineering/`, `CODING_STANDARDS.md`, `REVIEW.md`; при этом `CODING_STANDARDS.md` использован как источник критериев.
- Предварительная проверка не выдала срочных P0/P1; кандидат про `.auignore` отозван, поскольку имя документировано как намеренное. Исправления и тестовые прогоны на этапе отчёта не выполнялись.

---

### Verdict

> **Not ready.** Открыты 19 подтверждённых P1, включая обходы границ доступа, потерю или ложное восстановление данных и отсутствие безопасного пути миграции.
>
> **Первое действие:** до cutover закрыть сохранность saga и восстановление старых журналов (#8, #9, #10), затем границы доступа (#5, #6, #17, #20), reconcile и миграцию. После исправлений нужны адресные проверки отказов, гонок и авторизации.

### Actionable Findings: приоритетный список

Все пункты: `downstream-resolver`, `suggested_fix` присутствует. Confidence 100, кроме #42 с 75.

- **#1 P1** `tb-engine/crates/tb-cli/src/daemon.rs:510` -- проверять экземпляр PID перед сигналом; `manual`.
- **#2 P1** `tb-engine/crates/tb-cli/src/migrate.rs:70` -- восстановить offline-миграцию vault; `manual`.
- **#3 P1** `tb-engine/crates/tb-engine/src/repo.rs:982` -- перенести registries с проверкой конфликтов; `manual`.
- **#4 P1** `tb-engine/crates/tb-engine/src/serve.rs:11520` -- запретить или компенсировать non-git batch; `gated_auto`.
- **#5 P1** `tb-engine/crates/tb-engine/src/serve.rs:13650` -- закрыть выход create через symlink; `gated_auto`.
- **#6 P1** `tb-engine/crates/tb-engine/src/serve.rs:15438` -- закрыть чтение внешних symlink через grep; `gated_auto`.
- **#8 P1** `tb-engine/crates/tb-engine/src/serve.rs:5087` -- сохранять preimages при ошибке rollback; `manual`.
- **#9 P1** `tb-engine/crates/tb-engine/src/serve.rs:5645` -- восстанавливать старые saga journals до cutover; `manual`.
- **#10 P1** `tb-engine/crates/tb-engine/src/serve.rs:5739` -- устойчиво отмечать завершённую saga; `manual`.
- **#11 P1** `tb-mcp-rs/src/bounded_stdio.rs:183` -- учитывать permits и timeout для rejection; `manual`.
- **#12 P1** `tb-mcp-rs/src/lib.rs:1201` -- требовать полное чтение перед заменой файла; `gated_auto`.
- **#13 P1** `tb-mcp-rs/src/lib.rs:1687` -- ограничить ожидание mutation preflight; `gated_auto`.
- **#14 P1** `tb-mcp-rs/src/lib.rs:2457` -- ограничить default candidates и добавить фильтр; `manual`.
- **#15 P1** `tb-mcp-rs/src/lib.rs:2794` -- не выводить `not_applied` из одного несовпадения хеша; `manual`.
- **#16 P1** `tb-mcp-rs/src/lib.rs:2806` -- ожидать установленного исхода in-flight mutation; `manual`.
- **#17 P1** `tb-mcp-rs/src/lib.rs:2979` -- проверять grant фактического владельца create; `manual`.
- **#18 P1** `tb-mcp-rs/src/lib.rs:4330` -- сделать `next_offset` используемым; `gated_auto`.
- **#19 P1** `tb-mcp-rs/src/transport.rs:200` -- отличать неполный rollback от clean rejection; `manual`.
- **#20 P1** `tb-mcp-rs/src/wire.rs:866` -- авторизовать фактический symlink target; `gated_auto`.
- **#21 P2** `install.sh:79` -- исправить чтение `2BRAIN_HOME` в fallback; `manual`.
- **#22 P2** `tb-engine/crates/tb-cli/src/daemon.rs:513` -- подтверждать остановку до удаления PID; `gated_auto`.
- **#23 P2** `tb-engine/crates/tb-cli/src/migrate.rs:44` -- проверять legacy claims; `gated_auto`.
- **#24 P2** `tb-engine/crates/tb-cli/src/migrate.rs:93` -- не скрывать ошибки inventory; `gated_auto`.
- **#25 P2** `tb-engine/crates/tb-engine/src/search.rs:1256` -- игнорировать пустые postings при fallback; `manual`.
- **#26 P2** `tb-engine/crates/tb-engine/src/serve.rs:11415` -- проверять отражение delete в catalog; `gated_auto`.
- **#27 P2** `tb-engine/crates/tb-engine/src/serve.rs:13666` -- отделить ошибки чтения от `NotFound`; `gated_auto`.
- **#28 P2** `tb-engine/crates/tb-engine/src/serve.rs:15261` -- строить excerpt по разрешённому термину; `gated_auto`.
- **#29 P2** `tb-engine/crates/tb-engine/src/serve.rs:4925` -- проверять grants только для записываемых путей; `gated_auto`.
- **#30 P2** `tb-engine/scripts/swap-engine.sh:24` -- исправить socket для swap и restore; `gated_auto`.
- **#31 P2** `tb-mcp-rs/skills/2brain/references/tool-map.md:99` -- обновить карту и naming guidance; `gated_auto`.
- **#32 P2** `tb-mcp-rs/src/lib.rs:2158` -- перенести paging `tb_instances_of` в engine; `manual`.
- **#33 P2** `tb-mcp-rs/src/lib.rs:2426` -- пагинировать `tb_instances` до projection; `manual`.
- **#34 P2** `tb-mcp-rs/src/lib.rs:4145` -- согласовать text payload и output schema; `gated_auto`.
- **#35 P2** `tb-mcp-rs/src/schemas.rs:199` -- объявить `verbs` в output schema; `gated_auto`.
- **#37 P2** `tb-mcp-rs/tests/real_engine.rs:385` -- изолировать тесты от `2BRAIN_HOME`; `gated_auto`.
- **#38 P2** `tools/kb/legacy_audit.py:171` -- включить rename audit в CI; `gated_auto`.
- **#39 P2** `tools/kb/moc_links.py:73` -- считать memberships через engine references; `manual`.
- **#40 P2** `tools/probe/visit_bound.py:269` -- обновить runtime paths и cleanup probes; `gated_auto`.
- **#42 P2** `tb-engine/crates/tb-engine/tests/it/mutate.rs:5097` -- детерминированно внедрять ошибку rollback; `gated_auto`.

Артефакты: `/tmp/compound-engineering-0/ce-code-review/20261119-145319-441a7399/`.
