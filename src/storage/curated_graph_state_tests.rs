use super::*;

fn graph_rows(storage: &Storage) -> (Vec<String>, Vec<String>) {
    let conn = storage.conn.lock().unwrap();
    let mut entities=conn.prepare("SELECT id||'|'||name||'|'||entity_type||'|'||mention_count||'|'||first_seen||'|'||last_seen FROM entities ORDER BY id").unwrap();
    let entities = entities
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    let mut edges=conn.prepare("SELECT id||'|'||source_entity||'|'||target_entity||'|'||relation||'|'||COALESCE(memory_id,'')||'|'||weight||'|'||timestamp FROM edges ORDER BY id").unwrap();
    let edges = edges
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    (entities, edges)
}

#[test]
fn curated_graph_archive_is_reversible_and_never_changes_an_entity_or_link() {
    let f = Fixture::new();
    f.project("alpha", None);
    f.entity("orphan", "outside-project", "project");
    f.entity("concept", "concept-only", "concept");
    f.memory("m1", 100, &["alpha", "orphan"]);
    f.storage
        .conn
        .lock()
        .unwrap()
        .execute(
            "INSERT INTO edges(id,source_entity,target_entity,relation,memory_id)
        VALUES ('edge','alpha','orphan','mentions','m1')",
            [],
        )
        .unwrap();
    let before = graph_rows(&f.storage);
    assert_eq!(f.storage.entity_archive_shelf(false).unwrap().len(), 1);
    assert!(f.storage.entity_archive_set("orphan", true, 200).unwrap());
    assert!(!f.storage.entity_archive_set("orphan", true, 300).unwrap());
    assert!(f.storage.entity_archive_shelf(false).unwrap().is_empty());
    assert_eq!(
        f.storage.entity_archive_shelf(true).unwrap()[0].archived_at,
        Some(200)
    );
    assert_eq!(graph_rows(&f.storage), before);
    assert!(f.storage.entity_archive_set("orphan", false, 400).unwrap());
    assert_eq!(f.storage.entity_archive_shelf(false).unwrap().len(), 1);
    assert_eq!(graph_rows(&f.storage), before);
    for id in ["alpha", "concept", "missing"] {
        assert!(f.storage.entity_archive_set(id, true, 500).is_err());
    }
    assert!(!f.storage.entity_archive_set("missing", false, 500).unwrap());
}

#[test]
fn curated_graph_archive_stays_effective_after_a_later_registry_link_until_undo() {
    let f = Fixture::new();
    f.entity("old", "later-project", "project");
    f.memory("m1", 100, &["old"]);
    f.storage.entity_archive_set("old", true, 200).unwrap();
    let id = f
        .storage
        .project_upsert("later-project", "later-project", None)
        .unwrap();
    f.storage.project_link_entity(id, "later-project").unwrap();
    assert_eq!(
        f.storage.curated_graph_evidence(&[], &[]).unwrap().projects[0].memory_count,
        0
    );
    assert_eq!(f.storage.entity_archive_shelf(true).unwrap().len(), 1);
    f.storage.entity_archive_set("old", false, 300).unwrap();
    assert_eq!(
        f.storage.curated_graph_evidence(&[], &[]).unwrap().projects[0].memory_count,
        1
    );
}

#[test]
fn curated_graph_archive_does_not_block_an_existing_entity_merge() {
    let f = Fixture::new();
    f.entity("canonical", "canonical-project", "project");
    f.entity("old", "old-project", "project");
    f.memory("m1", 100, &["old"]);
    f.storage.entity_archive_set("old", true, 200).unwrap();
    f.storage
        .merge_entities("canonical-project", "old-project")
        .unwrap();
    let shelf = f.storage.entity_archive_shelf(true).unwrap();
    assert_eq!(shelf.len(), 1);
    assert_eq!(shelf[0].id, "canonical");
    assert_eq!(shelf[0].archived_at, None);
    // Marks name exact historical IDs; merging never transfers an archive
    // choice to the canonical entity. Orphan marks can still be undone.
    assert!(f.storage.entity_archive_set("old", false, 300).unwrap());
}

#[test]
fn curated_graph_archive_does_not_block_an_existing_graph_rebuild() {
    let f = Fixture::new();
    f.entity("old", "old-project", "project");
    f.memory("m1", 100, &["old"]);
    f.storage.entity_archive_set("old", true, 200).unwrap();
    assert_eq!(f.storage.clear_graph().unwrap(), 1);
    assert!(f.storage.entity_archive_shelf(true).unwrap().is_empty());
    // No entity means no candidate; the exact-ID mark remains independent.
    assert!(f.storage.entity_archive_set("old", false, 300).unwrap());
}

#[test]
fn curated_graph_slots_keep_gaps_and_parent_namespaces_with_collision_fallback() {
    let f = Fixture::new();
    f.storage
        .ensure_graph_slots(&[
            slot("project:1", None, Some(5)),
            slot("project:2", None, Some(5)),
            slot("unit:1:task", Some("project:1"), None),
            slot("unit:2:task", Some("project:2"), None),
        ])
        .unwrap();
    let before = f.storage.graph_slots().unwrap();
    assert_eq!(
        before
            .iter()
            .find(|slot| slot.key == "project:1")
            .unwrap()
            .slot,
        5
    );
    assert_eq!(
        before
            .iter()
            .find(|slot| slot.key == "project:2")
            .unwrap()
            .slot,
        6
    );
    for key in ["unit:1:task", "unit:2:task"] {
        assert_eq!(before.iter().find(|slot| slot.key == key).unwrap().slot, 0);
    }
    let changes = f.storage.conn.lock().unwrap().total_changes();
    f.storage
        .ensure_graph_slots(&[slot("project:1", Some("project:2"), Some(99))])
        .unwrap();
    assert_eq!(f.storage.graph_slots().unwrap(), before);
    assert_eq!(f.storage.conn.lock().unwrap().total_changes(), changes);
    f.storage
        .ensure_graph_slots(&[slot("project:3", None, None)])
        .unwrap();
    assert_eq!(
        f.storage
            .graph_slots()
            .unwrap()
            .iter()
            .find(|slot| slot.key == "project:3")
            .unwrap()
            .slot,
        7
    );
    let snapshot = f.storage.graph_slots().unwrap();
    assert!(
        f.storage
            .ensure_graph_slots(&[
                slot("project:4", None, None),
                slot("path/forbidden", None, None)
            ])
            .is_err()
    );
    assert_eq!(
        f.storage.graph_slots().unwrap(),
        snapshot,
        "invalid batch allocates no partial slot"
    );
}
