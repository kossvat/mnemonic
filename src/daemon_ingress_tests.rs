use super::*;
use crate::embedding::HashEmbedder;
use crate::event::{EventKind, EventSource, MemoryEntry};
use crate::ingest::{IngestCursor, IngestPayload, IngestRecord};
use std::sync::Mutex;

struct RecordingSink(Arc<Mutex<Vec<MemoryEntry>>>);
impl OutputSink for RecordingSink {
    fn write(&self, entry: &MemoryEntry) -> Result<()> {
        self.0.lock().unwrap().push(entry.clone());
        Ok(())
    }
    fn name(&self) -> &str {
        "recording"
    }
}

/// A durable-ingress save enqueues its extraction INSIDE the save
/// transaction. Enqueueing again after attribution can mint a SECOND job once
/// the worker has already finished the first one, so the post-save enqueue
/// must skip ingress entries.
///
/// The race is made deterministic through the only ordering lever
/// `process_batch` exposes: session attribution blocks on the SessionTracker
/// mutex, which the stand-in worker holds until it has taken the job.
#[test]
fn ingress_save_never_enqueues_a_second_extraction_behind_the_worker() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let tmp = crate::test_support::temp_dir("mnemonic-ingress-once-");
    let dir = tmp.path();
    let storage = Arc::new(Storage::open(&dir.join("memory.db")).unwrap());
    let daemon = Daemon::new(Config::default());
    let classifier = RuleClassifier::new(daemon.config.classifier.clone());
    let sinks: Vec<Box<dyn OutputSink>> = Vec::new();
    let attributor = PeerAttributor::init(&storage, &daemon.config.peers).unwrap();
    let peer = storage.upsert_peer("claude", None, "agent").unwrap();
    let tracker = Arc::new(std::sync::Mutex::new(SessionTracker::new(
        peer,
        std::time::Duration::from_secs(60),
    )));

    // `jsonl_path` is what sends attribution through the tracker at all.
    let mut event = Event::new(
        EventSource::ConversationWatcher,
        EventKind::Custom("conversation_decision".into()),
        "Decision: use SQLite for demoapp",
    )
    .with_metadata(serde_json::json!({
        "jsonl_path": dir.join("session.jsonl").to_string_lossy(),
        "role": "user",
    }));
    event.timestamp = "2025-02-03T04:05:06Z".parse().unwrap();
    storage
        .append_ingest(
            None,
            &IngestCursor {
                stream: "demoapp".into(),
                generation: 0,
                offset: 10,
                file_id: "file".into(),
                prefix_len: 0,
                prefix_hash: String::new(),
                anchor_hash: String::new(),
            },
            &[IngestRecord {
                source_key: "demoapp/once".into(),
                source_at: None,
                observed_at: chrono::Utc::now(),
                payload: IngestPayload { event },
            }],
        )
        .unwrap();
    let events: Vec<_> = storage
        .pending_ingest(chrono::Utc::now(), 8)
        .unwrap()
        .into_iter()
        .map(|pending| (Some(pending.seq), pending.event))
        .collect();
    assert_eq!(events.len(), 1);

    let holding = Arc::new(AtomicBool::new(false));
    let worker = std::thread::spawn({
        let (storage, tracker, holding) = (storage.clone(), tracker.clone(), holding.clone());
        move || {
            let guard = tracker.lock().unwrap();
            holding.store(true, Ordering::SeqCst);
            // Bounded, so a build that never enqueues fails instead of hanging.
            let mut took = false;
            for _ in 0..1_000_000 {
                if let Some(id) = storage.next_extraction_batch(1).unwrap().first() {
                    storage.dequeue_extraction(id).unwrap();
                    took = true;
                    break;
                }
                std::thread::yield_now();
            }
            drop(guard);
            took
        }
    });
    while !holding.load(Ordering::SeqCst) {
        std::thread::yield_now();
    }
    daemon.process_batch(
        &events,
        &classifier,
        &storage,
        &sinks,
        &HashEmbedder,
        0.99,
        &ImportanceScorer::default(),
        0.0,
        &RuleExtractor::new(),
        true,
        Some(&attributor),
        Some(&*tracker),
    );
    assert!(
        worker.join().unwrap(),
        "the transactional enqueue never happened"
    );
    assert_eq!(storage.count().unwrap(), 1);
    assert_eq!(
        storage.extraction_queue_count().unwrap(),
        0,
        "a second extraction job appeared behind the worker"
    );
}

#[test]
fn ingress_daemon_replay_records_every_outcome_and_exports_only_derived_content() {
    let tmp = crate::test_support::temp_dir("mnemonic-ingress-daemon-");
    let storage = Storage::open(&tmp.path().join("memory.db")).unwrap();
    let daemon = Daemon::new(Config::default());
    let classifier = RuleClassifier::new(daemon.config.classifier.clone());
    let outputs = Arc::new(Mutex::new(Vec::new()));
    let sinks: Vec<Box<dyn OutputSink>> = vec![Box::new(RecordingSink(outputs.clone()))];
    let source_time: chrono::DateTime<chrono::Utc> = "2025-02-03T04:05:06Z".parse().unwrap();
    let mut cursor = IngestCursor {
        stream: "demoapp".into(),
        generation: 0,
        offset: 100,
        file_id: "file".into(),
        prefix_len: 0,
        prefix_hash: String::new(),
        anchor_hash: String::new(),
    };
    let records: Vec<_> = [
        (
            EventKind::Custom("conversation_decision".into()),
            "Decision: use SQLite for demoapp",
        ),
        (
            EventKind::Custom("conversation_decision".into()),
            "Decision: use SQLite for demoapp",
        ),
        (
            EventKind::Custom("uncaptured".into()),
            "No classified memory",
        ),
        (
            EventKind::UserCorrection,
            "No, switch the configuration for example-org",
        ),
    ]
    .into_iter()
    .enumerate()
    .map(|(n, (kind, content))| {
        let mut event = Event::new(EventSource::CodexWatcher, kind, content);
        event.timestamp = source_time;
        IngestRecord {
            source_key: format!("demoapp/{n}"),
            source_at: Some(source_time.to_rfc3339()),
            observed_at: chrono::Utc::now(),
            payload: IngestPayload { event },
        }
    })
    .collect();
    storage.append_ingest(None, &cursor, &records).unwrap();
    let process = |threshold| {
        let events: Vec<_> = storage
            .pending_ingest(chrono::Utc::now(), 128)
            .unwrap()
            .into_iter()
            .map(|p| (Some(p.seq), p.event))
            .collect();
        daemon.process_batch(
            &events,
            &classifier,
            &storage,
            &sinks,
            &HashEmbedder,
            0.99,
            &ImportanceScorer::default(),
            threshold,
            &RuleExtractor::new(),
            true,
            None,
            None,
        );
    };
    process(0.0);
    process(0.0);
    assert_eq!(storage.count().unwrap(), 2);
    let mut low = Event::new(
        EventSource::ConversationWatcher,
        EventKind::Custom("conversation_decision".into()),
        "Switching to isolated queues for unrelated workloads",
    );
    low.timestamp = source_time;
    let mut next = cursor.clone();
    next.offset = 200;
    storage
        .append_ingest(
            Some(&cursor),
            &next,
            &[IngestRecord {
                source_key: "demoapp/low".into(),
                source_at: None,
                observed_at: chrono::Utc::now(),
                payload: IngestPayload { event: low },
            }],
        )
        .unwrap();
    cursor = next;
    let mut urgent = Event::new(
        EventSource::CodexWatcher,
        EventKind::UserCorrection,
        "No, use the other configuration for example-org",
    );
    urgent.timestamp = source_time;
    let mut next = cursor.clone();
    next.offset = 300;
    storage
        .append_ingest(
            Some(&cursor),
            &next,
            &[IngestRecord {
                source_key: "demoapp/urgent".into(),
                source_at: None,
                observed_at: chrono::Utc::now(),
                payload: IngestPayload { event: urgent },
            }],
        )
        .unwrap();
    process(2.0); // urgent corrections still bypass scoring and dedup
    process(2.0);
    assert!(
        storage
            .pending_ingest(chrono::Utc::now(), 128)
            .unwrap()
            .is_empty()
    );
    assert_eq!(storage.count().unwrap(), 3);
    assert_eq!(storage.extraction_queue_count().unwrap(), 3);
    let conn = storage.conn.lock().unwrap();
    let outcomes: Vec<String> = conn
        .prepare("SELECT outcome FROM consumer_receipts ORDER BY event_id")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(
        outcomes,
        [
            "saved",
            "duplicate",
            "filtered",
            "saved",
            "low_importance",
            "saved"
        ]
    );
    drop(conn);
    let outputs = outputs.lock().unwrap();
    assert_eq!(outputs.len(), 3);
    for output in outputs.iter() {
        assert_eq!(output.timestamp, source_time);
        assert!(
            !serde_json::to_string(output)
                .unwrap()
                .contains("local-only")
        );
    }
    // A delete after processing cannot make the daemon replay the raw turn.
    storage.forget_by_id(&outputs[0].id).unwrap();
    drop(outputs);
    process(0.0);
    assert_eq!(storage.count().unwrap(), 2);
}

/// A turn read back from history is filed like a live one, but must not be
/// linked to a session opened now: a months-old conversation would show up
/// as a session happening today, and the dream worker would summarise it.
#[test]
fn a_turn_from_history_opens_no_session_a_live_one_does() {
    let dir =
        std::env::temp_dir().join(format!("mnemonic-history-session-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let storage = Storage::open(&dir.join("memory.db")).unwrap();
    let daemon = Daemon::new(Config::default());
    let classifier = RuleClassifier::new(daemon.config.classifier.clone());
    let sinks: Vec<Box<dyn OutputSink>> = Vec::new();
    let attributor = PeerAttributor::init(&storage, &daemon.config.peers).unwrap();
    let peer = storage.upsert_peer("claude", None, "agent").unwrap();
    let tracker = std::sync::Mutex::new(SessionTracker::new(
        peer,
        std::time::Duration::from_secs(60),
    ));

    let turn = |key: &str, text: &str, history: bool| {
        let mut metadata = serde_json::json!({
            "jsonl_path": dir.join(format!("{key}.jsonl")).to_string_lossy(),
            "role": "user",
        });
        if history {
            metadata[crate::watcher::history::HISTORY_FLAG] = true.into();
        }
        let mut event = Event::new(
            EventSource::ConversationWatcher,
            EventKind::UserCorrection,
            text,
        )
        .with_metadata(metadata);
        event.timestamp = "2026-07-01T10:00:00Z".parse().unwrap();
        IngestRecord {
            source_key: key.into(),
            source_at: None,
            observed_at: chrono::Utc::now(),
            payload: IngestPayload { event },
        }
    };
    storage
        .append_history(&[
            turn("live", "это не то, живая правка про demoapp", false),
            turn("old", "это не то, правка из июля про другое", true),
        ])
        .unwrap();
    let events: Vec<_> = storage
        .pending_ingest(chrono::Utc::now(), 8)
        .unwrap()
        .into_iter()
        .map(|pending| (Some(pending.seq), pending.event))
        .collect();
    daemon.process_batch(
        &events,
        &classifier,
        &storage,
        &sinks,
        &HashEmbedder,
        0.92,
        &ImportanceScorer::default(),
        0.0,
        &RuleExtractor::new(),
        false,
        Some(&attributor),
        Some(&tracker),
    );

    let rows: Vec<(String, Option<String>)> = {
        let conn = rusqlite::Connection::open(dir.join("memory.db")).unwrap();
        let mut stmt = conn
            .prepare("SELECT content, session_id FROM memories ORDER BY content")
            .unwrap();
        stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    };
    assert_eq!(rows.len(), 2, "both are filed: {rows:?}");
    for (content, session) in &rows {
        if content.contains("живая") {
            assert!(session.is_some(), "a live turn joins its session");
        } else {
            assert!(session.is_none(), "a history turn opens no session");
        }
    }
    drop(storage);
    let _ = std::fs::remove_dir_all(dir);
}

/// Queue one decision turn through durable ingress and process it the way
/// the daemon does, with every vector identical (every memory is a near
/// duplicate) and the given importance floor. Returns the receipt outcome.
fn process_decision(
    daemon: &Daemon,
    storage: &Storage,
    key: &str,
    text: &str,
    importance_floor: f32,
) -> String {
    process_decision_at(
        daemon,
        storage,
        key,
        text,
        importance_floor,
        chrono::Utc::now(),
    )
}

fn process_decision_at(
    daemon: &Daemon,
    storage: &Storage,
    key: &str,
    text: &str,
    importance_floor: f32,
    at: chrono::DateTime<chrono::Utc>,
) -> String {
    let mut event = Event::new(
        EventSource::ConversationWatcher,
        EventKind::Custom("conversation_decision".into()),
        text,
    )
    .with_metadata(serde_json::json!({"role": "user"}));
    event.timestamp = at;
    let cursor = storage.ingest_cursor("demoapp").unwrap();
    let offset = cursor.as_ref().map_or(0, |c| c.offset) + 10;
    storage
        .append_ingest(
            cursor.as_ref(),
            &IngestCursor {
                stream: "demoapp".into(),
                generation: 0,
                offset,
                file_id: "file".into(),
                prefix_len: 0,
                prefix_hash: String::new(),
                anchor_hash: String::new(),
            },
            &[IngestRecord {
                source_key: format!("demoapp/{key}"),
                source_at: None,
                observed_at: chrono::Utc::now(),
                payload: IngestPayload { event },
            }],
        )
        .unwrap();
    let events: Vec<_> = storage
        .pending_ingest(chrono::Utc::now(), 8)
        .unwrap()
        .into_iter()
        .map(|pending| (Some(pending.seq), pending.event))
        .collect();
    assert_eq!(events.len(), 1);
    let seq = events[0].0.unwrap();
    daemon.process_batch(
        &events,
        &RuleClassifier::new(daemon.config.classifier.clone()),
        storage,
        &[],
        &crate::test_support::ConstEmbedder,
        0.92,
        &ImportanceScorer::default(),
        importance_floor,
        &RuleExtractor::new(),
        false,
        None,
        None,
    );
    storage
        .conn
        .lock()
        .unwrap()
        .query_row(
            "SELECT outcome FROM consumer_receipts WHERE event_id = ?1",
            [seq],
            |r| r.get(0),
        )
        .unwrap()
}

#[test]
fn decision_turn_with_changed_price_is_saved_and_linked() {
    let tmp = crate::test_support::temp_dir("mnemonic-ingress-update-");
    let storage = Storage::open(&tmp.path().join("memory.db")).unwrap();
    let daemon = Daemon::new(Config::default());
    let first = "Decision: Widget price is $5";
    assert_eq!(
        process_decision(&daemon, &storage, "a", first, 0.0),
        "saved"
    );
    let changed = "Decision: Widget price is now $6";
    assert_eq!(
        process_decision(&daemon, &storage, "b", changed, 0.0),
        "saved"
    );
    let conn = storage.conn.lock().unwrap();
    let links: i64 = conn
        .query_row("SELECT COUNT(*) FROM memory_updates", [], |r| r.get(0))
        .unwrap();
    assert_eq!(links, 1);
    // The same price again is still a duplicate.
    drop(conn);
    let again = "Decision: Widget price is now $6";
    assert_eq!(
        process_decision(&daemon, &storage, "c", again, 0.0),
        "duplicate"
    );
}

#[test]
fn value_bearing_turn_survives_importance_floor() {
    let tmp = crate::test_support::temp_dir("mnemonic-ingress-floor-");
    let storage = Storage::open(&tmp.path().join("memory.db")).unwrap();
    let daemon = Daemon::new(Config::default());
    // A floor nothing can reach: a plain decision falls under it ...
    let plain = "Decision: go with the smaller parser";
    assert_eq!(
        process_decision(&daemon, &storage, "a", plain, 2.0),
        "low_importance"
    );
    // ... a stated price does not.
    let priced = "Decision: Gadget price is $9";
    assert_eq!(
        process_decision(&daemon, &storage, "b", priced, 2.0),
        "saved"
    );
}

#[test]
fn a_replayed_old_turn_is_planned_at_its_own_time() {
    let tmp = crate::test_support::temp_dir("mnemonic-ingress-replay-");
    let storage = Storage::open(&tmp.path().join("memory.db")).unwrap();
    let daemon = Daemon::new(Config::default());
    let hours_ago = |h| chrono::Utc::now() - chrono::Duration::hours(h);
    let five = "Decision: Widget price is $5";
    let six = "Decision: Widget price is now $6";
    assert_eq!(
        process_decision_at(&daemon, &storage, "a", five, 0.0, hours_ago(3)),
        "saved"
    );
    assert_eq!(
        process_decision_at(&daemon, &storage, "b", six, 0.0, hours_ago(1)),
        "saved"
    );
    // The $5 statement read again from its transcript, dated before $6.
    assert_eq!(
        process_decision_at(&daemon, &storage, "c", five, 0.0, hours_ago(2)),
        "duplicate"
    );
}

#[test]
fn a_duplicate_turn_records_when_the_value_was_said_again() {
    let tmp = crate::test_support::temp_dir("mnemonic-ingress-reaffirm-");
    let storage = Storage::open(&tmp.path().join("memory.db")).unwrap();
    let daemon = Daemon::new(Config::default());
    let five = "Decision: Widget price is $5";
    let hours_ago = |h| chrono::Utc::now() - chrono::Duration::hours(h);
    assert_eq!(
        process_decision_at(&daemon, &storage, "a", five, 0.0, hours_ago(3)),
        "saved"
    );
    assert_eq!(
        process_decision_at(&daemon, &storage, "b", five, 0.0, hours_ago(1)),
        "duplicate"
    );
    let reaffirmed: i64 = storage
        .conn
        .lock()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM memory_reaffirmed", [], |r| r.get(0))
        .unwrap();
    assert_eq!(reaffirmed, 1);
}
