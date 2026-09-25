//! What makes two fact statements about the same slot, and two values the
//! same value. Pure except for the alias lookup.

use anyhow::{Result, bail, ensure};
use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use rusqlite::Connection;

use crate::updates::scan::{Class, scan};

/// Longest predicate or qualifier key.
const MAX_KEY: usize = 48;

/// Collapse whitespace and fold case.
fn fold(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Lowercase, spaces and underscores to `-`, runs collapsed, ends trimmed.
fn slug(text: &str) -> String {
    let lowered: String = text
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_whitespace() || c == '_' {
                '-'
            } else {
                c
            }
        })
        .collect();
    let mut out = String::with_capacity(lowered.len());
    for c in lowered.chars() {
        if c == '-' && out.ends_with('-') {
            continue;
        }
        out.push(c);
    }
    out.trim_matches('-').to_owned()
}

/// The subject a fact is about: an alias resolves to its target, then the
/// graph's own canonical form, so "Widget App" and "widget-app" are one.
/// Uncapped: the graph cuts names at 60 characters, and two long product
/// names that differ only after that are two subjects (review point).
pub fn subject_key(conn: &Connection, subject: &str) -> Result<String> {
    let raw = subject.trim();
    ensure!(!raw.is_empty(), "a fact needs a subject");
    let canonical = crate::graph::canonical::canonicalize_name_uncapped(raw);
    for candidate in [raw.to_owned(), canonical.clone()] {
        if candidate.is_empty() {
            continue;
        }
        if let Some(target) = crate::storage::Storage::canonical_for_alias_conn(conn, &candidate)? {
            let key = crate::graph::canonical::canonicalize_name_uncapped(&target);
            if !key.is_empty() {
                return Ok(key);
            }
        }
    }
    if !canonical.is_empty() {
        return Ok(canonical);
    }
    let folded = fold(raw);
    ensure!(!folded.is_empty(), "a fact needs a subject");
    Ok(folded)
}

/// The predicate key: a slug, with the common business predicates folded
/// to one spelling. Anything else is kept as written (agents use free
/// keys such as `checkpoint`).
pub fn predicate_key(predicate: &str) -> Result<String> {
    let key = slug(predicate);
    ensure!(!key.is_empty(), "a fact needs a predicate");
    ensure!(
        key.chars().count() <= MAX_KEY,
        "a predicate is at most {MAX_KEY} characters"
    );
    Ok(match key.as_str() {
        "price" | "has-price" | "pricing" | "cost" | "цена" | "стоимость" => "price",
        "deadline" | "due" | "due-date" | "дедлайн" | "срок" => "deadline",
        "commission" | "комиссия" => "commission",
        "discount" | "скидка" => "discount",
        "budget" | "бюджет" => "budget",
        "payment-terms" | "terms" | "условия-оплаты" | "условия" => {
            "payment-terms"
        }
        _ => return Ok(key),
    }
    .to_owned())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PredicateClass {
    Money,
    Date,
    Terms,
    Other,
}

/// Which trust rules a predicate falls under.
pub fn predicate_class(key: &str) -> PredicateClass {
    const MONEY: &[&str] = &[
        "price",
        "cost",
        "fee",
        "commission",
        "discount",
        "budget",
        "rate",
        "deposit",
        "margin",
        "markup",
    ];
    const DATE: &[&str] = &["deadline", "launch-date", "renewal-date", "eta"];
    const TERMS: &[&str] = &[
        "payment-terms",
        "lead-time",
        "moq",
        "supplier",
        "sla",
        "warranty",
    ];
    if MONEY
        .iter()
        .any(|m| key == *m || key.ends_with(&format!("-{m}")))
    {
        PredicateClass::Money
    } else if DATE.contains(&key) || key.ends_with("-date") {
        PredicateClass::Date
    } else if TERMS.contains(&key) {
        PredicateClass::Terms
    } else {
        PredicateClass::Other
    }
}

/// A qualifier (a plan, a region, a variant) as a key; empty when none.
pub fn qualifier_key(qualifier: &str) -> Result<String> {
    let key = slug(qualifier);
    ensure!(
        key.chars().count() <= MAX_KEY,
        "a qualifier is at most {MAX_KEY} characters"
    );
    Ok(key)
}

/// What two values are compared by. Money reads the same whichever way it
/// is written (`$6`, `$6.00`, `6 USD`); a percentage too; anything else by
/// its folded text. A false "different" only costs a history row; a false
/// "equal" is kept out by comparing nothing looser than this.
pub fn value_norm(value: &str) -> String {
    let scanned = scan(value);
    let folded = fold(value);
    // Only when the one value is the whole of it: "$6 per pallet" is not
    // "$6 per square foot", "$6 + 10%" is not "$6 + 20%".
    // Around the amount only layout may remain; an operator (<, +, ~, =)
    // changes what the value says.
    let whole = |surface: &str| {
        folded.replacen(surface, "", 1).chars().all(|c| {
            c.is_whitespace() || matches!(c, '.' | ',' | ';' | ':' | '!' | '(' | ')' | '"' | '\'')
        })
    };
    if scanned.values.len() == 1
        && scanned.prior.is_empty()
        && scanned.words.is_empty()
        && scanned.preds.is_empty()
        && whole(&scanned.values[0].surface)
        && matches!(
            scanned.values[0].class,
            Class::Money | Class::Terms | Class::Date
        )
    {
        return scanned.values[0].key.clone();
    }
    if let Some(number) = folded.strip_suffix('%').map(str::trim_end)
        && let Some(amount) = crate::updates::scan::amount(number, 0)
        && number
            .chars()
            .all(|c| c.is_ascii_digit() || c == '.' || c == ',')
    {
        return format!("pct:{amount}");
    }
    folded
}

/// A time as epoch milliseconds: RFC 3339, `YYYY-MM-DD HH:MM:SS` (UTC), or a
/// bare date (its midnight, UTC).
pub fn parse_time_ms(text: &str) -> Result<i64> {
    let text = text.trim();
    if let Ok(time) = DateTime::parse_from_rfc3339(text) {
        return Ok(time.timestamp_millis());
    }
    if let Ok(time) = NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S") {
        return Ok(time.and_utc().timestamp_millis());
    }
    if let Ok(date) = NaiveDate::parse_from_str(text, "%Y-%m-%d") {
        return Ok(date
            .and_hms_opt(0, 0, 0)
            .unwrap_or_default()
            .and_utc()
            .timestamp_millis());
    }
    bail!("unreadable time {text:?}: use RFC 3339 or YYYY-MM-DD")
}

/// Epoch milliseconds back to the RFC 3339 text stored beside them.
pub fn format_ms(ms: i64) -> String {
    DateTime::<Utc>::from_timestamp_millis(ms)
        .unwrap_or_default()
        .to_rfc3339()
}

#[cfg(test)]
#[path = "keys_tests.rs"]
mod tests;
