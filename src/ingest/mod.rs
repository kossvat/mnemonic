//! Local, bounded transcript ingress. Replay returns the bounded capture
//! event, and a terminal receipt drops the payload that produced it.
mod schema;
#[cfg(test)]
mod tests;

use anyhow::{Result, bail, ensure};
use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};

use crate::embedding::{Embedding, embedding_to_bytes};
use crate::event::{Event, MemoryEntry};
use crate::storage::Storage;

pub(crate) use schema::install;
pub const RAW_TTL_DAYS: i64 = 7;
const CONSUMER: &str = "transcript-memory-v1";
const SCHEMA_VERSION: i64 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IngestCursor {
    pub stream: String,
    pub generation: u64,
    pub offset: u64,
    pub file_id: String,
    pub prefix_len: u64,
    pub prefix_hash: String,
    pub anchor_hash: String,
}

/// What an accepted turn carries into the store. The unabridged turn text is
/// deliberately NOT here: nothing ever read it back (replay uses `event`
/// alone), so keeping a second copy of private conversation text was storage
/// without a reader. Deserialization ignores unknown fields, so rows written
/// before this still load.
#[derive(Serialize, Deserialize)]
pub struct IngestPayload {
    pub event: Event,
}

pub struct IngestRecord {
    pub source_key: String,
    pub source_at: Option<String>,
    pub observed_at: DateTime<Utc>,
    pub payload: IngestPayload,
}

pub struct PendingEvent {
    pub seq: i64,
    pub event: Event,
}

pub enum SkipReason {
    Filtered,
    Duplicate,
    LowImportance,
}
impl SkipReason {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Filtered => "filtered",
            Self::Duplicate => "duplicate",
            Self::LowImportance => "low_importance",
        }
    }
}

pub enum ProcessingDecision<'a> {
    Save {
        entry: &'a MemoryEntry,
        embedding: Option<&'a Embedding>,
        enqueue_extraction: bool,
        /// The update link the save gate planned, written with the memory.
        link: Option<&'a crate::updates::plan::LinkPlan>,
    },
    Skip(SkipReason),
}

/// Replay is bounded by observation time, never by the original source time.
/// Source identities and terminal receipts outlive the raw payload's TTL.
#[derive(Debug, Serialize)]
pub struct IngestRetention {
    pub raw_ttl_days: i64,
    pub replay_since: DateTime<Utc>,
    pub oldest_pending_seq: Option<i64>,
    pub pending: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Fault {
    None,
    #[cfg(test)]
    BeforeIngestCommit,
    #[cfg(test)]
    AfterIngestCommit,
    #[cfg(test)]
    BeforeProcessingCommit,
    #[cfg(test)]
    AfterProcessingCommit,
}
impl Fault {
    fn hit(self, _ingest: bool, _before: bool) {
        #[cfg(test)]
        if matches!(
            (self, _ingest, _before),
            (Self::BeforeIngestCommit, true, true)
                | (Self::AfterIngestCommit, true, false)
                | (Self::BeforeProcessingCommit, false, true)
                | (Self::AfterProcessingCommit, false, false)
        ) {
            // Deliberately bypass destructors to exercise SQLite recovery.
            std::process::exit(91);
        }
    }
}

fn timestamp(time: DateTime<Utc>) -> String {
    time.to_rfc3339_opts(SecondsFormat::Nanos, true)
}
fn floor(now: DateTime<Utc>) -> DateTime<Utc> {
    now - chrono::Duration::days(RAW_TTL_DAYS)
}
fn cursor_on(conn: &Connection, stream: &str) -> Result<Option<IngestCursor>> {
    Ok(conn
        .query_row(
            "SELECT stream, generation, offset, file_id, prefix_len, prefix_hash, anchor_hash
         FROM ingest_cursors WHERE stream = ?1",
            [stream],
            |r| {
                Ok(IngestCursor {
                    stream: r.get(0)?,
                    generation: r.get(1)?,
                    offset: r.get(2)?,
                    file_id: r.get(3)?,
                    prefix_len: r.get(4)?,
                    prefix_hash: r.get(5)?,
                    anchor_hash: r.get(6)?,
                })
            },
        )
        .optional()?)
}

/// Record, once and for good, that this store holds transcript memories
/// captured before durable ingress. Those carry no key, so history must never
/// reach back into their era; the fact has to outlive the memories themselves,
/// or forgetting the last of them would let history bring them all back.
pub(crate) fn remember_legacy_capture(conn: &Connection) -> Result<()> {
    let sources = [
        serde_json::to_string(&crate::event::EventSource::ConversationWatcher)?,
        serde_json::to_string(&crate::event::EventSource::CodexWatcher)?,
    ];
    conn.execute(
        "INSERT OR IGNORE INTO ingest_facts (name, value)
         SELECT 'legacy_capture', '1'
         WHERE NOT EXISTS (SELECT 1 FROM ingest_facts WHERE name = 'legacy_capture')
           AND EXISTS (SELECT 1 FROM memories m WHERE m.source IN (?1, ?2)
                AND NOT EXISTS (SELECT 1 FROM consumer_receipts r WHERE r.memory_id = m.id))",
        params![sources[0], sources[1]],
    )?;
    Ok(())
}

#[cfg(test)]
thread_local! {
    /// Standalone cursor lookups issued on this thread (reads that are not
    /// part of a write transaction). Lets a test prove that a poll tick costs
    /// one bulk read instead of one lookup per transcript.
    pub(crate) static CURSOR_LOOKUPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// Makes the next bulk cursor read fail, to prove a failed read fails the
    /// tick instead of becoming an empty snapshot.
    pub(crate) static FAIL_NEXT_BULK_READ: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}
fn count_cursor_lookup() {
    #[cfg(test)]
    CURSOR_LOOKUPS.with(|c| c.set(c.get() + 1));
}

impl Storage {
    /// Whether this store ever held transcript memories captured before
    /// durable ingress. See `remember_legacy_capture`.
    pub fn had_legacy_capture(&self) -> Result<bool> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        Ok(conn
            .query_row(
                "SELECT 1 FROM ingest_facts WHERE name = 'legacy_capture'",
                [],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    pub fn ingest_cursor(&self, stream: &str) -> Result<Option<IngestCursor>> {
        count_cursor_lookup();
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        cursor_on(&conn, stream)
    }

    /// Every cursor whose stream key starts with `prefix`, in ONE query. A
    /// watcher reads its whole namespace once per tick instead of issuing one
    /// lookup (and one lock of the shared connection) per transcript. The
    /// mutex is released before the caller touches the filesystem.
    ///
    /// A failure is returned as an error, never as an empty map: an empty map
    /// would make every transcript look new and be re-read from byte zero.
    pub fn ingest_cursors_with_prefix(
        &self,
        prefix: &str,
    ) -> Result<std::collections::HashMap<String, IngestCursor>> {
        count_cursor_lookup();
        #[cfg(test)]
        if FAIL_NEXT_BULK_READ.with(|f| f.replace(false)) {
            bail!("injected bulk cursor read failure");
        }
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let mut stmt = conn.prepare(
            "SELECT stream, generation, offset, file_id, prefix_len, prefix_hash, anchor_hash
               FROM ingest_cursors WHERE substr(stream, 1, length(?1)) = ?1",
        )?;
        let rows = stmt.query_map([prefix], |r| {
            Ok(IngestCursor {
                stream: r.get(0)?,
                generation: r.get(1)?,
                offset: r.get(2)?,
                file_id: r.get(3)?,
                prefix_len: r.get(4)?,
                prefix_hash: r.get(5)?,
                anchor_hash: r.get(6)?,
            })
        })?;
        let mut cursors = std::collections::HashMap::new();
        for cursor in rows {
            let cursor = cursor?;
            cursors.insert(cursor.stream.clone(), cursor);
        }
        Ok(cursors)
    }

    /// The cursor is an acknowledgement. Its compare-and-swap and all accepted
    /// events commit together, including an empty batch/unfinished first line.
    pub fn append_ingest(
        &self,
        expected: Option<&IngestCursor>,
        next: &IngestCursor,
        records: &[IngestRecord],
    ) -> Result<()> {
        self.append_ingest_with_fault(expected, next, records, Fault::None)
    }

    fn append_ingest_with_fault(
        &self,
        expected: Option<&IngestCursor>,
        next: &IngestCursor,
        records: &[IngestRecord],
        fault: Fault,
    ) -> Result<()> {
        ensure!(
            next.offset <= i64::MAX as u64 && next.generation <= i64::MAX as u64,
            "ingress cursor exceeds SQLite integer range"
        );
        if let Some(previous) = expected {
            ensure!(
                previous.stream == next.stream
                    && (next.generation > previous.generation
                        || (next.generation == previous.generation
                            && next.offset >= previous.offset)),
                "ingress cursor cannot move backwards within a generation"
            );
        }
        let mut conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        ensure!(
            cursor_on(&tx, &next.stream)?.as_ref() == expected,
            "stale ingress cursor"
        );
        for record in records {
            tx.execute(
                "INSERT INTO ingest_events (source_key, source_at, observed_at, schema_version, payload)
                 VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT(source_key) DO NOTHING",
                params![record.source_key, record.source_at, timestamp(record.observed_at),
                    SCHEMA_VERSION, serde_json::to_string(&record.payload)?],
            )?;
        }
        tx.execute(
            "INSERT INTO ingest_cursors (stream, generation, offset, file_id, prefix_len, prefix_hash, anchor_hash)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(stream) DO UPDATE SET generation = excluded.generation, offset = excluded.offset,
                 file_id = excluded.file_id, prefix_len = excluded.prefix_len,
                 prefix_hash = excluded.prefix_hash, anchor_hash = excluded.anchor_hash",
            params![next.stream, next.generation, next.offset, next.file_id,
                next.prefix_len, next.prefix_hash, next.anchor_hash],
        )?;
        fault.hit(true, true);
        tx.commit()?;
        fault.hit(true, false);
        Ok(())
    }

    /// When durable ingress accepted its first turn in this store, if ever.
    pub fn ingress_started_at(&self) -> Result<Option<DateTime<Utc>>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let first: Option<String> =
            conn.query_row("SELECT min(observed_at) FROM ingest_events", [], |row| {
                row.get(0)
            })?;
        Ok(first
            .and_then(|text| DateTime::parse_from_rfc3339(&text).ok())
            .map(|time| time.with_timezone(&Utc)))
    }

    /// Which of these keys were ever ingested. A settled event keeps its key
    /// after its payload is dropped, so this remembers everything since the
    /// durable store existed.
    pub fn ingest_keys_known(&self, keys: &[String]) -> Result<std::collections::HashSet<String>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let mut stmt = conn.prepare("SELECT 1 FROM ingest_events WHERE source_key = ?1")?;
        let mut known = std::collections::HashSet::new();
        for key in keys {
            if stmt.exists([key])? {
                known.insert(key.clone());
            }
        }
        Ok(known)
    }

    /// Queue turns read from transcript history. No cursor moves: live
    /// capture keeps its own position. A key already present is left alone,
    /// so this can run again safely. Returns how many were new.
    pub fn append_history(&self, records: &[IngestRecord]) -> Result<usize> {
        let mut conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut added = 0;
        for record in records {
            added += tx.execute(
                "INSERT INTO ingest_events (source_key, source_at, observed_at, schema_version, payload)
                 VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT(source_key) DO NOTHING",
                params![record.source_key, record.source_at, timestamp(record.observed_at),
                    SCHEMA_VERSION, serde_json::to_string(&record.payload)?],
            )?;
        }
        tx.commit()?;
        Ok(added)
    }

    /// Even if maintenance has not run yet, expired events and deletion
    /// suppressions cannot enter processing.
    pub fn pending_ingest(&self, now: DateTime<Utc>, limit: usize) -> Result<Vec<PendingEvent>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let mut stmt = conn.prepare(
            "SELECT e.seq, e.payload FROM ingest_events e
             WHERE e.payload IS NOT NULL AND e.schema_version = ?1 AND e.observed_at >= ?2
               AND NOT EXISTS (SELECT 1 FROM consumer_receipts r WHERE r.event_id = e.seq
                               AND (r.consumer = ?3 OR r.outcome = 'forgotten'))
             ORDER BY e.seq LIMIT ?4",
        )?;
        let rows = stmt.query_map(
            params![
                SCHEMA_VERSION,
                timestamp(floor(now)),
                CONSUMER,
                limit as i64
            ],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)),
        )?;
        rows.map(|row| {
            let (seq, payload) = row?;
            let payload: IngestPayload = serde_json::from_str(&payload)?;
            Ok(PendingEvent {
                seq,
                event: payload.event,
            })
        })
        .collect()
    }

    /// The terminal outcome and memory (including optional extraction enqueue)
    /// are one transaction. Errors leave the event pending; retries are no-ops
    /// after commit, regardless of process death or subsequent memory deletion.
    pub fn finish_ingest(
        &self,
        seq: i64,
        decision: ProcessingDecision<'_>,
        now: DateTime<Utc>,
    ) -> Result<Option<MemoryEntry>> {
        self.finish_ingest_with_fault(seq, decision, now, Fault::None)
    }

    fn finish_ingest_with_fault(
        &self,
        seq: i64,
        decision: ProcessingDecision<'_>,
        now: DateTime<Utc>,
        fault: Fault,
    ) -> Result<Option<MemoryEntry>> {
        let mut conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let terminal: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM consumer_receipts WHERE event_id = ?1
                           AND (consumer = ?2 OR outcome = 'forgotten'))",
            params![seq, CONSUMER],
            |r| r.get(0),
        )?;
        if terminal {
            return Ok(None);
        }
        let row: Option<(i64, Option<String>, String)> = tx
            .query_row(
                "SELECT schema_version, payload, observed_at FROM ingest_events WHERE seq = ?1",
                [seq],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let Some((version, payload, observed_at)) = row else {
            bail!("unknown ingress event");
        };
        ensure!(
            version == SCHEMA_VERSION,
            "unsupported ingress schema version"
        );
        let payload = payload.filter(|_| observed_at >= timestamp(floor(now)));
        let (outcome, saved) = if let Some(payload) = payload {
            match decision {
                ProcessingDecision::Skip(reason) => (reason.as_str(), None),
                ProcessingDecision::Save {
                    entry,
                    embedding,
                    enqueue_extraction,
                    link,
                } => {
                    let payload: IngestPayload = serde_json::from_str(&payload)?;
                    let mut entry = entry.clone();
                    // Deterministic id = replay cannot mint a second memory.
                    // The event id is reused AS IS: digests, the context and
                    // the CLI cite a memory by the first 8 characters of its
                    // id, and a constant prefix would collapse every
                    // transcript-derived citation into the same few strings.
                    entry.id = payload.event.id.clone();
                    entry.timestamp = payload.event.timestamp;
                    // Plain INSERT: never replace a row or silently erase its links.
                    tx.execute(
                        "INSERT INTO memories (id, timestamp, title, content, memory_type, tags, source, importance, metadata, embedding)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                        params![entry.id, entry.timestamp.to_rfc3339(), entry.title, entry.content,
                            entry.memory_type.to_string(), serde_json::to_string(&entry.tags)?,
                            serde_json::to_string(&entry.source)?, entry.importance, entry.metadata.to_string(),
                            embedding.map(|value| embedding_to_bytes(value))],
                    )?;
                    // With the memory's final id and original time, before the
                    // receipt: a changed-price turn is saved and linked, never
                    // receipted as a duplicate with its text erased.
                    if let Some(plan) = link {
                        crate::updates::plan::commit_link(&tx, &entry, plan, "daemon")?;
                    }
                    if enqueue_extraction {
                        tx.execute(
                            "INSERT OR IGNORE INTO extraction_queue (memory_id) VALUES (?1)",
                            [&entry.id],
                        )?;
                    }
                    ("saved", Some(entry))
                }
            }
        } else {
            ("expired", None)
        };
        tx.execute(
            "INSERT INTO consumer_receipts (consumer, event_id, outcome, memory_id) VALUES (?1, ?2, ?3, ?4)",
            params![CONSUMER, seq, outcome, saved.as_ref().map(|entry| &entry.id)],
        )?;
        // The receipt is terminal, so the payload has no reader left. Dropping
        // it in the SAME transaction is what gives a SKIPPED turn a deletion
        // path at all: it produced no memory, so `forget` has no id to aim at
        // and only the TTL would ever have cleared its text.
        tx.execute(
            "UPDATE ingest_events SET payload = NULL WHERE seq = ?1",
            [seq],
        )?;
        fault.hit(false, true);
        tx.commit()?;
        fault.hit(false, false);
        Ok(saved)
    }

    /// Logical TTL applies to processed and unprocessed raw payloads alike.
    /// Empty source-key tombstones and receipts are intentionally retained.
    pub fn expire_ingest(&self, now: DateTime<Utc>) -> Result<usize> {
        let mut conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let cutoff = timestamp(floor(now));
        tx.execute(
            "INSERT OR IGNORE INTO consumer_receipts (consumer, event_id, outcome)
             SELECT ?1, seq, 'expired' FROM ingest_events WHERE observed_at < ?2 AND payload IS NOT NULL",
            params![CONSUMER, cutoff])?;
        let expired = tx.execute(
            "UPDATE ingest_events SET payload = NULL WHERE observed_at < ?1 AND payload IS NOT NULL", [&cutoff])?;
        tx.commit()?;
        Ok(expired)
    }

    pub fn ingest_retention(&self, now: DateTime<Utc>) -> Result<IngestRetention> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let replay_since = floor(now);
        let (oldest_pending_seq, pending) = conn.query_row(
            "SELECT MIN(e.seq), COUNT(*) FROM ingest_events e
             WHERE e.payload IS NOT NULL AND e.observed_at >= ?1
               AND NOT EXISTS (SELECT 1 FROM consumer_receipts r WHERE r.event_id = e.seq
                               AND (r.consumer = ?2 OR r.outcome = 'forgotten'))",
            params![timestamp(replay_since), CONSUMER],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        Ok(IngestRetention {
            raw_ttl_days: RAW_TTL_DAYS,
            replay_since,
            oldest_pending_seq,
            pending,
        })
    }
}
