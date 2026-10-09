pub mod backfill;
pub mod codex;
pub mod conversation;
mod discovery;
pub mod files;
pub mod git;
pub mod history;
mod ingress;
mod observe;
mod origin;
#[cfg(test)]
mod origin_claude_tests;
#[cfg(test)]
mod origin_tests;
#[cfg(test)]
mod quality_tests;
pub mod replay;
pub mod scope;
mod tail;

use anyhow::Result;
use tokio::sync::mpsc;

use crate::event::Event;

/// Trait for all event sources. Extensible for Phase 2 (conversation watcher, whisper input).
pub trait Watcher: Send + 'static {
    /// Start watching, send events to the channel
    fn start(self, tx: mpsc::Sender<Event>)
    -> impl std::future::Future<Output = Result<()>> + Send;
}
