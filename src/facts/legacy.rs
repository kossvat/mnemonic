//! The old `facts` table, carried over verbatim.
//!
//! Every open imports the old rows that have no twin yet: all of them the
//! first time, afterwards only what an old binary still wrote there. Each
//! row keeps its value, time and confidence exactly, as a manual value in
//! its slot; the chain order comes from the times, so the current value is
//! the same one the old table held. Links left dangling by deletes made
//! while a trigger was missing (an old binary rebuilding `memories`) are
//! repaired in the same pass.

use anyhow::Result;
use rusqlite::{Connection, TransactionBehavior, params};
use serde::Serialize;

use super::keys;

/// Whether an open has anything to do: an old row with no twin, or a link
/// to a memory that is gone. Read-only, so the common open takes no lock.
pub fn needs_catch_up(conn: &Connection) -> Result<bool> {
    let old_table: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'facts'",
        [],
        |r| r.get(0),
    )?;
    // Rows the policy holds back whole never get a twin: alone, they are
    // no reason for an open to take the write lock.
    let new_rows = old_table > 0
        && twinless(conn)?
            .iter()
            .any(|row| verdict(conn, row) != Verdict::HeldBack);
    let dangling: bool = conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM memory_updates
                 WHERE new_id NOT IN (SELECT id FROM memories)
                    OR old_id NOT IN (SELECT id FROM memories))
             OR EXISTS (SELECT 1 FROM fact_values WHERE source_memory_id IS NOT NULL
                 AND source_memory_id NOT IN (SELECT id FROM memories))",
        [],
        |r| r.get(0),
    )?;
    Ok(new_rows || dangling)
}

/// What one catch-up did with the old rows that had no twin.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CaughtUp {
    pub imported: usize,
    /// Rows the redaction policy holds back: a credential in the value or
    /// in what names it. They stay in the old table exactly as they are,
    /// and nothing of their value enters the new ones.
    pub held_back: usize,
}

/// How the policy reads an old row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    /// Imported as it is.
    Fact,
    /// What names it is admitted, its value is not. The slot records that
    /// a value took over at that time and is withheld, so the value before
    /// it does not read as the current one.
    Withheld,
    /// What names it is not admitted: nothing of it enters the new tables.
    HeldBack,
}

/// Import what is new in the old table and repair dangling links, in one
/// IMMEDIATE transaction. Cheap when there is nothing to do.
pub fn catch_up(conn: &mut Connection) -> Result<CaughtUp> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let caught_up = import(&tx)?;
    repair(&tx)?;
    tx.commit()?;
    Ok(caught_up)
}

/// The judgement a statement made today gets, as given and as the store
/// would file it, plus the fields only an old row has. What names the row
/// is judged first, as it is written, so that nothing later shows a name
/// or an id the policy refuses.
fn verdict(conn: &Connection, row: &OldRow) -> Verdict {
    use super::admit::{check, check_resolved};
    let write = super::store::FactWrite {
        subject: &row.subject,
        predicate: &row.predicate,
        value: Some(&row.value),
        as_of: Some(&row.valid_from),
        actor: "legacy",
        source_memory_id: Some(&row.source),
        ..Default::default()
    };
    let named = super::store::FactWrite {
        value: None,
        ..write.clone()
    };
    let own = crate::redaction::state::check_identities(
        [row.id.as_str(), row.created_at.as_str()]
            .into_iter()
            .chain(row.valid_to.as_deref()),
    );
    if own.is_err() || check(&named).is_err() {
        return Verdict::HeldBack;
    }
    // A row whose keys cannot be read has no slot to be filed or withheld
    // in: a clean one is left to the import, which leaves it where it is
    // for the audit to list; any other is held back.
    let Ok(resolved) = super::store::Resolved::of(conn, &write) else {
        return match check(&write) {
            Ok(()) => Verdict::Fact,
            Err(_) => Verdict::HeldBack,
        };
    };
    if check_resolved(&named, &resolved).is_err() {
        Verdict::HeldBack
    } else if check(&write).is_ok() && check_resolved(&write, &resolved).is_ok() {
        Verdict::Fact
    } else {
        Verdict::Withheld
    }
}

/// The outcome recorded for an old row whose value is withheld.
const WITHHELD: &str = "legacy_withheld";

struct OldRow {
    id: String,
    subject: String,
    predicate: String,
    value: String,
    valid_from: String,
    valid_to: Option<String>,
    confidence: f64,
    source: String,
    created_at: String,
}

/// The old rows with no twin yet, in the order they are imported.
fn twinless(conn: &Connection) -> Result<Vec<OldRow>> {
    let mut stmt = conn.prepare(
        "SELECT f.id, f.subject, f.predicate, f.value, f.valid_from, f.valid_to, f.confidence,
                f.source_memory_id, f.created_at
           FROM facts f
          WHERE NOT EXISTS (SELECT 1 FROM fact_values v WHERE v.legacy_fact_id = f.id)
          -- At a shared instant the row the old binary closed comes before
          -- the one it left open: the last imported becomes current
          -- (review point).
          ORDER BY f.valid_from, f.created_at, f.valid_to IS NULL, f.valid_to, f.id",
    )?;
    let rows = stmt
        .query_map([], |r| {
            Ok(OldRow {
                id: r.get(0)?,
                subject: r.get(1)?,
                predicate: r.get(2)?,
                value: r.get(3)?,
                valid_from: r.get(4)?,
                valid_to: r.get(5)?,
                confidence: r.get(6)?,
                source: r.get(7)?,
                created_at: r.get(8)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

fn import(conn: &Connection) -> Result<CaughtUp> {
    let old_table: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'facts'",
        [],
        |r| r.get(0),
    )?;
    if old_table == 0 {
        return Ok(CaughtUp::default());
    }
    let rows = twinless(conn)?;
    let mut imported = 0;
    let mut held_back = 0;
    let mut touched: Vec<String> = Vec::new();
    for row in rows {
        // Nothing of a held-back row is copied, masked or rewritten.
        let verdict = verdict(conn, &row);
        if verdict == Verdict::HeldBack {
            held_back += 1;
            continue;
        }
        let withheld = verdict == Verdict::Withheld;
        // A row whose time or keys cannot be read is left in the old table,
        // where the audit reports it; it is never guessed at.
        let (Ok(valid_from_ms), Ok(subject_key), Ok(predicate)) = (
            keys::parse_time_ms(&row.valid_from),
            keys::subject_key(conn, &row.subject),
            keys::predicate_key(&row.predicate),
        ) else {
            continue;
        };
        conn.execute(
            "INSERT OR IGNORE INTO fact_slots (id, scope_key, scope, subject_key, subject,
                 predicate, predicate_label, qualifier_key, qualifier, created_at, updated_at)
             VALUES (?1, '', '', ?2, ?3, ?4, ?5, '', '', ?6, ?6)",
            params![
                uuid::Uuid::new_v4().to_string(),
                subject_key,
                row.subject,
                predicate,
                row.predicate,
                row.created_at
            ],
        )?;
        let slot: String = conn.query_row(
            "SELECT id FROM fact_slots WHERE scope_key = '' AND subject_key = ?1
               AND predicate = ?2 AND qualifier_key = ''",
            params![subject_key, predicate],
            |r| r.get(0),
        )?;
        // "manual" was the old placeholder for "no memory"; a real id is
        // kept only while that memory still exists.
        let source: Option<String> = conn
            .query_row(
                "SELECT id FROM memories WHERE id = ?1",
                [&row.source],
                |r| r.get(0),
            )
            .ok();
        let id = uuid::Uuid::new_v4().to_string();
        // A withheld value is filed as a retraction at its time: the row
        // says that the value before it ended there, and nothing else.
        let (kind, value, value_norm) = if withheld {
            ("retraction", String::new(), String::new())
        } else {
            ("value", row.value.clone(), keys::value_norm(&row.value))
        };
        conn.execute(
            "INSERT INTO fact_values (id, slot_id, kind, value, value_norm, status, trust,
                 valid_from, valid_from_ms, confidence, source_memory_id, actor,
                 legacy_fact_id, legacy_valid_to, created_at)
             VALUES (?1, ?2, ?12, ?3, ?4, 'active', 'manual', ?5, ?6, ?7, ?8, 'legacy',
                     ?9, ?10, ?11)",
            params![
                id,
                slot,
                value,
                value_norm,
                row.valid_from,
                valid_from_ms,
                row.confidence.clamp(0.0, 1.0),
                source,
                row.id,
                row.valid_to,
                row.created_at,
                kind
            ],
        )?;
        conn.execute(
            "UPDATE fact_slots SET revision = revision + 1 WHERE id = ?1",
            [&slot],
        )?;
        conn.execute(
            "INSERT OR IGNORE INTO fact_events (slot_id, value_id, action, outcome, actor,
                 occurred_at, request_id)
             VALUES (?1, ?2, 'legacy_import', ?5, 'legacy', ?3, ?4)",
            params![
                slot,
                id,
                chrono::Utc::now().to_rfc3339(),
                format!("legacy:{}", row.id),
                if withheld { WITHHELD } else { "legacy_import" }
            ],
        )?;
        if !touched.contains(&slot) {
            touched.push(slot);
        }
        if withheld {
            held_back += 1;
        } else {
            imported += 1;
        }
    }
    // A row an old binary wrote can land inside a span a newer process
    // reconfirmed: the reconfirmed value comes back after it (review point).
    let now = chrono::Utc::now().to_rfc3339();
    for slot in touched {
        super::store::settle_spans(conn, &slot, &now)?;
    }
    Ok(CaughtUp {
        imported,
        held_back,
    })
}

/// Links to memories that no longer exist, left by a delete that ran while
/// a trigger was missing.
fn repair(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "DELETE FROM memory_updates
          WHERE new_id NOT IN (SELECT id FROM memories)
             OR old_id NOT IN (SELECT id FROM memories);
         DELETE FROM memory_reaffirmed WHERE memory_id NOT IN (SELECT id FROM memories);
         DELETE FROM fact_values
          WHERE trust = 'provisional' AND source_memory_id IS NOT NULL
            AND source_memory_id NOT IN (SELECT id FROM memories);
         UPDATE fact_values
            SET source_memory_id = NULL, evidence = NULL,
                source_forgotten_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
          WHERE source_memory_id IS NOT NULL
            AND source_memory_id NOT IN (SELECT id FROM memories);",
    )?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Audit {
    pub legacy_rows: usize,
    pub imported: usize,
    /// Old rows the import could not read (bad time or empty keys).
    pub not_imported: Vec<String>,
    /// Old rows the redaction policy holds back (SENSITIVE_CONTENT): only
    /// counted, since an id can be what the policy refused. A row whose
    /// names are admitted is in its slot as a withheld value (the slot has
    /// no current value from that time on); any other is in the old table
    /// only.
    pub held_back: usize,
    pub slots: usize,
    pub current: usize,
    /// Values whose source memory is gone but still linked.
    pub dangling_sources: usize,
    /// Old rows whose current value differs between the two tables.
    pub mismatched_current: Vec<String>,
}

impl Audit {
    pub fn clean(&self) -> bool {
        self.not_imported.is_empty()
            && self.held_back == 0
            && self.dangling_sources == 0
            && self.mismatched_current.is_empty()
    }
}

/// Compare the old table with the new chains, read-only.
pub fn audit(conn: &Connection) -> Result<Audit> {
    let count = |sql: &str| -> Result<usize> {
        Ok(conn.query_row(sql, [], |r| r.get::<_, i64>(0))? as usize)
    };
    let old_table =
        count("SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'facts'")? > 0;
    let legacy_rows = if old_table {
        count("SELECT COUNT(*) FROM facts")?
    } else {
        0
    };
    let imported = count(
        "SELECT COUNT(*) FROM fact_values WHERE legacy_fact_id IS NOT NULL AND kind = 'value'",
    )?;
    let mut not_imported = Vec::new();
    // Values withheld, filed as the end of the value before them.
    let mut held_back = count(
        "SELECT COUNT(*) FROM fact_values v JOIN fact_events e ON e.value_id = v.id
          WHERE v.legacy_fact_id IS NOT NULL AND e.outcome = 'legacy_withheld'",
    )?;
    let mut mismatched_current = Vec::new();
    if old_table {
        for row in twinless(conn)? {
            if verdict(conn, &row) == Verdict::HeldBack {
                held_back += 1;
            } else {
                not_imported.push(row.id);
            }
        }
        // The old current row of each chain must be the new chain's current.
        let mut stmt = conn.prepare(
            "SELECT f.id, v.slot_id FROM facts f JOIN fact_values v ON v.legacy_fact_id = f.id
              WHERE f.valid_to IS NULL",
        )?;
        let pairs: Vec<(String, String)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        for (old_id, slot) in pairs {
            // Among the imported rows of its slot, the old current row must
            // be the last in chain order (two old chains folded into one
            // slot by the new keys show up here).
            let last_legacy: Option<String> = conn.query_row(
                "SELECT legacy_fact_id FROM fact_values
                  WHERE slot_id = ?1 AND status = 'active' AND legacy_fact_id IS NOT NULL
                  ORDER BY valid_from_ms DESC, seq DESC LIMIT 1",
                [&slot],
                |r| r.get(0),
            )?;
            if last_legacy.as_deref() != Some(old_id.as_str()) {
                mismatched_current.push(old_id);
            }
        }
    }
    let slots = count("SELECT COUNT(*) FROM fact_slots")?;
    let mut current = 0;
    let mut stmt = conn.prepare("SELECT id FROM fact_slots")?;
    let slot_ids: Vec<String> = stmt
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    for slot in slot_ids {
        if super::store::slot_view(conn, &slot)?.current.is_some() {
            current += 1;
        }
    }
    let dangling_sources = count(
        "SELECT COUNT(*) FROM fact_values
          WHERE source_memory_id IS NOT NULL AND source_memory_id NOT IN (SELECT id FROM memories)",
    )?;
    Ok(Audit {
        legacy_rows,
        imported,
        not_imported,
        held_back,
        slots,
        current,
        dangling_sources,
        mismatched_current,
    })
}

#[cfg(test)]
#[path = "legacy_tests.rs"]
mod tests;
