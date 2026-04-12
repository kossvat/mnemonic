use crate::config::ClassifierConfig;
use crate::event::{Event, EventKind, MemoryEntry, MemoryType};

/// Rule-based classifier — no LLM, pattern matching only (<1ms)
pub struct RuleClassifier {
    config: ClassifierConfig,
}

impl RuleClassifier {
    pub fn new(config: ClassifierConfig) -> Self {
        Self { config }
    }

    fn classify_git_commit(&self, event: &Event) -> Option<MemoryEntry> {
        let message = event
            .metadata
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or(&event.content);

        let msg_lower = message.to_lowercase();

        // Determine type from commit message convention
        let (memory_type, importance) = if msg_lower.starts_with("feat")
            || msg_lower.contains("add ")
            || msg_lower.contains("implement")
        {
            (MemoryType::Decision, 0.7)
        } else if msg_lower.starts_with("fix") || msg_lower.contains("bug") {
            (MemoryType::Note, 0.6)
        } else if msg_lower.starts_with("refactor") || msg_lower.starts_with("chore") {
            (MemoryType::Note, 0.3)
        } else if msg_lower.starts_with("docs") || msg_lower.starts_with("test") {
            (MemoryType::Note, 0.2)
        } else {
            (MemoryType::Note, 0.4)
        };

        if importance < self.config.importance_threshold {
            return None;
        }

        let files_changed = event
            .metadata
            .get("files_changed")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        // Boost importance for large commits
        let importance = if files_changed > 10 {
            (importance + 0.2).min(1.0)
        } else {
            importance
        };

        let mut entry = MemoryEntry::new(
            message.trim(),
            &event.content,
            memory_type,
            event.source.clone(),
        );
        entry.importance = importance;
        entry.tags = extract_tags_from_commit(message);
        entry.metadata = event.metadata.clone();

        Some(entry)
    }

    fn classify_file_event(&self, event: &Event) -> Option<MemoryEntry> {
        let path = event
            .metadata
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        match &event.kind {
            EventKind::DependencyAdded => {
                let mut entry = MemoryEntry::new(
                    format!("Dependency change: {}", basename(path)),
                    &event.content,
                    MemoryType::Decision,
                    event.source.clone(),
                );
                entry.importance = 0.6;
                entry.tags = vec!["dependency".into()];
                Some(entry)
            }
            EventKind::FileCreated => {
                // Only track significant file creations
                let ext = event
                    .metadata
                    .get("extension")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");

                let importance = match ext {
                    "rs" | "ts" | "py" => 0.5,
                    "toml" | "json" | "yaml" => 0.4,
                    "md" => 0.3,
                    _ => 0.2,
                };

                if importance < self.config.importance_threshold {
                    return None;
                }

                let mut entry = MemoryEntry::new(
                    format!("New file: {}", basename(path)),
                    &event.content,
                    MemoryType::Note,
                    event.source.clone(),
                );
                entry.importance = importance;
                Some(entry)
            }
            EventKind::FileDeleted => {
                let mut entry = MemoryEntry::new(
                    format!("Deleted: {}", basename(path)),
                    &event.content,
                    MemoryType::Note,
                    event.source.clone(),
                );
                entry.importance = 0.4;
                Some(entry)
            }
            // FileModified — too noisy, skip by default
            _ => None,
        }
    }
}

impl super::Classifier for RuleClassifier {
    fn classify(&self, event: &Event) -> Option<MemoryEntry> {
        match &event.kind {
            EventKind::GitCommit => self.classify_git_commit(event),
            EventKind::FileCreated
            | EventKind::FileModified
            | EventKind::FileDeleted
            | EventKind::DependencyAdded => self.classify_file_event(event),
            EventKind::UserCorrection => {
                let mut entry = MemoryEntry::new(
                    "User correction",
                    &event.content,
                    MemoryType::Feedback,
                    event.source.clone(),
                );
                entry.importance = 0.9; // Always high priority
                entry.tags = vec!["feedback".into(), "correction".into()];
                Some(entry)
            }
            EventKind::ErrorFixed => {
                let mut entry = MemoryEntry::new(
                    "Error fixed",
                    &event.content,
                    MemoryType::Note,
                    event.source.clone(),
                );
                entry.importance = 0.6;
                entry.tags = vec!["bugfix".into()];
                Some(entry)
            }
            EventKind::SessionStart | EventKind::SessionEnd => {
                let mut entry = MemoryEntry::new(
                    format!(
                        "Session {}",
                        if event.kind == EventKind::SessionStart {
                            "started"
                        } else {
                            "ended"
                        }
                    ),
                    &event.content,
                    MemoryType::SessionSummary,
                    event.source.clone(),
                );
                entry.importance = 0.5;
                entry.tags = vec!["session".into()];
                Some(entry)
            }
            _ => None,
        }
    }
}

fn extract_tags_from_commit(message: &str) -> Vec<String> {
    let mut tags = Vec::new();
    let lower = message.to_lowercase();

    // Conventional commit prefix
    if let Some(prefix) = lower.split(':').next() {
        let prefix = prefix.split('(').next().unwrap_or(prefix).trim();
        match prefix {
            "feat" => tags.push("feature".into()),
            "fix" => tags.push("bugfix".into()),
            "refactor" => tags.push("refactor".into()),
            "docs" => tags.push("docs".into()),
            "test" => tags.push("test".into()),
            "chore" => tags.push("chore".into()),
            "perf" => tags.push("performance".into()),
            "security" | "sec" => tags.push("security".into()),
            _ => {}
        }
    }

    // Scope from conventional commit: feat(auth): ...
    if let Some(start) = lower.find('(')
        && let Some(end) = lower.find(')')
        && end > start + 1
    {
        tags.push(lower[start + 1..end].to_string());
    }

    tags
}

fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::classifier::Classifier;
    use crate::event::EventSource;

    fn make_classifier() -> RuleClassifier {
        RuleClassifier::new(ClassifierConfig {
            importance_threshold: 0.4,
            dedup_threshold: 0.92,
        })
    }

    #[test]
    fn test_classify_feat_commit() {
        let classifier = make_classifier();
        let event = Event::new(
            EventSource::GitWatcher,
            EventKind::GitCommit,
            "Git commit: feat(auth): Add JWT token refresh",
        )
        .with_metadata(serde_json::json!({
            "message": "feat(auth): Add JWT token refresh",
            "files_changed": 3,
        }));

        let entry = classifier.classify(&event).unwrap();
        assert_eq!(entry.memory_type, MemoryType::Decision);
        assert!(entry.importance >= 0.7);
        assert!(entry.tags.contains(&"feature".to_string()));
        assert!(entry.tags.contains(&"auth".to_string()));
    }

    #[test]
    fn test_classify_fix_commit() {
        let classifier = make_classifier();
        let event = Event::new(
            EventSource::GitWatcher,
            EventKind::GitCommit,
            "Git commit: fix: Resolve race condition",
        )
        .with_metadata(serde_json::json!({
            "message": "fix: Resolve race condition",
            "files_changed": 1,
        }));

        let entry = classifier.classify(&event).unwrap();
        assert_eq!(entry.memory_type, MemoryType::Note);
        assert!(entry.tags.contains(&"bugfix".to_string()));
    }

    #[test]
    fn test_skip_docs_commit() {
        let classifier = make_classifier();
        let event = Event::new(
            EventSource::GitWatcher,
            EventKind::GitCommit,
            "Git commit: docs: Update README",
        )
        .with_metadata(serde_json::json!({
            "message": "docs: Update README",
            "files_changed": 1,
        }));

        // importance 0.2 < threshold 0.4 → None
        assert!(classifier.classify(&event).is_none());
    }

    #[test]
    fn test_user_correction_always_saved() {
        let classifier = make_classifier();
        let event = Event::new(
            EventSource::Manual,
            EventKind::UserCorrection,
            "Don't use monolith architecture",
        );

        let entry = classifier.classify(&event).unwrap();
        assert_eq!(entry.memory_type, MemoryType::Feedback);
        assert!(entry.importance >= 0.9);
    }
}
