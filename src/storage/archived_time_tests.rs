//! Archive is reversible state; graph history and cached seconds survive it.
use super::*;
use crate::activity::{ActivityStore, ProjectTimeData, ProjectTimeRow};
use crate::event::{EventSource, MemoryType};
use crate::graph::EntityType;
use crate::test_support::InTempDir;
use chrono::{DateTime, Duration, Utc};
use std::collections::BTreeSet;

struct Fixture {
    storage: Storage,
    activity: ActivityStore,
    legacy: String,
    real: String,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
}

fn fixture() -> InTempDir<Fixture> {
    InTempDir::new("mn-curated-archive-time-", |dir| {
        let storage = Storage::open(&dir.join("memory.db")).unwrap();
        let activity = ActivityStore::open(&dir.join("activity.db")).unwrap();
        let from = Utc::now() - Duration::minutes(20);
        let mut ids = Vec::new();
        for name in ["phantom-board", "harbor-board"] {
            let entity = Entity {
                name: name.into(),
                entity_type: EntityType::Project,
            };
            let id = storage.upsert_entity(&entity).unwrap();
            for minute in 1..=2 {
                let mut entry = MemoryEntry::new(
                    format!("{name} implementation {minute}"),
                    "The fixture implementation was checked.",
                    MemoryType::Note,
                    EventSource::Manual,
                );
                entry.timestamp = from + Duration::minutes(minute);
                storage
                    .save_with_embedding(&entry, Some(&vec![1.0, 0.0]))
                    .unwrap();
                storage.link_memory_entity(&entry.id, &id).unwrap();
            }
            ids.push(id);
        }
        let project = storage
            .project_upsert("harbor-board", "Harbor Board", None)
            .unwrap();
        storage
            .project_link_entity(project, "harbor-board")
            .unwrap();
        Fixture {
            storage,
            activity,
            legacy: ids[0].clone(),
            real: ids[1].clone(),
            from,
            to: from + Duration::minutes(10),
        }
    })
}

fn graph_rows(storage: &Storage) -> Vec<Vec<Vec<rusqlite::types::Value>>> {
    let conn = storage.conn.lock().unwrap();
    ["entities", "edges", "memory_entities"]
        .iter()
        .map(|table| {
            let mut stmt = conn
                .prepare(&format!("SELECT * FROM {table} ORDER BY rowid"))
                .unwrap();
            let count = stmt.column_count();
            stmt.query_map([], |row| (0..count).map(|i| row.get(i)).collect())
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap()
        })
        .collect()
}

fn pool_keys(storage: &Storage) -> BTreeSet<String> {
    storage
        .project_reference_pool(2)
        .unwrap()
        .into_iter()
        .map(|p| p.project_key)
        .collect()
}

#[test]
fn curated_graph_archive_removes_legacy_direct_carry_semantic_and_project_readers() {
    let f = fixture();
    let snapshot = graph_rows(&f.storage);
    let original = f
        .storage
        .project_signals_in_window(f.from, f.to, 2)
        .unwrap();
    let original_times = f
        .storage
        .project_mem_times_in_window(f.from, f.to, 2)
        .unwrap();
    let original_pool = pool_keys(&f.storage);
    assert_eq!(original.len(), 2);
    assert_eq!(original_times.len(), 4);
    assert_eq!(original_pool.len(), 2);
    assert!(f.storage.entity_archive_set(&f.legacy, true, 10).unwrap());
    let signals = f
        .storage
        .project_signals_in_window(f.from, f.to, 2)
        .unwrap();
    assert_eq!(signals.len(), 1);
    assert_eq!(signals[0].project_key, f.real);
    assert!(
        f.storage
            .project_mem_times_in_window(f.from, f.to, 2)
            .unwrap()
            .iter()
            .all(|(key, _)| key == &f.real)
    );
    assert_eq!(
        pool_keys(&f.storage),
        BTreeSet::from(["harbor-board".into()])
    );
    let memories = f
        .storage
        .window_memories_with_embeddings(f.from, f.to)
        .unwrap();
    assert_eq!(memories.len(), 4);
    assert!(
        memories
            .iter()
            .all(|m| !m.linked_projects.contains(&"phantom-board".into()))
    );
    let projects = f.storage.projects_overview(10, 5).unwrap();
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].key, f.real);
    assert!(
        f.storage
            .project_overview_by_id(&f.legacy, 5)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        f.storage
            .project_memories_in_window(f.from, f.to, 2)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(graph_rows(&f.storage), snapshot);
    assert!(f.storage.entity_archive_set(&f.legacy, false, 11).unwrap());
    assert_eq!(
        f.storage
            .project_signals_in_window(f.from, f.to, 2)
            .unwrap(),
        original
    );
    assert_eq!(
        f.storage
            .project_mem_times_in_window(f.from, f.to, 2)
            .unwrap(),
        original_times
    );
    assert_eq!(pool_keys(&f.storage), original_pool);
    assert_eq!(graph_rows(&f.storage), snapshot);
}

fn time_row(key: &str, today: f64, week: f64) -> ProjectTimeRow {
    ProjectTimeRow {
        project_key: key.into(),
        today_seconds: today,
        week_seconds: week,
        week: [week - today, 0.0, 0.0, 0.0, 0.0, 0.0, today],
        confidence: Some("high".into()),
    }
}

#[test]
fn curated_graph_archive_cached_time_is_unattributed_and_undo_restores_it() {
    let f = fixture();
    let time = ProjectTimeData {
        rows: vec![
            time_row(&f.legacy, 60.0, 300.0),
            time_row(&f.real, 180.0, 600.0),
        ],
        unattributed_today: 30.0,
        unattributed_week: 90.0,
    };
    f.storage.entity_archive_set(&f.legacy, true, 10).unwrap();
    let result = f.storage.rekeyed_time(&time).unwrap();
    assert_eq!(result.rows, vec![time.rows[1].clone()]);
    assert_eq!(result.unattributed_today, 90.0);
    assert_eq!(result.unattributed_week, 390.0);
    let payload = f.storage.projects_payload(10, 5, Some(&time)).unwrap();
    assert_eq!(payload["projects"].as_array().unwrap().len(), 1);
    assert_eq!(payload["unattributed"]["today_seconds"], 90);
    assert_eq!(payload["unattributed"]["week_seconds"], 390);
    let day = vec![
        (f.legacy.clone(), 60.0, Some("low".into())),
        (f.real.clone(), 180.0, Some("high".into())),
    ];
    let result = f
        .storage
        .rekeyed_day_with_unattributed((day.clone(), 30.0))
        .unwrap();
    assert_eq!(result, (vec![day[1].clone()], 90.0));
    assert_eq!(
        f.storage.rekeyed_day(day.clone()).unwrap(),
        vec![day[1].clone()]
    );
    f.storage.entity_archive_set(&f.legacy, false, 11).unwrap();
    let restored = f.storage.rekeyed_time(&time).unwrap();
    assert_eq!(restored.rows.len(), 2);
    assert_eq!(restored.unattributed_today, 30.0);
    assert_eq!(restored.unattributed_week, 90.0);
    let (rows, unattributed) = f
        .storage
        .rekeyed_day_with_unattributed((day, 30.0))
        .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(unattributed, 30.0);
}

#[test]
fn curated_graph_archive_journal_conserves_cached_day_and_undo_is_immediate() {
    let f = fixture();
    let day = f.from.with_timezone(&chrono::Local).date_naive();
    f.activity
        .write_day(
            day,
            &[(&f.legacy, 1200.0, "low"), (&f.real, 2400.0, "high")],
            300.0,
        )
        .unwrap();
    let stored = f.activity.project_attribution_for_day(day).unwrap();
    f.storage.entity_archive_set(&f.legacy, true, 10).unwrap();
    let archived = crate::journal::collect(&f.storage, &f.activity, day).unwrap();
    assert!(archived.projects.iter().all(|p| p.key != f.legacy));
    assert_eq!(archived.unattributed_seconds, 1500.0);
    assert_eq!(
        archived.projects.iter().map(|p| p.seconds).sum::<f64>() + archived.unattributed_seconds,
        3900.0
    );
    assert_eq!(f.activity.project_attribution_for_day(day).unwrap(), stored);
    f.storage.entity_archive_set(&f.legacy, false, 11).unwrap();
    let restored = crate::journal::collect(&f.storage, &f.activity, day).unwrap();
    assert!(
        restored
            .projects
            .iter()
            .any(|p| p.key == f.legacy && p.seconds == 1200.0)
    );
    assert_eq!(restored.unattributed_seconds, 300.0);
}

#[test]
fn curated_graph_archive_later_registry_link_stays_excluded_until_undo() {
    let f = fixture();
    f.storage.entity_archive_set(&f.legacy, true, 10).unwrap();
    let id = f
        .storage
        .project_upsert("phantom-board", "Phantom Board", None)
        .unwrap();
    f.storage.project_link_entity(id, "phantom-board").unwrap();
    assert!(
        f.storage
            .registry_signals(f.from, f.to)
            .unwrap()
            .iter()
            .all(|(key, _)| *key != id)
    );
    let keys = f.storage.attribution_keys_now().unwrap();
    assert!(keys.is_archived(&f.legacy));
    assert!(!keys.is_archived(&crate::owner_time::registry_key(id)));
    f.storage.entity_archive_set(&f.legacy, false, 11).unwrap();
    assert!(
        f.storage
            .registry_signals(f.from, f.to)
            .unwrap()
            .iter()
            .any(|(key, _)| *key == id)
    );
}

#[path = "archived_time_adoption_tests.rs"]
mod adoption_tests;
