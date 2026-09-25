//! What recall adds to a memory it returns: the newer memory that replaced
//! it, and, for a memory that stated a fact, what the fact holds now. An
//! agent reading an old value sees at once that it is old.

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension};
use serde_json::{Value, json};

use crate::storage::Storage;

/// Annotate recall hits (JSON objects with an `id`) in place.
pub fn annotate(storage: &Storage, hits: &mut [Value]) -> Result<()> {
    let conn = storage
        .conn
        .lock()
        .map_err(|e| anyhow::anyhow!("lock: {e}"))?;
    for hit in hits.iter_mut() {
        let Some(id) = hit.get("id").and_then(Value::as_str).map(str::to_owned) else {
            continue;
        };
        let Some(object) = hit.as_object_mut() else {
            continue;
        };
        if let Some(by) = replaced_by(&conn, &id)? {
            object.insert("replaced_by".into(), by);
        }
        let facts = facts_stated(&conn, &id)?;
        if !facts.is_empty() {
            object.insert("facts".into(), Value::Array(facts));
        }
    }
    Ok(())
}

/// What a text listing adds after an annotated hit: that a newer memory
/// replaced it, or that a fact it stated holds another value now.
pub fn note(hit: &Value) -> String {
    let mut out = String::new();
    if let Some(by) = hit.get("replaced_by") {
        let id = by["id"].as_str().unwrap_or("");
        let day = by["timestamp"].as_str().unwrap_or("");
        out.push_str(&format!(
            " [replaced by `{}` of {}]",
            id.get(..8).unwrap_or(id),
            day.get(..10).unwrap_or(day)
        ));
    }
    let own = hit.get("facts").and_then(Value::as_array);
    let replacement = hit
        .get("replaced_by")
        .and_then(|by| by.get("facts"))
        .and_then(Value::as_array);
    for fact in own.into_iter().chain(replacement).flatten() {
        if fact["is_current"] == true {
            continue;
        }
        let now = match fact["current"].as_str() {
            Some(value) => value.chars().take(60).collect::<String>(),
            None => "retracted".to_owned(),
        };
        out.push_str(&format!(
            " [{} {} is now {}]",
            fact["subject"].as_str().unwrap_or(""),
            fact["predicate"].as_str().unwrap_or(""),
            now
        ));
    }
    out
}

/// The memory that holds now in `id`'s update chain, when that is not `id`.
fn replaced_by(conn: &Connection, id: &str) -> Result<Option<Value>> {
    let Some(head) = super::store::head(conn, id)?.filter(|head| head != id) else {
        return Ok(None);
    };
    let found: Option<(String, String)> = conn
        .query_row(
            "SELECT title, timestamp FROM memories WHERE id = ?1",
            [&head],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let Some((title, timestamp)) = found else {
        return Ok(None);
    };
    let mut by = json!({"id": head, "title": title, "timestamp": timestamp});
    // The memory that replaced this one stated a fact that has moved on
    // since (a retraction links no memory): say so here too (review point).
    let facts = facts_stated(conn, &head)?;
    if !facts.is_empty() {
        by["facts"] = Value::Array(facts);
    }
    Ok(Some(by))
}

/// Each fact value `id` stated or reconfirmed, with the value its fact
/// holds now.
fn facts_stated(conn: &Connection, id: &str) -> Result<Vec<Value>> {
    let stated: Vec<(String, String, String, String)> = {
        let mut stmt = conn.prepare(
            // A memory that recorded a reconfirmation cites its value only
            // as evidence of the event (review point).
            "SELECT id, slot_id, kind, value FROM fact_values
              WHERE status = 'active'
                AND (source_memory_id = ?1
                     OR id IN (SELECT value_id FROM fact_events
                                WHERE evidence_memory_id = ?1 AND value_id IS NOT NULL))
              ORDER BY seq",
        )?;
        stmt.query_map([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect::<rusqlite::Result<_>>()?
    };
    let mut out = Vec::new();
    for (value_id, slot, kind, value) in stated {
        let view = crate::facts::view::slot_view(conn, &slot)?;
        let current = view.current.as_ref();
        out.push(json!({
            "project": view.project,
            "subject": view.subject,
            "predicate": view.predicate,
            "qualifier": view.qualifier,
            "this": (kind == "value").then_some(value),
            "current": current.and_then(|c| c.value.clone()),
            "since": current.map(|c| c.valid_from.clone()),
            "is_current": current.is_some_and(|c| c.id == value_id),
        }));
    }
    Ok(out)
}

#[cfg(test)]
#[path = "recall_tests.rs"]
mod tests;
