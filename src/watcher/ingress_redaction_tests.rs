//! Capture admission: what a transcript turn may carry into the durable
//! queue, for both parsers, and through history. Credential fixtures are
//! assembled at run time; assertions never print them.
use std::collections::HashMap;

use super::*;
use crate::redaction::SUMMARY_KEY;
use crate::watcher::{codex::CodexWatcher, conversation::ConversationWatcher};

fn body(n: usize) -> String {
    "a1B2c3D4e5F6".chars().cycle().take(n).collect()
}

fn token() -> String {
    ["sk-", "proj-", &body(40)].concat()
}

struct Fixture {
    dir: tempfile::TempDir,
    storage: Arc<Storage>,
}

impl Fixture {
    fn new() -> Self {
        let dir = crate::test_support::temp_dir("mnemonic-redaction-ingress-");
        let storage = Arc::new(Storage::open(&dir.path().join("memory.db")).unwrap());
        Self { dir, storage }
    }

    fn reader(&self, namespace: &'static str) -> IngressTail {
        IngressTail::new(self.storage.clone(), namespace)
    }

    fn queued(&self) -> Vec<Event> {
        self.storage
            .pending_ingest(Utc::now(), 100)
            .unwrap()
            .into_iter()
            .map(|p| p.event)
            .collect()
    }

    /// Every text column of the queue tables, joined: what a scanner of the
    /// database file would see.
    fn queue_text(&self) -> String {
        let conn = self.storage.conn.lock().unwrap();
        let mut out = String::new();
        for sql in [
            "SELECT source_key || ' ' || coalesce(payload, '') FROM ingest_events",
            "SELECT stream FROM ingest_cursors",
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

type Parser = fn(&Path, &str) -> Option<ParsedTurn>;

fn formats() -> [(&'static str, Parser); 2] {
    [
        ("conversation", ConversationWatcher::parse_turn),
        ("codex", CodexWatcher::parse_turn),
    ]
}

/// One user turn in either format, with a message id when given.
fn line(namespace: &str, id: Option<&str>, text: &str) -> String {
    let mut value = if namespace == "conversation" {
        serde_json::json!({"type":"user", "sessionId":"demoapp-session",
            "timestamp":"2025-01-02T04:04:05+01:00", "message":{"content":text}})
    } else {
        serde_json::json!({"type":"response_item", "timestamp":"2025-01-02T04:04:05+01:00",
            "payload":{"type":"message", "role":"user", "content":[{"type":"input_text", "text":text}]}})
    };
    if let Some(id) = id {
        if namespace == "conversation" {
            value["uuid"] = id.into();
        } else {
            value["payload"]["id"] = id.into();
        }
    }
    format!("{value}\n")
}

#[test]
fn redaction_ingress_both_parsers_queue_prepared_text_only() {
    let token = token();
    for (namespace, parse) in formats() {
        let fixture = Fixture::new();
        let path = fixture.dir.path().join("rollout-demoapp.jsonl");
        let text = format!("не так, use {token} for the deploy <private>call 555-0100</private>");
        std::fs::write(&path, line(namespace, Some("m1"), &text)).unwrap();
        let reader = fixture.reader(namespace);
        reader.bootstrap(&[], &HashMap::new()).unwrap();
        assert!(reader.poll_file(&path, parse).unwrap() == 1, "{namespace}");

        let events = fixture.queued();
        assert!(events.len() == 1);
        let event = &events[0];
        assert!(event.kind == EventKind::UserCorrection);
        assert!(!event.content.contains(&token) && !event.content.contains("555"));
        assert!(event.content.contains("[REDACTED:credential]"));
        assert!(event.content.contains("[REDACTED:private]"));
        let summary = &event.metadata[SUMMARY_KEY];
        assert!(summary["counts"]["provider_token"] == 1, "{namespace}");
        assert!(summary["counts"]["private_block"] == 1, "{namespace}");
        assert!(event.metadata["role"] == "user");

        let stored = fixture.queue_text();
        assert!(
            !stored.contains(&token) && !stored.contains("555"),
            "{namespace}"
        );
    }
}

#[test]
fn redaction_ingress_a_private_region_is_gone_before_the_excerpt_is_cut() {
    for (namespace, parse) in formats() {
        let fixture = Fixture::new();
        let reader = fixture.reader(namespace);
        reader.bootstrap(&[], &HashMap::new()).unwrap();

        // The region ends inside the decision line: nothing of it, and no
        // decision either, reaches the queue.
        let crossing = fixture.dir.path().join("rollout-crossing.jsonl");
        std::fs::write(
            &crossing,
            line(
                namespace,
                Some("m1"),
                "<private>the numbers\nDecision: use SQLite</private> for the index\nmore",
            ),
        )
        .unwrap();
        assert!(
            reader.poll_file(&crossing, parse).unwrap() == 0,
            "{namespace}"
        );

        // The region ends before the decision line: the excerpt starts after it.
        let before = fixture.dir.path().join("rollout-before.jsonl");
        std::fs::write(
            &before,
            line(
                namespace,
                Some("m2"),
                "<private>the numbers</private>\nDecision: use SQLite for the index",
            ),
        )
        .unwrap();
        assert!(
            reader.poll_file(&before, parse).unwrap() == 1,
            "{namespace}"
        );
        let events = fixture.queued();
        assert!(events.len() == 1);
        assert!(!events[0].content.contains("numbers"));
        assert!(events[0].content.starts_with("Decision: use SQLite"));
        assert!(!fixture.queue_text().contains("numbers"));
    }
}

#[test]
fn redaction_ingress_a_sensitive_message_identity_is_not_captured_but_the_cursor_moves() {
    let token = token();
    for (namespace, parse) in formats() {
        let fixture = Fixture::new();
        let path = fixture.dir.path().join("rollout-demoapp.jsonl");
        let text = [
            line(
                namespace,
                Some(&token),
                "Decision: use SQLite for the index",
            ),
            line(
                namespace,
                Some("m2"),
                "Decision: use Postgres for the ledger",
            ),
        ]
        .concat();
        std::fs::write(&path, &text).unwrap();
        let reader = fixture.reader(namespace);
        reader.bootstrap(&[], &HashMap::new()).unwrap();
        assert!(reader.poll_file(&path, parse).unwrap() == 1, "{namespace}");
        let cursor = fixture
            .storage
            .ingest_cursor(&reader.stream(&path))
            .unwrap()
            .unwrap();
        assert!(
            (cursor.offset as usize) == text.len(),
            "the cursor moved past it"
        );
        let events = fixture.queued();
        assert!(events.len() == 1);
        assert!(events[0].content.contains("Postgres"));
        assert!(!fixture.queue_text().contains(&token), "{namespace}");
        // Nothing to replay: the next poll is idle.
        assert!(reader.poll_file(&path, parse).unwrap() == 0);
    }
}

#[test]
fn redaction_ingress_a_transcript_with_a_sensitive_path_gets_no_cursor() {
    let token = token();
    for (namespace, parse) in formats() {
        let fixture = Fixture::new();
        let path = fixture.dir.path().join(format!("rollout-{token}.jsonl"));
        std::fs::write(
            &path,
            line(namespace, Some("m1"), "Decision: use SQLite for the index"),
        )
        .unwrap();
        let safe = fixture.dir.path().join("rollout-safe.jsonl");
        std::fs::write(
            &safe,
            line(
                namespace,
                Some("m2"),
                "Decision: use Postgres for the ledger",
            ),
        )
        .unwrap();
        let reader = fixture.reader(namespace);
        // Bootstrap adopts the safe file and settles the other without a row.
        let files = [path.clone(), safe.clone()];
        assert!(
            reader
                .bootstrap(&files, &HashMap::new())
                .unwrap()
                .is_empty()
        );
        assert!(
            fixture
                .storage
                .ingest_cursor(&reader.stream(&path))
                .unwrap()
                .is_none()
        );
        assert!(
            fixture
                .storage
                .ingest_cursor(&reader.stream(&safe))
                .unwrap()
                .is_some()
        );
        // A tick skips it, keeps polling the rest, and never logs its name.
        std::fs::OpenOptions::new()
            .append(true)
            .open(&safe)
            .unwrap()
            .write_all(line(namespace, Some("m3"), "Decision: use Redis for the cache").as_bytes())
            .unwrap();
        // A reader that has not seen the file yet reports it on this tick.
        let reader = fixture.reader(namespace);
        let log = Log::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(log.clone())
            .with_ansi(false)
            .finish();
        let captured = tracing::subscriber::with_default(subscriber, || {
            reader.tick(&files, &HashMap::new(), parse).unwrap()
        });
        assert!(captured == 1);
        let logged = log.text();
        assert!(
            logged.matches("transcript path refused").count() == 1,
            "{namespace}: reported other than once"
        );
        assert!(
            !logged.contains(&token),
            "{namespace}: the log names the transcript"
        );
        // Reported once per path, not once per tick.
        let again = Log::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(again.clone())
            .with_ansi(false)
            .finish();
        tracing::subscriber::with_default(subscriber, || {
            reader.tick(&files, &HashMap::new(), parse).unwrap()
        });
        assert!(
            !again.text().contains("refused"),
            "{namespace}: reported again"
        );
        assert!(
            fixture
                .storage
                .ingest_cursor(&reader.stream(&path))
                .unwrap()
                .is_none()
        );
        // Polling it directly is an error, not a row.
        assert!(reader.poll_file(&path, parse).is_err());
        assert!(!fixture.queue_text().contains(&token), "{namespace}");
    }
}

/// A turn whose timestamp is credential-shaped is captured, dated at
/// observation, and the timestamp is not kept: the queue would otherwise
/// refuse the whole batch and the transcript would stall on it.
/// A file name whose JSON encoding joins a value that a raw tab split: the
/// stored stream would be refused by the queue. Bootstrap must settle such
/// a file without a row instead of failing the whole namespace on it.
#[test]
fn redaction_ingress_a_name_refused_only_in_its_stored_form_is_settled_not_fatal() {
    for (namespace, parse) in formats() {
        let fixture = Fixture::new();
        let tricky = fixture
            .dir
            .path()
            .join("token=A1B2C3D4E5F6G7H8I9\tJ0.jsonl");
        std::fs::write(
            &tricky,
            line(namespace, Some("m1"), "Decision: use SQLite for the index"),
        )
        .unwrap();
        let safe = fixture.dir.path().join("rollout-safe.jsonl");
        std::fs::write(
            &safe,
            line(
                namespace,
                Some("m2"),
                "Decision: use Postgres for the ledger",
            ),
        )
        .unwrap();
        let reader = fixture.reader(namespace);
        let files = [tricky.clone(), safe.clone()];
        assert!(
            reader
                .bootstrap(&files, &HashMap::new())
                .unwrap()
                .is_empty(),
            "{namespace}"
        );
        assert!(
            fixture
                .storage
                .ingest_cursor(&format!("initialized:{namespace}"))
                .unwrap()
                .is_some(),
            "{namespace}: the namespace finished bootstrapping"
        );
        assert!(
            fixture
                .storage
                .ingest_cursor(&reader.stream(&tricky))
                .unwrap()
                .is_none()
        );
        assert!(reader.tick(&files, &HashMap::new(), parse).unwrap() == 0);
        assert!(reader.poll_file(&tricky, parse).is_err());
    }
}

/// A message id that is clean raw but dirty once JSON-escaped into its
/// stored key (a tab becomes `\t`, joining the value): the turn is skipped,
/// the cursor moves on, and the next turn is captured.
#[test]
fn redaction_ingress_a_key_dirty_only_when_serialized_skips_the_turn_not_the_stream() {
    let raw_id = "token=A1B2C3D4E5F6G7H8I9\tJ0";
    for (namespace, parse) in formats() {
        let fixture = Fixture::new();
        let path = fixture.dir.path().join("rollout-demoapp.jsonl");
        let text = [
            line(
                namespace,
                Some(raw_id),
                "Decision: use SQLite for the index",
            ),
            line(
                namespace,
                Some("m2"),
                "Decision: use Postgres for the ledger",
            ),
        ]
        .concat();
        std::fs::write(&path, &text).unwrap();
        let reader = fixture.reader(namespace);
        reader.bootstrap(&[], &HashMap::new()).unwrap();
        assert!(reader.poll_file(&path, parse).unwrap() == 1, "{namespace}");
        let cursor = fixture
            .storage
            .ingest_cursor(&reader.stream(&path))
            .unwrap()
            .unwrap();
        assert!((cursor.offset as usize) == text.len(), "{namespace}");
        let events = fixture.queued();
        assert!(events.len() == 1);
        assert!(events[0].content.contains("Postgres"));
        assert!(
            !fixture.queue_text().contains("A1B2C3D4E5F6G7H8I9"),
            "{namespace}"
        );
    }
}

#[test]
fn redaction_ingress_a_sensitive_timestamp_is_dropped_and_the_stream_goes_on() {
    let token = token();
    for (namespace, parse) in formats() {
        let fixture = Fixture::new();
        let path = fixture.dir.path().join("rollout-demoapp.jsonl");
        let text = line(namespace, Some("m1"), "Decision: use SQLite for the index")
            .replace("2025-01-02T04:04:05+01:00", &token);
        assert!(
            text.contains(&token),
            "{namespace}: the fixture carries the token"
        );
        std::fs::write(
            &path,
            [
                text.as_str(),
                &line(namespace, Some("m2"), "Decision: use Redis"),
            ]
            .concat(),
        )
        .unwrap();
        let reader = fixture.reader(namespace);
        reader.bootstrap(&[], &HashMap::new()).unwrap();
        let before = Utc::now();
        assert!(reader.poll_file(&path, parse).unwrap() == 2, "{namespace}");
        let events = fixture.queued();
        assert!(events.len() == 2);
        assert!(events[0].timestamp >= before, "dated at observation");
        assert!(!fixture.queue_text().contains(&token), "{namespace}");
        let source_at: Option<String> = fixture
            .storage
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT source_at FROM ingest_events ORDER BY seq LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(source_at.is_none(), "{namespace}");
    }
}

use std::io::Write;
use std::sync::Mutex;

/// Everything the code under test logs, to prove what it does not say.
#[derive(Clone, Default)]
struct Log(Arc<Mutex<Vec<u8>>>);

impl Log {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

impl Write for Log {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Log {
    type Writer = Log;

    fn make_writer(&'a self) -> Log {
        self.clone()
    }
}

/// History reads through the same parsers: a turn with a credential is
/// queued prepared, flagged and summarized; a transcript with a sensitive
/// path is counted as refused and left out.
#[test]
fn redaction_ingress_history_queues_prepared_turns_and_refuses_sensitive_paths() {
    use crate::config::Config;
    use crate::watcher::history::{HISTORY_FLAG, ingest_history};

    let token = token();
    let fixture = Fixture::new();
    let root = fixture.dir.path();
    let claude = root.join("claude-projects");
    std::fs::create_dir_all(claude.join("-code-demoapp")).unwrap();
    let turn = |id: &str, text: &str| {
        serde_json::json!({"type": "user", "uuid": id, "sessionId": format!("s-{id}"),
            "cwd": "/code/demoapp", "timestamp": "2026-01-02T10:00:00Z",
            "message": {"content": text}})
        .to_string()
            + "\n"
    };
    std::fs::write(
        claude.join("-code-demoapp/one.jsonl"),
        turn("h1", &format!("не так, the deploy key is {token}")),
    )
    .unwrap();
    std::fs::write(
        claude.join(format!("-code-demoapp/{token}.jsonl")),
        turn("h2", "не так, use the staging bucket"),
    )
    .unwrap();

    let mut config = Config::default();
    config.storage.db_path = root.join("memory.db");
    config.watchers.conversation_enabled = true;
    config.watchers.conversation_sessions_dir = Some(claude.clone());
    config.watchers.codex_enabled = false;

    // The same refused turn copied into a second transcript by /compact:
    // one refused key, one repeat.
    for name in ["two.jsonl", "three.jsonl"] {
        std::fs::write(
            claude.join(format!("-code-demoapp/{name}")),
            turn(
                "token=A1B2C3D4E5F6G7H8I9\tJ0",
                "не так, use the other bucket",
            ),
        )
        .unwrap();
    }

    let dry = ingest_history(&fixture.storage, &config, None, false).unwrap();
    assert!(
        (
            dry.turns,
            dry.new,
            dry.refused,
            dry.refused_turns,
            dry.repeated,
            dry.files
        ) == (1, 1, 1, 1, 1, 3)
    );
    assert!(fixture.queued().is_empty());
    let applied = ingest_history(&fixture.storage, &config, None, true).unwrap();
    assert!((applied.turns, applied.new, applied.refused) == (1, 1, 1));

    let events = fixture.queued();
    assert!(events.len() == 1);
    let event = &events[0];
    assert!(!event.content.contains(&token));
    assert!(event.content.contains("[REDACTED:credential]"));
    assert!(event.metadata[HISTORY_FLAG] == true);
    assert!(event.metadata[SUMMARY_KEY]["counts"]["provider_token"] == 1);
    assert!(!fixture.queue_text().contains(&token));
}
