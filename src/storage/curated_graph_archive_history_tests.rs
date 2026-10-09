use super::*;

struct Fixture {
    root: tempfile::TempDir,
    storage: Storage,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let storage = Storage::open(&root.path().join("memory.db")).unwrap();
        Self { root, storage }
    }

    fn entity(&self, id: &str, name: &str) {
        self.storage
            .conn
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO entities(id,name,entity_type) VALUES(?1,?2,'project')",
                params![id, name],
            )
            .unwrap();
    }

    fn memory(&self, id: &str, entity: &str, at: i64, kind: &str, superseded: Option<&str>) {
        let conn = self.storage.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO memories(id,timestamp,title,content,memory_type,source,superseded_by)
             VALUES(?1,datetime(?2,'unixepoch'),'Fixture note','Fixture content',?3,'test',?4)",
            params![id, at, kind, superseded],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO memory_entities(memory_id,entity_id) VALUES(?1,?2)",
            params![id, entity],
        )
        .unwrap();
    }

    fn files(&self) -> [Option<Vec<u8>>; 2] {
        ["memory.db", "memory.db-wal"].map(|name| std::fs::read(self.root.path().join(name)).ok())
    }
}

#[test]
fn curated_graph_archive_history_preserves_orphan_marks_without_archiving_the_survivor() {
    let f = Fixture::new();
    f.entity("old", "old-project");
    f.entity("survivor", "surviving-project");
    f.memory("shared-note", "old", 100, "decision", None);
    assert!(f.storage.entity_archive_set("old", true, 200).unwrap());
    let before = f.storage.entity_archive_history().unwrap();
    assert_eq!(before.len(), 1);
    assert_eq!(before[0].name, "old-project");
    assert_eq!(before[0].memory_count, 1);
    assert_eq!(before[0].last_memory, Some(100));
    f.storage
        .merge_entities("surviving-project", "old-project")
        .unwrap();
    assert!(f.storage.entity_name("old").unwrap().is_none());
    assert_eq!(f.storage.entity_archive_at("old").unwrap(), Some(200));
    assert_eq!(f.storage.entity_archive_at("survivor").unwrap(), None);
    let (files, changes) = (f.files(), f.storage.conn.lock().unwrap().total_changes());
    let rows = f.storage.entity_archive_history().unwrap();
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row.id, "old");
    assert_eq!(row.name, "Archived entity");
    assert_eq!(row.archived_at, Some(200));
    assert_eq!(row.memory_count, 0);
    assert_eq!(row.last_memory, None);
    assert_eq!(f.files(), files);
    assert_eq!(f.storage.conn.lock().unwrap().total_changes(), changes);
    let cached = (vec![("old".into(), 60.0, Some("low".into()))], 0.0);
    assert_eq!(
        f.storage
            .rekeyed_day_with_unattributed(cached.clone())
            .unwrap(),
        (Vec::new(), 60.0)
    );
    assert!(f.storage.entity_archive_set("old", false, 300).unwrap());
    assert!(f.storage.entity_archive_history().unwrap().is_empty());
    assert_eq!(
        f.storage
            .rekeyed_day_with_unattributed(cached.clone())
            .unwrap(),
        cached
    );
    assert!(f.storage.entity_name("survivor").unwrap().is_some());
}

#[test]
fn curated_graph_archive_history_survives_graph_clear_and_exact_id_undo() {
    let f = Fixture::new();
    for (id, at) in [("first", 200), ("second", 300)] {
        f.entity(id, id);
        f.storage.entity_archive_set(id, true, at).unwrap();
    }
    assert_eq!(f.storage.clear_graph().unwrap(), 2);
    let rows = f.storage.entity_archive_history().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].id, "first");
    assert_eq!(rows[1].id, "second");
    assert!(rows.iter().all(|row| row.name == "Archived entity"));
    assert!(f.storage.entity_archive_set("first", false, 400).unwrap());
    assert!(!f.storage.entity_archive_set("first", false, 500).unwrap());
    let rows = f.storage.entity_archive_history().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, "second");
    assert_eq!(rows[0].archived_at, Some(300));
}

#[test]
fn curated_graph_archive_history_redacts_labels_and_counts_only_admissible_current_memories() {
    let f = Fixture::new();
    let secret = format!("{}{}", "ghp_", "x".repeat(36));
    f.entity("shown", &format!("Fixture {secret}"));
    f.memory("current", "shown", 100, "decision", None);
    f.memory("summary", "shown", 200, "session_summary", None);
    f.memory("superseded", "shown", 300, "decision", Some("current"));
    f.memory(&secret, "shown", 400, "decision", None);
    f.storage.entity_archive_set("shown", true, 500).unwrap();
    f.storage
        .conn
        .lock()
        .unwrap()
        .execute(
            "INSERT INTO entity_archive(entity_id,at) VALUES(?1,600)",
            [&secret],
        )
        .unwrap();
    let (files, changes) = (f.files(), f.storage.conn.lock().unwrap().total_changes());
    let rows = f.storage.entity_archive_history().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, "shown");
    assert!(!rows[0].name.contains(&secret));
    assert_eq!(rows[0].memory_count, 1);
    assert_eq!(rows[0].last_memory, Some(100));
    assert!(f.storage.entity_archive_at(&secret).is_err());
    assert_eq!(f.storage.entity_archive_at("missing").unwrap(), None);
    assert_eq!(f.files(), files);
    assert_eq!(f.storage.conn.lock().unwrap().total_changes(), changes);
}
