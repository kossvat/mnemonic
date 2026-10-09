//! Project reads through the router, against isolated synthetic stores.
use super::super::{AppState, TOKEN_HEADER, router};
use crate::embedding::HashEmbedder;
use crate::event::{EventSource, MemoryEntry, MemoryType};
use crate::graph::{Entity, EntityType};
use crate::storage::Storage;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use chrono::{Duration, Local, Utc};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use tower::Service;

struct Silent;

impl crate::live_signal::Post for Silent {
    fn post(&self, _name: &str) {}
}

struct Fixture {
    _temp: tempfile::TempDir,
    state: AppState,
    project: PathBuf,
    activity: PathBuf,
    decision: String,
    yesterday: String,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        let project = workspace.join("projects/demoapp");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(
            project.join("tasks.md"),
            "- [x] T01: Build the fixture\n- [ ] T02: Review the result\n",
        )
        .unwrap();
        let storage = Arc::new(Storage::open(&temp.path().join("memory.db")).unwrap());
        let id = storage.project_upsert("demoapp", "demoapp", None).unwrap();
        let entity = storage
            .upsert_entity(&Entity {
                name: "demoapp".into(),
                entity_type: EntityType::Project,
            })
            .unwrap();
        storage.project_link_entity(id, "demoapp").unwrap();
        let decision = MemoryEntry::new(
            "Use the local cache",
            "Keep fixture reads local",
            MemoryType::Decision,
            EventSource::Manual,
        );
        storage.save(&decision).unwrap();
        storage.link_memory_entity(&decision.id, &entity).unwrap();
        let yesterday = Local::now().date_naive() - Duration::days(1);
        let (from, _) = crate::activity::local_day_bounds_utc(yesterday).unwrap();
        let start = from + Duration::hours(2);
        let end = start + Duration::hours(1);
        {
            let conn = storage.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO work_segments
                (stream, driver, agent, project_id, started_at, last_at, source)
                VALUES ('fixture-history', 'owner', 'claude', ?1, ?2, ?3, 'path')",
                rusqlite::params![
                    id,
                    start.format("%Y-%m-%d %H:%M:%S").to_string(),
                    end.format("%Y-%m-%d %H:%M:%S").to_string()
                ],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO recall_events
                (id, at, surface, tool, client, run, returned, corrections, chars, hidden)
                VALUES (1, ?1, 'mcp', 'memory_search', NULL, 'fixture', 1, 0, 20, 0)",
                [Utc::now().to_rfc3339()],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO recall_items (event_id, rank, memory_id, section)
                VALUES (1, 0, ?1, NULL)",
                [&decision.id],
            )
            .unwrap();
        }
        let activity_path = temp.path().join("activity.db");
        let activity = Arc::new(crate::activity::ActivityStore::open(&activity_path).unwrap());
        activity
            .write_day(yesterday, &[(&entity, 1200.0, "high")], 0.0)
            .unwrap();
        activity
            .write_day(
                Local::now().date_naive(),
                &[(&entity, 300.0, "medium")],
                0.0,
            )
            .unwrap();
        let mut config = crate::config::Config::default();
        config.projects = crate::config::ProjectsConfig {
            workspace: Some(workspace),
            folder: Some("projects".into()),
            plans: BTreeMap::from([(
                "demoapp".into(),
                crate::plans::Config {
                    provider: Some(crate::plans::Provider::Checkbox),
                    authority: Some("tasks.md".into()),
                    gate_required: Some(false),
                    ..Default::default()
                },
            )]),
            ..Default::default()
        };
        let live =
            crate::live_run::Live::new("project-test", "test signal".into(), Box::new(Silent));
        live.load_projects(&storage).unwrap();
        let now = Utc::now();
        live.observed(
            "fixture-session",
            crate::live::Session {
                agent: "claude".into(),
                program: false,
                project: Some(id),
                last_kind: crate::projects::facts::Kind::Call,
                last_at: now,
                started_at: now - Duration::seconds(60),
            },
        );
        live.configure_plans(storage.clone(), config.projects.clone());
        let state = AppState {
            storage,
            embedder: Arc::new(HashEmbedder::new()),
            token: "test-token".into(),
            config,
            activity: Some(activity),
            live: Some(live),
        };
        Self {
            _temp: temp,
            state,
            project,
            activity: activity_path,
            decision: decision.id,
            yesterday: yesterday.to_string(),
        }
    }

    fn changes(&self) -> u64 {
        self.state.storage.conn.lock().unwrap().total_changes()
    }

    fn stores(&self) -> (BTreeMap<String, Vec<String>>, BTreeMap<String, Vec<String>>) {
        let memory = dump(&self.state.storage.conn.lock().unwrap());
        let activity = rusqlite::Connection::open_with_flags(
            &self.activity,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        (memory, dump(&activity))
    }
}

/// Every logical value, including complete blob bytes and schema. Rows
/// are sorted so a changed query plan cannot masquerade as a write.
fn dump(conn: &rusqlite::Connection) -> BTreeMap<String, Vec<String>> {
    let mut names = conn
        .prepare("SELECT name FROM sqlite_schema WHERE type = 'table' ORDER BY name")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    names.push("sqlite_schema".into());
    names
        .into_iter()
        .map(|name| {
            let quoted = name.replace('"', "\"\"");
            let mut statement = conn
                .prepare(&format!("SELECT * FROM \"{quoted}\""))
                .unwrap();
            let columns = statement.column_count();
            let mut values = statement
                .query_map([], |row| {
                    let values = (0..columns)
                        .map(|column| row.get::<_, rusqlite::types::Value>(column))
                        .collect::<rusqlite::Result<Vec<_>>>()?;
                    Ok(format!("{values:?}"))
                })
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap();
            values.sort();
            (name, values)
        })
        .collect()
}

fn request(path: &str, token: bool, host: &str) -> Request<Body> {
    let mut request = Request::builder().uri(path).header(header::HOST, host);
    if token {
        request = request.header(TOKEN_HEADER, "test-token");
    }
    request.body(Body::empty()).unwrap()
}

async fn call(state: &AppState, request: Request<Body>) -> (StatusCode, Value) {
    let response = router(state.clone()).call(request).await.unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (status, serde_json::from_slice(&body).unwrap())
}

#[tokio::test]
async fn plan_project_routes_return_plans_sessions_days_and_history_without_store_changes() {
    let f = Fixture::new();
    let (before, changes) = (f.stores(), f.changes());
    assert_eq!(before.0["recall_events"].len(), 1);
    assert_eq!(before.0["recall_items"].len(), 1);
    let (status, mut board) = call(&f.state, request("/api/projects", true, "localhost")).await;
    assert_eq!(status, StatusCode::OK);
    let project = &board["projects"][0];
    assert_eq!(project["key"], "demoapp");
    assert_eq!(project["week"]["owner_s"], 1500.0);
    assert_eq!(project["week"]["agent_s"], 3600);
    assert_eq!(project["plan"]["total"], 2);
    let (status, mut detail) = call(
        &f.state,
        request("/api/projects/demoapp", true, "localhost"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(detail["key"], "demoapp");
    assert_eq!(
        detail["plan"]["units"][0]["tasks"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(detail["sessions"].as_array().unwrap().len(), 1);
    assert_eq!(detail["sessions"][0]["agent"], "claude");
    assert_eq!(detail["sessions"][0]["driver"], "owner");
    assert_eq!(detail["sessions"][0]["state"], "running");
    assert!(detail["sessions"][0]["started_at"].as_i64().is_some());
    assert_eq!(detail["time"].as_array().unwrap().len(), 14);
    let yesterday = detail["time"]
        .as_array()
        .unwrap()
        .iter()
        .find(|day| day["day"] == f.yesterday)
        .unwrap();
    assert_eq!(yesterday["owner_s"], 1200.0);
    assert_eq!(yesterday["agent_s"], 3600);
    assert_eq!(yesterday["confidence"], "high");
    assert_eq!(detail["history"][0]["id"], f.decision);
    assert_eq!(detail["history"][0]["title"], "Use the local cache");
    assert!(detail["history"][0]["at"].as_i64().is_some());
    // Match the exact payload decoded by both Swift clients. Only the
    // synthetic clock, file time and generated memory id are normalized.
    let expected_board: Value = serde_json::from_str(include_str!(
        "../clients/macos/Tests/Fixtures/project-board.json"
    ))
    .unwrap();
    let expected_detail: Value = serde_json::from_str(include_str!(
        "../clients/macos/Tests/Fixtures/project-detail.json"
    ))
    .unwrap();
    assert!(board["projects"][0]["last_active_at"].as_i64().is_some());
    board["projects"][0]["last_active_at"] =
        expected_board["projects"][0]["last_active_at"].clone();
    assert!(detail["plan"]["units"][0]["as_of"].as_i64().is_some());
    detail["plan"]["units"][0]["as_of"] = expected_detail["plan"]["units"][0]["as_of"].clone();
    detail["sessions"][0]["started_at"] = expected_detail["sessions"][0]["started_at"].clone();
    for (actual, expected) in detail["time"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .zip(expected_detail["time"].as_array().unwrap())
    {
        actual["day"] = expected["day"].clone();
    }
    detail["history"][0]["id"] = expected_detail["history"][0]["id"].clone();
    detail["history"][0]["at"] = expected_detail["history"][0]["at"].clone();
    assert_eq!(board, expected_board);
    assert_eq!(detail, expected_detail);
    assert_eq!(f.stores(), before);
    assert_eq!(f.changes(), changes);
}

#[tokio::test]
async fn plan_project_routes_refuse_missing_token_foreign_host_and_unknown_project() {
    let f = Fixture::new();
    let (before, changes) = (f.stores(), f.changes());
    for path in ["/api/projects", "/api/projects/demoapp"] {
        assert_eq!(
            call(&f.state, request(path, false, "localhost")).await.0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            call(&f.state, request(path, true, "elsewhere")).await.0,
            StatusCode::MISDIRECTED_REQUEST
        );
    }
    assert_eq!(
        call(
            &f.state,
            request("/api/projects/missing", true, "localhost")
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(f.stores(), before);
    assert_eq!(f.changes(), changes);
}

#[tokio::test]
async fn plan_project_route_serves_capped_titles_prepared_again_after_the_cut() {
    let f = Fixture::new();
    let secret = format!("sk-{}", "AbCd1234".repeat(6));
    let body = "AbCd1234".repeat(3);
    let prefix = "a".repeat(160 - " password=".len() - body.len());
    let cut = format!("{prefix} password={body}(arg) for later");
    assert!(!crate::redaction::redact_text(&cut).changed);
    std::fs::write(
        f.project.join("tasks.md"),
        format!("- [ ] T01: Replace {secret}\n- [ ] T02: {cut}\n"),
    )
    .unwrap();
    let live = f.state.live.as_ref().unwrap();
    live.refresh_plans_at(Utc::now(), std::time::Instant::now(), true);
    let (before, changes) = (f.stores(), f.changes());
    let (status, detail) = call(
        &f.state,
        request("/api/projects/demoapp", true, "localhost"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let shown = detail.to_string();
    assert!(!shown.contains(&secret) && !shown.contains(&body));
    assert!(!shown.contains(f.project.to_str().unwrap()));
    for task in detail["plan"]["units"][0]["tasks"].as_array().unwrap() {
        let title = task["title"].as_str().unwrap();
        assert!(title.chars().count() <= 160);
        assert!(!crate::redaction::redact_text(title).changed);
    }
    assert_eq!(f.stores(), before);
    assert_eq!(f.changes(), changes);
}
