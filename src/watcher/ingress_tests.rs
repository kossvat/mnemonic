//! Both parsers exercise the same durable byte-reader, without sockets.
use super::*;
use crate::watcher::{codex::CodexWatcher, conversation::ConversationWatcher};
use std::io::Write;

struct Fixture {
    dir: PathBuf,
    storage: Arc<Storage>,
}
impl Fixture {
    fn new() -> Self {
        let dir =
            std::env::temp_dir().join(format!("mnemonic-transcript-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let storage = Arc::new(Storage::open(&dir.join("memory.db")).unwrap());
        Self { dir, storage }
    }
    fn reader(&self, namespace: &'static str) -> IngressTail {
        IngressTail::new(self.storage.clone(), namespace)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

type Parser = fn(&Path, &str) -> Option<ParsedTurn>;
fn formats() -> [(&'static str, Parser); 2] {
    [
        ("conversation", ConversationWatcher::parse_turn),
        ("codex", CodexWatcher::parse_turn),
    ]
}
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
fn ingress_partial_utf8_reopen_then_complete_is_consumed_once_by_both_watchers() {
    for (namespace, parse) in formats() {
        let fixture = Fixture::new();
        let path = fixture.dir.join("rollout-demoapp.jsonl");
        let text = line(namespace, Some("m1"), "Decision: используем 🚀 for demoapp");
        let split = text.find('🚀').unwrap() + 2;
        std::fs::write(&path, &text.as_bytes()[..split]).unwrap();
        let reader = fixture.reader(namespace);
        reader
            .bootstrap(std::slice::from_ref(&path), &HashMap::new())
            .unwrap();
        assert_eq!(reader.poll_file(&path, parse).unwrap(), 0);
        assert_eq!(
            fixture
                .storage
                .ingest_cursor(&reader.stream(&path))
                .unwrap()
                .unwrap()
                .offset,
            0
        );
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        file.write_all(&text.as_bytes()[split..]).unwrap();
        drop(file);
        let reopened = Arc::new(Storage::open(&fixture.dir.join("memory.db")).unwrap());
        let reader = IngressTail::new(reopened.clone(), namespace);
        reader
            .bootstrap(std::slice::from_ref(&path), &HashMap::new())
            .unwrap();
        assert_eq!(reader.poll_file(&path, parse).unwrap(), 1);
        assert_eq!(reader.poll_file(&path, parse).unwrap(), 0);
        let events = reopened.pending_ingest(chrono::Utc::now(), 10).unwrap();
        assert_eq!(events.len(), 1);
        assert!(events[0].event.content.contains('🚀'));
        assert_eq!(
            events[0].event.timestamp.to_rfc3339(),
            "2025-01-02T03:04:05+00:00"
        );
    }
}

#[test]
fn ingress_rotation_and_truncation_bump_generation_and_capture_new_records() {
    for (namespace, parse) in formats() {
        let fixture = Fixture::new();
        let reader = fixture.reader(namespace);
        reader.bootstrap(&[], &HashMap::new()).unwrap();
        let path = fixture.dir.join("rollout-demoapp.jsonl");
        std::fs::write(
            &path,
            line(
                namespace,
                None,
                "Decision: use a considerably longer original configuration",
            ),
        )
        .unwrap();
        reader.poll_file(&path, parse).unwrap();
        // Same inode, shorter file: replay from zero in a new generation.
        std::fs::write(&path, line(namespace, None, "Decision: use SQLite")).unwrap();
        reader.poll_file(&path, parse).unwrap();
        assert_eq!(
            fixture
                .storage
                .ingest_cursor(&reader.stream(&path))
                .unwrap()
                .unwrap()
                .generation,
            1
        );
        // Replacement inode of equal size is rotation, too.
        let replacement = fixture.dir.join("replacement");
        std::fs::write(&replacement, line(namespace, None, "Decision: use Redis!")).unwrap();
        std::fs::rename(&replacement, &path).unwrap();
        reader.poll_file(&path, parse).unwrap();
        assert_eq!(
            fixture
                .storage
                .ingest_cursor(&reader.stream(&path))
                .unwrap()
                .unwrap()
                .generation,
            2
        );
        assert_eq!(
            fixture
                .storage
                .pending_ingest(chrono::Utc::now(), 10)
                .unwrap()
                .len(),
            3
        );
    }
}

#[test]
fn ingress_truncate_regrow_past_offset_is_detected_from_content_anchor() {
    let fixture = Fixture::new();
    let reader = fixture.reader("conversation");
    reader.bootstrap(&[], &HashMap::new()).unwrap();
    let path = fixture.dir.join("session.jsonl");
    std::fs::write(&path, line("conversation", None, "Decision: use SQLite")).unwrap();
    reader
        .poll_file(&path, ConversationWatcher::parse_turn)
        .unwrap();
    std::fs::write(
        &path,
        line(
            "conversation",
            None,
            "Decision: use PostgreSQL and a much longer configuration",
        ),
    )
    .unwrap();
    reader
        .poll_file(&path, ConversationWatcher::parse_turn)
        .unwrap();
    assert_eq!(
        fixture
            .storage
            .ingest_cursor(&reader.stream(&path))
            .unwrap()
            .unwrap()
            .generation,
        1
    );
    assert_eq!(
        fixture
            .storage
            .pending_ingest(chrono::Utc::now(), 10)
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn ingress_message_identity_dedupes_rotation_but_equal_timestamps_do_not() {
    for (namespace, parse) in formats() {
        let fixture = Fixture::new();
        let reader = fixture.reader(namespace);
        reader.bootstrap(&[], &HashMap::new()).unwrap();
        let path = fixture.dir.join("rollout-demoapp.jsonl");
        let one = line(namespace, Some("m1"), "Decision: use SQLite");
        std::fs::write(&path, &one).unwrap();
        reader.poll_file(&path, parse).unwrap();
        let replacement = fixture.dir.join("replacement");
        std::fs::write(
            &replacement,
            format!(
                "{one}{}",
                line(namespace, Some("m2"), "Decision: use Redis")
            ),
        )
        .unwrap();
        std::fs::rename(&replacement, &path).unwrap();
        reader.poll_file(&path, parse).unwrap();
        assert_eq!(
            fixture
                .storage
                .pending_ingest(chrono::Utc::now(), 10)
                .unwrap()
                .len(),
            2
        );
    }
}

#[test]
fn ingress_bootstrap_imports_legacy_offsets_once_and_reads_new_files_after_downtime() {
    let fixture = Fixture::new();
    let reader = fixture.reader("conversation");
    let history = fixture.dir.join("old.jsonl");
    let known = fixture.dir.join("known.jsonl");
    std::fs::write(
        &history,
        line("conversation", Some("old"), "Decision: skip historic turn"),
    )
    .unwrap();
    std::fs::write(
        &known,
        line("conversation", Some("known"), "Decision: use SQLite"),
    )
    .unwrap();
    let legacy = HashMap::from([(known.clone(), 0)]);
    reader
        .bootstrap(&[history.clone(), known.clone()], &legacy)
        .unwrap();
    assert_eq!(
        reader
            .poll_file(&history, ConversationWatcher::parse_turn)
            .unwrap(),
        0
    );
    assert_eq!(
        reader
            .poll_file(&known, ConversationWatcher::parse_turn)
            .unwrap(),
        1
    );
    let new = fixture.dir.join("created-while-down.jsonl");
    std::fs::write(
        &new,
        line("conversation", Some("new"), "Decision: use Redis"),
    )
    .unwrap();
    reader
        .bootstrap(std::slice::from_ref(&new), &HashMap::new())
        .unwrap();
    assert_eq!(
        reader
            .poll_file(&new, ConversationWatcher::parse_turn)
            .unwrap(),
        1
    );
}

/// One transcript that cannot be fingerprinted must not stop every other
/// stream from being polled, and must not be skipped to EOF either: a file
/// left without a cursor is read from byte zero, replaying its whole history
/// the moment it becomes readable.
#[test]
fn ingress_one_unadoptable_file_does_not_starve_the_healthy_streams() {
    let fixture = Fixture::new();
    let reader = fixture.reader("conversation");
    let healthy = fixture.dir.join("healthy.jsonl");
    std::fs::write(
        &healthy,
        line("conversation", Some("known"), "Decision: use SQLite"),
    )
    .unwrap();
    // A path that opens but cannot be read. A directory is EISDIR for every
    // user, which a permission bit would not be under a root test runner.
    let broken = fixture.dir.join("unreadable.jsonl");
    std::fs::create_dir(&broken).unwrap();
    std::fs::write(broken.join("entry"), "x").unwrap();
    assert!(
        std::fs::metadata(&broken).unwrap().len() > 0,
        "the stand-in must have bytes to read, or nothing is read at all"
    );

    let legacy = HashMap::from([(healthy.clone(), 0u64)]);
    let files = [broken.clone(), healthy.clone()];
    assert_eq!(
        reader.bootstrap(&files, &legacy).unwrap(),
        HashSet::from([broken.clone()])
    );
    // The healthy stream was adopted at its legacy offset and keeps flowing.
    assert_eq!(
        reader
            .poll_file(&healthy, ConversationWatcher::parse_turn)
            .unwrap(),
        1
    );
    // The broken one is retried, so the marker stays open and a later pass
    // still resumes it instead of treating it as a brand-new session.
    assert!(
        fixture
            .storage
            .ingest_cursor("initialized:conversation")
            .unwrap()
            .is_none()
    );
    assert_eq!(
        reader.bootstrap(&files, &legacy).unwrap(),
        HashSet::from([broken.clone()])
    );

    std::fs::remove_dir_all(&broken).unwrap();
    assert!(reader.bootstrap(&files, &legacy).unwrap().is_empty());
    assert!(
        fixture
            .storage
            .ingest_cursor("initialized:conversation")
            .unwrap()
            .is_some()
    );
}

/// What a captured turn actually carries, stated honestly. A DECISION is
/// bounded twice: everything BEFORE the decision line is dropped, and the tail
/// is capped. A CORRECTION is not bounded at all, because the whole message IS
/// the memory. So dropping the separate turn copy bounds how many copies exist,
/// it does not make the payload free of private text; what bounds its lifetime
/// is the terminal receipt.
#[test]
fn ingress_payload_carries_the_event_alone_with_the_real_decision_bounds() {
    let fixture = Fixture::new();
    let reader = fixture.reader("conversation");
    reader.bootstrap(&[], &HashMap::new()).unwrap();
    let path = fixture.dir.join("session.jsonl");
    let before = "SENTINEL-before-the-decision-line";
    let far = "SENTINEL-past-the-excerpt-cap";
    let filler = "x".repeat(1100);
    let correction = "stop, use the other configuration for demoapp";
    std::fs::write(
        &path,
        format!(
            "{}{}",
            line(
                "conversation",
                Some("d1"),
                &format!("{before}\nDecision: use SQLite for demoapp.\n{filler}\n{far}")
            ),
            line("conversation", Some("c1"), correction)
        ),
    )
    .unwrap();
    assert_eq!(
        reader
            .poll_file(&path, ConversationWatcher::parse_turn)
            .unwrap(),
        2
    );

    let payloads: Vec<String> = {
        let conn = fixture.storage.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT payload FROM ingest_events ORDER BY seq")
            .unwrap();
        stmt.query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    };
    assert_eq!(payloads.len(), 2);
    for payload in &payloads {
        assert!(!payload.contains("raw_turn"), "{payload}");
    }
    assert!(payloads[0].contains("use SQLite"));
    assert!(
        !payloads[0].contains(before),
        "text before the decision line must be dropped"
    );
    assert!(
        !payloads[0].contains(far),
        "the excerpt cap must bound the tail"
    );
    assert!(
        payloads[1].contains(correction),
        "a correction IS its whole message, by design: {}",
        payloads[1]
    );
}

fn ingest_event_count(fixture: &Fixture) -> i64 {
    fixture
        .storage
        .conn
        .lock()
        .unwrap()
        .query_row("SELECT count(*) FROM ingest_events", [], |r| r.get(0))
        .unwrap()
}

fn append_line(path: &Path, text: String) {
    let mut file = std::fs::OpenOptions::new().append(true).open(path).unwrap();
    file.write_all(text.as_bytes()).unwrap();
}

/// A tick reads its whole namespace in ONE query. Before, every transcript
/// cost its own lookup and its own lock of the shared connection, every ten
/// seconds, on ~1,700 files.
#[test]
fn ingress_an_idle_tick_reads_the_cursors_once_not_once_per_file() {
    let fixture = Fixture::new();
    let reader = fixture.reader("conversation");
    let parse = ConversationWatcher::parse_turn;
    let mut files = Vec::new();
    for n in 0..25 {
        let path = fixture.dir.join(format!("session-{n}.jsonl"));
        std::fs::write(
            &path,
            line(
                "conversation",
                Some(&format!("m{n}")),
                "Decision: use SQLite",
            ),
        )
        .unwrap();
        files.push(path);
    }
    // The first tick adopts all 25 as history and writes the marker.
    assert_eq!(reader.tick(&files, &HashMap::new(), parse).unwrap(), 0);

    let lookups = || crate::ingest::CURSOR_LOOKUPS.with(|c| c.get());
    let before = lookups();
    assert_eq!(reader.tick(&files, &HashMap::new(), parse).unwrap(), 0);
    assert_eq!(
        lookups() - before,
        2,
        "the marker check plus ONE bulk read, whatever the number of files"
    );

    // Work still flows through the snapshot: a new turn is captured once.
    append_line(
        &files[7],
        line("conversation", Some("new"), "Decision: use Redis"),
    );
    let before = lookups();
    assert_eq!(reader.tick(&files, &HashMap::new(), parse).unwrap(), 1);
    assert_eq!(lookups() - before, 2);
    assert_eq!(reader.tick(&files, &HashMap::new(), parse).unwrap(), 0);
    assert_eq!(ingest_event_count(&fixture), 1);
}

/// Batching is a performance change and must not change behaviour. Before it,
/// every poll looked its cursor up fresh, so a second path sharing a stream
/// in the same tick saw the cursor the first one had just committed. The
/// snapshot keeps that by recording each committed cursor; without it the
/// second path would be refused as stale.
#[test]
fn ingress_batching_keeps_fresh_lookup_semantics_within_one_tick() {
    let fixture = Fixture::new();
    let reader = fixture.reader("conversation");
    let parse = ConversationWatcher::parse_turn;
    reader.bootstrap(&[], &HashMap::new()).unwrap();
    let mut files = Vec::new();
    for (dir, uuid) in [("first", "u1"), ("second", "u2")] {
        let path = fixture.dir.join(dir).join("same-name.jsonl");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            line("conversation", Some(uuid), "Decision: use SQLite"),
        )
        .unwrap();
        files.push(path);
    }
    assert_eq!(reader.stream(&files[0]), reader.stream(&files[1]));
    assert_eq!(
        reader.tick(&files, &HashMap::new(), parse).unwrap(),
        2,
        "the second path must see the cursor the first one committed"
    );
}

/// A snapshot can go stale. The compare-and-swap inside `append_ingest`
/// re-reads the cursor in the write transaction and refuses, so a stale
/// snapshot costs one skipped poll, never a duplicate or a lost turn.
#[test]
fn ingress_a_stale_snapshot_is_refused_by_the_cursor_cas_and_the_next_tick_recovers() {
    let fixture = Fixture::new();
    let reader = fixture.reader("conversation");
    let parse = ConversationWatcher::parse_turn;
    reader.bootstrap(&[], &HashMap::new()).unwrap();
    let path = fixture.dir.join("session.jsonl");
    std::fs::write(
        &path,
        line("conversation", Some("t1"), "Decision: use SQLite"),
    )
    .unwrap();
    let files = vec![path.clone()];
    assert_eq!(reader.tick(&files, &HashMap::new(), parse).unwrap(), 1);

    let mut stale = reader.cursor_snapshot().unwrap();
    append_line(
        &path,
        line("conversation", Some("t2"), "Decision: use Redis"),
    );
    assert_eq!(reader.tick(&files, &HashMap::new(), parse).unwrap(), 1);

    append_line(
        &path,
        line("conversation", Some("t3"), "Decision: use Postgres"),
    );
    let refused = reader.poll_file_with(&path, parse, &mut stale);
    assert!(
        refused
            .as_ref()
            .is_err_and(|e| e.to_string().contains("stale ingress cursor")),
        "a stale snapshot must not commit: {refused:?}"
    );
    assert_eq!(ingest_event_count(&fixture), 2);

    assert_eq!(
        reader.tick(&files, &HashMap::new(), parse).unwrap(),
        1,
        "the next tick takes only the turn that is really new"
    );
    assert_eq!(ingest_event_count(&fixture), 3);
}

/// If the bulk read fails, the tick fails. Treating the failure as an empty
/// snapshot would make every transcript look new and re-read it from byte zero.
#[test]
fn ingress_a_failed_cursor_read_fails_the_tick_instead_of_rereading_history() {
    let fixture = Fixture::new();
    let reader = fixture.reader("conversation");
    let parse = ConversationWatcher::parse_turn;
    let path = fixture.dir.join("history.jsonl");
    std::fs::write(
        &path,
        line("conversation", Some("old"), "Decision: use SQLite"),
    )
    .unwrap();
    let files = vec![path];
    assert_eq!(reader.tick(&files, &HashMap::new(), parse).unwrap(), 0);

    crate::ingest::FAIL_NEXT_BULK_READ.with(|f| f.set(true));
    assert!(
        reader.tick(&files, &HashMap::new(), parse).is_err(),
        "no snapshot, no poll"
    );
    assert_eq!(ingest_event_count(&fixture), 0);
    assert_eq!(
        reader.tick(&files, &HashMap::new(), parse).unwrap(),
        0,
        "history stays history"
    );
}

/// A /compact or resume copies earlier turns into a NEW transcript with the
/// same per-message uuid but a rewritten sessionId. Keying the message on the
/// transcript would re-ingest every copied turn, and an ingress correction
/// bypasses semantic dedup, so it would land in the store twice.
#[test]
fn ingress_a_forked_transcript_does_not_reingest_the_turns_it_copied() {
    let fixture = Fixture::new();
    let reader = fixture.reader("conversation");
    reader.bootstrap(&[], &HashMap::new()).unwrap();

    let turn = |session: &str| {
        let mut value: serde_json::Value = serde_json::from_str(&line(
            "conversation",
            Some("shared-message-uuid"),
            "stop, use the other configuration for demoapp",
        ))
        .unwrap();
        value["sessionId"] = session.into();
        format!("{value}\n")
    };
    let original = fixture.dir.join("original.jsonl");
    std::fs::write(&original, turn("session-a")).unwrap();
    assert_eq!(
        reader
            .poll_file(&original, ConversationWatcher::parse_turn)
            .unwrap(),
        1
    );

    // The fork: same turn, new file, new session id.
    let forked = fixture.dir.join("forked.jsonl");
    std::fs::write(&forked, turn("session-b")).unwrap();
    reader
        .poll_file(&forked, ConversationWatcher::parse_turn)
        .unwrap();

    let events: i64 = fixture
        .storage
        .conn
        .lock()
        .unwrap()
        .query_row("SELECT count(*) FROM ingest_events", [], |r| r.get(0))
        .unwrap();
    assert_eq!(events, 1, "the copied turn was ingested a second time");
    // A genuinely different turn in the fork is still captured.
    let mut appended = std::fs::OpenOptions::new()
        .append(true)
        .open(&forked)
        .unwrap();
    appended
        .write_all(line("conversation", Some("new-uuid"), "Decision: use Redis").as_bytes())
        .unwrap();
    drop(appended);
    assert_eq!(
        reader
            .poll_file(&forked, ConversationWatcher::parse_turn)
            .unwrap(),
        1
    );
}

/// An idle poll must not read and hash the same prefix bytes twice, and the
/// cursor it writes must be identical either way.
#[test]
fn ingress_idle_poll_reuses_the_prefix_hash_and_keeps_the_cursor_identical() {
    let fixture = Fixture::new();
    let reader = fixture.reader("conversation");
    reader.bootstrap(&[], &HashMap::new()).unwrap();
    let path = fixture.dir.join("session.jsonl");
    // Past the 4096-byte prefix window, so the reuse condition is the one
    // that holds on a real transcript.
    let mut body = String::new();
    for n in 0..40 {
        body.push_str(&line(
            "conversation",
            Some(&format!("m{n}")),
            &format!("Decision: use SQLite for demoapp because of reason number {n} which is long enough to push this file past the prefix window"),
        ));
    }
    assert!(body.len() > 4096, "fixture must exceed the prefix window");
    std::fs::write(&path, &body).unwrap();

    assert_eq!(
        reader
            .poll_file(&path, ConversationWatcher::parse_turn)
            .unwrap(),
        40
    );
    let first = fixture
        .storage
        .ingest_cursor(&reader.stream(&path))
        .unwrap()
        .unwrap();
    assert_eq!(first.prefix_len, 4096);
    assert_eq!(
        reader
            .poll_file(&path, ConversationWatcher::parse_turn)
            .unwrap(),
        0
    );
    let second = fixture
        .storage
        .ingest_cursor(&reader.stream(&path))
        .unwrap()
        .unwrap();
    assert_eq!(first, second, "a reused hash must equal the computed one");

    // A file still INSIDE the prefix window must not reuse a hash taken over
    // a shorter prefix: the stored prefix_len would not match its own hash and
    // every later poll would bump the generation and re-read the whole file.
    let short = fixture.dir.join("short.jsonl");
    std::fs::write(
        &short,
        line("conversation", Some("s1"), "Decision: use SQLite"),
    )
    .unwrap();
    reader
        .poll_file(&short, ConversationWatcher::parse_turn)
        .unwrap();
    let mut appended = std::fs::OpenOptions::new()
        .append(true)
        .open(&short)
        .unwrap();
    appended
        .write_all(line("conversation", Some("s2"), "Decision: use Redis").as_bytes())
        .unwrap();
    drop(appended);
    reader
        .poll_file(&short, ConversationWatcher::parse_turn)
        .unwrap();
    let grown = fixture
        .storage
        .ingest_cursor(&reader.stream(&short))
        .unwrap()
        .unwrap();
    assert!(grown.prefix_len < 4096 && grown.prefix_len == grown.offset);
    assert_eq!(
        reader
            .poll_file(&short, ConversationWatcher::parse_turn)
            .unwrap(),
        0
    );
    assert_eq!(
        fixture
            .storage
            .ingest_cursor(&reader.stream(&short))
            .unwrap()
            .unwrap(),
        grown,
        "a grown prefix must be re-hashed, not reused"
    );

    // Detection still works: rewrite the prefix in place, same length.
    let mut rewritten = body.clone();
    rewritten.replace_range(0..1, "X");
    std::fs::write(&path, &rewritten).unwrap();
    reader
        .poll_file(&path, ConversationWatcher::parse_turn)
        .unwrap();
    let third = fixture
        .storage
        .ingest_cursor(&reader.stream(&path))
        .unwrap()
        .unwrap();
    assert_eq!(
        third.generation,
        first.generation + 1,
        "a rewritten prefix must still bump the generation"
    );
}

/// Block cursor writes for one stream, so a single `adopt` fails the way a
/// transient store error would while its neighbours succeed.
fn block_cursor_writes(fixture: &Fixture, matching: &str) {
    fixture
        .storage
        .conn
        .lock()
        .unwrap()
        .execute_batch(&format!(
            "CREATE TRIGGER test_block_cursor BEFORE INSERT ON ingest_cursors
             WHEN NEW.stream LIKE '%{matching}%'
             BEGIN SELECT RAISE(ABORT, 'injected store failure'); END;"
        ))
        .unwrap();
}
fn unblock_cursor_writes(fixture: &Fixture) {
    fixture
        .storage
        .conn
        .lock()
        .unwrap()
        .execute_batch("DROP TRIGGER test_block_cursor")
        .unwrap();
}

/// A store error partway through the first pass must not drop the candidates
/// the pass never reached. A file left unadopted has no cursor, and the poll
/// reads a cursor-less file from byte zero: its entire history would be
/// replayed as if it were new work.
#[test]
fn ingress_bootstrap_keeps_unvisited_files_when_the_store_fails_midway() {
    let fixture = Fixture::new();
    let reader = fixture.reader("conversation");
    let mut files = Vec::new();
    for name in ["a.jsonl", "blocked.jsonl", "c.jsonl"] {
        let path = fixture.dir.join(name);
        std::fs::write(
            &path,
            line("conversation", Some(name), "Decision: use SQLite"),
        )
        .unwrap();
        files.push(path);
    }

    block_cursor_writes(&fixture, "blocked.jsonl");
    assert!(reader.bootstrap(&files, &HashMap::new()).is_err());
    unblock_cursor_writes(&fixture);

    // The recovery pass must still adopt c.jsonl, which the failed pass never
    // reached, so its history stays history.
    assert!(
        reader
            .bootstrap(&files, &HashMap::new())
            .unwrap()
            .is_empty()
    );
    for path in &files {
        assert!(
            fixture
                .storage
                .ingest_cursor(&reader.stream(path))
                .unwrap()
                .is_some(),
            "{} was dropped from the retry set",
            path.display()
        );
        assert_eq!(
            reader
                .poll_file(path, ConversationWatcher::parse_turn)
                .unwrap(),
            0,
            "{} replayed its history",
            path.display()
        );
    }
}

/// The first pass is recorded even when the marker write fails. Otherwise the
/// next tick rescans every file and adopts a session created in between as
/// history, checkpointing it at EOF and losing its opening turns.
#[test]
fn ingress_bootstrap_failing_marker_does_not_turn_new_sessions_into_history() {
    let fixture = Fixture::new();
    let reader = fixture.reader("conversation");
    let old = fixture.dir.join("old.jsonl");
    std::fs::write(
        &old,
        line("conversation", Some("old"), "Decision: use SQLite"),
    )
    .unwrap();

    block_cursor_writes(&fixture, "initialized:");
    assert!(
        reader
            .bootstrap(std::slice::from_ref(&old), &HashMap::new())
            .is_err()
    );
    unblock_cursor_writes(&fixture);
    assert!(
        fixture
            .storage
            .ingest_cursor(&reader.stream(&old))
            .unwrap()
            .is_some(),
        "the file itself was adopted before the marker failed"
    );

    // A session that starts after that pass is NEW work, not history.
    let new = fixture.dir.join("started-after.jsonl");
    std::fs::write(
        &new,
        line("conversation", Some("new"), "Decision: use Redis"),
    )
    .unwrap();
    assert!(
        reader
            .bootstrap(&[old, new.clone()], &HashMap::new())
            .unwrap()
            .is_empty()
    );
    assert!(
        fixture
            .storage
            .ingest_cursor(&reader.stream(&new))
            .unwrap()
            .is_none(),
        "a file created after the first pass must not be adopted as history"
    );
    assert_eq!(
        reader
            .poll_file(&new, ConversationWatcher::parse_turn)
            .unwrap(),
        1,
        "its opening turn must still be captured"
    );
}

/// A turn appended while a poll parses its snapshot belongs to the NEXT poll.
/// Treating it as a rewrite would throw the batch away and, on a transcript
/// that is written continuously, stop the cursor from ever advancing.
#[test]
fn ingress_append_during_a_poll_keeps_the_batch_and_advances_the_cursor() {
    fn append(path: &Path, original: &str) -> Option<ParsedTurn> {
        let parsed = ConversationWatcher::parse_turn(path, original);
        let mut file = std::fs::OpenOptions::new().append(true).open(path).unwrap();
        file.write_all(line("conversation", Some("later"), "Decision: use Redis").as_bytes())
            .unwrap();
        parsed
    }
    let fixture = Fixture::new();
    let reader = fixture.reader("conversation");
    reader.bootstrap(&[], &HashMap::new()).unwrap();
    let path = fixture.dir.join("session.jsonl");
    std::fs::write(
        &path,
        line("conversation", Some("first"), "Decision: use SQLite"),
    )
    .unwrap();

    assert_eq!(reader.poll_file(&path, append).unwrap(), 1);
    let cursor = fixture
        .storage
        .ingest_cursor(&reader.stream(&path))
        .unwrap()
        .expect("the append must not discard the acknowledgement");
    assert!(cursor.offset > 0);
    assert_eq!(
        reader
            .poll_file(&path, ConversationWatcher::parse_turn)
            .unwrap(),
        1,
        "the appended turn is captured by the next poll"
    );
    let captured: Vec<String> = fixture
        .storage
        .pending_ingest(chrono::Utc::now(), 10)
        .unwrap()
        .into_iter()
        .map(|pending| pending.event.content)
        .collect();
    assert_eq!(captured.len(), 2);
    assert!(captured[0].contains("SQLite"), "{captured:?}");
    assert!(captured[1].contains("Redis"), "{captured:?}");
}

#[test]
fn ingress_rewrite_during_read_is_retried_without_acknowledging_mixed_bytes() {
    fn rewrite(path: &Path, original: &str) -> Option<ParsedTurn> {
        let parsed = ConversationWatcher::parse_turn(path, original);
        std::fs::write(
            path,
            line(
                "conversation",
                Some("replacement"),
                "Decision: use a substantially longer replacement configuration",
            ),
        )
        .unwrap();
        parsed
    }
    let fixture = Fixture::new();
    let reader = fixture.reader("conversation");
    reader.bootstrap(&[], &HashMap::new()).unwrap();
    let path = fixture.dir.join("session.jsonl");
    std::fs::write(
        &path,
        line("conversation", Some("original"), "Decision: use SQLite"),
    )
    .unwrap();
    assert!(reader.poll_file(&path, rewrite).is_err());
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
            .pending_ingest(chrono::Utc::now(), 10)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        reader
            .poll_file(&path, ConversationWatcher::parse_turn)
            .unwrap(),
        1
    );
    assert!(
        fixture
            .storage
            .pending_ingest(chrono::Utc::now(), 1)
            .unwrap()[0]
            .event
            .content
            .contains("replacement")
    );
}
