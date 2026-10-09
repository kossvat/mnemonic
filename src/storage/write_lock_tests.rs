//! Writers that read before they write take the write lock from the start.
//! Another process (a backfill, a memory server) can commit between the
//! read and the write; a lock taken to read is then refused when it is
//! raised, with no wait, and what the caller asked for is lost.
use super::*;
use crate::graph::{Entity, EntityType};
use chrono::{TimeZone, Utc};

struct Store {
    dir: tempfile::TempDir,
    storage: Storage,
}

impl Store {
    fn new() -> Self {
        let dir = crate::test_support::temp_dir("mn-write-lock-");
        let storage = Storage::open(&dir.path().join("memory.db")).unwrap();
        Self { dir, storage }
    }

    /// A second connection that holds the write lock with a change of its
    /// own and commits it a moment later.
    fn another_writer(&self) -> std::thread::JoinHandle<()> {
        let other = rusqlite::Connection::open(self.dir.path().join("memory.db")).unwrap();
        other
            .execute_batch(
                "BEGIN IMMEDIATE;
                 INSERT INTO work_stretches (transcript, started_at, last_at)
                 VALUES ('/sessions/other.jsonl', '2026-09-20 09:00:00', '2026-09-20 09:05:00');",
            )
            .unwrap();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(300));
            other.execute_batch("COMMIT").unwrap();
        })
    }
}

#[test]
fn write_lock_the_registry_waits_for_another_writer() {
    let s = Store::new();
    // A new project.
    let writer = s.another_writer();
    let demo = s
        .storage
        .project_upsert("demoapp", "demoapp", None)
        .unwrap();
    writer.join().unwrap();
    assert!(s.storage.project_by_key("demoapp").unwrap().is_some());

    // Its entity becomes a project.
    s.storage
        .upsert_entity(&Entity {
            name: "demoapp".into(),
            entity_type: EntityType::Concept,
        })
        .unwrap();
    let writer = s.another_writer();
    assert!(s.storage.project_link_entity(demo, "demoapp").unwrap());
    writer.join().unwrap();

    // A merge.
    let notes = s.storage.project_upsert("notes", "notes", None).unwrap();
    let writer = s.another_writer();
    s.storage.project_merge(notes, demo).unwrap();
    writer.join().unwrap();
    assert_eq!(s.storage.project_canonical(notes).unwrap(), demo);
}

#[test]
fn write_lock_a_correction_waits_for_another_writer() {
    let s = Store::new();
    let demo = s
        .storage
        .project_upsert("demoapp", "demoapp", None)
        .unwrap();
    let from = Utc.with_ymd_and_hms(2026, 9, 30, 10, 0, 0).unwrap();
    let range = Some((from, from + chrono::Duration::hours(1)));

    let writer = s.another_writer();
    let id = s.storage.override_add(None, range, Some(demo)).unwrap();
    writer.join().unwrap();
    assert_eq!(s.storage.overrides().unwrap().len(), 1);

    let writer = s.another_writer();
    assert!(s.storage.override_remove(id).unwrap());
    writer.join().unwrap();
    assert!(s.storage.overrides().unwrap().is_empty());
}

/// The stamp a periodic reader compares before it counts: it stays while
/// nothing is written and moves for a write of this connection and for a
/// commit of another.
#[test]
fn write_lock_the_change_stamp_moves_only_when_the_store_is_written() {
    let s = Store::new();
    let first = s.storage.change_stamp().unwrap();
    assert_eq!(s.storage.change_stamp().unwrap(), first);

    s.storage
        .project_upsert("demoapp", "demoapp", None)
        .unwrap();
    let own = s.storage.change_stamp().unwrap();
    assert_ne!(own, first);
    assert_eq!(s.storage.change_stamp().unwrap(), own);

    s.another_writer().join().unwrap();
    assert_ne!(s.storage.change_stamp().unwrap(), own);
}
