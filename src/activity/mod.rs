//! Work-activity tracking — accurate daily "time worked" from input
//! activity, kept in its OWN SQLite file (`activity.db`) next to the
//! memory DB.
//!
//! ## Why a separate DB file (not memory.db)
//!
//! The sampler writes every ~30s. memory.db carries FTS5 + HNSW and
//! serves latency-sensitive retrieval; mixing a high-frequency write
//! stream into it would invite lock contention and bloat. Same daemon
//! process, separate file — the bounded context is isolated in storage
//! as well as in code.
//!
//! ## What "worked" means
//!
//! A *work session* is a continuous span of input activity where the
//! gap between inputs never exceeds `idle_threshold`. The daily total
//! is the sum of session durations. When the user steps away, the
//! session ends at the *last input*, not at the moment we notice —
//! so the idle grace window is never counted. Sleep is handled for
//! free: the process suspends, and on wake the first tick sees a huge
//! idle value and closes the stale session.
//!
//! No keystrokes, no window titles, no screenshots — only an idle
//! counter (seconds since last HID event) drives the state machine.

use std::path::Path;
use std::sync::Mutex;

use anyhow::Result;
use chrono::{DateTime, Local, NaiveDate, TimeZone, Utc};
use rusqlite::Connection;

/// An open or closed span of continuous work.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkSession {
    pub id: String,
    pub started_at: DateTime<Utc>,
    /// Wall-clock time of the most recent input we attributed to this
    /// session. While the session is open this is advanced every tick;
    /// when it closes, the session's effective end is exactly this.
    pub last_input_at: DateTime<Utc>,
}

/// What a single tick should do, decided purely from the current open
/// session (if any), the reported idle seconds, and `now`. Pure and
/// side-effect-free so the whole state machine is unit-testable
/// without the platform idle FFI.
#[derive(Debug, Clone, PartialEq)]
pub enum TickAction {
    /// Away with nothing open — record nothing.
    NoOp,
    /// Begin a new session.
    Open(WorkSession),
    /// Advance the open session's last-input watermark.
    Extend {
        id: String,
        last_input_at: DateTime<Utc>,
    },
    /// Close the open session (it already holds its true end time).
    Close { id: String },
    /// A gap longer than the threshold appeared since the last input
    /// (the user stepped away / the Mac slept) and there is fresh
    /// activity now: close the stale span and open a new one.
    Rotate { close_id: String, open: WorkSession },
}

/// Pure tick decision. `idle_secs` is seconds since the last HID input
/// (from the platform). `now` is wall clock. `threshold_secs` is the
/// inactivity gap that splits one session from the next.
///
/// `make_id` mints a fresh session id for the Open/Rotate cases —
/// injected so tests stay deterministic (no RNG in the pure core).
pub fn decide(
    open: Option<&WorkSession>,
    idle_secs: f64,
    now: DateTime<Utc>,
    threshold_secs: u64,
    make_id: impl FnOnce() -> String,
) -> TickAction {
    let threshold = threshold_secs as f64;
    let active = idle_secs < threshold;
    // Absolute time of the last input the OS knows about. Clamp the
    // idle value at 0 — a negative idle would be nonsensical and would
    // push last_input into the future.
    let last_input = now - chrono::Duration::milliseconds((idle_secs.max(0.0) * 1000.0) as i64);

    match open {
        None => {
            if active {
                let id = make_id();
                TickAction::Open(WorkSession {
                    id,
                    started_at: last_input,
                    last_input_at: last_input,
                })
            } else {
                TickAction::NoOp
            }
        }
        Some(s) => {
            // Time elapsed since we last recorded input for this open
            // session. A value over the threshold means a break slipped
            // between ticks (away long enough, a sleep/wake, or a daemon
            // stall) even if idle is small right now.
            let gap = (now - s.last_input_at).num_milliseconds() as f64 / 1000.0;
            if gap > threshold {
                if active {
                    let id = make_id();
                    TickAction::Rotate {
                        close_id: s.id.clone(),
                        open: WorkSession {
                            id,
                            started_at: last_input,
                            last_input_at: last_input,
                        },
                    }
                } else {
                    TickAction::Close { id: s.id.clone() }
                }
            } else if active {
                TickAction::Extend {
                    id: s.id.clone(),
                    last_input_at: last_input,
                }
            } else {
                TickAction::Close { id: s.id.clone() }
            }
        }
    }
}

/// One day's worked total, used by the CLI / API for the history graph.
#[derive(Debug, Clone, PartialEq)]
pub struct DailyTotal {
    /// Local date, `YYYY-MM-DD`.
    pub date: String,
    pub seconds: f64,
}

/// A single work block (one session, clipped to the day it's shown on).
/// Powers the day's "session timeline" lane in the widget.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionBlock {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

impl SessionBlock {
    pub fn seconds(&self) -> f64 {
        (self.end - self.start).num_milliseconds() as f64 / 1000.0
    }
}

/// Everything the widget shows when a single day is selected.
#[derive(Debug, Clone, PartialEq)]
pub struct DayDetail {
    /// Local date, `YYYY-MM-DD`.
    pub date: String,
    pub total_seconds: f64,
    pub session_count: usize,
    pub longest_seconds: f64,
    /// First work start / last work end of the day (clipped to the day),
    /// i.e. the "span". None when the day has no work.
    pub first_start: Option<DateTime<Utc>>,
    pub last_end: Option<DateTime<Utc>>,
    /// Day-clipped work blocks, ordered, for the timeline lane.
    pub blocks: Vec<SessionBlock>,
}

/// Rolling 7-day work summary for the hero's honest stat chip
/// ("This week 26h 38m · +2h 16m vs avg · Best day Tue").
#[derive(Debug, Clone, PartialEq)]
pub struct WeekStats {
    /// Sum over the last 7 local days (incl. today).
    pub total_seconds: f64,
    /// Average of prior whole 7-day windows that had any activity, or
    /// None if there isn't enough history to compare honestly.
    pub avg_seconds: Option<f64>,
    /// `total_seconds - avg_seconds`, None when avg is None.
    pub delta_vs_avg_seconds: Option<f64>,
    /// Short weekday name of the busiest of the last 7 days ("Tue"), and
    /// its seconds. None when the window is entirely empty.
    pub best_weekday: Option<String>,
    pub best_weekday_seconds: f64,
}

/// Minimum duration for a block to count as a real, displayable session.
/// Sub-minute blocks are sampling / daemon-restart noise — excluded from
/// the timeline, session count, longest, and span, but their seconds still
/// land in the daily total.
const MIN_SESSION_SECS: f64 = 60.0;

/// SQLite-backed activity store (own file, thread-safe via Mutex).
pub struct ActivityStore {
    conn: Mutex<Connection>,
    /// Closed sessions shorter than this are deleted instead of kept
    /// (phantom idle-counter blips from peripherals/utilities). Only the
    /// daemon's writer store sets this; read/reporting opens keep 0 so
    /// they never delete anything.
    min_session_secs: f64,
}

impl ActivityStore {
    /// Open (creating if needed) the activity DB at `path` and run the
    /// schema migration. This is read-only with respect to live session
    /// state: status/reporting commands use it without closing the
    /// daemon's current session.
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_inner(path)
    }

    /// Open for the daemon sampler. Any session left in the `open`
    /// state by a previous daemon run is finalized at its stored end —
    /// we never resume across a process restart, which guarantees
    /// downtime can't be folded into a work span.
    ///
    /// `min_session_secs`: closed sessions shorter than this are dropped
    /// (phantom idle blips). 0 keeps everything.
    pub fn open_for_daemon(path: &Path, min_session_secs: u64) -> Result<Self> {
        let mut store = Self::open_inner(path)?;
        store.min_session_secs = min_session_secs as f64;
        store.close_open_sessions()?;
        Ok(store)
    }

    fn open_inner(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
            tighten_default_data_dir(parent);
        }
        let conn = Connection::open(path)?;
        tighten_owner_only_file(path);
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        let store = Self {
            conn: Mutex::new(conn),
            min_session_secs: 0.0,
        };
        store.init_schema()?;
        Ok(store)
    }

    fn init_schema(&self) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS work_sessions (
                id          TEXT PRIMARY KEY,
                started_at  TEXT NOT NULL,
                ended_at    TEXT NOT NULL,
                open        INTEGER NOT NULL DEFAULT 0
            );
            CREATE INDEX IF NOT EXISTS idx_work_sessions_started
                ON work_sessions(started_at);
            CREATE INDEX IF NOT EXISTS idx_work_sessions_open
                ON work_sessions(open) WHERE open = 1;

            -- Project-time attribution (recomputable per local day).
            CREATE TABLE IF NOT EXISTS project_attribution (
                day          TEXT NOT NULL,
                project_key  TEXT NOT NULL,
                seconds      REAL NOT NULL,
                confidence   TEXT NOT NULL,
                PRIMARY KEY (day, project_key)
            );
            CREATE INDEX IF NOT EXISTS idx_proj_attr_day
                ON project_attribution(day);
            CREATE TABLE IF NOT EXISTS unattributed_time (
                day      TEXT PRIMARY KEY,
                seconds  REAL NOT NULL
            );",
        )?;
        Ok(())
    }

    /// Finalize every still-open session. Called on startup so a crash
    /// or restart can never leave a span that later ticks would extend
    /// across the downtime. Sessions below the minimum are dropped
    /// rather than finalized.
    pub fn close_open_sessions(&self) -> Result<usize> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        if self.min_session_secs > 0.0 {
            conn.execute(
                "DELETE FROM work_sessions WHERE open = 1
                 AND (julianday(ended_at) - julianday(started_at)) * 86400.0 < ?1",
                [self.min_session_secs],
            )?;
        }
        let n = conn.execute("UPDATE work_sessions SET open = 0 WHERE open = 1", [])?;
        Ok(n)
    }

    /// Finalize one session by id: drop it when shorter than the minimum,
    /// otherwise mark it closed. The duration check runs on the STORED
    /// timestamps, so it is exact regardless of tick timing.
    fn finalize_session(&self, conn: &Connection, id: &str) -> Result<()> {
        if self.min_session_secs > 0.0 {
            let dropped = conn.execute(
                "DELETE FROM work_sessions WHERE id = ?1
                 AND (julianday(ended_at) - julianday(started_at)) * 86400.0 < ?2",
                rusqlite::params![id, self.min_session_secs],
            )?;
            if dropped > 0 {
                return Ok(());
            }
        }
        conn.execute("UPDATE work_sessions SET open = 0 WHERE id = ?1", [id])?;
        Ok(())
    }

    /// The currently-open session, if any.
    pub fn current_open(&self) -> Result<Option<WorkSession>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let row = conn
            .query_row(
                "SELECT id, started_at, ended_at FROM work_sessions WHERE open = 1
                 ORDER BY started_at DESC LIMIT 1",
                [],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                },
            )
            .ok();
        Ok(row.and_then(|(id, started, ended)| {
            Some(WorkSession {
                id,
                started_at: parse_dt(&started)?,
                last_input_at: parse_dt(&ended)?,
            })
        }))
    }

    /// Apply a decided action to the store. Returns the new open
    /// session (if one is open after the action) so the worker can hold
    /// it in memory without a re-read.
    pub fn apply(&self, action: &TickAction) -> Result<Option<WorkSession>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        match action {
            TickAction::NoOp => Ok(None),
            TickAction::Open(s) => {
                conn.execute(
                    "INSERT OR REPLACE INTO work_sessions (id, started_at, ended_at, open)
                     VALUES (?1, ?2, ?3, 1)",
                    rusqlite::params![
                        s.id,
                        s.started_at.to_rfc3339(),
                        s.last_input_at.to_rfc3339()
                    ],
                )?;
                Ok(Some(s.clone()))
            }
            TickAction::Extend { id, last_input_at } => {
                conn.execute(
                    "UPDATE work_sessions SET ended_at = ?2 WHERE id = ?1",
                    rusqlite::params![id, last_input_at.to_rfc3339()],
                )?;
                // Return the refreshed open session.
                let started: Option<String> = conn
                    .query_row(
                        "SELECT started_at FROM work_sessions WHERE id = ?1",
                        [id],
                        |r| r.get(0),
                    )
                    .ok();
                Ok(started.and_then(|st| {
                    Some(WorkSession {
                        id: id.clone(),
                        started_at: parse_dt(&st)?,
                        last_input_at: *last_input_at,
                    })
                }))
            }
            TickAction::Close { id } => {
                self.finalize_session(&conn, id)?;
                Ok(None)
            }
            TickAction::Rotate { close_id, open } => {
                self.finalize_session(&conn, close_id)?;
                conn.execute(
                    "INSERT OR REPLACE INTO work_sessions (id, started_at, ended_at, open)
                     VALUES (?1, ?2, ?3, 1)",
                    rusqlite::params![
                        open.id,
                        open.started_at.to_rfc3339(),
                        open.last_input_at.to_rfc3339()
                    ],
                )?;
                Ok(Some(open.clone()))
            }
        }
    }

    /// Seconds worked on the local calendar day that contains `now`
    /// (includes the currently-open session up to its last input).
    pub fn seconds_on_local_day(&self, day_offset: i64) -> Result<f64> {
        let day = Local::now().date_naive() + chrono::Duration::days(day_offset);
        self.seconds_on_local_date(day)
    }

    /// Seconds worked on a specific local calendar date. Sessions that
    /// cross midnight are split by overlap, so daily totals remain true
    /// even when work continues past 00:00.
    pub fn seconds_on_local_date(&self, day: NaiveDate) -> Result<f64> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        seconds_overlapping_local_date(&conn, day)
    }

    /// Per-day worked totals for the last `days` local days (oldest
    /// first), including days with zero so the graph has a continuous
    /// axis.
    pub fn daily_totals(&self, days: u32) -> Result<Vec<DailyTotal>> {
        let today = chrono::Local::now().date_naive();
        let mut out = Vec::with_capacity(days as usize);
        for i in (0..days as i64).rev() {
            let d = today - chrono::Duration::days(i);
            let key = d.format("%Y-%m-%d").to_string();
            let seconds = self.seconds_on_local_date(d)?.max(0.0);
            out.push(DailyTotal { date: key, seconds });
        }
        Ok(out)
    }

    /// Count of sessions overlapping the current local day — handy for
    /// the status line ("3 sessions, 4h 20m").
    pub fn session_count_today(&self) -> Result<i64> {
        let day = Local::now().date_naive();
        self.session_count_on_local_date(day)
    }

    /// Count of sessions that overlap a specific local calendar date.
    pub fn session_count_on_local_date(&self, day: NaiveDate) -> Result<i64> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let Some((start, end)) = local_day_bounds(day) else {
            return Ok(0);
        };
        count_overlapping_window(&conn, start, end)
    }

    /// Duration (seconds) of the currently-open session, or None if not
    /// in a session. `now` is injected so the value stays testable.
    pub fn current_session_seconds(&self, now: DateTime<Utc>) -> Result<Option<f64>> {
        Ok(self
            .current_open()?
            .map(|s| ((now - s.started_at).num_milliseconds() as f64 / 1000.0).max(0.0)))
    }

    /// Full detail for one local day: total, session count, longest
    /// block, span, and the day-clipped work blocks for the timeline.
    pub fn day_detail(&self, day: NaiveDate) -> Result<DayDetail> {
        let date = day.format("%Y-%m-%d").to_string();
        let Some((win_start, win_end)) = local_day_bounds(day) else {
            return Ok(DayDetail {
                date,
                total_seconds: 0.0,
                session_count: 0,
                longest_seconds: 0.0,
                first_start: None,
                last_end: None,
                blocks: vec![],
            });
        };

        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let mut stmt = conn.prepare(
            "SELECT started_at, ended_at FROM work_sessions
             WHERE julianday(ended_at) > julianday(?1)
               AND julianday(started_at) < julianday(?2)
             ORDER BY started_at",
        )?;
        let rows = stmt.query_map([win_start.to_rfc3339(), win_end.to_rfc3339()], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;

        let mut all: Vec<SessionBlock> = Vec::new();
        for row in rows.filter_map(|r| r.ok()) {
            let (Some(s), Some(e)) = (parse_dt(&row.0), parse_dt(&row.1)) else {
                continue;
            };
            // Clip each session to the day window so a span crossing
            // midnight only contributes its part of this day.
            let start = s.max(win_start);
            let end = e.min(win_end);
            if end > start {
                all.push(SessionBlock { start, end });
            }
        }

        // The DAILY TOTAL counts every scrap of activity (matches the
        // chart, which sums all overlaps) so worked-time stays honest.
        let total_seconds = all.iter().map(|b| b.seconds()).sum::<f64>().max(0.0);

        // But the DISPLAYED sessions (timeline, count, longest, span) drop
        // sub-minute micro-blocks. Those are sampling/daemon-restart noise
        // (0–2s bursts) that would clutter the timeline with invisible dots
        // and inflate the session count. Codex flagged 11 "sessions" today,
        // 6 of them <60s. Their seconds still live in total_seconds above.
        let blocks: Vec<SessionBlock> = all
            .into_iter()
            .filter(|b| b.seconds() >= MIN_SESSION_SECS)
            .collect();
        let longest_seconds = blocks.iter().map(|b| b.seconds()).fold(0.0, f64::max);
        let first_start = blocks.first().map(|b| b.start);
        let last_end = blocks.iter().map(|b| b.end).max();

        Ok(DayDetail {
            date,
            total_seconds,
            session_count: blocks.len(),
            longest_seconds,
            first_start,
            last_end,
            blocks,
        })
    }

    /// Rolling 7-day stats: total, average of prior whole 7-day windows
    /// with activity, delta vs that average, and the busiest weekday.
    pub fn week_stats(&self) -> Result<WeekStats> {
        let today = Local::now().date_naive();

        // This week = last 7 local days (incl today).
        let mut total = 0.0;
        let mut best_secs = 0.0;
        let mut best_day: Option<NaiveDate> = None;
        for i in 0..7i64 {
            let d = today - chrono::Duration::days(i);
            let s = self.seconds_on_local_date(d)?;
            total += s;
            if s > best_secs {
                best_secs = s;
                best_day = Some(d);
            }
        }

        // Average of prior whole 7-day windows (up to 4) that had any
        // activity — keeps "vs avg" honest and ignores empty history.
        let mut prior_totals: Vec<f64> = Vec::new();
        for w in 1..=4i64 {
            let mut wt = 0.0;
            for i in 0..7i64 {
                let d = today - chrono::Duration::days(w * 7 + i);
                wt += self.seconds_on_local_date(d)?;
            }
            if wt > 0.0 {
                prior_totals.push(wt);
            }
        }
        let avg_seconds = if prior_totals.is_empty() {
            None
        } else {
            Some(prior_totals.iter().sum::<f64>() / prior_totals.len() as f64)
        };
        let delta_vs_avg_seconds = avg_seconds.map(|a| total - a);

        let best_weekday = best_day
            .filter(|_| best_secs > 0.0)
            .map(|d| d.format("%a").to_string());

        Ok(WeekStats {
            total_seconds: total,
            avg_seconds,
            delta_vs_avg_seconds,
            best_weekday,
            best_weekday_seconds: best_secs,
        })
    }

    /// Canonical JSON for a single day — shared by the CLI and the HTTP
    /// API so both surfaces speak the identical contract. Includes
    /// RFC3339 instants plus pre-formatted local labels (so clients need
    /// no timezone math).
    pub fn day_value(&self, day: NaiveDate) -> Result<serde_json::Value> {
        let d = self.day_detail(day)?;
        let span_human = match (d.first_start, d.last_end) {
            (Some(s), Some(e)) => Some(format!("{}–{}", local_clock(&s), local_clock(&e))),
            _ => None,
        };
        Ok(serde_json::json!({
            "date": d.date,
            "total_seconds": d.total_seconds.round() as i64,
            "total_human": fmt_hm(d.total_seconds),
            "sessions": d.session_count,
            "longest_seconds": d.longest_seconds.round() as i64,
            "longest_human": fmt_hm(d.longest_seconds),
            "span_human": span_human,
            "blocks": d.blocks.iter().map(|b| serde_json::json!({
                "start": b.start.to_rfc3339(),
                "end": b.end.to_rfc3339(),
                "start_local": local_clock(&b.start),
                "end_local": local_clock(&b.end),
                "seconds": b.seconds().round() as i64,
            })).collect::<Vec<_>>(),
        }))
    }

    /// Canonical JSON for the widget's whole main screen in one call:
    /// worked-today, live session, week stats, today's detail/timeline,
    /// and the 7-day chart series.
    pub fn summary_value(&self) -> Result<serde_json::Value> {
        let now = Utc::now();
        let today = Local::now().date_naive();
        let worked = self.seconds_on_local_day(0)?;
        let session = self.current_session_seconds(now)?;
        let week = self.week_stats()?;
        let days = self.daily_totals(7)?;

        Ok(serde_json::json!({
            "worked_today": {
                "seconds": worked.round() as i64,
                "human": fmt_hm(worked),
            },
            "in_session": session.is_some(),
            "session_seconds": session.map(|s| s.round() as i64),
            "session_human": session.map(fmt_hm),
            "week": {
                "total_seconds": week.total_seconds.round() as i64,
                "total_human": fmt_hm(week.total_seconds),
                "delta_vs_avg_seconds": week.delta_vs_avg_seconds.map(|d| d.round() as i64),
                "delta_human": week.delta_vs_avg_seconds.map(fmt_signed_hm),
                "best_weekday": week.best_weekday,
            },
            "today": self.day_value(today)?,
            "days": days.iter().map(|t| serde_json::json!({
                "date": t.date,
                "seconds": t.seconds.round() as i64,
                "human": fmt_hm(t.seconds),
            })).collect::<Vec<_>>(),
        }))
    }
}

#[cfg(unix)]
fn tighten_default_data_dir(path: &Path) {
    if path
        .file_name()
        .is_some_and(|name| name == std::ffi::OsStr::new(".mnemonic"))
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700));
    }
}

#[cfg(not(unix))]
fn tighten_default_data_dir(_path: &Path) {}

#[cfg(unix)]
fn tighten_owner_only_file(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn tighten_owner_only_file(_path: &Path) {}

/// Format a UTC instant as a local clock label like "9:48am".
fn local_clock(dt: &DateTime<Utc>) -> String {
    dt.with_timezone(&Local).format("%-I:%M%P").to_string()
}

/// "+2h 16m" / "−45m" — signed duration for the "vs avg" chip.
fn fmt_signed_hm(seconds: f64) -> String {
    let sign = if seconds < 0.0 { "−" } else { "+" };
    format!("{sign}{}", fmt_hm(seconds.abs()))
}

// ── Project-time attribution (read/write over activity.db) ─────────────

/// Per-project attributed time over the recent window, for the widget.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectTimeRow {
    pub project_key: String,
    pub today_seconds: f64,
    pub week_seconds: f64,
    pub week: [f64; 7], // last 7 local days, oldest first
    pub confidence: Option<String>,
}

/// Project time + the honest "couldn't attribute" bucket.
#[derive(Debug, Clone, Default)]
pub struct ProjectTimeData {
    pub rows: Vec<ProjectTimeRow>,
    pub unattributed_today: f64,
    pub unattributed_week: f64,
}

/// One local day's attribution: `(project_key, seconds, confidence)` rows
/// sorted by seconds desc, plus that day's unattributed seconds.
pub type DayAttribution = (Vec<(String, f64, Option<String>)>, f64);

/// The confidence level holding the most attributed seconds. Ties break toward
/// the stronger level (high > medium > low) so an even split never under-reports.
/// Returns None only when there's no attributed time.
fn dominant_confidence(conf_secs: &std::collections::HashMap<String, f64>) -> Option<String> {
    let rank = |c: &str| match c {
        "high" => 3,
        "medium" => 2,
        "low" => 1,
        _ => 0,
    };
    conf_secs
        .iter()
        .max_by(|a, b| {
            a.1.partial_cmp(b.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| rank(a.0).cmp(&rank(b.0)))
        })
        .map(|(c, _)| c.clone())
}

impl ActivityStore {
    /// Day-clipped work sessions (the displayed ones, ≥ MIN_SESSION_SECS) as
    /// (start, end) — the units attribution runs over.
    pub fn sessions_on_local_date(
        &self,
        day: NaiveDate,
    ) -> Result<Vec<(DateTime<Utc>, DateTime<Utc>)>> {
        let d = self.day_detail(day)?;
        Ok(d.blocks.iter().map(|b| (b.start, b.end)).collect())
    }

    /// Build the day's work sessions, each with its **direct** per-session
    /// attribution (padded signal window + `attribute_session`). Shared by the
    /// live recompute and the `attribute carry` dry-run so the two computations
    /// can never drift. `get_signals(start, end)` is injected to keep this
    /// decoupled from the memory Storage and unit-testable.
    pub fn day_sessions_with_signals<F>(
        &self,
        day: NaiveDate,
        cfg: &crate::attribution::AttribCfg,
        pad: chrono::Duration,
        mut get_signals: F,
    ) -> Result<Vec<crate::attribution::DaySession>>
    where
        F: FnMut(DateTime<Utc>, DateTime<Utc>) -> Vec<crate::attribution::ProjectSignal>,
    {
        use crate::attribution::{DaySession, attribute_session};
        let sessions = self.sessions_on_local_date(day)?;
        let mut out = Vec::with_capacity(sessions.len());
        for (i, (s, e)) in sessions.iter().enumerate() {
            let (s, e) = (*s, *e);
            // Pad the signal window so memories saved a few minutes after a
            // work burst still attach — but clamp the pad to the MIDPOINT of
            // the gap to each neighbouring session, so a padded window can
            // never reach into another session and steal its project signal
            // (the idle threshold splits sessions as little as 3 min apart,
            // well under a 10 min pad).
            let lo = match i.checked_sub(1).and_then(|j| sessions.get(j)) {
                Some((_, prev_end)) => (s - pad).max(*prev_end + (s - *prev_end) / 2),
                None => s - pad,
            };
            let hi = match sessions.get(i + 1) {
                Some((next_start, _)) => (e + pad).min(e + (*next_start - e) / 2),
                None => e + pad,
            };
            let secs = (e - s).num_milliseconds() as f64 / 1000.0;
            let signals = get_signals(lo, hi);
            let direct = attribute_session(secs, &signals, cfg);
            out.push(DaySession {
                start: s,
                end: e,
                seconds: secs,
                direct,
            });
        }
        Ok(out)
    }

    /// Recompute project-time attribution for one local day: per-session direct
    /// attribution, then day-level **carry-forward** of the dominant project's
    /// no-signal neighbours (guarded by dominance / window / cap — see
    /// `attribution::carry_forward_day`). Confidence is by majority-seconds, so
    /// a carry-dominated project reads Low rather than borrowing a stray High.
    /// Idempotent: replaces the day's rows.
    pub fn recompute_day<F>(
        &self,
        day: NaiveDate,
        cfg: &crate::attribution::AttribCfg,
        pad: chrono::Duration,
        carry: &crate::attribution::CarryCfg,
        mem_times: &[(String, DateTime<Utc>)],
        get_signals: F,
    ) -> Result<()>
    where
        F: FnMut(DateTime<Utc>, DateTime<Utc>) -> Vec<crate::attribution::ProjectSignal>,
    {
        let day_sessions = self.day_sessions_with_signals(day, cfg, pad, get_signals)?;
        let result = crate::attribution::carry_forward_day(&day_sessions, mem_times, carry);

        let day_str = day.format("%Y-%m-%d").to_string();
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let tx = conn.unchecked_transaction()?;
        tx.execute("DELETE FROM project_attribution WHERE day = ?1", [&day_str])?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO project_attribution (day, project_key, seconds, confidence)
                 VALUES (?1, ?2, ?3, ?4)",
            )?;
            for (k, secs, conf) in &result.per_project {
                stmt.execute(rusqlite::params![day_str, k, secs, conf.as_str()])?;
            }
        }
        tx.execute(
            "INSERT OR REPLACE INTO unattributed_time (day, seconds) VALUES (?1, ?2)",
            rusqlite::params![day_str, result.unattributed_seconds],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Per-project time for today + the last 7 local days, plus the
    /// unattributed buckets. Reads only the precomputed attribution table.
    pub fn project_time(&self) -> Result<ProjectTimeData> {
        let today = Local::now().date_naive();
        let day_keys: Vec<String> = (0..7)
            .rev()
            .map(|i| {
                (today - chrono::Duration::days(i))
                    .format("%Y-%m-%d")
                    .to_string()
            })
            .collect();
        let today_str = today.format("%Y-%m-%d").to_string();
        let since = day_keys
            .first()
            .cloned()
            .unwrap_or_else(|| today_str.clone());

        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;

        use std::collections::HashMap;
        // project_key -> (week[7], today_secs, seconds-by-confidence)
        type Acc = ([f64; 7], f64, HashMap<String, f64>);
        let mut map: HashMap<String, Acc> = HashMap::new();
        {
            // ORDER BY makes the read deterministic; confidence is no longer
            // "whichever row landed first" but aggregated by time below.
            let mut stmt = conn.prepare(
                "SELECT day, project_key, seconds, confidence FROM project_attribution
                 WHERE day >= ?1 ORDER BY day, project_key",
            )?;
            let rows = stmt.query_map([&since], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, f64>(2)?,
                    r.get::<_, String>(3)?,
                ))
            })?;
            for row in rows.filter_map(|r| r.ok()) {
                let (day, key, secs, conf) = row;
                let idx = day_keys.iter().position(|d| d == &day);
                let ent = map.entry(key).or_insert(([0.0; 7], 0.0, HashMap::new()));
                if let Some(i) = idx {
                    ent.0[i] += secs;
                }
                if day == today_str {
                    ent.1 += secs;
                }
                *ent.2.entry(conf).or_insert(0.0) += secs;
            }
        }

        let mut rows: Vec<ProjectTimeRow> = map
            .into_iter()
            .map(
                |(project_key, (week, today_seconds, conf_secs))| ProjectTimeRow {
                    project_key,
                    today_seconds,
                    week_seconds: week.iter().sum(),
                    week,
                    // The confidence covering the most of this project's time over
                    // the window — ties broken toward the stronger level so a
                    // 50/50 high/low split doesn't report "low".
                    confidence: dominant_confidence(&conf_secs),
                },
            )
            .collect();
        rows.sort_by(|a, b| {
            b.week_seconds
                .partial_cmp(&a.week_seconds)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let unattributed_today: f64 = conn
            .query_row(
                "SELECT seconds FROM unattributed_time WHERE day = ?1",
                [&today_str],
                |r| r.get(0),
            )
            .unwrap_or(0.0);
        let unattributed_week: f64 = conn
            .query_row(
                "SELECT COALESCE(SUM(seconds), 0) FROM unattributed_time WHERE day >= ?1",
                [&since],
                |r| r.get(0),
            )
            .unwrap_or(0.0);

        Ok(ProjectTimeData {
            rows,
            unattributed_today,
            unattributed_week,
        })
    }

    /// One local day's attributed projects `(key, seconds, confidence)` sorted
    /// by seconds desc, plus that day's unattributed seconds. Reads only the
    /// precomputed tables — the journal's deterministic fact-collector uses it.
    pub fn project_attribution_for_day(&self, day: NaiveDate) -> Result<DayAttribution> {
        let day_str = day.format("%Y-%m-%d").to_string();
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let mut rows: Vec<(String, f64, Option<String>)> = {
            let mut stmt = conn.prepare(
                "SELECT project_key, seconds, confidence FROM project_attribution
                 WHERE day = ?1 ORDER BY seconds DESC",
            )?;
            stmt.query_map([&day_str], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, f64>(1)?,
                    r.get::<_, Option<String>>(2)?,
                ))
            })?
            .filter_map(|r| r.ok())
            .collect()
        };
        rows.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        let unattributed: f64 = conn
            .query_row(
                "SELECT seconds FROM unattributed_time WHERE day = ?1",
                [&day_str],
                |r| r.get(0),
            )
            .unwrap_or(0.0);
        Ok((rows, unattributed))
    }
}

/// Local-day → UTC `[start, end)` bounds. Public so the journal can read a
/// day's memory window with the same local-midnight semantics as activity.
pub fn local_day_bounds_utc(day: NaiveDate) -> Option<(DateTime<Utc>, DateTime<Utc>)> {
    local_day_bounds(day)
}

fn local_day_bounds(day: NaiveDate) -> Option<(DateTime<Utc>, DateTime<Utc>)> {
    let start_naive = day.and_hms_opt(0, 0, 0)?;
    let end_naive = (day + chrono::Duration::days(1)).and_hms_opt(0, 0, 0)?;
    let start = Local
        .from_local_datetime(&start_naive)
        .earliest()?
        .with_timezone(&Utc);
    let end = Local
        .from_local_datetime(&end_naive)
        .earliest()?
        .with_timezone(&Utc);
    Some((start, end))
}

fn seconds_overlapping_local_date(conn: &Connection, day: NaiveDate) -> Result<f64> {
    let Some((start, end)) = local_day_bounds(day) else {
        return Ok(0.0);
    };
    seconds_overlapping_window(conn, start, end)
}

fn seconds_overlapping_window(
    conn: &Connection,
    window_start: DateTime<Utc>,
    window_end: DateTime<Utc>,
) -> Result<f64> {
    let mut stmt = conn.prepare(
        "SELECT started_at, ended_at FROM work_sessions
         WHERE julianday(ended_at) > julianday(?1)
           AND julianday(started_at) < julianday(?2)",
    )?;
    let rows = stmt.query_map([window_start.to_rfc3339(), window_end.to_rfc3339()], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })?;

    let mut total = 0.0;
    for row in rows.filter_map(|r| r.ok()) {
        let (started, ended) = row;
        let Some(started_at) = parse_dt(&started) else {
            continue;
        };
        let Some(ended_at) = parse_dt(&ended) else {
            continue;
        };
        let overlap_start = started_at.max(window_start);
        let overlap_end = ended_at.min(window_end);
        if overlap_end > overlap_start {
            total += (overlap_end - overlap_start).num_milliseconds() as f64 / 1000.0;
        }
    }
    Ok(total.max(0.0))
}

fn count_overlapping_window(
    conn: &Connection,
    window_start: DateTime<Utc>,
    window_end: DateTime<Utc>,
) -> Result<i64> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM work_sessions
         WHERE julianday(ended_at) > julianday(?1)
           AND julianday(started_at) < julianday(?2)",
        [window_start.to_rfc3339(), window_end.to_rfc3339()],
        |r| r.get(0),
    )?;
    Ok(n)
}

fn parse_dt(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

/// Human formatting for durations: `4h 20m`, `45m`, `0m`.
pub fn fmt_hm(seconds: f64) -> String {
    let total_min = (seconds / 60.0).round() as i64;
    let h = total_min / 60;
    let m = total_min % 60;
    if h > 0 {
        format!("{h}h {m:02}m")
    } else {
        format!("{m}m")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dt(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }
    fn fixed_id() -> String {
        "sess-1".to_string()
    }

    #[test]
    fn decide_opens_when_active_and_nothing_open() {
        let now = dt("2026-05-30T10:00:00Z");
        let action = decide(None, 5.0, now, 180, fixed_id);
        match action {
            TickAction::Open(s) => {
                assert_eq!(s.id, "sess-1");
                // started ~5s ago
                assert!((now - s.started_at).num_seconds() == 5);
            }
            other => panic!("expected Open, got {other:?}"),
        }
    }

    #[test]
    fn decide_noop_when_away_and_nothing_open() {
        let now = dt("2026-05-30T10:00:00Z");
        assert_eq!(decide(None, 600.0, now, 180, fixed_id), TickAction::NoOp);
    }

    #[test]
    fn decide_extends_open_session_while_active() {
        let now = dt("2026-05-30T10:00:30Z");
        let open = WorkSession {
            id: "s".into(),
            started_at: dt("2026-05-30T10:00:00Z"),
            last_input_at: dt("2026-05-30T10:00:15Z"),
        };
        match decide(Some(&open), 2.0, now, 180, fixed_id) {
            TickAction::Extend { id, last_input_at } => {
                assert_eq!(id, "s");
                assert_eq!(last_input_at, dt("2026-05-30T10:00:28Z"));
            }
            other => panic!("expected Extend, got {other:?}"),
        }
    }

    #[test]
    fn decide_closes_when_idle_crosses_threshold() {
        let now = dt("2026-05-30T10:05:00Z");
        let open = WorkSession {
            id: "s".into(),
            started_at: dt("2026-05-30T10:00:00Z"),
            last_input_at: dt("2026-05-30T10:01:30Z"),
        };
        // idle 200s > 180 threshold, gap from last_input is small enough
        // it's the idle branch that closes.
        assert_eq!(
            decide(Some(&open), 200.0, now, 180, fixed_id),
            TickAction::Close { id: "s".into() }
        );
    }

    #[test]
    fn decide_rotates_on_post_sleep_instant_activity() {
        // Session last saw input at 10:00:15. The Mac slept; the daemon
        // froze. It wakes at 11:00:00 and the user is already typing
        // (idle ~1s). A naive Extend would fold the whole hour of sleep
        // into the session — Rotate must split it instead.
        let now = dt("2026-05-30T11:00:00Z");
        let open = WorkSession {
            id: "old".into(),
            started_at: dt("2026-05-30T09:30:00Z"),
            last_input_at: dt("2026-05-30T10:00:15Z"),
        };
        match decide(Some(&open), 1.0, now, 180, fixed_id) {
            TickAction::Rotate {
                close_id,
                open: new,
            } => {
                assert_eq!(close_id, "old");
                assert_eq!(new.id, "sess-1");
                // new session starts ~now, not back during sleep
                assert!((now - new.started_at).num_seconds() == 1);
            }
            other => panic!("expected Rotate, got {other:?}"),
        }
    }

    #[test]
    fn store_round_trip_open_extend_close_and_totals() {
        let dir = std::env::temp_dir().join(format!("act-{}.db", uuid::Uuid::new_v4()));
        let store = ActivityStore::open(&dir).unwrap();
        assert!(store.current_open().unwrap().is_none());

        // Anchor to 10:00 local today (not `now - 30m`) so the session can't
        // straddle local midnight — that made this flake on CI runs just after
        // 00:00 UTC, where `now - 30m` lands on the previous local day.
        let today = Local::now().date_naive();
        let start = Local
            .from_local_datetime(&today.and_hms_opt(10, 0, 0).unwrap())
            .earliest()
            .unwrap()
            .with_timezone(&Utc);
        let s = WorkSession {
            id: "s1".into(),
            started_at: start,
            last_input_at: start,
        };
        store.apply(&TickAction::Open(s.clone())).unwrap();
        assert!(store.current_open().unwrap().is_some());

        // Extend to 20 minutes of work.
        let end = start + chrono::Duration::minutes(20);
        store
            .apply(&TickAction::Extend {
                id: "s1".into(),
                last_input_at: end,
            })
            .unwrap();
        store.apply(&TickAction::Close { id: "s1".into() }).unwrap();
        assert!(store.current_open().unwrap().is_none());

        let secs = store.seconds_on_local_day(0).unwrap();
        // ~20 minutes (1200s); allow a little slack for julianday math.
        assert!((secs - 1200.0).abs() < 5.0, "expected ~1200s, got {secs}");

        let _ = std::fs::remove_file(&dir);
    }

    /// Phantom idle blips (peripherals resetting the idle counter for an
    /// instant) produce 0-second sessions. With a minimum configured, the
    /// writer store drops them at finalize time; real sessions survive.
    #[test]
    fn short_sessions_are_dropped_on_finalize() {
        let dir = std::env::temp_dir().join(format!("act-{}.db", uuid::Uuid::new_v4()));
        let store = ActivityStore::open_for_daemon(&dir, 30).unwrap();
        let start = Utc::now() - chrono::Duration::hours(2);

        // 1) Zero-length blip: Open then immediate Close → row deleted.
        store
            .apply(&TickAction::Open(WorkSession {
                id: "blip".into(),
                started_at: start,
                last_input_at: start,
            }))
            .unwrap();
        store
            .apply(&TickAction::Close { id: "blip".into() })
            .unwrap();

        // 2) Real 20-minute session → survives the close.
        store
            .apply(&TickAction::Open(WorkSession {
                id: "real".into(),
                started_at: start,
                last_input_at: start,
            }))
            .unwrap();
        store
            .apply(&TickAction::Extend {
                id: "real".into(),
                last_input_at: start + chrono::Duration::minutes(20),
            })
            .unwrap();
        store
            .apply(&TickAction::Close { id: "real".into() })
            .unwrap();

        // 3) Rotate away from a blip: closed side dropped, new side open.
        store
            .apply(&TickAction::Open(WorkSession {
                id: "blip2".into(),
                started_at: start + chrono::Duration::minutes(30),
                last_input_at: start + chrono::Duration::minutes(30),
            }))
            .unwrap();
        let rotated = store
            .apply(&TickAction::Rotate {
                close_id: "blip2".into(),
                open: WorkSession {
                    id: "after-rotate".into(),
                    started_at: start + chrono::Duration::minutes(50),
                    last_input_at: start + chrono::Duration::minutes(50),
                },
            })
            .unwrap();
        assert_eq!(rotated.unwrap().id, "after-rotate");

        // 4) Startup finalize drops a short leftover open session too.
        //    ("after-rotate" is still open at 0 seconds here.)
        store.close_open_sessions().unwrap();

        let ids: Vec<String> = {
            let conn = store.conn.lock().unwrap();
            let mut stmt = conn
                .prepare("SELECT id FROM work_sessions ORDER BY started_at")
                .unwrap();
            stmt.query_map([], |r| r.get::<_, String>(0))
                .unwrap()
                .filter_map(|r| r.ok())
                .collect()
        };
        assert_eq!(
            ids,
            vec!["real".to_string()],
            "only the real session survives"
        );

        // Reader-mode stores (min = 0) never delete: closing a blip keeps it.
        let reader = ActivityStore::open(&dir).unwrap();
        store
            .apply(&TickAction::Open(WorkSession {
                id: "blip3".into(),
                started_at: start,
                last_input_at: start,
            }))
            .unwrap();
        reader
            .apply(&TickAction::Close { id: "blip3".into() })
            .unwrap();
        let n: i64 = {
            let conn = reader.conn.lock().unwrap();
            conn.query_row(
                "SELECT COUNT(*) FROM work_sessions WHERE id='blip3'",
                [],
                |r| r.get(0),
            )
            .unwrap()
        };
        assert_eq!(n, 1, "min=0 store must not delete anything");

        let _ = std::fs::remove_file(&dir);
    }

    /// Helper: write a closed session [start, end] directly.
    fn put_session(store: &ActivityStore, id: &str, start: DateTime<Utc>, end: DateTime<Utc>) {
        store
            .apply(&TickAction::Open(WorkSession {
                id: id.into(),
                started_at: start,
                last_input_at: start,
            }))
            .unwrap();
        store
            .apply(&TickAction::Extend {
                id: id.into(),
                last_input_at: end,
            })
            .unwrap();
        store.apply(&TickAction::Close { id: id.into() }).unwrap();
    }

    #[test]
    fn current_session_seconds_reflects_open_span() {
        let dir = std::env::temp_dir().join(format!("act-{}.db", uuid::Uuid::new_v4()));
        let store = ActivityStore::open(&dir).unwrap();
        assert!(store.current_session_seconds(Utc::now()).unwrap().is_none());

        let start = Utc::now() - chrono::Duration::minutes(17);
        store
            .apply(&TickAction::Open(WorkSession {
                id: "s".into(),
                started_at: start,
                last_input_at: start,
            }))
            .unwrap();
        let secs = store
            .current_session_seconds(start + chrono::Duration::minutes(17))
            .unwrap()
            .unwrap();
        assert!((secs - 1020.0).abs() < 2.0, "expected ~1020s, got {secs}");
        let _ = std::fs::remove_file(&dir);
    }

    #[test]
    fn day_detail_reports_blocks_longest_and_span() {
        let dir = std::env::temp_dir().join(format!("act-{}.db", uuid::Uuid::new_v4()));
        let store = ActivityStore::open(&dir).unwrap();

        // Anchor both sessions safely inside today's local window: noon
        // and 2pm local, so the test never straddles local midnight.
        let today = Local::now().date_naive();
        let at = |h: u32, m: u32| -> DateTime<Utc> {
            Local
                .from_local_datetime(&today.and_hms_opt(h, m, 0).unwrap())
                .earliest()
                .unwrap()
                .with_timezone(&Utc)
        };
        put_session(&store, "a", at(10, 0), at(11, 0)); // 60 min
        put_session(&store, "b", at(14, 0), at(14, 20)); // 20 min

        let d = store.day_detail(today).unwrap();
        assert_eq!(d.session_count, 2);
        assert!((d.total_seconds - 4800.0).abs() < 5.0, "total {d:?}");
        assert!(
            (d.longest_seconds - 3600.0).abs() < 5.0,
            "longest {}",
            d.longest_seconds
        );
        assert_eq!(d.blocks.len(), 2);
        assert_eq!(d.first_start, Some(at(10, 0)));
        assert_eq!(d.last_end, Some(at(14, 20)));
        let _ = std::fs::remove_file(&dir);
    }

    #[test]
    fn day_detail_drops_micro_sessions_but_keeps_their_seconds_in_total() {
        let dir = std::env::temp_dir().join(format!("act-{}.db", uuid::Uuid::new_v4()));
        let store = ActivityStore::open(&dir).unwrap();
        let today = Local::now().date_naive();
        let at = |h: u32, m: u32, s: u32| -> DateTime<Utc> {
            Local
                .from_local_datetime(&today.and_hms_opt(h, m, s).unwrap())
                .earliest()
                .unwrap()
                .with_timezone(&Utc)
        };
        // One real 30-min session + two restart-noise micro-bursts (2s, 5s).
        put_session(&store, "real", at(10, 0, 0), at(10, 30, 0));
        put_session(&store, "noise1", at(11, 0, 0), at(11, 0, 2));
        put_session(&store, "noise2", at(12, 0, 0), at(12, 0, 5));

        let d = store.day_detail(today).unwrap();
        // Only the real session shows.
        assert_eq!(d.session_count, 1, "micro-sessions must not count");
        assert_eq!(d.blocks.len(), 1);
        assert_eq!(d.first_start, Some(at(10, 0, 0)));
        assert_eq!(d.last_end, Some(at(10, 30, 0)));
        // Total still includes the ~7s of noise (1800 + 7).
        assert!(
            (d.total_seconds - 1807.0).abs() < 5.0,
            "total should keep micro seconds: {}",
            d.total_seconds
        );
        let _ = std::fs::remove_file(&dir);
    }

    #[test]
    fn week_stats_best_day_with_no_prior_history() {
        let dir = std::env::temp_dir().join(format!("act-{}.db", uuid::Uuid::new_v4()));
        let store = ActivityStore::open(&dir).unwrap();
        let today = Local::now().date_naive();
        let at = |h: u32| -> DateTime<Utc> {
            Local
                .from_local_datetime(&today.and_hms_opt(h, 0, 0).unwrap())
                .earliest()
                .unwrap()
                .with_timezone(&Utc)
        };
        put_session(&store, "t", at(9), at(11)); // 2h today

        let w = store.week_stats().unwrap();
        assert!(w.total_seconds >= 7000.0, "week total {}", w.total_seconds);
        // No prior weeks of data → honest: no average / delta.
        assert!(w.avg_seconds.is_none());
        assert!(w.delta_vs_avg_seconds.is_none());
        // Busiest day is today.
        assert_eq!(w.best_weekday, Some(today.format("%a").to_string()));
        let _ = std::fs::remove_file(&dir);
    }

    #[test]
    fn open_for_daemon_finalizes_open_sessions_but_readers_do_not() {
        let dir = std::env::temp_dir().join(format!("act-{}.db", uuid::Uuid::new_v4()));
        {
            // min_session_secs = 0: this test is about finalize-vs-reader
            // semantics, not the short-session filter.
            let store = ActivityStore::open_for_daemon(&dir, 0).unwrap();
            let now = Utc::now();
            store
                .apply(&TickAction::Open(WorkSession {
                    id: "x".into(),
                    started_at: now,
                    last_input_at: now,
                }))
                .unwrap();
            assert!(store.current_open().unwrap().is_some());
        }
        // Read-only opens (CLI/UI reporting) must not close the live
        // session underneath the daemon.
        let reader = ActivityStore::open(&dir).unwrap();
        assert!(reader.current_open().unwrap().is_some());

        // Daemon startup still finalizes the prior run's open session
        // so downtime is never resumed into the same span.
        let daemon = ActivityStore::open_for_daemon(&dir, 0).unwrap();
        assert!(daemon.current_open().unwrap().is_none());
        let _ = std::fs::remove_file(&dir);
    }

    #[test]
    fn daily_totals_split_sessions_across_local_midnight() {
        let dir = std::env::temp_dir().join(format!("act-{}.db", uuid::Uuid::new_v4()));
        let store = ActivityStore::open(&dir).unwrap();
        let today = Local::now().date_naive();
        let yesterday = today - chrono::Duration::days(1);
        let (today_start, _) = local_day_bounds(today).unwrap();

        let session_start = today_start - chrono::Duration::minutes(30);
        let session_end = today_start + chrono::Duration::minutes(30);
        store
            .apply(&TickAction::Open(WorkSession {
                id: "midnight".into(),
                started_at: session_start,
                last_input_at: session_start,
            }))
            .unwrap();
        store
            .apply(&TickAction::Extend {
                id: "midnight".into(),
                last_input_at: session_end,
            })
            .unwrap();
        store
            .apply(&TickAction::Close {
                id: "midnight".into(),
            })
            .unwrap();

        let today_secs = store.seconds_on_local_date(today).unwrap();
        let yesterday_secs = store.seconds_on_local_date(yesterday).unwrap();
        assert!(
            (today_secs - 1800.0).abs() < 5.0,
            "expected ~1800s today, got {today_secs}"
        );
        assert!(
            (yesterday_secs - 1800.0).abs() < 5.0,
            "expected ~1800s yesterday, got {yesterday_secs}"
        );
        assert_eq!(store.session_count_on_local_date(today).unwrap(), 1);
        assert_eq!(store.session_count_on_local_date(yesterday).unwrap(), 1);

        let _ = std::fs::remove_file(&dir);
    }

    #[test]
    fn fmt_hm_formats_durations() {
        assert_eq!(fmt_hm(0.0), "0m");
        assert_eq!(fmt_hm(45.0 * 60.0), "45m");
        assert_eq!(fmt_hm(4.0 * 3600.0 + 20.0 * 60.0), "4h 20m");
    }

    #[test]
    fn dominant_confidence_picks_most_seconds_and_breaks_ties_up() {
        use std::collections::HashMap;
        let mut m = HashMap::new();
        m.insert("low".to_string(), 100.0);
        m.insert("high".to_string(), 900.0);
        assert_eq!(dominant_confidence(&m).as_deref(), Some("high"));

        // Even split → break toward the stronger level, never under-report.
        let mut tie = HashMap::new();
        tie.insert("low".to_string(), 500.0);
        tie.insert("high".to_string(), 500.0);
        assert_eq!(dominant_confidence(&tie).as_deref(), Some("high"));

        assert_eq!(dominant_confidence(&HashMap::new()), None);
    }

    #[cfg(unix)]
    #[test]
    fn open_tightens_default_mnemonic_dir_and_activity_db_file() {
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!("mnemonic-act-{}", uuid::Uuid::new_v4()));
        let dir = root.join(".mnemonic");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        let db = dir.join("activity.db");

        let _store = ActivityStore::open(&db).unwrap();

        let dir_mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        let db_mode = std::fs::metadata(&db).unwrap().permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o700);
        assert_eq!(db_mode, 0o600);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn recompute_day_pad_clamps_to_neighbouring_sessions() {
        // Two close sessions today (4-min gap). The ±10-min pad would overlap
        // their windows badly; the midpoint clamp must split the gap so a
        // memory in it can only reach the nearer session.
        let dir = std::env::temp_dir().join(format!("act-{}.db", uuid::Uuid::new_v4()));
        let store = ActivityStore::open(&dir).unwrap();
        let today = Local::now().date_naive();
        let at = |h, m| {
            Local
                .from_local_datetime(&today.and_hms_opt(h, m, 0).unwrap())
                .earliest()
                .unwrap()
                .with_timezone(&Utc)
        };
        put_session(&store, "a", at(10, 0), at(10, 10));
        put_session(&store, "b", at(10, 14), at(10, 24));

        // Capture the windows the signal fn is actually asked about.
        let mut windows: Vec<(DateTime<Utc>, DateTime<Utc>)> = Vec::new();
        let pad = chrono::Duration::minutes(10);
        store
            .day_sessions_with_signals(
                today,
                &crate::attribution::AttribCfg::default(),
                pad,
                |s, e| {
                    windows.push((s, e));
                    Vec::new()
                },
            )
            .unwrap();

        assert_eq!(windows.len(), 2, "two sessions → two signal windows");
        let (a_lo, a_hi) = windows[0];
        let (b_lo, b_hi) = windows[1];
        // Gap midpoint is 10:12 — both clamp to it, so windows touch but never
        // overlap (a padded window can't steal the neighbour's signal).
        let mid = at(10, 12);
        assert_eq!(a_hi, mid, "session A high clamps to gap midpoint");
        assert_eq!(b_lo, mid, "session B low clamps to gap midpoint");
        assert!(a_hi <= b_lo, "windows must not overlap");
        // Outer edges keep the full pad (no neighbour on that side).
        assert_eq!(a_lo, at(9, 50));
        assert_eq!(b_hi, at(10, 34));

        let _ = std::fs::remove_file(&dir);
    }
}
