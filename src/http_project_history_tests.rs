//! The history of a project holds decisions and corrections (design 9.3),
//! against the isolated synthetic store of the project route tests.
use super::tests::{Fixture, call, request};
use crate::event::{EventSource, MemoryEntry, MemoryType};
use crate::graph::{Entity, EntityType};
use chrono::{Duration, Utc};

fn remember(f: &Fixture, entity: &str, title: &str, kind: MemoryType, minutes_ago: i64) -> String {
    let mut entry = MemoryEntry::new(title, "Fixture body", kind, EventSource::Manual);
    entry.timestamp = Utc::now() - Duration::minutes(minutes_ago);
    f.state.storage.save(&entry).unwrap();
    f.state
        .storage
        .link_memory_entity(&entry.id, entity)
        .unwrap();
    entry.id
}

fn entity_of(f: &Fixture, key: &str) -> String {
    f.state
        .storage
        .project_by_key(key)
        .unwrap()
        .unwrap()
        .entity_id
        .unwrap()
}

async fn history(f: &Fixture, key: &str) -> Vec<(String, String)> {
    let (_, detail) = call(
        &f.state,
        request(&format!("/api/projects/{key}"), true, "localhost"),
    )
    .await;
    detail["history"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| {
            (
                entry["id"].as_str().unwrap().to_owned(),
                entry["kind"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

#[tokio::test]
async fn plan_project_route_history_holds_the_latest_decisions_and_corrections() {
    let f = Fixture::new();
    let entity = entity_of(&f, "demoapp");
    let mut expected = vec![(history(&f, "demoapp").await[0].0.clone(), "decision".into())];
    for minutes in (5..60).step_by(5) {
        let (kind, word) = if minutes % 10 == 0 {
            (MemoryType::Decision, "decision")
        } else {
            (MemoryType::Feedback, "correction")
        };
        let id = remember(&f, &entity, "Keep fixture reads local", kind, minutes);
        expected.push((id, word.to_owned()));
    }
    // Newer than every entry, and of a type the history does not hold.
    for minutes in [1, 2] {
        remember(
            &f,
            &entity,
            "A note of the fixture",
            MemoryType::Note,
            minutes,
        );
    }
    let (before, changes) = (f.stores(), f.changes());
    let served = history(&f, "demoapp").await;
    assert_eq!(served, expected[..10].to_vec());
    // The library's read of decisions keeps serving decisions only.
    let demo = f
        .state
        .storage
        .project_by_key("demoapp")
        .unwrap()
        .unwrap()
        .id;
    let decisions: Vec<String> = f
        .state
        .storage
        .project_decisions(demo, 10)
        .unwrap()
        .into_iter()
        .map(|decision| decision.id)
        .collect();
    let only: Vec<String> = expected
        .iter()
        .filter(|(_, kind)| kind == "decision")
        .map(|(id, _)| id.clone())
        .collect();
    assert_eq!(decisions, only);
    assert_eq!(f.stores(), before);
    assert_eq!(f.changes(), changes);

    // Rows the read may not serve still take their part of the limit.
    {
        let conn = f.state.storage.conn.lock().unwrap();
        for (n, (id, _)) in expected[1..3].iter().enumerate() {
            let refused = format!("sk-{n}{}", "AbCd1234".repeat(6));
            conn.execute(
                "UPDATE memories SET id = ?1 WHERE id = ?2",
                rusqlite::params![refused, id],
            )
            .unwrap();
            conn.execute(
                "UPDATE memory_entities SET memory_id = ?1 WHERE memory_id = ?2",
                rusqlite::params![refused, id],
            )
            .unwrap();
        }
    }
    let served = history(&f, "demoapp").await;
    assert_eq!(served.len(), 8);
    assert_eq!(served[0], expected[0]);
    assert_eq!(served[1..], expected[3..10]);
}

#[tokio::test]
async fn plan_project_route_history_of_corrections_only_holds_them() {
    let f = Fixture::new();
    let storage = &f.state.storage;
    let id = storage
        .project_upsert("otherapp", "otherapp", None)
        .unwrap();
    let entity = storage
        .upsert_entity(&Entity {
            name: "otherapp".into(),
            entity_type: EntityType::Project,
        })
        .unwrap();
    storage.project_link_entity(id, "otherapp").unwrap();
    let ids: Vec<String> = (1..=3)
        .map(|minutes| {
            remember(
                &f,
                &entity,
                "Review the result first",
                MemoryType::Feedback,
                minutes,
            )
        })
        .collect();
    let (before, changes) = (f.stores(), f.changes());
    let served = history(&f, "otherapp").await;
    assert_eq!(
        served,
        ids.into_iter()
            .map(|id| (id, "correction".to_owned()))
            .collect::<Vec<_>>()
    );
    let (_, detail) = call(
        &f.state,
        request("/api/projects/otherapp", true, "localhost"),
    )
    .await;
    assert!(
        detail["history"]
            .as_array()
            .unwrap()
            .iter()
            .all(|entry| entry["title"] == "Review the result first")
    );
    assert_eq!(f.stores(), before);
    assert_eq!(f.changes(), changes);
}
