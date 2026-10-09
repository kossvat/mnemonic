//! The writer of the recall log on a store in a scratch folder: what it
//! writes, what it drops, what a pass of retention removes, and what the
//! store does with the tables.
use super::*;
use crate::event::{EventSource, MemoryEntry, MemoryType};
use crate::storage::Storage;

struct Scratch {
    _dir: tempfile::TempDir,
    path: PathBuf,
    storage: Storage,
}

impl Scratch {
    fn new() -> Self {
        let dir = crate::test_support::temp_dir("mn-recall-");
        let path = dir.path().join("memory.db");
        let storage = Storage::open(&path).unwrap();
        Self {
            _dir: dir,
            path,
            storage,
        }
    }

    fn writer(&self) -> Writer {
        Writer::new(&self.path, true)
    }

    fn memory(&self, title: &str, memory_type: MemoryType) -> String {
        let entry = MemoryEntry::new(title, "body", memory_type, EventSource::Socket);
        self.storage.save(&entry).unwrap();
        entry.id
    }

    fn count(&self, sql: &str) -> i64 {
        let conn = self.storage.conn.lock().unwrap();
        conn.query_row(sql, [], |r| r.get(0)).unwrap()
    }

    fn items(&self, event: i64) -> Vec<(i64, String, Option<String>)> {
        let conn = self.storage.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT rank, memory_id, section FROM recall_items WHERE event_id = ?1 ORDER BY rank",
            )
            .unwrap();
        stmt.query_map(params![event], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    }
}

fn record(ids: &[&str]) -> Record {
    Record {
        surface: Surface::Mcp,
        tool: Tool::MemorySearch,
        items: ids
            .iter()
            .map(|id| Item {
                memory_id: id.to_string(),
                section: None,
            })
            .collect(),
        chars: 10,
        hidden: 0,
    }
}

#[test]
fn recall_log_an_event_holds_its_numbers_and_its_items_in_order() {
    let s = Scratch::new();
    let a = s.memory("a", MemoryType::Decision);
    let b = s.memory("b", MemoryType::Feedback);
    let w = s.writer();
    w.name_client("Claude Code");
    let mut rec = record(&[&b, &a]);
    rec.chars = 321;
    rec.hidden = 2;
    rec.items[0].section = Some(Section::Feedback);
    rec.items[1].section = Some(Section::Decisions);
    let at: DateTime<Utc> = "2026-09-30T10:00:00Z".parse().unwrap();
    let event = w.write(&rec, at).unwrap();
    let conn = s.storage.conn.lock().unwrap();
    let row: (
        String,
        String,
        String,
        Option<String>,
        String,
        i64,
        i64,
        i64,
        i64,
    ) = conn
        .query_row(
            "SELECT at, surface, tool, client, run, returned, corrections, chars, hidden
             FROM recall_events WHERE id = ?1",
            params![event],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                    r.get(7)?,
                    r.get(8)?,
                ))
            },
        )
        .unwrap();
    drop(conn);
    assert_eq!(row.0, at.to_rfc3339());
    assert_eq!((row.1.as_str(), row.2.as_str()), ("mcp", "memory_search"));
    assert_eq!(row.3.as_deref(), Some("claude code"));
    assert_eq!(row.4, w.run());
    assert_eq!((row.5, row.6, row.7, row.8), (2, 1, 321, 2));
    assert_eq!(
        s.items(event),
        [
            (0, b.clone(), Some("feedback".to_string())),
            (1, a.clone(), Some("decisions".to_string())),
        ]
    );
    // The run is one per writer, and each writer has one of its own.
    assert_ne!(w.run(), s.writer().run());
    assert_eq!(w.run().len(), 36);
}

#[test]
fn recall_log_a_memory_gone_before_the_record_leaves_no_item_and_is_counted() {
    let s = Scratch::new();
    let a = s.memory("a", MemoryType::Decision);
    let gone = s.memory("gone", MemoryType::Decision);
    s.storage.forget_by_id(&gone).unwrap();
    let w = s.writer();
    let event = w.write(&record(&[&gone, &a]), Utc::now()).unwrap();
    assert_eq!(s.items(event), [(1, a.clone(), None)]);
    assert_eq!(
        s.count(&format!(
            "SELECT returned FROM recall_events WHERE id = {event}"
        )),
        2
    );
}

#[test]
fn recall_log_forgetting_a_memory_later_removes_its_items_and_keeps_the_numbers() {
    let s = Scratch::new();
    let a = s.memory("a", MemoryType::Decision);
    let b = s.memory("b", MemoryType::Decision);
    let w = s.writer();
    let event = w.write(&record(&[&a, &b]), Utc::now()).unwrap();
    assert_eq!(s.items(event).len(), 2);
    s.storage.forget_by_id(&a).unwrap();
    assert_eq!(s.items(event), [(1, b.clone(), None)]);
    assert_eq!(
        s.count(&format!(
            "SELECT returned FROM recall_events WHERE id = {event}"
        )),
        2
    );
}

#[test]
fn recall_log_the_connection_of_the_writer_has_foreign_keys_on() {
    let s = Scratch::new();
    let a = s.memory("a", MemoryType::Decision);
    let w = s.writer();
    let event = w.write(&record(&[&a]), Utc::now()).unwrap();
    let conn = w.connect().unwrap();
    let on: i64 = conn
        .query_row("PRAGMA foreign_keys", [], |r| r.get(0))
        .unwrap();
    assert_eq!(on, 1);
    conn.execute("DELETE FROM recall_events WHERE id = ?1", params![event])
        .unwrap();
    drop(conn);
    assert_eq!(s.count("SELECT COUNT(*) FROM recall_items"), 0);
}

#[test]
fn recall_log_the_trigger_and_the_tables_survive_a_reopen_and_a_rebuild() {
    let dir = crate::test_support::temp_dir("mn-recall-");
    let path = dir.path().join("memory.db");
    let exists = |what: &str, name: &str| -> bool {
        let conn = Connection::open(&path).unwrap();
        conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = ?1 AND name = ?2",
            params![what, name],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
            == 1
    };
    let generation = || -> i64 {
        let conn = Connection::open(&path).unwrap();
        conn.query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap()
    };
    let storage = Storage::open(&path).unwrap();
    assert!(exists("trigger", "trg_recall_items_memory_deleted"));
    let before = generation();
    // A store without the tables opens and gains them; the generation
    // does not move.
    {
        let conn = storage.conn.lock().unwrap();
        conn.execute_batch(
            "DROP TRIGGER trg_recall_items_memory_deleted;
             DROP TABLE recall_items; DROP TABLE recall_events;",
        )
        .unwrap();
    }
    drop(storage);
    assert!(!exists("table", "recall_events"));
    let storage = Storage::open(&path).unwrap();
    assert!(exists("table", "recall_events"));
    assert!(exists("table", "recall_items"));
    assert!(exists("trigger", "trg_recall_items_memory_deleted"));
    assert_eq!(generation(), before);
    // A rebuild of the memories table drops its triggers; a reopen brings
    // the trigger back.
    {
        let conn = storage.conn.lock().unwrap();
        conn.execute_batch(
            "CREATE TABLE memories_new AS SELECT * FROM memories;
             DROP TABLE memories;
             ALTER TABLE memories_new RENAME TO memories;",
        )
        .unwrap();
    }
    drop(storage);
    assert!(!exists("trigger", "trg_recall_items_memory_deleted"));
    let _storage = Storage::open(&path).unwrap();
    assert!(exists("trigger", "trg_recall_items_memory_deleted"));
}

#[test]
fn recall_log_a_pass_removes_the_oldest_thousand_and_younger_events_stay() {
    let s = Scratch::new();
    let a = s.memory("a", MemoryType::Decision);
    let w = s.writer();
    let now = Utc::now();
    // 2500 expired events, the oldest first, and 3 young ones.
    {
        let mut conn = w.connect().unwrap();
        let tx = conn.transaction().unwrap();
        for i in 0..2500 {
            let at =
                now - chrono::Duration::days(RETENTION_DAYS + 200) + chrono::Duration::minutes(i);
            tx.execute(
                "INSERT INTO recall_events (at, surface, tool, run, returned, corrections, chars, hidden)
                 VALUES (?1, 'mcp', 'memory_search', 'r', 1, 0, 1, 0)",
                params![at.to_rfc3339()],
            )
            .unwrap();
            let event = tx.last_insert_rowid();
            tx.execute(
                "INSERT INTO recall_items (event_id, rank, memory_id) VALUES (?1, 0, ?2)",
                params![event, a],
            )
            .unwrap();
        }
        for i in 0..3 {
            let at = now - chrono::Duration::days(i);
            tx.execute(
                "INSERT INTO recall_events (at, surface, tool, run, returned, corrections, chars, hidden)
                 VALUES (?1, 'mcp', 'memory_search', 'r', 0, 0, 1, 0)",
                params![at.to_rfc3339()],
            )
            .unwrap();
        }
        tx.commit().unwrap();
    }
    assert_eq!(s.count("SELECT COUNT(*) FROM recall_events"), 2503);
    let oldest = |s: &Scratch| -> String {
        let conn = s.storage.conn.lock().unwrap();
        conn.query_row("SELECT MIN(at) FROM recall_events", [], |r| r.get(0))
            .unwrap()
    };
    let before = oldest(&s);

    // The first record of the process makes a pass.
    w.write(&record(&[&a]), now).unwrap();
    assert_eq!(s.count("SELECT COUNT(*) FROM recall_events"), 1504);
    assert_eq!(s.count("SELECT COUNT(*) FROM recall_items"), 1501);
    assert!(oldest(&s) > before, "the oldest went first");
    // The next record makes none: the hour has not passed.
    w.write(&record(&[&a]), now).unwrap();
    assert_eq!(s.count("SELECT COUNT(*) FROM recall_events"), 1505);
    // A pass of its own removes the next thousand, and the young ones stay.
    let conn = w.connect().unwrap();
    assert_eq!(Writer::pass(&conn, now).unwrap(), 1000);
    assert_eq!(Writer::pass(&conn, now).unwrap(), 500);
    assert_eq!(Writer::pass(&conn, now).unwrap(), 0);
    drop(conn);
    assert_eq!(s.count("SELECT COUNT(*) FROM recall_events"), 5);
    assert_eq!(
        s.count(&format!(
            "SELECT COUNT(*) FROM recall_events WHERE at < '{}'",
            (now - chrono::Duration::days(RETENTION_DAYS)).to_rfc3339()
        )),
        0
    );
}

#[test]
fn recall_log_a_pass_that_failed_is_tried_with_the_next_record() {
    let s = Scratch::new();
    let a = s.memory("a", MemoryType::Decision);
    let w = s.writer();
    let now = Utc::now();
    {
        let conn = w.connect().unwrap();
        conn.execute(
            "INSERT INTO recall_events (at, surface, tool, run, returned, corrections, chars, hidden)
             VALUES (?1, 'mcp', 'memory_search', 'r', 0, 0, 1, 0)",
            params![(now - chrono::Duration::days(RETENTION_DAYS + 1)).to_rfc3339()],
        )
        .unwrap();
        conn.execute_batch(
            "CREATE TRIGGER refuse_the_pass BEFORE DELETE ON recall_events
             BEGIN SELECT RAISE(ABORT, 'not now'); END",
        )
        .unwrap();
    }
    // The record is written; the pass fails and is not counted as made.
    w.write(&record(&[&a]), now).unwrap();
    assert_eq!(s.count("SELECT COUNT(*) FROM recall_events"), 2);
    assert!(w.pass_is_due());
    {
        let conn = w.connect().unwrap();
        conn.execute_batch("DROP TRIGGER refuse_the_pass").unwrap();
    }
    w.write(&record(&[&a]), now).unwrap();
    assert_eq!(s.count("SELECT COUNT(*) FROM recall_events"), 2);
    assert!(!w.pass_is_due());
}

#[test]
fn recall_log_with_the_log_off_nothing_is_written() {
    let s = Scratch::new();
    let a = s.memory("a", MemoryType::Decision);
    let w = Writer::new(&s.path, false);
    let mut errors = Vec::new();
    w.record(&record(&[&a]), &mut errors);
    assert!(errors.is_empty());
    assert_eq!(s.count("SELECT COUNT(*) FROM recall_events"), 0);
    assert!(!w.enabled());
}

#[test]
fn recall_log_a_record_that_cannot_be_written_is_dropped_with_one_line() {
    let s = Scratch::new();
    let a = s.memory("a", MemoryType::Decision);
    let w = s.writer();
    // Another connection holds the write lock longer than the writer waits.
    let holder = rusqlite::Connection::open(&s.path).unwrap();
    holder.execute_batch("BEGIN IMMEDIATE").unwrap();
    let mut errors = Vec::new();
    let started = Instant::now();
    w.record(&record(&[&a]), &mut errors);
    let waited = started.elapsed();
    holder.execute_batch("ROLLBACK").unwrap();
    assert_eq!(
        String::from_utf8(errors).unwrap(),
        "mnemonic: a record of memory_search was dropped\n"
    );
    assert!(
        waited >= LOCK_WAIT && waited < Duration::from_secs(5),
        "{waited:?}"
    );
    assert_eq!(s.count("SELECT COUNT(*) FROM recall_events"), 0);
    // Written once the lock is free.
    let mut errors = Vec::new();
    w.record(&record(&[&a]), &mut errors);
    assert!(errors.is_empty());
    assert_eq!(s.count("SELECT COUNT(*) FROM recall_events"), 1);
}

#[test]
fn recall_log_the_name_of_a_client_is_an_identity() {
    let s = Scratch::new();
    let credential = ["sk-", "proj-", &"a1B2c3D4e5F6".repeat(4)].concat();
    // As it arrived.
    let w = s.writer();
    w.name_client(&credential);
    assert_eq!(w.client(), None);
    // In lower case, when the shape is reached by lowering.
    let w = s.writer();
    w.name_client(&credential.to_uppercase());
    assert_eq!(w.client(), None);
    // After the cut: a long name whose first 64 characters are a credential.
    let w = s.writer();
    w.name_client(&format!(
        "{credential} and a long tail of letters that carries it past the cut"
    ));
    assert_eq!(w.client(), None);
    // An empty name, and a name of spaces, are no name.
    let w = s.writer();
    w.name_client("");
    assert_eq!(w.client(), None);
    // An accepted name is stored in lower case and cut to 64 characters.
    let w = s.writer();
    let long = format!("Claude Code {}", "x".repeat(100));
    w.name_client(&long);
    let stored = w.client().unwrap();
    assert_eq!(stored.chars().count(), CLIENT_NAME_CHARS);
    assert!(stored.starts_with("claude code x"));
    // The first accepted name stays.
    w.name_client("Another Client");
    assert_eq!(w.client().unwrap(), stored);
    // A refused first name does not take the place of a later one.
    let w = s.writer();
    w.name_client(&credential);
    w.name_client("Codex");
    assert_eq!(w.client().as_deref(), Some("codex"));
}

#[test]
fn recall_log_an_id_that_reads_as_a_credential_is_refused_before_anything_is_written() {
    let s = Scratch::new();
    let w = s.writer();
    let credential = ["sk-", "proj-", &"a1B2c3D4e5F6".repeat(4)].concat();
    assert!(w.write(&record(&[&credential]), Utc::now()).is_err());
    assert_eq!(s.count("SELECT COUNT(*) FROM recall_events"), 0);
}

#[test]
fn recall_log_the_words_of_the_columns_are_fixed() {
    assert_eq!(Surface::Mcp.as_str(), "mcp");
    assert_eq!(Surface::Context.as_str(), "context");
    assert_eq!(
        [
            Tool::MemorySearch,
            Tool::MemorySimilar,
            Tool::MemoryRecent,
            Tool::MemoryGraph,
            Tool::MemoryContext,
            Tool::MemoryFacts,
            Tool::Context,
        ]
        .map(Tool::as_str),
        [
            "memory_search",
            "memory_similar",
            "memory_recent",
            "memory_graph",
            "memory_context",
            "memory_facts",
            "context",
        ]
    );
    for (word, section) in [
        ("digest", Section::Digest),
        ("feedback", Section::Feedback),
        ("decision", Section::Decisions),
        ("decisions", Section::Decisions),
        ("recent", Section::Recent),
        ("topic", Section::Topic),
    ] {
        assert_eq!(Section::of(word), Some(section));
    }
    assert_eq!(Section::of("other"), None);
    assert_eq!(Section::Decisions.as_str(), "decisions");
}
