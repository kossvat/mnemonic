use super::*;
use crate::embedding::Embedder;
use crate::event::{EventSource, MemoryEntry, MemoryType};
use crate::facts::rule::Trust;
use crate::facts::store::FactWrite;
use crate::test_support::{ConstEmbedder, InTempDir};

fn store() -> InTempDir<Storage> {
    InTempDir::new("mnemonic-recall-", |dir| {
        Storage::open(&dir.join("memory.db")).unwrap()
    })
}

/// Save a plain memory the way memory_save does, linked by the gate.
fn save(s: &Storage, content: &str, minutes_ago: i64) -> String {
    let mut entry = MemoryEntry::new(
        "Widget price",
        content,
        MemoryType::Note,
        EventSource::Manual,
    );
    entry.timestamp = chrono::Utc::now() - chrono::Duration::minutes(minutes_ago);
    let vector = ConstEmbedder.embed("").unwrap();
    let link = match crate::updates::plan::plan_save(
        s,
        &entry,
        &vector,
        0.92,
        crate::updates::plan::Mode::LinkOnly,
        true,
    )
    .unwrap()
    {
        crate::updates::plan::Plan::Save { link, .. } => link,
        crate::updates::plan::Plan::Duplicate { .. } => None,
    };
    s.save_with_links(&entry, Some(&vector), link.as_ref(), "test")
        .unwrap();
    entry.id
}

fn hit(s: &Storage, id: &str) -> Value {
    let mut hits = vec![json!({"id": id})];
    annotate(s, &mut hits).unwrap();
    hits.pop().unwrap()
}

#[test]
fn the_replaced_memory_names_the_one_that_holds_now() {
    let s = store();
    let five = save(&s, "Widget price is $5", 30);
    let six = save(&s, "Widget price is now $6", 20);
    let seven = save(&s, "Widget price is now $7", 10);
    assert_eq!(hit(&s, &five)["replaced_by"]["id"], seven.as_str());
    assert_eq!(hit(&s, &six)["replaced_by"]["id"], seven.as_str());
    assert!(hit(&s, &seven).get("replaced_by").is_none());
    // Forgetting the newest makes the one before it current again.
    assert!(s.forget_by_id(&seven).unwrap());
    assert!(hit(&s, &six).get("replaced_by").is_none());
    assert_eq!(hit(&s, &five)["replaced_by"]["id"], six.as_str());
}

#[test]
fn a_memory_that_stated_a_fact_shows_what_it_holds_now() {
    let s = store();
    let write = |value| FactWrite {
        project: Some("alpha-shop"),
        subject: "Gadget",
        predicate: "price",
        value,
        actor: "test",
        ..Default::default()
    };
    let declare = |value| {
        crate::facts::declare::declare(&s, &ConstEmbedder, 0.92, &write(value), None).unwrap()
    };
    let first = declare(Some("$7")).memory.unwrap().id;
    let facts = &hit(&s, &first)["facts"];
    assert_eq!(facts[0]["this"], "$7");
    assert_eq!(facts[0]["is_current"], true);

    declare(Some("$8"));
    let facts = &hit(&s, &first)["facts"];
    assert_eq!(facts[0]["this"], "$7");
    assert_eq!(facts[0]["current"], "$8");
    assert_eq!(facts[0]["is_current"], false);

    let gone = declare(None).memory.unwrap().id;
    let facts = &hit(&s, &gone)["facts"];
    assert!(facts[0]["this"].is_null());
    assert!(facts[0]["current"].is_null());
    assert!(hit(&s, &first)["facts"][0]["current"].is_null());
}

#[test]
fn a_noted_reconfirmation_shows_what_holds_now() {
    let s = store();
    let owner = |value, note| {
        crate::facts::declare::declare(
            &s,
            &ConstEmbedder,
            0.92,
            &FactWrite {
                subject: "alpha-shop",
                predicate: "owner",
                value: Some(value),
                actor: "test",
                ..Default::default()
            },
            note,
        )
        .unwrap()
    };
    owner("Alice", None);
    let noted = owner("Alice", Some("confirmed on the call"));
    assert_eq!(noted.outcome.outcome, "reconfirm");
    let noted = noted.memory.unwrap().id;
    owner("Bob", None);
    let facts = &hit(&s, &noted)["facts"];
    assert_eq!(facts[0]["this"], "Alice");
    assert_eq!(facts[0]["current"], "Bob");
}

#[test]
fn a_plain_value_replaced_by_a_retracted_fact_says_so() {
    let s = store();
    let five = save(&s, "Widget price is $5", 30);
    let widget = |value| {
        crate::facts::declare::declare(
            &s,
            &ConstEmbedder,
            0.92,
            &FactWrite {
                subject: "Widget",
                predicate: "price",
                value,
                actor: "test",
                ..Default::default()
            },
            None,
        )
        .unwrap()
    };
    let six = widget(Some("$6")).memory.unwrap().id;
    let annotated = hit(&s, &five);
    assert_eq!(annotated["replaced_by"]["id"], six.as_str());
    assert!(!note(&annotated).contains("is now"), "{}", note(&annotated));
    widget(None);
    let annotated = hit(&s, &five);
    assert!(
        note(&annotated).ends_with("[Widget price is now retracted]"),
        "{}",
        note(&annotated)
    );
}

#[test]
fn a_memory_with_nothing_to_say_is_left_as_it_is() {
    let s = store();
    let id = save(&s, "Deploy notes for the widget page", 5);
    assert_eq!(hit(&s, &id), json!({"id": id}));
}

#[test]
fn a_proposal_waiting_for_review_is_not_shown() {
    let s = store();
    let write = |value, trust, source| FactWrite {
        subject: "Gadget",
        predicate: "price",
        value: Some(value),
        trust: Some(trust),
        source_memory_id: source,
        actor: "test",
        ..Default::default()
    };
    crate::facts::store::apply(&s, &write("$7", Trust::Declared, None)).unwrap();
    let id = save(&s, "Gadget might cost $9 soon", 5);
    let proposal = crate::facts::store::apply(&s, &write("$9", Trust::Provisional, Some(&id)));
    assert_eq!(proposal.unwrap().outcome, "pending_review");
    assert!(hit(&s, &id).get("facts").is_none());
}

#[test]
fn the_note_says_what_replaced_a_hit_and_what_a_fact_holds() {
    let hit = json!({
        "id": "a",
        "replaced_by": {"id": "bbbb2222cccc", "title": "t", "timestamp": "2026-09-10T08:00:00+00:00"},
        "facts": [
            {"subject": "Gadget", "predicate": "price", "current": "$8", "is_current": false},
            {"subject": "Gadget", "predicate": "owner", "current": null, "is_current": false},
            {"subject": "Gadget", "predicate": "size", "current": "L", "is_current": true},
        ],
    });
    assert_eq!(
        note(&hit),
        " [replaced by `bbbb2222` of 2026-09-10] [Gadget price is now $8] [Gadget owner is now retracted]"
    );
    assert_eq!(note(&json!({"id": "a"})), "");
}
