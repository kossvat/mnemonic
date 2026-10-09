use super::*;
use crate::storage::curated_graph::unit_key;

#[tokio::test]
async fn curated_graph_http_preserves_safe_proposal_unit_names_in_expand_and_evidence() {
    let f = Fixture::new(3);
    let changes = f
        .state
        .config
        .projects
        .root()
        .unwrap()
        .join("alpha/openspec/changes");
    let names = [
        "release-one".to_string(),
        "phase one".into(),
        "\u{043f}\u{043b}\u{0430}\u{043d}".into(),
        "milestone:one".into(),
        "x".repeat(161),
    ];
    let project = f.state.storage.project_by_key("alpha").unwrap().unwrap().id;
    let mut units_with_evidence = Vec::new();
    for name in &names {
        let folder = changes.join(name);
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("proposal.md"), "# Fixture proposal\n").unwrap();
        let entity = f
            .state
            .storage
            .upsert_entity(&Entity {
                name: name.clone(),
                entity_type: EntityType::Concept,
            })
            .unwrap();
        let memory = MemoryEntry::new(
            "Fixture unit evidence",
            "Names the plan unit and its project",
            MemoryType::Decision,
            EventSource::Manual,
        );
        f.state.storage.save(&memory).unwrap();
        for entity in [&f.alpha, &entity] {
            f.state
                .storage
                .link_memory_entity(&memory.id, entity)
                .unwrap();
        }
        units_with_evidence.push((unit_key(project, name).unwrap(), memory.id));
    }
    let live = f.state.live.as_ref().unwrap();
    live.configure_plans(f.state.storage.clone(), f.state.config.projects.clone());
    let plans = live.project_plans();
    let units = &plans[&project].units;
    // Establish the real provider path: these names reach PlanUnit unchanged.
    for name in &names {
        assert!(units.iter().any(|unit| &unit.id == name));
    }
    live.refresh_graph().unwrap();
    let (before, count, files) = (f.dump(), f.changes(), f.files());
    let slots = f.state.storage.graph_slots().unwrap();
    for (key, _) in &units_with_evidence {
        assert!(slots.iter().any(|slot| &slot.key == key));
    }
    let pages = std::iter::once(("/api/graph/curated".to_string(), None)).chain(
        units_with_evidence.iter().map(|(key, _)| {
            (
                format!("/api/graph/curated?expand={key}"),
                Some(key.as_str()),
            )
        }),
    );
    for (uri, focus) in pages {
        let (status, _, body) = call(&f.state, request(&uri, true, "localhost", None)).await;
        assert_eq!(status, StatusCode::OK);
        let payload = json(&body);
        let nodes = payload["n"].as_array().unwrap();
        let source = nodes
            .iter()
            .position(|node| node[0] == format!("project:{project}"))
            .unwrap();
        for (key, _) in &units_with_evidence {
            if focus.is_some_and(|focus| focus != key) {
                continue;
            }
            let target = nodes.iter().position(|node| node[0] == *key).unwrap();
            let edge = payload["e"]
                .as_array()
                .unwrap()
                .iter()
                .find(|edge| edge[0] == source && edge[1] == target && edge[2] == 0)
                .unwrap();
            assert_eq!(edge[3], 1);
        }
    }
    for (key, memory) in &units_with_evidence {
        let uri =
            format!("/api/graph/curated/edge?source=project:{project}&target={key}&kind=contains");
        let (status, _, body) = call(&f.state, request(&uri, true, "localhost", None)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json(&body)["memory_ids"], serde_json::json!([memory]));
    }
    for name in &names[1..] {
        let key = unit_key(project, name).unwrap();
        assert!(key.starts_with(&format!("unit:{project}:h:")));
        assert!(
            !slots
                .iter()
                .any(|slot| slot.key == format!("unit:{project}:{name}"))
        );
    }
    for suffix in ["phase%20one", "%D0%BF%D0%BB%D0%B0%D0%BD", "phase%3Aone"] {
        let uri = format!("/api/graph/curated?expand=unit:{project}:{suffix}");
        assert_eq!(
            call(&f.state, request(&uri, true, "localhost", None))
                .await
                .0,
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(f.dump(), before);
    assert_eq!(f.changes(), count);
    assert_eq!(f.files(), files);
}

#[test]
fn curated_graph_http_accepts_exactly_the_shared_node_key_domain() {
    use crate::storage::curated_graph::valid_key;
    for key in [
        "project:1",
        "entity:fixture-id",
        "entity:person:abc123",
        "unit:1:release-1_2.3",
    ] {
        assert!(valid_key(key));
        assert!(super::super::node_key(key).is_ok());
    }
    for key in [
        "project:01",
        "unit:1:phase one",
        "unit:1:\u{043f}",
        "unit:1:phase:one",
        "unit:1:%D0%BF",
        "unit:1:.",
        "unit:1:..",
        "unit:1:h:abc",
        "ring:tech",
        "ring:person",
    ] {
        assert!(super::super::node_key(key).is_err());
    }
    assert!(super::super::node_key(&format!("unit:1:{}", "x".repeat(160))).is_ok());
    assert!(super::super::node_key(&format!("unit:1:{}", "x".repeat(161))).is_err());
    for raw in ["phase one", "\u{043f}\u{043b}\u{0430}\u{043d}", "phase:one"] {
        let key = unit_key(1, raw).unwrap();
        assert!(valid_key(&key));
        assert!(super::super::node_key(&key).is_ok());
    }
    let secret = format!("{}{}", "ghp_", "x".repeat(36));
    for raw in ["", ".", "..", secret.as_str()] {
        assert!(unit_key(1, raw).is_none());
        assert!(super::super::node_key(&format!("unit:1:{raw}")).is_err());
    }
}
