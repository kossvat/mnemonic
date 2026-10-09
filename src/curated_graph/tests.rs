use super::*;
use crate::storage::curated_graph::{
    ArchivedCandidate, EntityEvidence, EvidenceLink, ProjectEvidence,
};
use crate::storage::projects::ProjectRow;

const NOW: i64 = 4_000_000;

fn project(id: i64, parent: Option<i64>) -> ProjectEvidence {
    ProjectEvidence {
        row: ProjectRow {
            id,
            slug: format!("fixture-{id}"),
            name: format!("Fixture {id}"),
            parent_id: parent,
            entity_id: None,
            state: ProjectState::Active,
            slot: id - 1,
            merged_into: None,
            prev_entity_type: None,
        },
        memory_count: 3,
        last_memory: Some(NOW),
        last_segment: None,
    }
}

fn evidence() -> Evidence {
    Evidence {
        projects: Vec::new(),
        entities: Vec::new(),
        links: Vec::new(),
        shelf: Vec::new(),
    }
}

fn link(source: &str, target: &str, kind: LinkKind, memory_count: u32) -> EvidenceLink {
    EvidenceLink {
        source: source.into(),
        target: target.into(),
        kind,
        memory_count,
    }
}

fn unit(id: &str, closed_at: Option<i64>) -> PlanUnit {
    PlanUnit {
        project_id: 1,
        id: id.into(),
        title: format!("Work on {id}"),
        closed_at,
        last_memory: None,
        memory_count: 0,
    }
}

/// Fixture allocator preserves old assignments when adding more fixtures.
fn assign(evidence: &Evidence, units: &[PlanUnit], slots: &mut Vec<GraphSlot>) {
    for request in slot_requests(evidence, units) {
        if slots.iter().any(|slot| slot.key == request.key) {
            continue;
        }
        let next = slots
            .iter()
            .filter(|slot| slot.parent_key == request.parent_key)
            .map(|slot| slot.slot)
            .max()
            .unwrap_or(-1)
            + 1;
        slots.push(GraphSlot {
            key: request.key,
            parent_key: request.parent_key,
            slot: request.preferred_slot.unwrap_or(next),
        });
    }
}

fn draw(
    evidence: &Evidence,
    units: &[PlanUnit],
    slots: &[GraphSlot],
    now: i64,
    expand: Option<&str>,
) -> Snapshot {
    render(
        Input {
            evidence,
            units,
            slots,
            now,
            hours_30d: &BTreeMap::new(),
        },
        expand,
    )
    .unwrap()
}

fn node<'a>(snapshot: &'a Snapshot, key: &str) -> &'a Node {
    snapshot
        .payload
        .n
        .iter()
        .find(|node| node.key == key)
        .unwrap()
}

fn positions(snapshot: &Snapshot) -> BTreeMap<String, (i32, i32)> {
    snapshot
        .payload
        .n
        .iter()
        .map(|node| (node.key.clone(), (node.x, node.y)))
        .collect()
}

#[test]
fn curated_graph_nodes_links_hours_and_selected_plan_units() {
    let mut input = evidence();
    input.projects = vec![project(1, None), project(2, None), project(3, Some(1))];
    input.entities = vec![
        EntityEvidence {
            id: "tool".into(),
            name: "Compiler".into(),
            kind: EntityKind::Tech,
            memory_count: 3,
            last_memory: Some(NOW - 31 * DAY),
        },
        EntityEvidence {
            id: "person".into(),
            name: "Reviewer".into(),
            kind: EntityKind::Person,
            memory_count: 1,
            last_memory: Some(NOW),
        },
        EntityEvidence {
            id: "rare".into(),
            name: "Rare tool".into(),
            kind: EntityKind::Tech,
            memory_count: 2,
            last_memory: Some(NOW),
        },
        EntityEvidence {
            id: "unlinked".into(),
            name: "Unused tool".into(),
            kind: EntityKind::Tech,
            memory_count: 9,
            last_memory: Some(NOW),
        },
    ];
    input.links = vec![
        link("project:1", "project:3", LinkKind::Contains, 2),
        link("project:1", "entity:tool", LinkKind::Uses, 3),
        link("project:1", "entity:rare", LinkKind::Uses, 2),
        link("project:1", "entity:person", LinkKind::Involves, 1),
        link("project:1", "project:2", LinkKind::Relates, 2),
        link("project:2", "project:3", LinkKind::Relates, 1),
    ];
    input.shelf.push(ArchivedCandidate {
        id: "outside".into(),
        name: "Unregistered".into(),
        memory_count: 1,
        last_memory: None,
        archived_at: None,
    });
    let units = vec![
        unit("open-one", None),
        unit("open-two", None),
        unit("last-closed", Some(NOW)),
    ];
    let mut slots = Vec::new();
    assign(&input, &units, &mut slots);
    let hours = BTreeMap::from([(
        1,
        Hours {
            owner_seconds: Some(3_600),
            agent_seconds: Some(7_200),
        },
    )]);
    let snapshot = render(
        Input {
            evidence: &input,
            units: &units,
            slots: &slots,
            now: NOW,
            hours_30d: &hours,
        },
        None,
    )
    .unwrap();
    assert_eq!(snapshot.payload.n.len(), 8);
    assert_eq!(
        node(&snapshot, "project:1").hours_30d_tenths,
        [Some(10), Some(20)]
    );
    assert_eq!(
        node(&snapshot, "project:1").project_slug.as_deref(),
        Some("fixture-1")
    );
    assert_eq!(node(&snapshot, "entity:tool").state, State::Stale);
    assert_eq!(
        node(&snapshot, "entity:tool").hours_30d_tenths,
        [None, None]
    );
    assert_eq!(node(&snapshot, "unit:1:open-one").state, State::Stale);
    assert!(node(&snapshot, "unit:1:last-closed").closed);
    assert_eq!(snapshot.payload.s[0].0, "entity:outside");
    assert!(!snapshot.payload.n.iter().any(|node| matches!(
        node.key.as_str(),
        "entity:rare" | "entity:unlinked" | "entity:outside"
    )));
    let edges: Vec<_> = snapshot
        .payload
        .e
        .iter()
        .map(|edge| {
            (
                snapshot.payload.n[usize::from(edge.0)].key.as_str(),
                snapshot.payload.n[usize::from(edge.1)].key.as_str(),
                edge.2,
                edge.3,
            )
        })
        .collect();
    for expected in [
        ("project:1", "project:3", 0, 2),
        ("project:1", "entity:tool", 1, 3),
        ("project:1", "entity:person", 2, 1),
        ("project:1", "project:2", 3, 2),
        ("project:1", "unit:1:open-one", 0, 0),
    ] {
        assert!(edges.contains(&expected));
    }
    assert!(!edges.contains(&("project:2", "project:3", 3, 1)));
}

#[test]
fn curated_graph_reordered_input_has_identical_bytes_and_etag() {
    let mut input = evidence();
    input.projects = vec![project(1, None), project(2, None), project(3, Some(1))];
    input.links = vec![
        link("project:1", "project:2", LinkKind::Relates, 4),
        link("project:1", "project:3", LinkKind::Contains, 2),
    ];
    let mut units = vec![unit("second", None), unit("first", Some(NOW))];
    let mut slots = Vec::new();
    assign(&input, &units, &mut slots);
    let first = draw(&input, &units, &slots, NOW, None);
    input.projects.reverse();
    input.links.reverse();
    units.reverse();
    slots.reverse();
    let second = draw(&input, &units, &slots, NOW, None);
    assert_eq!(first.bytes, second.bytes);
    assert_eq!(first.version, second.version);
    assert_eq!(first.etag, second.etag);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&first.bytes).unwrap()["v"],
        1
    );
}

#[test]
fn curated_graph_staleness_boundaries_shelf_and_return() {
    assert_eq!(
        freshness(Some(NOW - 7 * DAY), NOW, false),
        (State::Active, Some(7))
    );
    assert_eq!(
        freshness(Some(NOW - 7 * DAY - 1), NOW, false).0,
        State::Quiet
    );
    assert_eq!(
        freshness(Some(NOW - 30 * DAY), NOW, false),
        (State::Stale, Some(30))
    );
    assert_eq!(freshness(None, NOW, false), (State::Stale, None));
    assert_eq!(
        freshness(Some(NOW + DAY), NOW, false),
        (State::Active, Some(0))
    );
    let mut input = evidence();
    input.projects.push(project(1, None));
    let mut slots = Vec::new();
    assign(&input, &[], &mut slots);
    let first = draw(&input, &[], &slots, NOW, None);
    let stale = draw(&input, &[], &slots, NOW + 30 * DAY, None);
    assert!(stale.payload.n.is_empty());
    assert_eq!(stale.payload.s[0].0, "project:1");
    assert_eq!(stale.payload.s[0].6.as_deref(), Some("fixture-1"));
    let expanded = draw(&input, &[], &slots, NOW + 30 * DAY, Some("project:1"));
    assert_eq!(positions(&first), positions(&expanded));
    input.projects[0].last_segment = Some(NOW + 30 * DAY);
    let returned = draw(&input, &[], &slots, NOW + 30 * DAY, None);
    assert_eq!(positions(&first), positions(&returned));
    assert!(returned.payload.s.is_empty());
}

#[test]
fn curated_graph_positions_survive_insertion_archiving_hours_and_reparenting() {
    let mut input = evidence();
    input.projects = vec![project(1, None), project(2, Some(1))];
    let units = vec![unit("work", None)];
    let mut slots = Vec::new();
    assign(&input, &units, &mut slots);
    let first = draw(&input, &units, &slots, NOW, None);
    input.projects.push(project(3, None));
    assign(&input, &units, &mut slots);
    input.projects[1].row.parent_id = Some(3);
    input.projects[0].row.state = ProjectState::Archived;
    let after = draw(&input, &units, &slots, NOW, None);
    assert_eq!(
        (node(&first, "project:2").x, node(&first, "project:2").y),
        (node(&after, "project:2").x, node(&after, "project:2").y)
    );
    assert!(!after.payload.n.iter().any(|node| node.key == "project:1"));
    input.projects[0].row.state = ProjectState::Active;
    let hours = BTreeMap::from([(
        1,
        Hours {
            owner_seconds: None,
            agent_seconds: Some(600_000),
        },
    )]);
    let restored = render(
        Input {
            evidence: &input,
            units: &units,
            slots: &slots,
            now: NOW,
            hours_30d: &hours,
        },
        None,
    )
    .unwrap();
    for (key, position) in positions(&first) {
        assert_eq!(positions(&restored)[&key], position);
    }
    assert_eq!(node(&restored, "project:1").size_bucket, 4);
    assert_eq!(
        slots
            .iter()
            .find(|slot| slot.key == "project:2")
            .unwrap()
            .parent_key
            .as_deref(),
        Some("project:1")
    );
}

#[test]
fn curated_graph_person_and_technology_rings_do_not_depend_on_other_nodes() {
    let mut input = evidence();
    input.projects.push(project(1, None));
    for (id, kind) in [("tool", EntityKind::Tech), ("person", EntityKind::Person)] {
        input.entities.push(EntityEvidence {
            id: id.into(),
            name: id.into(),
            kind,
            memory_count: 4,
            last_memory: Some(NOW),
        });
        input.links.push(link(
            "project:1",
            &format!("entity:{id}"),
            if kind == EntityKind::Tech {
                LinkKind::Uses
            } else {
                LinkKind::Involves
            },
            4,
        ));
    }
    let mut slots = Vec::new();
    assign(&input, &[], &mut slots);
    let first = draw(&input, &[], &slots, NOW, None);
    assert_eq!(
        (node(&first, "entity:tool").x, node(&first, "entity:tool").y),
        (5_000, 0)
    );
    assert_eq!(
        (
            node(&first, "entity:person").x,
            node(&first, "entity:person").y
        ),
        (6_500, 0)
    );
    input.projects.push(project(2, None));
    assign(&input, &[], &mut slots);
    let later = draw(&input, &[], &slots, NOW + DAY, None);
    for (key, position) in positions(&first) {
        assert_eq!(positions(&later)[&key], position);
    }
}

#[test]
fn curated_graph_caps_dense_expansion_and_records_payload_sizes() {
    let mut input = evidence();
    input.projects = (1..=160).map(|id| project(id, None)).collect();
    for source in 1..=160 {
        for target in source + 1..=160 {
            input.links.push(link(
                &format!("project:{source}"),
                &format!("project:{target}"),
                LinkKind::Relates,
                3,
            ));
        }
    }
    let mut slots = Vec::new();
    assign(&input, &[], &mut slots);
    let overview = draw(&input, &[], &slots, NOW, None);
    let expanded = draw(&input, &[], &slots, NOW, Some("project:1"));
    assert_eq!(overview.payload.n.len(), OVERVIEW_NODES);
    assert_eq!(expanded.payload.n.len(), EXPANDED_NODES);
    assert_eq!(expanded.payload.e.len(), EXPANDED_EDGES);
    assert!(overview.bytes.len() <= OVERVIEW_BYTES);
    assert!(expanded.bytes.len() <= EXPANDED_BYTES);
    assert_eq!(overview.payload.o[0], 100);
    assert_eq!(expanded.payload.o[0], 10);
    assert!(expanded.payload.o[1] > 0);
    eprintln!(
        "curated graph: overview {} bytes; expanded {} bytes",
        overview.bytes.len(),
        expanded.bytes.len()
    );
}

#[test]
fn curated_graph_long_labels_shelf_clipping_and_wire_indices_are_bounded() {
    let mut input = evidence();
    input.projects.push(project(1, None));
    let units = (1..=160)
        .map(|id| {
            let mut row = unit(&format!("unit-{id:03}-{}", "x".repeat(151)), None);
            row.title = "\u{1f9ea}".repeat(64);
            assert_eq!(row.id.len(), 160);
            row
        })
        .collect::<Vec<_>>();
    for id in 0..50 {
        input.shelf.push(ArchivedCandidate {
            id: format!("outside-{id}"),
            name: "Unregistered fixture".into(),
            memory_count: 0,
            last_memory: None,
            archived_at: None,
        });
    }
    let mut slots = Vec::new();
    assign(&input, &units, &mut slots);
    let result = draw(&input, &units, &slots, NOW, Some("project:1"));
    assert!(result.bytes.len() <= EXPANDED_BYTES);
    assert!(result.payload.n.len() <= EXPANDED_NODES);
    assert!(
        result.payload.o[0] > 11,
        "long keys may require fewer budget nodes"
    );
    assert!(result.payload.o[3] >= 30);
    for edge in &result.payload.e {
        assert!(usize::from(edge.0) < result.payload.n.len());
        assert!(usize::from(edge.1) < result.payload.n.len());
    }
    assert!(
        render(
            Input {
                evidence: &input,
                units: &[],
                slots: &slots,
                now: NOW,
                hours_30d: &BTreeMap::new()
            },
            Some("project:missing")
        )
        .is_err()
    );
}

#[test]
fn curated_graph_narrative_is_redacted_and_invalid_slots_fail_closed() {
    let mut input = evidence();
    input.projects.push(project(1, None));
    let credential = format!("{}{}", "ghp_", "x".repeat(36));
    input.projects[0].row.name = format!("Fixture {credential}");
    let mut slots = Vec::new();
    assign(&input, &[], &mut slots);
    let snapshot = draw(&input, &[], &slots, NOW, None);
    assert!(
        !String::from_utf8(snapshot.bytes)
            .unwrap()
            .contains(&credential)
    );
    slots[0].parent_key = Some(slots[0].key.clone());
    assert!(draw(&input, &[], &slots, NOW, None).payload.n.is_empty());
    slots[0].parent_key = None;
    slots[0].slot = -1;
    assert!(draw(&input, &[], &slots, NOW, None).payload.n.is_empty());
}
