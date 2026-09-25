//! Reading fact chains: a slot as recall shows it, and the slots about a
//! subject.

use anyhow::Result;
use rusqlite::{Connection, params};

use super::keys;
use super::store::{SlotView, ValueView, scope_key};
use crate::storage::Storage;

/// A slot as recall shows it: the current value and the dated trail.
pub fn slot_view(conn: &Connection, slot: &str) -> Result<SlotView> {
    let (project, subject, predicate, qualifier, revision): (String, String, String, String, i64) =
        conn.query_row(
            "SELECT scope, subject, predicate_label, qualifier, revision FROM fact_slots WHERE id = ?1",
            [slot],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )?;
    let mut stmt = conn.prepare(
        "SELECT id, kind, value, valid_from, trust, source_memory_id FROM fact_values
          WHERE slot_id = ?1 AND status = 'active' ORDER BY valid_from_ms, seq",
    )?;
    let mut in_order: Vec<ValueView> = stmt
        .query_map([slot], |r| {
            let kind: String = r.get(1)?;
            Ok(ValueView {
                id: r.get(0)?,
                value: (kind == "value")
                    .then(|| r.get::<_, String>(2))
                    .transpose()?,
                valid_from: r.get(3)?,
                valid_to: None,
                trust: r.get(4)?,
                source_memory_id: r.get(5)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    for i in 1..in_order.len() {
        let next_from = in_order[i].valid_from.clone();
        in_order[i - 1].valid_to = Some(next_from);
    }
    let current = in_order.last().filter(|v| v.value.is_some()).cloned();
    let pending: i64 = conn.query_row(
        "SELECT COUNT(*) FROM fact_values WHERE slot_id = ?1 AND status = 'pending_review'",
        [slot],
        |r| r.get(0),
    )?;
    in_order.reverse();
    Ok(SlotView {
        slot_id: slot.to_owned(),
        project,
        subject,
        predicate,
        qualifier,
        revision,
        current,
        history: in_order,
        pending: pending as usize,
    })
}

/// The slots about `subject`: in one project, or (`None`) in every scope.
pub fn slots_for(conn: &Connection, project: Option<&str>, subject: &str) -> Result<Vec<String>> {
    let subject_key = keys::subject_key(conn, subject)?;
    let mut ids = Vec::new();
    match project {
        Some(project) => {
            let (scope, _) = scope_key(conn, Some(project))?;
            let mut stmt = conn.prepare(
                "SELECT id FROM fact_slots WHERE subject_key = ?1 AND scope_key = ?2
                  ORDER BY predicate, qualifier_key",
            )?;
            for id in stmt.query_map(params![subject_key, scope], |r| r.get(0))? {
                ids.push(id?);
            }
        }
        None => {
            let mut stmt = conn.prepare(
                "SELECT id FROM fact_slots WHERE subject_key = ?1
                  ORDER BY scope_key, predicate, qualifier_key",
            )?;
            for id in stmt.query_map([subject_key], |r| r.get(0))? {
                ids.push(id?);
            }
        }
    }
    Ok(ids)
}

/// Every slot about `subject` (in one project, or all), as recall shows it.
pub fn views(storage: &Storage, project: Option<&str>, subject: &str) -> Result<Vec<SlotView>> {
    let conn = storage
        .conn
        .lock()
        .map_err(|e| anyhow::anyhow!("lock: {e}"))?;
    slots_for(&conn, project, subject)?
        .iter()
        .map(|slot| slot_view(&conn, slot))
        .collect()
}

/// Every current value (all scopes): the migration check reads this.
#[cfg(test)]
#[derive(Debug, Clone, PartialEq)]
pub struct CurrentFact {
    pub subject_key: String,
    pub predicate: String,
    pub value: String,
}

#[cfg(test)]
pub fn current_all(storage: &Storage) -> Result<Vec<CurrentFact>> {
    let conn = storage
        .conn
        .lock()
        .map_err(|e| anyhow::anyhow!("lock: {e}"))?;
    let slots: Vec<(String, String, String)> = {
        let mut stmt = conn.prepare("SELECT id, subject_key, predicate FROM fact_slots")?;
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<_>>()?
    };
    let mut out = Vec::new();
    for (slot, subject_key, predicate) in slots {
        if let Some(value) = slot_view(&conn, &slot)?.current.and_then(|v| v.value) {
            out.push(CurrentFact {
                subject_key,
                predicate,
                value,
            });
        }
    }
    Ok(out)
}

/// How many values are stored, in every state.
#[cfg(test)]
pub fn value_count(storage: &Storage) -> Result<usize> {
    let conn = storage
        .conn
        .lock()
        .map_err(|e| anyhow::anyhow!("lock: {e}"))?;
    Ok(conn.query_row("SELECT COUNT(*) FROM fact_values", [], |r| {
        r.get::<_, i64>(0)
    })? as usize)
}
