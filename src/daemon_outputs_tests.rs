//! What the daemon makes of a captured turn, copy by copy: the row, the
//! index, what the model is given and what each sink writes are all of
//! the prepared entry, and a turn the store does not take reaches no
//! sink. Credential fixtures are assembled at run time; assertions never
//! print them.
use std::sync::Mutex;

use super::*;
use crate::event::{EventKind, EventSource, MemoryEntry};
use crate::ingest::{IngestCursor, IngestPayload, IngestRecord};
use crate::redaction::{CREDENTIAL_MARKER, STRUCTURAL_KEYS, SUMMARY_KEY};
use crate::test_support::RecordingEmbedder;

fn token() -> String {
    ["sk-", "proj-", &"a1B2c3D4e5F6".repeat(4)].concat()
}

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

struct Fixture {
    dir: tempfile::TempDir,
    storage: Storage,
    daemon: Daemon,
    embedder: RecordingEmbedder,
    written: Arc<Mutex<Vec<MemoryEntry>>>,
    sinks: Vec<Box<dyn OutputSink>>,
}

impl Fixture {
    fn new() -> Self {
        let dir = crate::test_support::temp_dir("mnemonic-daemon-outputs-");
        let storage = Storage::open(&dir.path().join("memory.db")).unwrap();
        let written = Arc::new(Mutex::new(Vec::new()));
        let sinks: Vec<Box<dyn OutputSink>> = vec![
            Box::new(RecordingSink(written.clone())),
            Box::new(crate::output::memory_files::MemoryFileSink::new(
                dir.path().join("memory-files"),
            )),
            Box::new(crate::output::obsidian::ObsidianSink::new(
                dir.path().join("obsidian"),
            )),
        ];
        Self {
            dir,
            storage,
            daemon: Daemon::new(Config::default()),
            embedder: RecordingEmbedder::new(),
            written,
            sinks,
        }
    }

    /// Capture one turn as a watcher does (prepared, then queued) and let
    /// the daemon process what is pending.
    fn capture(&self, text: &str) {
        let event = Event::new(
            EventSource::ConversationWatcher,
            EventKind::Custom("conversation_decision".into()),
            text,
        )
        .with_metadata(serde_json::json!({"role": "user"}));
        let event = crate::redaction::prepare_event(event, STRUCTURAL_KEYS)
            .unwrap()
            .into_event();
        self.storage
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
                    source_key: "demoapp/turn".into(),
                    source_at: None,
                    observed_at: chrono::Utc::now(),
                    payload: IngestPayload { event },
                }],
            )
            .unwrap();
        let events: Vec<_> = self
            .storage
            .pending_ingest(chrono::Utc::now(), 8)
            .unwrap()
            .into_iter()
            .map(|pending| (Some(pending.seq), pending.event))
            .collect();
        assert!(events.len() == 1);
        self.daemon.process_batch(
            &events,
            &RuleClassifier::new(self.daemon.config.classifier.clone()),
            &self.storage,
            &self.sinks,
            &self.embedder,
            0.92,
            &ImportanceScorer::default(),
            0.0,
            &RuleExtractor::new(),
            true,
            None,
            None,
        );
    }

    /// A correction, which the daemon stores at once, past the queue and
    /// the batch: it is given the event as the watcher made it.
    fn urgent(&self, text: &str) {
        let event = Event::new(
            EventSource::ConversationWatcher,
            EventKind::UserCorrection,
            text,
        );
        self.daemon.process_urgent(
            &event,
            &RuleClassifier::new(self.daemon.config.classifier.clone()),
            &self.storage,
            &self.sinks,
            &self.embedder,
            &RuleExtractor::new(),
            true,
            None,
            None,
        );
    }

    /// Every file the sinks wrote, as (path, bytes).
    fn files(&self) -> Vec<(String, Vec<u8>)> {
        let mut out = Vec::new();
        let mut pending = vec![
            self.dir.path().join("memory-files"),
            self.dir.path().join("obsidian"),
        ];
        while let Some(dir) = pending.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    pending.push(path);
                } else {
                    let bytes = std::fs::read(&path).unwrap();
                    out.push((path.to_string_lossy().into_owned(), bytes));
                }
            }
        }
        out
    }

    fn stored(&self) -> String {
        let conn = self.storage.conn.lock().unwrap();
        let mut out = String::new();
        for sql in [
            "SELECT id || title || content || tags || metadata FROM memories",
            "SELECT title || content || tags FROM memories_fts",
            "SELECT coalesce(payload, '') FROM ingest_events",
        ] {
            let mut stmt = conn.prepare(sql).unwrap();
            for row in stmt.query_map([], |r| r.get::<_, String>(0)).unwrap() {
                out.push_str(&row.unwrap());
                out.push('\n');
            }
        }
        out
    }
}

fn holds(bytes: &[u8], text: &str) -> bool {
    bytes
        .windows(text.len())
        .any(|window| window == text.as_bytes())
}

#[test]
fn redaction_outputs_daemon_every_copy_of_a_turn_is_of_the_prepared_entry() {
    let token = token();
    let f = Fixture::new();
    f.capture(&format!(
        "Decision: use {token} for demoapp <private>and a plan</private>"
    ));
    assert!(f.storage.count().unwrap() == 1);

    let stored = f.stored();
    assert!(!stored.contains(&token) && !stored.contains("and a plan"));
    assert!(stored.contains(CREDENTIAL_MARKER));
    let given = f.embedder.texts();
    assert!(!given.is_empty() && given.iter().all(|text| !text.contains(&token)));
    let written = f.written.lock().unwrap();
    assert!(written.len() == 1);
    assert!(!serde_json::to_string(&written[0]).unwrap().contains(&token));
    let files = f.files();
    assert!(files.len() >= 2, "a sink wrote no file");
    for (path, bytes) in &files {
        assert!(!path.contains(&token), "a file is named after the token");
        assert!(!holds(bytes, &token) && !holds(bytes, "and a plan"));
    }

    // Prepared at capture and again by the daemon, counted once: the
    // second preparation meets a marker, not a token.
    let kept = f.storage.recent(1).unwrap();
    let summary = &kept[0].metadata[SUMMARY_KEY];
    assert!(summary["counts"]["provider_token"] == 1);
    assert!(summary["counts"]["private_block"] == 1);
    assert!(written[0].metadata[SUMMARY_KEY] == *summary);
}

/// A turn the store does not take reaches no sink, and stays pending.
#[test]
fn redaction_outputs_daemon_failed_commit_reaches_no_sink() {
    let token = token();
    let f = Fixture::new();
    f.storage
        .conn
        .lock()
        .unwrap()
        .execute_batch(
            "CREATE TRIGGER refuse_every_memory BEFORE INSERT ON memories
             BEGIN SELECT RAISE(ABORT, 'the store takes nothing'); END",
        )
        .unwrap();
    f.capture(&format!("Decision: use {token} for demoapp"));
    assert!(f.storage.count().unwrap() == 0);
    assert!(f.written.lock().unwrap().is_empty(), "a sink was written");
    assert!(f.files().is_empty(), "a sink wrote a file");
    assert!(!f.stored().contains(&token));
}

/// The urgent path stores a correction at once: it prepares the entry
/// itself, and every copy is of the prepared one.
#[test]
fn redaction_outputs_daemon_every_copy_of_a_correction_is_of_the_prepared_entry() {
    let token = token();
    let f = Fixture::new();
    f.urgent(&format!(
        "No, switch the configuration for example-org to {token} <private>and a plan</private>"
    ));
    assert!(f.storage.count().unwrap() == 1);

    let stored = f.stored();
    assert!(!stored.contains(&token) && !stored.contains("and a plan"));
    assert!(stored.contains(CREDENTIAL_MARKER));
    let given = f.embedder.texts();
    assert!(given.len() == 1 && !given[0].contains(&token));
    assert!(given[0].contains(CREDENTIAL_MARKER));
    let written = f.written.lock().unwrap();
    assert!(written.len() == 1);
    assert!(!serde_json::to_string(&written[0]).unwrap().contains(&token));
    let files = f.files();
    assert!(files.len() >= 2, "a sink wrote no file");
    for (path, bytes) in &files {
        assert!(!path.contains(&token), "a file is named after the token");
        assert!(!holds(bytes, &token) && !holds(bytes, "and a plan"));
    }
    // The title is cut from the content, and both held the token.
    let kept = f.storage.recent(1).unwrap();
    let summary = &kept[0].metadata[SUMMARY_KEY];
    assert!(
        summary["counts"]["provider_token"]
            .as_u64()
            .is_some_and(|n| n >= 1)
    );
    assert!(summary["counts"]["private_block"] == 1);
    assert!(written[0].metadata[SUMMARY_KEY] == *summary);
    assert!(f.storage.extraction_queue_count().unwrap() == 1);
}

/// A correction the store does not take reaches no sink and no queue.
#[test]
fn redaction_outputs_daemon_failed_urgent_commit_reaches_no_sink() {
    let token = token();
    let f = Fixture::new();
    f.storage
        .conn
        .lock()
        .unwrap()
        .execute_batch(
            "CREATE TRIGGER refuse_every_memory BEFORE INSERT ON memories
             BEGIN SELECT RAISE(ABORT, 'the store takes nothing'); END",
        )
        .unwrap();
    f.urgent(&format!(
        "No, switch the configuration for example-org to {token}"
    ));
    assert!(f.storage.count().unwrap() == 0);
    assert!(f.written.lock().unwrap().is_empty(), "a sink was written");
    assert!(f.files().is_empty(), "a sink wrote a file");
    assert!(f.storage.extraction_queue_count().unwrap() == 0);
}
