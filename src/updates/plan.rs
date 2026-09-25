//! The value-aware save gate: what to do with a new memory that embeds close
//! to existing ones.
//!
//! Text with no business values keeps the old rule exactly: any live near
//! duplicate drops it. Text with values is dropped only when it restates the
//! values the HEAD of a near duplicate's update chain holds now. A changed
//! value is saved and linked to that head; anything the scanner cannot
//! compare is saved unlinked. So a revert ($5, $6, back to $5) links instead
//! of vanishing, and replaying an old statement cannot resurrect it as new.

use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};

use super::scan::{Class, Value, scan};
use super::store::{self, Link, Rule};
use super::verdict::{Verdict, verdict};
use crate::embedding::Embedding;
use crate::event::MemoryEntry;
use crate::storage::Storage;

/// How many near duplicates the gate weighs.
const CANDIDATES: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// A near duplicate may drop the new memory.
    Normal,
    /// The memory is saved regardless (a user correction); only links are
    /// planned.
    LinkOnly,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LinkPlan {
    /// Head of the chain the new memory updates (or, when older, precedes).
    pub target: String,
    /// The new memory is at least as recent as `target`.
    pub new_is_newer: bool,
    pub class: Class,
    pub was: Vec<Value>,
    pub now: Vec<Value>,
    pub similarity: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Plan {
    Duplicate {
        of: String,
        similarity: f32,
    },
    Save {
        link: Option<LinkPlan>,
        /// The text states business values: an importance floor must not
        /// drop it (a price change is worth keeping however plain it reads).
        valued: bool,
    },
}

/// What the gate reads of an existing memory.
#[derive(Clone)]
struct Row {
    id: String,
    text: String,
    /// When the memory was said: its place in a chain.
    timestamp: DateTime<Utc>,
    /// When its value was last said, counting statements dropped as its
    /// duplicates: which end of a fork is current.
    last_said: DateTime<Utc>,
    project_key: Option<String>,
    /// Written by a machine or derived from other memories: it can neither
    /// drop nor anchor a value-bearing save.
    derived: bool,
}

pub fn text_of(title: &str, content: &str) -> String {
    format!("{title}\n{content}")
}

/// The project a memory was saved under, if the saver said.
pub fn project_key(metadata: &serde_json::Value) -> Option<String> {
    metadata
        .get("project_key")
        .and_then(|v| v.as_str())
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
}

fn row(conn: &Connection, id: &str) -> Result<Option<Row>> {
    let found = conn
        .query_row(
            "SELECT m.id, m.title, m.content, m.timestamp,
                    max(m.timestamp, COALESCE((SELECT a.at FROM memory_reaffirmed a
                                               WHERE a.memory_id = m.id), m.timestamp)),
                    m.metadata, m.source, m.memory_type,
                    EXISTS (SELECT 1 FROM reflection_sources r WHERE r.canonical_id = m.id)
               FROM memories m WHERE m.id = ?1 AND m.superseded_by IS NULL",
            params![id],
            |r| {
                Ok((
                    (
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                    ),
                    (r.get::<_, String>(3)?, r.get::<_, String>(4)?),
                    (
                        r.get::<_, String>(5)?,
                        r.get::<_, String>(6)?,
                        r.get::<_, String>(7)?,
                        r.get::<_, bool>(8)?,
                    ),
                ))
            },
        )
        .optional()?;
    let Some(((id, title, content), (said, last_said), (metadata, source, memory_type, canonical))) =
        found
    else {
        return Ok(None);
    };
    let parse = |text: &str| -> Result<DateTime<Utc>> {
        Ok(DateTime::parse_from_rfc3339(text)?.with_timezone(&Utc))
    };
    let (timestamp, last_said) = (parse(&said)?, parse(&last_said)?);
    let metadata: serde_json::Value = serde_json::from_str(&metadata).unwrap_or_default();
    let machine = source.contains("GitWatcher") || source.contains("FileWatcher");
    Ok(Some(Row {
        id,
        text: text_of(&title, &content),
        timestamp,
        last_said,
        project_key: project_key(&metadata),
        derived: machine || memory_type == "session_summary" || canonical,
    }))
}

fn disjoint(a: &Option<String>, b: &Option<String>) -> bool {
    matches!((a, b), (Some(a), Some(b)) if a != b)
}

/// Machine-written or derived memories stay out of update chains.
fn entry_is_derived(entry: &MemoryEntry) -> bool {
    use crate::event::{EventSource, MemoryType};
    matches!(entry.memory_type, MemoryType::SessionSummary)
        || matches!(
            entry.source,
            EventSource::GitWatcher | EventSource::FileWatcher
        )
}

/// Decide what to do with `entry`, whose embedding is `embedding`.
pub fn plan_save(
    storage: &Storage,
    entry: &MemoryEntry,
    embedding: &Embedding,
    threshold: f32,
    mode: Mode,
    value_aware: bool,
) -> Result<Plan> {
    let candidates = storage.near_duplicates(embedding, threshold, CANDIDATES)?;
    let new_scan = scan(&text_of(&entry.title, &entry.content));
    if !value_aware || !new_scan.has_values() || entry_is_derived(entry) {
        return Ok(match (mode, candidates.into_iter().next()) {
            (Mode::Normal, Some((of, similarity))) => Plan::Duplicate { of, similarity },
            _ => Plan::Save {
                link: None,
                valued: false,
            },
        });
    }
    let conn = storage
        .conn
        .lock()
        .map_err(|e| anyhow::anyhow!("lock: {e}"))?;
    let new_project = project_key(&entry.metadata);
    // Another project's memory, or a derived one, neither drops nor anchors
    // this save; nor does a chain that leads into one.
    let eligible = |row: &Row| !row.derived && !disjoint(&new_project, &row.project_key);
    // Every chain about the same thing is compared at the new memory's own
    // time: its current end for a new statement, the value that held then
    // for an older one. The newest of those values decides (review points:
    // an older value restated must not eat a revert from a newer one, and
    // a replay in one chain must not outvote a change in another).
    let mut newest: Option<(DateTime<Utc>, Plan)> = None;
    for (id, similarity) in candidates {
        let Some(candidate) = row(&conn, &id)? else {
            continue;
        };
        if !eligible(&candidate) {
            continue;
        }
        let candidate_scan = scan(&candidate.text);
        if !candidate_scan.has_values() {
            continue;
        }
        // A chain with no end to find (a cycle) decides nothing.
        let Some(head_id) = store::head(&conn, &candidate.id)? else {
            continue;
        };
        let head = if head_id == candidate.id {
            candidate
        } else {
            match row(&conn, &head_id)? {
                Some(head) if eligible(&head) => head,
                _ => continue,
            }
        };
        let then = if entry.timestamp < head.timestamp {
            match held_at(&conn, &head, entry.timestamp)? {
                Some(then) if eligible(&then) => then,
                _ => continue,
            }
        } else {
            head.clone()
        };
        // Said after the value that held then, but before that value was
        // said again: a change that did not last. It is kept, not linked:
        // the chain cannot hold that value twice.
        let transient = entry.timestamp >= then.timestamp && entry.timestamp < then.last_said;
        let outcome = match verdict(&new_scan, &scan(&then.text)) {
            Verdict::Update { .. } if transient => Plan::Save {
                link: None,
                valued: true,
            },
            // The value it repeats is one a declared fact has since
            // retracted or replaced: said again, it is news (review point).
            Verdict::Same if fact_moved_on(&conn, &then.id, entry.timestamp)? => Plan::Save {
                link: None,
                valued: true,
            },
            Verdict::Same => Plan::Duplicate {
                of: then.id.clone(),
                similarity,
            },
            Verdict::Update { class, was, now } => Plan::Save {
                link: Some(LinkPlan {
                    new_is_newer: entry.timestamp >= head.timestamp,
                    target: head.id.clone(),
                    class,
                    was,
                    now,
                    similarity,
                }),
                valued: true,
            },
            Verdict::Distinct => continue,
        };
        // How recent this chain's word on the value is, as of the new
        // memory's time: its last restatement when that came before it,
        // otherwise when it was first said.
        let word_at = if then.last_said <= entry.timestamp {
            then.last_said
        } else {
            then.timestamp
        };
        if newest.as_ref().is_none_or(|(at, _)| word_at > *at) {
            newest = Some((word_at, outcome));
        }
    }
    let decided = newest.map(|(_, plan)| plan);
    Ok(match (mode, decided) {
        (Mode::Normal, Some(plan)) => plan,
        (Mode::LinkOnly, Some(plan @ Plan::Save { .. })) => plan,
        _ => Plan::Save {
            link: None,
            valued: true,
        },
    })
}

/// Memory `id` stated fact values, and at `at` none of them holds in its
/// slot any more: the fact was retracted or changed after it.
fn fact_moved_on(conn: &Connection, id: &str, at: DateTime<Utc>) -> Result<bool> {
    Ok(conn.query_row(
        "WITH stated AS (
             SELECT v.id, v.slot_id FROM fact_values v
              WHERE v.status = 'active' AND v.kind = 'value'
                AND (v.source_memory_id = ?1
                     OR v.id IN (SELECT value_id FROM fact_events
                                  WHERE evidence_memory_id = ?1 AND value_id IS NOT NULL))),
         holders AS (
             SELECT (SELECT c.id FROM fact_values c
                      WHERE c.slot_id = s.slot_id AND c.status = 'active'
                        AND c.valid_from_ms <= ?2
                      ORDER BY c.valid_from_ms DESC, c.seq DESC LIMIT 1) AS id
               FROM (SELECT DISTINCT slot_id FROM stated) s)
         SELECT EXISTS (SELECT 1 FROM stated)
            AND NOT EXISTS (SELECT 1 FROM stated WHERE id IN (SELECT id FROM holders))",
        params![id, at.timestamp_millis()],
        |r| r.get(0),
    )?)
}

/// Settle a planned duplicate of `id` for a statement made at `at`, inside
/// the caller's write transaction: another writer may have moved the chain
/// on since the plan. The duplicate stands when `id` is still the memory
/// whose value held at `at`; its restatement is then recorded, so a value
/// backfilled later cannot slip in before it. Anything else means the
/// chain moved: answer false and plan again.
pub fn settle_duplicate(conn: &Connection, id: &str, at: DateTime<Utc>) -> Result<bool> {
    // Gone, or in a chain with no end: nothing left to be a duplicate of.
    let Some(head_id) = store::head(conn, id)? else {
        return Ok(false);
    };
    let Some(head) = row(conn, &head_id)? else {
        return Ok(false);
    };
    let held = if at < head.timestamp {
        held_at(conn, &head, at)?
    } else {
        Some(head)
    };
    match held {
        Some(member) if member.id == id && !fact_moved_on(conn, id, at)? => {
            store::reaffirm(conn, id, &at.to_rfc3339())?;
            Ok(true)
        }
        _ => Ok(false),
    }
}

/// `plan_save`, with a duplicate settled under the write lock before it
/// counts (see `Storage::confirm_duplicate`): a chain moved on by another
/// writer in between is planned again, a few times at most, then the memory
/// is simply saved. The answer is final: a Duplicate here has been recorded.
pub fn plan_settled(
    storage: &Storage,
    entry: &MemoryEntry,
    embedding: &Embedding,
    threshold: f32,
    mode: Mode,
    value_aware: bool,
) -> Result<Plan> {
    // The plain similarity rule (value awareness off, no values, a derived
    // memory) has no chain to settle against: its duplicate stands as is.
    if !value_aware
        || entry_is_derived(entry)
        || !scan(&text_of(&entry.title, &entry.content)).has_values()
    {
        return plan_save(storage, entry, embedding, threshold, mode, value_aware);
    }
    for _ in 0..3 {
        let plan = plan_save(storage, entry, embedding, threshold, mode, value_aware)?;
        match &plan {
            Plan::Duplicate { of, .. } if !storage.confirm_duplicate(of, entry.timestamp)? => {}
            _ => return Ok(plan),
        }
    }
    Ok(Plan::Save {
        link: None,
        valued: true,
    })
}

/// Write the planned link for `entry`, which the caller has just inserted in
/// the same transaction. Another writer may have extended the chain since
/// the plan was made: then the new memory is compared with the chain's new
/// head and linked only if it still updates it. A memory older than the
/// head is placed where its time puts it in the chain.
pub fn commit_link(
    conn: &Connection,
    entry: &MemoryEntry,
    plan: &LinkPlan,
    actor: &'static str,
) -> Result<Option<Link>> {
    let Some(head_id) = store::head(conn, &plan.target)? else {
        return Ok(None);
    };
    let Some(head) = row(conn, &head_id)? else {
        return Ok(None);
    };
    if head.derived || disjoint(&project_key(&entry.metadata), &head.project_key) {
        return Ok(None);
    }
    let new_scan = scan(&text_of(&entry.title, &entry.content));
    if entry.timestamp < head.timestamp {
        return place_in_history(conn, entry, &new_scan, head, plan.similarity, actor);
    }
    if entry.timestamp < head.last_said {
        // The head's value was said again after this: not the current end.
        return Ok(None);
    }
    let (class, was, now) = if head.id == plan.target {
        (plan.class, plan.was.clone(), plan.now.clone())
    } else {
        match verdict(&new_scan, &scan(&head.text)) {
            Verdict::Update { class, was, now } => (class, was, now),
            _ => return Ok(None),
        }
    };
    let link = Link {
        new_id: entry.id.clone(),
        old_id: head.id,
        rule: Rule::ValueDiff,
        class,
        was,
        now,
        similarity: Some(plan.similarity),
        actor,
    };
    Ok(store::insert(conn, &link)?.then_some(link))
}

/// The member of `head`'s chain whose value held at `at`: the newest one
/// not after it, or the chain's first member when all of it came later.
/// `None` for a chain that loops.
fn held_at(conn: &Connection, head: &Row, at: DateTime<Utc>) -> Result<Option<Row>> {
    let mut member = head.clone();
    let mut seen = std::collections::HashSet::new();
    while member.timestamp > at {
        if !seen.insert(member.id.clone()) {
            return Ok(None);
        }
        match newest_predecessor(conn, &member.id)? {
            Some(before) => member = before,
            None => break,
        }
    }
    Ok(Some(member))
}

/// The chain member `id` directly updates that is newest.
fn newest_predecessor(conn: &Connection, id: &str) -> Result<Option<Row>> {
    let predecessor: Option<String> = conn
        .query_row(
            "SELECT u.old_id FROM memory_updates u JOIN memories m ON m.id = u.old_id
              WHERE u.new_id = ?1 AND u.status = 'active'
              ORDER BY m.timestamp DESC, u.old_id DESC LIMIT 1",
            params![id],
            |r| r.get(0),
        )
        .optional()?;
    match predecessor {
        Some(id) => row(conn, &id),
        None => Ok(None),
    }
}

/// Splice an older memory into the chain by time: between the newest member
/// before it and the oldest member after it, so the chain still reads in
/// the order the values held. Returns the link from its successor.
fn place_in_history(
    conn: &Connection,
    entry: &MemoryEntry,
    new_scan: &crate::updates::scan::Scan,
    head: Row,
    similarity: f32,
    actor: &'static str,
) -> Result<Option<Link>> {
    let mut successor = head;
    let mut seen = std::collections::HashSet::new();
    let predecessor = loop {
        if !seen.insert(successor.id.clone()) {
            return Ok(None);
        }
        match newest_predecessor(conn, &successor.id)? {
            Some(p) if p.timestamp > entry.timestamp => successor = p,
            other => break other,
        }
    };
    // Both ends must be memories this one may join: a chain can hold
    // memories of more than one project when an unscoped one sits between.
    let project = project_key(&entry.metadata);
    let joinable = |row: &Row| !row.derived && !disjoint(&project, &row.project_key);
    if !joinable(&successor) || predecessor.as_ref().is_some_and(|p| !joinable(p)) {
        return Ok(None);
    }
    // The value before it was said again after it: a change that did not
    // last, kept off the chain (as for the head, in `commit_link`).
    if predecessor
        .as_ref()
        .is_some_and(|p| p.last_said > entry.timestamp)
    {
        return Ok(None);
    }
    // Both new links must hold before anything changes.
    let Verdict::Update { class, was, now } = verdict(&scan(&successor.text), new_scan) else {
        return Ok(None);
    };
    let before = match &predecessor {
        Some(p) => match verdict(new_scan, &scan(&p.text)) {
            Verdict::Update { class, was, now } => Some((p.id.clone(), class, was, now)),
            _ => return Ok(None),
        },
        None => None,
    };
    if let Some((old_id, class, was, now)) = before {
        conn.execute(
            "DELETE FROM memory_updates WHERE new_id = ?1 AND old_id = ?2",
            params![successor.id, old_id],
        )?;
        store::insert(
            conn,
            &Link {
                new_id: entry.id.clone(),
                old_id,
                rule: Rule::ValueDiff,
                class,
                was,
                now,
                similarity: Some(similarity),
                actor,
            },
        )?;
    }
    let link = Link {
        new_id: successor.id,
        old_id: entry.id.clone(),
        rule: Rule::ValueDiff,
        class,
        was,
        now,
        similarity: Some(similarity),
        actor,
    };
    Ok(store::insert(conn, &link)?.then_some(link))
}

/// Record that `entry` belongs to `project`: the display name and the key
/// scoped reads and the save gate compare.
pub fn set_project(storage: &Storage, entry: &mut MemoryEntry, project: &str) -> Result<()> {
    let name = crate::followups::resolve_project(storage, project)?;
    put_project(entry, &name);
    Ok(())
}

/// `set_project` in the caller's transaction: a project merged meanwhile
/// resolves to its new name here, as the fact's own slot does.
pub fn set_project_in(conn: &Connection, entry: &mut MemoryEntry, project: &str) -> Result<()> {
    let name = crate::followups::canonical_project(conn, project)?;
    put_project(entry, &name);
    Ok(())
}

fn put_project(entry: &mut MemoryEntry, name: &str) {
    let key = crate::followups::project_key(name);
    if !entry.metadata.is_object() {
        entry.metadata = serde_json::json!({});
    }
    if let Some(metadata) = entry.metadata.as_object_mut() {
        metadata.insert("project".into(), name.into());
        metadata.insert("project_key".into(), key.into());
    }
}

/// A written link as a save response shows it, from `saved_id`'s side.
pub fn link_json(saved_id: &str, link: &Link) -> serde_json::Value {
    let join = |values: &[Value]| {
        values
            .iter()
            .map(|v| v.surface.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };
    let (id, direction) = if link.new_id == saved_id {
        (&link.old_id, "updates")
    } else {
        (&link.new_id, "updated_by")
    };
    serde_json::json!({
        "id": id,
        "direction": direction,
        "class": link.class.as_str(),
        "was": join(&link.was),
        "now": join(&link.now),
    })
}

#[cfg(test)]
#[path = "plan_tests.rs"]
mod tests;
