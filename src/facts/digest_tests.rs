use super::*;
use crate::facts::rule::Trust;
use crate::facts::store::{FactWrite, apply};
use crate::test_support::InTempDir;

fn store() -> InTempDir<Storage> {
    InTempDir::new("mnemonic-fact-digest-", |dir| {
        Storage::open(&dir.join("memory.db")).unwrap()
    })
}

fn state<'a>(
    project: &'a str,
    subject: &'a str,
    predicate: &'a str,
    value: &'a str,
    as_of: &'a str,
) -> FactWrite<'a> {
    FactWrite {
        project: Some(project),
        subject,
        predicate,
        value: Some(value),
        as_of: Some(as_of),
        actor: "test",
        ..Default::default()
    }
}

fn now() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-09-24T12:00:00Z")
        .unwrap()
        .with_timezone(&Utc)
}

fn lines(s: &Storage, project: &str) -> Vec<String> {
    key_facts(s, project, now(), 5)
        .unwrap()
        .into_iter()
        .map(|f| f.line)
        .collect()
}

#[test]
fn recent_changes_come_first_and_money_before_other_facts() {
    let s = store();
    for write in [
        state("alpha-shop", "Order 7", "status", "on hold", "2026-05-01"),
        state("alpha-shop", "Widget", "price", "$4", "2026-05-01"),
        state("alpha-shop", "Gadget", "price", "$5", "2026-05-01"),
        state("alpha-shop", "Gadget", "price", "$6", "2026-09-20"),
        state("alpha-shop", "Order 8", "status", "shipped", "2026-09-21"),
    ] {
        apply(&s, &write).unwrap();
    }
    assert_eq!(
        lines(&s, "alpha-shop"),
        vec![
            "- Gadget price: $6; was $5 until 2026-09-20",
            "- Order 8 status: shipped",
            "- Widget price: $4",
            "- Order 7 status: on hold",
        ]
    );
}

#[test]
fn proposals_are_marked_and_pending_ones_counted() {
    let s = store();
    let proposal = |value, as_of| FactWrite {
        trust: Some(Trust::Provisional),
        ..state("alpha-shop", "Order 7", "status", value, as_of)
    };
    apply(&s, &proposal("on hold", "2026-09-01")).unwrap();
    apply(
        &s,
        &state("alpha-shop", "Widget", "price", "$5", "2026-09-01"),
    )
    .unwrap();
    apply(
        &s,
        &FactWrite {
            trust: Some(Trust::Provisional),
            ..state("alpha-shop", "Widget", "price", "$9", "2026-09-02")
        },
    )
    .unwrap();
    let got = lines(&s, "alpha-shop");
    assert!(
        got.contains(&"- Widget price: $5 (+1 pending review)".to_owned()),
        "{got:?}"
    );
    assert!(
        got.contains(&"- Order 7 status: on hold (unconfirmed)".to_owned()),
        "{got:?}"
    );
    assert!(!got.iter().any(|l| l.contains("$9")), "{got:?}");
}

#[test]
fn another_projects_facts_stay_out() {
    let s = store();
    apply(
        &s,
        &state("alpha-shop", "Widget", "price", "$5", "2026-09-01"),
    )
    .unwrap();
    apply(
        &s,
        &state("beta-lab", "Widget", "price", "$9", "2026-09-01"),
    )
    .unwrap();
    // A fact with no project shows only under the project it is about.
    apply(
        &s,
        &FactWrite {
            project: None,
            subject: "alpha-shop",
            ..state("", "", "owner", "Dana", "2026-09-01")
        },
    )
    .unwrap();
    assert_eq!(
        lines(&s, "alpha-shop"),
        vec!["- Widget price: $5", "- alpha-shop owner: Dana"]
    );
    assert_eq!(lines(&s, "beta-lab"), vec!["- Widget price: $9"]);
}

#[test]
fn a_long_value_is_clipped_and_an_old_retraction_dropped() {
    let s = store();
    let long = "Net 30 with a 2% discount inside 10 days, ".repeat(10);
    apply(
        &s,
        &state("alpha-shop", "Acme", "payment-terms", &long, "2026-09-01"),
    )
    .unwrap();
    let got = lines(&s, "alpha-shop");
    assert!(got[0].chars().count() <= MAX_LINE, "{got:?}");
    assert!(got[0].ends_with("..."), "{got:?}");

    apply(
        &s,
        &state("alpha-shop", "Gizmo", "price", "$3", "2026-06-01"),
    )
    .unwrap();
    let retract = |as_of| FactWrite {
        value: None,
        ..state("alpha-shop", "Gizmo", "price", "", as_of)
    };
    apply(&s, &retract("2026-09-20")).unwrap();
    assert!(
        lines(&s, "alpha-shop").contains(&"- Gizmo price: retracted 2026-09-20; was $3".to_owned())
    );
    let s = store();
    apply(
        &s,
        &state("alpha-shop", "Gizmo", "price", "$3", "2026-06-01"),
    )
    .unwrap();
    apply(&s, &retract("2026-07-01")).unwrap();
    assert!(lines(&s, "alpha-shop").is_empty());
}

#[test]
fn projects_with_changed_facts_are_listed() {
    let s = store();
    apply(
        &s,
        &state("alpha-shop", "Widget", "price", "$5", "2026-09-01"),
    )
    .unwrap();
    let names = projects_with_recent_facts(&s, 14, Utc::now(), 10).unwrap();
    assert_eq!(names, vec!["alpha-shop"]);
    assert!(
        projects_with_recent_facts(&s, 14, Utc::now() + Duration::days(30), 10)
            .unwrap()
            .is_empty()
    );
}
