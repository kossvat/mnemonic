use super::*;

#[tokio::test]
async fn curated_graph_http_idempotent_archive_retry_repairs_failed_refresh() {
    let f = Fixture::new(3);
    let uri = format!("/api/entities/{}/archive", f.outside);
    let key = format!("entity:{}", f.outside);
    for archived in [true, false] {
        let (_, old_headers, old_body) = call(
            &f.state,
            request("/api/graph/curated", true, "localhost", None),
        )
        .await;
        let body = serde_json::json!({"archived": archived}).to_string();
        // Archive state is independent of graph slots: the write commits,
        // then refresh reaches this missing table and fails.
        f.state
            .storage
            .conn
            .lock()
            .unwrap()
            .execute(
                "ALTER TABLE graph_slots RENAME TO fixture_saved_graph_slots",
                [],
            )
            .unwrap();
        let (failed, _, _) = call(&f.state, request(&uri, true, "localhost", Some(&body))).await;
        f.state
            .storage
            .conn
            .lock()
            .unwrap()
            .execute(
                "ALTER TABLE fixture_saved_graph_slots RENAME TO graph_slots",
                [],
            )
            .unwrap();
        assert_eq!(failed, StatusCode::INTERNAL_SERVER_ERROR);
        let stored = f
            .state
            .storage
            .entity_archive_shelf(true)
            .unwrap()
            .into_iter()
            .find(|row| row.id == f.outside)
            .unwrap();
        assert_eq!(stored.archived_at.is_some(), archived);
        let (_, cached_headers, cached_body) = call(
            &f.state,
            request("/api/graph/curated", true, "localhost", None),
        )
        .await;
        assert_eq!(cached_body, old_body);
        assert_eq!(cached_headers[header::ETAG], old_headers[header::ETAG]);

        let (before_retry, changes) = (f.dump(), f.changes());
        let (status, _, retried) =
            call(&f.state, request(&uri, true, "localhost", Some(&body))).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json(&retried)["archived"], archived);
        assert_eq!(json(&retried)["changed"], false);
        assert_eq!(f.dump(), before_retry);
        assert_eq!(f.changes(), changes);
        let (status, headers, updated) = call(
            &f.state,
            request("/api/graph/curated", true, "localhost", None),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_ne!(headers[header::ETAG], old_headers[header::ETAG]);
        let has_shelf_item = json(&updated)["s"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row[0] == key);
        assert_eq!(has_shelf_item, !archived);
    }
}

#[tokio::test]
async fn curated_graph_http_adopted_entity_undo_retry_repairs_cache_after_committed_removal() {
    let f = Fixture::new(3);
    for memory in &f.memories {
        f.state
            .storage
            .link_memory_entity(memory, &f.outside)
            .unwrap();
    }
    let uri = format!("/api/entities/{}/archive", f.outside);
    let archive = Some(r#"{"archived":true}"#);
    let undo = Some(r#"{"archived":false}"#);
    assert_eq!(
        call(&f.state, request(&uri, true, "localhost", archive))
            .await
            .0,
        StatusCode::OK
    );
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
    f.state.live.as_ref().unwrap().refresh_graph().unwrap();
    let (status, old_headers, old_body) = call(
        &f.state,
        request("/api/graph/curated", true, "localhost", None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let key = format!("project:{adopted}");
    assert!(
        json(&old_body)["n"]
            .as_array()
            .unwrap()
            .iter()
            .all(|node| node[0] != key)
    );

    f.state
        .storage
        .conn
        .lock()
        .unwrap()
        .execute(
            "ALTER TABLE graph_slots RENAME TO fixture_saved_graph_slots",
            [],
        )
        .unwrap();
    let (failed, _, _) = call(&f.state, request(&uri, true, "localhost", undo)).await;
    f.state
        .storage
        .conn
        .lock()
        .unwrap()
        .execute(
            "ALTER TABLE fixture_saved_graph_slots RENAME TO graph_slots",
            [],
        )
        .unwrap();
    assert_eq!(failed, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(
        f.state
            .storage
            .entity_archive_shelf(true)
            .unwrap()
            .iter()
            .all(|row| row.id != f.outside)
    );
    let (before, changes, files) = (f.dump(), f.changes(), f.files());
    assert!(before["entity_archive"].is_empty());
    let (_, cached_headers, cached_body) = call(
        &f.state,
        request("/api/graph/curated", true, "localhost", None),
    )
    .await;
    assert_eq!(cached_body, old_body);
    assert_eq!(cached_headers[header::ETAG], old_headers[header::ETAG]);

    let (status, _, retried) = call(&f.state, request(&uri, true, "localhost", undo)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json(&retried)["archived"], false);
    assert_eq!(json(&retried)["changed"], false);
    assert!(json(&retried)["archived_at"].is_null());
    let (status, headers, repaired) = call(
        &f.state,
        request("/api/graph/curated", true, "localhost", None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_ne!(headers[header::ETAG], old_headers[header::ETAG]);
    assert!(
        json(&repaired)["n"]
            .as_array()
            .unwrap()
            .iter()
            .any(|node| node[0] == key)
    );
    // A no-op undo does not grant permission to archive a registry entity.
    for (path, body) in [
        (uri.as_str(), archive),
        ("/api/entities/missing/archive", archive),
    ] {
        assert_eq!(
            call(&f.state, request(path, true, "localhost", body))
                .await
                .0,
            StatusCode::NOT_FOUND
        );
    }
    let (status, _, missing_undo) = call(
        &f.state,
        request("/api/entities/missing/archive", true, "localhost", undo),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json(&missing_undo)["changed"], false);
    assert_eq!(json(&missing_undo)["archived"], false);
    assert_eq!(f.dump(), before);
    assert_eq!(f.changes(), changes);
    assert_eq!(f.files(), files);
}

#[tokio::test]
async fn curated_graph_http_existing_nonproject_undo_preserves_archive_and_identity_guards() {
    let f = Fixture::new(3);
    let (before, changes, files) = (f.dump(), f.changes(), f.files());
    let uri = format!("/api/entities/{}/archive", f.tool);
    let (status, _, body) = call(
        &f.state,
        request(&uri, true, "localhost", Some(r#"{"archived":false}"#)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json(&body)["archived"], false);
    assert_eq!(json(&body)["changed"], false);
    assert!(json(&body)["archived_at"].is_null());
    assert_eq!(
        call(
            &f.state,
            request(&uri, true, "localhost", Some(r#"{"archived":true}"#))
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    let secret = format!("{}{}", "ghp_", "x".repeat(36));
    let refused = format!("/api/entities/{secret}/archive");
    let (status, _, body) = call(
        &f.state,
        request(&refused, true, "localhost", Some(r#"{"archived":false}"#)),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(!String::from_utf8(body).unwrap().contains(&secret));
    assert_eq!(f.dump(), before);
    assert_eq!(f.changes(), changes);
    assert_eq!(f.files(), files);
}
