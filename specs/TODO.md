# mnemonic - TODO (очередь для петли)

**Цель прогона:** реализовать Codex transcript watcher (README Roadmap).
Ловить Codex-сессии из `~/.codex/sessions/**/rollout-*.jsonl` (+ `~/.codex/archived_sessions/`),
по образцу Claude-watcher `src/watcher/conversation.rs`.

**Формат rollout jsonl:** каждая строка `{payload, timestamp, type}`.
Типы записей: `session_meta`, `turn_context`, `event_msg`, `response_item`, `compacted`.
Реальные сообщения user/assistant лежат в `response_item.payload`.

**Правила петли:** см. `../AGENTS.md`. Один пункт за тик, cargo зелёный, коммит, затем `codex review`.
Промежуточные тики: `cargo test` (debug, для скорости). Финальный T7: полный release-hygiene.

## Очередь
- [x] T1: добавить вариант `CodexWatcher` в enum `EventSource` (`src/event.rs`), обновить совпадающие match-арми, `cargo build && cargo test` зелёные
- [x] T2: создать `src/watcher/codex.rs` со структурами serde для rollout-записи (`payload`/`timestamp`/`type`) + unit-тест парсинга по одной строке каждого типа (inline fixture)
- [x] T3: реализовать обход `~/.codex/sessions/**/rollout-*.jsonl` + `archived_sessions`, инкрементальное чтение с offset (по образцу `conversation.rs`) + тест
- [x] T4: извлечь текст user/assistant из `response_item`, смаппить в `Event` (как `conversation.rs` делает decisions/corrections) + тест
- [x] T5: реализовать trait `Watcher` для `CodexWatcher`, зарегистрировать `pub mod codex;` в `src/watcher/mod.rs`, cargo зелёный
- [x] T6: заспавнить `CodexWatcher` в `src/daemon.rs` рядом с `ConversationWatcher` (под конфиг-флаг, по умолчанию вкл) + тест что spawn не падает

  Примечание: T2-T6 реализованы одним связным коммитом (watcher нельзя частично подключить без dead_code под `-D warnings`). Переиспользованы `is_correction`/`is_decision`/offset-логика из `conversation.rs` (видимость расширена до `pub(super)`). Атрибуция в `daemon.rs` (`attribute_fallback`) обновлена: Codex-память тоже получает agent-participant линк.
- [x] T7: отметить пункт README Roadmap "Codex transcript watcher" как `[x]`, прогнать полный `cargo fmt && cargo clippy --release --all-targets -- -D warnings && cargo test --release` (всё зелёное, CI-эквивалент пройден)

## BLOCKED
(сюда петля пишет что застряло и почему)

## Done
(закрытые пункты)
