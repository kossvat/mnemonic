//! The observation pass (live-projects, design 7.2 and 7.4): a pass of its
//! own over the transcripts, beside admission.
//!
//! For every transcript, also one a program ran, it reads the bytes after
//! its own checkpoint, takes the facts of each record, walks them through
//! the rules into segments, and writes the segments, their activity, the
//! owner's prompts and the new checkpoint in one transaction. It never
//! admits a memory, never makes a stretch of the owner's work, and never
//! reads or moves the cursor of admission.
//!
//! The checkpoint carries the fingerprint admission uses to tell a file
//! that was replaced or rewritten, and the state of the rules as ids and
//! enums, so a restart resumes exactly where the pass stopped.
use anyhow::{Result, bail};
use chrono::{DateTime, Utc};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tracing::warn;

use super::discovery::FileStamp;
use super::ingress::{checkpoint, stream_of, unchanged};
use super::tail;
use crate::projects::ProjectMap;
use crate::projects::facts::{Kind, Observed};
use crate::projects::resolver::State;
use crate::projects::segments::Walker;
use crate::redaction;
use crate::storage::Storage;
use crate::storage::observation::Checkpoint;

/// How one format is read.
#[derive(Clone, Copy)]
pub(super) struct Format {
    /// The watcher's namespace: transcripts are named as admission names
    /// them, by this and the name of their file.
    pub namespace: &'static str,
    /// The kind of the client, as the segments name it.
    pub agent: &'static str,
    pub read:
        fn(&str, &ProjectMap, Option<&mut crate::plans::location::Location>) -> Option<Observed>,
}

pub(super) const CLAUDE: Format = Format {
    namespace: super::ingress::CONVERSATION_NAMESPACE,
    agent: "claude",
    read: crate::projects::facts::claude_with_location,
};

pub(super) const CODEX: Format = Format {
    namespace: "codex",
    agent: "codex",
    read: crate::projects::facts::codex_with_location,
};

/// How often a pass over one transcript starts again from a checkpoint
/// another writer moved, before it gives the transcript up for this tick.
const TRIES: usize = 4;

#[path = "observe_pass.rs"]
mod pass;

/// What became of one transcript in a pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Done {
    /// New records were read and written.
    Written,
    /// Nothing new was there.
    AtEnd,
    /// History the live pass leaves to the backfill.
    Left,
}

pub(super) struct Observation {
    storage: Arc<Storage>,
    format: Format,
    idle_secs: u64,
    /// Read every transcript from its checkpoint, however old: the
    /// backfill. The live pass leaves a transcript it has no checkpoint of
    /// and that nobody writes to any more.
    history: bool,
    stamps: Mutex<HashMap<PathBuf, FileStamp>>,
    /// The live state of the daemon, told after each write.
    live: Option<Arc<crate::live_run::Live>>,
    /// Where each transcript was read to, for the pass that keeps nothing
    /// in the store (`tick_unstored`).
    unstored: Mutex<HashMap<String, Unstored>>,
    /// Called between reading a transcript and writing what was read, for
    /// the tests of a file or a checkpoint that moves in between.
    #[cfg(test)]
    pub(super) before_write: Mutex<Option<BeforeWrite>>,
}

#[cfg(test)]
type BeforeWrite = Box<dyn FnMut(&Path) + Send>;

/// Where a transcript was read to by the pass that stores nothing, and
/// what its records said of the session.
#[derive(Default)]
struct Unstored {
    /// Where the reading stopped, with the fingerprint admission keeps of
    /// what was read: the file, the bytes it begins with, the bytes before
    /// the place it stopped at. `None` before the first reading.
    cursor: Option<crate::ingest::IngestCursor>,
    program: Option<bool>,
    /// The kind and the time of the last record.
    last: Option<(Kind, DateTime<Utc>)>,
    /// When the stretch the session is in began.
    started_at: Option<DateTime<Utc>>,
}

impl Observation {
    pub fn new(storage: Arc<Storage>, format: Format, idle_secs: u64) -> Self {
        Self {
            storage,
            format,
            idle_secs,
            history: false,
            stamps: Mutex::new(HashMap::new()),
            live: None,
            unstored: Mutex::new(HashMap::new()),
            #[cfg(test)]
            before_write: Mutex::new(None),
        }
    }

    /// Tell the live state of the daemon what each write leaves.
    pub fn with_live(mut self, live: Option<Arc<crate::live_run::Live>>) -> Self {
        self.live = live;
        self
    }

    /// Read history too: every transcript, from its checkpoint or its start.
    pub fn with_history(mut self) -> Self {
        self.history = true;
        self
    }

    /// A pass that asks the file system for every stamp.
    #[cfg(test)]
    pub fn tick(&self, files: &[PathBuf], map: &ProjectMap, full: bool) -> Result<usize> {
        self.tick_seen(files, map, full, |_| None)
    }

    /// One pass over the transcripts whose stamp moved since their last
    /// successful read (every one on a `full` pass). A failure on one
    /// transcript is logged and does not stop the rest; that transcript is
    /// read again at the next tick. Returns how many were written.
    ///
    /// `seen` gives the stamp admission took of a transcript in the same
    /// tick, so a tick over files that did not move asks the file system
    /// once for each, not twice. Where it gives none the pass asks itself.
    pub fn tick_seen(
        &self,
        files: &[PathBuf],
        map: &ProjectMap,
        full: bool,
        seen: impl Fn(&Path) -> Option<FileStamp>,
    ) -> Result<usize> {
        let mut stamps = self
            .stamps
            .lock()
            .map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        if full {
            let listed: HashSet<&PathBuf> = files.iter().collect();
            stamps.retain(|path, _| listed.contains(path));
        }
        let mut known: Option<HashMap<String, Checkpoint>> = None;
        let mut written = 0;
        for path in files {
            let stamp = seen(path).or_else(|| FileStamp::now(path));
            if !full && stamp.is_some() && stamps.get(path) == stamp.as_ref() {
                continue;
            }
            let stream = stream_of(self.format.namespace, path);
            if redaction::check_identity(&stream).is_err() {
                continue;
            }
            if known.is_none() {
                let prefix = format!("[{},", serde_json::to_string(self.format.namespace)?);
                known = Some(self.storage.observation_checkpoints(&prefix)?);
            }
            let start = known.as_ref().and_then(|known| known.get(&stream)).cloned();
            match self.pass(path, &stream, map, start) {
                Ok((done, read)) => {
                    written += usize::from(done == Done::Written);
                    stamps.insert(path.clone(), read);
                }
                Err(e) => {
                    stamps.remove(path);
                    warn!(
                        "{} observation failed for a transcript: {e}",
                        self.format.namespace
                    );
                }
            }
        }
        if written > 0
            && let Some(live) = &self.live
        {
            live.refresh_plans_at(Utc::now(), std::time::Instant::now(), true);
        }
        Ok(written)
    }

    /// The pass of a profile that has no workspace: no registry and no
    /// segment, so nothing is written, but the sessions at work are told
    /// to the live state, with no project. What was read of each
    /// transcript is held in memory. A transcript nobody wrote to for the
    /// idle time is not read: its session is live no more.
    pub fn tick_unstored(
        &self,
        files: &[PathBuf],
        full: bool,
        seen: impl Fn(&Path) -> Option<FileStamp>,
    ) -> Result<usize> {
        let Some(live) = &self.live else {
            return Ok(0);
        };
        let mut stamps = self
            .stamps
            .lock()
            .map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let mut held = self
            .unstored
            .lock()
            .map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        if full {
            let listed: HashSet<&PathBuf> = files.iter().collect();
            stamps.retain(|path, _| listed.contains(path));
        }
        let nowhere = ProjectMap::default();
        let mut told = 0;
        for path in files {
            let stamp = seen(path).or_else(|| FileStamp::now(path));
            if !full && stamp.is_some() && stamps.get(path) == stamp.as_ref() {
                continue;
            }
            let stream = stream_of(self.format.namespace, path);
            if redaction::check_identity(&stream).is_err() {
                continue;
            }
            let read = (|| -> Result<Option<FileStamp>> {
                let mut file = File::open(path)?;
                let meta = file.metadata()?;
                let read = FileStamp::of(&meta);
                let quiet = meta
                    .modified()
                    .ok()
                    .and_then(|at| at.elapsed().ok())
                    .is_some_and(|age| age.as_secs() >= self.idle_secs);
                if quiet && !held.contains_key(&stream) {
                    return Ok(Some(read));
                }
                // What was read before must still be what the file holds,
                // by the fingerprint the stored pass uses: a file that is
                // another one, shorter, or rewritten in place is read anew,
                // and what was known of its session is dropped.
                let mut state = held.remove(&stream).unwrap_or_default();
                let offset = match &state.cursor {
                    Some(cursor) if unchanged(&mut file, &meta, cursor)?.is_some() => cursor.offset,
                    Some(_) => {
                        state = Unstored::default();
                        0
                    }
                    None => 0,
                };
                let chunk = tail::read_snapshot(&mut file, meta.len(), offset)?;
                let idle = chrono::Duration::seconds(self.idle_secs as i64);
                for (_, line) in chunk.records(offset) {
                    let Some(observed) =
                        (self.format.read)(line.trim_end_matches('\n'), &nowhere, None)
                    else {
                        continue;
                    };
                    if state.program.is_none() {
                        state.program = observed.program;
                    }
                    let Some(at) = observed.at else {
                        continue;
                    };
                    match state.last {
                        // A copied record carries an earlier time.
                        Some((_, last)) if at < last => continue,
                        Some((_, last)) if at - last < idle => {}
                        _ => state.started_at = Some(at),
                    }
                    state.last = Some((observed.kind, at));
                }
                state.cursor = Some(checkpoint(
                    &mut file,
                    &meta,
                    stream.clone(),
                    0,
                    chunk.next_offset,
                    None,
                )?);
                if state.last.is_none() {
                    // Rewritten into a transcript with no timed record.
                    live.forget(&stream);
                }
                if let (Some((last_kind, last_at)), Some(started_at)) =
                    (state.last, state.started_at)
                {
                    live.observed(
                        &stream,
                        crate::live::Session {
                            agent: self.format.agent.to_string(),
                            program: state.program == Some(true),
                            project: None,
                            last_kind,
                            last_at,
                            started_at,
                        },
                    );
                }
                held.insert(stream.clone(), state);
                Ok(Some(read))
            })();
            match read {
                Ok(Some(read)) => {
                    told += 1;
                    stamps.insert(path.clone(), read);
                }
                Ok(None) => {}
                Err(e) => {
                    stamps.remove(path);
                    warn!(
                        "{} live reading failed for a transcript: {e}",
                        self.format.namespace
                    );
                }
            }
        }
        Ok(told)
    }

    /// Read what is new in one transcript and write it. Starts again from
    /// the stored checkpoint when another writer moved it meanwhile.
    pub(super) fn pass(
        &self,
        path: &Path,
        stream: &str,
        map: &ProjectMap,
        mut start: Option<Checkpoint>,
    ) -> Result<(Done, FileStamp)> {
        for attempt in 0..TRIES {
            if attempt > 0 {
                start = self.storage.observation_checkpoint(stream)?;
            }
            if let Some(done) = self.once(path, stream, map, start.as_ref())? {
                return Ok(done);
            }
        }
        bail!("the checkpoint of a transcript kept moving")
    }
}

#[cfg(test)]
#[path = "observe_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "observe_plans_tests.rs"]
mod plan_tests;
