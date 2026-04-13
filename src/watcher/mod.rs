pub mod conversation;
pub mod files;
pub mod git;

use anyhow::Result;
use tokio::sync::mpsc;

use crate::event::Event;

/// Trait for all event sources. Extensible for Phase 2 (conversation watcher, whisper input).
pub trait Watcher: Send + 'static {
    /// Start watching, send events to the channel
    fn start(self, tx: mpsc::Sender<Event>)
    -> impl std::future::Future<Output = Result<()>> + Send;
}
