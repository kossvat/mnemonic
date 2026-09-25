//! The "Key facts" block of a project digest: what the project's facts
//! hold now, the value each replaced when that is recent, and proposals
//! waiting for review, each line cited.

use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use rusqlite::params;

use super::keys::{self, PredicateClass};
use super::store::{SlotView, ValueView, scope_key};
use super::view::slot_view;
use crate::storage::Storage;

/// A changed value shows what it replaced for this long.
const TRAIL_DAYS: i64 = 30;
/// A retraction stays on the page for this long.
const RETRACTED_DAYS: i64 = 14;
const MAX_LINE: usize = 160;
const MAX_VALUE: usize = 60;

#[derive(Debug, Clone, PartialEq)]
pub struct KeyFact {
    pub line: String,
    /// Memory ids the line cites.
    pub cites: Vec<String>,
}

fn since(value: &ValueView) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(&value.valid_from)
        .ok()
        .map(|t| t.with_timezone(&Utc))
}

fn day(value: &ValueView) -> String {
    value.valid_from.chars().take(10).collect()
}

fn clip(text: &str, max: usize) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.chars().count() <= max {
        return text;
    }
    let mut cut: String = text.chars().take(max.saturating_sub(3)).collect();
    cut.push_str("...");
    cut
}

fn cite(value: &ValueView, cites: &mut Vec<String>) -> String {
    match &value.source_memory_id {
        Some(id) => {
            cites.push(id.clone());
            format!(" `{}`", id.get(..8).unwrap_or(id))
        }
        None => String::new(),
    }
}

/// One slot's line, or `None` when it holds nothing worth showing.
fn line(view: &SlotView, predicate: &str, now: DateTime<Utc>) -> Option<(KeyFact, Rank)> {
    let latest = view.history.first()?;
    let latest_at = since(latest).unwrap_or(DateTime::<Utc>::MIN_UTC);
    let mut about = format!("{} {}", view.subject, view.predicate);
    if !view.qualifier.is_empty() {
        about.push_str(&format!(" ({})", view.qualifier));
    }
    let mut cites = Vec::new();
    let mut text = match (&view.current, latest.value.as_ref()) {
        (Some(current), _) => {
            let mut text = format!(
                "- {about}: {}",
                clip(current.value.as_deref().unwrap_or(""), MAX_VALUE)
            );
            if current.trust == "provisional" {
                text.push_str(" (unconfirmed)");
            }
            text.push_str(&cite(current, &mut cites));
            if now - latest_at <= Duration::days(TRAIL_DAYS)
                && let Some(previous) = view.history.get(1)
                && let Some(value) = &previous.value
            {
                text.push_str(&format!(
                    "; was {} until {}",
                    clip(value, MAX_VALUE),
                    day(current)
                ));
                text.push_str(&cite(previous, &mut cites));
            }
            text
        }
        (None, None) if now - latest_at <= Duration::days(RETRACTED_DAYS) => {
            let mut text = format!("- {about}: retracted {}", day(latest));
            if let Some(value) = view.history.get(1).and_then(|v| v.value.as_ref()) {
                text.push_str(&format!("; was {}", clip(value, MAX_VALUE)));
            }
            text
        }
        _ => return None,
    };
    if view.pending > 0 {
        text.push_str(&format!(" (+{} pending review)", view.pending));
    }
    if text.chars().count() > MAX_LINE {
        text = clip(&text, MAX_LINE);
    }
    let rank = Rank {
        recent: now - latest_at <= Duration::days(TRAIL_DAYS),
        guarded: keys::predicate_class(predicate) != PredicateClass::Other,
        at: latest_at,
    };
    Some((KeyFact { line: text, cites }, rank))
}

/// Recently changed first, then money, dates and terms, then newest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Rank {
    recent: bool,
    guarded: bool,
    at: DateTime<Utc>,
}

/// The key facts of `project`: its own slots, plus slots with no project
/// whose subject is the project itself.
pub fn key_facts(
    storage: &Storage,
    project: &str,
    now: DateTime<Utc>,
    limit: usize,
) -> Result<Vec<KeyFact>> {
    let conn = storage
        .conn
        .lock()
        .map_err(|e| anyhow::anyhow!("lock: {e}"))?;
    let (scope, _) = scope_key(&conn, Some(project))?;
    let subject = keys::subject_key(&conn, project)?;
    let slots: Vec<(String, String)> = {
        let mut stmt = conn.prepare(
            "SELECT id, predicate FROM fact_slots
              WHERE scope_key = ?1 OR (scope_key = '' AND subject_key = ?2)
              ORDER BY subject_key, predicate, qualifier_key",
        )?;
        stmt.query_map(params![scope, subject], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?
    };
    let mut ranked = Vec::new();
    for (slot, predicate) in slots {
        if let Some(found) = line(&slot_view(&conn, &slot)?, &predicate, now) {
            ranked.push(found);
        }
    }
    // Stable: equal ranks keep the subject and predicate order.
    ranked.sort_by_key(|(_, rank)| std::cmp::Reverse(*rank));
    Ok(ranked
        .into_iter()
        .take(limit)
        .map(|(fact, _)| fact)
        .collect())
}

/// Projects whose facts changed within `days`, newest first.
pub fn projects_with_recent_facts(
    storage: &Storage,
    days: i64,
    now: DateTime<Utc>,
    limit: usize,
) -> Result<Vec<String>> {
    let conn = storage
        .conn
        .lock()
        .map_err(|e| anyhow::anyhow!("lock: {e}"))?;
    let cutoff = (now - Duration::days(days)).to_rfc3339();
    let mut stmt = conn.prepare(
        "SELECT scope FROM fact_slots WHERE scope_key <> '' AND updated_at >= ?1
          GROUP BY scope_key ORDER BY MAX(updated_at) DESC LIMIT ?2",
    )?;
    let names = stmt
        .query_map(params![cutoff, limit as i64], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(names)
}

#[cfg(test)]
#[path = "digest_tests.rs"]
mod tests;
