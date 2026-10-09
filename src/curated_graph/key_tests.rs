use super::*;
use crate::storage::Storage;
use crate::storage::curated_graph::{EvidenceLink, ProjectEvidence};
use crate::storage::projects::ProjectRow;
use std::collections::BTreeSet;

#[path = "merge_tests.rs"]
mod merge_tests;

fn plan_unit(project_id: i64, id: &str) -> PlanUnit {
    PlanUnit {
        project_id,
        id: id.into(),
        title: id.into(),
        closed_at: None,
        memory_count: 0,
        last_memory: None,
    }
}

#[test]
fn curated_graph_all_safe_plan_names_keep_distinct_durable_positions() {
    let temp = tempfile::tempdir().unwrap();
    let storage = Storage::open(&temp.path().join("memory.db")).unwrap();
    let project = storage.project_upsert("fixture", "Fixture", None).unwrap();
    let mut evidence = storage.curated_graph_evidence(&[], &[]).unwrap();
    let now = 1_800_000_000;
    evidence.projects[0].last_memory = Some(now);
    let ids = [
        "release-one".to_string(),
        "phase one".into(),
        "phase  one".into(),
        "phase-one".into(),
        "\u{043f}\u{043b}\u{0430}\u{043d}".into(),
        "phase:one".into(),
        "x".repeat(161),
        "\u{00e9}".into(),
        "e\u{0301}".into(),
    ];
    let mut units: Vec<_> = ids.iter().map(|id| plan_unit(project, id)).collect();
    let keys: BTreeSet<_> = ids
        .iter()
        .map(|id| unit_key(project, id).unwrap())
        .collect();
    assert_eq!(keys.len(), ids.len());
    assert_eq!(
        unit_key(project, &ids[0]),
        Some(format!("unit:{project}:release-one"))
    );
    assert!(keys.iter().all(|key| valid_key(key)));
    let requests = slot_requests(&evidence, &units);
    assert_eq!(requests.len(), ids.len() + 1);
    // The real allocator must admit the entire mixed-name batch atomically.
    storage.ensure_graph_slots(&requests).unwrap();
    let original_slots = storage.graph_slots().unwrap();
    let hours = BTreeMap::new();
    let draw = |units: &[PlanUnit], slots: &[GraphSlot], focus: Option<&str>| {
        render(
            Input {
                evidence: &evidence,
                units,
                slots,
                hours_30d: &hours,
                now,
            },
            focus,
        )
        .unwrap()
    };
    let overview = draw(&units, &original_slots, None);
    assert_eq!(overview.payload.n.len(), ids.len() + 1);
    assert_eq!(overview.payload.o[0], 0);
    let positions: BTreeMap<_, _> = overview
        .payload
        .n
        .iter()
        .map(|node| (node.key.clone(), (node.x, node.y)))
        .collect();
    for key in &keys {
        let expanded = draw(&units, &original_slots, Some(key));
        assert_eq!(expanded.payload.n.len(), 2);
        let focused = expanded
            .payload
            .n
            .iter()
            .find(|node| node.key == *key)
            .unwrap();
        assert_eq!(positions.get(key), Some(&(focused.x, focused.y)));
        assert_eq!(expanded.payload.e.len(), 1);
    }
    units.reverse();
    units.push(plan_unit(project, "another late unit"));
    storage
        .ensure_graph_slots(&slot_requests(&evidence, &units))
        .unwrap();
    let later_slots = storage.graph_slots().unwrap();
    for before in &original_slots {
        assert_eq!(
            later_slots.iter().find(|slot| slot.key == before.key),
            Some(before)
        );
    }
    let later = draw(&units, &later_slots, None);
    for node in &later.payload.n {
        if let Some(before) = positions.get(&node.key) {
            assert_eq!(*before, (node.x, node.y));
        }
    }
    let parent_key = format!("project:{project}");
    let highest_original = original_slots
        .iter()
        .filter(|slot| slot.parent_key.as_deref() == Some(parent_key.as_str()))
        .map(|slot| slot.slot)
        .max()
        .unwrap();
    let added = unit_key(project, "another late unit").unwrap();
    assert!(
        later_slots
            .iter()
            .find(|slot| slot.key == added)
            .unwrap()
            .slot
            > highest_original
    );
}

#[test]
fn curated_graph_unit_hash_namespace_cannot_alias_a_literal_name() {
    let hashed = unit_key(1, "phase one").unwrap();
    assert_eq!(hashed.len(), "unit:1:h:".len() + 40);
    assert_eq!(unit_key(1, "phase one"), Some(hashed.clone()));
    let literal = hashed.strip_prefix("unit:1:").unwrap();
    let literal_key = unit_key(1, literal).unwrap();
    assert_ne!(hashed, literal_key);
    assert_ne!(unit_key(1, "phase one"), unit_key(2, "phase one"));
    assert_ne!(unit_key(1, "phase one"), unit_key(1, "phase-one"));
    assert_ne!(unit_key(1, "phase one"), unit_key(1, "Phase one"));
    assert_ne!(unit_key(1, "phase one"), unit_key(1, "phase  one"));
    assert_ne!(unit_key(1, " x"), unit_key(1, "x "));
}

#[test]
fn curated_graph_unit_hashes_never_admit_refused_raw_identities() {
    let body: String = "a1B2c3D4e5F6".chars().cycle().take(40).collect();
    let token = ["sk-", "proj-", &body].concat();
    let assignment = ["password", "=", &body].concat();
    for raw in [
        token.clone(),
        assignment,
        format!("{} {token}", "ordinary ".repeat(40)),
    ] {
        assert!(check_identities([raw.as_str()]).is_err());
        assert!(unit_key(1, &raw).is_none());
        let requests = slot_requests(&Evidence::default(), &[plan_unit(1, &raw)]);
        assert!(requests.is_empty());
    }
    for raw in ["", ".", ".."] {
        assert!(unit_key(1, raw).is_none());
    }
    assert!(unit_key(0, "phase one").is_none());
    assert!(unit_key(-1, "phase one").is_none());
}

#[test]
fn curated_graph_key_domain_has_one_syntax_and_bound() {
    for key in [
        "project:1",
        "project:9223372036854775807",
        "entity:fixture-id",
        "entity:person:abc123",
        "unit:1:release-1_2.3",
        "ring:tech",
        "ring:person",
    ] {
        assert!(valid_key(key), "supported key {key}");
    }
    for key in [
        "project:0",
        "project:-1",
        "project:01",
        "entity:",
        "entity:..",
        "entity:person::abc",
        "unit:1:",
        "unit:1:phase one",
        "unit:1:phase:one",
        "unit:1:\u{043f}",
        "unit:1:%D0%BF",
        "ring:other",
        "other:1",
    ] {
        assert!(!valid_key(key), "unsupported key {key}");
    }
    assert!(valid_key(&format!("unit:1:{}", "x".repeat(160))));
    assert!(!valid_key(&format!("unit:1:{}", "x".repeat(161))));
    assert!(valid_key(&format!("entity:{}", "x".repeat(185))));
    assert!(!valid_key(&format!("entity:{}", "x".repeat(186))));
    assert!(valid_key(&format!("unit:1:h:{}", "a1".repeat(20))));
    assert!(!valid_key(&format!("unit:1:h:{}", "A1".repeat(20))));
    assert!(!valid_key(&format!("unit:1:h:{}", "g1".repeat(20))));
    for len in [0, 39, 41, 64] {
        assert!(!valid_key(&format!("unit:1:h:{}", "a".repeat(len))));
    }
}

#[test]
fn curated_graph_capped_expansion_preserves_its_late_slot_parent() {
    let now = 1_800_000_000;
    let mut evidence = Evidence::default();
    // 150 lower-slot related hubs compete with the focused part's parent.
    for id in 1..=152 {
        evidence.projects.push(ProjectEvidence {
            row: ProjectRow {
                id,
                slug: format!("p{id}"),
                name: "Project".into(),
                parent_id: (id == 152).then_some(151),
                entity_id: None,
                state: ProjectState::Active,
                slot: id - 1,
                merged_into: None,
                prev_entity_type: None,
            },
            memory_count: 2,
            last_memory: Some(now),
            last_segment: None,
        });
        if id <= 150 {
            evidence.links.push(EvidenceLink {
                source: "project:152".into(),
                target: format!("project:{id}"),
                kind: LinkKind::Relates,
                memory_count: 2,
            });
        }
    }
    let slots: Vec<_> = slot_requests(&evidence, &[])
        .into_iter()
        .map(|request| GraphSlot {
            key: request.key,
            parent_key: request.parent_key,
            slot: request.preferred_slot.unwrap_or(0),
        })
        .collect();
    let snapshot = render(
        Input {
            evidence: &evidence,
            units: &[],
            slots: &slots,
            hours_30d: &BTreeMap::new(),
            now,
        },
        Some("project:152"),
    )
    .unwrap();
    assert_eq!(snapshot.payload.n.len(), EXPANDED_NODES);
    assert_eq!(snapshot.payload.n[0].key, "project:152");
    let parent = snapshot
        .payload
        .n
        .iter()
        .position(|node| node.key == "project:151")
        .unwrap();
    assert_eq!(parent, 1);
    assert!(snapshot.payload.e.contains(&Edge(parent as u16, 0, 0, 0)));
    assert_eq!(snapshot.payload.o[0], 2);
}
