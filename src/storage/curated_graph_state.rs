//! Reversible archive state and write-once parent-local slots.
use super::*;

impl Storage {
    /// The exact retained mark, including after its entity was merged or
    /// removed. A mark never follows a merge to the surviving entity.
    pub fn entity_archive_at(&self, id: &str) -> Result<Option<i64>> {
        crate::redaction::state::admit_identities("entity archive", [id])?;
        let conn = self
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("store lock unavailable"))?;
        Ok(conn
            .query_row(
                "SELECT at FROM entity_archive WHERE entity_id=?1",
                [id],
                |row| row.get(0),
            )
            .optional()?)
    }

    /// Retained archive marks are history even when their entity no longer
    /// exists. Only current, admissible memory metadata contributes counts.
    pub fn entity_archive_history(&self) -> Result<Vec<ArchivedCandidate>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("store lock unavailable"))?;
        let mut query = conn.prepare(
            "SELECT a.entity_id,e.name,a.at,m.id,unixepoch(m.timestamp)
             FROM entity_archive a
             LEFT JOIN entities e ON e.id=a.entity_id
             LEFT JOIN memory_entities me ON me.entity_id=e.id
             LEFT JOIN memories m ON m.id=me.memory_id
               AND m.superseded_by IS NULL AND m.memory_type!='session_summary'
             ORDER BY a.entity_id,m.id",
        )?;
        let rows = query.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<i64>>(4)?,
            ))
        })?;
        let mut history = BTreeMap::new();
        for row in rows {
            let (id, name, archived_at, memory, at) = row?;
            if crate::redaction::state::check_identities([id.as_str()]).is_err() {
                continue;
            }
            let entry = history
                .entry(id.clone())
                .or_insert_with(|| ArchivedCandidate {
                    id,
                    name: name.map_or_else(
                        || "Archived entity".into(),
                        |name| crate::redaction::state::prepare_capped(&name, 80).value,
                    ),
                    memory_count: 0,
                    last_memory: None,
                    archived_at: Some(archived_at),
                });
            if memory
                .is_some_and(|id| crate::redaction::state::check_identities([id.as_str()]).is_ok())
            {
                entry.memory_count = entry.memory_count.saturating_add(1);
                entry.last_memory = entry.last_memory.max(at);
            }
        }
        Ok(history.into_values().collect())
    }

    pub fn entity_archive_set(&self, id: &str, archived: bool, at: i64) -> Result<bool> {
        crate::redaction::state::admit_identities("entity archive", [id])?;
        let mut conn = self
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("store lock unavailable"))?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = if archived {
            let data = data(&tx)?;
            let entity = data
                .entities
                .iter()
                .find(|entity| entity.id == id)
                .ok_or_else(|| anyhow::anyhow!("no such entity"))?;
            ensure!(
                entity.kind == "project" && !data.owners.contains_key(id),
                "entity is not a non-registry project"
            );
            tx.execute(
                "INSERT OR IGNORE INTO entity_archive(entity_id,at) VALUES (?1,?2)",
                params![id, at],
            )?
        } else {
            tx.execute("DELETE FROM entity_archive WHERE entity_id=?1", [id])?
        };
        tx.commit()?;
        Ok(changed > 0)
    }
    pub(crate) fn graph_slots(&self) -> Result<Vec<GraphSlot>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("store lock unavailable"))?;
        let mut query =
            conn.prepare("SELECT node_key,parent_key,slot FROM graph_slots ORDER BY node_key")?;
        Ok(query
            .query_map([], |r| {
                Ok(GraphSlot {
                    key: r.get(0)?,
                    parent_key: r.get(1)?,
                    slot: r.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?)
    }
    /// Outside GET only. Existing slots and parents are immutable, including
    /// after archive/merge/reparent. A preferred collision uses the next slot.
    pub(crate) fn ensure_graph_slots(&self, requests: &[SlotRequest]) -> Result<()> {
        for request in requests {
            ensure!(
                valid_key(&request.key) && request.parent_key.as_deref().is_none_or(valid_key),
                "invalid graph key"
            );
            ensure!(
                request.preferred_slot.is_none_or(|slot| slot >= 0),
                "invalid graph slot"
            );
        }
        let mut conn = self
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("store lock unavailable"))?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut ordered: Vec<_> = requests.iter().collect();
        ordered.sort_by(|a, b| a.key.cmp(&b.key));
        for request in ordered {
            if tx
                .query_row(
                    "SELECT 1 FROM graph_slots WHERE node_key=?1",
                    [&request.key],
                    |r| r.get::<_, i64>(0),
                )
                .optional()?
                .is_some()
            {
                continue;
            }
            let highest: Option<i64> = tx.query_row(
                "SELECT MAX(slot) FROM graph_slots WHERE parent_key IS ?1",
                [&request.parent_key],
                |r| r.get(0),
            )?;
            let preferred = if let Some(slot) = request.preferred_slot {
                let taken: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM graph_slots WHERE parent_key IS ?1 AND slot=?2)",
                    params![request.parent_key, slot],
                    |r| r.get(0),
                )?;
                (!taken).then_some(slot)
            } else {
                None
            };
            let slot = match preferred {
                Some(slot) => slot,
                None => highest
                    .unwrap_or(-1)
                    .checked_add(1)
                    .ok_or_else(|| anyhow::anyhow!("graph slots exhausted"))?,
            };
            tx.execute(
                "INSERT INTO graph_slots(node_key,parent_key,slot) VALUES (?1,?2,?3)",
                params![request.key, request.parent_key, slot],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
#[path = "curated_graph_archive_history_tests.rs"]
mod archive_history_tests;
