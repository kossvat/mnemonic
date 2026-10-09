use super::*;
use crate::activity::ActivityStore;
use crate::owner_time_day::{Calendar, LocalCalendar};
use chrono::{DateTime, Duration, Utc};
use std::path::Path;

fn active(path: &Path, id: &str, from: DateTime<Utc>, seconds: i64) {
    rusqlite::Connection::open(path)
        .unwrap()
        .execute(
            "INSERT INTO work_sessions(id,started_at,ended_at,open) VALUES(?1,?2,?3,0)",
            rusqlite::params![
                id,
                from.to_rfc3339(),
                (from + Duration::seconds(seconds)).to_rfc3339()
            ],
        )
        .unwrap();
}

fn explicit_segment(f: &Fixture, stream: &str, project: i64, from: DateTime<Utc>, seconds: i64) {
    let stamp = |at: DateTime<Utc>| at.format("%Y-%m-%d %H:%M:%S").to_string();
    let conn = f.state.storage.conn.lock().unwrap();
    conn.execute(
        "INSERT INTO work_segments
         (stream,driver,agent,project_id,started_at,last_at,source)
         VALUES(?1,'owner','codex',?2,?3,?4,'path')",
        rusqlite::params![
            stream,
            project,
            stamp(from),
            stamp(from + Duration::seconds(seconds))
        ],
    )
    .unwrap();
    let segment = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO segment_activity(segment_id,minute,records,touches) VALUES(?1,?2,1,1)",
        rusqlite::params![segment, from.timestamp().div_euclid(60)],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO owner_prompts(stream,at) VALUES(?1,?2)",
        rusqlite::params![stream, stamp(from)],
    )
    .unwrap();
}

#[tokio::test]
async fn curated_graph_http_undo_retries_failed_activity_repair_before_publishing_graph() {
    let mut f = Fixture::new(3);
    assert!(f.state.config.projects.root().is_some());
    let activity_path = f.temp.path().join("activity.db");
    let activity = Arc::new(ActivityStore::open(&activity_path).unwrap());
    f.state.activity = Some(activity.clone());
    let live = f.state.live.as_ref().unwrap();
    live.attach_graph_activity(activity.clone()).unwrap();
    let day = LocalCalendar.date_of(Utc::now()) - Duration::days(7);
    let start = LocalCalendar.bounds(day).unwrap().0;
    let fallback = start + Duration::hours(9);
    let explicit = start + Duration::hours(14);
    active(&activity_path, "fixture-fallback", fallback, 300);
    active(&activity_path, "fixture-explicit", explicit, 180);
    explicit_segment(&f, "fixture-explicit", 1, explicit, 180);
    let mut memory = MemoryEntry::new(
        "Fixture historical evidence",
        "Names the adopted project",
        MemoryType::Decision,
        EventSource::Manual,
    );
    memory.timestamp = fallback + Duration::minutes(1);
    f.state.storage.save(&memory).unwrap();
    f.state
        .storage
        .link_memory_entity(&memory.id, &f.outside)
        .unwrap();
    let uri = format!("/api/entities/{}/archive", f.outside);
    let (status, _, _) = call(
        &f.state,
        request(&uri, true, "localhost", Some(r#"{"archived":true}"#)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let adopted = f
        .state
        .storage
        .project_upsert("adopted", "adopted", None)
        .unwrap();
    assert!(
        f.state
            .storage
            .project_link_entity(adopted, "outside-project")
            .unwrap()
    );
    assert_eq!(
        f.state
            .storage
            .project_by_key("adopted")
            .unwrap()
            .unwrap()
            .entity_id
            .as_deref(),
        Some(f.outside.as_str())
    );
    crate::owner_time_day::recompute_day(
        &activity,
        &f.state.storage,
        &LocalCalendar,
        day,
        &crate::attribution::AttribCfg::default(),
    )
    .unwrap();
    let initial = activity.project_attribution_for_day(day).unwrap();
    assert_eq!(
        initial,
        (vec![(f.alpha.clone(), 180.0, Some("high".into()))], 300.0)
    );
    live.refresh_graph().unwrap();
    let (_, old_headers, old_body) = call(
        &f.state,
        request("/api/graph/curated", true, "localhost", None),
    )
    .await;

    // New explicit evidence after the archived cache was made must survive
    // repair. Restoring an older saved allocation would lose these seconds.
    let new_explicit = start + Duration::hours(16);
    active(&activity_path, "fixture-new-explicit", new_explicit, 120);
    explicit_segment(&f, "fixture-new-explicit", 2, new_explicit, 120);
    let beta = f
        .state
        .storage
        .project_by_key("beta")
        .unwrap()
        .unwrap()
        .entity_id
        .unwrap();
    let conn = rusqlite::Connection::open(&activity_path).unwrap();
    conn.execute_batch(
        "CREATE TRIGGER fixture_repair_failure BEFORE INSERT ON project_attribution
         BEGIN SELECT RAISE(ABORT,'fixture activity repair failure'); END;",
    )
    .unwrap();
    drop(conn);
    let undo = Some(r#"{"archived":false}"#);
    let (failed, _, _) = call(&f.state, request(&uri, true, "localhost", undo)).await;
    assert_eq!(failed, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(
        f.state
            .storage
            .entity_archive_shelf(true)
            .unwrap()
            .iter()
            .all(|row| row.id != f.outside)
    );
    assert_eq!(activity.project_attribution_for_day(day).unwrap(), initial);
    let (_, failed_headers, failed_body) = call(
        &f.state,
        request("/api/graph/curated", true, "localhost", None),
    )
    .await;
    assert_eq!(failed_body, old_body);
    assert_eq!(failed_headers[header::ETAG], old_headers[header::ETAG]);

    rusqlite::Connection::open(&activity_path)
        .unwrap()
        .execute_batch("DROP TRIGGER fixture_repair_failure;")
        .unwrap();
    let (before, changes) = (f.dump(), f.changes());
    let (status, _, body) = call(&f.state, request(&uri, true, "localhost", undo)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json(&body)["changed"], false);
    assert_eq!(json(&body)["archived"], false);
    assert_eq!(
        activity.project_attribution_for_day(day).unwrap(),
        (
            vec![
                (f.outside.clone(), 300.0, Some("low".into())),
                (f.alpha.clone(), 180.0, Some("high".into())),
                (beta, 120.0, Some("high".into())),
            ],
            0.0
        )
    );
    let (status, headers, repaired) = call(
        &f.state,
        request("/api/graph/curated", true, "localhost", None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_ne!(headers[header::ETAG], old_headers[header::ETAG]);
    let graph = json(&repaired);
    let adopted_key = format!("project:{adopted}");
    let node = graph["n"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node[0] == adopted_key)
        .unwrap();
    assert_eq!(node[5][0], 1);
    assert_eq!(f.dump(), before);
    assert_eq!(f.changes(), changes);
}
