use super::*;
use crate::facts::rule::Trust;
use crate::facts::store::{self, FactWrite};
use crate::storage::Storage;
use crate::test_support::InTempDir;

fn store() -> InTempDir<Storage> {
    InTempDir::new("mnemonic-fact-rekey-", |dir| {
        Storage::open(&dir.join("memory.db")).unwrap()
    })
}

fn write(value: &str) -> FactWrite<'_> {
    FactWrite {
        project: Some("alpha-shop"),
        subject: "Widget",
        predicate: "price",
        value: Some(value),
        actor: "test",
        ..Default::default()
    }
}

fn merge(s: &Storage, old: &str, new: &str) {
    let conn = s.conn.lock().unwrap();
    rekey_in_tx(&conn, old, new).unwrap();
}

#[test]
fn merging_a_subject_moves_its_facts_and_the_newest_stays_current() {
    let s = store();
    let old = FactWrite {
        subject: "orbit",
        ..write("$5")
    };
    store::apply(&s, &old).unwrap();
    let new = FactWrite {
        subject: "orbit-supply",
        ..write("$6")
    };
    store::apply(&s, &new).unwrap();
    {
        let conn = s.conn.lock().unwrap();
        rekey_in_tx(&conn, "orbit", "orbit-supply").unwrap();
    }
    let views = store::views(&s, Some("alpha-shop"), "orbit-supply").unwrap();
    assert_eq!(views.len(), 1);
    assert_eq!(views[0].history.len(), 2);
    assert_eq!(
        views[0].current.as_ref().unwrap().value.as_deref(),
        Some("$6")
    );
    assert!(
        store::views(&s, Some("alpha-shop"), "orbit")
            .unwrap()
            .is_empty()
    );
}

#[test]
fn a_proposal_already_in_the_kept_slot_waits_for_review_too() {
    let s = store();
    store::apply(
        &s,
        &FactWrite {
            subject: "orbit",
            as_of: Some("2026-01-01"),
            ..write("$5")
        },
    )
    .unwrap();
    store::apply(
        &s,
        &FactWrite {
            subject: "orbit-supply",
            trust: Some(Trust::Provisional),
            ..write("$9")
        },
    )
    .unwrap();
    merge(&s, "orbit", "orbit-supply");
    let view = &store::views(&s, Some("alpha-shop"), "orbit-supply").unwrap()[0];
    assert_eq!(view.current.as_ref().unwrap().value.as_deref(), Some("$5"));
    assert_eq!(view.pending, 1);
}

#[test]
fn a_reconfirmed_span_survives_a_merge() {
    let s = store();
    let at = |subject, value, as_of| FactWrite {
        subject,
        as_of: Some(as_of),
        ..write(value)
    };
    store::apply(&s, &at("orbit", "$5", "2026-01-01")).unwrap();
    store::apply(&s, &at("orbit", "$5", "2026-03-01")).unwrap();
    store::apply(&s, &at("orbit-supply", "$6", "2026-02-01")).unwrap();
    merge(&s, "orbit", "orbit-supply");
    let view = &store::views(&s, Some("alpha-shop"), "orbit-supply").unwrap()[0];
    let trail: Vec<_> = view
        .history
        .iter()
        .map(|v| v.value.as_deref().unwrap())
        .collect();
    assert_eq!(trail, vec!["$5", "$6", "$5"]);
    assert_eq!(view.current.as_ref().unwrap().value.as_deref(), Some("$5"));
}

#[test]
fn a_span_comes_back_after_the_last_interruption() {
    let s = store();
    let at = |subject, value, as_of| FactWrite {
        subject,
        as_of: Some(as_of),
        ..write(value)
    };
    store::apply(&s, &at("orbit", "$5", "2026-01-01")).unwrap();
    store::apply(&s, &at("orbit", "$5", "2026-04-01")).unwrap();
    store::apply(&s, &at("orbit-supply", "$6", "2026-02-01")).unwrap();
    store::apply(&s, &at("orbit-supply", "$5", "2026-03-01")).unwrap();
    store::apply(&s, &at("orbit-supply", "$7", "2026-03-15")).unwrap();
    merge(&s, "orbit", "orbit-supply");
    let view = &store::views(&s, Some("alpha-shop"), "orbit-supply").unwrap()[0];
    let trail: Vec<_> = view
        .history
        .iter()
        .map(|v| v.value.as_deref().unwrap())
        .collect();
    // $5 was said through April: it comes back after $7, once.
    assert_eq!(trail, vec!["$5", "$7", "$5", "$6", "$5"]);
}

#[test]
fn a_span_the_same_value_continues_moves_to_it() {
    let s = store();
    let at = |subject, value, as_of| FactWrite {
        subject,
        as_of: Some(as_of),
        ..write(value)
    };
    store::apply(&s, &at("orbit", "$5", "2026-01-01")).unwrap();
    store::apply(&s, &at("orbit", "$5", "2026-04-01")).unwrap();
    store::apply(&s, &at("orbit-supply", "$6", "2026-02-01")).unwrap();
    store::apply(&s, &at("orbit-supply", "$5", "2026-03-01")).unwrap();
    merge(&s, "orbit", "orbit-supply");
    let trail = |s: &Storage| -> Vec<String> {
        store::views(s, Some("alpha-shop"), "orbit-supply").unwrap()[0]
            .history
            .iter()
            .map(|v| v.value.clone().unwrap())
            .collect()
    };
    // $5 already holds again from March: no copy is needed.
    assert_eq!(trail(&s), vec!["$5", "$6", "$5"]);
    // The March row now carries the span through April.
    let eight = store::apply(&s, &at("orbit-supply", "$8", "2026-03-20")).unwrap();
    assert_eq!(eight.outcome, "history");
    assert_eq!(trail(&s), vec!["$5", "$8", "$5", "$6", "$5"]);
}

#[test]
fn a_retried_request_still_replays_after_its_slot_was_merged() {
    let s = store();
    let old = FactWrite {
        subject: "orbit",
        request_id: Some("r1"),
        ..write("$5")
    };
    store::apply(&s, &old).unwrap();
    store::apply(
        &s,
        &FactWrite {
            subject: "orbit-supply",
            ..write("$6")
        },
    )
    .unwrap();
    merge(&s, "orbit", "orbit-supply");
    let again = store::apply(&s, &old).unwrap();
    assert!(again.replayed);
    assert_eq!(again.fact.subject, "orbit-supply");
}

#[test]
fn a_proposal_merged_into_a_stated_fact_waits_for_review() {
    let s = store();
    store::apply(
        &s,
        &FactWrite {
            subject: "orbit",
            as_of: Some("2026-01-01"),
            ..write("$5")
        },
    )
    .unwrap();
    store::apply(
        &s,
        &FactWrite {
            subject: "orbit-supply",
            trust: Some(Trust::Provisional),
            ..write("$9")
        },
    )
    .unwrap();
    merge(&s, "orbit-supply", "orbit");
    let view = &store::views(&s, Some("alpha-shop"), "orbit").unwrap()[0];
    assert_eq!(view.current.as_ref().unwrap().value.as_deref(), Some("$5"));
    assert_eq!(view.pending, 1);
}

#[test]
fn a_revision_read_before_a_merge_no_longer_passes() {
    let s = store();
    let alias = |value| FactWrite {
        subject: "orbit",
        ..write(value)
    };
    store::apply(&s, &alias("$5")).unwrap();
    let seen = store::apply(&s, &alias("$6")).unwrap().fact.revision;
    store::apply(
        &s,
        &FactWrite {
            subject: "orbit-supply",
            ..write("$7")
        },
    )
    .unwrap();
    merge(&s, "orbit", "orbit-supply");
    let stale = FactWrite {
        subject: "orbit-supply",
        expected_revision: Some(seen),
        ..write("$8")
    };
    assert!(store::apply(&s, &stale).is_err());
}

#[test]
fn a_proposal_repeating_a_stated_value_is_absorbed_by_it() {
    let s = store();
    let status = |subject, value, trust, as_of| FactWrite {
        subject,
        predicate: "status",
        trust: Some(trust),
        as_of,
        ..write(value)
    };
    store::apply(
        &s,
        &status("orbit", "ready", Trust::Manual, Some("2026-01-01")),
    )
    .unwrap();
    store::apply(
        &s,
        &status("orbit-supply", "ready", Trust::Provisional, None),
    )
    .unwrap();
    merge(&s, "orbit-supply", "orbit");
    let view = &store::views(&s, Some("alpha-shop"), "orbit").unwrap()[0];
    assert_eq!(view.current.as_ref().unwrap().trust, "manual");
    assert_eq!(view.history.len(), 1);
    let closed = store::apply(&s, &status("orbit", "closed", Trust::Provisional, None)).unwrap();
    assert_eq!(closed.outcome, "pending_review");
}
