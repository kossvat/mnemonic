use anyhow::Result;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use super::conversation::ConversationWatcher;
use crate::event::{Event, EventKind, EventSource};

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
    /// Where to persist per-file read offsets. None = RAM only (tests).
    state_path: Option<PathBuf>,
}

impl CodexWatcher {
    pub fn new(sessions_dir: PathBuf) -> Self {
        Self {
            sessions_dir,
            archived_dir: None,
            poll_interval_secs: 10,
            state_path: None,
        }
    }

    /// Also scan an archived-sessions directory (flat list of rollouts).
    pub fn with_archived_dir(mut self, dir: PathBuf) -> Self {
        self.archived_dir = Some(dir);
        self
    }

    /// Persist read offsets to `path` so restarts resume where they left
    /// off instead of skipping to EOF.
    pub fn with_state_path(mut self, path: PathBuf) -> Self {
        self.state_path = Some(path);
        self
    }

    /// Recursively collect `rollout-*.jsonl` files under the live sessions
    /// dir (nested `YYYY/MM/DD`) plus the flat archived dir.
    fn find_rollout_files(&self) -> Vec<PathBuf> {
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

impl super::Watcher for CodexWatcher {
    async fn start(self, tx: mpsc::Sender<Event>) -> Result<()> {
        info!(
            "Codex watcher started, monitoring: {}",
            self.sessions_dir.display()
        );

        // Reuse the conversation watcher's offset persistence so both
        // watchers resume from disk identically across daemon restarts.
        let mut file_positions: HashMap<PathBuf, u64> = self
            .state_path
            .as_deref()
            .map(ConversationWatcher::load_positions)
            .unwrap_or_default();

        // Files present at startup with no persisted offset start at EOF
        // (a first run must not replay months of Codex history). Known
        // files resume where they stopped (clamped to current size in case
        // the transcript was rewritten). Files appearing AFTER startup are
        // brand-new sessions, read from 0.
        for path in self.find_rollout_files() {
            if let Ok(meta) = std::fs::metadata(&path) {
                let pos = file_positions
                    .get(&path)
                    .map(|&p| p.min(meta.len()))
                    .unwrap_or(meta.len());
                file_positions.insert(path, pos);
            }
        }

        let mut interval =
            tokio::time::interval(tokio::time::Duration::from_secs(self.poll_interval_secs));

        loop {
            interval.tick().await;

            let files = self.find_rollout_files();
            let mut dirty = false;

            for path in &files {
                // Unknown file = created after startup = brand-new session:
                // read from the top.
                let current_pos = file_positions.get(path).copied().unwrap_or(0);
                let file_size = match std::fs::metadata(path) {
                    Ok(m) => m.len(),
                    Err(_) => continue,
                };

                if file_size == current_pos {
                    continue;
                }
                // Transcript shrank (rewritten between polls): re-sync to
                // the new EOF instead of replaying rewritten history.
                if file_size < current_pos {
                    file_positions.insert(path.clone(), file_size);
                    dirty = true;
                    continue;
                }

                match std::fs::read_to_string(path) {
                    Ok(content) => {
                        let new_content = if (current_pos as usize) < content.len() {
                            let pos = current_pos as usize;
                            // A cached BYTE offset can land inside a
                            // multi-byte UTF-8 codepoint if the file was
                            // rewritten — slicing there panics. Re-sync to
                            // EOF instead of crashing.
                            if !content.is_char_boundary(pos) {
                                file_positions.insert(path.clone(), file_size);
                                dirty = true;
                                continue;
                            }
                            &content[pos..]
                        } else {
                            continue;
                        };

                        for line in new_content.lines() {
                            if line.trim().is_empty() {
                                continue;
                            }
                            let Some((role, message, _ts)) = Self::parse_message_line(line) else {
                                continue;
                            };

                            let meta = Self::build_event_metadata(path, &role);

                            // User turns: corrections take priority, then
                            // decisions. Assistant turns: decisions only
                            // (an assistant doesn't tell itself "no, redo").
                            // Detection logic is shared with the
                            // conversation watcher.
                            if role == "user" && ConversationWatcher::is_correction(&message) {
                                let event = Event::new(
                                    EventSource::CodexWatcher,
                                    EventKind::UserCorrection,
                                    &message,
                                )
                                .with_metadata(meta);
                                debug!("Codex: correction detected");
                                if tx.send(event).await.is_err() {
                                    return Ok(());
                                }
                            } else if ConversationWatcher::is_decision(&message) {
                                let first_line = message.lines().next().unwrap_or(&message);
                                // chars() not bytes() — UTF-8 safe truncation.
                                let truncated: String = first_line.chars().take(200).collect();
                                let event = Event::new(
                                    EventSource::CodexWatcher,
                                    EventKind::Custom("conversation_decision".into()),
                                    truncated,
                                )
                                .with_metadata(meta);
                                debug!("Codex: decision detected ({role})");
                                if tx.send(event).await.is_err() {
                                    return Ok(());
                                }
                            }
                        }

                        file_positions.insert(path.clone(), file_size);
                        dirty = true;
                    }
                    Err(e) => {
                        warn!("Failed to read Codex rollout {}: {e}", path.display());
                    }
                }
            }

            if dirty && let Some(state_path) = self.state_path.as_deref() {
                ConversationWatcher::save_positions(state_path, &file_positions);
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
        let dir = std::env::temp_dir().join(format!("mnemonic-codex-{}", uuid::Uuid::new_v4()));
        let nested = dir.join("2026/06/15");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("rollout-a.jsonl"), "x").unwrap();
        std::fs::write(nested.join("other.jsonl"), "x").unwrap(); // wrong prefix
        std::fs::write(nested.join("rollout-b.txt"), "x").unwrap(); // wrong ext

        let mut found = Vec::new();
        collect_rollout(&dir, &mut found);
        let names: Vec<String> = found
            .iter()
            .filter_map(|p| p.file_name()?.to_str().map(str::to_string))
            .collect();
        assert!(names.contains(&"rollout-a.jsonl".to_string()));
        assert!(!names.iter().any(|n| n == "other.jsonl"));
        assert!(!names.iter().any(|n| n == "rollout-b.txt"));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
