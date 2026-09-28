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

/// Where the rows of a slot are about to be filed: the keys they will be
/// found by and the names they will be shown under.
struct Destination {
    scope_key: String,
    subject_key: String,
    predicate: String,
    qualifier_key: String,
    /// Scope, subject, predicate and qualifier as they will be shown.
    shown: [String; 4],
}

/// The rows of `slot` are about to be filed at `to`. Each of them is judged
/// as the statement it will be there, by the judgement a statement made
/// today gets: what a move makes of a fact must be something that could
/// have been stated. So a project renamed to `token` cannot show the
/// values of its facts as credentials, and a subject renamed to `Bearer`
/// cannot make one of a predicate. A value stored before the policy that
/// the policy refuses holds its facts where they are until it is erased.
fn admit_move(conn: &Connection, slot: &str, to: &Destination) -> Result<()> {
    use super::admit::{check, check_resolved};
    let mut stmt = conn.prepare(
        "SELECT value FROM fact_values
          WHERE slot_id = ?1 AND kind = 'value' AND status <> 'rejected'",
    )?;
    let values: Vec<String> = stmt
        .query_map([slot], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    let [scope, subject, predicate, qualifier] = &to.shown;
    let resolved = store::Resolved {
        scope_key: to.scope_key.clone(),
        scope: scope.clone(),
        subject_key: to.subject_key.clone(),
        subject_target: None,
        predicate: to.predicate.clone(),
        qualifier_key: to.qualifier_key.clone(),
        labels: Vec::new(),
    };
    // The slot by its names alone, then with each value it holds.
    for value in std::iter::once(None).chain(values.iter().map(|v| Some(v.as_str()))) {
        let statement = store::FactWrite {
            project: Some(scope.as_str()).filter(|s| !s.is_empty()),
            subject,
            predicate,
            qualifier: Some(qualifier.as_str()).filter(|q| !q.is_empty()),
            value,
            actor: "graph",
            ..Default::default()
        };
        check(&statement)
            .and_then(|()| check_resolved(&statement, &resolved))
            .map_err(|refused| crate::redaction::state::refused("rekey", refused.code))?;
    }
    Ok(())
}

/// Move the facts about `old` (as a subject, and as a project) to `new`.
/// A slot that already exists under the new name takes the moved values:
/// the chain re-derives by time, so the newest value stays current.
///
/// All of it or none: the names are judged first, and every move is judged
/// where it happens, against the store as the moves before it left it (a
/// name that is both a subject and a project moves a slot twice, and only
/// the second move shows where its values land). A refusal undoes the
/// moves already made, whatever transaction the caller runs.
pub fn rekey_in_tx(conn: &Connection, old: &str, new: &str) -> Result<()> {
    crate::graph::canonical::admit_renamed("rekey", [old, new])?;
    conn.execute_batch("SAVEPOINT fact_rekey")?;
    let moved = move_facts(conn, old, new);
    conn.execute_batch(match moved {
        Ok(()) => "RELEASE fact_rekey",
        Err(_) => "ROLLBACK TO fact_rekey; RELEASE fact_rekey",
    })?;
    moved
}

fn move_facts(conn: &Connection, old: &str, new: &str) -> Result<()> {
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
            let names = |r: &rusqlite::Row<'_>| -> rusqlite::Result<[String; 4]> {
                Ok([r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?])
            };
            // The slot the rows would join, with the names it keeps.
            let existing: Option<(String, [String; 4])> = conn
                .query_row(
                    "SELECT id, scope, subject, predicate_label, qualifier FROM fact_slots
                      WHERE scope_key = ?1 AND subject_key = ?2 AND predicate = ?3
                        AND qualifier_key = ?4",
                    params![scope, subject, predicate, qualifier],
                    |r| Ok((r.get(0)?, names(r)?)),
                )
                .optional()?;
            let shown = match &existing {
                Some((_, kept)) => kept.clone(),
                // Its own names, with the one that changes.
                None => {
                    let mut own = conn.query_row(
                        "SELECT id, scope, subject, predicate_label, qualifier FROM fact_slots
                          WHERE id = ?1",
                        [&slot],
                        names,
                    )?;
                    own[if column == "subject_key" { 1 } else { 0 }] = new.to_owned();
                    own
                }
            };
            admit_move(
                conn,
                &slot,
                &Destination {
                    scope_key: scope.clone(),
                    subject_key: subject.clone(),
                    predicate: predicate.clone(),
                    qualifier_key: qualifier.clone(),
                    shown,
                },
            )?;
            match existing.map(|(target, _)| target) {
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
