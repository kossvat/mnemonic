//! The owner's corrections of attribution (live-projects, design 9.1).
//!
//! A row with a stream and no range pins a transcript to a project. A row
//! with a range relabels that time: of one transcript, or, without a
//! stream, of the owner's own time (work that leaves no transcript can
//! only be assigned this way). No project is a project too: "none".
//!
//! Corrections are kept apart from the segments and applied when they are
//! read, so a replay of a transcript never loses one. Where two cover the
//! same time a range wins over a pin, and of two ranges the later made.
use anyhow::{Result, bail, ensure};
use chrono::{DateTime, NaiveDateTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::HashMap;

use super::Storage;
use crate::owner_time::Relabel;

pub(super) const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS attribution_overrides (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        stream TEXT,
        started_at TEXT,
        ended_at TEXT,
        project_id INTEGER REFERENCES projects(id),
        created_at TEXT NOT NULL DEFAULT (datetime('now')),
        CHECK ((started_at IS NULL) = (ended_at IS NULL)),
        CHECK (stream IS NOT NULL OR started_at IS NOT NULL)
    );
    CREATE INDEX IF NOT EXISTS idx_attribution_overrides_stream
        ON attribution_overrides(stream);
";

const STAMP: &str = "%Y-%m-%d %H:%M:%S";

fn stamp(at: DateTime<Utc>) -> String {
    at.format(STAMP).to_string()
}

fn unstamp(text: &str) -> Option<DateTime<Utc>> {
    NaiveDateTime::parse_from_str(text, STAMP)
        .ok()
        .map(|at| at.and_utc())
}

/// One correction as it is kept. Its id says when it was made: a later
/// one has a greater id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Override {
    pub id: i64,
    pub stream: Option<String>,
    pub range: Option<(DateTime<Utc>, DateTime<Utc>)>,
    /// `None` is "no project".
    pub project: Option<i64>,
}

/// The corrections of a store, as the readers of time apply them: with
/// every project under the one merges resolve it to.
#[derive(Debug, Clone, Default)]
pub struct Corrections {
    /// By stream: the last pin, as the project it names.
    pins: HashMap<String, Option<i64>>,
    /// By stream: the ranges, in the order they were made.
    ranges: HashMap<String, Vec<Relabel>>,
    /// The ranges of the owner's own time.
    pub owner: Vec<Relabel>,
}

impl Corrections {
    pub fn pin(&self, stream: &str) -> Option<Option<i64>> {
        self.pins.get(stream).copied()
    }

    pub fn ranges(&self, stream: &str) -> &[Relabel] {
        self.ranges.get(stream).map_or(&[], Vec::as_slice)
    }

    /// The pieces of a segment of `stream` on `project` from `from` to
    /// `to` (seconds), each with the project the corrections give it.
    pub fn pieces(
        &self,
        stream: &str,
        project: Option<i64>,
        (from, to): (i64, i64),
    ) -> Vec<(Option<i64>, i64, i64)> {
        let base = self.pin(stream).unwrap_or(project);
        let ranges = self.ranges(stream);
        let mut cuts: Vec<i64> = vec![from, to];
        for range in ranges {
            cuts.extend(
                [range.from, range.to]
                    .into_iter()
                    .filter(|at| from < *at && *at < to),
            );
        }
        cuts.sort_unstable();
        cuts.dedup();
        let mut pieces: Vec<(Option<i64>, i64, i64)> = Vec::new();
        for pair in cuts.windows(2) {
            let relabel = ranges
                .iter()
                .filter(|range| range.from <= pair[0] && pair[0] < range.to)
                .max_by_key(|range| range.order);
            let project = relabel.map_or(base, |range| range.project);
            match pieces.last_mut() {
                Some((known, _, end)) if *known == project && *end == pair[0] => *end = pair[1],
                _ => pieces.push((project, pair[0], pair[1])),
            }
        }
        // A segment of no length is still a segment.
        if pieces.is_empty() {
            pieces.push((base, from, to));
        }
        pieces
    }
}

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Override> {
    let (started, ended): (Option<String>, Option<String>) = (r.get(2)?, r.get(3)?);
    Ok(Override {
        id: r.get(0)?,
        stream: r.get(1)?,
        range: started
            .as_deref()
            .and_then(unstamp)
            .zip(ended.as_deref().and_then(unstamp)),
        project: r.get(4)?,
    })
}

/// Mark the days a correction changes the owner's time of: those of its
/// range, or, for a pin, those of the segments and prompts of its
/// transcript.
fn mark(conn: &Connection, made: &Override) -> Result<()> {
    if let Some((from, to)) = made.range {
        super::owner_time::mark_dirty_on(conn, from, to)?;
        return Ok(());
    }
    use super::owner_time::{PROMPT_REACH_SECS, SEGMENT_REACH_SECS};
    for (sql, reach) in [
        (
            "SELECT MIN(started_at), MAX(last_at) FROM work_segments WHERE stream = ?1",
            SEGMENT_REACH_SECS,
        ),
        (
            "SELECT MIN(at), MAX(at) FROM owner_prompts WHERE stream = ?1",
            PROMPT_REACH_SECS,
        ),
    ] {
        let kept: (Option<String>, Option<String>) =
            conn.query_row(sql, params![made.stream], |r| Ok((r.get(0)?, r.get(1)?)))?;
        if let (Some(first), Some(last)) = kept
            && let (Some(first), Some(last)) = (unstamp(&first), unstamp(&last))
        {
            let reach = chrono::Duration::seconds(reach);
            super::owner_time::mark_dirty_on(conn, first, last + reach)?;
        }
    }
    Ok(())
}

impl Storage {
    /// Keep a correction and mark the days it changes. A stream pins or
    /// relabels a transcript; a range without one relabels the owner's
    /// time. The id of the new row.
    pub fn override_add(
        &self,
        stream: Option<&str>,
        range: Option<(DateTime<Utc>, DateTime<Utc>)>,
        project: Option<i64>,
    ) -> Result<i64> {
        ensure!(
            stream.is_some() || range.is_some(),
            "a correction names a transcript, a range of time, or both"
        );
        if let Some((from, to)) = range {
            ensure!(from < to, "a range ends after it begins");
            // Every day of a correction is made again: one too long to
            // mark whole is not kept.
            ensure!(
                (to - from).num_days() < super::owner_time::MOST_DAYS,
                "a range is shorter than ten years"
            );
        }
        if let Some(stream) = stream {
            ensure!(
                crate::redaction::check_identity(stream).is_ok(),
                "correction stream refused ({})",
                crate::redaction::RedactionError::SensitiveContent.code()
            );
        }
        let mut conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        // The write lock from the start: this reads before it writes.
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        if let Some(project) = project {
            let known: Option<i64> = tx
                .query_row(
                    "SELECT id FROM projects WHERE id = ?1",
                    params![project],
                    |r| r.get(0),
                )
                .optional()?;
            if known.is_none() {
                bail!("no such project");
            }
        }
        tx.execute(
            "INSERT INTO attribution_overrides (stream, started_at, ended_at, project_id)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                stream,
                range.map(|(from, _)| stamp(from)),
                range.map(|(_, to)| stamp(to)),
                project
            ],
        )?;
        let id = tx.last_insert_rowid();
        let made = Override {
            id,
            stream: stream.map(str::to_owned),
            range,
            project,
        };
        mark(&tx, &made)?;
        tx.commit()?;
        Ok(id)
    }

    /// Undo a correction and mark the days it changed. Whether there was
    /// one.
    pub fn override_remove(&self, id: i64) -> Result<bool> {
        let mut conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        // The write lock from the start: this reads before it writes.
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let kept = tx
            .query_row(
                "SELECT id, stream, started_at, ended_at, project_id
                   FROM attribution_overrides WHERE id = ?1",
                params![id],
                row,
            )
            .optional()?;
        let Some(kept) = kept else {
            return Ok(false);
        };
        tx.execute(
            "DELETE FROM attribution_overrides WHERE id = ?1",
            params![id],
        )?;
        mark(&tx, &kept)?;
        tx.commit()?;
        Ok(true)
    }

    /// Every correction, in the order they were made.
    pub fn overrides(&self) -> Result<Vec<Override>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let mut stmt = conn.prepare(
            "SELECT id, stream, started_at, ended_at, project_id
               FROM attribution_overrides ORDER BY id",
        )?;
        let rows = stmt.query_map([], row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The corrections as the readers of time apply them. `canonical`
    /// gives the project a row was merged into.
    pub(super) fn corrections(&self, canonical: &HashMap<i64, i64>) -> Result<Corrections> {
        let resolve = |id: Option<i64>| id.map(|id| canonical.get(&id).copied().unwrap_or(id));
        let mut out = Corrections::default();
        for made in self.overrides()? {
            let project = resolve(made.project);
            match (made.stream, made.range) {
                (Some(stream), None) => {
                    out.pins.insert(stream, project);
                }
                (stream, Some((from, to))) => {
                    let relabel = Relabel {
                        from: from.timestamp(),
                        to: to.timestamp(),
                        project,
                        order: made.id,
                    };
                    match stream {
                        Some(stream) => out.ranges.entry(stream).or_default().push(relabel),
                        None => out.owner.push(relabel),
                    }
                }
                (None, None) => {}
            }
        }
        Ok(out)
    }
}
