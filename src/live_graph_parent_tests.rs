use super::*;
use crate::curated_graph::{Kind, Snapshot};
use crate::storage::curated_graph::LinkKind;
use std::collections::{BTreeMap, BTreeSet};

fn positions(snapshot: &Snapshot) -> BTreeMap<String, (i32, i32)> {
    snapshot
        .payload
        .n
        .iter()
        .filter(|node| matches!(node.kind, Kind::Project | Kind::Part))
        .map(|node| (node.key.clone(), (node.x, node.y)))
        .collect()
}

fn contains(snapshot: &Snapshot) -> BTreeSet<(String, String)> {
    snapshot
        .payload
        .e
        .iter()
        .filter(|edge| edge.2 == 0)
        .map(|edge| {
            (
                snapshot.payload.n[usize::from(edge.0)].key.clone(),
                snapshot.payload.n[usize::from(edge.1)].key.clone(),
            )
        })
        .collect()
}

fn cycle_case(allocate_before_merges: bool) {
    let f = Fixture::new();
    let alpha = f.project("alpha-root");
    let beta = f.project("beta-root");
    let alpha_part = f
        .storage
        .project_upsert("alpha-part", "Alpha part", Some(alpha))
        .unwrap();
    let beta_part = f
        .storage
        .project_upsert("beta-part", "Beta part", Some(beta))
        .unwrap();
    for id in [alpha, beta, alpha_part, beta_part] {
        f.segment(
            &format!("current-{id}"),
            id,
            f.now - Duration::minutes(2),
            f.now - Duration::minutes(1),
        );
    }
    let previous = if allocate_before_merges {
        assert!(f.refresh(0));
        positions(&f.live.graph_snapshot(None).unwrap().unwrap())
    } else {
        BTreeMap::new()
    };
    // Neither merge redirects into its own canonical group. Together their
    // canonical parent links form a cycle between the two surviving parts.
    f.storage.project_merge(alpha, beta_part).unwrap();
    f.storage.project_merge(beta, alpha_part).unwrap();
    assert!(f.refresh(60));
    let first = format!("project:{alpha_part}");
    let second = format!("project:{beta_part}");
    let expected = BTreeSet::from([
        (first.clone(), second.clone()),
        (second.clone(), first.clone()),
    ]);
    let overview = f.live.graph_snapshot(None).unwrap().unwrap();
    let placed = positions(&overview);
    assert_eq!(
        placed.keys().cloned().collect::<BTreeSet<_>>(),
        BTreeSet::from([first.clone(), second.clone(),])
    );
    assert_eq!(contains(&overview), expected);
    if allocate_before_merges {
        assert_eq!(placed[&first], previous[&first]);
        assert_eq!(placed[&second], previous[&second]);
    }
    let changes = f.changes();
    let evidence = f.storage.curated_graph_evidence(&[], &[]).unwrap();
    let ancestry: BTreeMap<_, _> = evidence
        .projects
        .iter()
        .map(|project| (project.row.id, project.row.parent_id))
        .collect();
    assert_eq!(ancestry[&alpha_part], None);
    assert_eq!(ancestry[&beta_part], Some(alpha_part));
    let source_links: BTreeSet<_> = evidence
        .links
        .iter()
        .filter(|link| link.kind == LinkKind::Contains)
        .map(|link| (link.source.clone(), link.target.clone()))
        .collect();
    assert_eq!(source_links, expected);
    for key in [&first, &second] {
        let expanded = f.live.graph_snapshot(Some(key)).unwrap().unwrap();
        assert_eq!(positions(&expanded), placed);
        assert_eq!(contains(&expanded), expected);
    }
    assert_eq!(f.changes(), changes);
    assert!(!f.refresh(120));
    assert_eq!(f.changes(), changes);
    assert_eq!(
        f.live.graph_snapshot(None).unwrap().unwrap().bytes,
        overview.bytes
    );
    f.storage.project_unmerge(alpha).unwrap();
    f.storage.project_unmerge(beta).unwrap();
    assert!(f.refresh(180));
    let restored = positions(&f.live.graph_snapshot(None).unwrap().unwrap());
    assert_eq!(restored[&first], placed[&first]);
    assert_eq!(restored[&second], placed[&second]);
    assert_eq!(restored.len(), 4);
}

#[test]
fn curated_graph_runtime_parent_cycles_before_slot_allocation_keep_both_projects() {
    cycle_case(false);
}

#[test]
fn curated_graph_runtime_parent_cycles_after_slot_allocation_preserve_positions() {
    cycle_case(true);
}
