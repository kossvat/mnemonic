//! Router guards, date dispatch and exact read purity for a synthetic day.
use super::super::{AppState, TOKEN_HEADER, router};
use crate::timeline::fixture::{Fixture, at};
use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode, header};
use chrono::Duration;
use serde_json::{Value, json};
use std::sync::Arc;
use tower::Service;

fn state(f: &Fixture, activity: bool) -> AppState {
    AppState {
        storage: f.storage.clone(),
        embedder: Arc::new(crate::embedding::HashEmbedder::new()),
        token: "test-token".into(),
        config: crate::config::Config::default(),
        activity: activity.then(|| f.activity.clone()),
        live: None,
    }
}
fn request(
    method: Method,
    path: &str,
    token: bool,
    host: &str,
    body: Option<Value>,
) -> Request<Body> {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header(header::HOST, host);
    if token {
        request = request.header(TOKEN_HEADER, "test-token");
    }
    if body.is_some() {
        request = request.header(header::CONTENT_TYPE, "application/json");
    }
    request
        .body(body.map_or_else(Body::empty, |value| Body::from(value.to_string())))
        .unwrap()
}
async fn call(state: &AppState, request: Request<Body>) -> (StatusCode, Value) {
    let response = router(state.clone()).call(request).await.unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (status, serde_json::from_slice(&body).unwrap_or_default())
}
async fn get(state: &AppState, path: &str) -> (StatusCode, Value) {
    call(state, request(Method::GET, path, true, "localhost", None)).await
}
const DAY_ROUTE: &str = "/api/projects/timeline?day=2026-09-29";

#[tokio::test]
async fn timeline_router_reads_both_stores_without_changing_recall_or_any_row_or_file() {
    let f = Fixture::full();
    let state = state(&f, true);
    let before = f.stores();
    let files = f.files();
    let changes = f.changes();
    let (status, value) = get(&state, DAY_ROUTE).await;
    assert_eq!(status, StatusCode::OK);
    let expected = crate::timeline::read(
        &f.storage,
        Some(&f.activity),
        &crate::owner_time_day::LocalCalendar,
        at(0, 0).date_naive(),
        chrono::Utc::now(),
        &crate::attribution::AttribCfg::default(),
    )
    .unwrap();
    assert_eq!(value, serde_json::to_value(expected).unwrap());
    assert_eq!(get(&state, DAY_ROUTE).await.1, value);
    assert_eq!(value["segments"].as_array().unwrap().len(), 4);
    assert!(value["owner_available"].as_bool().unwrap());
    assert_eq!(f.stores(), before);
    assert_eq!(f.files(), files);
    assert_eq!(f.changes(), changes);
    assert_eq!(before[0]["recall_events"].len(), 1);
    assert_eq!(before[0]["recall_items"].len(), 1);
}

#[tokio::test]
async fn timeline_router_refuses_missing_token_and_foreign_host_even_for_bad_day() {
    let f = Fixture::full();
    let state = state(&f, true);
    let before = f.stores();
    let files = f.files();
    let changes = f.changes();
    for path in [
        DAY_ROUTE,
        "/api/projects/timeline?day=bad",
        "/api/projects/timeline",
    ] {
        assert_eq!(
            call(&state, request(Method::GET, path, false, "localhost", None))
                .await
                .0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            call(&state, request(Method::GET, path, true, "elsewhere", None))
                .await
                .0,
            StatusCode::MISDIRECTED_REQUEST
        );
    }
    assert_eq!(f.stores(), before);
    assert_eq!(f.files(), files);
    assert_eq!(f.changes(), changes);
}

#[tokio::test]
async fn timeline_router_distinguishes_absent_day_from_present_invalid_day_and_project_key() {
    let f = Fixture::empty();
    f.storage
        .project_upsert("timeline", "Timeline project", None)
        .unwrap();
    let state = state(&f, true);
    let before = f.stores();
    let changes = f.changes();
    for path in [
        "/api/projects/timeline",
        "/api/projects/timeline?other=value",
    ] {
        let (status, value) = get(&state, path).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(value["key"], "timeline");
        assert!(value.get("day").is_none());
    }
    for query in [
        "day",
        "day=",
        "day=bad",
        "day=2026-02-29",
        "day=2026-9-29",
        "day=0000-01-01",
        "day=2026-09-29&day=2026-09-29",
        "day=2026-09-29%20",
        "day=%00",
    ] {
        assert_eq!(
            get(&state, &format!("/api/projects/timeline?{query}"))
                .await
                .0,
            StatusCode::BAD_REQUEST,
            "{query}"
        );
    }
    assert_eq!(
        get(&state, "/api/projects/timeline?%64ay=2024-02-29")
            .await
            .0,
        StatusCode::OK
    );
    let value = get(&state, DAY_ROUTE).await.1;
    assert_eq!(value["day"], "2026-09-29");
    assert!(value.get("key").is_none());
    assert_eq!(f.stores(), before);
    assert_eq!(f.changes(), changes);
}

#[tokio::test]
async fn timeline_router_reports_absent_activity_and_keeps_agent_segments() {
    let f = Fixture::full();
    let (status, value) = get(&state(&f, false), DAY_ROUTE).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["owner_available"], false);
    assert!(value["owner"].as_array().unwrap().is_empty());
    assert_eq!(value["segments"].as_array().unwrap().len(), 4);
    let absent = Fixture::empty();
    assert_eq!(
        get(&state(&absent, false), "/api/projects/timeline")
            .await
            .0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn timeline_router_post_correction_and_delete_undo_change_the_next_read_only() {
    let f = Fixture::full();
    let state = state(&f, true);
    let original = get(&state, DAY_ROUTE).await.1;
    let body = json!({"stream": "fixture-claude", "from": at(9, 5).to_rfc3339(),
        "to": at(9, 10).to_rfc3339(), "project": "notes"});
    let (status, added) = call(
        &state,
        request(
            Method::POST,
            "/api/attribution/overrides",
            true,
            "localhost",
            Some(body),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let id = added["id"].as_i64().unwrap();
    let after_write = f.stores();
    let changes = f.changes();
    let corrected = get(&state, DAY_ROUTE).await.1;
    assert_ne!(corrected, original);
    let pieces = corrected["segments"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|segment| segment["stream"] == "fixture-claude")
        .collect::<Vec<_>>();
    assert_eq!(pieces.len(), 3);
    assert_eq!(pieces[1]["project"], "notes");
    assert_eq!(f.stores(), after_write);
    assert_eq!(f.changes(), changes);
    let undo = format!("/api/attribution/overrides/{id}");
    assert_eq!(
        call(
            &state,
            request(Method::DELETE, &undo, true, "localhost", None)
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(get(&state, DAY_ROUTE).await.1, original);
}

#[tokio::test]
async fn timeline_router_prepares_legacy_names_and_refuses_unsafe_streams_without_echoing_paths() {
    let f = Fixture::empty();
    let path = f
        .memory_path
        .with_file_name("transcript")
        .to_string_lossy()
        .into_owned();
    f.segment(&path, "claude", false, Some(f.demo), at(9, 0), at(9, 30));
    f.segment(
        "safe-stream",
        "custom",
        true,
        Some(f.demo),
        at(9, 0),
        at(9, 30),
    );
    let secret = format!("sk-{}", "AbCd1234".repeat(6));
    f.storage
        .conn
        .lock()
        .unwrap()
        .execute(
            "UPDATE projects SET name = ?1 WHERE id = ?2",
            rusqlite::params![format!("Fixture {secret}"), f.demo],
        )
        .unwrap();
    let before = f.stores();
    let value = get(&state(&f, false), DAY_ROUTE).await.1;
    let shown = value.to_string();
    assert!(!shown.contains(&path) && !shown.contains(&secret));
    assert_eq!(value["segments"].as_array().unwrap().len(), 1);
    assert_eq!(value["segments"][0]["agent"], "unknown");
    assert_eq!(f.stores(), before);
    // Future records do not appear on a future day before that day begins.
    let tomorrow = chrono::Local::now().date_naive() + Duration::days(1);
    let value = get(
        &state(&f, true),
        &format!("/api/projects/timeline?day={tomorrow}"),
    )
    .await
    .1;
    assert!(value["segments"].as_array().unwrap().is_empty());
    assert!(value["owner"].as_array().unwrap().is_empty());
}

#[cfg(unix)]
#[path = "http_timeline_zone_tests.rs"]
mod zones;
