//! Facts v2 tables. Additive only: the old `facts` table is left exactly as
//! it was (old binaries keep reading and writing it), and its rows are
//! imported by `legacy::catch_up` on every open.

use anyhow::Result;
use rusqlite::Connection;

pub fn install(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS fact_slots (
             id TEXT PRIMARY KEY,
             scope_key TEXT NOT NULL,
             scope TEXT NOT NULL DEFAULT '',
             subject_key TEXT NOT NULL CHECK (subject_key <> ''),
             subject TEXT NOT NULL,
             predicate TEXT NOT NULL CHECK (predicate <> ''),
             predicate_label TEXT NOT NULL,
             qualifier_key TEXT NOT NULL DEFAULT '',
             qualifier TEXT NOT NULL DEFAULT '',
             revision INTEGER NOT NULL DEFAULT 0,
             created_at TEXT NOT NULL,
             updated_at TEXT NOT NULL,
             UNIQUE (scope_key, subject_key, predicate, qualifier_key)
         );
         CREATE INDEX IF NOT EXISTS idx_fact_slots_subject ON fact_slots(subject_key);

         CREATE TABLE IF NOT EXISTS fact_values (
             seq INTEGER PRIMARY KEY AUTOINCREMENT,
             id TEXT NOT NULL UNIQUE,
             slot_id TEXT NOT NULL REFERENCES fact_slots(id) ON DELETE CASCADE,
             kind TEXT NOT NULL DEFAULT 'value' CHECK (kind IN ('value', 'retraction')),
             value TEXT NOT NULL,
             value_norm TEXT NOT NULL,
             status TEXT NOT NULL CHECK (status IN ('active', 'pending_review', 'rejected')),
             trust TEXT NOT NULL CHECK (trust IN ('manual', 'declared', 'confirmed', 'provisional')),
             valid_from TEXT NOT NULL,
             valid_from_ms INTEGER NOT NULL,
             -- The latest time a reconfirmation said the value held; NULL
             -- for its start.
             asserted_ms INTEGER,
             confidence REAL NOT NULL DEFAULT 1.0 CHECK (confidence BETWEEN 0 AND 1),
             source_memory_id TEXT,
             source_forgotten_at TEXT,
             evidence TEXT,
             actor TEXT NOT NULL,
             agent TEXT,
             extractor TEXT,
             legacy_fact_id TEXT UNIQUE,
             legacy_valid_to TEXT,
             reconfirmed_at TEXT,
             reconfirm_count INTEGER NOT NULL DEFAULT 0,
             created_at TEXT NOT NULL
         );
         CREATE INDEX IF NOT EXISTS idx_fact_values_chain
             ON fact_values(slot_id, status, valid_from_ms, seq);
         CREATE INDEX IF NOT EXISTS idx_fact_values_source
             ON fact_values(source_memory_id) WHERE source_memory_id IS NOT NULL;

         -- Audit and idempotency. Ids and codes only: a value never lands
         -- here, so forgetting a value erases it.
         CREATE TABLE IF NOT EXISTS fact_events (
             id INTEGER PRIMARY KEY AUTOINCREMENT,
             slot_id TEXT NOT NULL,
             value_id TEXT,
             action TEXT NOT NULL CHECK (action IN ('create', 'update', 'reconfirm', 'history',
                 'retract', 'pending_review', 'legacy_import', 'merge', 'source_forgotten',
                 'forget_value')),
             outcome TEXT NOT NULL,
             revision_after INTEGER,
             actor TEXT NOT NULL,
             agent TEXT,
             evidence_memory_id TEXT,
             occurred_at TEXT NOT NULL,
             request_id TEXT NOT NULL UNIQUE
         );
         CREATE INDEX IF NOT EXISTS idx_fact_events_slot ON fact_events(slot_id, id);

         -- Forgetting a memory: a provisional value it sourced goes with it;
         -- any other value stays, losing only the link and its evidence.
         CREATE TRIGGER IF NOT EXISTS trg_fact_values_source_deleted
         AFTER DELETE ON memories BEGIN
             INSERT OR IGNORE INTO fact_events
                 (slot_id, value_id, action, outcome, actor, evidence_memory_id, occurred_at, request_id)
             SELECT slot_id, id, 'source_forgotten',
                    CASE WHEN trust = 'provisional' THEN 'value_removed' ELSE 'link_cleared' END,
                    'trigger', OLD.id, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                    'forget:' || OLD.id || ':' || id
               FROM fact_values WHERE source_memory_id = OLD.id;
             UPDATE fact_slots
                SET revision = revision + 1, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
              WHERE id IN (SELECT slot_id FROM fact_values
                            WHERE source_memory_id = OLD.id AND trust = 'provisional');
             DELETE FROM fact_values WHERE source_memory_id = OLD.id AND trust = 'provisional';
             UPDATE fact_values
                SET source_memory_id = NULL, evidence = NULL,
                    source_forgotten_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
              WHERE source_memory_id = OLD.id;
         END;",
    )?;
    Ok(())
}

#[cfg(test)]
#[path = "schema_tests.rs"]
mod tests;
