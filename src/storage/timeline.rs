//! Read-only corrected pieces of stored agent segments.
use super::Storage;
use anyhow::Result;
use chrono::{DateTime, NaiveDateTime, Utc};
use rusqlite::params;

#[derive(Debug, Clone)]
pub struct SegmentPiece {
    pub id: i64,
    pub stream: String,
    pub project: Option<i64>,
    pub agent: String,
    pub driver: String,
    pub from: i64,
    pub to: i64,
}

const SEGMENTS: &str = "SELECT id, stream, project_id, agent, driver, started_at, last_at
    FROM work_segments WHERE last_at >= ?1 AND started_at <= ?2
    ORDER BY started_at, id";

impl Storage {
    /// Canonical projects and current overrides, with points retained.
    /// Bounds are applied by the caller after the full segment is split.
    pub fn timeline_segments(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<Vec<SegmentPiece>> {
        if to <= from {
            return Ok(Vec::new());
        }
        let canonical = self.project_canonical_ids()?;
        let corrections = self.corrections(&canonical)?;
        let conn = self
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("store lock unavailable"))?;
        let mut query = conn.prepare(SEGMENTS)?;
        let rows = query
            .query_map(
                params![
                    from.format("%Y-%m-%d %H:%M:%S").to_string(),
                    to.format("%Y-%m-%d %H:%M:%S").to_string()
                ],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<i64>>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, String>(6)?,
                    ))
                },
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut result = Vec::new();
        for (id, stream, project, agent, driver, start, end) in rows {
            let second = |text: &str| {
                NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S")
                    .ok()
                    .map(|at| at.and_utc().timestamp())
            };
            let Some((start, end)) = second(&start).zip(second(&end)) else {
                continue;
            };
            if end < start {
                continue;
            }
            let project = project.map(|id| canonical.get(&id).copied().unwrap_or(id));
            for (project, start, end) in corrections.pieces(&stream, project, (start, end)) {
                result.push(SegmentPiece {
                    id,
                    stream: stream.clone(),
                    project,
                    agent: agent.clone(),
                    driver: driver.clone(),
                    from: start,
                    to: end,
                });
            }
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn timeline_segments_search_the_existing_time_index() {
        let fixture = crate::timeline::fixture::Fixture::empty();
        let conn = fixture.storage.conn.lock().unwrap();
        let sql = format!("EXPLAIN QUERY PLAN {}", super::SEGMENTS);
        let details = conn
            .prepare(&sql)
            .unwrap()
            .query_map(["2026-09-29 00:00:00", "2026-09-30 00:00:00"], |row| {
                row.get::<_, String>(3)
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert!(
            details
                .iter()
                .any(|detail| detail.contains("SEARCH work_segments")
                    && detail.contains("idx_work_segments_time")),
            "{details:?}"
        );
        assert!(
            !details
                .iter()
                .any(|detail| detail.contains("SCAN work_segments")),
            "{details:?}"
        );
    }
}
