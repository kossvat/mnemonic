use super::*;
use crate::test_support::InTempDir;

fn store() -> InTempDir<Storage> {
    InTempDir::new("mnemonic-fact-store-", |dir| {
        Storage::open(&dir.join("memory.db")).unwrap()
    })
}

fn set<'a>(project: Option<&'a str>, value: Option<&'a str>) -> FactWrite<'a> {
    FactWrite {
        project,
        subject: "Widget",
        predicate: "price",
        value,
        actor: "test",
        ..Default::default()
    }
}

fn history(outcome: &Outcome) -> Vec<Option<String>> {
    outcome
        .fact
        .history
        .iter()
        .map(|v| v.value.clone())
        .collect()
}

#[test]
fn supersede_chain_derived_valid_to_contiguous() {
    let s = store();
    assert_eq!(apply(&s, &set(None, Some("$5"))).unwrap().outcome, "create");
    let six = apply(&s, &set(None, Some("$6"))).unwrap();
    assert_eq!(six.outcome, "update");
    assert_eq!(six.replaced.as_ref().unwrap().value.as_deref(), Some("$5"));
    let seven = apply(&s, &set(None, Some("$7"))).unwrap();
    assert_eq!(
        history(&seven),
        vec![Some("$7".into()), Some("$6".into()), Some("$5".into())]
    );
    let h = &seven.fact.history;
    assert_eq!(h[0].valid_to, None);
    assert_eq!(h[1].valid_to.as_deref(), Some(h[0].valid_from.as_str()));
    assert_eq!(h[2].valid_to.as_deref(), Some(h[1].valid_from.as_str()));
    assert_eq!(seven.fact.current.unwrap().value.as_deref(), Some("$7"));
}

#[test]
fn equal_reassert_writes_no_row() {
    let s = store();
    apply(&s, &set(None, Some("$5"))).unwrap();
    let again = apply(&s, &set(None, Some("$5.00"))).unwrap();
    assert_eq!(again.outcome, "reconfirm");
    assert_eq!(again.fact.history.len(), 1);
    assert_eq!(value_count(&s).unwrap(), 1);
}

#[test]
fn backdated_value_is_history() {
    let s = store();
    apply(&s, &set(None, Some("$5"))).unwrap();
    let old = apply(
        &s,
        &FactWrite {
            as_of: Some("2020-01-01"),
            ..set(None, Some("$4"))
        },
    )
    .unwrap();
    assert_eq!(old.outcome, "history");
    assert_eq!(
        old.fact.current.as_ref().unwrap().value.as_deref(),
        Some("$5")
    );
    assert_eq!(history(&old), vec![Some("$5".into()), Some("$4".into())]);
}

#[test]
fn request_id_replay_writes_nothing_and_events_hold_no_values() {
    let s = store();
    let write = FactWrite {
        request_id: Some("req-1"),
        ..set(None, Some("$5"))
    };
    apply(&s, &write).unwrap();
    let replay = apply(&s, &write).unwrap();
    assert!(replay.replayed);
    assert_eq!(value_count(&s).unwrap(), 1);
    let conn = s.conn.lock().unwrap();
    let mut stmt = conn.prepare("SELECT * FROM fact_events").unwrap();
    let columns = stmt.column_count();
    let rows: Vec<String> = stmt
        .query_map([], |r| {
            Ok((0..columns)
                .map(|i| format!("{:?}", r.get_ref(i).unwrap()))
                .collect::<Vec<_>>()
                .join("|"))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert!(!rows.iter().any(|row| row.contains("$5")), "{rows:?}");
}

#[test]
fn expected_revision_conflict_leaves_store_unchanged() {
    let s = store();
    let first = apply(&s, &set(None, Some("$5"))).unwrap();
    let stale = FactWrite {
        expected_revision: Some(first.fact.revision - 1),
        ..set(None, Some("$6"))
    };
    let refused = apply(&s, &stale).unwrap_err().to_string();
    assert!(refused.contains("revision"), "{refused}");
    assert_eq!(value_count(&s).unwrap(), 1);
    let fresh = FactWrite {
        expected_revision: Some(first.fact.revision),
        ..set(None, Some("$6"))
    };
    assert_eq!(apply(&s, &fresh).unwrap().outcome, "update");
}

#[test]
fn racing_handles_commit_order_equals_chain_order() {
    let dir = crate::test_support::temp_dir("mnemonic-fact-race-");
    let path = dir.path().join("memory.db");
    let (one, two) = (Storage::open(&path).unwrap(), Storage::open(&path).unwrap());
    apply(&one, &set(None, Some("$5"))).unwrap();
    apply(&two, &set(None, Some("$6"))).unwrap();
    let last = apply(&one, &set(None, Some("$7"))).unwrap();
    // Whatever the clocks say, the last commit is the current value.
    assert_eq!(
        history(&last),
        vec![Some("$7".into()), Some("$6".into()), Some("$5".into())]
    );
}

#[test]
fn projects_are_separate_slots() {
    let s = store();
    apply(&s, &set(Some("alpha-shop"), Some("$5"))).unwrap();
    apply(&s, &set(Some("beta-lab"), Some("$9"))).unwrap();
    let alpha = views(&s, Some("alpha-shop"), "Widget").unwrap();
    assert_eq!(alpha.len(), 1);
    assert_eq!(
        alpha[0].current.as_ref().unwrap().value.as_deref(),
        Some("$5")
    );
    assert_eq!(views(&s, None, "widget").unwrap().len(), 2);
}

#[test]
fn a_retraction_ends_the_value_and_a_new_one_starts_again() {
    let s = store();
    apply(&s, &set(None, Some("$5"))).unwrap();
    let gone = apply(&s, &set(None, None)).unwrap();
    assert_eq!(gone.outcome, "retract");
    assert!(gone.fact.current.is_none());
    assert!(apply(&s, &set(None, None)).unwrap().outcome == "reconfirm");
    assert_eq!(apply(&s, &set(None, Some("$6"))).unwrap().outcome, "create");
}

#[test]
fn forgetting_a_value_erases_it_and_the_chain_closes_up() {
    let s = store();
    apply(&s, &set(None, Some("$5"))).unwrap();
    let six = apply(&s, &set(None, Some("$6"))).unwrap();
    let erased = forget_value(&s, six.value_id.as_deref().unwrap()).unwrap();
    assert_eq!(erased.predicate, "price");
    let view = &views(&s, None, "Widget").unwrap()[0];
    assert_eq!(view.current.as_ref().unwrap().value.as_deref(), Some("$5"));
    assert_eq!(view.history.len(), 1);
    assert!(forget_value(&s, "ab").is_err());
}

#[test]
fn a_source_must_be_a_live_memory_and_forgetting_it_keeps_the_value() {
    let s = store();
    assert!(live_source(&s, Some("no-such-memory")).is_err());
    let memory = crate::event::MemoryEntry::new(
        "Widget price",
        "Widget price is $5",
        crate::event::MemoryType::Note,
        crate::event::EventSource::Manual,
    );
    s.save(&memory).unwrap();
    let source = live_source(&s, Some(&memory.id)).unwrap();
    let written = apply(
        &s,
        &FactWrite {
            source_memory_id: source.as_deref(),
            ..set(None, Some("$5"))
        },
    )
    .unwrap();
    assert_eq!(
        written.fact.current.unwrap().source_memory_id.as_deref(),
        Some(memory.id.as_str())
    );
    assert!(s.forget_by_id(&memory.id).unwrap());
    let view = &views(&s, None, "Widget").unwrap()[0];
    let current = view.current.as_ref().unwrap();
    assert_eq!(current.value.as_deref(), Some("$5"));
    assert_eq!(current.source_memory_id, None);
}

#[test]
fn future_values_and_empty_values_are_refused() {
    let s = store();
    let future = FactWrite {
        as_of: Some("2999-01-01"),
        ..set(None, Some("$5"))
    };
    assert!(apply(&s, &future).is_err());
    assert!(apply(&s, &set(None, Some("  "))).is_err());
    assert_eq!(value_count(&s).unwrap(), 0);
}

#[test]
fn a_backfill_does_not_outrank_a_later_reconfirmation() {
    let s = store();
    let at = |value: &'static str, as_of: &'static str| FactWrite {
        as_of: Some(as_of),
        ..set(None, Some(value))
    };
    apply(&s, &at("$5", "2026-01-01")).unwrap();
    assert_eq!(
        apply(&s, &at("$5", "2026-03-01")).unwrap().outcome,
        "reconfirm"
    );
    let six = apply(&s, &at("$6", "2026-02-01")).unwrap();
    assert_eq!(six.outcome, "history");
    assert_eq!(
        history(&six),
        vec![Some("$5".into()), Some("$6".into()), Some("$5".into())]
    );
    assert_eq!(six.fact.current.unwrap().value.as_deref(), Some("$5"));
    // The span moved to the copy: another backfill does not copy it again.
    assert_eq!(
        apply(&s, &at("$7", "2026-01-15")).unwrap().outcome,
        "history"
    );
    assert_eq!(value_count(&s).unwrap(), 4);
}

#[test]
fn the_same_write_from_the_same_source_twice_reconfirms() {
    let s = store();
    let memory = crate::event::MemoryEntry::new(
        "Widget price",
        "Widget price is $5",
        crate::event::MemoryType::Note,
        crate::event::EventSource::Manual,
    );
    s.save(&memory).unwrap();
    let from = |value| FactWrite {
        source_memory_id: Some(&memory.id),
        ..set(None, Some(value))
    };
    assert_eq!(apply(&s, &from("$5")).unwrap().outcome, "create");
    assert_eq!(apply(&s, &from("$5")).unwrap().outcome, "reconfirm");
    assert_eq!(apply(&s, &from("$6")).unwrap().outcome, "update");
}

#[test]
fn a_write_checks_its_source_in_its_own_transaction() {
    let s = store();
    let write = FactWrite {
        source_memory_id: Some("no-such-memory"),
        ..set(None, Some("$5"))
    };
    let err = apply(&s, &write).unwrap_err();
    assert!(err.to_string().contains("no live memory"), "{err}");
    assert_eq!(value_count(&s).unwrap(), 0);
}

#[test]
fn the_same_proposal_waits_once() {
    let s = store();
    apply(&s, &set(None, Some("$5"))).unwrap();
    let proposal = FactWrite {
        trust: Some(Trust::Provisional),
        ..set(None, Some("$6"))
    };
    let first = apply(&s, &proposal).unwrap();
    assert_eq!(first.outcome, "pending_review");
    let again = apply(&s, &proposal).unwrap();
    assert_eq!(again.value_id, first.value_id);
    assert_eq!(again.fact.pending, 1);
}

fn memory(s: &Storage) -> String {
    let memory = crate::event::MemoryEntry::new(
        "Widget price",
        "Widget price is $5",
        crate::event::MemoryType::Note,
        crate::event::EventSource::Manual,
    );
    s.save(&memory).unwrap();
    memory.id
}

#[test]
fn a_value_that_comes_back_keeps_its_source() {
    let s = store();
    let source = memory(&s);
    let at = |value: &'static str, as_of: &'static str| FactWrite {
        as_of: Some(as_of),
        ..set(None, Some(value))
    };
    apply(
        &s,
        &FactWrite {
            source_memory_id: Some(&source),
            ..at("$5", "2026-01-01")
        },
    )
    .unwrap();
    apply(&s, &at("$5", "2026-03-01")).unwrap();
    let six = apply(&s, &at("$6", "2026-02-01")).unwrap();
    let current = six.fact.current.unwrap();
    assert_eq!(current.value.as_deref(), Some("$5"));
    assert_eq!(current.source_memory_id.as_deref(), Some(source.as_str()));
}

#[test]
fn an_implicit_now_lands_after_a_reconfirmation_dated_ahead() {
    let s = store();
    apply(&s, &set(None, Some("$5"))).unwrap();
    let ahead = (Utc::now() + chrono::Duration::minutes(2)).to_rfc3339();
    let again = FactWrite {
        as_of: Some(&ahead),
        ..set(None, Some("$5"))
    };
    assert_eq!(apply(&s, &again).unwrap().outcome, "reconfirm");
    let six = apply(&s, &set(None, Some("$6"))).unwrap();
    assert_eq!(six.outcome, "update");
    assert_eq!(six.fact.current.unwrap().value.as_deref(), Some("$6"));
}

#[test]
fn stating_a_proposed_value_makes_it_authoritative() {
    let s = store();
    let source = memory(&s);
    let status = |value, trust| FactWrite {
        subject: "Order 7",
        predicate: "status",
        value: Some(value),
        trust: Some(trust),
        actor: "test",
        ..Default::default()
    };
    apply(
        &s,
        &FactWrite {
            source_memory_id: Some(&source),
            ..status("on hold", Trust::Provisional)
        },
    )
    .unwrap();
    let adopted = apply(&s, &status("on hold", Trust::Declared)).unwrap();
    assert_eq!(adopted.outcome, "reconfirm");
    assert_eq!(adopted.fact.current.as_ref().unwrap().trust, "declared");
    // A later proposal no longer replaces it, and its source going away
    // no longer takes it with it.
    let proposal = apply(&s, &status("shipped", Trust::Provisional)).unwrap();
    assert_eq!(proposal.outcome, "pending_review");
    assert!(s.forget_by_id(&source).unwrap());
    let view = &views(&s, None, "Order 7").unwrap()[0];
    assert_eq!(
        view.current.as_ref().unwrap().value.as_deref(),
        Some("on hold")
    );
}

#[test]
fn a_backfill_does_not_bring_back_a_value_replaced_at_its_reconfirmed_time() {
    let s = store();
    let at = |value: &'static str, as_of: &'static str| FactWrite {
        as_of: Some(as_of),
        ..set(None, Some(value))
    };
    apply(&s, &at("$5", "2026-01-01")).unwrap();
    assert_eq!(
        apply(&s, &at("$5", "2026-03-01")).unwrap().outcome,
        "reconfirm"
    );
    assert_eq!(
        apply(&s, &at("$6", "2026-03-01")).unwrap().outcome,
        "update"
    );
    let seven = apply(&s, &at("$7", "2026-02-01")).unwrap();
    assert_eq!(seven.outcome, "history");
    assert_eq!(
        seven.fact.current.as_ref().unwrap().value.as_deref(),
        Some("$6")
    );
    assert_eq!(
        history(&seven),
        vec![Some("$6".into()), Some("$7".into()), Some("$5".into())]
    );
}
