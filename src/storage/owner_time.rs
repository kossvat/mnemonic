//! What the split of the owner's time reads and keeps in the store
//! (live-projects, design 8): the transcripts the owner drives near a
//! day, the days whose split must be made again, the key a project's time
//! is written under, and the memories the fallback reads.
use anyhow::Result;
use chrono::{DateTime, Duration, NaiveDateTime, Utc};
use rusqlite::{Connection, params};
use std::collections::{BTreeMap, HashMap, HashSet};

use super::Storage;
use super::overrides::Corrections;
use crate::owner_time::{
    KNOWN_TOUCH_SECS, PROMPT_SECS, ProjectId, RECORD_MINUTES, Span, Transcript,
};

pub(super) const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS attribution_dirty (
        utc_day INTEGER PRIMARY KEY,
        mark INTEGER NOT NULL DEFAULT 1
    );
    CREATE TABLE IF NOT EXISTS attribution_state (
        key TEXT PRIMARY KEY,
        value TEXT NOT NULL
    );
";

/// A segment says something of the owner's time for this long after its
/// last record (the trust of a touch), and a prompt for this long after
/// it was typed: a day they reach into is a day they change.
pub(super) const SEGMENT_REACH_SECS: i64 = KNOWN_TOUCH_SECS;
pub(super) const PROMPT_REACH_SECS: i64 = PROMPT_SECS;

/// Keep the time of the earliest segment ever written: the day the rule
/// of live projects is in force from. A segment that is dropped later
/// does not move it forward, so the days it was in are still made again.
pub(super) fn rule_since_on(conn: &Connection, started: DateTime<Utc>) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO attribution_state (key, value) VALUES ('rule_since', ?1)
         ON CONFLICT(key) DO UPDATE SET value = MIN(value, excluded.value)",
        params![stamp(started)],
    )?;
    Ok(())
}

const DAY_SECS: i64 = 86_400;
/// A range of time longer than this marks its last days only: nothing
/// that is kept spans one (a correction longer than it is refused), and
/// a clock gone wrong must not fill the table.
pub(super) const MOST_DAYS: i64 = 3660;
const STAMP: &str = "%Y-%m-%d %H:%M:%S";

fn stamp(at: DateTime<Utc>) -> String {
    at.format(STAMP).to_string()
}

fn seconds(text: &str) -> Option<i64> {
    NaiveDateTime::parse_from_str(text, STAMP)
        .ok()
        .map(|at| at.and_utc().timestamp())
}

/// Mark the days, in UTC, that hold a part of `[from, to]`: their split
/// of the owner's time is made again. A day is kept as a number, so the
/// mark holds whatever the timezone is when it is read.
pub(super) fn mark_dirty_on(
    conn: &Connection,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> rusqlite::Result<()> {
    let last = to.max(from).timestamp().div_euclid(DAY_SECS);
    let first = from.timestamp().div_euclid(DAY_SECS).max(last - MOST_DAYS);
    for day in first..=last {
        // Every marking gives the day a mark of its own. A counter that
        // began again at one after a clear let a reader that had read the
        // earlier one clear a later marking of the day.
        conn.execute(
            "INSERT INTO attribution_dirty (utc_day, mark)
             VALUES (?1, abs(random() % 4611686018427387904) + 1)
             ON CONFLICT(utc_day) DO UPDATE SET mark = excluded.mark",
            params![day],
        )?;
    }
    Ok(())
}

/// Mark the days the segments of `project` lie on. A merge and its undo
/// change which project those segments count for, so the days made while
/// the one stood are made again under the other.
pub(super) fn mark_days_of_project(conn: &Connection, project: i64) -> rusqlite::Result<()> {
    let (first, last): (Option<String>, Option<String>) = conn.query_row(
        "SELECT MIN(started_at), MAX(last_at) FROM work_segments WHERE project_id = ?1",
        params![project],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let at = |text: Option<String>| {
        text.as_deref()
            .and_then(seconds)
            .and_then(|at| DateTime::from_timestamp(at, 0))
    };
    if let (Some(from), Some(to)) = (at(first), at(last)) {
        mark_dirty_on(conn, from, to)?;
    }
    Ok(())
}

/// What a stored key of a time row stands for now.
///
/// A row is written under the entity of its project, or, while no entity
/// carries the project's name, under its place in the registry. A link
/// made later, and a merge, move no row: the readers of the rows follow
/// them, so one project is one row whatever its rows were written under.
pub struct KeysNow {
    by_project: HashMap<ProjectId, String>,
    canonical: HashMap<i64, i64>,
    by_entity: HashMap<String, i64>,
    archived: HashSet<String>,
}

impl KeysNow {
    pub(super) fn read(conn: &Connection) -> Result<Self> {
        let projects = conn
            .prepare("SELECT id, merged_into, entity_id FROM projects ORDER BY id")?
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let archived = conn
            .prepare("SELECT entity_id FROM entity_archive")?
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<HashSet<_>>>()?;
        let successors: HashMap<_, _> = projects.iter().map(|(id, into, _)| (*id, *into)).collect();
        let mut canonical = HashMap::new();
        for (id, _, _) in &projects {
            // Match project_groups: follow every redirect, but a corrupt
            // cycle leaves the originating project under its own id.
            let mut at = *id;
            let mut passed = HashSet::from([at]);
            while let Some(next) = successors.get(&at).copied().flatten() {
                if !passed.insert(next) {
                    at = *id;
                    break;
                }
                at = next;
            }
            canonical.insert(*id, at);
        }
        let mut by_project = HashMap::new();
        // The canonical entity wins, then an unarchived merged entity.
        for own in [true, false] {
            for (id, _, entity) in &projects {
                let into = canonical[id];
                if let Some(entity) = entity
                    && (into == *id) == own
                    && !archived.contains(entity)
                {
                    by_project.entry(into).or_insert_with(|| entity.clone());
                }
            }
        }
        for (id, _, _) in &projects {
            let into = canonical[id];
            by_project
                .entry(into)
                .or_insert_with(|| crate::owner_time::registry_key(into));
        }
        let by_entity = projects
            .into_iter()
            .filter_map(|(id, _, entity)| entity.map(|entity| (entity, id)))
            .collect();
        Ok(Self {
            by_project,
            canonical,
            by_entity,
            archived,
        })
    }

    pub(super) fn rekeyed_time(
        &self,
        time: &crate::activity::ProjectTimeData,
    ) -> crate::activity::ProjectTimeData {
        let mut rows: Vec<crate::activity::ProjectTimeRow> = Vec::new();
        let mut unattributed_today = time.unattributed_today;
        let mut unattributed_week = time.unattributed_week;
        for row in &time.rows {
            if self.is_archived(&row.project_key) {
                unattributed_today += row.today_seconds;
                unattributed_week += row.week_seconds;
                continue;
            }
            let key = self.now(&row.project_key);
            match rows.iter_mut().find(|known| known.project_key == key) {
                Some(known) => {
                    // The trust of the larger part.
                    if row.week_seconds > known.week_seconds {
                        known.confidence = row.confidence.clone();
                    }
                    known.today_seconds += row.today_seconds;
                    known.week_seconds += row.week_seconds;
                    for (sum, seconds) in known.week.iter_mut().zip(row.week) {
                        *sum += seconds;
                    }
                }
                None => rows.push(crate::activity::ProjectTimeRow {
                    project_key: key,
                    ..row.clone()
                }),
            }
        }
        rows.sort_by(|a, b| b.week_seconds.total_cmp(&a.week_seconds));
        crate::activity::ProjectTimeData {
            rows,
            unattributed_today,
            unattributed_week,
        }
    }

    pub fn is_archived(&self, key: &str) -> bool {
        self.archived.contains(key) || self.archived.contains(&self.now(key))
    }

    pub fn now(&self, key: &str) -> String {
        let project = crate::owner_time::registry_id(key).or(self.by_entity.get(key).copied());
        project
            .map(|id| self.canonical.get(&id).copied().unwrap_or(id))
            .and_then(|id| self.by_project.get(&id).cloned())
            .unwrap_or_else(|| key.to_string())
    }
}

impl Storage {
    /// The store is of the generation this binary writes for. The daemon
    /// asks before it starts anything: under an earlier generation a
    /// daemon of an earlier version could still open the store and make
    /// the rows of attribution again by its own rule.
    pub fn require_current_generation(&self) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let path = std::path::PathBuf::from(conn.path().unwrap_or_default());
        super::versioning::require_current(&conn, &path)
    }

    /// Mark the days that hold a part of `[from, to]` for a new split.
    pub fn attribution_mark_dirty(&self, from: DateTime<Utc>, to: DateTime<Utc>) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        Ok(mark_dirty_on(&conn, from, to)?)
    }

    /// The marked days, each as its start and its mark.
    pub fn attribution_dirty(&self) -> Result<Vec<(DateTime<Utc>, i64)>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let mut stmt = conn.prepare("SELECT utc_day, mark FROM attribution_dirty ORDER BY 1")?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows
            .into_iter()
            .filter_map(|(day, mark)| {
                DateTime::from_timestamp(day * DAY_SECS, 0).map(|at| (at, mark))
            })
            .collect())
    }

    /// Forget the marks that were read. One marked again since stays.
    pub fn attribution_dirty_clear(&self, seen: &[(DateTime<Utc>, i64)]) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        for (at, mark) in seen {
            conn.execute(
                "DELETE FROM attribution_dirty WHERE utc_day = ?1 AND mark = ?2",
                params![at.timestamp().div_euclid(DAY_SECS), mark],
            )?;
        }
        Ok(())
    }

    /// When the first segment began: days before it keep the rows they
    /// have.
    pub fn first_segment_at(&self) -> Result<Option<DateTime<Utc>>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        // The earliest ever written, which a later rewrite of a
        // transcript does not move.
        let first: Option<String> = conn.query_row(
            "SELECT MIN(at) FROM (
                SELECT value AS at FROM attribution_state WHERE key = 'rule_since'
                UNION ALL SELECT MIN(started_at) FROM work_segments)",
            [],
            |r| r.get(0),
        )?;
        Ok(first
            .as_deref()
            .and_then(seconds)
            .and_then(|at| DateTime::from_timestamp(at, 0)))
    }

    /// Every project's id under the one it was merged into.
    pub(super) fn project_canonical_ids(&self) -> Result<HashMap<i64, i64>> {
        Ok(self
            .project_groups()?
            .iter()
            .flat_map(|(canonical, members)| members.iter().map(move |id| (*id, *canonical)))
            .collect())
    }

    /// What the transcripts the owner drives say of the time between
    /// `from` and `to`, with enough before it to know what was live at its
    /// start. A transcript a program ran is not among them: its record a
    /// minute ago must not take the owner's time. Projects are the ones
    /// merges resolve to. With them, what the owner corrected: the pin
    /// and the relabelled ranges of each transcript, and the ranges of
    /// the owner's own time.
    pub fn owner_transcripts(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<(Vec<Transcript>, Corrections)> {
        let canonical = self.project_canonical_ids()?;
        let corrections = self.corrections(&canonical)?;
        let resolve = |id: Option<i64>| id.map(|id| canonical.get(&id).copied().unwrap_or(id));
        let reach = stamp(from - Duration::seconds(KNOWN_TOUCH_SECS));
        let until = stamp(to);
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let mut transcripts: BTreeMap<String, Transcript> = BTreeMap::new();
        // The project of each segment read, for the minutes that name it.
        let mut segments: HashMap<i64, (String, Option<ProjectId>)> = HashMap::new();
        {
            let mut stmt = conn.prepare(
                "SELECT id, stream, project_id, started_at, last_at FROM work_segments
                  WHERE driver = 'owner' AND last_at >= ?1 AND started_at < ?2
                  ORDER BY started_at, id",
            )?;
            let rows = stmt.query_map(params![reach, until], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<i64>>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                ))
            })?;
            for row in rows {
                let (id, stream, project, started, last) = row?;
                let (Some(started), Some(last)) = (seconds(&started), seconds(&last)) else {
                    continue;
                };
                let project = resolve(project);
                transcripts
                    .entry(stream.clone())
                    .or_default()
                    .spans
                    .push(Span {
                        project,
                        started,
                        last,
                    });
                segments.insert(id, (stream, project));
            }
        }
        {
            let first = from.timestamp().div_euclid(60) - KNOWN_TOUCH_SECS / 60 - RECORD_MINUTES;
            let mut stmt = conn.prepare(
                "SELECT a.segment_id, a.minute, a.touches
                   FROM segment_activity a JOIN work_segments s ON s.id = a.segment_id
                  WHERE s.driver = 'owner' AND s.last_at >= ?1 AND s.started_at < ?2
                    AND a.minute >= ?3 AND a.minute <= ?4",
            )?;
            let rows = stmt.query_map(
                params![reach, until, first, to.timestamp().div_euclid(60)],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, i64>(2)?,
                    ))
                },
            )?;
            for row in rows {
                let (segment, minute, touches) = row?;
                let Some((stream, project)) = segments.get(&segment) else {
                    continue;
                };
                let transcript = transcripts.entry(stream.clone()).or_default();
                transcript.minutes.push(minute);
                if let (true, Some(project)) = (touches > 0, project) {
                    transcript.touches.push((minute, *project));
                }
            }
        }
        {
            let mut stmt = conn.prepare(
                "SELECT stream, at FROM owner_prompts WHERE at > ?1 AND at < ?2 ORDER BY at",
            )?;
            let since = stamp(from - Duration::seconds(PROMPT_SECS));
            let rows = stmt.query_map(params![since, until], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?;
            for row in rows {
                let (stream, at) = row?;
                // A prompt with no segment near it names no project.
                if let (Some(transcript), Some(at)) = (transcripts.get_mut(&stream), seconds(&at)) {
                    transcript.prompts.push(at);
                }
            }
        }
        let transcripts = transcripts
            .into_iter()
            .map(|(stream, mut transcript)| {
                transcript.pin = corrections.pin(&stream);
                transcript.relabels = corrections.ranges(&stream).to_vec();
                transcript.sorted()
            })
            .collect();
        Ok((transcripts, corrections))
    }

    /// The key the time of each project is written under: the entity of
    /// its registry row, the canonical one first, so every reader of the
    /// rows that knows entities finds it; without an unarchived entity,
    /// its place in the registry. An archived historical entity must not
    /// absorb new explicit attribution after registry adoption.
    pub fn attribution_keys(&self) -> Result<HashMap<ProjectId, String>> {
        Ok(self.attribution_keys_now()?.by_project)
    }

    /// What every stored key of a time row stands for now.
    pub fn attribution_keys_now(&self) -> Result<KeysNow> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let snapshot = conn.unchecked_transaction()?;
        KeysNow::read(&snapshot)
    }

    /// The time rows of the last days, each under the key it stands for
    /// now, rows of one key summed.
    // Kept for library callers; payloads resolve keys in their own snapshot.
    #[allow(dead_code)]
    pub fn rekeyed_time(
        &self,
        time: &crate::activity::ProjectTimeData,
    ) -> Result<crate::activity::ProjectTimeData> {
        Ok(self.attribution_keys_now()?.rekeyed_time(time))
    }

    /// The time rows of one day, each under the key it stands for now,
    /// rows of one key summed, the longest first.
    pub fn rekeyed_day(
        &self,
        day: Vec<(String, f64, Option<String>)>,
    ) -> Result<Vec<(String, f64, Option<String>)>> {
        Ok(self.rekeyed_day_with_unattributed((day, 0.0))?.0)
    }

    /// Read a cached day without giving archived entities time. Its seconds
    /// remain accounted for as unattributed; the stored rows are untouched
    /// so undo restores the original split immediately.
    pub fn rekeyed_day_with_unattributed(
        &self,
        (day, mut unattributed): crate::activity::DayAttribution,
    ) -> Result<crate::activity::DayAttribution> {
        let keys = self.attribution_keys_now()?;
        let mut rows: Vec<(String, f64, Option<String>)> = Vec::new();
        for (key, seconds, confidence) in day {
            if keys.is_archived(&key) {
                unattributed += seconds;
                continue;
            }
            let key = keys.now(&key);
            match rows.iter_mut().find(|known| known.0 == key) {
                Some(known) => {
                    if seconds > known.1 {
                        known.2 = confidence;
                    }
                    known.1 += seconds;
                }
                None => rows.push((key, seconds, confidence)),
            }
        }
        rows.sort_by(|a, b| b.1.total_cmp(&a.1));
        Ok((rows, unattributed))
    }

    /// The projects of the registry that the memories written between
    /// `from` and `to` are tied to, each with how many: what the fallback
    /// reads when no transcript was live. The join is on the registry, not
    /// on the type of an entity: an entity outside it is not a project.
    pub fn registry_signals(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<Vec<(ProjectId, f64)>> {
        let canonical = self.project_canonical_ids()?;
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let mut stmt = conn.prepare(
            "SELECT DISTINCT p.id, m.id
               FROM memories m
               JOIN memory_entities me ON me.memory_id = m.id
               JOIN projects p ON p.entity_id = me.entity_id
                AND NOT EXISTS(SELECT 1 FROM entity_archive a WHERE a.entity_id = me.entity_id)
              WHERE m.superseded_by IS NULL AND m.memory_type != 'session_summary'
                AND m.timestamp >= ?1 AND m.timestamp < ?2",
        )?;
        let rows = stmt
            .query_map(params![from.to_rfc3339(), to.to_rfc3339()], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        // A memory counts once for a project, however many of the rows
        // merged into it carry its entity.
        let memories: std::collections::BTreeSet<(ProjectId, String)> = rows
            .into_iter()
            .map(|(id, memory)| (canonical.get(&id).copied().unwrap_or(id), memory))
            .collect();
        let mut weights: BTreeMap<ProjectId, f64> = BTreeMap::new();
        for (project, _) in memories {
            *weights.entry(project).or_default() += 1.0;
        }
        Ok(weights.into_iter().collect())
    }
}
