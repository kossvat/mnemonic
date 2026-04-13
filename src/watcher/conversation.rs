use anyhow::Result;
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::event::{Event, EventKind, EventSource};

/// Watches Claude Code conversation JSONL files for user messages.
/// Detects corrections, decisions, and important context from conversations.
pub struct ConversationWatcher {
    /// Directory containing JSONL conversation files
    sessions_dir: PathBuf,
    /// Poll interval in seconds
    poll_interval_secs: u64,
}

/// Patterns that indicate user corrections (high priority)
const CORRECTION_PATTERNS: &[&str] = &[
    "не так", "не то", "стоп", "stop", "wrong", "нет,", "nope",
    "переделай", "redo", "revert", "откатить", "верни",
    "не делай", "don't", "dont", "не надо",
    "лучше", "better", "instead",
    "я имел в виду", "i meant",
    "это неправильно", "that's wrong", "that is wrong",
    "забудь", "forget", "ignore that",
];

/// Patterns that indicate decisions
const DECISION_PATTERNS: &[&str] = &[
    "давай используем", "let's use", "lets use",
    "выбираем", "we'll go with", "going with",
    "решение:", "decision:", "decided",
    "используем", "будем использовать",
    "архитектура:", "architecture:",
    "стек:", "stack:",
    "переходим на", "switching to", "migrating to",
];

impl ConversationWatcher {
    pub fn new(sessions_dir: PathBuf) -> Self {
        Self {
            sessions_dir,
            poll_interval_secs: 10,
        }
    }

    /// Detect if a user message is a correction
    fn is_correction(text: &str) -> bool {
        let lower = text.to_lowercase();
        // Must be relatively short (corrections are usually brief)
        if lower.len() > 500 {
            return false;
        }
        CORRECTION_PATTERNS.iter().any(|p| lower.contains(p))
    }

    /// Detect if a user message contains a decision
    fn is_decision(text: &str) -> bool {
        let lower = text.to_lowercase();
        DECISION_PATTERNS.iter().any(|p| lower.contains(p))
    }

    /// Find all JSONL conversation files
    fn find_jsonl_files(&self) -> Vec<PathBuf> {
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
        let v: serde_json::Value = serde_json::from_str(line).ok()?;

        // Only user messages
        if v.get("type")?.as_str()? != "user" {
            return None;
        }

        let content = v.get("message")?.get("content")?.as_str()?;
        let timestamp = v.get("timestamp")
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
}

impl super::Watcher for ConversationWatcher {
    async fn start(self, tx: mpsc::Sender<Event>) -> Result<()> {
        info!(
            "Conversation watcher started, monitoring: {}",
            self.sessions_dir.display()
        );

        // Track file positions to only read new lines
        let mut file_positions: HashMap<PathBuf, u64> = HashMap::new();

        // Initialize positions to end of existing files (don't replay history)
        for path in self.find_jsonl_files() {
            if let Ok(meta) = std::fs::metadata(&path) {
                file_positions.insert(path, meta.len());
            }
        }

        let mut interval =
            tokio::time::interval(tokio::time::Duration::from_secs(self.poll_interval_secs));

        loop {
            interval.tick().await;

            let files = self.find_jsonl_files();

            for path in &files {
                let current_pos = file_positions.get(path).copied().unwrap_or(0);
                let file_size = match std::fs::metadata(path) {
                    Ok(m) => m.len(),
                    Err(_) => continue,
                };

                // No new data
                if file_size <= current_pos {
                    continue;
                }

                // Read new lines
                match std::fs::read_to_string(path) {
                    Ok(content) => {
                        let new_content = if current_pos > 0 && (current_pos as usize) < content.len() {
                            &content[current_pos as usize..]
                        } else if current_pos == 0 {
                            // New file — skip to end (don't replay)
                            file_positions.insert(path.clone(), file_size);
                            continue;
                        } else {
                            continue;
                        };

                        for line in new_content.lines() {
                            if line.trim().is_empty() {
                                continue;
                            }

                            if let Some((message, _timestamp)) = Self::parse_user_message(line) {
                                if Self::is_correction(&message) {
                                    let event = Event::new(
                                        EventSource::ConversationWatcher,
                                        EventKind::UserCorrection,
                                        &message,
                                    );
                                    debug!("Conversation: correction detected");
                                    if tx.send(event).await.is_err() {
                                        return Ok(());
                                    }
                                } else if Self::is_decision(&message) {
                                    let first_line = message.lines().next().unwrap_or(&message);
                                    let truncated = if first_line.len() > 200 {
                                        &first_line[..200]
                                    } else {
                                        first_line
                                    };
                                    let event = Event::new(
                                        EventSource::ConversationWatcher,
                                        EventKind::Custom("conversation_decision".into()),
                                        truncated,
                                    );
                                    debug!("Conversation: decision detected");
                                    if tx.send(event).await.is_err() {
                                        return Ok(());
                                    }
                                }
                                // Regular messages are not captured (too noisy)
                            }
                        }

                        file_positions.insert(path.clone(), file_size);
                    }
                    Err(e) => {
                        warn!("Failed to read conversation file {}: {e}", path.display());
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_correction_detection() {
        assert!(ConversationWatcher::is_correction("не так, переделай авторизацию"));
        assert!(ConversationWatcher::is_correction("Stop, that's wrong"));
        assert!(ConversationWatcher::is_correction("нет, лучше используй PostgreSQL"));
        assert!(!ConversationWatcher::is_correction("Добавь JWT авторизацию"));
        assert!(!ConversationWatcher::is_correction("покажи мне код"));
    }

    #[test]
    fn test_decision_detection() {
        assert!(ConversationWatcher::is_decision("давай используем PostgreSQL"));
        assert!(ConversationWatcher::is_decision("Let's use Redis for caching"));
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
        assert!(ConversationWatcher::parse_user_message(line).is_none());
    }
}
