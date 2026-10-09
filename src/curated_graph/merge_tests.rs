use super::*;
use crate::storage::curated_graph::EntityEvidence;

const NOW: i64 = 1_800_000_000;

fn project(id: i64, parent_id: Option<i64>) -> ProjectEvidence {
    ProjectEvidence {
        row: ProjectRow {
            id,
            slug: format!("fixture-{id}"),
            name: format!("Project {id}"),
            parent_id,
            entity_id: None,
            state: ProjectState::Active,
            slot: id - 1,
            merged_into: None,
            prev_entity_type: None,
        },
        memory_count: 0,
        last_memory: Some(NOW),
        last_segment: None,
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

fn slots(evidence: &Evidence, units: &[PlanUnit]) -> Vec<GraphSlot> {
    let mut next = BTreeMap::new();
    slot_requests(evidence, units)
        .into_iter()
        .map(|request| {
            let next = next.entry(request.parent_key.clone()).or_insert(0);
            let slot = request.preferred_slot.unwrap_or(*next);
            *next = (*next).max(slot + 1);
            GraphSlot {
                key: request.key,
                parent_key: request.parent_key,
                slot,
            }
        })
        .collect()
}

fn draw(
    evidence: &Evidence,
    units: &[PlanUnit],
    slots: &[GraphSlot],
    focus: Option<&str>,
) -> Snapshot {
    render(
        Input {
            evidence,
            units,
            slots,
            hours_30d: &BTreeMap::new(),
            now: NOW,
        },
        focus,
    )
    .unwrap()
}

fn edges(snapshot: &Snapshot) -> Vec<(String, String, u8, u32)> {
    snapshot
        .payload
        .e
        .iter()
        .map(|edge| {
            (
                snapshot.payload.n[usize::from(edge.0)].key.clone(),
                snapshot.payload.n[usize::from(edge.1)].key.clone(),
                edge.2,
                edge.3,
            )
        })
        .collect()
}

#[test]
fn curated_graph_canonical_contains_keeps_neighbors_without_reparenting_slots() {
    for count in [0, 1] {
        for target_parent in [None, Some(4)] {
            let original = Evidence {
                projects: vec![
                    project(1, target_parent),
                    project(2, None),
                    project(3, Some(2)),
                    project(4, None),
                ],
                links: vec![link("project:2", "project:3", LinkKind::Contains, count)],
                ..Default::default()
            };
            let slots = slots(&original, &[]);
            let before = draw(&original, &[], &slots, None);
            // Storage supplies the canonical edge after part 3 merges into
            // project 1. Project 1 can be a root or another parent's part.
            let mut merged = original.clone();
            merged.projects.retain(|project| project.row.id != 3);
            merged.links = vec![link("project:2", "project:1", LinkKind::Contains, count)];
            let snapshot = draw(&merged, &[], &slots, None);
            assert!(edges(&snapshot).contains(&("project:2".into(), "project:1".into(), 0, count)));
            assert!(snapshot.payload.e.iter().all(|edge| edge.2 != 3));
            assert!(
                snapshot
                    .payload
                    .n
                    .iter()
                    .all(|node| node.key != "project:3")
            );
            for node in &snapshot.payload.n {
                let previous = before
                    .payload
                    .n
                    .iter()
                    .find(|old| old.key == node.key)
                    .unwrap();
                assert_eq!((node.x, node.y), (previous.x, previous.y));
            }
            for (focus, neighbor) in [("project:1", "project:2"), ("project:2", "project:1")] {
                let expanded = draw(&merged, &[], &slots, Some(focus));
                assert!(expanded.payload.n.iter().any(|node| node.key == neighbor));
                assert!(edges(&expanded).contains(&(
                    "project:2".into(),
                    "project:1".into(),
                    0,
                    count
                )));
            }
            // Missing evidence must not synthesize a relation between roots
            // or between a hub and another hub's part.
            merged.links.clear();
            let isolated = draw(&merged, &[], &slots, Some("project:2"));
            assert_eq!(isolated.payload.n.len(), 1);
            assert!(isolated.payload.e.is_empty());
            assert_eq!(draw(&original, &[], &slots, None).bytes, before.bytes);
        }
    }
}

#[test]
fn curated_graph_supplied_containment_keeps_endpoint_and_unit_parent_validation() {
    let units = [plan_unit(1, "release")];
    let evidence = Evidence {
        projects: vec![project(1, None), project(2, None), project(3, Some(2))],
        entities: vec![
            EntityEvidence {
                id: "tool".into(),
                name: "rust".into(),
                kind: EntityKind::Tech,
                memory_count: 3,
                last_memory: Some(NOW),
            },
            EntityEvidence {
                id: "reviewer".into(),
                name: "Reviewer".into(),
                kind: EntityKind::Person,
                memory_count: 1,
                last_memory: Some(NOW),
            },
        ],
        links: vec![
            link("project:1", "entity:tool", LinkKind::Uses, 3),
            link("project:1", "entity:reviewer", LinkKind::Involves, 1),
            link("project:1", "unit:1:release", LinkKind::Contains, 1),
            link("project:1", "entity:tool", LinkKind::Contains, 3),
            link("entity:reviewer", "project:1", LinkKind::Contains, 1),
            link("project:2", "unit:1:release", LinkKind::Contains, 9),
            link("project:1", "project:1", LinkKind::Contains, 1),
            link("project:1", "project:99", LinkKind::Contains, 1),
            link("project:1", "project:2", LinkKind::Uses, 3),
            link("project:1", "project:2", LinkKind::Involves, 3),
            link("project:1", "project:2", LinkKind::Relates, 1),
        ],
        ..Default::default()
    };
    let slots = slots(&evidence, &units);
    let snapshot = draw(&evidence, &units, &slots, None);
    let mut actual = edges(&snapshot);
    actual.sort();
    let mut expected = vec![
        ("project:1".into(), "entity:tool".into(), 1, 3),
        ("project:1".into(), "entity:reviewer".into(), 2, 1),
        ("project:1".into(), "unit:1:release".into(), 0, 1),
        ("project:2".into(), "project:3".into(), 0, 0),
    ];
    expected.sort();
    assert_eq!(actual, expected);
}
