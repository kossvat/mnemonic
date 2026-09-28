//! The durable queue's own guard: it refuses what admission should have
//! prepared, prepares what was queued before the policy, and refuses to
//! complete a dirty memory. Credential fixtures are assembled at run time.
use super::*;
use crate::event::{EventKind, EventSource, MemoryType};
use crate::redaction::SUMMARY_KEY;

fn body(n: usize) -> String {
    "a1B2c3D4e5F6".chars().cycle().take(n).collect()
}

fn token() -> String {
    ["sk-", "proj-", &body(40)].concat()
}

fn open() -> crate::test_support::InTempDir<Storage> {
    crate::test_support::InTempDir::new("mnemonic-redaction-ingest-", |dir| {
        Storage::open(&dir.join("memory.db")).unwrap()
    })
}

fn cursor(stream: &str, offset: u64) -> IngestCursor {
    IngestCursor {
        stream: stream.into(),
        generation: 0,
        offset,
        file_id: "file-1".into(),
        prefix_len: 0,
        prefix_hash: String::new(),
        anchor_hash: String::new(),
    }
}

fn record(key: &str, content: &str, metadata: serde_json::Value) -> IngestRecord {
    let mut event = Event::new(
        EventSource::ConversationWatcher,
        EventKind::UserCorrection,
        content,
    );
    event.metadata = metadata;
    IngestRecord {
        source_key: key.into(),
        source_at: Some("2025-01-02T04:04:05+01:00".into()),
        observed_at: Utc::now(),
        payload: IngestPayload { event },
    }
}

fn queue_text(storage: &Storage) -> String {
    let conn = storage.conn.lock().unwrap();
    let mut out = String::new();
    for sql in [
        "SELECT source_key || ' ' || coalesce(payload, '') FROM ingest_events",
        "SELECT stream FROM ingest_cursors",
        "SELECT title || ' ' || content || ' ' || metadata FROM memories",
    ] {
        let mut stmt = conn.prepare(sql).unwrap();
        for row in stmt.query_map([], |r| r.get::<_, String>(0)).unwrap() {
            out.push_str(&row.unwrap());
            out.push('\n');
        }
    }
    out
}

fn count(storage: &Storage, sql: &str) -> i64 {
    let conn = storage.conn.lock().unwrap();
    conn.query_row(sql, [], |r| r.get(0)).unwrap()
}

#[test]
fn redaction_ingress_the_queue_refuses_an_unprepared_record() {
    let token = token();
    let storage = open();
    let dirty = [
        record(
            "k1",
            &format!("не так, use {token}"),
            serde_json::Value::Null,
        ),
        record(
            &format!("k-{token}"),
            "не так, use SQLite",
            serde_json::Value::Null,
        ),
        record(
            "k3",
            "не так, use SQLite",
            serde_json::json!({"jsonl_path": format!("/t/{token}.jsonl")}),
        ),
        record(
            "k4",
            "не так, use SQLite",
            serde_json::json!({"note": format!("see {token}")}),
        ),
    ];
    for r in &dirty {
        let refused = storage.append_ingest(None, &cursor("s", 10), std::slice::from_ref(r));
        assert!(refused.is_err());
        assert!(storage.append_history(std::slice::from_ref(r)).is_err());
    }
    // A cursor whose stream is sensitive is refused with no records at all.
    assert!(
        storage
            .append_ingest(None, &cursor(&format!("s-{token}"), 10), &[])
            .is_err()
    );
    assert!(storage.ingest_cursor("s").unwrap().is_none());
    assert!(count(&storage, "SELECT count(*) FROM ingest_events") == 0);
    assert!(!queue_text(&storage).contains(&token));

    // A record with a caller-written summary is refused as well.
    let mut forged = record("k5", "не так, use SQLite", serde_json::Value::Null);
    forged.payload.event.metadata = serde_json::json!({ SUMMARY_KEY: {"changed": true} });
    assert!(
        storage
            .append_ingest(None, &cursor("s", 10), &[forged])
            .is_err()
    );

    // The prepared shape of the same text goes through.
    let clean = crate::redaction::prepare_event(
        dirty[0].payload.event.clone(),
        crate::redaction::STRUCTURAL_KEYS,
    )
    .unwrap()
    .into_event();
    let mut r = record("k1", "", serde_json::Value::Null);
    r.payload.event = clean;
    storage.append_ingest(None, &cursor("s", 10), &[r]).unwrap();
    assert!(count(&storage, "SELECT count(*) FROM ingest_events") == 1);
    assert!(!queue_text(&storage).contains(&token));
}

/// A row queued before the policy existed, written the way the old code
/// wrote it: replay prepares it, keeps that summary on the next replay,
/// and never hands out the raw text.
#[test]
fn redaction_ingress_replay_prepares_a_pre_policy_payload_and_keeps_the_summary() {
    let token = token();
    let storage = open();
    let raw = record(
        "old-1",
        &format!("не так, the key is {token} now"),
        serde_json::json!({"jsonl_path": "/t/a.jsonl", "role": "user"}),
    );
    {
        let conn = storage.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO ingest_events (source_key, source_at, observed_at, schema_version, payload)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                raw.source_key,
                raw.source_at,
                timestamp(raw.observed_at),
                SCHEMA_VERSION,
                serde_json::to_string(&raw.payload).unwrap()
            ],
        )
        .unwrap();
    }
    let first = storage.pending_ingest(Utc::now(), 10).unwrap();
    assert!(first.len() == 1);
    let event = &first[0].event;
    assert!(!event.content.contains(&token));
    assert!(event.content.contains("[REDACTED:credential]"));
    assert!(event.metadata[SUMMARY_KEY]["counts"]["provider_token"] == 1);
    assert!(event.metadata["role"] == "user");

    // The row itself is untouched (replay is a read); the next replay
    // prepares it again and says the same.
    let again = storage.pending_ingest(Utc::now(), 10).unwrap();
    assert!(again[0].event.content == event.content);
    assert!(again[0].event.metadata == event.metadata);

    // A prepared event whose summary was stored at capture keeps it on
    // replay, with no marker counted a second time.
    let prepared = crate::redaction::prepare_event(
        record(
            "new-1",
            &format!("не так, use {token}"),
            serde_json::Value::Null,
        )
        .payload
        .event,
        crate::redaction::STRUCTURAL_KEYS,
    )
    .unwrap()
    .into_event();
    let summary = prepared.metadata[SUMMARY_KEY].clone();
    let mut r = record("new-1", "", serde_json::Value::Null);
    r.payload.event = prepared;
    storage.append_ingest(None, &cursor("s", 10), &[r]).unwrap();
    let replayed = storage.pending_ingest(Utc::now(), 10).unwrap();
    let new = replayed
        .iter()
        .find(|p| p.event.content.contains("use"))
        .unwrap();
    assert!(new.event.metadata[SUMMARY_KEY] == summary);
}

/// A queued payload whose identity cannot be admitted (a path holding a
/// credential) is never processed: it gets a terminal `rejected` receipt
/// and its text is dropped.
#[test]
fn redaction_ingress_replay_rejects_a_payload_with_a_sensitive_identity() {
    let token = token();
    let storage = open();
    let raw = record(
        "old-2",
        "не так, use SQLite",
        serde_json::json!({"jsonl_path": format!("/t/{token}.jsonl"), "role": "user"}),
    );
    {
        let conn = storage.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO ingest_events (source_key, source_at, observed_at, schema_version, payload)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                raw.source_key,
                raw.source_at,
                timestamp(raw.observed_at),
                SCHEMA_VERSION,
                serde_json::to_string(&raw.payload).unwrap()
            ],
        )
        .unwrap();
    }
    assert!(storage.pending_ingest(Utc::now(), 10).unwrap().is_empty());
    assert!(
        count(
            &storage,
            "SELECT count(*) FROM consumer_receipts WHERE outcome = 'rejected'"
        ) == 1
    );
    assert!(
        count(
            &storage,
            "SELECT count(*) FROM ingest_events WHERE payload IS NULL"
        ) == 1
    );
    assert!(!queue_text(&storage).contains(&token));
    assert!(count(&storage, "SELECT count(*) FROM memories") == 0);
    // Idempotent: nothing left to reject or return.
    assert!(storage.pending_ingest(Utc::now(), 10).unwrap().is_empty());
}

/// Rejected rows do not shorten a page: a caller that reads a full page as
/// "more to come" keeps draining at full speed.
#[test]
fn redaction_ingress_replay_refills_a_page_after_rejections() {
    let token = token();
    let storage = open();
    {
        let conn = storage.conn.lock().unwrap();
        for i in 0..135 {
            let metadata = if i < 5 {
                serde_json::json!({"jsonl_path": format!("/t/{token}.jsonl")})
            } else {
                serde_json::json!({"jsonl_path": "/t/a.jsonl"})
            };
            let r = record(&format!("row-{i}"), "не так, use SQLite", metadata);
            conn.execute(
                "INSERT INTO ingest_events (source_key, source_at, observed_at, schema_version, payload)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    r.source_key,
                    r.source_at,
                    timestamp(r.observed_at),
                    SCHEMA_VERSION,
                    serde_json::to_string(&r.payload).unwrap()
                ],
            )
            .unwrap();
        }
    }
    let first = storage.pending_ingest(Utc::now(), 128).unwrap();
    assert!(first.len() == 128, "a full page despite five rejections");
    assert!(
        count(
            &storage,
            "SELECT count(*) FROM consumer_receipts WHERE outcome = 'rejected'"
        ) == 5
    );
    // The refill continues where the page stopped: no row twice, in order.
    let seqs: Vec<i64> = first.iter().map(|p| p.seq).collect();
    assert!(seqs == (6..=133).collect::<Vec<i64>>());
    // A read: the clean rows stay pending, and only the five are gone.
    assert!(storage.pending_ingest(Utc::now(), 128).unwrap().len() == 128);
    assert!(storage.pending_ingest(Utc::now(), 200).unwrap().len() == 130);

    // The smallest case: rows 1-4, row 2 rejected, a page of three.
    let storage = open();
    {
        let conn = storage.conn.lock().unwrap();
        for i in 1..=4 {
            let metadata = if i == 2 {
                serde_json::json!({"jsonl_path": format!("/t/{token}.jsonl")})
            } else {
                serde_json::json!({"jsonl_path": "/t/a.jsonl"})
            };
            let r = record(&format!("row-{i}"), "не так, use SQLite", metadata);
            conn.execute(
                "INSERT INTO ingest_events (source_key, source_at, observed_at, schema_version, payload)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    r.source_key,
                    r.source_at,
                    timestamp(r.observed_at),
                    SCHEMA_VERSION,
                    serde_json::to_string(&r.payload).unwrap()
                ],
            )
            .unwrap();
        }
    }
    let seqs: Vec<i64> = storage
        .pending_ingest(Utc::now(), 3)
        .unwrap()
        .iter()
        .map(|p| p.seq)
        .collect();
    assert!(seqs == vec![1, 3, 4]);
}

/// The queued event's id becomes the memory id, and it is not part of the
/// entry the caller hands in: a pre-policy row with a credential-shaped id
/// must be refused at completion even when the entry is clean.
#[test]
fn redaction_ingress_completion_refuses_a_queued_id_it_cannot_store() {
    let token = token();
    let storage = open();
    let mut raw = record("old-3", "не так, use SQLite", serde_json::Value::Null);
    raw.payload.event.id = format!("id-{token}");
    {
        let conn = storage.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO ingest_events (source_key, source_at, observed_at, schema_version, payload)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                raw.source_key,
                raw.source_at,
                timestamp(raw.observed_at),
                SCHEMA_VERSION,
                serde_json::to_string(&raw.payload).unwrap()
            ],
        )
        .unwrap();
    }
    let seq: i64 = count(&storage, "SELECT max(seq) FROM ingest_events");
    let entry = MemoryEntry::new(
        "Correction",
        "не так, use SQLite",
        MemoryType::Feedback,
        EventSource::ConversationWatcher,
    );
    let refused = storage.finish_ingest(
        seq,
        ProcessingDecision::Save {
            entry: &entry,
            embedding: None,
            enqueue_extraction: true,
            link: None,
        },
        Utc::now(),
    );
    assert!(refused.is_err());
    assert!(count(&storage, "SELECT count(*) FROM memories") == 0);
    assert!(count(&storage, "SELECT count(*) FROM consumer_receipts") == 0);
    assert!(count(&storage, "SELECT count(*) FROM extraction_queue") == 0);
    // The queued row itself still holds the id this test planted; nothing
    // else does.
    let memories: i64 = count(
        &storage,
        "SELECT count(*) FROM memories WHERE id LIKE 'id-%'",
    );
    assert!(memories == 0);
}

#[test]
fn redaction_ingress_completion_refuses_a_dirty_entry_and_keeps_the_event_pending() {
    let token = token();
    let storage = open();
    storage
        .append_ingest(
            None,
            &cursor("s", 10),
            &[record("k1", "не так, use SQLite", serde_json::Value::Null)],
        )
        .unwrap();
    let seq = storage.pending_ingest(Utc::now(), 10).unwrap()[0].seq;
    let mut entry = MemoryEntry::new(
        "Correction",
        format!("не так, use SQLite {token}"),
        MemoryType::Feedback,
        EventSource::ConversationWatcher,
    );
    let refused = storage.finish_ingest(
        seq,
        ProcessingDecision::Save {
            entry: &entry,
            embedding: None,
            enqueue_extraction: false,
            link: None,
        },
        Utc::now(),
    );
    assert!(refused.is_err());
    assert!(count(&storage, "SELECT count(*) FROM memories") == 0);
    assert!(count(&storage, "SELECT count(*) FROM consumer_receipts") == 0);
    assert!(storage.pending_ingest(Utc::now(), 10).unwrap().len() == 1);
    assert!(!queue_text(&storage).contains(&token));

    // The clean entry completes as before.
    entry.content = "не так, use SQLite".into();
    let saved = storage
        .finish_ingest(
            seq,
            ProcessingDecision::Save {
                entry: &entry,
                embedding: None,
                enqueue_extraction: false,
                link: None,
            },
            Utc::now(),
        )
        .unwrap();
    assert!(saved.is_some());
    assert!(count(&storage, "SELECT count(*) FROM memories") == 1);
}
