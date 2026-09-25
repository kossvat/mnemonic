use anyhow::Result;
use rusqlite::Connection;

/// Called after all memories table rebuilds. Additive and repeatable on an
/// existing store; no raw ingress columns are added to exportable memories.
pub(crate) fn install(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS ingest_events (
             seq INTEGER PRIMARY KEY AUTOINCREMENT,
             source_key TEXT NOT NULL UNIQUE,
             source_at TEXT,
             observed_at TEXT NOT NULL,
             schema_version INTEGER NOT NULL,
             payload TEXT
         );
         CREATE INDEX IF NOT EXISTS idx_ingest_retention ON ingest_events(observed_at)
             WHERE payload IS NOT NULL;
         CREATE TABLE IF NOT EXISTS ingest_cursors (
             stream TEXT PRIMARY KEY,
             generation INTEGER NOT NULL CHECK(generation >= 0),
             offset INTEGER NOT NULL CHECK(offset >= 0),
             file_id TEXT NOT NULL,
             prefix_len INTEGER NOT NULL,
             prefix_hash TEXT NOT NULL,
             anchor_hash TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS consumer_receipts (
             consumer TEXT NOT NULL,
             event_id INTEGER NOT NULL REFERENCES ingest_events(seq),
             outcome TEXT NOT NULL,
             memory_id TEXT,
             PRIMARY KEY(consumer, event_id)
         );
         CREATE INDEX IF NOT EXISTS idx_ingest_receipt_memory ON consumer_receipts(memory_id)
             WHERE memory_id IS NOT NULL;
         CREATE TABLE IF NOT EXISTS ingest_facts (
             name TEXT PRIMARY KEY,
             value TEXT NOT NULL
         );
         CREATE INDEX IF NOT EXISTS idx_ingest_receipt_event ON consumer_receipts(event_id);
         -- Keep suppression independently of memory liveness. No FK cascade may
         -- erase the receipt when any delete path removes its derived memory.
         CREATE TRIGGER IF NOT EXISTS trg_ingest_memory_deleted
         AFTER DELETE ON memories BEGIN
             UPDATE ingest_events SET payload = NULL WHERE seq IN
                 (SELECT event_id FROM consumer_receipts WHERE memory_id = OLD.id);
             UPDATE consumer_receipts SET outcome = 'forgotten' WHERE memory_id = OLD.id;
         END;",
    )?;
    // Rows written before the turn text was dropped still carry it. Strip the
    // field in place: identities, timestamps, receipts and cursors are
    // untouched, and the schema version is deliberately NOT bumped, which
    // would strand these rows behind the version filter in pending_ingest.
    conn.execute(
        "UPDATE ingest_events SET payload = json_remove(payload, '$.raw_turn')
          WHERE payload IS NOT NULL AND json_extract(payload, '$.raw_turn') IS NOT NULL",
        [],
    )?;
    // Rows that reached a terminal receipt under an older build kept their
    // payload, because clearing it happens at receipt time. Those have no
    // reader left either, so the same rule is applied retroactively here. A
    // row still PENDING keeps its payload; it is the only kind that replays.
    conn.execute(
        "UPDATE ingest_events SET payload = NULL
          WHERE payload IS NOT NULL
            AND EXISTS (SELECT 1 FROM consumer_receipts r WHERE r.event_id = ingest_events.seq)",
        [],
    )?;
    Ok(())
}
