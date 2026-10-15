//! Read-only facts for the board and the page of a registry project.
use super::Storage;
use anyhow::Result;
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};

// Kept for library callers of project_decisions.
#[allow(dead_code)]
#[derive(Debug, Serialize)]
pub struct Decision {
    pub id: String,
    pub title: String,
    pub at: i64,
}

#[derive(Debug, Serialize)]
pub struct HistoryEntry {
    pub id: String,
    pub title: String,
    pub at: i64,
    /// "decision", or "correction" for a memory the store types as feedback.
    pub kind: &'static str,
}

impl Storage {
    /// Last activity is a segment or a linked memory, resolved through
    /// project merges. No derived row is refreshed by a read.
    pub fn project_last_activity(&self) -> Result<BTreeMap<i64, i64>> {
        let canonical: HashMap<_, _> = self
            .projects()?
            .into_iter()
            .map(|row| Ok((row.id, self.project_canonical(row.id)?)))
            .collect::<Result<_>>()?;
        let conn = self
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("store lock unavailable"))?;
        let mut query = conn.prepare(
            "SELECT project_id, MAX(at) FROM (
                 SELECT project_id, unixepoch(last_at) AS at FROM work_segments
                 WHERE project_id IS NOT NULL
                 UNION ALL
                 SELECT p.id, unixepoch(m.timestamp) AS at FROM projects p
                 JOIN memory_entities me ON me.entity_id = p.entity_id
                 JOIN memories m ON m.id = me.memory_id
             ) WHERE at IS NOT NULL GROUP BY project_id",
        )?;
        let mut result = BTreeMap::new();
        for pair in query.query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))? {
            let (id, at) = pair?;
            let id = canonical.get(&id).copied().unwrap_or(id);
            result
                .entry(id)
                .and_modify(|last: &mut i64| *last = (*last).max(at))
                .or_insert(at);
        }
        Ok(result)
    }

    /// The latest decisions of a project, newest first, as the library has
    /// always served them.
    // Kept for library callers; the routes serve project_history.
    #[allow(dead_code)]
    pub fn project_decisions(&self, project: i64, limit: usize) -> Result<Vec<Decision>> {
        Ok(self
            .project_memories(project, limit, &["decision"])?
            .into_iter()
            .map(|entry| Decision {
                id: entry.id,
                title: entry.title,
                at: entry.at,
            })
            .collect())
    }

    /// The latest decisions and corrections of a project, newest first.
    /// Rows the redaction policy refuses still take part of the limit.
    pub fn project_history(&self, project: i64, limit: usize) -> Result<Vec<HistoryEntry>> {
        self.project_memories(project, limit, &["decision", "feedback"])
    }

    /// Memories of the given fixed types linked to a project's group.
    fn project_memories(
        &self,
        project: i64,
        limit: usize,
        types: &[&'static str],
    ) -> Result<Vec<HistoryEntry>> {
        let canonical = self.project_canonical(project)?;
        let group = self
            .project_groups()?
            .remove(&canonical)
            .unwrap_or_default();
        if group.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders = vec!["?"; group.len()].join(",");
        let conn = self
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("store lock unavailable"))?;
        let mut query = conn.prepare(&format!(
            "SELECT DISTINCT m.id, m.title, unixepoch(m.timestamp), m.memory_type FROM memories m
             JOIN memory_entities me ON me.memory_id = m.id
             JOIN projects p ON p.entity_id = me.entity_id
             WHERE p.id IN ({placeholders}) AND m.memory_type IN ({kinds})
             ORDER BY m.timestamp DESC, m.id LIMIT {}",
            limit.min(20),
            kinds = types
                .iter()
                .map(|kind| format!("'{kind}'"))
                .collect::<Vec<_>>()
                .join(", ")
        ))?;
        let rows = query
            .query_map(rusqlite::params_from_iter(group), |row| {
                Ok(HistoryEntry {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    at: row.get::<_, Option<i64>>(2)?.unwrap_or(0),
                    kind: if row.get::<_, String>(3)? == "feedback" {
                        "correction"
                    } else {
                        "decision"
                    },
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows
            .into_iter()
            .filter_map(|mut row| {
                crate::redaction::state::check_identities([row.id.as_str()]).ok()?;
                row.title = crate::redaction::state::prepare_capped(&row.title, 160).value;
                Some(row)
            })
            .collect())
    }
}
