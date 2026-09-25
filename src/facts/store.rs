//! Writing and reading fact chains. Every write is one IMMEDIATE
//! transaction around `rule::decide`; the current value of a slot is
//! derived from its active rows, never stored.

use anyhow::{Result, bail, ensure};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::Serialize;

use super::keys;
use super::rule::{self, Candidate, Decision, Kind, Row, Trust};
#[cfg(test)]
pub use super::view::{current_all, value_count};
pub use super::view::{slot_view, views};
use crate::storage::Storage;

const MAX_SUBJECT: usize = 120;
const MAX_VALUE: usize = 2000;
const MAX_AGENT: usize = 40;
const MAX_REQUEST_ID: usize = 128;

/// One statement about a slot. `value: None` retracts it.
#[derive(Debug, Clone, Default)]
pub struct FactWrite<'a> {
    pub project: Option<&'a str>,
    pub subject: &'a str,
    pub predicate: &'a str,
    pub qualifier: Option<&'a str>,
    pub value: Option<&'a str>,
    pub as_of: Option<&'a str>,
    pub trust: Option<Trust>,
    pub actor: &'a str,
    pub agent: Option<&'a str>,
    pub evidence: Option<&'a str>,
    pub source_memory_id: Option<&'a str>,
    pub request_id: Option<&'a str>,
    pub expected_revision: Option<i64>,
    pub confidence: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ValueView {
    pub id: String,
    /// `None` for a retraction.
    pub value: Option<String>,
    pub valid_from: String,
    /// When the next value took over; `None` while current.
    pub valid_to: Option<String>,
    pub trust: String,
    pub source_memory_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SlotView {
    pub slot_id: String,
    pub project: String,
    pub subject: String,
    pub predicate: String,
    pub qualifier: String,
    pub revision: i64,
    pub current: Option<ValueView>,
    /// Every active value, newest first (the current one included).
    pub history: Vec<ValueView>,
    /// Provisional values waiting for review: hidden, only counted.
    pub pending: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Outcome {
    /// create, update, retract, reconfirm, history or pending_review.
    pub outcome: String,
    /// The same request was applied before: nothing was written now.
    pub replayed: bool,
    pub value_id: Option<String>,
    /// The value this write replaced as current, if it did.
    pub replaced: Option<ValueView>,
    /// When the statement took effect as the store resolved it (an implicit
    /// now lands after every row); `None` on a replay.
    pub effective_at: Option<String>,
    pub fact: SlotView,
}

/// A project's scope key; `''` for no project.
pub fn scope_key(conn: &Connection, project: Option<&str>) -> Result<(String, String)> {
    match project.map(str::trim).filter(|p| !p.is_empty()) {
        None => Ok((String::new(), String::new())),
        Some(name) => {
            let name = crate::followups::canonical_project(conn, name)?;
            Ok((crate::followups::project_key(&name), name))
        }
    }
}

fn now_text(now: DateTime<Utc>) -> String {
    now.to_rfc3339()
}

/// The slot for these keys, created on first use.
fn slot_id(
    conn: &Connection,
    (scope_key, scope): (&str, &str),
    (subject_key, subject): (&str, &str),
    (predicate, predicate_label): (&str, &str),
    (qualifier_key, qualifier): (&str, &str),
    now: &str,
) -> Result<String> {
    conn.execute(
        "INSERT OR IGNORE INTO fact_slots (id, scope_key, scope, subject_key, subject, predicate,
             predicate_label, qualifier_key, qualifier, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10)",
        params![
            uuid::Uuid::new_v4().to_string(),
            scope_key,
            scope,
            subject_key,
            subject,
            predicate,
            predicate_label,
            qualifier_key,
            qualifier,
            now
        ],
    )?;
    Ok(conn.query_row(
        "SELECT id FROM fact_slots
          WHERE scope_key = ?1 AND subject_key = ?2 AND predicate = ?3 AND qualifier_key = ?4",
        params![scope_key, subject_key, predicate, qualifier_key],
        |r| r.get(0),
    )?)
}

/// The slot's active rows, in chain order.
pub(super) fn chain(conn: &Connection, slot: &str) -> Result<Vec<(Row, String)>> {
    let mut stmt = conn.prepare(
        "SELECT seq, kind, value_norm, valid_from_ms, trust, id,
                COALESCE(asserted_ms, valid_from_ms) FROM fact_values
          WHERE slot_id = ?1 AND status = 'active' ORDER BY valid_from_ms, seq",
    )?;
    let rows = stmt.query_map([slot], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, i64>(3)?,
            r.get::<_, String>(4)?,
            r.get::<_, String>(5)?,
            r.get::<_, i64>(6)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (seq, kind, value_norm, valid_from_ms, trust, id, asserted_ms) = row?;
        out.push((
            Row {
                seq,
                kind: if kind == "retraction" {
                    Kind::Retraction
                } else {
                    Kind::Value
                },
                value_norm,
                valid_from_ms,
                asserted_ms: asserted_ms.max(valid_from_ms),
                trust: Trust::parse(&trust).unwrap_or(Trust::Manual),
            },
            id,
        ));
    }
    Ok(out)
}

fn kind_text(kind: Kind) -> &'static str {
    match kind {
        Kind::Value => "value",
        Kind::Retraction => "retraction",
    }
}

/// The value of row `seq` comes back at the last time it was said: a copy
/// of it starts then, and the row itself no longer claims that span.
fn reassert(conn: &Connection, rows: &[(Row, String)], seq: i64, now: &str) -> Result<()> {
    let Some((row, _)) = rows.iter().find(|(row, _)| row.seq == seq) else {
        bail!("no row {seq} to reassert");
    };
    // A row that starts at that very time was said after the
    // reconfirmation (which only matches a value nothing follows at its
    // time), so it wins that instant: a copy would outrank it by seq alone
    // (review point).
    let taken = rows
        .iter()
        .any(|(other, _)| other.seq != seq && other.valid_from_ms == row.asserted_ms);
    if !taken {
        insert_copy(conn, seq, row.asserted_ms, now)?;
    }
    conn.execute(
        "UPDATE fact_values SET asserted_ms = NULL WHERE seq = ?1",
        [seq],
    )?;
    Ok(())
}

fn insert_copy(conn: &Connection, seq: i64, at_ms: i64, now: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO fact_values (id, slot_id, kind, value, value_norm, status, trust,
             valid_from, valid_from_ms, confidence, source_memory_id, evidence, actor, agent,
             extractor, created_at)
         SELECT ?2, slot_id, kind, value, value_norm, 'active', trust, ?3, ?4, confidence,
                source_memory_id, evidence, 'reconfirm', agent, extractor, ?5
           FROM fact_values WHERE seq = ?1",
        params![
            seq,
            uuid::Uuid::new_v4().to_string(),
            keys::format_ms(at_ms),
            at_ms,
            now
        ],
    )?;
    Ok(())
}

/// After rows join a chain in bulk (a slot merge), a value said to hold
/// through a time after the next row began comes back at that time, as it
/// does when a single backfill interrupts it (review point).
pub(super) fn settle_spans(conn: &Connection, slot: &str, now: &str) -> Result<()> {
    loop {
        let rows = chain(conn, slot)?;
        let interrupted = rows.iter().enumerate().find_map(|(i, (row, _))| {
            if row.asserted_ms <= row.valid_from_ms {
                return None;
            }
            let (last, _) = rows[i + 1..]
                .iter()
                .take_while(|(next, _)| next.valid_from_ms < row.asserted_ms)
                .last()?;
            let same = last.kind == row.kind && last.value_norm == row.value_norm;
            Some((row.seq, last.seq, same))
        });
        match interrupted {
            None => return Ok(()),
            // The same value holds there anyway: that row carries the span.
            Some((seq, last, true)) => {
                conn.execute(
                    "UPDATE fact_values
                        SET asserted_ms = MAX(COALESCE(asserted_ms, valid_from_ms),
                                              (SELECT asserted_ms FROM fact_values WHERE seq = ?1))
                      WHERE seq = ?2",
                    params![seq, last],
                )?;
                conn.execute(
                    "UPDATE fact_values SET asserted_ms = NULL WHERE seq = ?1",
                    [seq],
                )?;
            }
            Some((seq, _, false)) => reassert(conn, &rows, seq, now)?,
        }
    }
}

/// Apply one statement in the caller's IMMEDIATE transaction.
pub fn apply_in(conn: &Connection, write: &FactWrite<'_>, now: DateTime<Utc>) -> Result<Outcome> {
    let now_ms = now.timestamp_millis();
    if let Some(request_id) = write.request_id {
        ensure!(
            request_id.len() <= MAX_REQUEST_ID,
            "request_id is at most {MAX_REQUEST_ID} bytes"
        );
        let earlier: Option<(String, String, Option<String>)> = conn
            .query_row(
                "SELECT slot_id, outcome, value_id FROM fact_events WHERE request_id = ?1",
                [request_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        if let Some((slot, outcome, value_id)) = earlier {
            return Ok(Outcome {
                outcome,
                replayed: true,
                value_id,
                replaced: None,
                effective_at: None,
                fact: slot_view(conn, &slot)?,
            });
        }
    }
    ensure!(
        write.subject.chars().count() <= MAX_SUBJECT,
        "a subject is at most {MAX_SUBJECT} characters"
    );
    if let Some(value) = write.value {
        ensure!(
            !value.trim().is_empty(),
            "a fact needs a value (or retract it)"
        );
        ensure!(
            value.chars().count() <= MAX_VALUE,
            "a value is at most {MAX_VALUE} characters"
        );
    }
    if let Some(agent) = write.agent {
        ensure!(
            agent.chars().count() <= MAX_AGENT,
            "an agent name is at most {MAX_AGENT} characters"
        );
    }
    if let Some(source) = write.source_memory_id {
        // Checked in the write's own transaction: a memory forgotten just
        // before would otherwise leave the value pointing at nothing.
        let live: bool = conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM memories WHERE id = ?1 AND superseded_by IS NULL)",
            [source],
            |r| r.get(0),
        )?;
        ensure!(live, "no live memory {source}");
    }
    let confidence = write.confidence.unwrap_or(1.0);
    ensure!(
        (0.0..=1.0).contains(&confidence),
        "confidence is between 0 and 1"
    );
    let trust = write.trust.unwrap_or(Trust::Declared);
    let now_s = now_text(now);

    let (scope_key, scope) = scope_key(conn, write.project)?;
    let subject_key = keys::subject_key(conn, write.subject)?;
    let predicate = keys::predicate_key(write.predicate)?;
    let qualifier = write.qualifier.unwrap_or("").trim();
    let qualifier_key = keys::qualifier_key(qualifier)?;
    let slot = slot_id(
        conn,
        (&scope_key, &scope),
        (&subject_key, write.subject.trim()),
        (&predicate, write.predicate.trim()),
        (&qualifier_key, qualifier),
        &now_s,
    )?;
    let revision: i64 = conn.query_row(
        "SELECT revision FROM fact_slots WHERE id = ?1",
        [&slot],
        |r| r.get(0),
    )?;
    if let Some(expected) = write.expected_revision
        && expected != revision
    {
        let current = slot_view(conn, &slot)?.current.and_then(|v| v.value);
        bail!(
            "revision mismatch: the fact is at revision {revision}, not {expected}; current value {}",
            current.as_deref().unwrap_or("none")
        );
    }

    let rows = chain(conn, &slot)?;
    let valid_from_ms = match write.as_of {
        Some(as_of) => keys::parse_time_ms(as_of)?,
        // An implicit now is always the newest: commit order is chain order
        // across every process writing this store.
        // Above any time a reconfirmation claimed too (review point).
        None => rows
            .iter()
            .map(|(row, _)| row.valid_from_ms.max(row.asserted_ms) + 1)
            .max()
            .map_or(now_ms, |next| next.max(now_ms)),
    };
    let (kind, value, value_norm) = match write.value {
        Some(value) => (Kind::Value, value.to_owned(), keys::value_norm(value)),
        None => (Kind::Retraction, String::new(), String::new()),
    };
    let candidate = Candidate {
        kind,
        value_norm: value_norm.clone(),
        valid_from_ms,
        trust,
        class: keys::predicate_class(&predicate),
    };
    let chain_rows: Vec<Row> = rows.iter().map(|(row, _)| row.clone()).collect();
    let decision = rule::decide(&candidate, &chain_rows, now_ms);
    let previous = rule::current(&chain_rows).map(|row| row.seq);

    let waiting: Option<String> = if decision == Decision::PendingReview {
        conn.query_row(
            "SELECT id FROM fact_values WHERE slot_id = ?1 AND status = 'pending_review'
               AND kind = ?2 AND value_norm = ?3",
            params![slot, kind_text(kind), value_norm],
            |r| r.get(0),
        )
        .optional()?
    } else {
        None
    };
    let (outcome, value_id) = match decision {
        Decision::Reject(why) => bail!(why),
        Decision::Reconfirm(seq) => {
            // An owner or agent stating a proposed value makes it theirs: a
            // later proposal or the proposal's source going away must not
            // take it back (review point).
            let adopted = trust.authoritative()
                && rows
                    .iter()
                    .any(|(row, _)| row.seq == seq && row.trust == Trust::Provisional);
            conn.execute(
                "UPDATE fact_values SET reconfirm_count = reconfirm_count + 1, reconfirmed_at = ?2,
                        asserted_ms = MAX(COALESCE(asserted_ms, valid_from_ms), ?3)
                  WHERE seq = ?1",
                params![seq, now_s, valid_from_ms],
            )?;
            if adopted {
                conn.execute(
                    "UPDATE fact_values SET trust = ?2,
                            source_memory_id = COALESCE(?3, source_memory_id)
                      WHERE seq = ?1",
                    params![seq, trust.as_str(), write.source_memory_id],
                )?;
                conn.execute(
                    "UPDATE fact_slots SET revision = revision + 1, updated_at = ?2 WHERE id = ?1",
                    params![slot, now_s],
                )?;
            }
            let id = rows
                .iter()
                .find(|(row, _)| row.seq == seq)
                .map(|(_, id)| id.clone());
            ("reconfirm", id)
        }
        // The same proposal again waits once.
        Decision::PendingReview if waiting.is_some() => ("pending_review", waiting),
        decision => {
            let id = uuid::Uuid::new_v4().to_string();
            let (outcome, status) = match decision {
                Decision::Create => ("create", "active"),
                Decision::Update => ("update", "active"),
                Decision::Retract => ("retract", "active"),
                Decision::History | Decision::Interrupt(_) => ("history", "active"),
                _ => ("pending_review", "pending_review"),
            };
            if let Decision::Interrupt(seq) = decision {
                reassert(conn, &rows, seq, &now_s)?;
            }
            conn.execute(
                "INSERT INTO fact_values (id, slot_id, kind, value, value_norm, status, trust,
                     valid_from, valid_from_ms, confidence, source_memory_id, evidence, actor,
                     agent, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
                params![
                    id,
                    slot,
                    kind_text(kind),
                    value,
                    value_norm,
                    status,
                    trust.as_str(),
                    keys::format_ms(valid_from_ms),
                    valid_from_ms,
                    confidence,
                    write.source_memory_id,
                    write
                        .evidence
                        .map(|e| e.chars().take(500).collect::<String>()),
                    write.actor,
                    write.agent,
                    now_s
                ],
            )?;
            conn.execute(
                "UPDATE fact_slots SET revision = revision + 1, updated_at = ?2 WHERE id = ?1",
                params![slot, now_s],
            )?;
            (outcome, Some(id))
        }
    };
    // Without an explicit id a retry is not a replay; the rule makes it a
    // reconfirmation instead (review point: an id derived from the source
    // collided on the second write from the same memory).
    let request_id = write
        .request_id
        .map_or_else(|| uuid::Uuid::new_v4().to_string(), str::to_owned);
    let fact = slot_view(conn, &slot)?;
    conn.execute(
        "INSERT INTO fact_events (slot_id, value_id, action, outcome, revision_after, actor,
             agent, evidence_memory_id, occurred_at, request_id)
         VALUES (?1, ?2, ?3, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            slot,
            value_id,
            outcome,
            fact.revision,
            write.actor,
            write.agent,
            write.source_memory_id,
            now_s,
            request_id
        ],
    )?;
    let replaced = match (outcome, previous) {
        ("update" | "retract", Some(seq)) => {
            let id = rows
                .iter()
                .find(|(row, _)| row.seq == seq)
                .map(|(_, id)| id.as_str());
            fact.history
                .iter()
                .find(|v| Some(v.id.as_str()) == id)
                .cloned()
        }
        _ => None,
    };
    Ok(Outcome {
        outcome: outcome.to_owned(),
        replayed: false,
        value_id,
        replaced,
        effective_at: Some(keys::format_ms(valid_from_ms)),
        fact,
    })
}

/// What `write` would do, without doing it: applied inside a savepoint
/// that is rolled back. Must run in the caller's write transaction, so the
/// answer still holds when the caller then applies it.
pub fn peek_in(conn: &Connection, write: &FactWrite<'_>, now: DateTime<Utc>) -> Result<Outcome> {
    conn.execute_batch("SAVEPOINT fact_peek")?;
    let outcome = apply_in(conn, write, now);
    conn.execute_batch("ROLLBACK TO fact_peek; RELEASE fact_peek")?;
    outcome
}

/// Apply one statement in its own IMMEDIATE transaction.
pub fn apply(storage: &Storage, write: &FactWrite<'_>) -> Result<Outcome> {
    let mut conn = storage
        .conn
        .lock()
        .map_err(|e| anyhow::anyhow!("lock: {e}"))?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let outcome = apply_in(&tx, write, Utc::now())?;
    tx.commit()?;
    Ok(outcome)
}

/// A source memory id, checked: `None` passes, an id must name a live
/// memory (a value never points at nothing).
pub fn live_source(storage: &Storage, source: Option<&str>) -> Result<Option<String>> {
    let Some(source) = source.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    let conn = storage
        .conn
        .lock()
        .map_err(|e| anyhow::anyhow!("lock: {e}"))?;
    let found: Option<String> = conn
        .query_row(
            "SELECT id FROM memories WHERE id = ?1 AND superseded_by IS NULL",
            [source],
            |r| r.get(0),
        )
        .optional()?;
    match found {
        Some(id) => Ok(Some(id)),
        None => bail!("no live memory {source}"),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Erased {
    pub id_short: String,
    pub subject: String,
    pub predicate: String,
}

/// Erase one stored value for good, found by the start of its id (it must
/// match exactly one). The chain re-derives around the gap; the event
/// records only that a value was erased, never which.
pub fn forget_value(storage: &Storage, id_prefix: &str) -> Result<Erased> {
    let id_prefix = id_prefix.trim();
    ensure!(
        id_prefix.len() >= 4,
        "give at least 4 characters of the value id"
    );
    let mut conn = storage
        .conn
        .lock()
        .map_err(|e| anyhow::anyhow!("lock: {e}"))?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let matches: Vec<(String, String)> = {
        let mut stmt = tx.prepare("SELECT id, slot_id FROM fact_values WHERE id LIKE ?1 || '%'")?;
        stmt.query_map([id_prefix], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?
    };
    let [(id, slot)] = matches.as_slice() else {
        bail!(
            "{} values start with {id_prefix}; give more of the id",
            matches.len()
        );
    };
    let (subject, predicate): (String, String) = tx.query_row(
        "SELECT subject, predicate_label FROM fact_slots WHERE id = ?1",
        [slot],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let now = now_text(Utc::now());
    // An imported value is erased from the old table too, or the next open
    // would import it again (review point).
    let legacy: Option<String> = tx.query_row(
        "SELECT legacy_fact_id FROM fact_values WHERE id = ?1",
        [id],
        |r| r.get(0),
    )?;
    if let Some(legacy) = legacy {
        tx.execute("DELETE FROM facts WHERE id = ?1", [legacy])?;
    }
    // The memories written only to record statements of this value repeat
    // it: they go too, unless another stored value still cites them as its
    // source or as evidence of a statement (review points).
    let recorded: Vec<String> = {
        let mut stmt = tx.prepare(
            "SELECT source_memory_id FROM fact_values
              WHERE id = ?1 AND source_memory_id IS NOT NULL
             UNION
             SELECT evidence_memory_id FROM fact_events
              WHERE value_id = ?1 AND evidence_memory_id IS NOT NULL",
        )?;
        stmt.query_map([id], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?
    };
    tx.execute("DELETE FROM fact_values WHERE id = ?1", [id])?;
    for memory in recorded {
        let erase: bool = tx.query_row(
            "SELECT EXISTS (SELECT 1 FROM memories
                             WHERE id = ?1 AND json_extract(metadata, '$.fact') IS NOT NULL)
                AND NOT EXISTS (SELECT 1 FROM fact_values WHERE source_memory_id = ?1)
                AND NOT EXISTS (SELECT 1 FROM fact_events e JOIN fact_values v ON v.id = e.value_id
                                 WHERE e.evidence_memory_id = ?1)",
            [&memory],
            |r| r.get(0),
        )?;
        if erase {
            tx.execute(
                "DELETE FROM memory_entities WHERE memory_id = ?1",
                [&memory],
            )?;
            tx.execute("DELETE FROM edges WHERE memory_id = ?1", [&memory])?;
            tx.execute("DELETE FROM memories WHERE id = ?1", [&memory])?;
        }
    }
    tx.execute(
        "UPDATE fact_slots SET revision = revision + 1, updated_at = ?2 WHERE id = ?1",
        params![slot, now],
    )?;
    tx.execute(
        "INSERT INTO fact_events (slot_id, action, outcome, actor, occurred_at, request_id)
         VALUES (?1, 'forget_value', 'erased', 'cli', ?2, ?3)",
        params![slot, now, uuid::Uuid::new_v4().to_string()],
    )?;
    tx.commit()?;
    Ok(Erased {
        id_short: id.chars().take(8).collect(),
        subject,
        predicate,
    })
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
