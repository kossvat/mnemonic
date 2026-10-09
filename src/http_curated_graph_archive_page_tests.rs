use super::*;

#[tokio::test]
async fn curated_graph_http_pages_all_unarchived_candidates_without_writes() {
    let f = Fixture::new(3);
    let mut expected = Vec::new();
    for index in 0..45 {
        let id = f
            .state
            .storage
            .upsert_entity(&Entity {
                name: format!("candidate-{index:03}"),
                entity_type: EntityType::Project,
            })
            .unwrap();
        f.state
            .storage
            .link_memory_entity(&f.memories[0], &id)
            .unwrap();
        expected.push(id);
    }
    expected.sort();
    f.state
        .storage
        .entity_archive_set(&f.outside, true, 1_800_000_000)
        .unwrap();
    f.state.live.as_ref().unwrap().refresh_graph().unwrap();
    let (before, changes, files) = (f.dump(), f.changes(), f.files());
    let (_, _, overview) = call(
        &f.state,
        request("/api/graph/curated", true, "localhost", None),
    )
    .await;
    let overview = json(&overview);
    assert_eq!(
        overview["s"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|row| row[2] == 1)
            .count(),
        20
    );
    assert_eq!(overview["o"][3], 25);

    let mut found = Vec::new();
    for offset in [0, 20, 40] {
        let uri = format!("/api/graph/curated/archives?archived=false&offset={offset}&limit=20");
        let (status, _, body) = call(&f.state, request(&uri, true, "localhost", None)).await;
        assert_eq!(status, StatusCode::OK);
        let page = json(&body);
        assert_eq!(page["total"], 45);
        assert_eq!(page["offset"], offset);
        assert_eq!(page["limit"], 20);
        let rows = page["entities"].as_array().unwrap();
        assert_eq!(rows.len(), if offset == 40 { 5 } else { 20 });
        if offset == 40 {
            assert!(page["next_offset"].is_null());
        } else {
            assert_eq!(page["next_offset"], offset + 20);
        }
        for row in rows {
            assert_eq!(row["memory_count"], 1);
            assert!(row["archived_at"].is_null());
            found.push(row["id"].as_str().unwrap().to_string());
        }
    }
    assert_eq!(found, expected);
    for suffix in ["", "?archived=true"] {
        let uri = format!("/api/graph/curated/archives{suffix}");
        let (status, _, body) = call(&f.state, request(&uri, true, "localhost", None)).await;
        assert_eq!(status, StatusCode::OK);
        let page = json(&body);
        assert_eq!(page["total"], 1);
        assert_eq!(page["entities"][0]["id"], f.outside);
    }
    assert_eq!(
        call(
            &f.state,
            request(
                "/api/graph/curated/archives?archived=invalid",
                true,
                "localhost",
                None,
            ),
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(f.dump(), before);
    assert_eq!(f.changes(), changes);
    assert_eq!(f.files(), files);
}
