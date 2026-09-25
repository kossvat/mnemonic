use super::*;
use crate::embedding::Embedder;
use crate::test_support::{ConstEmbedder, InTempDir};

fn store() -> InTempDir<Storage> {
    InTempDir::new("mnemonic-fact-declare-", |dir| {
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

#[test]
fn a_statement_is_recorded_as_a_memory_of_the_fact() {
    let s = store();
    let declared = declare(&s, &ConstEmbedder, 0.92, &write("$5"), None).unwrap();
    assert_eq!(declared.outcome.outcome, "create");
    let memory = declared.memory.unwrap();
    assert_eq!(memory.metadata["fact"]["value"], "$5");
    assert_eq!(memory.metadata["project_key"], "alpha-shop");
    assert_eq!(
        declared
            .outcome
            .fact
            .current
            .unwrap()
            .source_memory_id
            .as_deref(),
        Some(memory.id.as_str())
    );
}

#[test]
fn saying_the_current_value_again_writes_no_memory_unless_noted() {
    let s = store();
    declare(&s, &ConstEmbedder, 0.92, &write("$5"), None).unwrap();
    let again = declare(&s, &ConstEmbedder, 0.92, &write("$5.00"), None).unwrap();
    assert_eq!(again.outcome.outcome, "reconfirm");
    assert!(again.memory.is_none());
    let noted = declare(
        &s,
        &ConstEmbedder,
        0.92,
        &write("$5"),
        Some("confirmed on the call"),
    )
    .unwrap();
    assert_eq!(noted.outcome.outcome, "reconfirm");
    assert!(noted.memory.is_some());
}

#[test]
fn mixed_plain_then_declared_links_plain_memory() {
    let s = store();
    let mut plain = crate::event::MemoryEntry::new(
        "Widget price",
        "Widget price is $5",
        crate::event::MemoryType::Note,
        crate::event::EventSource::Manual,
    );
    plain.timestamp = Utc::now() - chrono::Duration::minutes(10);
    crate::updates::plan::set_project(&s, &mut plain, "alpha-shop").unwrap();
    let vector = ConstEmbedder.embed("").unwrap();
    s.save_with_links(&plain, Some(&vector), None, "test")
        .unwrap();
    let declared = declare(&s, &ConstEmbedder, 0.92, &write("$6"), None).unwrap();
    let link = declared.link.expect("the plain $5 is marked replaced");
    assert_eq!(link.old_id, plain.id);
    assert_eq!(link.new_id, declared.memory.unwrap().id);
}

#[test]
fn a_retraction_is_recorded_too() {
    let s = store();
    declare(&s, &ConstEmbedder, 0.92, &write("$5"), None).unwrap();
    let gone = declare(
        &s,
        &ConstEmbedder,
        0.92,
        &FactWrite {
            value: None,
            ..write("")
        },
        None,
    )
    .unwrap();
    assert_eq!(gone.outcome.outcome, "retract");
    assert!(gone.memory.unwrap().title.ends_with("retracted"));
}

/// A plain memory saved the way memory_save saves it, linked by the gate.
fn plain(s: &Storage, content: &str) -> String {
    plain_at(s, content, None)
}

fn plain_at(s: &Storage, content: &str, at: Option<&str>) -> String {
    let mut entry = crate::event::MemoryEntry::new(
        "Widget price",
        content,
        crate::event::MemoryType::Note,
        crate::event::EventSource::Manual,
    );
    if let Some(at) = at {
        entry.timestamp = chrono::DateTime::parse_from_rfc3339(at)
            .unwrap()
            .with_timezone(&Utc);
    }
    plan::set_project(s, &mut entry, "alpha-shop").unwrap();
    let vector = ConstEmbedder.embed("").unwrap();
    let link = match plan::plan_save(s, &entry, &vector, 0.92, Mode::LinkOnly, true).unwrap() {
        plan::Plan::Save { link, .. } => link,
        plan::Plan::Duplicate { .. } => None,
    };
    s.save_with_links(&entry, Some(&vector), link.as_ref(), "test")
        .unwrap();
    entry.id
}

fn head(s: &Storage, id: &str) -> Option<String> {
    let conn = s.conn.lock().unwrap();
    crate::updates::store::head(&conn, id).unwrap()
}

#[test]
fn forgetting_a_declared_value_erases_the_memories_that_state_it() {
    let s = store();
    let five = declare(&s, &ConstEmbedder, 0.92, &write("$5"), None).unwrap();
    let noted = declare(&s, &ConstEmbedder, 0.92, &write("$5"), Some("per the call")).unwrap();
    let six = declare(&s, &ConstEmbedder, 0.92, &write("$6"), None).unwrap();
    let five_id = five.outcome.value_id.clone().unwrap();
    store::forget_value(&s, &five_id).unwrap();
    for gone in [five.memory.unwrap().id, noted.memory.unwrap().id] {
        assert!(s.get_by_id(&gone).unwrap().is_none(), "{gone}");
    }
    assert!(s.search("$5", 10).unwrap().is_empty());
    assert!(s.get_by_id(&six.memory.unwrap().id).unwrap().is_some());
}

#[test]
fn a_backdated_statement_is_filed_before_the_newer_one() {
    let s = store();
    let at = |value, as_of| FactWrite {
        as_of: Some(as_of),
        ..write(value)
    };
    let six = declare(&s, &ConstEmbedder, 0.92, &at("$6", "2026-03-01"), None).unwrap();
    let five = declare(&s, &ConstEmbedder, 0.92, &at("$5", "2026-01-01"), None).unwrap();
    assert_eq!(five.outcome.outcome, "history");
    let five = five.memory.unwrap();
    assert_eq!(five.timestamp.to_rfc3339(), "2026-01-01T00:00:00+00:00");
    let six = six.memory.unwrap().id;
    assert_eq!(head(&s, &five.id).as_deref(), Some(six.as_str()));
    assert_eq!(head(&s, &six).as_deref(), Some(six.as_str()));
}

#[test]
fn saying_the_value_again_after_a_plain_change_updates_that_memory() {
    let s = store();
    let first = declare(&s, &ConstEmbedder, 0.92, &write("$5"), None).unwrap();
    let first = first.memory.unwrap().id;
    let six = plain(&s, "Widget price is now $6");
    assert_eq!(head(&s, &first).as_deref(), Some(six.as_str()));
    let again = declare(&s, &ConstEmbedder, 0.92, &write("$5"), None).unwrap();
    assert_eq!(again.outcome.outcome, "reconfirm");
    let again = again.memory.expect("written to update the plain $6").id;
    assert_eq!(head(&s, &six).as_deref(), Some(again.as_str()));
}

#[test]
fn a_quiet_reconfirmation_keeps_when_it_was_said() {
    let s = store();
    let first = declare(&s, &ConstEmbedder, 0.92, &write("$5"), None).unwrap();
    let first = first.memory.unwrap().id;
    assert!(
        declare(&s, &ConstEmbedder, 0.92, &write("$5"), None)
            .unwrap()
            .memory
            .is_none()
    );
    let said: i64 = s
        .conn
        .lock()
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM memory_reaffirmed WHERE memory_id = ?1",
            [&first],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(said, 1);
}

struct Mismatched;

impl Embedder for Mismatched {
    fn embed(&self, _text: &str) -> anyhow::Result<crate::embedding::Embedding> {
        Err(
            crate::embedding::daemon_client::EmbedClientError::Mismatch {
                got_dim: 3,
                expected_dim: 4,
            }
            .into(),
        )
    }

    fn model_id(&self) -> &'static str {
        "mismatched"
    }
}

#[test]
fn a_hard_embedding_failure_fails_the_statement() {
    let s = store();
    assert!(declare(&s, &Mismatched, 0.92, &write("$5"), None).is_err());
    assert!(
        store::views(&s, Some("alpha-shop"), "Widget")
            .unwrap()
            .is_empty()
    );
}

fn at<'a>(value: &'a str, as_of: &'a str) -> FactWrite<'a> {
    FactWrite {
        as_of: Some(as_of),
        ..write(value)
    }
}

#[test]
fn an_implicit_now_after_a_value_dated_ahead_is_its_update() {
    let s = store();
    let ahead = (Utc::now() + chrono::Duration::minutes(2)).to_rfc3339();
    let five = declare(&s, &ConstEmbedder, 0.92, &at("$5", &ahead), None).unwrap();
    let six = declare(&s, &ConstEmbedder, 0.92, &write("$6"), None).unwrap();
    assert_eq!(six.outcome.outcome, "update");
    let (five, six) = (five.memory.unwrap(), six.memory.unwrap());
    assert!(six.timestamp > five.timestamp);
    assert_eq!(head(&s, &five.id).as_deref(), Some(six.id.as_str()));
    assert_eq!(head(&s, &six.id).as_deref(), Some(six.id.as_str()));
}

#[test]
fn a_noted_reconfirmation_keeps_the_value_ahead_of_a_backfill() {
    let s = store();
    let first = declare(&s, &ConstEmbedder, 0.92, &at("$5", "2026-01-01"), None).unwrap();
    let first = first.memory.unwrap().id;
    let noted = declare(
        &s,
        &ConstEmbedder,
        0.92,
        &at("$5", "2026-03-01"),
        Some("confirmed on the call"),
    )
    .unwrap();
    assert_eq!(noted.outcome.outcome, "reconfirm");
    let said: String = s
        .conn
        .lock()
        .unwrap()
        .query_row(
            "SELECT at FROM memory_reaffirmed WHERE memory_id = ?1",
            [&first],
            |r| r.get(0),
        )
        .unwrap();
    assert!(said.starts_with("2026-03-01"), "{said}");
}

#[test]
fn a_backdated_reconfirmation_reaffirms_the_value_it_matched() {
    let s = store();
    let five = declare(&s, &ConstEmbedder, 0.92, &at("$5", "2026-01-01"), None).unwrap();
    let five = five.memory.unwrap().id;
    declare(&s, &ConstEmbedder, 0.92, &at("$6", "2026-03-01"), None).unwrap();
    let again = declare(&s, &ConstEmbedder, 0.92, &at("$5", "2026-02-01"), None).unwrap();
    assert_eq!(again.outcome.outcome, "reconfirm");
    let said: Option<String> = s
        .conn
        .lock()
        .unwrap()
        .query_row(
            "SELECT at FROM memory_reaffirmed WHERE memory_id = ?1",
            [&five],
            |r| r.get(0),
        )
        .ok();
    assert!(said.is_some_and(|at| at.starts_with("2026-02-01")));
    // A plain $7 from before that reaffirmation is not placed after it.
    let seven = plain_at(&s, "Widget price is $7", Some("2026-01-15T00:00:00+00:00"));
    assert_ne!(head(&s, &five).as_deref(), Some(seven.as_str()));
}

#[test]
fn a_plain_change_that_lands_after_planning_is_updated_anyway() {
    let s = store();
    let first = declare(&s, &ConstEmbedder, 0.92, &write("$5"), None).unwrap();
    let first = first.memory.unwrap().id;
    let prepared = prepare(&s, &ConstEmbedder, 0.92, &write("$5"), None).unwrap();
    assert!(prepared.planned.is_none(), "nothing to update yet");
    let six = plain(&s, "Widget price is now $6");
    assert_eq!(head(&s, &first).as_deref(), Some(six.as_str()));
    let again = commit(&s, prepared, 0.92, &write("$5"), None).unwrap();
    assert_eq!(again.outcome.outcome, "reconfirm");
    let again = again.memory.expect("written to update the plain $6").id;
    assert_eq!(head(&s, &six).as_deref(), Some(again.as_str()));
}

struct Short;

impl Embedder for Short {
    fn embed(&self, _text: &str) -> anyhow::Result<crate::embedding::Embedding> {
        Ok(vec![1.0, 0.0, 0.0])
    }

    fn model_id(&self) -> &'static str {
        "short"
    }
}

#[test]
fn a_retraction_with_a_vector_of_another_size_is_refused() {
    let s = store();
    declare(&s, &ConstEmbedder, 0.92, &write("$5"), None).unwrap();
    let retract = FactWrite {
        value: None,
        ..write("")
    };
    assert!(declare(&s, &Short, 0.92, &retract, None).is_err());
    let view = &store::views(&s, Some("alpha-shop"), "Widget").unwrap()[0];
    assert_eq!(view.current.as_ref().unwrap().value.as_deref(), Some("$5"));
}

/// How a plain save of `content` at `at` is planned (memory_save's mode).
fn planned(s: &Storage, content: &str, at: &str) -> plan::Plan {
    let mut entry = crate::event::MemoryEntry::new(
        "Widget price",
        content,
        crate::event::MemoryType::Note,
        crate::event::EventSource::Manual,
    );
    entry.timestamp = chrono::DateTime::parse_from_rfc3339(at)
        .unwrap()
        .with_timezone(&Utc);
    plan::set_project(s, &mut entry, "alpha-shop").unwrap();
    let vector = ConstEmbedder.embed("").unwrap();
    plan::plan_save(s, &entry, &vector, 0.92, Mode::Normal, true).unwrap()
}

#[test]
fn a_value_said_again_after_its_fact_was_retracted_is_kept() {
    let s = store();
    let five = declare(&s, &ConstEmbedder, 0.92, &at("$5", "2026-01-01"), None).unwrap();
    let five = five.memory.unwrap().id;
    let repeat = || planned(&s, "Widget price is $5", "2026-04-01T00:00:00+00:00");
    assert!(matches!(repeat(), plan::Plan::Duplicate { of, .. } if of == five));
    let retract = FactWrite {
        value: None,
        ..at("", "2026-03-01")
    };
    declare(&s, &ConstEmbedder, 0.92, &retract, None).unwrap();
    assert!(
        matches!(repeat(), plan::Plan::Save { .. }),
        "{:?}",
        repeat()
    );
    // Said for a time the value still held: a repeat as before.
    let backfill = planned(&s, "Widget price is $5", "2026-02-01T00:00:00+00:00");
    assert!(matches!(backfill, plan::Plan::Duplicate { of, .. } if of == five));
    // A duplicate planned before the retraction does not settle after it.
    let conn = s.conn.lock().unwrap();
    let april = chrono::DateTime::parse_from_rfc3339("2026-04-01T00:00:00+00:00")
        .unwrap()
        .with_timezone(&Utc);
    assert!(!plan::settle_duplicate(&conn, &five, april).unwrap());
    let february = chrono::DateTime::parse_from_rfc3339("2026-02-01T00:00:00+00:00")
        .unwrap()
        .with_timezone(&Utc);
    assert!(plan::settle_duplicate(&conn, &five, february).unwrap());
}

#[test]
fn a_value_that_came_back_still_counts_as_held() {
    let s = store();
    let five = declare(&s, &ConstEmbedder, 0.92, &at("$5", "2026-01-01"), None).unwrap();
    let five = five.memory.unwrap().id;
    declare(&s, &ConstEmbedder, 0.92, &at("$5", "2026-03-01"), None).unwrap();
    // Dated inside the reconfirmed span: $5 comes back in March, from the
    // same memory.
    let six = declare(&s, &ConstEmbedder, 0.92, &at("$6", "2026-02-01"), None).unwrap();
    assert_eq!(six.outcome.outcome, "history");
    let repeat = planned(&s, "Widget price is $5", "2026-04-01T00:00:00+00:00");
    assert!(
        matches!(&repeat, plan::Plan::Duplicate { of, .. } if *of == five),
        "{repeat:?}"
    );
}

#[test]
fn variants_of_a_fact_are_not_each_others_updates() {
    let s = store();
    let phase = |qualifier, value| FactWrite {
        qualifier: Some(qualifier),
        ..write(value)
    };
    let one = declare(&s, &ConstEmbedder, 0.92, &phase("phase 1", "$5"), None).unwrap();
    let two = declare(&s, &ConstEmbedder, 0.92, &phase("phase 2", "$6"), None).unwrap();
    assert!(two.link.is_none());
    let one = one.memory.unwrap().id;
    assert_eq!(head(&s, &one).as_deref(), Some(one.as_str()));
    // The same variant again is its update.
    let again = declare(&s, &ConstEmbedder, 0.92, &phase("phase 1", "$7"), None).unwrap();
    assert!(again.link.is_some());
}

#[test]
fn forgetting_a_value_keeps_a_memory_another_value_cites_as_evidence() {
    let s = store();
    let five = declare(&s, &ConstEmbedder, 0.92, &write("$5"), None).unwrap();
    let memory = five.memory.unwrap().id;
    let other = |source| FactWrite {
        subject: "Gadget",
        source_memory_id: source,
        ..write("$9")
    };
    store::apply(&s, &other(None)).unwrap();
    let cited = store::apply(&s, &other(Some(&memory))).unwrap();
    assert_eq!(cited.outcome, "reconfirm");
    store::forget_value(&s, &five.outcome.value_id.unwrap()).unwrap();
    assert!(s.get_by_id(&memory).unwrap().is_some());
}

#[test]
fn a_statement_is_not_queued_for_extraction() {
    let s = store();
    let five = declare(&s, &ConstEmbedder, 0.92, &write("$5"), None).unwrap();
    let queued: i64 = s
        .conn
        .lock()
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM extraction_queue WHERE memory_id = ?1",
            [&five.memory.unwrap().id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(queued, 0);
}

#[test]
fn a_project_merged_while_planning_names_the_memory_too() {
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
    let old_shop = FactWrite {
        project: Some("old-shop"),
        ..write("$5")
    };
    let prepared = prepare(&s, &ConstEmbedder, 0.92, &old_shop, None).unwrap();
    assert_eq!(prepared.entry.metadata["project_key"], "old-shop");
    s.merge_entities("new-shop", "old-shop").unwrap();
    let declared = commit(&s, prepared, 0.92, &old_shop, None).unwrap();
    assert_eq!(declared.outcome.fact.project, "new-shop");
    assert_eq!(declared.memory.unwrap().metadata["project_key"], "new-shop");
}
