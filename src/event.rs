use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Core event type — all watchers normalize into this
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub id: String,
    pub timestamp: DateTime<Utc>,
    pub source: EventSource,
    pub kind: EventKind,
    pub content: String,
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum EventSource {
    FileWatcher,
    GitWatcher,
    ConversationWatcher,
    Manual,
    Socket,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum EventKind {
    FileCreated,
    FileModified,
    FileDeleted,
    GitCommit,
    GitBranchCreated,
    UserCorrection,
    DependencyAdded,
    ErrorFixed,
    SessionStart,
    SessionEnd,
    Custom(String),
}

/// Classified memory entry — output of classifier
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub id: String,
    pub timestamp: DateTime<Utc>,
    pub title: String,
    pub content: String,
    pub memory_type: MemoryType,
    pub tags: Vec<String>,
    pub source: EventSource,
    pub importance: f32,
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum MemoryType {
    Decision,
    Feedback,
    Note,
    SessionSummary,
    Security,
}

impl std::fmt::Display for MemoryType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Decision => write!(f, "decision"),
            Self::Feedback => write!(f, "feedback"),
            Self::Note => write!(f, "note"),
            Self::SessionSummary => write!(f, "session_summary"),
            Self::Security => write!(f, "security"),
        }
    }
}

impl Event {
    pub fn new(source: EventSource, kind: EventKind, content: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            timestamp: Utc::now(),
            source,
            kind,
            content: content.into(),
            metadata: serde_json::Value::Null,
        }
    }

    pub fn with_metadata(mut self, metadata: serde_json::Value) -> Self {
        self.metadata = metadata;
        self
    }
}

impl MemoryEntry {
    pub fn new(
        title: impl Into<String>,
        content: impl Into<String>,
        memory_type: MemoryType,
        source: EventSource,
    ) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            timestamp: Utc::now(),
            title: title.into(),
            content: content.into(),
            memory_type,
            tags: Vec::new(),
            source,
            importance: 0.5,
            metadata: serde_json::Value::Null,
        }
    }
}
