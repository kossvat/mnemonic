//! The backfill of history (live-projects, design 9.2): every transcript
//! the watchers read, from its checkpoint or its start, into segments.
//!
//! It is the pass the daemon runs, told to read history too, and it goes
//! through the same checkpoint. So it can run while the daemon runs and
//! neither writes a record twice; a run that was interrupted is run
//! again and goes on where it stopped; a second run writes nothing.
use anyhow::{Result, bail};
use std::sync::Arc;

use super::codex::CodexWatcher;
use super::conversation::ConversationWatcher;
use super::history::TranscriptDirs;
use super::observe::{self, Observation};
use crate::config::Config;
use crate::storage::Storage;

/// What a run of the backfill did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Backfilled {
    /// Projects of the workspace.
    pub projects: usize,
    /// Transcripts found, and those among them that had something new.
    pub transcripts: usize,
    pub written: usize,
}

/// Read every transcript of the watched folders into segments. A
/// transcript that fails is logged and left for the next run.
pub fn run(config: &Config, storage: Arc<Storage>) -> Result<Backfilled> {
    run_in(config, storage, TranscriptDirs::of(config))
}

/// The backfill over the transcript folders given.
fn run_in(config: &Config, storage: Arc<Storage>, dirs: TranscriptDirs) -> Result<Backfilled> {
    let Some((map, registry)) = crate::projects::sync(&storage, &config.projects)? else {
        bail!("no workspace is configured: set [projects] workspace in the configuration");
    };
    let idle = config.sessions.idle_timeout_secs;
    let mut done = Backfilled {
        projects: registry.projects,
        ..Backfilled::default()
    };
    if let Some(dir) = dirs.conversation.filter(|dir| dir.exists()) {
        let files = ConversationWatcher::new(dir).find_jsonl_files();
        let pass = Observation::new(storage.clone(), observe::CLAUDE, idle).with_history();
        done.transcripts += files.len();
        done.written += pass.tick_seen(&files, &map, true, |_| None)?;
    }
    // The archive beside the sessions folder holds history when that
    // folder itself is gone: either is enough.
    let archived = dirs.codex_archive.filter(|dir| dir.exists());
    if let Some(dir) = dirs.codex.filter(|dir| dir.exists() || archived.is_some()) {
        let mut watcher = CodexWatcher::new(dir);
        if let Some(archive) = archived {
            watcher = watcher.with_archived_dir(archive);
        }
        let files = watcher.find_rollout_files();
        let pass = Observation::new(storage, observe::CODEX, idle).with_history();
        done.transcripts += files.len();
        done.written += pass.tick_seen(&files, &map, true, |_| None)?;
    }
    Ok(done)
}

#[cfg(test)]
#[path = "backfill_tests.rs"]
mod tests;
