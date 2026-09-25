use super::*;
use crate::storage::Storage;
use crate::test_support::InTempDir;

fn store() -> InTempDir<Storage> {
    InTempDir::new("mnemonic-fact-keys-", |dir| {
        Storage::open(&dir.join("memory.db")).unwrap()
    })
}

#[test]
fn predicates_fold_to_one_spelling_and_keep_free_keys() {
    for (raw, key) in [
        ("price", "price"),
        ("Has Price", "price"),
        ("has_price", "price"),
        ("Цена", "price"),
        ("стоимость", "price"),
        ("Дедлайн", "deadline"),
        ("срок", "deadline"),
        ("комиссия", "commission"),
        ("скидка", "discount"),
        ("Payment Terms", "payment-terms"),
        ("условия оплаты", "payment-terms"),
        // Free keys pass unchanged, as agents already write them.
        ("checkpoint", "checkpoint"),
        ("setup-checkpoint", "setup-checkpoint"),
        ("next-step", "next-step"),
        ("current_status", "current-status"),
        ("owner  contact", "owner-contact"),
    ] {
        assert_eq!(predicate_key(raw).unwrap(), key, "{raw}");
    }
    assert!(predicate_key("  ").is_err());
    assert!(predicate_key(&"x".repeat(49)).is_err());
}

#[test]
fn predicate_classes() {
    assert_eq!(predicate_class("price"), PredicateClass::Money);
    assert_eq!(predicate_class("setup-fee"), PredicateClass::Money);
    assert_eq!(predicate_class("deadline"), PredicateClass::Date);
    assert_eq!(predicate_class("launch-date"), PredicateClass::Date);
    assert_eq!(predicate_class("payment-terms"), PredicateClass::Terms);
    assert_eq!(predicate_class("checkpoint"), PredicateClass::Other);
}

#[test]
fn subjects_resolve_aliases_and_spelling() {
    let s = store();
    let conn = s.conn.lock().unwrap();
    assert_eq!(
        subject_key(&conn, "Widget App").unwrap(),
        subject_key(&conn, "widget-app").unwrap()
    );
    assert!(subject_key(&conn, "   ").is_err());
    conn.execute(
        "INSERT INTO entities (id, name, entity_type, first_seen, last_seen, mention_count)
         VALUES ('e1', 'widget-app', 'project', datetime('now'), datetime('now'), 1)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO entity_aliases (alias, canonical) VALUES ('wdgt', 'widget-app')",
        [],
    )
    .unwrap();
    assert_eq!(
        subject_key(&conn, "WDGT").unwrap(),
        subject_key(&conn, "Widget App").unwrap()
    );
}

#[test]
fn values_compare_by_what_they_say() {
    assert_eq!(value_norm("$6"), value_norm("$6.00"));
    assert_eq!(value_norm("$6"), value_norm("6 USD"));
    assert_ne!(value_norm("$6"), value_norm("$60"));
    assert_eq!(value_norm("15%"), value_norm("15 %"));
    assert_eq!(value_norm("Net 30"), value_norm("net 30"));
    assert_eq!(value_norm("On  Hold "), "on hold");
    // A value is normalized only when the amount is all of it.
    assert_ne!(
        value_norm("$6 per pallet"),
        value_norm("$6 per square foot")
    );
    assert_eq!(value_norm("$6/month"), value_norm("$6 per month"));
    assert_ne!(value_norm("-$5"), value_norm("$5"));
    assert_ne!(value_norm("$6 + 10%"), value_norm("$6 + 20%"));
    assert_ne!(value_norm("<$5"), value_norm("$5"));
    assert_ne!(value_norm("$5+"), value_norm("$5"));
    assert_eq!(value_norm("($5)"), value_norm("$5"));
    assert_ne!(value_norm("$0.005"), value_norm("$5"));
    assert_ne!(value_norm("$0.00001"), value_norm("$0.00002"));
}

#[test]
fn times_parse_to_epoch_ms() {
    let rfc = parse_time_ms("2026-09-23T10:00:00Z").unwrap();
    assert_eq!(parse_time_ms("2026-09-23 10:00:00").unwrap(), rfc);
    assert_eq!(parse_time_ms("2026-09-23T13:00:00+03:00").unwrap(), rfc);
    assert_eq!(parse_time_ms("2026-09-23").unwrap(), rfc - 10 * 3600 * 1000);
    assert!(parse_time_ms("next tuesday").is_err());
    assert_eq!(parse_time_ms(&format_ms(rfc)).unwrap(), rfc);
}

#[test]
fn long_subjects_that_differ_after_the_graph_cap_stay_apart() {
    let s = store();
    let conn = s.conn.lock().unwrap();
    let long = "Industrial grade modular shelving unit with adjustable brackets, Package";
    let blue = subject_key(&conn, &format!("{long} Blue")).unwrap();
    let grey = subject_key(&conn, &format!("{long} Grey")).unwrap();
    assert_ne!(blue, grey);
    assert!(blue.ends_with("blue"), "{blue}");
}
