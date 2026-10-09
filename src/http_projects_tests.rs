//! The routes of the registry through the router: with the token they
//! answer, without it they refuse and change nothing.
use super::super::{AppState, TOKEN_HEADER, router};
use crate::embedding::HashEmbedder;
use crate::storage::Storage;
use crate::storage::projects::ProjectState;
use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode, header};
use std::sync::Arc;
use tower::Service;

fn state() -> (crate::test_support::InTempDir<std::path::PathBuf>, AppState) {
    let db = crate::test_support::temp_path("mnemonic-http-projects-", "memory.db");
    let storage = Arc::new(Storage::open(&db).unwrap());
    for name in ["demoapp", "notes"] {
        storage.project_upsert(name, name, None).unwrap();
    }
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

fn post(uri: &str, body: Option<&str>, token: bool) -> Request<Body> {
    let mut builder = Request::builder()
        .method(Method::POST)
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

const MERGE: Option<&str> = Some(r#"{"into": "notes"}"#);

#[tokio::test]
async fn registry_routes_answer_with_the_token() {
    let (_db, state) = state();
    let row = |key: &str| state.storage.project_by_key(key).unwrap().unwrap();

    let (status, json) = call(&state, post("/api/projects/demoapp/archive", None, true)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["key"], "demoapp");
    assert_eq!(json["state"], "archived");
    assert_eq!(row("demoapp").state, ProjectState::Archived);

    let (status, json) = call(&state, post("/api/projects/demoapp/unarchive", None, true)).await;
    assert_eq!(
        (status, json["state"].as_str()),
        (StatusCode::OK, Some("active"))
    );
    assert_eq!(row("demoapp").state, ProjectState::Active);

    let (status, json) = call(&state, post("/api/projects/demoapp/merge", MERGE, true)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["merged_into"], "notes");
    assert_eq!(
        state.storage.project_canonical(row("demoapp").id).unwrap(),
        row("notes").id
    );

    let (status, json) = call(&state, post("/api/projects/demoapp/unmerge", None, true)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(json["merged_into"].is_null());
    assert_eq!(
        state.storage.project_canonical(row("demoapp").id).unwrap(),
        row("demoapp").id
    );
}

struct Silent;

impl crate::live_signal::Post for Silent {
    fn post(&self, _name: &str) {}
}

/// A merge is in the live snapshot when the route answers, not ten
/// minutes later at the registry's next pass.
#[tokio::test]
async fn registry_merge_reaches_the_live_snapshot_with_the_reply() {
    let (_db, mut state) = state();
    let id = |key: &str| state.storage.project_by_key(key).unwrap().unwrap().id;
    let live = crate::live_run::Live::new("boot-a", "a name".to_string(), Box::new(Silent));
    live.load_projects(&state.storage).unwrap();
    let now = chrono::Utc::now();
    for (stream, project) in [("a", id("demoapp")), ("b", id("notes"))] {
        live.observed(
            stream,
            crate::live::Session {
                agent: "claude".into(),
                program: false,
                project: Some(project),
                last_kind: crate::projects::facts::Kind::Call,
                last_at: now,
                started_at: now,
            },
        );
    }
    state.live = Some(live.clone());
    let shown = |live: &crate::live_run::Live| -> Vec<(String, usize)> {
        let body: serde_json::Value = serde_json::from_slice(&live.state.snapshot().1).unwrap();
        body["projects"]
            .as_array()
            .unwrap()
            .iter()
            .map(|project| {
                (
                    project["key"].as_str().unwrap().to_string(),
                    project["sessions"].as_array().unwrap().len(),
                )
            })
            .collect()
    };
    assert_eq!(shown(&live).len(), 2);

    let (status, _) = call(&state, post("/api/projects/demoapp/merge", MERGE, true)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(shown(&live), [("notes".to_string(), 2)]);
}

#[tokio::test]
async fn registry_routes_refuse_without_the_token_and_change_nothing() {
    let (_db, state) = state();
    let before = state.storage.projects().unwrap();
    for (uri, body) in [
        ("/api/projects/demoapp/archive", None),
        ("/api/projects/demoapp/unarchive", None),
        ("/api/projects/demoapp/merge", MERGE),
        ("/api/projects/demoapp/unmerge", None),
    ] {
        let (status, _) = call(&state, post(uri, body, false)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{uri}");
    }
    assert_eq!(state.storage.projects().unwrap(), before);

    // Nor from another host, token or not.
    let mut foreign = post("/api/projects/demoapp/archive", None, true);
    foreign
        .headers_mut()
        .insert(header::HOST, "example.test".parse().unwrap());
    let (status, _) = call(&state, foreign).await;
    assert_eq!(status, StatusCode::MISDIRECTED_REQUEST);
    assert_eq!(state.storage.projects().unwrap(), before);
}

#[tokio::test]
async fn registry_routes_say_what_they_cannot_do() {
    let (_db, state) = state();
    let (status, _) = call(&state, post("/api/projects/nothing/archive", None, true)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let itself = Some(r#"{"into": "demoapp"}"#);
    let (status, _) = call(&state, post("/api/projects/demoapp/merge", itself, true)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let unknown = Some(r#"{"into": "nothing"}"#);
    let (status, _) = call(&state, post("/api/projects/demoapp/merge", unknown, true)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // A merge back onto what was merged into it is refused.
    call(&state, post("/api/projects/demoapp/merge", MERGE, true)).await;
    let back = Some(r#"{"into": "demoapp"}"#);
    let (status, _) = call(&state, post("/api/projects/notes/merge", back, true)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // A key the identity policy refuses is not looked up and not said back.
    let key: String = ["sk-", "proj-", &"a1B2c3D4e5F6".repeat(4)].concat();
    let uri = format!("/api/projects/{key}/archive");
    let (status, json) = call(&state, post(&uri, None, true)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(!json.to_string().contains(&key));
}
