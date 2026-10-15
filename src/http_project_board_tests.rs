//! The counts of a board row and the board's week (design 9.2), against
//! the isolated synthetic store of the project route tests.
use super::tests::{Fixture, call, request};
use crate::graph::{Entity, EntityType};
use crate::storage::projects::ProjectState;
use chrono::{DateTime, Duration, Local, NaiveDate, Utc};
use serde_json::Value;

fn segment(f: &Fixture, stream: &str, project: Option<i64>, start: DateTime<Utc>, minutes: i64) {
    let conn = f.state.storage.conn.lock().unwrap();
    conn.execute(
        "INSERT INTO work_segments
        (stream, driver, agent, project_id, started_at, last_at, source)
        VALUES (?1, 'owner', 'codex', ?2, ?3, ?4, 'path')",
        rusqlite::params![
            stream,
            project,
            start.format("%Y-%m-%d %H:%M:%S").to_string(),
            (start + Duration::minutes(minutes))
                .format("%Y-%m-%d %H:%M:%S")
                .to_string()
        ],
    )
    .unwrap();
}

/// A second project whose plan has an open unit and an archived one.
fn other_project(f: &Fixture) -> (i64, String) {
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
    let root = f
        .project
        .parent()
        .unwrap()
        .join("otherapp/openspec/changes");
    for (folder, mark) in [("live-work", " "), ("archive/2026-01-02-shipped", "x")] {
        std::fs::create_dir_all(root.join(folder)).unwrap();
        std::fs::write(
            root.join(folder).join("tasks.md"),
            format!("- [{mark}] T01: Complete the fixture\n"),
        )
        .unwrap();
    }
    (id, entity)
}

fn refresh(f: &Fixture) {
    f.state
        .live
        .as_ref()
        .unwrap()
        .refresh_plans_at(Utc::now(), std::time::Instant::now(), true);
}

fn row<'a>(board: &'a Value, key: &str) -> &'a Value {
    board["projects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["key"] == key)
        .unwrap()
}

#[tokio::test]
async fn plan_project_route_board_counts_units_and_names_what_waits_and_what_closed() {
    let f = Fixture::new();
    other_project(&f);
    std::fs::write(
        f.project.join("tasks.md"),
        "- [x] T01: Build the fixture\n- [ ] T02: Review the result\n  Owner: Confirm the copy\n",
    )
    .unwrap();
    refresh(&f);
    let (before, changes) = (f.stores(), f.changes());
    let (_, board) = call(&f.state, request("/api/projects", true, "localhost")).await;
    let demo = row(&board, "demoapp");
    assert_eq!(demo["open_units"], 1);
    assert_eq!(demo["last_closed"], Value::Null);
    let (_, detail) = call(
        &f.state,
        request("/api/projects/demoapp", true, "localhost"),
    )
    .await;
    assert_eq!(
        demo["waiting_on_owner"],
        serde_json::json!(["Confirm the copy"])
    );
    assert_eq!(
        demo["waiting_on_owner"],
        detail["plan"]["units"][0]["waiting_on_owner"]
    );

    let other = row(&board, "otherapp");
    assert_eq!(other["open_units"], 1);
    let (_, detail) = call(
        &f.state,
        request("/api/projects/otherapp", true, "localhost"),
    )
    .await;
    let units = detail["plan"]["units"].as_array().unwrap();
    assert_eq!(units.len(), 2);
    let closed = units.iter().find(|unit| unit["closed"] == true).unwrap();
    let archived = NaiveDate::from_ymd_opt(2026, 1, 2)
        .unwrap()
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_utc()
        .timestamp();
    assert_eq!(other["last_closed"]["title"], closed["title"]);
    assert_eq!(other["last_closed"]["closed_at"], archived);
    assert_eq!(f.stores(), before);
    assert_eq!(f.changes(), changes);

    // Done in place, with no closure time: named, with no time.
    std::fs::write(
        f.project.join("tasks.md"),
        "- [x] T01: Build the fixture\n- [x] T02: Review the result\n",
    )
    .unwrap();
    refresh(&f);
    let (_, board) = call(&f.state, request("/api/projects", true, "localhost")).await;
    let demo = row(&board, "demoapp");
    assert_eq!(demo["open_units"], 0);
    assert_eq!(demo["last_closed"]["closed_at"], Value::Null);
    assert_eq!(demo["waiting_on_owner"], serde_json::json!([]));

    std::fs::remove_file(f.project.join("tasks.md")).unwrap();
    refresh(&f);
    let (_, board) = call(&f.state, request("/api/projects", true, "localhost")).await;
    let demo = row(&board, "demoapp").as_object().unwrap();
    for key in ["open_units", "waiting_on_owner", "last_closed"] {
        assert_eq!(demo.get(key), Some(&Value::Null), "{key}");
    }
}

#[tokio::test]
async fn plan_project_route_board_week_sums_daily_unions_and_the_owner_before_rekeying() {
    let f = Fixture::new();
    let (other, entity) = other_project(&f);
    let yesterday = Local::now().date_naive() - Duration::days(1);
    let (from, _) = crate::activity::local_day_bounds_utc(yesterday).unwrap();
    // The fixture holds demoapp from 02:00 to 03:00; otherapp works at
    // the same time for half an hour, and ten minutes belong to no project.
    segment(
        &f,
        "fixture-other",
        Some(other),
        from + Duration::minutes(150),
        60,
    );
    segment(&f, "fixture-none", None, from + Duration::hours(5), 10);
    let activity = f.state.activity.as_ref().unwrap();
    let demo_entity = f
        .state
        .storage
        .project_by_key("demoapp")
        .unwrap()
        .unwrap()
        .entity_id
        .unwrap();
    activity
        .write_day(
            yesterday,
            &[(&demo_entity, 1200.0, "high"), (&entity, 500.0, "low")],
            0.0,
        )
        .unwrap();
    activity
        .write_day(
            Local::now().date_naive(),
            &[(&demo_entity, 300.0, "medium")],
            100.0,
        )
        .unwrap();
    let (before, changes) = (f.stores(), f.changes());
    let (_, board) = call(&f.state, request("/api/projects", true, "localhost")).await;
    let rows: i64 = board["projects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["week"]["agent_s"].as_i64().unwrap())
        .sum();
    assert_eq!(rows, 7200);
    assert_eq!(board["week"]["agent_s"], 90 * 60 + 10 * 60);
    assert_eq!(board["week"]["owner_s"], 1200.0 + 500.0 + 300.0 + 100.0);
    assert_eq!(board["week"]["owner_estimate"], true);
    assert_eq!(row(&board, "demoapp")["week"]["agent_s"], 3600);
    assert_eq!(f.stores(), before);
    assert_eq!(f.changes(), changes);

    // Archiving moves the project's rows out of the registry's keys, not
    // out of the owner's time at the Mac.
    assert!(
        f.state
            .storage
            .project_set_state(other, ProjectState::Archived)
            .unwrap()
    );
    let (_, board) = call(&f.state, request("/api/projects", true, "localhost")).await;
    assert_eq!(board["week"]["owner_s"], 2100.0);

    let mut blind = f.state.clone();
    blind.activity = None;
    let (_, board) = call(&blind, request("/api/projects", true, "localhost")).await;
    assert_eq!(board["week"]["owner_s"], Value::Null);
    assert_eq!(board["week"]["agent_s"], 90 * 60 + 10 * 60);
}
