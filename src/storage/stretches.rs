//! Stretches of work: how long a transcript was written to without a
//! pause as long as the idle time. The Records page ranks them.
//!
//! A session of the store (`sessions`) groups memories, begins at the
//! first memory captured and is closed by a sweep; it was never a measure
//! of how long anyone worked. A stretch is: it is made of the times the
//! records of a transcript were written at, whatever they hold, begins at
//! the first record and ends at the last one before a pause. The live
//! reader extends it as records arrive, and a transcript read whole (the
//! history that was there before) gives the same stretches.
//!
//! A transcript is named by the stream the reader keeps its cursor under
//! (the watcher and the file's name), not by its path: a rollout moved
//! into the archive is the same transcript.
use anyhow::Result;
use chrono::{DateTime, Duration, Timelike, Utc};
use rusqlite::{OptionalExtension, params};

use super::{Storage, parse_session_timestamp};

/// The time format of the table: what SQLite's `datetime()` writes, in UTC.
const STAMP: &str = "%Y-%m-%d %H:%M:%S";

/// A stretch as the leaderboard reads it: its id, start and last time.
pub(super) type Span = (i64, DateTime<Utc>, DateTime<Utc>);

/// A stretch that was written to this recently is still going on.
pub const LIVE_WITHIN_SECS: i64 = 300;

/// Walk `written`, the times of records in the order of their file, from
/// the stretch `open` (its start and last time), and say what the
/// transcript's stretches are after it: `open` as it now stands, first,
/// when there was one, and the stretches begun after it.
///
/// A time ahead of `now` is no time a record was written at, and one at or
/// before the last time adds nothing: both are passed over. A time the
/// idle time or more after the last one begins a new stretch.
pub fn walk(
    open: Option<(DateTime<Utc>, DateTime<Utc>)>,
    written: &[DateTime<Utc>],
    idle: Duration,
    now: DateTime<Utc>,
) -> Vec<(DateTime<Utc>, DateTime<Utc>)> {
    let mut done = Vec::new();
    let mut current = open;
    for at in written {
        if *at > now {
            continue;
        }
        let Some(at) = at.with_nanosecond(0) else {
            continue;
        };
        match current {
            None => current = Some((at, at)),
            Some((_, last)) if at <= last => {}
            Some((start, last)) if at - last >= idle => {
                done.push((start, last));
                current = Some((at, at));
            }
            Some((start, _)) => current = Some((start, at)),
        }
    }
    done.extend(current);
    done
}

/// Stretches of one transcript that came from two readings of it, as one
/// set: two that overlap or touch, or lie less than the idle time apart,
/// are one. Overlap is judged by itself, so the same stretch read twice is
/// one stretch whatever the idle time, zero included.
pub fn joined(
    mut spans: Vec<(DateTime<Utc>, DateTime<Utc>)>,
    idle: Duration,
) -> Vec<(DateTime<Utc>, DateTime<Utc>)> {
    spans.sort();
    let mut out: Vec<(DateTime<Utc>, DateTime<Utc>)> = Vec::new();
    for (start, last) in spans {
        match out.last_mut() {
            Some((_, end)) if start <= *end || start - *end < idle => *end = (*end).max(last),
            _ => out.push((start, last)),
        }
    }
    out
}

impl Storage {
    /// The records of `transcript` the live reader just read were written
    /// at `written`: extend the transcript's last stretch, or begin new
    /// ones. Read again after a failed poll, the same times add nothing.
    pub fn extend_work_stretches(
        &self,
        transcript: &str,
        written: &[DateTime<Utc>],
        idle_timeout_secs: u64,
        now: DateTime<Utc>,
    ) -> Result<()> {
        if written.is_empty() {
            return Ok(());
        }
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        // The write lock from the start: a lock taken to read is refused,
        // not waited for, when it is raised after another process (a
        // backfill beside the daemon) committed in between.
        let tx =
            rusqlite::Transaction::new_unchecked(&conn, rusqlite::TransactionBehavior::Immediate)?;
        let latest: Option<(i64, String, String)> = tx
            .query_row(
                "SELECT id, started_at, last_at FROM work_stretches
                  WHERE transcript = ?1 ORDER BY last_at DESC, id DESC LIMIT 1",
                params![transcript],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let open = latest.as_ref().and_then(|(id, start, last)| {
            Some((
                *id,
                parse_session_timestamp(start)?,
                parse_session_timestamp(last)?,
            ))
        });
        let idle = Duration::seconds(idle_timeout_secs as i64);
        let mut stretches = walk(open.map(|(_, s, l)| (s, l)), written, idle, now).into_iter();
        if let Some((id, _, last)) = open
            && let Some((_, now_last)) = stretches.next()
            && now_last != last
        {
            tx.execute(
                "UPDATE work_stretches SET last_at = ?1 WHERE id = ?2",
                params![now_last.format(STAMP).to_string(), id],
            )?;
        }
        for (start, last) in stretches {
            tx.execute(
                "INSERT INTO work_stretches (transcript, started_at, last_at)
                 VALUES (?1, ?2, ?3)",
                params![
                    transcript,
                    start.format(STAMP).to_string(),
                    last.format(STAMP).to_string()
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// `written` is every record of `transcript` there was when it was read
    /// from its first byte: the stretches those times give are joined with
    /// the ones the transcript already has. Used to bring in the
    /// transcripts that were there before stretches were kept, while the
    /// live reader goes on: what it kept of records written after the
    /// reading began stays. Reading the same transcript again changes
    /// nothing. Returns how many stretches the transcript has.
    pub fn bring_in_work_stretches(
        &self,
        transcript: &str,
        written: &[DateTime<Utc>],
        idle_timeout_secs: u64,
        now: DateTime<Utc>,
    ) -> Result<usize> {
        let idle = Duration::seconds(idle_timeout_secs as i64);
        let mut spans = walk(None, written, idle, now);
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let tx = conn.unchecked_transaction()?;
        {
            let mut stmt =
                tx.prepare("SELECT started_at, last_at FROM work_stretches WHERE transcript = ?1")?;
            let kept = stmt
                .query_map(params![transcript], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            spans.extend(kept.into_iter().filter_map(|(start, last)| {
                Some((
                    parse_session_timestamp(&start)?,
                    parse_session_timestamp(&last)?,
                ))
            }));
        }
        let stretches = joined(spans, idle);
        tx.execute(
            "DELETE FROM work_stretches WHERE transcript = ?1",
            params![transcript],
        )?;
        for (start, last) in &stretches {
            tx.execute(
                "INSERT INTO work_stretches (transcript, started_at, last_at)
                 VALUES (?1, ?2, ?3)",
                params![
                    transcript,
                    start.format(STAMP).to_string(),
                    last.format(STAMP).to_string()
                ],
            )?;
        }
        tx.commit()?;
        Ok(stretches.len())
    }

    /// Whether the transcripts that were there before stretches were kept
    /// have been brought in.
    pub fn work_stretches_backfilled(&self) -> Result<bool> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let done: Option<String> = conn
            .query_row(
                "SELECT value FROM work_stretch_state WHERE name = 'backfilled_at'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        Ok(done.is_some())
    }

    /// Say that the earlier transcripts have been brought in, at `at`.
    pub fn mark_work_stretches_backfilled(&self, at: DateTime<Utc>) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        conn.execute(
            "INSERT INTO work_stretch_state (name, value) VALUES ('backfilled_at', ?1)
             ON CONFLICT(name) DO UPDATE SET value = excluded.value",
            params![at.format(STAMP).to_string()],
        )?;
        Ok(())
    }

    /// Every stretch: its id, start and last time. Rows whose times cannot
    /// be read are left out.
    pub(super) fn work_stretch_spans(&self) -> Result<Vec<Span>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let mut stmt = conn.prepare("SELECT id, started_at, last_at FROM work_stretches")?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows
            .into_iter()
            .filter_map(|(id, start, last)| {
                let (start, last) = (
                    parse_session_timestamp(&start)?,
                    parse_session_timestamp(&last)?,
                );
                (last >= start).then_some((id, start, last))
            })
            .collect())
    }
}

#[cfg(test)]
#[path = "stretches_tests.rs"]
mod tests;
