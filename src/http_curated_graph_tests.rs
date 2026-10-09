use super::super::{AppState, TOKEN_HEADER, router};
use crate::embedding::HashEmbedder;
use crate::event::{EventSource, MemoryEntry, MemoryType};
use crate::graph::{Entity, EntityType};
use crate::storage::Storage;
use axum::body::{Body, to_bytes};
use axum::http::{HeaderMap, Method, Request, StatusCode, header};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Arc;
use tower::Service;

#[path = "http_curated_graph_archive_orphan_tests.rs"]
mod archive_orphan_tests;
#[path = "http_curated_graph_archive_page_tests.rs"]
mod archive_page_tests;
#[path = "http_curated_graph_archive_retry_tests.rs"]
mod archive_retry_tests;
#[path = "http_curated_graph_archive_time_tests.rs"]
mod archive_time_tests;
#[path = "http_curated_graph_bounds_tests.rs"]
mod bounds_tests;
#[path = "http_curated_graph_evidence_page_tests.rs"]
mod evidence_page_tests;
#[path = "http_curated_graph_key_tests.rs"]
mod key_tests;
#[path = "http_curated_graph_registry_tests.rs"]
mod registry_tests;
#[path = "http_curated_graph_fixture.rs"]
mod shared_fixture;

struct Silent;
impl crate::live_signal::Post for Silent {
    fn post(&self, _: &str) {}
}

struct Fixture {
    temp: tempfile::TempDir,
    state: AppState,
    alpha: String,
    tool: String,
    outside: String,
    memories: Vec<String>,
}

impl Fixture {
    fn new(count: usize) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let storage = Arc::new(Storage::open(&temp.path().join("memory.db")).unwrap());
        let workspace = temp.path().join("workspace");
        let mut entities = Vec::new();
        for (name, kind) in [
            ("alpha", EntityType::Project),
            ("beta", EntityType::Project),
            ("rust", EntityType::Tech),
            ("outside-project", EntityType::Project),
        ] {
            let id = storage
                .upsert_entity(&Entity {
                    name: name.into(),
                    entity_type: kind,
                })
                .unwrap();
            if matches!(name, "alpha" | "beta") {
                std::fs::create_dir_all(workspace.join("projects").join(name)).unwrap();
                let project = storage.project_upsert(name, name, None).unwrap();
                storage.project_link_entity(project, name).unwrap();
            }
            entities.push(id);
        }
        let mut memories = Vec::new();
        for n in 0..count {
            let entry = MemoryEntry::new(
                format!("Fixture decision {n}"),
                "Shared fixture evidence",
                MemoryType::Decision,
                EventSource::Manual,
            );
            storage.save(&entry).unwrap();
            for entity in &entities[..3] {
                storage.link_memory_entity(&entry.id, entity).unwrap();
            }
            memories.push(entry.id);
        }
        {
            let conn = storage.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO edges(id,source_entity,target_entity,relation,memory_id)
                VALUES ('fixture-edge',?1,?2,'mentions',?3)",
                rusqlite::params![entities[0], entities[3], memories[0]],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO recall_events
                (id,at,surface,tool,client,run,returned,corrections,chars,hidden)
                VALUES (1,'2001-01-01T00:00:00Z','mcp','memory_search',NULL,'fixture',1,0,20,0)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO recall_items(event_id,rank,memory_id,section) VALUES(1,0,?1,NULL)",
                [&memories[0]],
            )
            .unwrap();
        }
        memories.sort();
        let mut config = crate::config::Config::default();
        config.projects.workspace = Some(workspace);
        config.projects.folder = Some("projects".into());
        config.graph.tech = vec!["rust".into()];
        let live =
            crate::live_run::Live::new("curated-test", "fixture signal".into(), Box::new(Silent));
        live.configure_graph(storage.clone(), &config).unwrap();
        live.refresh_graph().unwrap();
        let state = AppState {
            storage,
            embedder: Arc::new(HashEmbedder::new()),
            token: "test-token".into(),
            config,
            activity: None,
            live: Some(live),
        };
        Self {
            temp,
            state,
            alpha: entities[0].clone(),
            tool: entities[2].clone(),
            outside: entities[3].clone(),
            memories,
        }
    }
    fn changes(&self) -> u64 {
        self.state.storage.conn.lock().unwrap().total_changes()
    }
    fn files(&self) -> Vec<Option<Vec<u8>>> {
        ["memory.db", "memory.db-wal"]
            .map(|name| std::fs::read(self.temp.path().join(name)).ok())
            .into()
    }
    fn dump(&self) -> BTreeMap<String, Vec<String>> {
        let conn = self.state.storage.conn.lock().unwrap();
        let mut names = conn
            .prepare("SELECT name FROM sqlite_schema WHERE type='table' ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        names.push("sqlite_schema".into());
        names
            .into_iter()
            .map(|name| {
                let mut query = conn
                    .prepare(&format!("SELECT * FROM \"{}\"", name.replace('"', "\"\"")))
                    .unwrap();
                let columns = query.column_count();
                let mut rows = query
                    .query_map([], |row| {
                        let values = (0..columns)
                            .map(|column| row.get::<_, rusqlite::types::Value>(column))
                            .collect::<rusqlite::Result<Vec<_>>>()?;
                        Ok(format!("{values:?}"))
                    })
                    .unwrap()
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .unwrap();
                rows.sort();
                (name, rows)
            })
            .collect()
    }
}

fn request(uri: &str, token: bool, host: &str, body: Option<&str>) -> Request<Body> {
    let mut request = Request::builder()
        .method(if body.is_some() {
            Method::POST
        } else {
            Method::GET
        })
        .uri(uri)
        .header(header::HOST, host);
    if token {
        request = request.header(TOKEN_HEADER, "test-token");
    }
    if body.is_some() {
        request = request.header(header::CONTENT_TYPE, "application/json");
    }
    request
        .body(body.map_or_else(Body::empty, |body| Body::from(body.to_string())))
        .unwrap()
}

async fn call(state: &AppState, request: Request<Body>) -> (StatusCode, HeaderMap, Vec<u8>) {
    let response = router(state.clone()).call(request).await.unwrap();
    let (status, headers) = (response.status(), response.headers().clone());
    let body = to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap()
        .to_vec();
    (status, headers, body)
}

fn json(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).unwrap()
}

#[tokio::test]
async fn curated_graph_http_guards_every_route_without_mutating_any_store() {
    let f = Fixture::new(3);
    let (before, changes, files) = (f.dump(), f.changes(), f.files());
    let archive = format!("/api/entities/{}/archive", f.outside);
    for (uri, body) in [
        ("/api/graph/curated", None),
        (
            "/api/graph/curated/edge?source=project:1&target=project:2&kind=relates",
            None,
        ),
        ("/api/graph/curated/archives", None),
        (archive.as_str(), Some(r#"{"archived":true}"#)),
    ] {
        assert_eq!(
            call(&f.state, request(uri, false, "localhost", body))
                .await
                .0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            call(&f.state, request(uri, true, "elsewhere", body))
                .await
                .0,
            StatusCode::MISDIRECTED_REQUEST
        );
    }
    assert_eq!(f.dump(), before);
    assert_eq!(f.changes(), changes);
    assert_eq!(f.files(), files);
}

#[tokio::test]
async fn curated_graph_http_gets_etags_and_expansion_leave_bytes_and_recall_unchanged() {
    let f = Fixture::new(3);
    let (before, changes, files) = (f.dump(), f.changes(), f.files());
    let (status, headers, bytes) = call(
        &f.state,
        request("/api/graph/curated", true, "localhost", None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let tag = headers[header::ETAG].to_str().unwrap();
    for held in [tag.to_string(), format!("\"older\", W/{tag}"), "*".into()] {
        let mut req = request("/api/graph/curated", true, "localhost", None);
        req.headers_mut()
            .insert(header::IF_NONE_MATCH, held.parse().unwrap());
        let (status, headers, body) = call(&f.state, req).await;
        assert_eq!(status, StatusCode::NOT_MODIFIED);
        assert!(body.is_empty());
        assert_eq!(headers[header::ETAG].to_str().unwrap(), tag);
    }
    let (_, repeated, repeated_bytes) = call(
        &f.state,
        request("/api/graph/curated", true, "localhost", None),
    )
    .await;
    assert_eq!(repeated_bytes, bytes);
    assert_eq!(repeated[header::ETAG], headers[header::ETAG]);
    for uri in [
        "/api/graph/curated?expand=project:1",
        "/api/graph/curated/edge?source=project:1&target=project:2&kind=relates&limit=2",
        "/api/graph/curated/archives?offset=0&limit=10",
    ] {
        assert_eq!(
            call(&f.state, request(uri, true, "localhost", None))
                .await
                .0,
            StatusCode::OK
        );
    }
    assert_eq!(before["recall_events"].len(), 1);
    assert_eq!(before["recall_items"].len(), 1);
    assert_eq!(f.dump(), before);
    assert_eq!(f.changes(), changes);
    assert_eq!(f.files(), files);
}

#[tokio::test]
async fn curated_graph_http_pages_evidence_and_rejects_invalid_inputs() {
    let f = Fixture::new(110);
    let (before, changes) = (f.dump(), f.changes());
    let path = "/api/graph/curated/edge?source=project:1&target=project:2&kind=relates";
    let (_, _, first) = call(
        &f.state,
        request(&format!("{path}&limit=999"), true, "localhost", None),
    )
    .await;
    let (_, _, second) = call(
        &f.state,
        request(
            &format!("{path}&offset=100&limit=100"),
            true,
            "localhost",
            None,
        ),
    )
    .await;
    let (first, second) = (json(&first), json(&second));
    assert_eq!(first["memory_ids"].as_array().unwrap().len(), 100);
    assert_eq!(first["next_offset"], 100);
    assert_eq!(second["memory_ids"].as_array().unwrap().len(), 10);
    assert!(second["next_offset"].is_null());
    let ids: Vec<_> = first["memory_ids"]
        .as_array()
        .unwrap()
        .iter()
        .chain(second["memory_ids"].as_array().unwrap())
        .map(|value| value.as_str().unwrap().to_string())
        .collect();
    assert_eq!(ids, f.memories);
    for (uri, expected) in [
        (
            "/api/graph/curated?expand=project:999",
            StatusCode::NOT_FOUND,
        ),
        ("/api/graph/curated?expand=bad", StatusCode::BAD_REQUEST),
        (
            "/api/graph/curated?expand=project:1&expand=project:2",
            StatusCode::BAD_REQUEST,
        ),
        (
            "/api/graph/curated/edge?source=project:1&target=project:2&kind=mentions",
            StatusCode::BAD_REQUEST,
        ),
        (
            "/api/graph/curated/archives?limit=0",
            StatusCode::BAD_REQUEST,
        ),
        (
            "/api/graph/curated/archives?offset=-1",
            StatusCode::BAD_REQUEST,
        ),
    ] {
        assert_eq!(
            call(&f.state, request(uri, true, "localhost", None))
                .await
                .0,
            expected
        );
    }
    let secret = format!("{}{}", "ghp_", "x".repeat(36));
    let path = format!("/api/graph/curated?expand=entity:{secret}");
    let (status, _, body) = call(&f.state, request(&path, true, "localhost", None)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(!String::from_utf8(body).unwrap().contains(&secret));
    assert_eq!(f.dump(), before);
    assert_eq!(f.changes(), changes);
}

#[tokio::test]
async fn curated_graph_http_archive_and_undo_preserve_entities_links_and_recall() {
    let f = Fixture::new(3);
    let (before, changes) = (f.dump(), f.changes());
    let (_, old_headers, old_body) = call(
        &f.state,
        request("/api/graph/curated", true, "localhost", None),
    )
    .await;
    let uri = format!("/api/entities/{}/archive", f.outside);
    let (status, _, body) = call(
        &f.state,
        request(&uri, true, "localhost", Some(r#"{"archived":true}"#)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json(&body)["archived"], true);
    let archived_changes = f.changes();
    let (_, _, repeat) = call(
        &f.state,
        request(&uri, true, "localhost", Some(r#"{"archived":true}"#)),
    )
    .await;
    assert_eq!(json(&repeat)["changed"], false);
    assert_eq!(f.changes(), archived_changes);
    let (_, archived_headers, archived_body) = call(
        &f.state,
        request("/api/graph/curated", true, "localhost", None),
    )
    .await;
    assert_ne!(old_headers[header::ETAG], archived_headers[header::ETAG]);
    assert!(
        json(&archived_body)["s"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row[0] != format!("entity:{}", f.outside))
    );
    let (_, _, archives) = call(
        &f.state,
        request("/api/graph/curated/archives", true, "localhost", None),
    )
    .await;
    assert_eq!(json(&archives)["entities"][0]["id"], f.outside);
    let (status, _, body) = call(
        &f.state,
        request(&uri, true, "localhost", Some(r#"{"archived":false}"#)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json(&body)["archived"], false);
    let (_, _, restored) = call(
        &f.state,
        request("/api/graph/curated", true, "localhost", None),
    )
    .await;
    assert_eq!(restored, old_body);
    assert_eq!(f.dump(), before);
    assert_eq!(f.changes(), changes + 2);
    for id in [f.alpha.as_str(), f.tool.as_str(), "missing"] {
        let uri = format!("/api/entities/{id}/archive");
        assert_eq!(
            call(
                &f.state,
                request(&uri, true, "localhost", Some(r#"{"archived":true}"#))
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
    }
    assert_eq!(
        call(&f.state, request(&uri, true, "localhost", Some("{}")))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn curated_graph_http_serves_the_exact_shared_swift_fixture() {
    let f = Fixture::new(3);
    let snapshot = shared_fixture::swift_curated_fixture();
    let expected = include_bytes!("../clients/macos/Tests/Fixtures/curated-graph.json");
    assert_eq!(snapshot.bytes.as_slice(), expected);
    let tag = snapshot.etag.clone();
    let (before, changes, files) = (f.dump(), f.changes(), f.files());
    f.state
        .live
        .as_ref()
        .unwrap()
        .set_graph_snapshot(None, Arc::new(snapshot));
    let (status, headers, body) = call(
        &f.state,
        request("/api/graph/curated", true, "localhost", None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.as_slice(), expected);
    assert_eq!(headers[header::ETAG].to_str().unwrap(), tag);
    assert_eq!(f.dump(), before);
    assert_eq!(f.changes(), changes);
    assert_eq!(f.files(), files);
}
