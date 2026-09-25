//! An agent states a fact: the value goes into its slot's chain, and a
//! memory records the statement, both in one transaction.
//!
//! The memory is what search and context find. It is dated when the value
//! took effect and linked as an update of any plain memory stating the old
//! value, so recall marks that one as replaced too. It goes to no output
//! sink and is not queued for extraction: forgetting the value erases it,
//! and a file export or a derived copy (the extraction cache, graph
//! entities) could not be erased. Saying the current value again writes no memory, only a
//! reconfirmation, unless the agent adds a note or a plain memory of
//! another value has become the newest word since.

use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use serde_json::json;

use super::keys;
use super::store::{self, FactWrite, Outcome};
use crate::embedding::Embedder;
use crate::event::{EventSource, MemoryEntry, MemoryType};
use crate::storage::Storage;
use crate::updates::plan::{self, LinkPlan, Mode};
use crate::updates::scan::scan;
use crate::updates::store::Link;
use crate::updates::verdict::{Verdict, verdict};

pub struct Declared {
    pub outcome: Outcome,
    /// The memory that records the statement, when one was written.
    pub memory: Option<MemoryEntry>,
    pub link: Option<Link>,
}

/// The memory a statement is recorded as.
fn source_memory(write: &FactWrite<'_>, note: Option<&str>) -> MemoryEntry {
    let mut about = format!("{} {}", write.subject.trim(), write.predicate.trim());
    if let Some(qualifier) = write.qualifier.map(str::trim).filter(|q| !q.is_empty()) {
        about.push_str(&format!(" ({qualifier})"));
    }
    let (title, mut content) = match write.value {
        Some(value) => (format!("{about}: {value}"), format!("{about} is {value}")),
        None => (
            format!("{about}: retracted"),
            format!("{about} no longer has a value"),
        ),
    };
    // The project stays out of the text: the scanner reads an extra word
    // as a different context, and the statement would then no longer
    // replace a plain memory of the old value.
    content.push('.');
    if let Some(note) = note.map(str::trim).filter(|n| !n.is_empty()) {
        content.push_str("\n\n");
        content.push_str(note);
    }
    let mut entry = MemoryEntry::new(title, content, MemoryType::Note, EventSource::Socket);
    entry.importance = 0.8;
    entry.tags = vec!["fact".into()];
    entry.metadata = json!({
        "fact": {
            "subject": write.subject.trim(),
            "predicate": write.predicate.trim(),
            "qualifier": write.qualifier.map(str::trim).unwrap_or(""),
            "value": write.value,
        },
    });
    entry
}

/// A reconfirmation says the matched value again: its memory keeps that
/// time, so a memory replayed later from before it cannot outrank it. The
/// matched row, not the current one: a backdated reconfirmation matches an
/// older value (review points).
fn reaffirm_matched(
    conn: &rusqlite::Connection,
    outcome: &Outcome,
    entry: &MemoryEntry,
) -> Result<()> {
    if outcome.outcome != "reconfirm" || outcome.replayed {
        return Ok(());
    }
    if let Some(source) = matched_source(outcome) {
        crate::updates::store::reaffirm(conn, source, &entry.timestamp.to_rfc3339())?;
    }
    Ok(())
}

/// The value `outcome` matched or wrote, when a memory recorded it.
fn matched_source(outcome: &Outcome) -> Option<&str> {
    outcome
        .value_id
        .as_deref()
        .and_then(|id| outcome.fact.history.iter().find(|v| v.id == id))
        .and_then(|v| v.source_memory_id.as_deref())
}

/// A plan to link into the chain of this slot's own value: the value this
/// statement replaced, or the one it reconfirmed. Decided against the
/// chain's current end, which a plain memory of another value may have
/// become since the plan was made (review points).
fn own_chain(
    conn: &rusqlite::Connection,
    preview: &Outcome,
    entry: &MemoryEntry,
    threshold: f32,
) -> Result<Option<LinkPlan>> {
    if preview.replayed {
        return Ok(None);
    }
    let source = match preview.outcome.as_str() {
        "reconfirm" => matched_source(preview),
        "update" => preview
            .replaced
            .as_ref()
            .and_then(|v| v.source_memory_id.as_deref()),
        _ => None,
    };
    let Some(head) = source
        .map(|source| crate::updates::store::head(conn, source))
        .transpose()?
        .flatten()
    else {
        return Ok(None);
    };
    let text: Option<String> = conn
        .query_row(
            "SELECT title || char(10) || content FROM memories WHERE id = ?1",
            [&head],
            |r| r.get(0),
        )
        .optional()?;
    let Some(text) = text else {
        return Ok(None);
    };
    Ok(
        match verdict(
            &scan(&plan::text_of(&entry.title, &entry.content)),
            &scan(&text),
        ) {
            Verdict::Update { class, was, now } => Some(LinkPlan {
                target: head,
                new_is_newer: true,
                class,
                was,
                now,
                similarity: threshold,
            }),
            _ => None,
        },
    )
}

/// The chain of `memory` ends in a statement of another fact slot only: a
/// variant (another qualifier, subject or project) the free-text gate could
/// not tell apart, not an older value of this one (review point).
fn states_another_slot(conn: &rusqlite::Connection, memory: &str, slot: &str) -> Result<bool> {
    let Some(head) = crate::updates::store::head(conn, memory)? else {
        return Ok(false);
    };
    Ok(conn.query_row(
        "WITH stated AS (
             SELECT slot_id FROM fact_values WHERE source_memory_id = ?1
             UNION
             SELECT slot_id FROM fact_events WHERE evidence_memory_id = ?1)
         SELECT EXISTS (SELECT 1 FROM stated WHERE slot_id <> ?2)
            AND NOT EXISTS (SELECT 1 FROM stated WHERE slot_id = ?2)",
        params![head, slot],
        |r| r.get(0),
    )?)
}

/// A statement planned outside the write lock.
struct Prepared {
    entry: MemoryEntry,
    embedding: Option<crate::embedding::Embedding>,
    planned: Option<LinkPlan>,
}

/// State `write` (trust declared unless set) and record it as a memory.
pub fn declare(
    storage: &Storage,
    embedder: &dyn Embedder,
    threshold: f32,
    write: &FactWrite<'_>,
    note: Option<&str>,
) -> Result<Declared> {
    let prepared = prepare(storage, embedder, threshold, write, note)?;
    commit(storage, prepared, threshold, write, note)
}

fn prepare(
    storage: &Storage,
    embedder: &dyn Embedder,
    threshold: f32,
    write: &FactWrite<'_>,
    note: Option<&str>,
) -> Result<Prepared> {
    let mut entry = source_memory(write, note);
    // Dated when the value took effect, so a backfilled value is filed
    // before the newer memories, never as their update (review point).
    entry.timestamp = match write.as_of {
        Some(as_of) => DateTime::from_timestamp_millis(keys::parse_time_ms(as_of)?)
            .ok_or_else(|| anyhow::anyhow!("as_of out of range"))?,
        // Settled under the lock: the time the store resolves.
        None => Utc::now(),
    };
    if let Some(project) = write.project.map(str::trim).filter(|p| !p.is_empty()) {
        plan::set_project(storage, &mut entry, project)?;
        entry.tags.push(project.to_owned());
    }
    // Planned outside the lock: which plain memory of the old value this
    // statement replaces. Settled again inside the transaction.
    // A hard failure (wrong model, bad request) fails the call as it does
    // for memory_save; a daemon that is down only leaves the memory
    // without a vector.
    let embedding = match embedder.embed(&format!("{} {}", entry.title, entry.content)) {
        Ok(embedding) => Some(embedding),
        Err(e) if crate::embedding::daemon_client::is_hard_failure(&e) => return Err(e),
        Err(_) => None,
    };
    // Checked for every statement: a retraction is not planned, and a
    // fallback vector of another size must not reach the store (review point).
    if let Some(embedding) = &embedding {
        storage.check_embedding_dims(embedding)?;
    }
    let planned = match (&embedding, write.value) {
        (Some(embedding), Some(_)) => {
            match plan::plan_save(storage, &entry, embedding, threshold, Mode::LinkOnly, true)? {
                plan::Plan::Save { link, .. } => link,
                plan::Plan::Duplicate { .. } => None,
            }
        }
        _ => None,
    };
    Ok(Prepared {
        entry,
        embedding,
        planned,
    })
}

fn commit(
    storage: &Storage,
    prepared: Prepared,
    threshold: f32,
    write: &FactWrite<'_>,
    note: Option<&str>,
) -> Result<Declared> {
    let Prepared {
        mut entry,
        embedding,
        planned,
    } = prepared;
    let mut conn = storage
        .conn
        .lock()
        .map_err(|e| anyhow::anyhow!("lock: {e}"))?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    // Read under the lock, like the store's own implicit now: commit order
    // is chain order for memories too (review point).
    let now = Utc::now();
    // Resolved again under the lock, like the fact's slot: a project merged
    // while this was planned must not leave the memory under its old name
    // (review point).
    if let Some(project) = write.project.map(str::trim).filter(|p| !p.is_empty()) {
        plan::set_project_in(&tx, &mut entry, project)?;
    }
    let preview = store::peek_in(&tx, write, now)?;
    // The memory sits where the value sits: the time the store resolved,
    // which for an implicit now can be after rows dated ahead (review point).
    if let Some(at) = preview.effective_at.as_deref() {
        let at = DateTime::parse_from_rfc3339(at)?.with_timezone(&Utc);
        // The store keeps milliseconds; an implicit now keeps the clock's
        // finer time, so a memory saved earlier in the same millisecond
        // stays before this one (review point).
        entry.timestamp = if write.as_of.is_none() {
            at.max(now)
        } else {
            at
        };
    }
    let planned = match planned {
        Some(plan) if !states_another_slot(&tx, &plan.target, &preview.fact.slot_id)? => Some(plan),
        // Planned against another variant of the fact, or not at all.
        _ => own_chain(&tx, &preview, &entry, threshold)?,
    };
    // A plain memory of another value saved since is the newest word: the
    // statement is then written, to update it (review point).
    let quiet =
        preview.replayed || (preview.outcome == "reconfirm" && note.is_none() && planned.is_none());
    if quiet {
        let outcome = store::apply_in(&tx, write, now)?;
        reaffirm_matched(&tx, &outcome, &entry)?;
        tx.commit()?;
        return Ok(Declared {
            outcome,
            memory: None,
            link: None,
        });
    }
    tx.execute(
        "INSERT INTO memories (id, timestamp, title, content, memory_type, tags, source, importance, metadata, embedding)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            entry.id,
            entry.timestamp.to_rfc3339(),
            entry.title,
            entry.content,
            entry.memory_type.to_string(),
            serde_json::to_string(&entry.tags)?,
            serde_json::to_string(&entry.source)?,
            entry.importance,
            entry.metadata.to_string(),
            embedding.as_ref().map(|e| crate::embedding::embedding_to_bytes(e)),
        ],
    )?;
    let link = match &planned {
        Some(link) => plan::commit_link(&tx, &entry, link, "mcp")?,
        None => None,
    };
    let sourced = FactWrite {
        source_memory_id: Some(&entry.id),
        ..write.clone()
    };
    let outcome = store::apply_in(&tx, &sourced, now)?;
    reaffirm_matched(&tx, &outcome, &entry)?;
    tx.commit()?;
    Ok(Declared {
        outcome,
        memory: Some(entry),
        link,
    })
}

#[cfg(test)]
#[path = "declare_tests.rs"]
mod tests;
