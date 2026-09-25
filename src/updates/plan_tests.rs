//! The save gate against a real store. Every memory carries the same vector,
//! so every existing memory is a near duplicate and only the gate's rule
//! decides.
use super::*;
use crate::embedding::Embedder;
use crate::event::{EventSource, MemoryType};
use crate::test_support::{ConstEmbedder, InTempDir};
use chrono::Duration;

const THRESHOLD: f32 = 0.92;

fn store() -> InTempDir<Storage> {
    InTempDir::new("mnemonic-plan-", |dir| {
        Storage::open(&dir.join("memory.db")).unwrap()
    })
}

fn vector() -> Embedding {
    ConstEmbedder.embed("").unwrap()
}

fn entry(text: &str, minutes_ago: i64) -> MemoryEntry {
    let mut entry = MemoryEntry::new(text, text, MemoryType::Note, EventSource::Manual);
    entry.timestamp = Utc::now() - Duration::minutes(minutes_ago);
    entry
}

/// Plan and, when the plan saves, save with its link. Returns the plan and
/// the saved id.
fn save(storage: &Storage, entry: &MemoryEntry, mode: Mode) -> (Plan, Option<String>) {
    let plan = plan_save(storage, entry, &vector(), THRESHOLD, mode, true).unwrap();
    let saved = match &plan {
        Plan::Save { link, .. } => {
            storage
                .save_with_links(entry, Some(&vector()), link.as_ref(), "test")
                .unwrap();
            Some(entry.id.clone())
        }
        Plan::Duplicate { .. } => None,
    };
    (plan, saved)
}

fn saved(storage: &Storage, text: &str, minutes_ago: i64) -> String {
    let e = entry(text, minutes_ago);
    let (plan, id) = save(storage, &e, Mode::Normal);
    id.unwrap_or_else(|| panic!("{text}: {plan:?}"))
}

fn target(plan: &Plan) -> Option<&str> {
    match plan {
        Plan::Save {
            link: Some(link), ..
        } => Some(&link.target),
        _ => None,
    }
}

fn duplicate_of(plan: &Plan) -> Option<&str> {
    match plan {
        Plan::Duplicate { of, .. } => Some(of),
        _ => None,
    }
}

fn head_of(storage: &Storage, id: &str) -> String {
    store::head(&storage.conn.lock().unwrap(), id)
        .unwrap()
        .unwrap()
}

#[test]
fn text_without_values_keeps_the_plain_rule() {
    let s = store();
    let first = saved(&s, "Released the parser", 10);
    let (plan, _) = save(&s, &entry("Released the parser again", 0), Mode::Normal);
    assert_eq!(duplicate_of(&plan), Some(first.as_str()));
}

#[test]
fn a_changed_value_is_saved_and_linked_to_what_it_changes() {
    let s = store();
    let five = saved(&s, "Widget price is $5", 10);
    let (plan, six) = save(&s, &entry("Widget price is now $6", 0), Mode::Normal);
    assert_eq!(target(&plan), Some(five.as_str()));
    assert_eq!(head_of(&s, &five), six.unwrap());
}

#[test]
fn the_same_value_again_is_a_duplicate() {
    let s = store();
    let five = saved(&s, "Widget price is $5", 10);
    let (plan, _) = save(&s, &entry("Widget price is $5", 0), Mode::Normal);
    assert_eq!(duplicate_of(&plan), Some(five.as_str()));
}

#[test]
fn a_revert_links_to_the_head_instead_of_vanishing() {
    let s = store();
    saved(&s, "Widget price is $5", 20);
    let six = saved(&s, "Widget price is now $6", 10);
    let (plan, back) = save(&s, &entry("Widget price is back to $5", 0), Mode::Normal);
    assert_eq!(target(&plan), Some(six.as_str()));
    assert!(back.is_some());
}

#[test]
fn a_replayed_old_statement_is_not_new_again() {
    let s = store();
    let five = saved(&s, "Widget price is $5", 20);
    saved(&s, "Widget price is now $6", 10);
    // The $5 statement arrives again, dated before $6 replaced it.
    let (plan, _) = save(&s, &entry("Widget price is $5", 15), Mode::Normal);
    assert_eq!(duplicate_of(&plan), Some(five.as_str()));
}

#[test]
fn an_older_value_is_filed_as_history() {
    let s = store();
    let five = saved(&s, "Widget price is $5", 10);
    // Read from a transcript months old: it precedes what is stored.
    let (plan, four) = save(&s, &entry("Widget price is $4", 60 * 24 * 30), Mode::Normal);
    let Plan::Save {
        link: Some(link), ..
    } = &plan
    else {
        panic!("{plan:?}");
    };
    assert!(!link.new_is_newer);
    // The stored $5 stays current; the old $4 is what it replaced.
    assert_eq!(head_of(&s, &four.unwrap()), five);
}

#[test]
fn other_projects_neither_drop_nor_link() {
    let s = store();
    let mut alpha = entry("Widget price is $5", 10);
    set_project(&s, &mut alpha, "alpha-shop").unwrap();
    save(&s, &alpha, Mode::Normal);
    let mut beta = entry("Widget price is $5", 0);
    set_project(&s, &mut beta, "beta-lab").unwrap();
    let (plan, id) = save(&s, &beta, Mode::Normal);
    assert!(id.is_some(), "{plan:?}");
    assert_eq!(target(&plan), None);
}

#[test]
fn derived_memories_neither_drop_nor_anchor() {
    let s = store();
    let mut summary = entry("Widget price is $5", 10);
    summary.memory_type = MemoryType::SessionSummary;
    s.save_with_embedding(&summary, Some(&vector())).unwrap();
    // The same value a summary quotes is not dropped by it ...
    let (plan, id) = save(&s, &entry("Widget price is $5", 0), Mode::Normal);
    assert!(id.is_some(), "{plan:?}");
    assert_eq!(target(&plan), None);
    // ... and a changed value links to the real memory, never the summary.
    let (plan, _) = save(&s, &entry("Widget price is $6", 0), Mode::Normal);
    assert_eq!(target(&plan), id.as_deref());
    assert_ne!(target(&plan), Some(summary.id.as_str()));
}

#[test]
fn a_correction_is_never_dropped_but_still_linked() {
    let s = store();
    let five = saved(&s, "Widget price is $5", 10);
    let (plan, id) = save(&s, &entry("Widget price is $5", 0), Mode::LinkOnly);
    assert!(id.is_some(), "{plan:?}");
    let (plan, _) = save(&s, &entry("Widget price is $6", 0), Mode::LinkOnly);
    assert!(target(&plan).is_some(), "{plan:?}");
    let _ = five;
}

#[test]
fn switching_value_awareness_off_restores_the_plain_rule() {
    let s = store();
    let five = saved(&s, "Widget price is $5", 10);
    let plan = plan_save(
        &s,
        &entry("Widget price is now $6", 0),
        &vector(),
        THRESHOLD,
        Mode::Normal,
        false,
    )
    .unwrap();
    assert_eq!(duplicate_of(&plan), Some(five.as_str()));
}

#[test]
fn racing_handles_chain_not_fork() {
    let dir = crate::test_support::temp_dir("mnemonic-plan-race-");
    let path = dir.path().join("memory.db");
    let (one, two) = (Storage::open(&path).unwrap(), Storage::open(&path).unwrap());
    let five = saved(&one, "Widget price is $5", 20);
    // Both plan against $5 ...
    let six = entry("Widget price is now $6", 1);
    let plan_six = plan_save(&one, &six, &vector(), THRESHOLD, Mode::Normal, true).unwrap();
    let seven = entry("Widget price is now $7", 5);
    let plan_seven = plan_save(&two, &seven, &vector(), THRESHOLD, Mode::Normal, true).unwrap();
    assert_eq!(target(&plan_six), Some(five.as_str()));
    assert_eq!(target(&plan_seven), Some(five.as_str()));
    // ... $7 commits first, then $6 finds the chain moved and follows it.
    let Plan::Save { link, .. } = &plan_seven else {
        panic!()
    };
    two.save_with_links(&seven, Some(&vector()), link.as_ref(), "test")
        .unwrap();
    let Plan::Save { link, .. } = &plan_six else {
        panic!()
    };
    let written = one
        .save_with_links(&six, Some(&vector()), link.as_ref(), "test")
        .unwrap()
        .unwrap();
    assert_eq!(written.old_id, seven.id);
    assert_eq!(head_of(&one, &five), six.id);
}

#[test]
fn a_chain_into_another_project_neither_drops_nor_links() {
    let s = store();
    // An unscoped $5, then alpha-shop's $6 updating it.
    let five = saved(&s, "Widget price is $5", 30);
    let mut alpha = entry("Widget price is now $6", 20);
    set_project(&s, &mut alpha, "alpha-shop").unwrap();
    let (plan, _) = save(&s, &alpha, Mode::Normal);
    assert_eq!(target(&plan), Some(five.as_str()));
    // beta-lab reaches alpha-shop only through the unscoped $5: no say.
    let mut beta = entry("Widget price is now $6", 10);
    set_project(&s, &mut beta, "beta-lab").unwrap();
    let (plan, beta_six) = save(&s, &beta, Mode::Normal);
    assert!(beta_six.is_some(), "{plan:?}");
    assert_eq!(target(&plan), None);
    // Beta's own chain is its own: $7 updates beta's $6, never alpha's.
    let mut seven = entry("Widget price is now $7", 0);
    set_project(&s, &mut seven, "beta-lab").unwrap();
    let (plan, _) = save(&s, &seven, Mode::Normal);
    assert_eq!(target(&plan), beta_six.as_deref());
}

#[test]
fn a_derived_head_neither_drops_nor_links() {
    let s = store();
    let five = saved(&s, "Widget price is $5", 30);
    let mut summary = entry("Widget price is $6", 20);
    summary.memory_type = MemoryType::SessionSummary;
    s.save_with_embedding(&summary, Some(&vector())).unwrap();
    {
        let conn = s.conn.lock().unwrap();
        let plan = LinkPlan {
            target: five.clone(),
            new_is_newer: true,
            class: Class::Money,
            was: vec![],
            now: vec![],
            similarity: 1.0,
        };
        // Linked by hand: the gate itself never links a derived memory.
        let link = store::Link {
            new_id: summary.id.clone(),
            old_id: five.clone(),
            rule: store::Rule::ValueDiff,
            class: plan.class,
            was: vec![],
            now: vec![],
            similarity: None,
            actor: "test",
        };
        store::insert(&conn, &link).unwrap();
    }
    let (plan, id) = save(&s, &entry("Widget price is $6", 0), Mode::Normal);
    assert!(id.is_some(), "{plan:?}");
    assert_eq!(target(&plan), None);
}

#[test]
fn a_derived_new_memory_keeps_the_plain_rule() {
    let s = store();
    saved(&s, "Widget price is $5", 10);
    let mut summary = entry("Widget price is now $6", 0);
    summary.memory_type = MemoryType::SessionSummary;
    let plan = plan_save(&s, &summary, &vector(), THRESHOLD, Mode::Normal, true).unwrap();
    assert!(matches!(plan, Plan::Duplicate { .. }), "{plan:?}");
}

#[test]
fn a_long_chain_still_resolves_to_its_real_end() {
    let s = store();
    let first = saved(&s, "Widget price is $5", 100);
    let mut last = first.clone();
    for (step, price) in (6..30).enumerate() {
        last = saved(
            &s,
            &format!("Widget price is now ${price}"),
            99 - step as i64,
        );
    }
    assert_eq!(head_of(&s, &first), last);
}

#[test]
fn history_backfilled_out_of_order_keeps_time_order() {
    let s = store();
    let day = 60 * 24;
    let five = saved(&s, "Widget price is $5", 10);
    // Oldest first: $3, then $4 lands between it and $5.
    let three = saved(&s, "Widget price is $3", 60 * day);
    let four = saved(&s, "Widget price is $4", 30 * day);
    assert_eq!(head_of(&s, &three), five);
    // Forgetting the head leaves one chain, not two loose values.
    assert!(s.forget_by_id(&five).unwrap());
    assert_eq!(head_of(&s, &three), four);
    // So a current $3 is a revert from $4, not a duplicate of the old $3.
    let (plan, id) = save(&s, &entry("Widget price is back to $3", 0), Mode::Normal);
    assert!(id.is_some(), "{plan:?}");
    assert_eq!(target(&plan), Some(four.as_str()));
}

#[test]
fn value_awareness_off_takes_no_importance_exemption() {
    let s = store();
    let plan = plan_save(
        &s,
        &entry("Widget price is $5", 0),
        &vector(),
        THRESHOLD,
        Mode::Normal,
        false,
    )
    .unwrap();
    assert_eq!(
        plan,
        Plan::Save {
            link: None,
            valued: false
        }
    );
}

#[test]
fn a_chain_that_moved_into_another_project_is_not_joined_at_commit() {
    let dir = crate::test_support::temp_dir("mnemonic-plan-moved-");
    let path = dir.path().join("memory.db");
    let (one, two) = (Storage::open(&path).unwrap(), Storage::open(&path).unwrap());
    let five = saved(&one, "Widget price is $5", 30);
    // Beta plans against the unscoped $5 ...
    let mut beta = entry("Widget price is now $6", 0);
    set_project(&one, &mut beta, "beta-lab").unwrap();
    let plan = plan_save(&one, &beta, &vector(), THRESHOLD, Mode::Normal, true).unwrap();
    assert_eq!(target(&plan), Some(five.as_str()));
    // ... alpha's $7 takes that chain first ...
    let mut alpha = entry("Widget price is now $7", 10);
    set_project(&two, &mut alpha, "alpha-shop").unwrap();
    let (_, alpha_id) = save(&two, &alpha, Mode::Normal);
    // ... so beta's save lands without joining alpha's chain.
    let Plan::Save { link, .. } = &plan else {
        panic!()
    };
    let written = one
        .save_with_links(&beta, Some(&vector()), link.as_ref(), "test")
        .unwrap();
    assert_eq!(written, None);
    assert_eq!(head_of(&one, &five), alpha_id.unwrap());
}

#[test]
fn the_newest_head_decides_between_equal_old_values() {
    let s = store();
    saved(&s, "Widget price is $5", 30);
    // The same $5 again, as a correction: saved, a second head.
    let (_, restated) = save(&s, &entry("Widget price is $5", 20), Mode::LinkOnly);
    let six = saved(&s, "Widget price is now $6", 10);
    // Back to $5: the $6 is the newest word on the price, so this is a
    // revert, not a duplicate of either old $5.
    let (plan, back) = save(&s, &entry("Widget price is back to $5", 0), Mode::Normal);
    assert!(back.is_some(), "{plan:?}");
    assert_eq!(target(&plan), Some(six.as_str()));
    let _ = restated;
}

#[test]
fn a_history_splice_never_joins_another_project() {
    let s = store();
    let day = 60 * 24;
    let mut alpha = entry("Widget price is $5", 30 * day);
    set_project(&s, &mut alpha, "alpha-shop").unwrap();
    save(&s, &alpha, Mode::Normal);
    // An unscoped $6 later updates alpha's $5.
    let six = saved(&s, "Widget price is now $6", 10);
    // Beta's older $4 would land between them, next to alpha's memory.
    let mut beta = entry("Widget price is $4", 20 * day);
    set_project(&s, &mut beta, "beta-lab").unwrap();
    let plan = plan_save(&s, &beta, &vector(), THRESHOLD, Mode::Normal, true).unwrap();
    let Plan::Save { link, .. } = &plan else {
        panic!("{plan:?}")
    };
    let written = s
        .save_with_links(&beta, Some(&vector()), link.as_ref(), "test")
        .unwrap();
    assert_eq!(written, None);
    assert_eq!(head_of(&s, &alpha.id), six);
    let conn = s.conn.lock().unwrap();
    let touching_beta: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM memory_updates WHERE new_id = ?1 OR old_id = ?1",
            [&beta.id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(touching_beta, 0);
}

#[test]
fn a_nearer_stale_head_does_not_outvote_a_newer_one() {
    let s = store();
    // Unit vectors: `exact` is the query itself, `close` sits at 0.95.
    let exact = vector();
    let mut close = vec![0.0; exact.len()];
    close[0] = 0.95;
    close[1] = (1.0f32 - 0.95 * 0.95).sqrt();
    let put = |text: &str, minutes_ago: i64, emb: &Embedding, mode: Mode| {
        let e = entry(text, minutes_ago);
        let plan = plan_save(&s, &e, emb, THRESHOLD, mode, true).unwrap();
        let Plan::Save { link, .. } = &plan else {
            panic!("{text}: {plan:?}")
        };
        s.save_with_links(&e, Some(emb), link.as_ref(), "test")
            .unwrap();
        e.id
    };
    put("Widget price is $5", 40, &close, Mode::Normal);
    let six = put("Widget price is now $6", 30, &close, Mode::Normal);
    // A restated $5 that embeds exactly like the query, and heads nothing.
    put("Widget price is $5", 50, &exact, Mode::LinkOnly);
    // The nearest candidate is that stale $5; the newest head is $6.
    let back = entry("Widget price is $5", 0);
    let plan = plan_save(&s, &back, &exact, THRESHOLD, Mode::Normal, true).unwrap();
    assert_eq!(target(&plan), Some(six.as_str()), "{plan:?}");
}

#[test]
fn a_historical_return_to_an_old_value_is_a_change_not_a_replay() {
    let s = store();
    let five = saved(&s, "Widget price is $5", 40);
    let six = saved(&s, "Widget price is now $6", 30);
    let seven = saved(&s, "Widget price is now $7", 10);
    // Read late from a transcript: at minute 20 the price went back to $5.
    let (plan, back) = save(&s, &entry("Widget price is back to $5", 20), Mode::Normal);
    let back = back.unwrap_or_else(|| panic!("{plan:?}"));
    // It sits in time order: $5, $6, back to $5, $7.
    assert_eq!(head_of(&s, &five), seven);
    let conn = s.conn.lock().unwrap();
    let link = |new: &str, old: &str| -> bool {
        conn.query_row(
            "SELECT COUNT(*) FROM memory_updates WHERE new_id = ?1 AND old_id = ?2",
            [new, old],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
            == 1
    };
    assert!(link(&back, &six));
    assert!(link(&seven, &back));
    assert!(!link(&seven, &six));
}

#[test]
fn the_value_held_then_must_be_this_projects_too() {
    let s = store();
    let mut alpha = entry("Widget price is $5", 40);
    set_project(&s, &mut alpha, "alpha-shop").unwrap();
    save(&s, &alpha, Mode::Normal);
    // An unscoped $6 later updates alpha's $5.
    saved(&s, "Widget price is now $6", 10);
    // Beta's own $5 from minute 20: alpha's value then is not beta's.
    let mut beta = entry("Widget price is $5", 20);
    set_project(&s, &mut beta, "beta-lab").unwrap();
    let (plan, id) = save(&s, &beta, Mode::Normal);
    assert!(id.is_some(), "{plan:?}");
}

#[test]
fn a_replay_in_one_chain_does_not_outvote_a_change_in_another() {
    let s = store();
    saved(&s, "Widget price is $5", 60);
    saved(&s, "Widget price is now $6", 50);
    saved(&s, "Widget price is now $7", 20);
    // $7 read late at minute 30: equals its successor, saved on its own.
    let seven_early = saved(&s, "Widget price is $7", 30);
    // Then $6 at minute 25: after the early $7, so a revert from it, even
    // though the first chain held $6 back at minute 50.
    let (plan, id) = save(&s, &entry("Widget price is back to $6", 25), Mode::Normal);
    assert!(id.is_some(), "{plan:?}");
    assert_eq!(target(&plan), Some(seven_early.as_str()));
}

#[test]
fn a_value_said_again_later_is_not_overtaken_by_an_older_backfill() {
    let s = store();
    let five = saved(&s, "Widget price is $5", 30);
    // Said again at minute 10: a duplicate, but a later word on the price.
    let again = entry("Widget price is $5", 10);
    let plan = plan_settled(&s, &again, &vector(), THRESHOLD, Mode::Normal, true).unwrap();
    assert_eq!(duplicate_of(&plan), Some(five.as_str()));
    // $6 read late, dated minute 20: before the $5 was said again, so kept
    // but not made current.
    let (plan, six) = save(&s, &entry("Widget price is $6", 20), Mode::Normal);
    assert!(six.is_some(), "{plan:?}");
    assert_eq!(target(&plan), None);
    // Now the $5 is still the latest word: $6 said today changes it.
    let (plan, _) = save(&s, &entry("Widget price is $6", 0), Mode::Normal);
    assert_eq!(target(&plan), Some(five.as_str()));
}

#[test]
fn a_merged_project_still_meets_its_old_saves() {
    let s = store();
    {
        let conn = s.conn.lock().unwrap();
        for (id, name) in [("e1", "old-shop"), ("e2", "new-shop")] {
            conn.execute(
                "INSERT INTO entities (id, name, entity_type) VALUES (?1, ?2, 'project')",
                [id, name],
            )
            .unwrap();
        }
    }
    let mut old = entry("Widget price is $5", 20);
    set_project(&s, &mut old, "old-shop").unwrap();
    save(&s, &old, Mode::Normal);
    s.merge_entities("new-shop", "old-shop").unwrap();
    let mut new = entry("Widget price is now $6", 0);
    set_project(&s, &mut new, "old-shop").unwrap();
    assert_eq!(new.metadata["project_key"], "new-shop");
    let (plan, _) = save(&s, &new, Mode::Normal);
    assert_eq!(target(&plan), Some(old.id.as_str()));
}

#[test]
fn a_change_between_a_value_and_its_restatement_is_kept_unlinked() {
    let s = store();
    let five = saved(&s, "Widget price is $5", 50);
    let six = saved(&s, "Widget price is now $6", 40);
    // $6 said again at minute 20: recorded on the $6 memory.
    let again = entry("Widget price is now $6", 20);
    let plan = plan_settled(&s, &again, &vector(), THRESHOLD, Mode::Normal, true).unwrap();
    assert_eq!(duplicate_of(&plan), Some(six.as_str()));
    // $5 read late, dated minute 30: a real change, between $6 and its
    // restatement. Not a duplicate of the first $5, and $6 stays current.
    let (plan, back) = save(&s, &entry("Widget price is back to $5", 30), Mode::Normal);
    assert!(back.is_some(), "{plan:?}");
    assert_eq!(target(&plan), None);
    assert_eq!(head_of(&s, &five), six);
}

#[test]
fn a_duplicate_is_settled_against_the_chain_as_it_is_now() {
    let dir = crate::test_support::temp_dir("mnemonic-plan-settle-");
    let path = dir.path().join("memory.db");
    let (one, two) = (Storage::open(&path).unwrap(), Storage::open(&path).unwrap());
    let five = saved(&one, "Widget price is $5", 50);
    // One plans $5 at minute 5 as a duplicate of the $5 ...
    let again = entry("Widget price is $5", 5);
    let plan = plan_save(&one, &again, &vector(), THRESHOLD, Mode::Normal, true).unwrap();
    assert_eq!(duplicate_of(&plan), Some(five.as_str()));
    // ... while two commits $6 at minute 10.
    let six = saved(&two, "Widget price is now $6", 10);
    // The duplicate no longer holds: $5 after $6 is a revert.
    assert!(!one.confirm_duplicate(&five, again.timestamp).unwrap());
    let plan = plan_settled(&one, &again, &vector(), THRESHOLD, Mode::Normal, true).unwrap();
    assert_eq!(target(&plan), Some(six.as_str()));
}

#[test]
fn a_restatement_committed_first_keeps_a_planned_change_off_the_chain() {
    let s = store();
    let five = saved(&s, "Widget price is $5", 30);
    // $6 from minute 20 is planned as an update of $5 ...
    let six = entry("Widget price is $6", 20);
    let plan = plan_save(&s, &six, &vector(), THRESHOLD, Mode::Normal, true).unwrap();
    assert_eq!(target(&plan), Some(five.as_str()));
    // ... but $5 is said again at minute 10 before it commits.
    assert!(s.confirm_duplicate(&five, entry("", 10).timestamp).unwrap());
    let Plan::Save { link, .. } = &plan else {
        panic!()
    };
    let written = s
        .save_with_links(&six, Some(&vector()), link.as_ref(), "test")
        .unwrap();
    assert_eq!(written, None);
    assert_eq!(head_of(&s, &five), five);
}

#[test]
fn a_backfill_before_a_restated_predecessor_stays_off_the_chain() {
    let s = store();
    let five = saved(&s, "Widget price is $5", 60);
    // $5 said again at minute 40, then $7 at minute 30.
    let again = entry("Widget price is $5", 40);
    plan_settled(&s, &again, &vector(), THRESHOLD, Mode::Normal, true).unwrap();
    let seven = saved(&s, "Widget price is now $7", 30);
    // $6 read late from minute 50: between $5 and its restatement.
    let (plan, six) = save(&s, &entry("Widget price is $6", 50), Mode::Normal);
    assert!(six.is_some(), "{plan:?}");
    let conn = s.conn.lock().unwrap();
    let touching: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM memory_updates WHERE new_id = ?1 OR old_id = ?1",
            [six.as_ref().unwrap()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(touching, 0);
    drop(conn);
    assert_eq!(head_of(&s, &five), seven);
}

#[test]
fn the_kill_switch_keeps_the_plain_duplicate() {
    let s = store();
    let five = saved(&s, "Widget price is $5", 30);
    saved(&s, "Widget price is now $6", 20);
    // With value awareness off, the nearest memory decides, as it used to,
    // whatever chain it sits in.
    let plan = plan_settled(
        &s,
        &entry("Widget price is $5", 0),
        &vector(),
        THRESHOLD,
        Mode::Normal,
        false,
    )
    .unwrap();
    assert!(matches!(plan, Plan::Duplicate { .. }), "{plan:?}");
    let _ = five;
}

#[test]
fn a_splice_rechecks_the_predecessor_restated_since_the_plan() {
    let s = store();
    let five = saved(&s, "Widget price is $5", 60);
    // $6 from minute 50 is planned while $5 is the whole chain ...
    let six = entry("Widget price is $6", 50);
    let plan = plan_save(&s, &six, &vector(), THRESHOLD, Mode::Normal, true).unwrap();
    assert_eq!(target(&plan), Some(five.as_str()));
    // ... then $5 is said again at minute 40 and $7 arrives at minute 30.
    assert!(s.confirm_duplicate(&five, entry("", 40).timestamp).unwrap());
    let seven = saved(&s, "Widget price is now $7", 30);
    // Committed now, $6 would sit before $5's restatement: kept off.
    let Plan::Save { link, .. } = &plan else {
        panic!()
    };
    let written = s
        .save_with_links(&six, Some(&vector()), link.as_ref(), "test")
        .unwrap();
    assert_eq!(written, None);
    assert_eq!(head_of(&s, &five), seven);
}

#[test]
fn the_kill_switch_keeps_the_nearest_duplicate_even_behind_a_chain() {
    let s = store();
    let exact = vector();
    let mut close = vec![0.0; exact.len()];
    close[0] = 0.95;
    close[1] = (1.0f32 - 0.95 * 0.95).sqrt();
    let put = |text: &str, minutes_ago: i64, emb: &Embedding| {
        let e = entry(text, minutes_ago);
        let plan = plan_save(&s, &e, emb, THRESHOLD, Mode::LinkOnly, true).unwrap();
        let Plan::Save { link, .. } = &plan else {
            panic!("{plan:?}")
        };
        s.save_with_links(&e, Some(emb), link.as_ref(), "test")
            .unwrap();
        e.id
    };
    // $5 embeds exactly like the query; $6 updates it and embeds apart.
    let five = put("Widget price is $5", 30, &exact);
    put("Widget price is now $6", 20, &close);
    let plan = plan_settled(
        &s,
        &entry("Widget price is $5", 0),
        &exact,
        THRESHOLD,
        Mode::Normal,
        false,
    )
    .unwrap();
    assert_eq!(duplicate_of(&plan), Some(five.as_str()));
}

#[test]
fn a_historical_restatement_is_recorded_on_the_value_it_repeats() {
    let s = store();
    let five = saved(&s, "Widget price is $5", 60);
    let seven = saved(&s, "Widget price is now $7", 20);
    // $5 read late from minute 30: the $5 held then, said again.
    let again = entry("Widget price is $5", 30);
    let plan = plan_settled(&s, &again, &vector(), THRESHOLD, Mode::Normal, true).unwrap();
    assert_eq!(duplicate_of(&plan), Some(five.as_str()));
    // $6 from minute 40, before that restatement: kept off the chain.
    let (plan, six) = save(&s, &entry("Widget price is $6", 40), Mode::Normal);
    assert!(six.is_some(), "{plan:?}");
    assert_eq!(target(&plan), None);
    assert_eq!(head_of(&s, &five), seven);
}

#[test]
fn a_duplicate_of_a_memory_forgotten_since_the_plan_is_saved() {
    let s = store();
    let five = saved(&s, "Widget price is $5", 30);
    let again = entry("Widget price is $5", 0);
    let plan = plan_save(&s, &again, &vector(), THRESHOLD, Mode::Normal, true).unwrap();
    assert_eq!(duplicate_of(&plan), Some(five.as_str()));
    assert!(s.forget_by_id(&five).unwrap());
    assert!(!s.confirm_duplicate(&five, again.timestamp).unwrap());
    let plan = plan_settled(&s, &again, &vector(), THRESHOLD, Mode::Normal, true).unwrap();
    assert!(matches!(plan, Plan::Save { .. }), "{plan:?}");
}
