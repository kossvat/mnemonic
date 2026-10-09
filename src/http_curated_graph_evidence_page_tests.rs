use super::*;

#[tokio::test]
async fn long_safe_memory_ids_keep_graph_counts_and_evidence_pages_consistent() {
    let f = Fixture::new(3);
    let beta = f
        .state
        .storage
        .project_by_key("beta")
        .unwrap()
        .unwrap()
        .entity_id
        .unwrap();
    let mut expected = f.memories.clone();
    for index in 0..3 {
        let mut entry = MemoryEntry::new(
            "Fixture long identity",
            "Evidence with a safe long identifier",
            MemoryType::Decision,
            EventSource::Manual,
        );
        entry.id = format!("fixture-long-memory-{index}-{}", "safe".repeat(55));
        assert!(entry.id.len() > 192);
        assert!(crate::redaction::state::check_identities([entry.id.as_str()]).is_ok());
        f.state.storage.save(&entry).unwrap();
        for entity in [&f.alpha, &beta] {
            f.state
                .storage
                .link_memory_entity(&entry.id, entity)
                .unwrap();
        }
        expected.push(entry.id);
    }
    expected.sort();
    f.state.live.as_ref().unwrap().refresh_graph().unwrap();
    let (before, changes, files) = (f.dump(), f.changes(), f.files());
    let (_, _, graph) = call(
        &f.state,
        request("/api/graph/curated", true, "localhost", None),
    )
    .await;
    let graph = json(&graph);
    let nodes = graph["n"].as_array().unwrap();
    let alpha = nodes
        .iter()
        .position(|node| node[0] == "project:1")
        .unwrap();
    let beta = nodes
        .iter()
        .position(|node| node[0] == "project:2")
        .unwrap();
    let edge = graph["e"]
        .as_array()
        .unwrap()
        .iter()
        .find(|edge| edge[0] == alpha && edge[1] == beta && edge[2] == 3)
        .unwrap();
    assert_eq!(edge[3], expected.len());

    let path = "/api/graph/curated/edge?source=project:1&target=project:2&kind=relates&limit=4";
    let mut ids = Vec::new();
    for (offset, count, next) in [(0, 4, Some(4)), (4, 2, None)] {
        let (status, _, body) = call(
            &f.state,
            request(&format!("{path}&offset={offset}"), true, "localhost", None),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let page = json(&body);
        let returned = page["memory_ids"].as_array().unwrap();
        assert_eq!(returned.len(), count);
        assert_eq!(page["next_offset"], serde_json::json!(next));
        ids.extend(returned.iter().map(|id| id.as_str().unwrap().to_string()));
    }
    assert_eq!(ids, expected);
    assert_eq!(f.dump(), before);
    assert_eq!(f.changes(), changes);
    assert_eq!(f.files(), files);
}
