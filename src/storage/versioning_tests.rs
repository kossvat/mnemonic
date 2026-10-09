use super::*;
use crate::storage::Storage;

/// A store as a pre-generation binary left it: real schema, generation 0,
/// and a raw transcript payload the snapshot must not carry.
fn legacy_store(dir: &Path) -> PathBuf {
    let path = dir.join("memory.db");
    drop(Storage::open(&path).unwrap());
    let conn = Connection::open(&path).unwrap();
    conn.pragma_update(None, "user_version", 0).unwrap();
    conn.execute(
        "INSERT INTO ingest_events (source_key, source_at, observed_at, schema_version, payload)
         VALUES ('demo/1', NULL, '2026-09-23T00:00:00Z', 1, '{\"raw\":\"secret turn\"}')",
        [],
    )
    .unwrap();
    let _ = std::fs::remove_file(snapshot_path(&path));
    path
}

fn temporaries(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|name| name.contains(".bak.tmp."))
        .collect()
}

#[test]
fn fresh_db_gets_generation_without_snapshot() {
    let dir = crate::test_support::temp_dir("mnemonic-gen-fresh-");
    let path = dir.path().join("memory.db");
    let storage = Storage::open(&path).unwrap();
    assert_eq!(
        generation(&storage.conn.lock().unwrap()).unwrap(),
        MEMORY_SCHEMA_GENERATION
    );
    assert!(!snapshot_path(&path).exists());
}

#[test]
fn legacy_db_snapshot_taken_once_scrubbed_and_0600() {
    let dir = crate::test_support::temp_dir("mnemonic-gen-legacy-");
    let path = legacy_store(dir.path());
    let storage = Storage::open(&path).unwrap();
    assert_eq!(
        generation(&storage.conn.lock().unwrap()).unwrap(),
        MEMORY_SCHEMA_GENERATION
    );
    let snapshot = snapshot_path(&path);
    let copy = Connection::open(&snapshot).unwrap();
    // The snapshot is the store as it was, without raw transcript text.
    assert_eq!(generation(&copy).unwrap(), 0);
    let payloads: i64 = copy
        .query_row(
            "SELECT COUNT(*) FROM ingest_events WHERE payload IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(payloads, 0);
    drop(copy);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&snapshot).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    // Opening again takes no second snapshot.
    let before = std::fs::metadata(&snapshot).unwrap().modified().unwrap();
    drop(storage);
    drop(Storage::open(&path).unwrap());
    assert_eq!(
        std::fs::metadata(&snapshot).unwrap().modified().unwrap(),
        before
    );
    assert!(temporaries(dir.path()).is_empty());
}

#[test]
fn concurrent_openers_take_one_snapshot() {
    let dir = crate::test_support::temp_dir("mnemonic-gen-race-");
    let path = legacy_store(dir.path());
    let handles: Vec<_> = (0..4)
        .map(|_| {
            let path = path.clone();
            std::thread::spawn(move || drop(Storage::open(&path).unwrap()))
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }
    let snapshots = std::fs::read_dir(dir.path())
        .unwrap()
        .flatten()
        .filter(|e| {
            e.file_name().to_string_lossy().ends_with(".bak")
                && e.file_name().to_string_lossy().contains(".pre-v")
        })
        .count();
    assert_eq!(snapshots, 1);
    assert!(temporaries(dir.path()).is_empty());
}

#[test]
fn vacuum_into_under_foreign_write_lock_on_bundled_sqlite() {
    let dir = crate::test_support::temp_dir("mnemonic-gen-vacuum-");
    let path = dir.path().join("memory.db");
    drop(Storage::open(&path).unwrap());
    let holder = Connection::open(&path).unwrap();
    holder.execute_batch("BEGIN IMMEDIATE").unwrap();
    let copy = dir.path().join("copy.db");
    copy_scrubbed(&path, &copy).unwrap();
    assert!(copy.exists());
    holder.execute_batch("COMMIT").unwrap();
}

#[test]
fn stale_tmp_does_not_block_upgrade() {
    let dir = crate::test_support::temp_dir("mnemonic-gen-stale-");
    let path = legacy_store(dir.path());
    let stale = dir.path().join(format!(
        "memory.db.pre-v{MEMORY_SCHEMA_GENERATION}.bak.tmp.1.1"
    ));
    std::fs::write(&stale, b"torn").unwrap();
    let old = SystemTime::now() - Duration::from_secs(2 * 3600);
    std::fs::File::options()
        .write(true)
        .open(&stale)
        .unwrap()
        .set_modified(old)
        .unwrap();
    drop(Storage::open(&path).unwrap());
    assert!(!stale.exists());
    assert!(snapshot_path(&path).exists());
}

#[test]
fn newer_generation_is_refused() {
    let dir = crate::test_support::temp_dir("mnemonic-gen-newer-");
    let path = dir.path().join("memory.db");
    drop(Storage::open(&path).unwrap());
    Connection::open(&path)
        .unwrap()
        .pragma_update(None, "user_version", MEMORY_SCHEMA_GENERATION + 1)
        .unwrap();
    let refused = Storage::open(&path).err().unwrap().to_string();
    assert!(refused.contains("newer than this binary"), "{refused}");
}

#[test]
fn snapshot_failure_keeps_open_working_and_retries() {
    let dir = crate::test_support::temp_dir("mnemonic-gen-fail-");
    let path = legacy_store(dir.path());
    FAIL_NEXT_SNAPSHOT.with(|f| f.set(true));
    let storage = Storage::open(&path).unwrap();
    // Open, usable, but not moved on without its way back.
    assert_eq!(generation(&storage.conn.lock().unwrap()).unwrap(), 0);
    assert!(!snapshot_path(&path).exists());
    drop(storage);
    let storage = Storage::open(&path).unwrap();
    assert_eq!(
        generation(&storage.conn.lock().unwrap()).unwrap(),
        MEMORY_SCHEMA_GENERATION
    );
    assert!(snapshot_path(&path).exists());
}

#[test]
fn the_snapshot_is_copied_inside_an_owner_only_directory() {
    let dir = crate::test_support::temp_dir("mnemonic-gen-private-");
    let private = dir.path().join("private");
    private_dir(&private).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&private).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
    }
    // A stale temporary directory is cleared like a stale file.
    let path = legacy_store(dir.path());
    let stale = dir.path().join(format!(
        "memory.db.pre-v{MEMORY_SCHEMA_GENERATION}.bak.tmp.1.2"
    ));
    private_dir(&stale).unwrap();
    std::fs::write(stale.join("snapshot.db"), b"torn").unwrap();
    let old = SystemTime::now() - Duration::from_secs(2 * 3600);
    filetime_dir(&stale, old).unwrap();
    drop(Storage::open(&path).unwrap());
    assert!(!stale.exists());
    assert!(temporaries(dir.path()).is_empty());
}

/// Set a directory's modification time (std can do it through a handle).
fn filetime_dir(path: &Path, when: SystemTime) -> std::io::Result<()> {
    std::fs::File::open(path)?.set_modified(when)
}

/// The tables this binary's generation brought: a store of the earlier
/// one has none of them.
const SINCE_THE_EARLIER_GENERATION: [&str; 9] = [
    "attribution_state",
    "attribution_overrides",
    "segment_activity",
    "work_segments",
    "owner_prompts",
    "observation_checkpoints",
    "attribution_dirty",
    "project_aliases",
    "projects",
];

/// A populated store as the binary of the earlier generation left it.
fn earlier_store(dir: &Path) -> PathBuf {
    let path = dir.join("memory.db");
    drop(Storage::open(&path).unwrap());
    let conn = Connection::open(&path).unwrap();
    for table in SINCE_THE_EARLIER_GENERATION {
        conn.execute_batch(&format!("DROP TABLE {table}")).unwrap();
    }
    conn.execute_batch(
        "INSERT INTO memories (id, timestamp, title, content, memory_type, source)
         VALUES ('m1', '2026-09-20T10:00:00+00:00', 'a note', 'kept as it is', 'note', 'test'),
                ('m2', '2026-09-21T10:00:00+00:00', 'a decision', 'kept too', 'decision', 'test');
         INSERT INTO entities (id, name, entity_type) VALUES ('e1', 'demoapp', 'project');
         INSERT INTO memory_entities (memory_id, entity_id) VALUES ('m1', 'e1');
         INSERT INTO work_stretches (transcript, started_at, last_at)
         VALUES ('t', '2026-09-20 10:00:00', '2026-09-20 10:05:00');",
    )
    .unwrap();
    conn.pragma_update(None, "user_version", MEMORY_SCHEMA_GENERATION - 1)
        .unwrap();
    let _ = std::fs::remove_file(snapshot_path(&path));
    path
}

/// Every row of every table of a store, by table, as text.
fn every_row(path: &Path) -> std::collections::BTreeMap<String, Vec<String>> {
    let conn = Connection::open(path).unwrap();
    let tables = crate::test_support::rows(
        &conn,
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'
            AND sql NOT LIKE 'CREATE VIRTUAL%' AND name NOT LIKE '%_fts%'
          ORDER BY name",
    );
    tables
        .into_iter()
        .map(|table| {
            let rows = crate::test_support::rows(&conn, &format!("SELECT * FROM {table}"));
            (table, rows)
        })
        .collect()
}

#[test]
fn migration_over_a_populated_store_keeps_every_row_and_raises_the_generation() {
    let dir = crate::test_support::temp_dir("mnemonic-gen-migrate-");
    let path = earlier_store(dir.path());
    let before = every_row(&path);
    assert!(before["memories"].len() == 2 && !before.contains_key("projects"));

    let storage = Storage::open(&path).unwrap();
    assert_eq!(
        generation(&storage.conn.lock().unwrap()).unwrap(),
        MEMORY_SCHEMA_GENERATION
    );
    storage.require_current_generation().unwrap();
    drop(storage);
    // The way back was taken first, as the store was.
    let snapshot = Connection::open(snapshot_path(&path)).unwrap();
    assert_eq!(generation(&snapshot).unwrap(), MEMORY_SCHEMA_GENERATION - 1);
    drop(snapshot);
    let after = every_row(&path);
    for (table, rows) in &before {
        assert_eq!(&after[table], rows, "{table} changed");
    }
    for table in SINCE_THE_EARLIER_GENERATION {
        assert!(after[table].is_empty(), "{table} was not made empty");
    }
}

#[test]
fn migration_interrupted_leaves_the_store_usable_and_lets_no_writer_start() {
    let dir = crate::test_support::temp_dir("mnemonic-gen-interrupted-");
    let path = earlier_store(dir.path());
    let before = every_row(&path);
    FAIL_NEXT_SNAPSHOT.with(|f| f.set(true));
    let storage = Storage::open(&path).unwrap();
    // The store is open and answers, at the generation it had.
    assert_eq!(
        generation(&storage.conn.lock().unwrap()).unwrap(),
        MEMORY_SCHEMA_GENERATION - 1
    );
    assert_eq!(storage.count().unwrap(), 2);
    // The daemon asks this before it starts anything, and stops.
    let refused = storage
        .require_current_generation()
        .err()
        .unwrap()
        .to_string();
    assert!(refused.contains("Nothing was written"), "{refused}");
    drop(storage);
    // What it held is as it was: a binary of the earlier generation
    // still opens it, and finds its rows.
    refuse_newer_than(
        &Connection::open(&path).unwrap(),
        MEMORY_SCHEMA_GENERATION - 1,
    )
    .unwrap();
    let after = every_row(&path);
    for (table, rows) in &before {
        assert_eq!(&after[table], rows, "{table} changed");
    }

    // The next open finishes the migration.
    let storage = Storage::open(&path).unwrap();
    storage.require_current_generation().unwrap();
    assert!(snapshot_path(&path).exists());
}

#[test]
fn migration_a_binary_of_the_earlier_generation_refuses_the_migrated_store() {
    let dir = crate::test_support::temp_dir("mnemonic-gen-refuse-");
    let path = earlier_store(dir.path());
    drop(Storage::open(&path).unwrap());
    let conn = Connection::open(&path).unwrap();
    refuse_newer_than(&conn, MEMORY_SCHEMA_GENERATION).unwrap();
    let refused = refuse_newer_than(&conn, MEMORY_SCHEMA_GENERATION - 1)
        .err()
        .unwrap()
        .to_string();
    assert!(refused.contains("newer than this binary"), "{refused}");
}
