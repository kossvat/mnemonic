//! The backup the runbook takes before an installation: whole, checked,
//! and taken without changing the stores.
use super::*;
use crate::activity::ActivityStore;
use crate::storage::Storage;

fn fixture() -> (tempfile::TempDir, Config) {
    let dir = crate::test_support::temp_dir("mn-backup-");
    let mut config = Config::default();
    config.storage.db_path = dir.path().join("store/memory.db");
    std::fs::create_dir_all(dir.path().join("store")).unwrap();
    (dir, config)
}

/// A store of the generation before this one, with a memory in it.
fn earlier_store(config: &Config) {
    drop(Storage::open(&config.storage.db_path).unwrap());
    let conn = Connection::open(&config.storage.db_path).unwrap();
    conn.execute_batch(
        "INSERT INTO memories (id, timestamp, title, content, memory_type, source)
         VALUES ('m1', '2026-09-20T10:00:00+00:00', 'a note', 'kept as it is', 'note', 'test');
         PRAGMA user_version = 2;",
    )
    .unwrap();
}

fn generation(path: &Path) -> i64 {
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .unwrap()
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap()
}

#[test]
fn backfill_the_backup_is_whole_and_moves_the_store_to_no_generation() {
    let (dir, config) = fixture();
    earlier_store(&config);
    drop(ActivityStore::open(&config.activity_db_path()).unwrap());
    let into = dir.path().join("backups/before");
    let copied = backup(&config, &into).unwrap();
    let names: Vec<_> = copied
        .iter()
        .map(|copy| {
            copy.file
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert_eq!(names, ["memory.db", "activity.db"]);
    assert!(copied.iter().all(|copy| copy.bytes > 0));

    // The store was copied as it is: of the generation it had, and it
    // still has that generation.
    assert_eq!(generation(&config.storage.db_path), 2);
    assert_eq!(generation(&into.join("memory.db")), 2);
    let copy = Connection::open(into.join("memory.db")).unwrap();
    let kept: String = copy
        .query_row("SELECT content FROM memories WHERE id = 'm1'", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(kept, "kept as it is");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&into), 0o700);
        assert_eq!(mode(&into.join("memory.db")), 0o600);
    }

    // The way back: the copy opens as a store of this binary.
    let restored = dir.path().join("restored");
    std::fs::create_dir_all(&restored).unwrap();
    std::fs::copy(into.join("memory.db"), restored.join("memory.db")).unwrap();
    assert_eq!(
        Storage::open(&restored.join("memory.db"))
            .unwrap()
            .count()
            .unwrap(),
        1
    );
}

#[test]
fn backfill_the_backup_never_writes_over_a_copy_and_says_what_is_missing() {
    let (dir, config) = fixture();
    // No store: nothing is made, and no empty store appears.
    let into = dir.path().join("backups/first");
    assert!(backup(&config, &into).is_err());
    assert!(!config.storage.db_path.exists());

    earlier_store(&config);
    // No activity store: the memory store alone is copied.
    assert_eq!(backup(&config, &into).unwrap().len(), 1);
    let before = std::fs::read(into.join("memory.db")).unwrap();
    // A second backup into the same folder is refused.
    let again = backup(&config, &into).err().unwrap().to_string();
    assert!(again.contains("never written over"), "{again}");
    assert_eq!(std::fs::read(into.join("memory.db")).unwrap(), before);
}

#[test]
fn backfill_the_backup_is_taken_while_the_store_is_written() {
    let (dir, config) = fixture();
    let storage = Storage::open(&config.storage.db_path).unwrap();
    let conn = storage.conn.lock().unwrap();
    conn.execute_batch(
        "INSERT INTO memories (id, timestamp, title, content, memory_type, source)
         VALUES ('m1', '2026-09-20T10:00:00+00:00', 'a note', 'in the journal', 'note', 't');",
    )
    .unwrap();
    // The daemon holds the store open, with rows still in its journal.
    let copied = backup(&config, &dir.path().join("b")).unwrap();
    let copy = Connection::open(&copied[0].file).unwrap();
    let rows: i64 = copy
        .query_row("SELECT COUNT(*) FROM memories", [], |row| row.get(0))
        .unwrap();
    assert_eq!(rows, 1);
    drop(conn);
}

#[test]
fn backfill_the_backup_into_a_folder_others_can_enter_stays_the_owners() {
    let (dir, config) = fixture();
    earlier_store(&config);
    // The folder is there already, open to others.
    let into = dir.path().join("shared");
    std::fs::create_dir_all(&into).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&into, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    backup(&config, &into).unwrap();
    // The copy was written in a folder of the run and has its name only
    // as a file the owner alone can read; that folder is gone.
    let names: Vec<String> = std::fs::read_dir(&into)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["memory.db"]);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(into.join("memory.db"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}

#[test]
fn backfill_a_copy_that_comes_second_removes_nothing_of_the_first() {
    let (dir, config) = fixture();
    earlier_store(&config);
    let into = dir.path().join("b");
    std::fs::create_dir_all(&into).unwrap();
    // Another run finished its copy between this run's look and its
    // publishing.
    let target = into.join("memory.db");
    let staging = into.join(".staging-test");
    private_dir(&staging).unwrap();
    let staged = staging.join("memory.db");
    std::fs::write(&staged, b"this run's copy").unwrap();
    std::fs::write(&target, b"the first run's copy").unwrap();
    let refused = publish(&staged, &target).err().unwrap().to_string();
    assert!(refused.contains("never written over"), "{refused}");
    assert_eq!(std::fs::read(&target).unwrap(), b"the first run's copy");
    // And a whole run that finds the name taken leaves it as it is, and
    // leaves no folder of its own behind.
    std::fs::remove_dir_all(&staging).unwrap();
    assert!(backup(&config, &into).is_err());
    assert_eq!(std::fs::read(&target).unwrap(), b"the first run's copy");
    assert_eq!(std::fs::read_dir(&into).unwrap().count(), 1);
}
