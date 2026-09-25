//! What a new statement does to a slot's chain. Pure: no clock, no store.
//!
//! A slot's rows are ordered by (valid_from_ms, seq); "current" is derived
//! as the last active row, never stored, so there is nothing to keep in
//! sync and nothing a delete can leave inconsistent.

use super::keys::PredicateClass;

/// How far into the future a statement may be dated: clock skew, not
/// scheduling.
pub const FUTURE_SLACK_MS: i64 = 5 * 60 * 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trust {
    /// Written by the owner through the CLI, or imported from the old table.
    Manual,
    /// Stated by an agent over MCP.
    Declared,
    /// A provisional value a human confirmed.
    Confirmed,
    /// Proposed by an extractor; reachable only through the Rust API.
    Provisional,
}

impl Trust {
    pub fn authoritative(self) -> bool {
        !matches!(self, Trust::Provisional)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Trust::Manual => "manual",
            Trust::Declared => "declared",
            Trust::Confirmed => "confirmed",
            Trust::Provisional => "provisional",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "manual" => Trust::Manual,
            "declared" => Trust::Declared,
            "confirmed" => Trust::Confirmed,
            "provisional" => Trust::Provisional,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Value,
    /// "This slot has no value any more."
    Retraction,
}

/// An active row of a slot's chain.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub seq: i64,
    pub kind: Kind,
    pub value_norm: String,
    pub valid_from_ms: i64,
    /// The latest time the value was said to hold: its start, or later when
    /// it was reconfirmed as of a later time.
    pub asserted_ms: i64,
    pub trust: Trust,
}

/// A statement about to be applied.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub kind: Kind,
    pub value_norm: String,
    pub valid_from_ms: i64,
    pub trust: Trust,
    pub class: PredicateClass,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// The first value of a slot with none.
    Create,
    /// A new current value.
    Update,
    /// The slot has no value from now on.
    Retract,
    /// The row with this seq already says it: nothing new is written.
    Reconfirm(i64),
    /// True then, not now: kept in the trail, never current.
    History,
    /// Dated inside the span the row with this seq was said to hold
    /// through: kept in the trail, and that row's value comes back at the
    /// time it was last said.
    Interrupt(i64),
    /// A provisional value that may not replace what the slot holds.
    PendingReview,
    Reject(&'static str),
}

/// The value the slot holds now: its last row, unless that is a retraction.
pub fn current(chain: &[Row]) -> Option<&Row> {
    chain.last().filter(|row| row.kind == Kind::Value)
}

fn same(a_kind: Kind, a_norm: &str, b: &Row) -> bool {
    a_kind == b.kind && a_norm == b.value_norm
}

/// Decide what `candidate` does to `chain` (active rows in order).
pub fn decide(candidate: &Candidate, chain: &[Row], now_ms: i64) -> Decision {
    if candidate.valid_from_ms > now_ms + FUTURE_SLACK_MS {
        return Decision::Reject("a fact cannot be dated in the future");
    }
    let now = current(chain);
    if candidate.trust == Trust::Provisional {
        let guarded = matches!(
            candidate.class,
            PredicateClass::Money | PredicateClass::Date | PredicateClass::Terms
        );
        if let Some(now) = now
            && (now.trust.authoritative() || guarded)
        {
            return if same(candidate.kind, &candidate.value_norm, now) {
                Decision::Reconfirm(now.seq)
            } else {
                Decision::PendingReview
            };
        }
    }
    // Where the statement falls in time. A tie goes after the existing
    // rows: it is the later commit.
    let at = chain.partition_point(|row| row.valid_from_ms <= candidate.valid_from_ms);
    let before = at.checked_sub(1).map(|i| &chain[i]);
    if let Some(before) = before
        && same(candidate.kind, &candidate.value_norm, before)
    {
        return Decision::Reconfirm(before.seq);
    }
    if let Some(after) = chain.get(at)
        && same(candidate.kind, &candidate.value_norm, after)
    {
        return Decision::Reconfirm(after.seq);
    }
    // The value before was said again later than this statement's date: it
    // held then too, so this one did not last (review point: a backfill
    // must not outrank a later reconfirmation).
    if let Some(before) = before
        && before.asserted_ms > candidate.valid_from_ms
    {
        return Decision::Interrupt(before.seq);
    }
    if at < chain.len() {
        return Decision::History;
    }
    match (candidate.kind, now) {
        (Kind::Retraction, None) => Decision::Reject("nothing to retract"),
        (Kind::Retraction, Some(_)) => Decision::Retract,
        (Kind::Value, None) => Decision::Create,
        (Kind::Value, Some(_)) => Decision::Update,
    }
}

#[cfg(test)]
#[path = "rule_tests.rs"]
mod tests;
