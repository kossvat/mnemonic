pub mod rules;

use crate::event::{Event, MemoryEntry};

/// Trait for classifiers. Extensible for Phase 2 (embedding-based classifier).
pub trait Classifier: Send + Sync {
    /// Classify an event into a memory entry, or None if it should be skipped
    fn classify(&self, event: &Event) -> Option<MemoryEntry>;
}
