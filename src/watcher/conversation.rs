use anyhow::Result;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tokio::sync::mpsc;
use tracing::{info, warn};

use std::sync::Arc;

use super::ingress::{IngressTail, ParsedTurn, RecordScope};
use super::scope::{ClaudeScope, ProjectScope};
use crate::event::EventSource;
use crate::storage::Storage;

/// Watches Claude Code conversation JSONL files for user messages.
/// Detects corrections, decisions, and important context from conversations.
pub struct ConversationWatcher {
    /// Directory containing JSONL conversation files
    sessions_dir: PathBuf,
    /// Poll interval in seconds
    poll_interval_secs: u64,
    /// Optional legacy offset file, read once when adopting SQLite cursors.
    state_path: Option<PathBuf>,
    /// Capture only sessions working inside these project roots.
    /// Unrestricted by default.
    scope: ProjectScope,
}

/// Strong correction markers — self-sufficient anywhere in the message.
const CORRECTION_PATTERNS_STRONG: &[&str] = &[
    "не так",
    "не то",
    "не туда",
    "не подходит",
    "стоп",
    "stop",
    "нет,",
    "nope",
    "переделай",
    "redo",
    "revert",
    "откатить",
    "верни",
    "не делай",
    "я имел в виду",
    "i meant",
    "это неправильно",
    "that's wrong",
    "that is wrong",
    "ignore that",
];

/// Strong markers that flip meaning under negation: «не забудь …» /
/// "don't forget …" are reminders, not corrections.
const CORRECTION_PATTERNS_NEGATABLE: &[&str] = &["забудь", "forget"];
const CORRECTION_NEGATED: &[&str] = &["не забудь", "don't forget", "dont forget", "not forget"];

/// Weak markers — words that also live in plain questions and requests.
/// Counted only near the START of a non-question message, because real
/// corrections lead with the refusal («Не надо переименовывать этот
/// столбец», "don't do X") while requests bury the word mid-sentence.
/// «лучше»/"better" were dropped entirely: typical false positives showed
/// they matched ordinary questions («Какой формат лучше подойдёт для
/// экспорта?», "would it be better to cache the parsed config?") far more
/// often than corrections, and the real corrections that use them carry a
/// strong marker anyway («Это неправильно, лучше считать хеш по
/// содержимому»).
const CORRECTION_PATTERNS_WEAK: &[&str] = &["не надо", "don't", "dont", "wrong", "instead"];

/// Patterns that indicate decisions
const DECISION_PATTERNS: &[&str] = &[
    "давай используем",
    "let's use",
    "lets use",
    "выбираем",
    "we'll go with",
    "going with",
    "решение:",
    "decision:",
    "decided",
    "используем",
    "будем использовать",
    "архитектура:",
    "architecture:",
    "стек:",
    "stack:",
    "переходим на",
    "switching to",
    "migrating to",
];

impl ConversationWatcher {
    pub fn new(sessions_dir: PathBuf) -> Self {
        Self {
            sessions_dir,
            poll_interval_secs: 10,
            state_path: None,
            scope: ProjectScope::default(),
        }
    }

    /// Limit capture to sessions working inside the scope's project roots.
    pub fn with_scope(mut self, scope: ProjectScope) -> Self {
        self.scope = scope;
        self
    }

    /// Import legacy offsets from `path` on first durable-ingress startup.
    pub fn with_state_path(mut self, path: PathBuf) -> Self {
        self.state_path = Some(path);
        self
    }

    pub(super) fn load_positions(state_path: &Path) -> HashMap<PathBuf, u64> {
        let Ok(raw) = std::fs::read_to_string(state_path) else {
            return HashMap::new();
        };
        match serde_json::from_str::<HashMap<String, u64>>(&raw) {
            Ok(m) => m.into_iter().map(|(k, v)| (PathBuf::from(k), v)).collect(),
            Err(e) => {
                warn!("Watcher offsets: unreadable state file, starting fresh: {e}");
                HashMap::new()
            }
        }
    }

    /// Detect if a user message is a correction.
    ///
    /// Tiered matching tuned against typical false positives: a flat
    /// keyword list captured questions («Какой формат лучше для экспорта?»),
    /// reminders («Не забудь поднять версию перед релизом») and attachment paths
    /// as high-importance feedback, polluting the journal.
    pub(super) fn is_correction(text: &str) -> bool {
        let cleaned = strip_attachment_refs(text);
        let lower = cleaned.to_lowercase();
        let trimmed = lower.trim();
        // Must be relatively short (corrections are usually brief)
        if trimmed.is_empty() || trimmed.len() > 500 {
            return false;
        }
        // A message that ends with a question mark is a question — the
        // user is asking, not steering.
        if trimmed.ends_with('?') {
            return false;
        }

        if CORRECTION_PATTERNS_STRONG
            .iter()
            .any(|p| trimmed.contains(p))
        {
            return true;
        }
        if CORRECTION_NEGATED.iter().any(|p| trimmed.contains(p)) {
            return false; // «не забудь…» — a reminder, never a correction
        }
        if CORRECTION_PATTERNS_NEGATABLE
            .iter()
            .any(|p| trimmed.contains(p))
        {
            return true;
        }

        // Weak markers: question-heavy messages («Не надо ли увеличить таймаут?
        // Или тесты падают по другой причине?») are clarifications even when
        // they don't END with a question mark.
        if trimmed.matches('?').count() >= 2 {
            return false;
        }
        let head: String = trimmed.chars().take(50).collect();
        CORRECTION_PATTERNS_WEAK.iter().any(|p| head.contains(p))
    }

    /// Detect if a user message contains a decision
    pub(super) fn is_decision(text: &str) -> bool {
        let lower = strip_attachment_refs(text).to_lowercase();
        DECISION_PATTERNS.iter().any(|p| lower.contains(p))
    }

    /// Keep the statement that triggered capture, plus bounded following
    /// context (e.g. a decision header followed by its bullets). The first
    /// line alone is often just "Done" and discards the actual decision.
    pub(super) fn decision_excerpt(text: &str) -> String {
        let lines: Vec<&str> = text.lines().collect();
        let start = lines
            .iter()
            .position(|line| Self::is_decision(line))
            .unwrap_or(0);
        lines[start..].join("\n").chars().take(1000).collect()
    }

    /// Find all JSONL conversation files
    pub(super) fn find_jsonl_files(&self) -> Vec<PathBuf> {
        let mut files = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&self.sessions_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    // Check project subdirectories for conversation files
                    if let Ok(sub_entries) = std::fs::read_dir(&path) {
                        for sub in sub_entries.flatten() {
                            let sub_path = sub.path();
                            if sub_path.extension().is_some_and(|e| e == "jsonl") {
                                files.push(sub_path);
                            }
                        }
                    }
                }
            }
        }
        files
    }

    /// Parse a JSONL line and extract user message if present
    fn parse_user_message(line: &str) -> Option<(String, String)> {
        Self::parse_message(line, "user")
    }

    /// Parse a JSONL line and extract assistant message if present.
    /// Wired into the watcher's emission loop for capturing
    /// assistant-side decisions (e.g. "we'll go with SQLite for the index",
    /// "switching to a bounded channel"). User-side `is_correction`
    /// detection isn't applied to assistant turns — assistants
    /// don't tell themselves "revert that edit".
    fn parse_assistant_message(line: &str) -> Option<(String, String)> {
        Self::parse_message(line, "assistant")
    }

    /// Parse a JSONL line for a message with the given role ("user" or
    /// "assistant"). Returns `(content, timestamp)` if the line matches
    /// the role and passes the quality filters. Both user and assistant
    /// callers route through this helper today; the role string is
    /// then attached to event metadata so the daemon's PeerAttributor
    /// can route to the right speaker/addressee.
    fn parse_message(line: &str, expected_role: &str) -> Option<(String, String)> {
        let v: serde_json::Value = serde_json::from_str(line).ok()?;

        if v.get("type")?.as_str()? != expected_role {
            return None;
        }

        // Codex P1: real Claude Code JSONL uses TWO shapes for
        // `message.content`:
        //   1. String:  "content": "the text"
        //   2. Array:   "content": [{"type":"text","text":"..."}, ...]
        // Sampling typical ~/.claude/projects JSONLs showed array
        // form is 4667 assistant + 2853 user rows vs only 295 user
        // strings — i.e., the previous string-only parser silently
        // dropped >95% of real lines. Both shapes route through
        // `extract_message_content` which concatenates `text` blocks
        // and ignores `tool_use` / `tool_result` / `thinking`.
        let content_value = v.get("message")?.get("content")?;
        let content = extract_message_content(content_value)?;
        let timestamp = v
            .get("timestamp")
            .or_else(|| v.get("message").and_then(|m| m.get("timestamp")))
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string();

        // Skip very short messages (greetings, "ok", "yes")
        if content.len() < 10 {
            return None;
        }

        // Skip system-reminder injected content
        if content.contains("<system-reminder>") {
            return None;
        }

        Some((content.to_string(), timestamp))
    }

    /// Build the metadata JSON the watcher attaches to each emitted
    /// event: `{"jsonl_path": "...", "role": "user"}`. The daemon's
    /// PeerAttributor reads this to (a) resolve which session a memory
    /// belongs to (by JSONL path) and (b) attribute the right
    /// speaker/addressee roles to peers based on whose turn it was.
    ///
    /// Path is stored as a string (lossless on UTF-8 paths, lossy
    /// fallback on non-UTF-8 — both real-world Claude Code paths and
    /// our temp dirs are UTF-8 so this is safe in practice).
    fn build_event_metadata(jsonl_path: &std::path::Path, role: &str) -> serde_json::Value {
        serde_json::json!({
            "jsonl_path": jsonl_path.to_string_lossy(),
            "role": role,
        })
    }
}

/// Extract the textual content of a Claude Code message field.
/// Handles two shapes found in Claude Code JSONLs:
///
/// 1. Plain string: `"content": "some text"`
/// 2. Block array: `"content": [{"type":"text","text":"..."}, ...]`
///
/// For the block-array form, concatenates every `type:"text"` block's
/// `text` field with newline separators, and IGNORES other block
/// types (`tool_use`, `tool_result`, `thinking`, image blocks).
/// Those carry no usable user-facing text and would dilute the
/// signal for `is_correction` / `is_decision` pattern matching.
///
/// Returns `None` if the value is neither shape OR if the extracted
/// text is empty after concatenation. The caller (parse_message)
/// already handles the empty / too-short case downstream, but
/// surfacing `None` early lets the calling chain stay `?`-friendly.
fn extract_message_content(value: &serde_json::Value) -> Option<String> {
    // Shape 1: bare string.
    if let Some(s) = value.as_str() {
        let trimmed = s.trim();
        if trimmed.is_empty() {
            return None;
        }
        return Some(trimmed.to_string());
    }
    // Shape 2: array of typed blocks.
    if let Some(blocks) = value.as_array() {
        let mut parts: Vec<&str> = Vec::new();
        for block in blocks {
            let block_type = block.get("type").and_then(|t| t.as_str()).unwrap_or("");
            if block_type == "text"
                && let Some(text) = block.get("text").and_then(|t| t.as_str())
            {
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    parts.push(trimmed);
                }
            }
            // tool_use / tool_result / thinking / image / etc:
            // intentionally skipped — no usable text content for
            // correction/decision pattern matching.
        }
        if parts.is_empty() {
            return None;
        }
        return Some(parts.join("\n"));
    }
    None
}

impl ConversationWatcher {
    pub(super) fn parse_turn(path: &Path, line: &str) -> Option<ParsedTurn> {
        let (role, message, timestamp) = if let Some((message, ts)) = Self::parse_user_message(line)
        {
            ("user", message, ts)
        } else {
            let (message, ts) = Self::parse_assistant_message(line)?;
            ("assistant", message, ts)
        };
        ParsedTurn::new(
            line,
            EventSource::ConversationWatcher,
            role,
            message,
            timestamp,
            Self::build_event_metadata(path, role),
        )
    }

    pub async fn start(self, storage: Arc<Storage>, wake: mpsc::Sender<()>) -> Result<()> {
        info!(
            "Conversation watcher started, monitoring: {}",
            self.sessions_dir.display()
        );
        let legacy = self
            .state_path
            .as_deref()
            .map(Self::load_positions)
            .unwrap_or_default();
        let ingress = IngressTail::new(storage, "conversation");
        let mut scoped = self
            .scope
            .is_restricted()
            .then(|| ClaudeScope::new(self.scope.clone()));
        let mut interval =
            tokio::time::interval(tokio::time::Duration::from_secs(self.poll_interval_secs));
        loop {
            interval.tick().await;
            if wake.is_closed() {
                return Ok(());
            }
            let files = self.find_jsonl_files();
            let scope = scoped.as_mut().map(|scope| scope as &mut dyn RecordScope);
            match ingress.tick_scoped(&files, &legacy, Self::parse_turn, scope) {
                Ok(n) if n > 0 => {
                    let _ = wake.try_send(());
                }
                Ok(_) => {}
                Err(e) => warn!("Conversation ingress tick will retry: {e}"),
            }
        }
    }
}

/// Strip Claude Code attachment references (`@"…path…"`) before pattern
/// matching — file names regularly contain trigger words ("stop-words.txt",
/// "redo-queue.json"), and an attach-only message is not user feedback.
fn strip_attachment_refs(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("@\"") {
        out.push_str(&rest[..start]);
        match rest[start + 2..].find('"') {
            Some(end) => rest = &rest[start + 2 + end + 1..],
            // Unclosed quote — the tail is all path, drop it.
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The one-time migration accepts the old JSON offset format.
    #[test]
    fn legacy_offsets_load_missing_and_corrupt_files() {
        let tmp = crate::test_support::temp_dir("mnemonic-watcher-offsets-");
        let dir = tmp.path();
        let live = dir.join("live.jsonl");
        std::fs::write(&live, "x").unwrap();
        let dead = dir.join("dead.jsonl"); // never created on disk

        let mut positions = HashMap::new();
        positions.insert(live.clone(), 42u64);
        positions.insert(dead.clone(), 7u64);

        let state = dir.join("offsets.json");
        std::fs::write(&state, serde_json::to_string(&positions).unwrap()).unwrap();
        let loaded = ConversationWatcher::load_positions(&state);
        assert_eq!(loaded.get(&live), Some(&42));
        assert_eq!(loaded.get(&dead), Some(&7));

        assert!(ConversationWatcher::load_positions(&dir.join("nope.json")).is_empty());
        std::fs::write(&state, "{not json").unwrap();
        assert!(ConversationWatcher::load_positions(&state).is_empty());
    }

    #[test]
    fn test_correction_detection() {
        assert!(ConversationWatcher::is_correction(
            "не так, переделай авторизацию"
        ));
        assert!(ConversationWatcher::is_correction("Stop, that's wrong"));
        assert!(ConversationWatcher::is_correction(
            "нет, лучше используй PostgreSQL"
        ));
        assert!(!ConversationWatcher::is_correction(
            "Добавь JWT авторизацию"
        ));
        assert!(!ConversationWatcher::is_correction("покажи мне код"));
    }

    /// Corpus of typical false positives: questions, reminders and
    /// requests that the old flat keyword list captured as
    /// importance-0.9 feedback, polluting the journal.
    #[test]
    fn questions_and_requests_are_not_corrections() {
        for msg in [
            "Какой формат лучше подойдёт для экспорта отчётов: CSV или Parquet?",
            "Would it be better to cache the parsed config between runs?",
            "Не надо ли увеличить таймаут? Или тесты падают по другой причине? Лог прикладываю ниже.",
            "Перед релизом не забудь поднять версию в Cargo.toml.",
            "Please don't forget to tag the release after the merge.",
            "Подскажи, какую библиотеку лучше взять для разбора дат в этом модуле.",
            "@\"/home/dev/exports/revert-candidates.csv\" Отсортируй строки по дате.",
        ] {
            assert!(
                !ConversationWatcher::is_correction(msg),
                "false positive: {msg}"
            );
        }
    }

    /// Typical TRUE positives of the same shapes must keep matching.
    #[test]
    fn real_corrections_still_detected() {
        for msg in [
            "Результат не тот, которого я ожидал: из схемы пропали два индекса.",
            "Обновилось не то поле, должен меняться updated_at.",
            "Не делай отдельный класс под каждый формат.",
            "Не надо кэшировать ответы с ошибкой.",
            "Не надо трогать публичный API в этом PR.",
            "Предыдущий черновик схемы забудь: актуальная версия лежит в ветке main.",
            "Ссылка в README ведёт не туда.",
            "Откуда взялась эта версия? Формат даты не подходит для API.",
        ] {
            assert!(
                ConversationWatcher::is_correction(msg),
                "missed correction: {msg}"
            );
        }
    }

    #[test]
    fn attachment_refs_are_stripped() {
        assert_eq!(
            strip_attachment_refs("@\"/srv/app/users.csv\" Импортируй эти строки в тестовую базу."),
            " Импортируй эти строки в тестовую базу."
        );
        // Unclosed quote → tail dropped, no panic.
        assert_eq!(
            strip_attachment_refs("Сравни с @\"/opt/data/partial"),
            "Сравни с "
        );
        // No refs → unchanged.
        assert_eq!(
            strip_attachment_refs("Сообщение без ссылок на файлы."),
            "Сообщение без ссылок на файлы."
        );
    }

    #[test]
    fn test_decision_detection() {
        assert!(ConversationWatcher::is_decision(
            "давай используем PostgreSQL"
        ));
        assert!(ConversationWatcher::is_decision(
            "Let's use Redis for caching"
        ));
        assert!(ConversationWatcher::is_decision("Переходим на FastAPI"));
        assert!(!ConversationWatcher::is_decision("покажи мне код"));
        assert!(!ConversationWatcher::is_decision("что думаешь?"));
    }

    #[test]
    fn test_parse_user_message() {
        let line = r#"{"type":"user","message":{"role":"user","content":"не так, переделай"},"timestamp":"2026-04-13T00:00:00Z"}"#;
        let result = ConversationWatcher::parse_user_message(line);
        assert!(result.is_some());
        let (msg, _ts) = result.unwrap();
        assert_eq!(msg, "не так, переделай");
    }

    #[test]
    fn test_skip_short_messages() {
        let line = r#"{"type":"user","message":{"role":"user","content":"ok"}}"#;
        assert!(ConversationWatcher::parse_user_message(line).is_none());
    }

    #[test]
    fn test_skip_assistant_messages() {
        let line = r#"{"type":"assistant","message":{"role":"assistant","content":"Here is the code for auth module"}}"#;
        // parse_user_message only matches `type=user` lines; assistant
        // lines are skipped because the watcher today doesn't capture
        // assistant turns.
        assert!(ConversationWatcher::parse_user_message(line).is_none());
    }

    /// `parse_message` is the generalized form parameterized by role —
    /// flips from user to assistant so future capture of assistant
    /// turns slots in without code changes. Today only `parse_user_message`
    /// uses it; the assistant path is covered here so the contract is
    /// pinned.
    #[test]
    fn test_parse_message_matches_assistant_role() {
        let line = r#"{"type":"assistant","message":{"role":"assistant","content":"Here is the code for auth module"}}"#;
        let r = ConversationWatcher::parse_message(line, "assistant");
        assert!(
            r.is_some(),
            "assistant role must match parse_message(_, \"assistant\")"
        );
        let (msg, _ts) = r.unwrap();
        assert!(msg.starts_with("Here is the code"));
    }

    /// `build_event_metadata` emits a JSON object with `jsonl_path` +
    /// `role` — the daemon's PeerAttributor and SessionTracker both
    /// read these keys, so the shape is part of the watcher↔daemon
    /// contract.
    #[test]
    fn test_build_event_metadata_shape() {
        use std::path::Path;
        let meta =
            ConversationWatcher::build_event_metadata(Path::new("/sessions/abc-123.jsonl"), "user");
        assert_eq!(meta.get("role").and_then(|v| v.as_str()), Some("user"));
        assert_eq!(
            meta.get("jsonl_path").and_then(|v| v.as_str()),
            Some("/sessions/abc-123.jsonl")
        );
    }

    /// `parse_assistant_message` matches `type=assistant` JSONL
    /// lines and returns the content. Same quality filters as the
    /// user path (≥10 chars, no system-reminder).
    #[test]
    fn test_parse_assistant_message_matches_and_filters() {
        let assistant = r#"{"type":"assistant","message":{"role":"assistant","content":"We'll go with PostgreSQL for the schema layer."}}"#;
        let parsed = ConversationWatcher::parse_assistant_message(assistant);
        assert!(parsed.is_some());
        let (msg, _) = parsed.unwrap();
        assert!(msg.starts_with("We'll go with PostgreSQL"));

        // User-typed line must NOT match the assistant parser.
        let user = r#"{"type":"user","message":{"role":"user","content":"что думаешь?"}}"#;
        assert!(ConversationWatcher::parse_assistant_message(user).is_none());

        // Short assistant content (< 10 chars) drops out — same
        // quality filter as user path.
        let short = r#"{"type":"assistant","message":{"role":"assistant","content":"ok"}}"#;
        assert!(ConversationWatcher::parse_assistant_message(short).is_none());

        // system-reminder content is skipped to avoid capturing
        // injected hook output as if it were the assistant's reply.
        let reminder = r#"{"type":"assistant","message":{"role":"assistant","content":"<system-reminder>Some injected reminder content here for testing</system-reminder>"}}"#;
        assert!(ConversationWatcher::parse_assistant_message(reminder).is_none());
    }

    /// `is_decision` patterns trigger on assistant-voice phrasings
    /// the watcher specifically wants to capture (e.g., the
    /// assistant committing to a tech choice in prose). Pins the
    /// shared pattern list against assistant-style input.
    #[test]
    fn test_decision_detection_works_on_assistant_phrasings() {
        assert!(ConversationWatcher::is_decision(
            "We'll go with PostgreSQL for the schema layer."
        ));
        assert!(ConversationWatcher::is_decision(
            "Switching to async tokio handles the connection pooling cleanly."
        ));
        assert!(ConversationWatcher::is_decision(
            "decision: ship the heuristic summarizer first, LLM behind a flag."
        ));
        // Pure code response shouldn't match — keeps the noise
        // floor low.
        assert!(!ConversationWatcher::is_decision(
            "Here is the code: fn foo() { println!(\"hi\"); }"
        ));
    }

    /// Codex P1: real Claude Code JSONL uses `content` as an
    /// ARRAY of typed blocks for most assistant and many user
    /// rows. The pre-fix `.as_str()` parser silently dropped all
    /// of those. Test pins the array-form happy path AND the
    /// non-text-block skip.
    #[test]
    fn test_parse_message_handles_array_content_with_text_blocks() {
        let line = r#"{"type":"assistant","message":{"role":"assistant","content":[
            {"type":"text","text":"We'll go with PostgreSQL for the schema layer."},
            {"type":"tool_use","id":"tu_1","name":"bash","input":{"cmd":"ls"}}
        ]}}"#;
        let parsed = ConversationWatcher::parse_assistant_message(line);
        assert!(parsed.is_some(), "array-form assistant content must parse");
        let (msg, _) = parsed.unwrap();
        assert!(msg.contains("PostgreSQL"));
        // tool_use block is skipped — bash command shouldn't leak
        // into the decision/correction text path.
        assert!(!msg.contains("ls"));
    }

    /// Array content with multiple text blocks gets concatenated
    /// with newlines, preserving the order. Real Claude responses
    /// often have intro text + a tool_use + closing text; we want
    /// the prose, joined.
    #[test]
    fn test_parse_message_joins_multiple_text_blocks() {
        let line = r#"{"type":"assistant","message":{"role":"assistant","content":[
            {"type":"text","text":"First, let's switch to async tokio."},
            {"type":"tool_use","id":"tu_2","name":"edit","input":{}},
            {"type":"text","text":"After the migration, run the test suite."}
        ]}}"#;
        let (msg, _) = ConversationWatcher::parse_assistant_message(line).unwrap();
        // Both text fragments present.
        assert!(msg.contains("switch to async tokio"));
        assert!(msg.contains("run the test suite"));
    }

    /// Content array containing ONLY non-text blocks (e.g., a
    /// turn that's purely a tool call) returns None — nothing
    /// usable for correction/decision matching.
    #[test]
    fn test_parse_message_skips_array_with_only_non_text_blocks() {
        let line = r#"{"type":"assistant","message":{"role":"assistant","content":[
            {"type":"tool_use","id":"tu_3","name":"bash","input":{}},
            {"type":"thinking","thinking":"internal reasoning..."}
        ]}}"#;
        assert!(ConversationWatcher::parse_assistant_message(line).is_none());
    }

    /// Long Cyrillic decisions must parse safely; excerpt bounds are tested
    /// separately below using the same helper called by both watchers.
    #[test]
    fn test_parse_message_does_not_panic_on_long_cyrillic_decision() {
        // 250 Cyrillic chars (~500 bytes) — the byte slice at
        // [..200] would land inside a 2-byte codepoint and panic.
        let cyrillic = "давай используем ".repeat(15); // ~255 chars
        let escaped = cyrillic.replace('"', "\\\"");
        let line =
            format!(r#"{{"type":"user","message":{{"role":"user","content":"{escaped}"}}}}"#);
        let parsed = ConversationWatcher::parse_user_message(&line);
        assert!(parsed.is_some());
        assert!(ConversationWatcher::decision_excerpt(&parsed.unwrap().0).contains("используем"));
    }

    #[test]
    fn decision_excerpt_preserves_the_statement_after_an_intro() {
        let text = "Готово.\n\nРешение:\n- используем PostgreSQL\n- SQLite оставляем только для локальных тестов";
        assert!(ConversationWatcher::is_decision(text));
        let excerpt = ConversationWatcher::decision_excerpt(text);
        assert!(excerpt.starts_with("Решение:"));
        assert!(excerpt.contains("используем PostgreSQL"));
        assert!(excerpt.contains("SQLite оставляем"));
        assert!(!excerpt.contains("Готово."));
    }

    #[test]
    fn decision_excerpt_is_bounded_and_utf8_safe() {
        let text = format!("Done.\nDecision: {}", "решение 🚀 ".repeat(200));
        let excerpt = ConversationWatcher::decision_excerpt(&text);
        assert!(excerpt.starts_with("Decision:"));
        assert_eq!(excerpt.chars().count(), 1000);
        assert!(excerpt.contains('🚀'));
    }
}
