//! Raw legacy attribution inputs and the visible archive-filtered view.
//! Recomputing the durable cache must not erase reversible archive history.
use super::Storage;
use crate::attribution::ProjectSignal;
use anyhow::Result;
use chrono::{DateTime, Utc};

impl Storage {
    /// Only the legacy cache writer uses this unmasked input. Display and
    /// fallback readers use the public archive-filtered signal methods.
    pub(crate) fn attribution_source_signals(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        min_mems: i64,
    ) -> Result<Vec<ProjectSignal>> {
        signals(self, start, end, min_mems, true)
    }

    pub(crate) fn attribution_source_memory_times(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        min_mems: i64,
    ) -> Result<Vec<(String, DateTime<Utc>)>> {
        memory_times(self, start, end, min_mems, true)
    }
}

pub(super) fn signals(
    storage: &Storage,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    min_mems: i64,
    include_archived: bool,
) -> Result<Vec<ProjectSignal>> {
    let conn = storage
        .conn
        .lock()
        .map_err(|e| anyhow::anyhow!("lock: {e}"))?;
    let mut stmt = conn.prepare(
        "SELECT e.id, COUNT(*) AS w
         FROM memories m
         JOIN memory_entities me ON me.memory_id = m.id
         JOIN entities e ON e.id = me.entity_id AND e.entity_type = 'project'
           AND (?4 OR NOT EXISTS(SELECT 1 FROM entity_archive a WHERE a.entity_id = e.id))
         WHERE m.superseded_by IS NULL
           AND m.timestamp >= ?1 AND m.timestamp < ?2
           AND (
             SELECT COUNT(*) FROM memory_entities me2
             JOIN memories m2 ON m2.id = me2.memory_id
             WHERE me2.entity_id = e.id
               AND m2.superseded_by IS NULL
               AND m2.memory_type != 'session_summary'
           ) >= ?3
         GROUP BY e.id",
    )?;
    let rows = stmt.query_map(
        rusqlite::params![
            start.to_rfc3339(),
            end.to_rfc3339(),
            min_mems,
            include_archived
        ],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)),
    )?;
    Ok(rows
        .filter_map(|r| r.ok())
        .map(|(key, weight)| ProjectSignal {
            project_key: key,
            weight: weight as f64,
        })
        .collect())
}

pub(super) fn memory_times(
    storage: &Storage,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    min_mems: i64,
    include_archived: bool,
) -> Result<Vec<(String, DateTime<Utc>)>> {
    let conn = storage
        .conn
        .lock()
        .map_err(|e| anyhow::anyhow!("lock: {e}"))?;
    let mut stmt = conn.prepare(
        "SELECT e.id, m.timestamp
         FROM memories m
         JOIN memory_entities me ON me.memory_id = m.id
         JOIN entities e ON e.id = me.entity_id AND e.entity_type = 'project'
           AND (?4 OR NOT EXISTS(SELECT 1 FROM entity_archive a WHERE a.entity_id = e.id))
         WHERE m.superseded_by IS NULL
           AND m.memory_type != 'session_summary'
           -- Conversation and correction chatter cannot anchor a work window.
           AND m.memory_type != 'feedback'
           AND COALESCE(m.tags, '') NOT LIKE '%conversation%'
           AND COALESCE(m.tags, '') NOT LIKE '%correction%'
           AND m.timestamp >= ?1 AND m.timestamp < ?2
           AND (
             SELECT COUNT(*) FROM memory_entities me2
             JOIN memories m2 ON m2.id = me2.memory_id
             WHERE me2.entity_id = e.id
               AND m2.superseded_by IS NULL
               AND m2.memory_type != 'session_summary'
           ) >= ?3",
    )?;
    let rows = stmt.query_map(
        rusqlite::params![
            start.to_rfc3339(),
            end.to_rfc3339(),
            min_mems,
            include_archived
        ],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
    )?;
    let mut out = Vec::new();
    for (key, stamp) in rows.filter_map(|r| r.ok()) {
        if let Ok(at) = DateTime::parse_from_rfc3339(&stamp) {
            out.push((key, at.with_timezone(&Utc)));
        }
    }
    Ok(out)
}
