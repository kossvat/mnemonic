//! A subject or project renamed or merged in the graph: its facts follow.

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};

use super::keys;
use super::rule::{self, Candidate, Decision, Row, Trust};
use super::store;

fn subject_key(name: &str) -> String {
    let key = crate::graph::canonical::canonicalize_name_uncapped(name);
    if key.is_empty() {
        name.trim().to_lowercase()
    } else {
        key
    }
}

/// A proposed value moving into a slot goes through the same guard as a
/// proposal written there: it waits for review rather than replacing what
/// the slot holds (review point).
fn hold_back_proposals(conn: &Connection, slot: &str, target: &str, predicate: &str) -> Result<()> {
    let chain: Vec<Row> = store::chain(conn, target)?
        .into_iter()
        .map(|(row, _)| row)
        .collect();
    let now = chrono::Utc::now().timestamp_millis();
    let class = keys::predicate_class(predicate);
    for (row, _) in store::chain(conn, slot)? {
        if row.trust != Trust::Provisional {
            continue;
        }
        let candidate = Candidate {
            kind: row.kind,
            value_norm: row.value_norm.clone(),
            valid_from_ms: row.valid_from_ms,
            trust: row.trust,
            class,
        };
        match rule::decide(&candidate, &chain, now) {
            Decision::PendingReview => {
                conn.execute(
                    "UPDATE fact_values SET status = 'pending_review' WHERE seq = ?1",
                    [row.seq],
                )?;
            }
            // The value the other slot already holds: a reconfirmation of
            // that row, not a proposal that would become current and lower
            // the slot's trust (review point).
            Decision::Reconfirm(matched) => {
                conn.execute(
                    "UPDATE fact_values
                        SET reconfirm_count = reconfirm_count + 1, reconfirmed_at = ?2,
                            asserted_ms = MAX(COALESCE(asserted_ms, valid_from_ms), ?3)
                      WHERE seq = ?1",
                    params![matched, chrono::Utc::now().to_rfc3339(), row.valid_from_ms],
                )?;
                conn.execute(
                    "UPDATE fact_values SET status = 'rejected' WHERE seq = ?1",
                    [row.seq],
                )?;
            }
            _ => {}
        }
    }
    Ok(())
}

/// Move the facts about `old` (as a subject, and as a project) to `new`.
/// A slot that already exists under the new name takes the moved values:
/// the chain re-derives by time, so the newest value stays current.
pub fn rekey_in_tx(conn: &Connection, old: &str, new: &str) -> Result<()> {
    let now = chrono::Utc::now().to_rfc3339();
    for column in ["subject_key", "scope_key"] {
        let (from, to) = if column == "subject_key" {
            (subject_key(old), subject_key(new))
        } else {
            (
                crate::followups::project_key(old),
                crate::followups::project_key(new),
            )
        };
        if from == to {
            continue;
        }
        let slots: Vec<(String, String, String, String, String)> = {
            let mut stmt = conn.prepare(&format!(
                "SELECT id, scope_key, subject_key, predicate, qualifier_key FROM fact_slots
                  WHERE {column} = ?1"
            ))?;
            stmt.query_map([&from], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
            })?
            .collect::<rusqlite::Result<_>>()?
        };
        for (slot, scope, subject, predicate, qualifier) in slots {
            let (scope, subject) = if column == "subject_key" {
                (scope, to.clone())
            } else {
                (to.clone(), subject)
            };
            let existing: Option<String> = conn
                .query_row(
                    "SELECT id FROM fact_slots WHERE scope_key = ?1 AND subject_key = ?2
                       AND predicate = ?3 AND qualifier_key = ?4",
                    params![scope, subject, predicate, qualifier],
                    |r| r.get(0),
                )
                .optional()?;
            match existing {
                Some(target) => {
                    // Both ways: which name is kept must not decide whether a
                    // proposal escapes review (review point).
                    hold_back_proposals(conn, &slot, &target, &predicate)?;
                    hold_back_proposals(conn, &target, &slot, &predicate)?;
                    conn.execute(
                        "UPDATE fact_values SET slot_id = ?1 WHERE slot_id = ?2",
                        params![target, slot],
                    )?;
                    // A retried request finds its slot (review point).
                    conn.execute(
                        "UPDATE fact_events SET slot_id = ?1 WHERE slot_id = ?2",
                        params![target, slot],
                    )?;
                    store::settle_spans(conn, &target, &now)?;
                    // Above both slots' revisions: a revision read from
                    // either before the merge no longer authorizes a write
                    // (review point).
                    conn.execute(
                        "UPDATE fact_slots
                            SET revision = MAX(revision,
                                               (SELECT revision FROM fact_slots WHERE id = ?3)) + 1,
                                updated_at = ?2
                          WHERE id = ?1",
                        params![target, now, slot],
                    )?;
                    conn.execute("DELETE FROM fact_slots WHERE id = ?1", [&slot])?;
                    conn.execute(
                        "INSERT INTO fact_events (slot_id, action, outcome, actor, occurred_at, request_id)
                         VALUES (?1, 'merge', 'merged', 'graph', ?2, ?3)",
                        params![target, now, uuid::Uuid::new_v4().to_string()],
                    )?;
                }
                None => {
                    let display = if column == "subject_key" {
                        "subject"
                    } else {
                        "scope"
                    };
                    conn.execute(
                        &format!(
                            "UPDATE fact_slots SET {column} = ?1, {display} = ?2, revision = revision + 1,
                                 updated_at = ?3 WHERE id = ?4"
                        ),
                        params![to, new, now, slot],
                    )?;
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "rekey_tests.rs"]
mod tests;
