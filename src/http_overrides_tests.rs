//! The routes of corrections through the router: with the token they
//! answer, without it they refuse and change nothing.
use super::super::{AppState, TOKEN_HEADER, router};
use crate::embedding::HashEmbedder;
use crate::storage::Storage;
use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode, header};
use std::sync::Arc;
use tower::Service;

fn state() -> (crate::test_support::InTempDir<std::path::PathBuf>, AppState) {
    let db = crate::test_support::temp_path("mnemonic-http-overrides-", "memory.db");
    let storage = Arc::new(Storage::open(&db).unwrap());
    storage.project_upsert("demoapp", "demoapp", None).unwrap();
    let state = AppState {
        storage,
        embedder: Arc::new(HashEmbedder::new()),
        token: "test-token".to_string(),
        config: crate::config::Config::default(),
        activity: None,
        live: None,
    };
    (db, state)
}

fn request(method: Method, uri: &str, body: Option<&str>, token: bool) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::HOST, "localhost");
    if token {
        builder = builder.header(TOKEN_HEADER, "test-token");
    }
    if body.is_some() {
        builder = builder.header(header::CONTENT_TYPE, "application/json");
    }
    builder
        .body(body.map_or_else(Body::empty, |body| Body::from(body.to_string())))
        .unwrap()
}

async fn call(state: &AppState, request: Request<Body>) -> (StatusCode, serde_json::Value) {
    let response = router(state.clone()).call(request).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or_default())
}

const ROUTE: &str = "/api/attribution/overrides";
const PIN: &str = r#"{"stream": "[\"conversation\",\"a.jsonl\"]", "project": "demoapp"}"#;
const RANGE: &str =
    r#"{"from": "2026-09-30T10:00:00Z", "to": "2026-09-30T11:00:00Z", "project": null}"#;

#[tokio::test]
async fn override_routes_keep_a_correction_and_undo_it_with_the_token() {
    let (_db, state) = state();
    let project = state.storage.project_by_key("demoapp").unwrap().unwrap().id;

    let (status, pin) = call(&state, request(Method::POST, ROUTE, Some(PIN), true)).await;
    assert_eq!(status, StatusCode::OK);
    let (status, range) = call(&state, request(Method::POST, ROUTE, Some(RANGE), true)).await;
    assert_eq!(status, StatusCode::OK);
    let kept = state.storage.overrides().unwrap();
    assert_eq!(kept.len(), 2);
    assert_eq!(
        (
            kept[0].id,
            kept[0].stream.as_deref(),
            kept[0].range,
            kept[0].project
        ),
        (
            pin["id"].as_i64().unwrap(),
            Some(r#"["conversation","a.jsonl"]"#),
            None,
            Some(project)
        )
    );
    // No project is said with `null`, for the owner's own time.
    assert_eq!((kept[1].stream.as_deref(), kept[1].project), (None, None));
    assert!(kept[1].range.is_some());
    // The range marked its day for a new split.
    assert_eq!(state.storage.attribution_dirty().unwrap().len(), 1);

    let undo = format!("{ROUTE}/{}", range["id"]);
    let (status, json) = call(&state, request(Method::DELETE, &undo, None, true)).await;
    assert_eq!(
        (status, json["removed"].as_i64()),
        (StatusCode::OK, range["id"].as_i64())
    );
    assert_eq!(state.storage.overrides().unwrap().len(), 1);
    // Undone once: there is nothing to undo again.
    let (status, _) = call(&state, request(Method::DELETE, &undo, None, true)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn override_routes_refuse_without_the_token_and_change_nothing() {
    let (_db, state) = state();
    let (_, kept) = call(&state, request(Method::POST, ROUTE, Some(PIN), true)).await;
    let undo = format!("{ROUTE}/{}", kept["id"]);
    let before = state.storage.overrides().unwrap();
    for (method, uri, body) in [
        (Method::POST, ROUTE, Some(RANGE)),
        (Method::DELETE, undo.as_str(), None),
    ] {
        let (status, _) = call(&state, request(method, uri, body, false)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{uri}");
    }
    assert_eq!(state.storage.overrides().unwrap(), before);
    // Nor from another host, token or not.
    let mut foreign = request(Method::DELETE, &undo, None, true);
    foreign
        .headers_mut()
        .insert(header::HOST, "example.test".parse().unwrap());
    let (status, _) = call(&state, foreign).await;
    assert_eq!(status, StatusCode::MISDIRECTED_REQUEST);
    assert_eq!(state.storage.overrides().unwrap(), before);
}

#[tokio::test]
async fn override_routes_say_what_they_cannot_keep() {
    let (_db, state) = state();
    let post = |body: &str| request(Method::POST, ROUTE, Some(body), true);
    // Neither a transcript nor a range; half a range; a range backwards.
    for body in [
        r#"{"project": "demoapp"}"#,
        r#"{"from": "2026-09-30T10:00:00Z", "project": null}"#,
        r#"{"from": "2026-09-30T11:00:00Z", "to": "2026-09-30T10:00:00Z", "project": null}"#,
    ] {
        let (status, _) = call(&state, post(body)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    }
    // The project has to be said, as a key or as null.
    let (status, _) = call(&state, post(r#"{"stream": "s"}"#)).await;
    assert!(status.is_client_error());
    let unknown = r#"{"stream": "s", "project": "nothing"}"#;
    let (status, _) = call(&state, post(unknown)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // A name the identity policy refuses is not kept and not said back.
    let key: String = ["sk-", "proj-", &"a1B2c3D4e5F6".repeat(4)].concat();
    let body = serde_json::json!({"stream": key, "project": null}).to_string();
    let (status, json) = call(&state, post(&body)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(!json.to_string().contains(&key));
    assert!(state.storage.overrides().unwrap().is_empty());
}

/// A signal that goes nowhere.
struct Silent;

impl crate::live_signal::Post for Silent {
    fn post(&self, _name: &str) {}
}

/// The count of memories is the store's at the moment of the request, so
/// a client that asks after a signal shows the count as it is.
#[tokio::test]
async fn live_route_counts_the_memories_of_the_store_at_each_request() {
    use crate::event::{EventSource, MemoryEntry, MemoryType};
    let (_db, mut state) = state();
    let live = crate::live_run::Live::new("boot-a", "a name".to_string(), Box::new(Silent));
    state.live = Some(live.clone());

    let (status, first) = call(&state, request(Method::GET, "/api/live", None, true)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(first["memories"], 0);

    let note = MemoryEntry::new("a note", "kept", MemoryType::Note, EventSource::Manual);
    state.storage.save(&note).unwrap();
    let (_, second) = call(&state, request(Method::GET, "/api/live", None, true)).await;
    assert_eq!(second["memories"], 1);
    assert_ne!(first["v"], second["v"]);

    // Nothing was saved since: the same snapshot.
    let (_, third) = call(&state, request(Method::GET, "/api/live", None, true)).await;
    assert_eq!(third, second);
}

#[tokio::test]
async fn live_route_serves_the_snapshot_with_a_tag_and_304_for_the_tag_it_gave() {
    let (_db, mut state) = state();
    // Without a live state the route says so.
    let (status, _) = call(&state, request(Method::GET, "/api/live", None, true)).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);

    let live = crate::live_run::Live::new("boot-a", "a name".to_string(), Box::new(Silent));
    live.load_projects(&state.storage).unwrap();
    state.live = Some(live.clone());
    let changes = |state: &AppState| state.storage.conn.lock().unwrap().total_changes();
    let before = changes(&state);

    let response = router(state.clone())
        .call(request(Method::GET, "/api/live", None, true))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let tag = response.headers()[header::ETAG]
        .to_str()
        .unwrap()
        .to_string();
    assert_eq!(tag, live.state.snapshot().0);
    let bytes = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
    assert_eq!(bytes.as_ref(), live.state.snapshot().1.as_slice());

    // The tag it gave: 304, the tag again, and no body.
    let mut again = request(Method::GET, "/api/live", None, true);
    again
        .headers_mut()
        .insert(header::IF_NONE_MATCH, tag.parse().unwrap());
    let response = router(state.clone()).call(again).await.unwrap();
    assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
    assert_eq!(response.headers()[header::ETAG].to_str().unwrap(), tag);
    assert!(
        to_bytes(response.into_body(), 1024)
            .await
            .unwrap()
            .is_empty()
    );

    // The snapshot changed: the tag held is of an earlier one.
    let session = crate::live::Session {
        agent: "claude".into(),
        program: false,
        project: state
            .storage
            .project_by_key("demoapp")
            .unwrap()
            .map(|row| row.id),
        last_kind: crate::projects::facts::Kind::Call,
        last_at: chrono::Utc::now(),
        started_at: chrono::Utc::now(),
    };
    live.observed("s", session);
    let mut stale = request(Method::GET, "/api/live", None, true);
    stale
        .headers_mut()
        .insert(header::IF_NONE_MATCH, tag.parse().unwrap());
    let (status, json) = call(&state, stale).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["projects"][0]["key"], "demoapp");

    // Behind the token, and nothing in the store was changed by asking.
    let (status, _) = call(&state, request(Method::GET, "/api/live", None, false)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(changes(&state), before);
}

#[tokio::test]
async fn live_route_lets_a_page_send_the_tag_it_holds_and_read_the_tag_it_is_given() {
    let (_db, mut state) = state();
    let live = crate::live_run::Live::new("boot-a", "a name".to_string(), Box::new(Silent));
    state.live = Some(live);
    let origin = state.config.ui.cors_origins[0].clone();
    // The question a browser asks before a request that carries a tag.
    let preflight = Request::builder()
        .method(Method::OPTIONS)
        .uri("/api/live")
        .header(header::HOST, "localhost")
        .header(header::ORIGIN, &origin)
        .header(header::ACCESS_CONTROL_REQUEST_METHOD, "GET")
        .header(
            header::ACCESS_CONTROL_REQUEST_HEADERS,
            "if-none-match, x-mnemonic-token",
        )
        .body(Body::empty())
        .unwrap();
    let response = router(state.clone()).call(preflight).await.unwrap();
    assert!(response.status().is_success());
    let allowed = response.headers()[header::ACCESS_CONTROL_ALLOW_HEADERS]
        .to_str()
        .unwrap()
        .to_lowercase();
    assert!(allowed.contains("if-none-match") && allowed.contains("x-mnemonic-token"));
    // And the tag of the answer is one the page may read.
    let mut asked = request(Method::GET, "/api/live", None, true);
    asked
        .headers_mut()
        .insert(header::ORIGIN, origin.parse().unwrap());
    let response = router(state.clone()).call(asked).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let exposed = response.headers()[header::ACCESS_CONTROL_EXPOSE_HEADERS]
        .to_str()
        .unwrap()
        .to_lowercase();
    assert!(exposed.contains("etag"));
}
