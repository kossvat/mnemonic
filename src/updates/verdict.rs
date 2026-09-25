//! Is a new memory the same statement as an older one, an update of it, or
//! about something else? Pure: the caller decides what to do with the answer.

use super::scan::{Class, Scan, Value};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Same values about the same thing: a duplicate.
    Same,
    /// The same thing with a changed value of `class`.
    Update {
        class: Class,
        was: Vec<Value>,
        now: Vec<Value>,
    },
    /// Not comparable: a different thing, or no shared kind of value.
    Distinct,
}

/// Compare two scans that both carry values. A pair where either side has
/// none is `Distinct`: value-less text keeps the plain duplicate rule, which
/// the caller applies before asking.
///
/// Distinct is the safe answer. It never drops a memory; the worst a miss
/// costs is an extra memory without a link.
pub fn verdict(new: &Scan, old: &Scan) -> Verdict {
    if !new.has_values() || !old.has_values() {
        return Verdict::Distinct;
    }
    // A value asked about, proposed or bounded replaces nothing and is
    // nothing's duplicate.
    if new.hypothetical || old.hypothetical {
        return Verdict::Distinct;
    }
    // Several values in one text ("Widget $5, Gadget $6", "commission 10%,
    // discount 20%"): which value belongs to what only survives as the
    // pairing of each value with its predicate and the words before it, so
    // compare pairings, and never guess an update from them.
    // One value each: it must be about the same subject, not merely sit in
    // a text with the same words ("Gadget $5. Widget is free." against
    // "Widget $6. Gadget is free.").
    if new.values.len() == 1
        && old.values.len() == 1
        && new.values[0].context != old.values[0].context
    {
        return Verdict::Distinct;
    }
    if new.values.len() > 1 || old.values.len() > 1 {
        fn pairs(scan: &Scan) -> Vec<(Option<&str>, &[String], &str)> {
            scan.values
                .iter()
                .map(|v| (v.pred, v.context.as_slice(), v.key.as_str()))
                .collect()
        }
        return if pairs(new) == pairs(old) && new.words == old.words && new.preds == old.preds {
            Verdict::Same
        } else {
            Verdict::Distinct
        };
    }
    fn keys(scan: &Scan, class: Option<Class>) -> Vec<&str> {
        let mut keys: Vec<&str> = scan
            .values
            .iter()
            .filter(|v| class.is_none_or(|c| v.class == c))
            .map(|v| v.key.as_str())
            .collect();
        keys.sort_unstable();
        keys
    }
    if keys(new, None) == keys(old, None) {
        // Several values in another order may belong to other things now
        // ("Widget $5, Gadget $6" against "Widget $6, Gadget $5"): not the
        // same statement, and not a change this rule can pin down.
        fn in_order(scan: &Scan) -> Vec<&str> {
            scan.values.iter().map(|v| v.key.as_str()).collect()
        }
        return if new.words == old.words && new.preds == old.preds && in_order(new) == in_order(old)
        {
            Verdict::Same
        } else {
            Verdict::Distinct
        };
    }
    if !new.preds.is_empty() && !old.preds.is_empty() && new.preds != old.preds {
        return Verdict::Distinct;
    }
    if new.words != old.words {
        return Verdict::Distinct;
    }
    // `$6/month` and `$6/year` are two prices, not one that changed; so
    // are a limit per day and one per year.
    if periods(new) != periods(old) {
        return Verdict::Distinct;
    }
    for class in [
        Class::Money,
        Class::Percent,
        Class::Terms,
        Class::Date,
        Class::Number,
    ] {
        let (a, b) = (keys(new, Some(class)), keys(old, Some(class)));
        if a.is_empty() || b.is_empty() || a == b {
            continue;
        }
        let apart = |scan: &Scan, other: &[&str]| -> Vec<Value> {
            scan.values
                .iter()
                .filter(|v| v.class == class && !other.contains(&v.key.as_str()))
                .cloned()
                .collect()
        };
        let now = apart(new, &b);
        let mut was = apart(old, &a);
        if was.is_empty() {
            was = apart(old, &[]);
        }
        return Verdict::Update { class, was, now };
    }
    Verdict::Distinct
}

/// The periods a scan's money and number values carry, as a sorted multiset.
fn periods(scan: &Scan) -> Vec<&str> {
    let mut periods: Vec<&str> = scan
        .values
        .iter()
        .filter(|v| matches!(v.class, Class::Money | Class::Number))
        .map(|v| v.key.split_once('/').map_or("", |(_, period)| period))
        .collect();
    periods.sort_unstable();
    periods
}

#[cfg(test)]
#[path = "verdict_tests.rs"]
mod tests;
