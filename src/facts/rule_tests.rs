use super::*;
use crate::facts::keys::value_norm;

const NOW: i64 = 1_000_000_000;
const MIN: i64 = 60_000;

fn row(seq: i64, value: &str, at: i64, trust: Trust) -> Row {
    let (kind, value_norm) = match value {
        "-" => (Kind::Retraction, String::new()),
        v => (Kind::Value, super::super::keys::value_norm(v)),
    };
    Row {
        seq,
        kind,
        value_norm,
        valid_from_ms: at,
        asserted_ms: at,
        trust,
    }
}

fn candidate(value: &str, at: i64, trust: Trust, class: PredicateClass) -> Candidate {
    let (kind, value_norm) = match value {
        "-" => (Kind::Retraction, String::new()),
        v => (Kind::Value, value_norm(v)),
    };
    Candidate {
        kind,
        value_norm,
        valid_from_ms: at,
        trust,
        class,
    }
}

use Decision::*;
use PredicateClass::{Money, Other};
use Trust::{Declared, Manual, Provisional};

/// (chain as value@minutes, candidate value@minutes, trust, class, expected)
type Case = (
    &'static [(&'static str, i64)],
    &'static str,
    i64,
    Trust,
    PredicateClass,
    Decision,
);

#[test]
fn policy_matrix() {
    let cases: &[Case] = &[
        // An empty slot.
        (&[], "$5", 0, Declared, Money, Create),
        (&[], "-", 0, Declared, Money, Reject("nothing to retract")),
        (
            &[],
            "$5",
            6,
            Declared,
            Money,
            Reject("a fact cannot be dated in the future"),
        ),
        (&[], "$5", 4, Declared, Money, Create),
        // A newer different value replaces; an equal one reconfirms.
        (&[("$5", -10)], "$6", 0, Declared, Money, Update),
        (&[("$5", -10)], "$5", 0, Declared, Money, Reconfirm(1)),
        (&[("$5", -10)], "$5.00", 0, Declared, Money, Reconfirm(1)),
        (&[("$5", -10)], "5 USD", 0, Declared, Money, Reconfirm(1)),
        (&[("$6", -10)], "$60", 0, Declared, Money, Update),
        (&[("$6", -10)], "$6", 0, Manual, Money, Reconfirm(1)),
        (
            &[("Net 30", -10)],
            "net 30",
            0,
            Declared,
            Other,
            Reconfirm(1),
        ),
        (&[("15%", -10)], "15 %", 0, Declared, Other, Reconfirm(1)),
        (
            &[("on hold", -10)],
            "On  Hold",
            0,
            Declared,
            Other,
            Reconfirm(1),
        ),
        (&[("on hold", -10)], "shipped", 0, Declared, Other, Update),
        // $5, $6, back to $5: an update, not the old row again.
        (
            &[("$5", -20), ("$6", -10)],
            "$5",
            0,
            Declared,
            Money,
            Update,
        ),
        (
            &[("$5", -20), ("$6", -10)],
            "$6",
            0,
            Declared,
            Money,
            Reconfirm(2),
        ),
        // An older statement is history, or reconfirms the row it matches.
        (
            &[("$5", -20), ("$6", -10)],
            "$4",
            -30,
            Declared,
            Money,
            History,
        ),
        (
            &[("$5", -20), ("$6", -10)],
            "$5.5",
            -15,
            Declared,
            Money,
            History,
        ),
        (
            &[("$5", -20), ("$6", -10)],
            "$6",
            -15,
            Declared,
            Money,
            Reconfirm(2),
        ),
        (
            &[("$5", -20), ("$6", -10)],
            "$5",
            -15,
            Declared,
            Money,
            Reconfirm(1),
        ),
        (
            &[("$5", -20), ("$6", -10)],
            "$7",
            -15,
            Declared,
            Money,
            History,
        ),
        // A tie in time goes after the existing row.
        (&[("$5", -10)], "$6", -10, Declared, Money, Update),
        (&[("$5", -10)], "$5", -10, Declared, Money, Reconfirm(1)),
        (
            &[("$5", -10), ("$6", -10)],
            "$7",
            -10,
            Declared,
            Money,
            Update,
        ),
        // Retractions.
        (&[("$5", -10)], "-", 0, Declared, Money, Retract),
        (
            &[("$5", -20), ("-", -10)],
            "-",
            0,
            Declared,
            Money,
            Reconfirm(2),
        ),
        (&[("$5", -20), ("-", -10)], "$5", 0, Declared, Money, Create),
        (&[("$5", -20), ("-", -10)], "$6", 0, Declared, Money, Create),
        // An earlier retraction next to a later one says what it says.
        (
            &[("$5", -20), ("-", -10)],
            "-",
            -15,
            Declared,
            Money,
            Reconfirm(2),
        ),
        (
            &[("$5", -20), ("-", -10)],
            "$7",
            -15,
            Declared,
            Money,
            History,
        ),
        // Provisional values never replace an authoritative one.
        (&[("$5", -10)], "$6", 0, Provisional, Money, PendingReview),
        (&[("$5", -10)], "$5", 0, Provisional, Money, Reconfirm(1)),
        (
            &[("on hold", -10)],
            "shipped",
            0,
            Provisional,
            Other,
            PendingReview,
        ),
        (
            &[("on hold", -10)],
            "on hold",
            0,
            Provisional,
            Other,
            Reconfirm(1),
        ),
        (&[], "$5", 0, Provisional, Money, Create),
        (
            &[("$5", -20), ("-", -10)],
            "$6",
            0,
            Provisional,
            Money,
            Create,
        ),
        // Manual and declared supersede each other by time.
        (&[("$5", -10)], "$6", 0, Manual, Money, Update),
        (&[("$5", -10)], "$4", -20, Manual, Money, History),
        (
            &[("$5", -10), ("$6", -5)],
            "$6",
            -1,
            Manual,
            Money,
            Reconfirm(2),
        ),
        // Long chains.
        (
            &[("$1", -50), ("$2", -40), ("$3", -30), ("$4", -20)],
            "$2",
            0,
            Declared,
            Money,
            Update,
        ),
        (
            &[("$1", -50), ("$2", -40), ("$3", -30), ("$4", -20)],
            "$9",
            -35,
            Declared,
            Money,
            History,
        ),
        (
            &[("$1", -50), ("$2", -40), ("$3", -30), ("$4", -20)],
            "$3",
            -25,
            Declared,
            Money,
            Reconfirm(3),
        ),
    ];
    assert!(cases.len() >= 40, "{}", cases.len());
    for (i, (chain, value, at, trust, class, want)) in cases.iter().enumerate() {
        let rows: Vec<Row> = chain
            .iter()
            .enumerate()
            .map(|(seq, (v, minutes))| row(seq as i64 + 1, v, NOW + minutes * MIN, Declared))
            .collect();
        let got = decide(
            &candidate(value, NOW + at * MIN, *trust, *class),
            &rows,
            NOW,
        );
        assert_eq!(
            &got, want,
            "case {i}: {chain:?} + {value}@{at} ({trust:?}, {class:?})"
        );
    }
}

#[test]
fn current_is_the_last_row_unless_retracted() {
    let chain = vec![
        row(1, "$5", NOW - MIN, Declared),
        row(2, "$6", NOW, Declared),
    ];
    assert_eq!(current(&chain).unwrap().seq, 2);
    let chain = vec![
        row(1, "$5", NOW - MIN, Declared),
        row(2, "-", NOW, Declared),
    ];
    assert!(current(&chain).is_none());
    assert!(current(&[]).is_none());
}

#[test]
fn a_provisional_current_guards_money_but_chains_elsewhere() {
    let provisional = |value: &str| vec![row(1, value, NOW - 10 * MIN, Provisional)];
    let next = |value: &str, class| candidate(value, NOW, Provisional, class);
    assert_eq!(
        decide(&next("$6", Money), &provisional("$5"), NOW),
        PendingReview
    );
    assert_eq!(
        decide(&next("shipped", Other), &provisional("on hold"), NOW),
        Update
    );
    // An authoritative write replaces a provisional value like any other.
    let declared = candidate("$6", NOW, Declared, Money);
    assert_eq!(decide(&declared, &provisional("$5"), NOW), Update);
}

#[test]
fn a_backfill_inside_a_reconfirmed_span_is_history_and_the_value_returns() {
    // $5 from -60, said again as of -10; $6 dated -30 fell inside that span.
    let mut five = row(1, "$5", NOW - 60 * MIN, Declared);
    five.asserted_ms = NOW - 10 * MIN;
    let chain = vec![five.clone()];
    let six = |at| candidate("$6", NOW + at * MIN, Declared, Money);
    assert_eq!(decide(&six(-30), &chain, NOW), Interrupt(1));
    assert_eq!(
        decide(
            &candidate("-", NOW - 30 * MIN, Declared, Money),
            &chain,
            NOW
        ),
        Interrupt(1)
    );
    // At or after the last time it was said, a new value takes over.
    assert_eq!(decide(&six(-10), &chain, NOW), Update);
    assert_eq!(decide(&six(-5), &chain, NOW), Update);
    // Saying $5 inside the span only reconfirms it.
    assert_eq!(
        decide(
            &candidate("$5", NOW - 30 * MIN, Declared, Money),
            &chain,
            NOW
        ),
        Reconfirm(1)
    );
    // With a later row, the span still decides.
    let chain = vec![five, row(2, "$7", NOW - 5 * MIN, Declared)];
    assert_eq!(decide(&six(-30), &chain, NOW), Interrupt(1));
    assert_eq!(decide(&six(-8), &chain, NOW), History);
}
