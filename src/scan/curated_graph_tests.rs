//! New state tables remain covered by the closed-snapshot scanner.
use super::*;
use std::collections::BTreeSet;

#[test]
fn curated_graph_state_identities_are_scanned_without_disclosing_values() {
    let f = Fixture::private();
    let credential = token();
    // A legacy or externally modified snapshot can bypass admission.
    f.conn()
        .execute(
            "INSERT INTO entity_archive(entity_id,at) VALUES (?1,1)",
            [&credential],
        )
        .unwrap();
    f.conn()
        .execute(
            "INSERT INTO graph_slots(node_key,parent_key,slot) VALUES (?1,NULL,0)",
            [&credential],
        )
        .unwrap();
    f.conn()
        .execute(
            "INSERT INTO graph_slots(node_key,parent_key,slot) VALUES ('project:1',?1,0)",
            [&credential],
        )
        .unwrap();
    let report = scanned(&f.snapshot());
    assert!(report.complete && report.exit_code() == 2);
    assert_eq!(
        places(&report).into_iter().collect::<BTreeSet<_>>(),
        BTreeSet::from([
            ("entity_archive", "entity_id"),
            ("graph_slots", "node_key"),
            ("graph_slots", "parent_key"),
        ])
    );
    assert!(!report.to_json().contains(&credential));
}
