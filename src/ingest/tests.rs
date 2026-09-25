//! Executable contract, written before the ingress implementation. Crash cases
//! exit a child process at the real SQLite boundaries (no unwinding/rollback).
use super::*;
use crate::event::{EventKind, EventSource, MemoryType};
use std::path::PathBuf;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("mnemonic-ingress-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    fn db(&self) -> PathBuf {
        self.0.join("memory.db")
    }
    fn open(&self) -> Storage {
        Storage::open(&self.db()).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn cursor(offset: u64) -> IngestCursor {
    IngestCursor {
        stream: "demoapp/session".into(),
        generation: 0,
        offset,
        file_id: "file-1".into(),
        prefix_len: 0,
        prefix_hash: String::new(),
        anchor_hash: String::new(),
    }
}
fn record() -> IngestRecord {
    let mut event = Event::new(
        EventSource::ConversationWatcher,
        EventKind::Custom("conversation_decision".into()),
        "Decision: use SQLite for demoapp",
    );
    event.timestamp = "2025-01-02T03:04:05.123456Z".parse().unwrap();
    IngestRecord {
        source_key: "demoapp/session/message-1".into(),
        source_at: Some("2025-01-02T04:04:05.123456+01:00".into()),
        observed_at: Utc::now(),
        payload: IngestPayload { event },
    }
}
fn entry() -> MemoryEntry {
    MemoryEntry::new(
        "Decision",
        "Decision: use SQLite for demoapp",
        MemoryType::Decision,
        EventSource::ConversationWatcher,
    )
}
fn finish(storage: &Storage, seq: i64, fault: Fault) -> Option<MemoryEntry> {
    storage
        .finish_ingest_with_fault(
            seq,
            ProcessingDecision::Save {
                entry: &entry(),
                embedding: None,
                enqueue_extraction: true,
                link: None,
            },
            Utc::now(),
            fault,
        )
        .unwrap()
}
fn receipt(storage: &Storage, seq: i64) -> String {
    storage
        .conn
        .lock()
        .unwrap()
        .query_row(
            "SELECT outcome FROM consumer_receipts WHERE consumer = ?1 AND event_id = ?2",
            rusqlite::params![CONSUMER, seq],
            |row| row.get(0),
        )
        .unwrap()
}

#[test]
fn ingress_crash_child() {
    let Ok(db) = std::env::var("MNEMONIC_INGRESS_CRASH_DB") else {
        return;
    };
    let fault = match std::env::var("MNEMONIC_INGRESS_CRASH_POINT")
        .unwrap()
        .as_str()
    {
        "before_ingest" => Fault::BeforeIngestCommit,
        "after_ingest" => Fault::AfterIngestCommit,
        "before_processing" => Fault::BeforeProcessingCommit,
        "after_processing" => Fault::AfterProcessingCommit,
        _ => panic!("invalid test fault"),
    };
    let storage = Storage::open(std::path::Path::new(&db)).unwrap();
    storage
        .append_ingest_with_fault(None, &cursor(100), &[record()], fault)
        .unwrap();
    let pending = storage.pending_ingest(Utc::now(), 10).unwrap();
    finish(&storage, pending[0].seq, fault);
    panic!("crash point was not reached");
}

#[test]
fn ingress_crash_reopen_replay_has_no_loss_or_duplicate_at_all_four_boundaries() {
    for point in [
        "before_ingest",
        "after_ingest",
        "before_processing",
        "after_processing",
    ] {
        let fixture = Fixture::new();
        drop(fixture.open());
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "ingest::tests::ingress_crash_child",
                "--nocapture",
            ])
            .env("MNEMONIC_INGRESS_CRASH_DB", fixture.db())
            .env("MNEMONIC_INGRESS_CRASH_POINT", point)
            .output()
            .unwrap();
        assert_eq!(
            result.status.code(),
            Some(91),
            "{point}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        let storage = fixture.open();
        if point == "before_ingest" {
            assert!(storage.ingest_cursor(&cursor(0).stream).unwrap().is_none());
            assert!(storage.pending_ingest(Utc::now(), 10).unwrap().is_empty());
            storage
                .append_ingest(None, &cursor(100), &[record()])
                .unwrap();
        } else {
            assert_eq!(
                storage
                    .ingest_cursor(&cursor(0).stream)
                    .unwrap()
                    .unwrap()
                    .offset,
                100
            );
        }
        assert_eq!(
            storage.count().unwrap(),
            usize::from(point == "after_processing")
        );
        for pending in storage.pending_ingest(Utc::now(), 10).unwrap() {
            finish(&storage, pending.seq, Fault::None);
        }
        assert_eq!(storage.count().unwrap(), 1, "{point}");
        assert!(storage.pending_ingest(Utc::now(), 10).unwrap().is_empty());
        assert_eq!(storage.extraction_queue_count().unwrap(), 1);
        drop(storage);
        let storage = fixture.open();
        // Source reread and processing retry both remain idempotent.
        storage
            .append_ingest(Some(&cursor(100)), &cursor(200), &[record()])
            .unwrap();
        assert!(finish(&storage, 1, Fault::None).is_none());
        assert_eq!(storage.count().unwrap(), 1);
    }
}

#[test]
fn ingress_preserves_source_and_observed_time_through_processing() {
    let fixture = Fixture::new();
    let storage = fixture.open();
    let rec = record();
    storage
        .append_ingest(None, &cursor(100), std::slice::from_ref(&rec))
        .unwrap();
    let pending = storage.pending_ingest(Utc::now(), 1).unwrap();
    assert_eq!(pending[0].event.timestamp, rec.payload.event.timestamp);
    let saved = finish(&storage, pending[0].seq, Fault::None).unwrap();
    assert_eq!(saved.timestamp, rec.payload.event.timestamp);
    let conn = storage.conn.lock().unwrap();
    let (source, observed): (String, String) = conn
        .query_row(
            "SELECT source_at, observed_at FROM ingest_events",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(Some(source), rec.source_at);
    assert_eq!(observed, timestamp(rec.observed_at));
}

#[test]
fn ingress_memory_ids_stay_citable_by_their_first_eight_characters() {
    let fixture = Fixture::new();
    let storage = fixture.open();
    let (mut first, mut second) = (record(), record());
    first.source_key = "demoapp/session/message-1".into();
    second.source_key = "demoapp/session/message-2".into();
    storage
        .append_ingest(None, &cursor(100), &[first, second])
        .unwrap();
    let ids: Vec<String> = storage
        .pending_ingest(Utc::now(), 10)
        .unwrap()
        .iter()
        .map(|p| finish(&storage, p.seq, Fault::None).unwrap().id)
        .collect();
    assert_eq!(ids.len(), 2);
    for id in &ids {
        assert!(uuid::Uuid::parse_str(id).is_ok(), "not a plain uuid: {id}");
    }
    assert_ne!(
        ids[0][..8],
        ids[1][..8],
        "short ids must tell memories apart"
    );
}

#[test]
fn ingress_expiry_removes_raw_payload_and_exposes_floor_without_resetting_identity() {
    let fixture = Fixture::new();
    let storage = fixture.open();
    let rec = record();
    let now = rec.observed_at;
    storage
        .append_ingest(None, &cursor(100), std::slice::from_ref(&rec))
        .unwrap();
    let expired = now + chrono::Duration::days(RAW_TTL_DAYS) + chrono::Duration::seconds(1);
    // Even before maintenance, expired input cannot be consumed.
    assert!(storage.pending_ingest(expired, 10).unwrap().is_empty());
    assert_eq!(storage.expire_ingest(expired).unwrap(), 1);
    let status = storage.ingest_retention(expired).unwrap();
    assert_eq!(
        status.replay_since,
        expired - chrono::Duration::days(RAW_TTL_DAYS)
    );
    assert_eq!(status.pending, 0);
    assert_eq!(receipt(&storage, 1), "expired");
    let raw: Option<String> = storage
        .conn
        .lock()
        .unwrap()
        .query_row("SELECT payload FROM ingest_events WHERE seq = 1", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert!(raw.is_none());
    storage
        .append_ingest(Some(&cursor(100)), &cursor(200), &[record()])
        .unwrap();
    assert!(storage.pending_ingest(Utc::now(), 10).unwrap().is_empty());
    assert_eq!(storage.count().unwrap(), 0);
}

#[test]
fn ingress_forget_is_a_durable_suppression_and_clears_the_payload() {
    let fixture = Fixture::new();
    let storage = fixture.open();
    storage
        .append_ingest(None, &cursor(100), &[record()])
        .unwrap();
    let saved = finish(&storage, 1, Fault::None).unwrap();
    assert!(storage.forget_by_id(&saved.id).unwrap());
    assert_eq!(receipt(&storage, 1), "forgotten");
    drop(storage);
    let storage = fixture.open();
    storage
        .append_ingest(Some(&cursor(100)), &cursor(200), &[record()])
        .unwrap();
    assert!(finish(&storage, 1, Fault::None).is_none());
    assert!(storage.pending_ingest(Utc::now(), 10).unwrap().is_empty());
    assert_eq!(storage.count().unwrap(), 0);
    let payload: Option<String> = storage
        .conn
        .lock()
        .unwrap()
        .query_row("SELECT payload FROM ingest_events WHERE seq = 1", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert!(payload.is_none());
}

#[test]
fn ingress_stale_cursor_cannot_ack_or_insert_records() {
    let fixture = Fixture::new();
    let storage = fixture.open();
    storage
        .append_ingest(None, &cursor(100), &[record()])
        .unwrap();
    let mut other = record();
    other.source_key = "another-message".into();
    assert!(storage.append_ingest(None, &cursor(200), &[other]).is_err());
    assert_eq!(storage.pending_ingest(Utc::now(), 10).unwrap().len(), 1);
    assert_eq!(
        storage
            .ingest_cursor(&cursor(0).stream)
            .unwrap()
            .unwrap()
            .offset,
        100
    );
}

#[test]
fn ingress_migrations_reopen_existing_memories_and_keep_delete_triggers() {
    let fixture = Fixture::new();
    let storage = fixture.open();
    let existing = entry();
    storage.save(&existing).unwrap();
    // Simulate the existing pre-ingress store; migration must preserve it.
    storage
        .conn
        .lock()
        .unwrap()
        .execute_batch(
            "DROP TRIGGER trg_ingest_memory_deleted;
         DROP TABLE consumer_receipts; DROP TABLE ingest_cursors; DROP TABLE ingest_events;",
        )
        .unwrap();
    drop(storage);
    for _ in 0..3 {
        let storage = fixture.open();
        assert_eq!(storage.count().unwrap(), 1);
        let conn = storage.conn.lock().unwrap();
        for trigger in ["trg_followups_source_deleted", "trg_ingest_memory_deleted"] {
            let count: i64 = conn
                .query_row(
                    "SELECT count(*) FROM sqlite_master WHERE type = 'trigger' AND name = ?1",
                    [trigger],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(count, 1);
        }
    }
}

/// The payload must carry the bounded event and NOTHING more. A decision keeps
/// only its excerpt, so the sentence after the decision line is the sentinel:
/// it exists in the source turn and must never reach the store.
#[test]
fn ingress_payload_holds_the_bounded_event_and_not_the_whole_turn() {
    let fixture = Fixture::new();
    let storage = fixture.open();
    let sentinel = "SENTINEL-the-part-of-the-turn-after-the-decision";
    let mut event = Event::new(
        EventSource::ConversationWatcher,
        EventKind::Custom("conversation_decision".into()),
        // What the watcher puts in a decision event: the excerpt only.
        "Decision: use SQLite for demoapp.",
    );
    event.timestamp = Utc::now();
    storage
        .append_ingest(
            None,
            &cursor(100),
            &[IngestRecord {
                source_key: "demoapp/sentinel".into(),
                source_at: None,
                observed_at: Utc::now(),
                payload: IngestPayload { event },
            }],
        )
        .unwrap();
    let stored: String = storage
        .conn
        .lock()
        .unwrap()
        .query_row("SELECT payload FROM ingest_events WHERE seq = 1", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert!(!stored.contains(sentinel), "stored: {stored}");
    assert!(stored.contains("use SQLite"));
    assert!(!stored.contains("raw_turn"));
}

/// Every terminal outcome drops the payload in the same transaction, so a turn
/// that produced NO memory still has its text removed. Before, only the TTL
/// would ever have cleared a skipped turn.
#[test]
fn ingress_a_terminal_receipt_drops_the_payload_for_saved_and_skipped_alike() {
    let fixture = Fixture::new();
    let storage = fixture.open();
    let mut second = record();
    second.source_key = "demoapp/second".into();
    storage
        .append_ingest(None, &cursor(100), &[record(), second])
        .unwrap();
    let payload_of = |seq: i64| -> Option<String> {
        storage
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT payload FROM ingest_events WHERE seq = ?1",
                [seq],
                |r| r.get(0),
            )
            .unwrap()
    };
    assert!(payload_of(1).is_some() && payload_of(2).is_some());

    assert!(finish(&storage, 1, Fault::None).is_some());
    storage
        .finish_ingest(
            2,
            ProcessingDecision::Skip(SkipReason::Duplicate),
            Utc::now(),
        )
        .unwrap();
    assert_eq!(receipt(&storage, 1), "saved");
    assert_eq!(receipt(&storage, 2), "duplicate");
    assert!(payload_of(1).is_none(), "a saved turn keeps no payload");
    assert!(payload_of(2).is_none(), "a skipped turn keeps none either");
    // Identity survives so replay stays idempotent.
    let keys: i64 = storage
        .conn
        .lock()
        .unwrap()
        .query_row(
            "SELECT count(*) FROM ingest_events WHERE source_key IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(keys, 2);
}

/// A store written before the turn text was dropped is migrated in place:
/// the field goes, the event does not.
#[test]
fn ingress_migration_strips_the_turn_text_from_rows_written_earlier() {
    let fixture = Fixture::new();
    let storage = fixture.open();
    let mut event = Event::new(
        EventSource::CodexWatcher,
        EventKind::Custom("conversation_decision".into()),
        "Decision: use SQLite for demoapp",
    );
    event.timestamp = "2025-01-02T03:04:05.123456Z".parse().unwrap();
    let legacy = serde_json::json!({
        "event": event,
        "raw_turn": "SENTINEL-unabridged-turn-from-an-older-build",
    })
    .to_string();
    storage
        .conn
        .lock()
        .unwrap()
        .execute(
            "INSERT INTO ingest_events (source_key, source_at, observed_at, schema_version, payload)
             VALUES ('demoapp/legacy', NULL, ?1, 1, ?2)",
            rusqlite::params![
                Utc::now().to_rfc3339_opts(SecondsFormat::Nanos, true),
                legacy
            ],
        )
        .unwrap();
    drop(storage);

    let storage = fixture.open();
    let migrated: String = storage
        .conn
        .lock()
        .unwrap()
        .query_row("SELECT payload FROM ingest_events WHERE seq = 1", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert!(!migrated.contains("SENTINEL"), "{migrated}");
    assert!(!migrated.contains("raw_turn"));
    // The event itself is intact and still replays.
    let pending = storage.pending_ingest(Utc::now(), 10).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].event.id, event.id);
    assert_eq!(pending[0].event.timestamp, event.timestamp);
    assert_eq!(pending[0].event.content, event.content);
    assert!(finish(&storage, pending[0].seq, Fault::None).is_some());
}

/// A row that reached a terminal receipt under an older build is cleared on
/// open; a row still pending keeps the payload it needs to replay.
#[test]
fn ingress_migration_clears_payloads_that_already_reached_a_receipt() {
    let fixture = Fixture::new();
    let storage = fixture.open();
    let mut second = record();
    second.source_key = "demoapp/pending".into();
    storage
        .append_ingest(None, &cursor(100), &[record(), second])
        .unwrap();
    // Give seq 1 a terminal receipt the OLD way: receipt written, payload kept.
    storage
        .conn
        .lock()
        .unwrap()
        .execute(
            "INSERT INTO consumer_receipts (consumer, event_id, outcome) VALUES ('transcript-memory-v1', 1, 'duplicate')",
            [],
        )
        .unwrap();
    drop(storage);

    let storage = fixture.open();
    let payload_of = |seq: i64| -> Option<String> {
        storage
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT payload FROM ingest_events WHERE seq = ?1",
                [seq],
                |r| r.get(0),
            )
            .unwrap()
    };
    assert!(payload_of(1).is_none(), "a settled row keeps no payload");
    assert!(payload_of(2).is_some(), "a pending row must still replay");
    let pending = storage.pending_ingest(Utc::now(), 10).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].seq, 2);
}

#[test]
fn ingress_export_contains_only_derived_memories() {
    let fixture = Fixture::new();
    let storage = fixture.open();
    storage
        .append_ingest(None, &cursor(100), &[record()])
        .unwrap();
    assert!(storage.export_all().unwrap().is_empty());
    finish(&storage, 1, Fault::None);
    let exported = serde_json::to_string(&storage.export_all().unwrap()).unwrap();
    assert!(exported.contains("Decision: use SQLite"));
    assert!(!exported.contains("raw_turn"), "the field is gone entirely");
}

#[test]
fn ingress_processing_error_rolls_back_memory_receipt_and_extraction_enqueue() {
    let fixture = Fixture::new();
    let storage = fixture.open();
    storage
        .append_ingest(None, &cursor(100), &[record()])
        .unwrap();
    storage
        .conn
        .lock()
        .unwrap()
        .execute_batch(
            "CREATE TRIGGER reject_receipt BEFORE INSERT ON consumer_receipts
         BEGIN SELECT RAISE(ABORT, 'injected receipt write failure'); END;",
        )
        .unwrap();
    assert!(
        storage
            .finish_ingest(
                1,
                ProcessingDecision::Save {
                    entry: &entry(),
                    embedding: None,
                    enqueue_extraction: true,
                    link: None,
                },
                Utc::now()
            )
            .is_err()
    );
    assert_eq!(storage.count().unwrap(), 0);
    assert_eq!(storage.extraction_queue_count().unwrap(), 0);
    assert_eq!(storage.pending_ingest(Utc::now(), 10).unwrap().len(), 1);
    storage
        .conn
        .lock()
        .unwrap()
        .execute_batch("DROP TRIGGER reject_receipt")
        .unwrap();
    assert!(finish(&storage, 1, Fault::None).is_some());
}

#[test]
fn ingress_expiry_between_selection_and_commit_prevents_late_processing() {
    let fixture = Fixture::new();
    let storage = fixture.open();
    let rec = record();
    let expires = rec.observed_at + chrono::Duration::days(RAW_TTL_DAYS + 1);
    storage.append_ingest(None, &cursor(100), &[rec]).unwrap();
    assert_eq!(storage.pending_ingest(Utc::now(), 1).unwrap().len(), 1);
    assert!(
        storage
            .finish_ingest(
                1,
                ProcessingDecision::Save {
                    entry: &entry(),
                    embedding: None,
                    enqueue_extraction: true,
                    link: None,
                },
                expires
            )
            .unwrap()
            .is_none()
    );
    assert_eq!(storage.count().unwrap(), 0);
    assert_eq!(receipt(&storage, 1), "expired");
}

#[test]
fn ingress_concurrent_consumers_commit_only_one_memory() {
    let fixture = Fixture::new();
    let storage = fixture.open();
    storage
        .append_ingest(None, &cursor(100), &[record()])
        .unwrap();
    let other = fixture.open();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let second_barrier = barrier.clone();
    let worker = std::thread::spawn(move || {
        second_barrier.wait();
        finish(&other, 1, Fault::None).is_some()
    });
    barrier.wait();
    let saved = finish(&storage, 1, Fault::None).is_some();
    assert_ne!(saved, worker.join().unwrap());
    assert_eq!(storage.count().unwrap(), 1);
    assert_eq!(storage.extraction_queue_count().unwrap(), 1);
}

#[test]
fn ingress_automatic_backup_does_not_retain_raw_payload() {
    let fixture = Fixture::new();
    let storage = fixture.open();
    storage
        .append_ingest(None, &cursor(100), &[record()])
        .unwrap();
    drop(storage);
    std::fs::remove_file(fixture.0.join("memory.db.bak")).unwrap();
    drop(fixture.open()); // force a fresh automatic snapshot
    let backup_path = fixture.0.join("memory.db.bak");
    let backup = rusqlite::Connection::open(&backup_path).unwrap();
    let raw: Option<String> = backup
        .query_row("SELECT payload FROM ingest_events", [], |r| r.get(0))
        .unwrap();
    assert!(raw.is_none());
    drop(backup);
    let bytes = std::fs::read(&backup_path).unwrap();
    assert!(
        !bytes
            .windows(b"local-only unabridged turn".len())
            .any(|w| w == b"local-only unabridged turn")
    );
}
