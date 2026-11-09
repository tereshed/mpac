# Потоковый терминал

Терминал использует постоянный WebSocket вместо получения вывода каждые 400 мс. В новом daemon запись PTY будит подписчиков через `watch`; таймер для доставки вывода не нужен. Короткие серии ввода объединяются на 8 мс и отправляются по открытому соединению. Локальное эхо не подменяет ответ CLI.

В браузере WebSocket подключается к тому же daemon. В macOS-клиенте встроенный Rust-мост связывает WebView с проверенным процессом: постоянный ключ daemon остаётся в Rust, интерфейс получает только временный ключ окна. CSP разрешает точный loopback-порт моста. Новый TCP-сокет проверяется до передачи ключа; переподключение не обходит проверку владельца процесса и блокировки пространства.

Для уже работающего старого daemon предусмотрена совместимость без завершения CLI: нативное окно общается с Rust по постоянному WebSocket, а мост забирает старый HTTP-вывод каждые 16 мс при активности и до 120 мс в простое. После ввода вывод запрашивается немедленно. Старый браузерный backend также поддержан адаптивным опросом. Полностью событийная доставка между daemon и клиентом включается при следующем запуске нового daemon; работающие пользовательские сессии ради обновления не останавливаются.

## Порядок и восстановление

Вывод отправляется блоками до 32 KiB, следующий блок ждёт подтверждения завершённой отрисовки xterm. Позиция считается в байтах UTF-8. Переподключение ждёт уже поставленные на отрисовку данные и продолжает с последнего отрисованного байта, без повторного добавления истории.

Ввод и изменение размера имеют отдельные номера подтверждений. Неподтверждённый ввод не повторяется автоматически: результат мог уже попасть в CLI. Очередь ограничена 64 KiB вместе с отправляемым блоком. Unicode, bracketed paste и Shift+Enter сохраняют порядок байтов; JSON-экранирование учитывается при разбиении кадров. Закрытие окна/соединения не останавливает CLI.

История по-прежнему ограничена исходным хвостом 512 KiB; поток не является эмулятором полного снимка терминала. Если сохранённый cursor вышел за хвост, UI явно сообщает о сбросе. При чтении старых строк поступление вывода не возвращает просмотр вниз. Resize объединяется в кадр отрисовки; после закрытия диалога пропущенный размер пересчитывается.

## Passive mobile replay geometry

Output HTTP chunks and WebSocket frames include `geometry: {cols, rows}` at the start of the chunk and ordered `resizes: [{offset, cols, rows}]` at raw byte offsets within it. Zero-byte frames communicate idle resizes. Replay applies each resize between byte segments, preserving the UTF-8 decoder. Dimensions are bounded to the existing PTY limits; resize metadata is bounded with the 512 KiB retained output. A passive mobile viewer never resizes the remote PTY and suppresses automatic terminal-query replies from replay.

The mobile Text view wraps the retained scrollback for the normal buffer. When a CLI switches to the alternate buffer, Text follows its current interactive screen. History is a frozen snapshot of retained parsed lines. A small warning in the terminal toolbar indicates a gap in raw replay.

**Latest reply** reads the most recent message supplied by the root Claude Code or Codex `Stop` hook. It stays available after a reconnect or daemon restart, even when the original terminal lines are gone. MPAC stores one complete message per session (up to 192 KiB within the hook event limit) in the local workspace database and serves it only through the authenticated session endpoint. It does not import older transcripts or store prompts and tool outputs in this reader. A Stop hook can be followed by a continuation; the reply view does not change the session's activity or mark it viewed.
