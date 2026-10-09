//! Helper to copy into the daemon's curated route tests. All data is invented.
//! The output bytes must equal clients/macos/Tests/Fixtures/curated-graph.json.
use crate::curated_graph::{Hours, Input, PlanUnit, Snapshot, render};
use crate::storage::curated_graph::{
    ArchivedCandidate, EntityEvidence, EntityKind, Evidence, EvidenceLink, GraphSlot, LinkKind,
    ProjectEvidence,
};
use crate::storage::projects::{ProjectRow, ProjectState};
use std::collections::BTreeMap;

pub(super) fn swift_curated_fixture() -> Snapshot {
    const NOW: i64 = 1_800_000_000;
    const DAY: i64 = 86_400;
    let project = |id, slug: &str, name: &str, parent_id, slot, days, count| ProjectEvidence {
        row: ProjectRow {
            id,
            slug: slug.into(),
            name: name.into(),
            parent_id,
            entity_id: None,
            state: ProjectState::Active,
            slot,
            merged_into: None,
            prev_entity_type: None,
        },
        memory_count: count,
        last_memory: Some(NOW - days * DAY),
        last_segment: None,
    };
    let entity = |id: &str, name: &str, kind, days, count| EntityEvidence {
        id: id.into(),
        name: name.into(),
        kind,
        memory_count: count,
        last_memory: Some(NOW - days * DAY),
    };
    let link = |target: &str, kind, count| EvidenceLink {
        source: "project:1".into(),
        target: target.into(),
        kind,
        memory_count: count,
    };
    let evidence = Evidence {
        projects: vec![
            project(1, "harbor", "Harbor", None, 0, 1, 3),
            project(2, "harbor:api", "Harbor API", Some(1), 1, 2, 2),
            project(3, "beacon", "Beacon", None, 2, 10, 2),
            project(4, "ledger", "Ledger", None, 3, 45, 4),
        ],
        entities: vec![
            entity("rust", "Rust", EntityKind::Tech, 3, 3),
            entity("mira", "Mira", EntityKind::Person, 9, 2),
        ],
        links: vec![
            link("entity:rust", LinkKind::Uses, 3),
            link("entity:mira", LinkKind::Involves, 2),
            link("project:3", LinkKind::Relates, 2),
        ],
        shelf: vec![ArchivedCandidate {
            id: "phantom".into(),
            name: "Phantom board".into(),
            memory_count: 2,
            last_memory: None,
            archived_at: None,
        }],
    };
    let units = vec![PlanUnit {
        project_id: 1,
        id: "LP-12".into(),
        title: "Review change".into(),
        closed_at: Some(NOW - DAY),
        last_memory: None,
        memory_count: 0,
    }];
    let slot = |key: &str, parent: Option<&str>, slot| GraphSlot {
        key: key.into(),
        parent_key: parent.map(str::to_owned),
        slot,
    };
    let slots = vec![
        slot("project:1", None, 0),
        slot("project:2", Some("project:1"), 0),
        slot("project:3", None, 2),
        slot("project:4", None, 3),
        slot("unit:1:LP-12", Some("project:1"), 1),
        slot("entity:rust", Some("ring:tech"), 0),
        slot("entity:mira", Some("ring:person"), 0),
    ];
    let hours = BTreeMap::from([
        (
            1,
            Hours {
                owner_seconds: Some(5_400),
                agent_seconds: Some(43_200),
            },
        ),
        (
            2,
            Hours {
                owner_seconds: None,
                agent_seconds: Some(0),
            },
        ),
        (
            3,
            Hours {
                owner_seconds: Some(0),
                agent_seconds: None,
            },
        ),
    ]);
    render(
        Input {
            evidence: &evidence,
            units: &units,
            slots: &slots,
            hours_30d: &hours,
            now: NOW,
        },
        None,
    )
    .unwrap()
}
