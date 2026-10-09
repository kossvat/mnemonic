use super::*;

fn archive_request(id: &str, archived: bool) -> Request<Body> {
    request(
        &format!("/api/entities/{id}/archive"),
        true,
        "localhost",
        Some(&serde_json::json!({"archived": archived}).to_string()),
    )
}

async fn history(f: &Fixture) -> Value {
    let (before, changes, files) = (f.dump(), f.changes(), f.files());
    let (status, _, body) = call(
        &f.state,
        request("/api/graph/curated/archives", true, "localhost", None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(f.dump(), before);
    assert_eq!(f.changes(), changes);
    assert_eq!(f.files(), files);
    json(&body)
}

#[tokio::test]
async fn curated_graph_http_orphan_history_after_merge_never_archives_the_survivor() {
    let f = Fixture::new(3);
    let (status, _, archived) = call(&f.state, archive_request(&f.outside, true)).await;
    assert_eq!(status, StatusCode::OK);
    let archived_at = json(&archived)["archived_at"].clone();
    f.state
        .storage
        .merge_entities("alpha", "outside-project")
        .unwrap();
    assert!(f.state.storage.entity_name(&f.outside).unwrap().is_none());
    assert_eq!(f.state.storage.entity_archive_at(&f.alpha).unwrap(), None);
    let page = history(&f).await;
    assert_eq!(page["total"], 1);
    assert!(page["next_offset"].is_null());
    assert_eq!(page["entities"][0]["id"], f.outside);
    assert_eq!(page["entities"][0]["name"], "Archived entity");
    assert_eq!(page["entities"][0]["memory_count"], 0);
    assert!(page["entities"][0]["last_memory"].is_null());
    assert_eq!(page["entities"][0]["archived_at"], archived_at);
    let (before, changes, files) = (f.dump(), f.changes(), f.files());
    // The retained mark still permits an unchanged archive retry, even
    // though a brand-new archive for this missing ID would be ineligible.
    let (status, _, retry) = call(&f.state, archive_request(&f.outside, true)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json(&retry)["changed"], false);
    assert_eq!(json(&retry)["archived_at"], archived_at);
    assert_eq!(f.dump(), before);
    assert_eq!(f.changes(), changes);
    assert_eq!(f.files(), files);
    let cached = (vec![(f.outside.clone(), 60.0, Some("low".into()))], 0.0);
    assert_eq!(
        f.state
            .storage
            .rekeyed_day_with_unattributed(cached.clone())
            .unwrap(),
        (Vec::new(), 60.0)
    );
    let (status, _, undone) = call(&f.state, archive_request(&f.outside, false)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json(&undone)["changed"], true);
    assert_eq!(history(&f).await["total"], 0);
    assert_eq!(
        f.state
            .storage
            .rekeyed_day_with_unattributed(cached.clone())
            .unwrap(),
        cached
    );
    assert_eq!(f.state.storage.entity_archive_at(&f.alpha).unwrap(), None);
    assert_eq!(
        f.state.storage.entity_name(&f.alpha).unwrap().as_deref(),
        Some("alpha")
    );
}

#[tokio::test]
async fn curated_graph_http_orphan_undo_retries_after_graph_clear_and_failed_refresh() {
    let f = Fixture::new(3);
    assert_eq!(
        call(&f.state, archive_request(&f.outside, true)).await.0,
        StatusCode::OK
    );
    let (_, old_headers, old_body) = call(
        &f.state,
        request("/api/graph/curated", true, "localhost", None),
    )
    .await;
    assert!(f.state.storage.clear_graph().unwrap() > 0);
    assert_eq!(history(&f).await["entities"][0]["name"], "Archived entity");
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
    for _ in 0..2 {
        let (failed, _, _) = call(&f.state, archive_request(&f.outside, false)).await;
        assert_eq!(failed, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(f.state.storage.entity_archive_at(&f.outside).unwrap(), None);
        assert!(f.state.storage.entity_name(&f.outside).unwrap().is_none());
        let (_, headers, body) = call(
            &f.state,
            request("/api/graph/curated", true, "localhost", None),
        )
        .await;
        assert_eq!(headers[header::ETAG], old_headers[header::ETAG]);
        assert_eq!(body, old_body);
    }
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
    let (before, changes, files) = (f.dump(), f.changes(), f.files());
    let (status, _, retry) = call(&f.state, archive_request(&f.outside, false)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json(&retry)["changed"], false);
    assert_eq!(json(&retry)["archived"], false);
    assert!(json(&retry)["archived_at"].is_null());
    let (status, headers, repaired) = call(
        &f.state,
        request("/api/graph/curated", true, "localhost", None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_ne!(headers[header::ETAG], old_headers[header::ETAG]);
    assert_ne!(repaired, old_body);
    assert_eq!(history(&f).await["total"], 0);
    // Missing-ID Undo is deliberately idempotent; it cannot create a mark.
    assert_eq!(
        call(&f.state, archive_request("missing", false)).await.0,
        StatusCode::OK
    );
    for id in [&f.outside, "missing"] {
        assert_eq!(
            call(&f.state, archive_request(id, true)).await.0,
            StatusCode::NOT_FOUND
        );
    }
    assert_eq!(f.dump(), before);
    assert_eq!(f.changes(), changes);
    assert_eq!(f.files(), files);
}
