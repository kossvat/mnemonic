//! Memory-level update links: `new_id` updates `old_id`.
//!
//! Keyed on memory ids, so they survive graph rebuilds, merges and renames.
//! There is no foreign key: a trigger on `memories` keeps the links whole
//! when a memory is deleted by any path, old binaries included. Deleting the
//! middle of a chain joins its neighbours; deleting the newest end makes the
//! one before it current again.

use anyhow::{Result, ensure};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::json;

use super::scan::{Class, Value};

pub fn install(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS memory_updates (
             new_id TEXT NOT NULL,
             old_id TEXT NOT NULL,
             status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'dismissed')),
             rule TEXT NOT NULL,
             rule_version INTEGER NOT NULL DEFAULT 1,
             value_class TEXT,
             was_values TEXT NOT NULL DEFAULT '[]',
             now_values TEXT NOT NULL DEFAULT '[]',
             similarity REAL,
             actor TEXT NOT NULL,
             created_at TEXT NOT NULL,
             PRIMARY KEY (new_id, old_id),
             CHECK (new_id <> old_id)
         ) WITHOUT ROWID;
         CREATE INDEX IF NOT EXISTS idx_memory_updates_old ON memory_updates(old_id, status);
         CREATE TRIGGER IF NOT EXISTS trg_memory_updates_source_deleted
         AFTER DELETE ON memories BEGIN
             INSERT OR IGNORE INTO memory_updates
                 (new_id, old_id, status, rule, value_class, was_values, now_values, actor, created_at)
             SELECT r.new_id, p.old_id, 'active', 'splice', r.value_class,
                    p.was_values, r.now_values, 'trigger',
                    strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
               FROM memory_updates r
               JOIN memory_updates p ON r.old_id = OLD.id AND p.new_id = OLD.id
              WHERE r.status = 'active' AND p.status = 'active' AND r.new_id <> p.old_id;
             DELETE FROM memory_updates WHERE new_id = OLD.id OR old_id = OLD.id;
         END;
         CREATE TABLE IF NOT EXISTS memory_reaffirmed (
             memory_id TEXT PRIMARY KEY,
             at TEXT NOT NULL
         ) WITHOUT ROWID;
         CREATE TRIGGER IF NOT EXISTS trg_memory_reaffirmed_deleted
         AFTER DELETE ON memories BEGIN
             DELETE FROM memory_reaffirmed WHERE memory_id = OLD.id;
         END;",
    )?;
    Ok(())
}

/// Memories that hold part of a value's history: both ends of every update
/// link, every fact value's source and every memory cited as evidence of a
/// fact statement. Reflection must not fuse them and retention must not
/// delete them: either would rewrite what the history says holds now.
pub const HISTORY_MEMBERS: &str = "SELECT new_id FROM memory_updates
     UNION SELECT old_id FROM memory_updates
     UNION SELECT source_memory_id FROM fact_values WHERE source_memory_id IS NOT NULL
     UNION SELECT evidence_memory_id FROM fact_events WHERE evidence_memory_id IS NOT NULL";

/// A later statement of the same value was dropped as a duplicate of `id`:
/// keep when it was said, so a value backfilled from before that time
/// cannot pass itself off as newer than it.
pub fn reaffirm(conn: &Connection, id: &str, at: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO memory_reaffirmed (memory_id, at) VALUES (?1, ?2)
         ON CONFLICT(memory_id) DO UPDATE SET at = max(at, excluded.at)",
        params![id, at],
    )?;
    Ok(())
}

/// Memories saved under project `old` belong to `new` after a merge or a
/// rename of the project entity.
pub fn rekey_project(conn: &Connection, old: &str, new: &str) -> Result<()> {
    conn.execute(
        "UPDATE memories
            SET metadata = json_set(metadata, '$.project', ?1, '$.project_key', ?2)
          WHERE json_valid(metadata)
            AND json_extract(metadata, '$.project_key') = ?3",
        params![
            new,
            crate::followups::project_key(new),
            crate::followups::project_key(old)
        ],
    )?;
    Ok(())
}

/// Why a link exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rule {
    /// The scanner saw the same thing with a changed value.
    ValueDiff,
}

impl Rule {
    fn as_str(self) -> &'static str {
        match self {
            Rule::ValueDiff => "value_diff",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Link {
    pub new_id: String,
    pub old_id: String,
    pub rule: Rule,
    pub class: Class,
    pub was: Vec<Value>,
    pub now: Vec<Value>,
    pub similarity: Option<f32>,
    pub actor: &'static str,
}

fn values_json(values: &[Value]) -> String {
    json!(
        values
            .iter()
            .map(|v| json!({"n": v.key, "s": v.surface}))
            .collect::<Vec<_>>()
    )
    .to_string()
}

/// Record a link. Both memories must exist and be live; a link that is
/// already there is left as it is. Returns whether a row was written.
pub fn insert(conn: &Connection, link: &Link) -> Result<bool> {
    ensure!(link.new_id != link.old_id, "a memory cannot update itself");
    let live: i64 = conn.query_row(
        "SELECT COUNT(*) FROM memories WHERE id IN (?1, ?2) AND superseded_by IS NULL",
        params![link.new_id, link.old_id],
        |r| r.get(0),
    )?;
    ensure!(live == 2, "both memories of an update link must be live");
    let written = conn.execute(
        "INSERT OR IGNORE INTO memory_updates
             (new_id, old_id, rule, value_class, was_values, now_values, similarity, actor, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
        params![
            link.new_id,
            link.old_id,
            link.rule.as_str(),
            link.class.as_str(),
            values_json(&link.was),
            values_json(&link.now),
            link.similarity,
            link.actor,
        ],
    )?;
    Ok(written == 1)
}

/// The newest memory in `id`'s update chain: follow "updated by" links to
/// the end. In a fork the newest end wins. A memory nothing updates is its
/// own head. `None` when the chain has no end (a cycle): nothing can be
/// decided from it.
pub fn head(conn: &Connection, id: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            // UNION visits each memory once, so any cycle ends the walk.
            "WITH RECURSIVE chain(id) AS (
                 SELECT ?1
                 UNION
                 SELECT u.new_id FROM memory_updates u JOIN chain c ON u.old_id = c.id
                  WHERE u.status = 'active'
             )
             SELECT c.id FROM chain c JOIN memories m ON m.id = c.id
              WHERE NOT EXISTS (SELECT 1 FROM memory_updates u
                                 WHERE u.old_id = c.id AND u.status = 'active')
              ORDER BY max(m.timestamp, COALESCE((SELECT r.at FROM memory_reaffirmed r
                                                 WHERE r.memory_id = m.id), m.timestamp)) DESC,
                       c.id DESC LIMIT 1",
            params![id],
            |r| r.get(0),
        )
        .optional()?)
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
