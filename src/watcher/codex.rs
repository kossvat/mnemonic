use anyhow::Result;
use std::path::{Path, PathBuf};
use tokio::sync::mpsc;
use tracing::{info, warn};

use super::conversation::ConversationWatcher;
use std::sync::Arc;

use super::ingress::{IngressTail, ParsedTurn, RecordScope};
use super::scope::{CodexScope, ProjectScope};
use crate::event::EventSource;
use crate::storage::Storage;

/// Watches Codex CLI rollout transcripts for user/assistant decisions and
/// corrections, the same way [`ConversationWatcher`] does for Claude Code.
///
/// Codex stores one JSONL transcript per session under
/// `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl` (live) and
/// `~/.codex/archived_sessions/rollout-*.jsonl` (archived). Each line is
/// `{"type": ..., "timestamp": ..., "payload": ...}`. Real user/assistant
/// turns are `type == "response_item"` with `payload.type == "message"`;
/// everything else (`function_call`, `reasoning`, `web_search_call`,
/// `session_meta`, tool output, `event_msg` UI mirrors) is ignored.
///
/// Correction/decision detection and read-offset persistence are reused
/// verbatim from [`ConversationWatcher`] so both watchers stay in lockstep.
pub struct CodexWatcher {
    /// Live sessions root (`~/.codex/sessions`), scanned recursively.
    sessions_dir: PathBuf,
    /// Optional archived sessions dir (`~/.codex/archived_sessions`), flat.
    archived_dir: Option<PathBuf>,
    /// Poll interval in seconds.
    poll_interval_secs: u64,
    /// Optional legacy offset file, read once when adopting SQLite cursors.
    state_path: Option<PathBuf>,
    /// Capture only sessions working inside these project roots.
    /// Unrestricted by default.
    scope: ProjectScope,
}

impl CodexWatcher {
    pub fn new(sessions_dir: PathBuf) -> Self {
        Self {
            sessions_dir,
            archived_dir: None,
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

    /// Also scan an archived-sessions directory (flat list of rollouts).
    pub fn with_archived_dir(mut self, dir: PathBuf) -> Self {
        self.archived_dir = Some(dir);
        self
    }

    /// Import legacy offsets from `path` on first durable-ingress startup.
    pub fn with_state_path(mut self, path: PathBuf) -> Self {
        self.state_path = Some(path);
        self
    }

    /// Recursively collect `rollout-*.jsonl` files under the live sessions
    /// dir (nested `YYYY/MM/DD`) plus the flat archived dir.
    pub(super) fn find_rollout_files(&self) -> Vec<PathBuf> {
        let mut files = Vec::new();
        collect_rollout(&self.sessions_dir, &mut files);
        if let Some(arch) = &self.archived_dir {
            collect_rollout(arch, &mut files);
        }
        files
    }

    /// Build the metadata JSON attached to each emitted event:
    /// `{"jsonl_path": "...", "role": "user", "agent": "codex"}`. Mirrors
    /// the conversation watcher's `{jsonl_path, role}` contract (the
    /// daemon's PeerAttributor reads those) and adds `agent: "codex"` so
    /// downstream can tell Codex memories from Claude ones beyond the
    /// EventSource.
    fn build_event_metadata(jsonl_path: &Path, role: &str) -> serde_json::Value {
        serde_json::json!({
            "jsonl_path": jsonl_path.to_string_lossy(),
            "role": role,
            "agent": "codex",
        })
    }

    /// Parse one rollout JSONL line. Returns `(role, content, timestamp)`
    /// for `user`/`assistant` message records that pass the quality
    /// filters; `None` for every other record type and role (developer,
    /// tool calls, reasoning, session meta, UI event mirrors).
    fn parse_message_line(line: &str) -> Option<(String, String, String)> {
        let v: serde_json::Value = serde_json::from_str(line).ok()?;

        // Only canonical API records carry the message; `event_msg`
        // user_message/agent_message are UI-stream duplicates and are
        // skipped to avoid double-ingesting every turn.
        if v.get("type")?.as_str()? != "response_item" {
            return None;
        }
        let payload = v.get("payload")?;
        if payload.get("type")?.as_str()? != "message" {
            return None;
        }
        let role = payload.get("role")?.as_str()?;
        // `developer` carries base instructions/system prompt — not a turn.
        if role != "user" && role != "assistant" {
            return None;
        }

        let content = extract_codex_content(payload.get("content")?)?;

        // Skip very short messages (greetings, "ok", "yes").
        if content.len() < 10 {
            return None;
        }
        // Skip injected reminder content (mirrors conversation watcher).
        if content.contains("<system-reminder>") {
            return None;
        }

        let timestamp = v
            .get("timestamp")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string();

        Some((role.to_string(), content, timestamp))
    }
}

/// Recursively push every `rollout-*.jsonl` file under `dir` into `out`.
/// Depth is bounded in practice (`YYYY/MM/DD` = 3 levels) and missing or
/// unreadable directories are silently skipped.
fn collect_rollout(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rollout(&path, out);
        } else if path.extension().is_some_and(|e| e == "jsonl")
            && path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("rollout-"))
        {
            out.push(path);
        }
    }
}

/// Extract the textual content of a Codex message payload.
///
/// Codex `payload.content` is an array of typed blocks:
///   - user turn:      `{"type": "input_text",  "text": "..."}`
///   - assistant turn: `{"type": "output_text", "text": "..."}`
///
/// Concatenates the `text` of `input_text`/`output_text` (and a defensive
/// `text`) blocks with newlines; ignores any other block type. Also
/// accepts a bare string defensively. Returns `None` when nothing usable
/// remains.
fn extract_codex_content(value: &serde_json::Value) -> Option<String> {
    if let Some(s) = value.as_str() {
        let trimmed = s.trim();
        return if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        };
    }
    let blocks = value.as_array()?;
    let mut parts: Vec<String> = Vec::new();
    for block in blocks {
        let block_type = block.get("type").and_then(|t| t.as_str()).unwrap_or("");
        if matches!(block_type, "input_text" | "output_text" | "text")
            && let Some(text) = block.get("text").and_then(|t| t.as_str())
        {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                parts.push(trimmed.to_string());
            }
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n"))
    }
}

impl CodexWatcher {
    pub(super) fn parse_turn(path: &Path, line: &str) -> Option<ParsedTurn> {
        let (role, message, timestamp) = Self::parse_message_line(line)?;
        ParsedTurn::new(
            line,
            EventSource::CodexWatcher,
            &role,
            message,
            timestamp,
            Self::build_event_metadata(path, &role),
        )
    }

    pub async fn start(self, storage: Arc<Storage>, wake: mpsc::Sender<()>) -> Result<()> {
        info!(
            "Codex watcher started, monitoring: {}",
            self.sessions_dir.display()
        );
        let legacy = self
            .state_path
            .as_deref()
            .map(ConversationWatcher::load_positions)
            .unwrap_or_default();
        let ingress = IngressTail::new(storage, "codex");
        // The folder in force is carried between polls, so one scope lives
        // as long as the watcher does.
        let mut scoped = self
            .scope
            .is_restricted()
            .then(|| CodexScope::new(self.scope.clone()));
        let mut interval =
            tokio::time::interval(tokio::time::Duration::from_secs(self.poll_interval_secs));
        loop {
            interval.tick().await;
            if wake.is_closed() {
                return Ok(());
            }
            let files = self.find_rollout_files();
            let scope = scoped.as_mut().map(|scope| scope as &mut dyn RecordScope);
            match ingress.tick_scoped(&files, &legacy, Self::parse_turn, scope) {
                Ok(n) if n > 0 => {
                    let _ = wake.try_send(());
                }
                Ok(_) => {}
                Err(e) => warn!("Codex ingress tick will retry: {e}"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user_line(text: &str) -> String {
        format!(
            r#"{{"type":"response_item","timestamp":"2026-06-15T00:00:00Z","payload":{{"type":"message","role":"user","content":[{{"type":"input_text","text":"{text}"}}]}}}}"#
        )
    }

    fn assistant_line(text: &str) -> String {
        format!(
            r#"{{"type":"response_item","timestamp":"2026-06-15T00:00:00Z","payload":{{"type":"message","role":"assistant","content":[{{"type":"output_text","text":"{text}"}}]}}}}"#
        )
    }

    #[test]
    fn parses_user_input_text() {
        let (role, msg, ts) =
            CodexWatcher::parse_message_line(&user_line("давай используем PostgreSQL")).unwrap();
        assert_eq!(role, "user");
        assert_eq!(msg, "давай используем PostgreSQL");
        assert_eq!(ts, "2026-06-15T00:00:00Z");
    }

    #[test]
    fn parses_assistant_output_text() {
        let (role, msg, _) =
            CodexWatcher::parse_message_line(&assistant_line("We'll go with Redis for caching"))
                .unwrap();
        assert_eq!(role, "assistant");
        assert!(msg.contains("Redis"));
    }

    #[test]
    fn skips_non_message_records() {
        // function_call / reasoning / web_search_call payloads carry no role.
        let fc = r#"{"type":"response_item","timestamp":"t","payload":{"type":"function_call","name":"shell"}}"#;
        assert!(CodexWatcher::parse_message_line(fc).is_none());
        let reasoning =
            r#"{"type":"response_item","timestamp":"t","payload":{"type":"reasoning"}}"#;
        assert!(CodexWatcher::parse_message_line(reasoning).is_none());
    }

    #[test]
    fn skips_event_msg_ui_mirrors() {
        // event_msg duplicates the message for the UI; must be ignored so
        // turns aren't double-counted.
        let em = r#"{"type":"event_msg","timestamp":"t","payload":{"type":"user_message","message":"hi there friend"}}"#;
        assert!(CodexWatcher::parse_message_line(em).is_none());
    }

    #[test]
    fn skips_developer_role() {
        let dev = r#"{"type":"response_item","timestamp":"t","payload":{"type":"message","role":"developer","content":[{"type":"input_text","text":"base instructions here and there"}]}}"#;
        assert!(CodexWatcher::parse_message_line(dev).is_none());
    }

    #[test]
    fn skips_short_and_reminder_messages() {
        assert!(CodexWatcher::parse_message_line(&user_line("ok")).is_none());
        let reminder = user_line("<system-reminder>injected hook output here</system-reminder>");
        assert!(CodexWatcher::parse_message_line(&reminder).is_none());
    }

    #[test]
    fn joins_multiple_text_blocks() {
        let line = r#"{"type":"response_item","timestamp":"t","payload":{"type":"message","role":"assistant","content":[
            {"type":"output_text","text":"First, switching to async tokio."},
            {"type":"output_text","text":"Then run the test suite."}
        ]}}"#;
        let (_, msg, _) = CodexWatcher::parse_message_line(line).unwrap();
        assert!(msg.contains("async tokio"));
        assert!(msg.contains("run the test suite"));
    }

    #[test]
    fn extract_skips_non_text_blocks() {
        let v = serde_json::json!([
            {"type":"input_text","text":"keep this"},
            {"type":"image","url":"x"},
        ]);
        assert_eq!(extract_codex_content(&v).as_deref(), Some("keep this"));
        let only_other = serde_json::json!([{"type":"image","url":"x"}]);
        assert!(extract_codex_content(&only_other).is_none());
    }

    #[test]
    fn metadata_shape_has_codex_agent() {
        let meta = CodexWatcher::build_event_metadata(Path::new("/s/rollout-x.jsonl"), "user");
        assert_eq!(meta.get("role").and_then(|v| v.as_str()), Some("user"));
        assert_eq!(meta.get("agent").and_then(|v| v.as_str()), Some("codex"));
        assert_eq!(
            meta.get("jsonl_path").and_then(|v| v.as_str()),
            Some("/s/rollout-x.jsonl")
        );
    }

    #[test]
    fn collect_rollout_recurses_and_filters() {
        let tmp = crate::test_support::temp_dir("mnemonic-codex-");
        let dir = tmp.path();
        let nested = dir.join("2026/06/15");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("rollout-a.jsonl"), "x").unwrap();
        std::fs::write(nested.join("other.jsonl"), "x").unwrap(); // wrong prefix
        std::fs::write(nested.join("rollout-b.txt"), "x").unwrap(); // wrong ext

        let mut found = Vec::new();
        collect_rollout(dir, &mut found);
        let names: Vec<String> = found
            .iter()
            .filter_map(|p| p.file_name()?.to_str().map(str::to_string))
            .collect();
        assert!(names.contains(&"rollout-a.jsonl".to_string()));
        assert!(!names.iter().any(|n| n == "other.jsonl"));
        assert!(!names.iter().any(|n| n == "rollout-b.txt"));
    }
}
