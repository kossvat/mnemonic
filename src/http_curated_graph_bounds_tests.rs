use super::*;

#[tokio::test]
async fn curated_graph_http_reports_unavailable_cache_without_starting_work() {
    let mut f = Fixture::new(3);
    f.state.live = None;
    let (before, changes, files) = (f.dump(), f.changes(), f.files());
    for uri in [
        "/api/graph/curated",
        "/api/graph/curated?expand=project:1",
        "/api/graph/curated/edge?source=project:1&target=project:2&kind=relates",
    ] {
        assert_eq!(
            call(&f.state, request(uri, true, "localhost", None))
                .await
                .0,
            StatusCode::SERVICE_UNAVAILABLE
        );
    }
    assert_eq!(f.dump(), before);
    assert_eq!(f.changes(), changes);
    assert_eq!(f.files(), files);
}

#[tokio::test]
async fn curated_graph_http_archives_are_bounded_redacted_and_read_only() {
    let f = Fixture::new(3);
    let secret = format!("{}{}", "ghp_", "x".repeat(36));
    for n in 0..105 {
        let id = f
            .state
            .storage
            .upsert_entity(&Entity {
                name: format!("Archived fixture {n}"),
                entity_type: EntityType::Project,
            })
            .unwrap();
        f.state
            .storage
            .entity_archive_set(&id, true, 100 + n)
            .unwrap();
        if n == 0 {
            f.state
                .storage
                .conn
                .lock()
                .unwrap()
                .execute(
                    "UPDATE entities SET name=?1 WHERE id=?2",
                    rusqlite::params![format!("Old label {secret}"), id],
                )
                .unwrap();
        }
    }
    let (before, changes, files) = (f.dump(), f.changes(), f.files());
    let (status, _, first) = call(
        &f.state,
        request(
            "/api/graph/curated/archives?limit=999",
            true,
            "localhost",
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(!String::from_utf8_lossy(&first).contains(&secret));
    let first = json(&first);
    assert_eq!(first["entities"].as_array().unwrap().len(), 100);
    assert_eq!(first["next_offset"], 100);
    assert_eq!(first["total"], 105);
    let (status, _, second) = call(
        &f.state,
        request(
            "/api/graph/curated/archives?offset=100&limit=100",
            true,
            "localhost",
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(!String::from_utf8_lossy(&second).contains(&secret));
    let second = json(&second);
    assert_eq!(second["entities"].as_array().unwrap().len(), 5);
    assert!(second["next_offset"].is_null());
    let mut ids = first["entities"]
        .as_array()
        .unwrap()
        .iter()
        .chain(second["entities"].as_array().unwrap())
        .map(|row| row["id"].as_str().unwrap())
        .collect::<Vec<_>>();
    let old_len = ids.len();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), old_len);
    assert!(f.dump() == before);
    assert_eq!(f.changes(), changes);
    assert!(f.files() == files);
}

#[tokio::test]
async fn curated_graph_http_keeps_overview_and_expansion_within_wire_budgets() {
    let f = Fixture::new(3);
    for id in 3..=160 {
        let name = format!("Fixture {id}");
        let key = format!("fixture-{id}");
        let project = f.state.storage.project_upsert(&key, &name, None).unwrap();
        let entity = f
            .state
            .storage
            .upsert_entity(&Entity {
                name: name.clone(),
                entity_type: EntityType::Project,
            })
            .unwrap();
        f.state.storage.project_link_entity(project, &name).unwrap();
        for memory in &f.memories {
            f.state.storage.link_memory_entity(memory, &entity).unwrap();
        }
    }
    f.state.live.as_ref().unwrap().refresh_graph().unwrap();
    let (before, changes, files) = (f.dump(), f.changes(), f.files());
    for (uri, nodes, edges, budget) in [
        ("/api/graph/curated", 60, 150, 12_000),
        ("/api/graph/curated?expand=project:1", 150, 375, 35_000),
    ] {
        let (status, _, body) = call(&f.state, request(uri, true, "localhost", None)).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.len() <= budget);
        let value = json(&body);
        assert_eq!(value["n"].as_array().unwrap().len(), nodes);
        assert_eq!(value["e"].as_array().unwrap().len(), edges);
        assert!(value["o"][0].as_u64().unwrap() > 0);
        for edge in value["e"].as_array().unwrap() {
            assert!(edge[0].as_u64().unwrap() < nodes as u64);
            assert!(edge[1].as_u64().unwrap() < nodes as u64);
        }
    }
    assert_eq!(f.dump(), before);
    assert_eq!(f.changes(), changes);
    assert_eq!(f.files(), files);
}
