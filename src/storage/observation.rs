//! What the observation pass keeps (live-projects, design 7.4): the
//! segments of each transcript, their activity by the minute, the times of
//! the owner's prompts, and the checkpoint of the pass.
//!
//! One immediate transaction per transcript holds all four. It first checks
//! that the stored checkpoint is the one the batch started from, so two
//! writers on one transcript (the live pass and a backfill) write each
//! record once: the later one finds the checkpoint moved and starts again
//! from it. A write that fails leaves everything where it was.
//!
//! The tables hold ids, enums, times and hashes. The stream names the
//! transcript by its file, as the cursor of admission does; no folder, path
//! or branch is kept.
use anyhow::{Result, ensure};
use chrono::{DateTime, NaiveDateTime, Utc};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use std::collections::{BTreeMap, HashMap};

use super::Storage;
use crate::ingest::IngestCursor;
use crate::projects::facts::Kind;
use crate::projects::resolver::{Source, State};
use crate::projects::segments::{Learned, Segment, union_seconds};

pub(super) const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS work_segments (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        stream TEXT NOT NULL,
        driver TEXT NOT NULL CHECK (driver IN ('owner', 'program')),
        agent TEXT NOT NULL,
        project_id INTEGER REFERENCES projects(id),
        started_at TEXT NOT NULL,
        last_at TEXT NOT NULL,
        last_touch_at TEXT,
        source TEXT NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_work_segments_stream
        ON work_segments(stream, last_at);
    CREATE INDEX IF NOT EXISTS idx_work_segments_time
        ON work_segments(last_at, started_at);
    CREATE TABLE IF NOT EXISTS segment_activity (
        segment_id INTEGER NOT NULL REFERENCES work_segments(id) ON DELETE CASCADE,
        minute INTEGER NOT NULL,
        records INTEGER NOT NULL,
        touches INTEGER NOT NULL,
        PRIMARY KEY (segment_id, minute)
    );
    CREATE TABLE IF NOT EXISTS owner_prompts (
        stream TEXT NOT NULL,
        at TEXT NOT NULL,
        PRIMARY KEY (stream, at)
    );
    CREATE INDEX IF NOT EXISTS idx_owner_prompts_at ON owner_prompts(at);
    CREATE TABLE IF NOT EXISTS observation_checkpoints (
        stream TEXT PRIMARY KEY,
        generation INTEGER NOT NULL,
        offset INTEGER NOT NULL,
        file_id TEXT NOT NULL,
        prefix_len INTEGER NOT NULL,
        prefix_hash TEXT NOT NULL,
        anchor_hash TEXT NOT NULL,
        driver TEXT,
        project_id INTEGER,
        candidate_id INTEGER,
        folder_id INTEGER,
        placed INTEGER NOT NULL DEFAULT 0,
        source TEXT NOT NULL DEFAULT 'unknown',
        last_kind TEXT NOT NULL DEFAULT 'other',
        last_at TEXT
    );
";

/// The time format of the tables: what SQLite's `datetime()` writes, in UTC.
const STAMP: &str = "%Y-%m-%d %H:%M:%S";

fn stamp(at: DateTime<Utc>) -> String {
    at.format(STAMP).to_string()
}

fn unstamp(text: &str) -> Option<DateTime<Utc>> {
    NaiveDateTime::parse_from_str(text, STAMP)
        .ok()
        .map(|at| at.and_utc())
}

fn learned_text(learned: Learned) -> &'static str {
    match learned {
        Learned::Backfill => "backfill",
        Learned::By(source) => source.as_str(),
    }
}

fn learned_of(text: &str) -> Learned {
    match text {
        "backfill" => Learned::Backfill,
        other => Learned::By(Source::parse(other)),
    }
}

/// Where the pass stands in a transcript: the position and fingerprint of
/// what it read, as admission keeps them, and the state of the rules at
/// that position, as ids, enums and a time.
#[derive(Debug, Clone, PartialEq)]
pub struct Checkpoint {
    pub cursor: IngestCursor,
    /// Who drives the session, once a record said so.
    pub program: Option<bool>,
    pub state: State,
    /// The kind and time of the last record read.
    pub last_kind: Kind,
    pub last_at: Option<DateTime<Utc>>,
}

/// What one pass over the new bytes of one transcript gives.
pub struct Batch<'a> {
    /// The checkpoint the pass started from; `None` for a transcript it
    /// had not read.
    pub expected: Option<&'a Checkpoint>,
    pub next: &'a Checkpoint,
    /// The transcript was replaced or rewritten: what was kept of it is
    /// dropped first.
    pub replaced: bool,
    /// The kind of the client: `claude` or `codex`.
    pub agent: &'static str,
    pub segments: &'a [Segment],
    pub prompts: &'a [DateTime<Utc>],
}

const CHECKPOINT_COLUMNS: &str = "stream, generation, offset, file_id, prefix_len, prefix_hash, \
     anchor_hash, driver, project_id, candidate_id, folder_id, placed, source, last_kind, last_at";

fn checkpoint_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Checkpoint> {
    let driver: Option<String> = r.get(7)?;
    Ok(Checkpoint {
        cursor: IngestCursor {
            stream: r.get(0)?,
            generation: r.get(1)?,
            offset: r.get(2)?,
            file_id: r.get(3)?,
            prefix_len: r.get(4)?,
            prefix_hash: r.get(5)?,
            anchor_hash: r.get(6)?,
        },
        program: driver.map(|driver| driver == "program"),
        state: State {
            project: r.get(8)?,
            candidate: r.get(9)?,
            folder: r.get(10)?,
            placed: r.get(11)?,
            source: Source::parse(&r.get::<_, String>(12)?),
        },
        last_kind: Kind::parse(&r.get::<_, String>(13)?),
        last_at: r.get::<_, Option<String>>(14)?.as_deref().and_then(unstamp),
    })
}

fn checkpoint_on(conn: &Connection, stream: &str) -> Result<Option<Checkpoint>> {
    Ok(conn
        .query_row(
            &format!("SELECT {CHECKPOINT_COLUMNS} FROM observation_checkpoints WHERE stream = ?1"),
            params![stream],
            checkpoint_row,
        )
        .optional()?)
}

fn session_start_on(conn: &Connection, stream: &str) -> Result<Option<DateTime<Utc>>> {
    let mut stmt = conn.prepare(
        "SELECT started_at, last_at FROM work_segments
          WHERE stream = ?1 ORDER BY started_at DESC, id DESC LIMIT 256",
    )?;
    let rows = stmt
        .query_map(params![stream], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut start: Option<&String> = None;
    for (started, last) in &rows {
        match start {
            // The segment before it ended where it began: one stretch.
            Some(known) if last == known => start = Some(started),
            Some(_) => break,
            None => start = Some(started),
        }
    }
    Ok(start.and_then(|at| unstamp(at)))
}

/// A row of `work_segments` as the open segment is read: its id, project,
/// start, last record, last touch and source.
type SegmentRow = (i64, Option<i64>, String, String, Option<String>, String);
/// The start and the last record of a segment.
type Interval = (DateTime<Utc>, DateTime<Utc>);

impl Storage {
    /// Every checkpoint of the pass whose stream begins with `prefix`, read
    /// in one query.
    pub fn observation_checkpoints(&self, prefix: &str) -> Result<HashMap<String, Checkpoint>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {CHECKPOINT_COLUMNS} FROM observation_checkpoints
              WHERE substr(stream, 1, length(?1)) = ?1"
        ))?;
        let rows = stmt
            .query_map([prefix], checkpoint_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows
            .into_iter()
            .map(|checkpoint| (checkpoint.cursor.stream.clone(), checkpoint))
            .collect())
    }

    /// The checkpoint of one transcript.
    pub fn observation_checkpoint(&self, stream: &str) -> Result<Option<Checkpoint>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        checkpoint_on(&conn, stream)
    }

    /// The last segment of a transcript: the one the next record extends
    /// when it comes before the idle time is over.
    pub fn open_segment(&self, stream: &str) -> Result<Option<Segment>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let row: Option<SegmentRow> = conn
            .query_row(
                "SELECT id, project_id, started_at, last_at, last_touch_at, source
                   FROM work_segments WHERE stream = ?1
                  ORDER BY last_at DESC, id DESC LIMIT 1",
                params![stream],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                    ))
                },
            )
            .optional()?;
        Ok(row.and_then(|(id, project, started, last, touch, source)| {
            Some(Segment {
                stored: Some(id),
                project,
                learned: learned_of(&source),
                started_at: unstamp(&started)?,
                last_at: unstamp(&last)?,
                last_touch_at: touch.as_deref().and_then(unstamp),
                activity: Vec::new(),
            })
        }))
    }

    /// Write what a pass over one transcript gave, in one transaction.
    /// `false` when the stored checkpoint is not the one the batch started
    /// from: another writer was there first, nothing was written, and the
    /// pass starts again from the stored one.
    pub fn observe(&self, batch: &Batch<'_>) -> Result<bool> {
        let stream = &batch.next.cursor.stream;
        ensure!(
            crate::redaction::check_identity(stream).is_ok(),
            "observation stream refused ({})",
            crate::redaction::RedactionError::SensitiveContent.code()
        );
        let mut conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if checkpoint_on(&tx, stream)?.as_ref() != batch.expected {
            return Ok(false);
        }
        // The days this write changes the owner's time of: those of what
        // is dropped and those of what is written.
        let mut changed: Vec<Interval> = Vec::new();
        // What is written says something of the time after it too.
        let segment_reach = chrono::Duration::seconds(super::owner_time::SEGMENT_REACH_SECS);
        let prompt_reach = chrono::Duration::seconds(super::owner_time::PROMPT_REACH_SECS);
        if batch.replaced {
            for (sql, reach) in [
                (
                    "SELECT MIN(started_at), MAX(last_at) FROM work_segments WHERE stream = ?1",
                    segment_reach,
                ),
                (
                    "SELECT MIN(at), MAX(at) FROM owner_prompts WHERE stream = ?1",
                    prompt_reach,
                ),
            ] {
                let kept: (Option<String>, Option<String>) =
                    tx.query_row(sql, params![stream], |r| Ok((r.get(0)?, r.get(1)?)))?;
                if let (Some(first), Some(last)) = kept
                    && let (Some(first), Some(last)) = (unstamp(&first), unstamp(&last))
                {
                    changed.push((first, last + reach));
                }
            }
            tx.execute(
                "DELETE FROM segment_activity WHERE segment_id IN
                    (SELECT id FROM work_segments WHERE stream = ?1)",
                params![stream],
            )?;
            tx.execute(
                "DELETE FROM work_segments WHERE stream = ?1",
                params![stream],
            )?;
            tx.execute(
                "DELETE FROM owner_prompts WHERE stream = ?1",
                params![stream],
            )?;
        }
        let driver = if batch.next.program == Some(true) {
            "program"
        } else {
            "owner"
        };
        // The record that says who drives the session came after rows were
        // written for it: they are what an uninterrupted read would have
        // written, from here on.
        if !batch.replaced
            && batch
                .expected
                .is_some_and(|before| before.program != batch.next.program)
        {
            let kept: (Option<String>, Option<String>) = tx.query_row(
                "SELECT MIN(started_at), MAX(last_at) FROM work_segments
                  WHERE stream = ?1 AND driver IS NOT ?2",
                params![stream, driver],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            if let (Some(first), Some(last)) = kept
                && let (Some(first), Some(last)) = (unstamp(&first), unstamp(&last))
            {
                changed.push((first, last + segment_reach));
            }
            tx.execute(
                "UPDATE work_segments SET driver = ?1 WHERE stream = ?2 AND driver IS NOT ?1",
                params![driver, stream],
            )?;
            if driver == "program" {
                tx.execute(
                    "DELETE FROM owner_prompts WHERE stream = ?1",
                    params![stream],
                )?;
            }
        }
        for segment in batch.segments {
            changed.push((segment.started_at, segment.last_at + segment_reach));
            super::owner_time::rule_since_on(&tx, segment.started_at)?;
            let touch = segment.last_touch_at.map(stamp);
            let id = match segment.stored {
                Some(id) => {
                    tx.execute(
                        "UPDATE work_segments
                            SET project_id = ?1, source = ?2, last_at = ?3,
                                last_touch_at = COALESCE(?4, last_touch_at)
                          WHERE id = ?5 AND stream = ?6",
                        params![
                            segment.project,
                            learned_text(segment.learned),
                            stamp(segment.last_at),
                            touch,
                            id,
                            stream
                        ],
                    )?;
                    id
                }
                None => {
                    tx.execute(
                        "INSERT INTO work_segments
                            (stream, driver, agent, project_id, started_at, last_at,
                             last_touch_at, source)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                        params![
                            stream,
                            driver,
                            batch.agent,
                            segment.project,
                            stamp(segment.started_at),
                            stamp(segment.last_at),
                            touch,
                            learned_text(segment.learned)
                        ],
                    )?;
                    tx.last_insert_rowid()
                }
            };
            for (minute, records, touches) in &segment.activity {
                tx.execute(
                    "INSERT INTO segment_activity (segment_id, minute, records, touches)
                     VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT(segment_id, minute) DO UPDATE SET
                        records = records + excluded.records,
                        touches = touches + excluded.touches",
                    params![id, minute, records, touches],
                )?;
            }
        }
        for at in batch.prompts {
            changed.push((*at, *at + prompt_reach));
            tx.execute(
                "INSERT OR IGNORE INTO owner_prompts (stream, at) VALUES (?1, ?2)",
                params![stream, stamp(*at)],
            )?;
        }
        for (first, last) in changed {
            super::owner_time::mark_dirty_on(&tx, first, last)?;
        }
        let next = batch.next;
        tx.execute(
            "INSERT INTO observation_checkpoints
                (stream, generation, offset, file_id, prefix_len, prefix_hash, anchor_hash,
                 driver, project_id, candidate_id, folder_id, placed, source, last_kind, last_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
             ON CONFLICT(stream) DO UPDATE SET
                generation = excluded.generation, offset = excluded.offset,
                file_id = excluded.file_id, prefix_len = excluded.prefix_len,
                prefix_hash = excluded.prefix_hash, anchor_hash = excluded.anchor_hash,
                driver = excluded.driver, project_id = excluded.project_id,
                candidate_id = excluded.candidate_id, folder_id = excluded.folder_id,
                placed = excluded.placed, source = excluded.source,
                last_kind = excluded.last_kind, last_at = excluded.last_at",
            params![
                stream,
                next.cursor.generation,
                next.cursor.offset,
                next.cursor.file_id,
                next.cursor.prefix_len,
                next.cursor.prefix_hash,
                next.cursor.anchor_hash,
                next.program
                    .map(|program| if program { "program" } else { "owner" }),
                next.state.project,
                next.state.candidate,
                next.state.folder,
                next.state.placed,
                next.state.source.as_str(),
                next.last_kind.as_str(),
                next.last_at.map(stamp),
            ],
        )?;
        tx.commit()?;
        Ok(true)
    }

    /// When the stretch of work a transcript is in began: the start of
    /// the first of its last segments that follow one another without a
    /// pause.
    pub fn session_start(&self, stream: &str) -> Result<Option<DateTime<Utc>>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        session_start_on(&conn, stream)
    }

    /// The sessions that had a record since `since`, as their checkpoints
    /// hold them: what the live state is rebuilt from at a start.
    pub fn live_sessions(
        &self,
        since: DateTime<Utc>,
    ) -> Result<Vec<(String, crate::live::Session)>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let mut stmt = conn.prepare(
            "SELECT c.stream, c.driver, c.project_id, c.last_kind, c.last_at,
                    (SELECT agent FROM work_segments s
                      WHERE s.stream = c.stream ORDER BY s.id DESC LIMIT 1)
               FROM observation_checkpoints c
              WHERE c.last_at >= ?1 ORDER BY c.stream",
        )?;
        let rows = stmt
            .query_map(params![stamp(since)], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, Option<i64>>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, Option<String>>(5)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut sessions = Vec::new();
        for (stream, driver, project, kind, last, agent) in rows {
            let (Some(last_at), Some(agent)) = (unstamp(&last), agent) else {
                continue;
            };
            let started_at = session_start_on(&conn, &stream)?.unwrap_or(last_at);
            sessions.push((
                stream,
                crate::live::Session {
                    agent,
                    program: driver.as_deref() == Some("program"),
                    project,
                    last_kind: Kind::parse(&kind),
                    last_at,
                    started_at,
                },
            ));
        }
        Ok(sessions)
    }

    /// The time of agents between `from` and `to`, per project as merges
    /// resolve it (`None` is unknown): the union of the intervals of its
    /// segments, so two sessions on one project at once count once.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn agent_seconds(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<BTreeMap<Option<i64>, i64>> {
        Ok(self.agent_time(from, to)?.0)
    }

    /// The time of agents between `from` and `to`: per project, and in
    /// all. The total is the union over all segments, not the sum of the
    /// projects: agents at work on two projects at once are one stretch
    /// of time.
    pub fn agent_time(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<(BTreeMap<Option<i64>, i64>, i64)> {
        let canonical: HashMap<i64, i64> = self.project_canonical_ids()?;
        // What the owner corrected is applied here, after the rules: the
        // segments are as the rules wrote them.
        let corrections = self.corrections(&canonical)?;
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let mut stmt = conn.prepare(
            "SELECT stream, project_id, started_at, last_at FROM work_segments
              WHERE last_at >= ?1 AND started_at <= ?2",
        )?;
        let rows = stmt
            .query_map(params![stamp(from), stamp(to)], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<i64>>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut intervals: BTreeMap<Option<i64>, Vec<Interval>> = BTreeMap::new();
        for (stream, project, started, last) in rows {
            let (Some(started), Some(last)) = (unstamp(&started), unstamp(&last)) else {
                continue;
            };
            let project = project.map(|id| canonical.get(&id).copied().unwrap_or(id));
            let span = (started.timestamp(), last.timestamp());
            for (project, from, to) in corrections.pieces(&stream, project, span) {
                if let (Some(from), Some(to)) = (
                    DateTime::from_timestamp(from, 0),
                    DateTime::from_timestamp(to, 0),
                ) {
                    intervals.entry(project).or_default().push((from, to));
                }
            }
        }
        let all: Vec<Interval> = intervals.values().flatten().copied().collect();
        let by_project = intervals
            .into_iter()
            .map(|(project, intervals)| (project, union_seconds(&intervals, from, to)))
            .collect();
        Ok((by_project, union_seconds(&all, from, to)))
    }
}

#[cfg(test)]
#[path = "observation_tests.rs"]
mod tests;
