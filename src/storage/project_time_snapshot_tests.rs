//! Rows, archive masking and totals share one read snapshot during a response.
use super::*;
use crate::activity::{ProjectTimeData, ProjectTimeRow};
use crate::event::{EventSource, MemoryType};
use crate::graph::{Entity, EntityType};
use crate::owner_time::registry_key;
use crate::timeline::fixture::Fixture;
use std::cell::RefCell;

thread_local! {
    static AFTER_REKEY: RefCell<Option<Box<dyn FnOnce()>>> = const { RefCell::new(None) };
}

// The callback belongs to this test thread only. It commits through another
// WAL connection after the time mask, before any overview or union query.
pub(super) fn after_rekey() {
    let callback = AFTER_REKEY.with(|slot| slot.borrow_mut().take());
    if let Some(callback) = callback {
        callback();
    }
}

fn interleave<T>(change: impl FnOnce() + 'static, read: impl FnOnce() -> T) -> T {
    AFTER_REKEY.with(|slot| assert!(slot.borrow_mut().replace(Box::new(change)).is_none()));
    let result = read();
    AFTER_REKEY.with(|slot| assert!(slot.borrow().is_none(), "read must reach the interleaving"));
    result
}

fn entity(storage: &Storage, name: &str) -> String {
    let id = storage
        .upsert_entity(&Entity {
            name: name.into(),
            entity_type: EntityType::Project,
        })
        .unwrap();
    let memory = MemoryEntry::new(
        format!("{name} implementation"),
        "The fixture implementation was verified.",
        MemoryType::Note,
        EventSource::Manual,
    );
    storage.save(&memory).unwrap();
    storage.link_memory_entity(&memory.id, &id).unwrap();
    id
}

fn row(key: &str, today: f64, week: f64) -> ProjectTimeRow {
    ProjectTimeRow {
        project_key: key.into(),
        today_seconds: today,
        week_seconds: week,
        week: [week - today, 0.0, 0.0, 0.0, 0.0, 0.0, today],
        confidence: Some("high".into()),
    }
}

fn time(first: &str, second: &str) -> ProjectTimeData {
    ProjectTimeData {
        rows: vec![row(first, 60.0, 300.0), row(second, 120.0, 600.0)],
        unattributed_today: 30.0,
        unattributed_week: 90.0,
    }
}

fn normalize(mut payload: serde_json::Value) -> serde_json::Value {
    let projects = if payload.is_array() {
        payload.as_array_mut().unwrap()
    } else {
        payload["projects"].as_array_mut().unwrap()
    };
    projects.sort_by(|a, b| a["key"].as_str().cmp(&b["key"].as_str()));
    payload
}

fn totals(payload: &serde_json::Value) -> (i64, i64) {
    let sum = |column: &str| {
        payload["projects"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row[column].as_i64().unwrap_or(0))
            .sum::<i64>()
            + payload["unattributed"][column].as_i64().unwrap()
    };
    (sum("today_seconds"), sum("week_seconds"))
}

#[test]
fn curated_graph_project_payload_conserves_both_totals_during_archive_and_undo() {
    for archived in [false, true] {
        // Both the normal overview and the attributed-outside-cap union
        // must continue reading the snapshot used for the mask.
        for cap in [0, 10] {
            let f = Fixture::empty();
            let first = entity(&f.storage, "pending-work");
            let second = entity(&f.storage, "kept-work");
            if archived {
                f.storage.entity_archive_set(&first, true, 10).unwrap();
            }
            let time = time(&first, &second);
            let before = normalize(f.storage.projects_payload(cap, 1, Some(&time)).unwrap());
            let writer = Storage::open(&f.memory_path).unwrap();
            let changed = first.clone();
            let changes = f.changes();
            let during = interleave(
                move || {
                    assert!(writer.entity_archive_set(&changed, !archived, 11).unwrap());
                },
                || f.storage.projects_payload(cap, 1, Some(&time)).unwrap(),
            );
            assert_eq!(normalize(during.clone()), before);
            assert_eq!(totals(&during), (210, 990));
            assert_eq!(f.changes(), changes, "the reader must write nothing");
            let after = f.storage.projects_payload(cap, 1, Some(&time)).unwrap();
            assert_ne!(normalize(after.clone()), before);
            assert_eq!(totals(&after), (210, 990));
            assert_eq!(
                after["unattributed"]["today_seconds"],
                if archived { 30 } else { 90 }
            );
            assert_eq!(
                after["unattributed"]["week_seconds"],
                if archived { 90 } else { 390 }
            );
        }
    }
}

#[test]
fn curated_graph_projects_value_shares_the_payload_snapshot() {
    let f = Fixture::empty();
    let first = entity(&f.storage, "pending-work");
    let second = entity(&f.storage, "kept-work");
    let time = time(&first, &second);
    let expected = f.storage.projects_payload(0, 1, Some(&time)).unwrap();
    let writer = Storage::open(&f.memory_path).unwrap();
    let during = interleave(
        move || assert!(writer.entity_archive_set(&first, true, 10).unwrap()),
        || f.storage.projects_value(0, 1, Some(&time)).unwrap(),
    );
    assert_eq!(normalize(during), normalize(expected["projects"].clone()));
    let after = f.storage.projects_payload(0, 1, Some(&time)).unwrap();
    assert_eq!(totals(&after), (210, 990));
    assert_eq!(after["projects"].as_array().unwrap().len(), 1);
}

#[test]
fn curated_graph_project_payload_keeps_registry_names_in_the_same_snapshot() {
    let f = Fixture::empty();
    let id = f
        .storage
        .project_upsert("registry-work", "Original work", None)
        .unwrap();
    let time = ProjectTimeData {
        rows: vec![row(&registry_key(id), 60.0, 300.0)],
        unattributed_today: 30.0,
        unattributed_week: 90.0,
    };
    let writer = Storage::open(&f.memory_path).unwrap();
    let during = interleave(
        move || {
            assert_eq!(
                writer
                    .project_upsert("registry-work", "Renamed work", None)
                    .unwrap(),
                id
            );
        },
        || f.storage.projects_payload(0, 1, Some(&time)).unwrap(),
    );
    assert_eq!(during["projects"][0]["name"], "Original work");
    assert_eq!(totals(&during), (90, 390));
    let after = f.storage.projects_payload(0, 1, Some(&time)).unwrap();
    assert_eq!(after["projects"][0]["name"], "Renamed work");
    assert_eq!(totals(&after), (90, 390));
}

#[test]
fn curated_graph_project_payload_preserves_merge_aliases_and_public_read_purity() {
    let f = Fixture::empty();
    let first = entity(&f.storage, "earlier-work");
    let second = entity(&f.storage, "current-work");
    let alias = f
        .storage
        .project_upsert("earlier-work", "Earlier work", None)
        .unwrap();
    let target = f
        .storage
        .project_upsert("current-work", "Current work", None)
        .unwrap();
    f.storage
        .project_link_entity(alias, "earlier-work")
        .unwrap();
    f.storage
        .project_link_entity(target, "current-work")
        .unwrap();
    f.storage.project_merge(alias, target).unwrap();
    let time = ProjectTimeData {
        rows: vec![
            row(&first, 30.0, 90.0),
            row(&registry_key(alias), 40.0, 100.0),
            row(&second, 60.0, 200.0),
            row(&registry_key(target), 70.0, 300.0),
        ],
        unattributed_today: 10.0,
        unattributed_week: 20.0,
    };
    let before = (f.changes(), f.stores());
    let keys = f.storage.attribution_keys_now().unwrap();
    for row in &time.rows {
        assert_eq!(keys.now(&row.project_key), second);
        assert!(!keys.is_archived(&row.project_key));
    }
    assert_eq!(f.storage.attribution_keys().unwrap()[&target], second);
    let mapped = f.storage.rekeyed_time(&time).unwrap();
    assert_eq!(mapped.rows, [row(&second, 200.0, 690.0)]);
    let payload = f.storage.projects_payload(0, 1, Some(&time)).unwrap();
    assert_eq!(totals(&payload), (210, 710));
    assert_eq!(payload["projects"].as_array().unwrap().len(), 1);
    assert_eq!(payload["projects"][0]["key"], second);
    assert_eq!(payload["projects"][0]["mem_count"], 1);
    assert_eq!(payload["projects"][0]["mems"].as_array().unwrap().len(), 1);
    assert_eq!(
        f.storage.projects_value(0, 1, Some(&time)).unwrap(),
        payload["projects"]
    );
    let none = f.storage.projects_payload(10, 0, None).unwrap();
    assert!(none["unattributed"].is_null());
    assert_eq!(
        normalize(f.storage.projects_value(10, 0, None).unwrap()),
        normalize(none["projects"].clone())
    );
    for project in none["projects"].as_array().unwrap() {
        assert_eq!(project["tracking"], false);
        assert!(project["today_seconds"].is_null());
        assert!(project["week_seconds"].is_null());
        assert!(project["week"].as_array().unwrap().is_empty());
        assert!(project["mems"].as_array().unwrap().is_empty());
    }
    assert_eq!((f.changes(), f.stores()), before);
    assert!(f.storage.conn.lock().unwrap().is_autocommit());
}

#[test]
fn curated_graph_project_payload_preserves_archived_adoption_and_undo() {
    let f = Fixture::empty();
    let historical = entity(&f.storage, "adopted-work");
    f.storage.entity_archive_set(&historical, true, 10).unwrap();
    let id = f
        .storage
        .project_upsert("adopted-work", "Adopted work", None)
        .unwrap();
    f.storage.project_link_entity(id, "adopted-work").unwrap();
    let key = registry_key(id);
    let time = time(&historical, &key);
    let before = (f.changes(), f.stores());
    let payload = f.storage.projects_payload(10, 1, Some(&time)).unwrap();
    assert_eq!(payload["projects"].as_array().unwrap().len(), 1);
    assert_eq!(payload["projects"][0]["key"], key);
    assert_eq!(payload["projects"][0]["today_seconds"], 120);
    assert_eq!(payload["projects"][0]["week_seconds"], 600);
    assert_eq!(payload["unattributed"]["today_seconds"], 90);
    assert_eq!(payload["unattributed"]["week_seconds"], 390);
    assert_eq!(totals(&payload), (210, 990));
    assert_eq!((f.changes(), f.stores()), before);
    f.storage
        .entity_archive_set(&historical, false, 11)
        .unwrap();
    let before = (f.changes(), f.stores());
    let restored = f.storage.projects_payload(0, 1, Some(&time)).unwrap();
    assert_eq!(restored["projects"].as_array().unwrap().len(), 1);
    assert_eq!(restored["projects"][0]["key"], historical);
    assert_eq!(restored["projects"][0]["today_seconds"], 180);
    assert_eq!(restored["projects"][0]["week_seconds"], 900);
    assert_eq!(restored["unattributed"]["today_seconds"], 30);
    assert_eq!(restored["unattributed"]["week_seconds"], 90);
    assert_eq!(totals(&restored), (210, 990));
    assert_eq!((f.changes(), f.stores()), before);
}
