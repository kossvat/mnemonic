//! Reading transcripts that already existed before capture started.
//!
//! Live capture adopts every pre-existing transcript at its end: a first run
//! must not replay months of history into the store on its own. This is the
//! deliberate way to bring that history in, through the same parsers, the same
//! project scope and the same message keys as live capture, so running it
//! twice, or alongside the daemon, adds each turn once.
//!
//! Turns land in the durable ingress queue with their original timestamps and
//! the running daemon files them exactly as it files live turns, except that
//! it does not open today's session for a months-old conversation.
//!
//! A store that was capturing before durable ingress existed holds turns from
//! that era with no key. One of them that was forgotten or consolidated since
//! cannot be recognised, and reading its transcript again would bring it back.
//! On such a store history therefore does not reach back past the moment
//! durable ingress started. A fresh project store has no such era.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::Serialize;

use super::codex::CodexWatcher;
use super::conversation::ConversationWatcher;
use super::ingress::{CONVERSATION_NAMESPACE, ParsedTurn, RecordScope, message_key, stream_of};
use super::scope::{ClaudeScope, CodexScope, ProjectScope};
use crate::config::Config;
use crate::ingest::{IngestPayload, IngestRecord};
use crate::storage::Storage;

/// Metadata flag the daemon reads: a history turn does not open a session.
pub const HISTORY_FLAG: &str = "history";

#[derive(Debug, Default, Serialize, PartialEq, Eq)]
pub struct HistoryReport {
    /// Transcript files read.
    pub files: usize,
    /// Corrections and decisions found inside the project scope.
    pub turns: usize,
    /// Of those, already in the store from live capture or an earlier run.
    pub known: usize,
    /// Of those, new. Queued with `apply`, otherwise what would be.
    pub new: usize,
    /// Turns outside the project scope, left out.
    pub out_of_scope: usize,
    /// Turns older than `since`, left out.
    pub before_since: usize,
    /// Turns from before durable ingress in a store that was already
    /// capturing then, left out. See `floor`.
    pub before_capture: usize,
    /// The earliest moment history may reach back to in this store, when
    /// earlier capture limits it.
    pub floor: Option<String>,
    /// Turns with no message id: no stable key to add them once, left out.
    pub without_id: usize,
    /// Turns with no readable timestamp: live capture would date them now,
    /// which would slip them past `since` and the capture floor, left out.
    pub undated: usize,
    /// The same turn copied into another transcript by /compact or a
    /// resume, counted once.
    pub repeated: usize,
    /// Whether the new turns were queued.
    pub applied: bool,
}

/// The transcript folders a store reads, decided in one place for the daemon
/// and for history alike.
pub struct TranscriptDirs {
    pub conversation: Option<PathBuf>,
    pub codex: Option<PathBuf>,
    pub codex_archive: Option<PathBuf>,
}

impl TranscriptDirs {
    pub fn of(config: &Config) -> Self {
        Self::resolve(config, !matches!(crate::profile::active(), Ok(None)))
    }

    fn resolve(config: &Config, in_profile: bool) -> Self {
        let home = dirs::home_dir().unwrap_or_default();
        let conversation = config.watchers.conversation_enabled.then(|| {
            config
                .watchers
                .conversation_sessions_dir
                .clone()
                .unwrap_or_else(|| home.join(".claude/projects"))
        });
        let codex = config.watchers.codex_enabled.then(|| {
            config
                .watchers
                .codex_sessions_dir
                .clone()
                .unwrap_or_else(|| home.join(".codex/sessions"))
        });
        // Archived sessions live in a sibling flat dir next to the sessions
        // root. An unscoped isolated profile watches only the directory it
        // named: the sibling would pull the owner's archived conversations in
        // behind an explicit client root. A project scope filters every
        // transcript, archived or not, so there the sibling is safe, and
        // leaving it out would lose a session archived before it was read.
        let implicit_archive =
            !in_profile || ProjectScope::new(&config.watchers.project_roots).is_restricted();
        let codex_archive = codex
            .as_ref()
            .filter(|_| implicit_archive)
            .and_then(|dir| dir.parent())
            .map(|parent| parent.join("archived_sessions"))
            .filter(|dir| dir.exists());
        Self {
            conversation,
            codex,
            codex_archive,
        }
    }
}

/// One transcript's complete records, in order. A trailing record still
/// being written is left for live capture.
fn complete_records(path: &Path, mut visit: impl FnMut(&str)) -> Result<()> {
    let mut reader = BufReader::new(std::fs::File::open(path)?);
    let mut record = Vec::new();
    loop {
        record.clear();
        if reader.read_until(b'\n', &mut record)? == 0 || record.last() != Some(&b'\n') {
            return Ok(());
        }
        // Same rule as the live reader: malformed UTF-8 is skipped.
        if let Ok(line) = std::str::from_utf8(&record) {
            visit(line);
        }
    }
}

struct Collector<'a> {
    since: Option<DateTime<Utc>>,
    floor: Option<DateTime<Utc>>,
    observed_at: DateTime<Utc>,
    report: &'a mut HistoryReport,
    records: Vec<IngestRecord>,
    seen: std::collections::HashSet<String>,
}

impl Collector<'_> {
    fn take(&mut self, namespace: &str, stream: &str, turn: Option<ParsedTurn>, admitted: bool) {
        let Some(mut turn) = turn else {
            return;
        };
        if !admitted {
            self.report.out_of_scope += 1;
            return;
        }
        let Some(source_key) = message_key(namespace, stream, &turn) else {
            self.report.without_id += 1;
            return;
        };
        // A copy first, so every distinct turn lands in exactly one count.
        if !self.seen.insert(source_key.clone()) {
            self.report.repeated += 1;
            return;
        }
        let dated = turn
            .source_at
            .as_deref()
            .is_some_and(|at| DateTime::parse_from_rfc3339(at).is_ok());
        if !dated {
            self.report.undated += 1;
            return;
        }
        if let Some(floor) = self.floor
            && turn.event.timestamp < floor
        {
            self.report.before_capture += 1;
            return;
        }
        if let Some(since) = self.since
            && turn.event.timestamp < since
        {
            self.report.before_since += 1;
            return;
        }
        self.report.turns += 1;
        if let Some(metadata) = turn.event.metadata.as_object_mut() {
            metadata.insert(HISTORY_FLAG.into(), true.into());
        }
        self.records.push(IngestRecord {
            source_key,
            source_at: turn.source_at,
            observed_at: self.observed_at,
            payload: IngestPayload { event: turn.event },
        });
    }
}

/// How far back history may reach in this store: `None` when nothing earlier
/// than durable ingress was ever captured here, otherwise the moment durable
/// ingress started (now, if it has not started yet).
///
/// Two signs of the earlier capture, because either can be missing: a fact
/// the store records the first time it is opened while holding memories with
/// no ingress receipt (it survives their being forgotten later), and the
/// earlier watcher's offset files next to the database.
pub fn history_floor(storage: &Storage, config: &Config) -> Result<Option<DateTime<Utc>>> {
    let offset_files = config
        .storage
        .db_path
        .parent()
        .map(|dir| {
            ["watcher_offsets.json", "codex_watcher_offsets.json"]
                .iter()
                .any(|name| dir.join(name).exists())
        })
        .unwrap_or(false);
    if !offset_files && !storage.had_legacy_capture()? {
        return Ok(None);
    }
    Ok(Some(storage.ingress_started_at()?.unwrap_or_else(Utc::now)))
}

/// Count, and with `apply` queue, the history of every transcript this store
/// reads.
pub fn ingest_history(
    storage: &Storage,
    config: &Config,
    since: Option<DateTime<Utc>>,
    apply: bool,
) -> Result<HistoryReport> {
    let dirs = TranscriptDirs::of(config);
    let scope = ProjectScope::new(&config.watchers.project_roots);
    let floor = history_floor(storage, config)?;
    let mut report = HistoryReport {
        floor: floor.map(|time| time.to_rfc3339()),
        ..HistoryReport::default()
    };
    let mut collector = Collector {
        since,
        floor,
        observed_at: Utc::now(),
        report: &mut report,
        records: Vec::new(),
        seen: std::collections::HashSet::new(),
    };

    if let Some(dir) = dirs.conversation.filter(|dir| dir.exists()) {
        let mut claude = ClaudeScope::new(scope.clone());
        for path in ConversationWatcher::new(dir).find_jsonl_files() {
            let stream = stream_of(CONVERSATION_NAMESPACE, &path);
            claude.reset();
            complete_records(&path, |line| {
                let admitted = !scope.is_restricted() || claude.admit(line);
                let turn = ConversationWatcher::parse_turn(&path, line);
                collector.take(CONVERSATION_NAMESPACE, &stream, turn, admitted);
            })?;
            collector.report.files += 1;
        }
    }
    if let Some(dir) = dirs.codex.filter(|dir| dir.exists()) {
        let mut watcher = CodexWatcher::new(dir);
        if let Some(archive) = dirs.codex_archive {
            watcher = watcher.with_archived_dir(archive);
        }
        let mut codex = CodexScope::new(scope.clone());
        for path in watcher.find_rollout_files() {
            let stream = stream_of("codex", &path);
            codex.reset();
            complete_records(&path, |line| {
                let admitted = !scope.is_restricted() || codex.admit(line);
                let turn = CodexWatcher::parse_turn(&path, line);
                collector.take("codex", &stream, turn, admitted);
            })?;
            collector.report.files += 1;
        }
    }

    let records = std::mem::take(&mut collector.records);
    let keys: Vec<String> = records.iter().map(|r| r.source_key.clone()).collect();
    let known = storage.ingest_keys_known(&keys)?;
    report.known = known.len();
    let fresh: Vec<IngestRecord> = records
        .into_iter()
        .filter(|record| !known.contains(&record.source_key))
        .collect();
    report.new = fresh.len();
    if apply {
        // Written in slices so one huge history does not hold the store's
        // write lock for long while the daemon is capturing.
        for slice in fresh.chunks(500) {
            storage.append_history(slice)?;
        }
        report.applied = true;
    }
    Ok(report)
}

#[cfg(test)]
#[path = "history_tests.rs"]
mod tests;
