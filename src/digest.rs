//! Project state digests — the "wiki page" layer of the memory.
//!
//! `mnemonic context` used to inject a flat feed of recent atomic memories.
//! A digest turns that into a per-project STATE page: what the project is,
//! which decisions currently stand, which corrections/gotchas the user gave,
//! what is next — short, fresh, and source-traceable. New memories UPDATE
//! the page instead of merely piling onto a feed; that is the compounding
//! the Karpathy LLM-wiki pattern gets right, applied to our passive memory.
//!
//! Design (reviewed with Codex 2026-07-10):
//! - Deterministic only in v1: a few SQL queries + ranking, built ON THE FLY
//!   at context time. No cache table, no invalidation problem — freshness is
//!   guaranteed by construction. (An LLM polish layer can add a cache later.)
//! - Projects are keyed by NAME, not entity UUID, so graph rebuilds
//!   (`reextract --clean-graph`) can't orphan anything.
//! - Every bullet cites its source memory (short id), same contract as the
//!   Journal: nothing invented, everything traceable.

use anyhow::Result;
use chrono::{DateTime, Utc};

use crate::event::MemoryType;
use crate::journal::{FOLLOWUP_MARKERS, clean_line, is_noise_title};
use crate::storage::{RankedEntry, Storage};

/// Noise floor: a project with fewer real memories than this gets no digest
/// (same floor the Journal/attribution use).
const MIN_PROJECT_MEMS: usize = 2;
/// How many linked memories to pull per project for ranking.
const POOL_LIMIT: usize = 200;
const MAX_STANDING_DECISIONS: usize = 5;
const MAX_CORRECTIONS: usize = 3;
const MAX_FOLLOWUPS: usize = 2;
const MAX_RECENT: usize = 3;

/// A rendered digest plus the memory ids it cites — the caller (whisper)
/// uses the ids to avoid re-listing the same decisions in other sections.
#[derive(Debug, Clone)]
pub struct ProjectDigest {
    pub project: String,
    pub markdown: String,
    pub source_ids: Vec<String>,
}

/// Build the state digest for one project, or None when the project is
/// below the noise floor. Deterministic: same store state → same output.
pub fn build_project_digest(
    storage: &Storage,
    project_name: &str,
    now: DateTime<Utc>,
) -> Result<Option<ProjectDigest>> {
    let pool: Vec<RankedEntry> = storage
        .project_digest_pool(project_name, POOL_LIMIT)?
        .into_iter()
        .filter(|r| !is_noise_title(&r.entry.title))
        .collect();
    if pool.len() < MIN_PROJECT_MEMS {
        return Ok(None);
    }

    // Contradiction lint verdicts: a decision confirmed as REVERSED by a
    // newer one is not "standing" anymore — hide it here (the memory
    // itself stays untouched) and tag its replacement. SCOPED to this
    // project (review point): a memory can be linked to several projects,
    // and a pair only acts where the REPLACEMENT is actually visible —
    // otherwise the old decision would vanish from a digest that has no
    // replacement guidance to show.
    let pool_ids: std::collections::HashSet<&str> =
        pool.iter().map(|r| r.entry.id.as_str()).collect();
    let confirmed: Vec<(String, String)> = storage
        .confirmed_conflicts()?
        .into_iter()
        .filter(|(_, new)| pool_ids.contains(new.as_str()))
        .collect();
    let supersedes: std::collections::HashMap<&str, &str> = confirmed
        .iter()
        .map(|(old, new)| (new.as_str(), old.as_str()))
        .collect();

    let mut lines: Vec<String> = Vec::new();
    let mut source_ids: Vec<String> = Vec::new();
    let cite = |ids: &mut Vec<String>, entry_id: &str| {
        if !ids.iter().any(|i| i == entry_id) {
            ids.push(entry_id.to_string());
        }
    };

    lines.push(format!("### {project_name}"));

    // Standing decisions — what currently governs the project. Ranked by
    // decay-effective importance so touched/fresh decisions beat stale
    // ones; decisions the lint confirmed as reversed are excluded, and
    // their replacement carries a "supersedes" tag.
    //
    // INVARIANT (review point): an old decision is hidden ONLY when its
    // replacement is actually RENDERED. Replacements are force-included
    // ahead of the ranked rest; if a replacement still doesn't fit (more
    // confirmed pairs than the cap), its old decision is NOT suppressed —
    // hiding guidance without showing what superseded it loses state.
    let by_effective = |a: &&RankedEntry, b: &&RankedEntry| {
        b.effective(now)
            .partial_cmp(&a.effective(now))
            .unwrap_or(std::cmp::Ordering::Equal)
    };
    let replacement_ids: std::collections::HashSet<&str> =
        confirmed.iter().map(|(_, new)| new.as_str()).collect();
    // Chains (A→B and B→C) collapse to their terminals: an intermediate
    // replacement is itself reversed and must not be forced. The graph is
    // multi-valued — an old decision can have SEVERAL confirmed
    // replacements, and dropping the alternate edges would let the old row
    // resurface while one of its replacements renders (review points).
    let mut old_to_new: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    for (old, new) in &confirmed {
        old_to_new.entry(old.clone()).or_default().push(new.clone());
    }
    let mut forced: Vec<&RankedEntry> = pool
        .iter()
        .filter(|r| {
            r.entry.memory_type == MemoryType::Decision
                && replacement_ids.contains(r.entry.id.as_str())
                && !old_to_new.contains_key(r.entry.id.as_str())
        })
        .collect();
    forced.sort_by(by_effective);
    forced.truncate(MAX_STANDING_DECISIONS);
    // Only pairs whose (chain-terminal) replacement made the cut suppress.
    let shown_new: std::collections::HashSet<&str> =
        forced.iter().map(|r| r.entry.id.as_str()).collect();
    let reversed_old: std::collections::HashSet<&str> = pool
        .iter()
        .filter(|r| r.entry.memory_type == MemoryType::Decision)
        .map(|r| r.entry.id.as_str())
        .filter(|id| {
            chain_terminals(&old_to_new, id)
                .iter()
                .any(|t| shown_new.contains(t))
        })
        .collect();
    let standing: Vec<&RankedEntry> = {
        let mut rest: Vec<&RankedEntry> = pool
            .iter()
            .filter(|r| {
                r.entry.memory_type == MemoryType::Decision
                    && !reversed_old.contains(r.entry.id.as_str())
                    && !shown_new.contains(r.entry.id.as_str())
            })
            .collect();
        rest.sort_by(by_effective);
        forced
            .into_iter()
            .chain(rest)
            .take(MAX_STANDING_DECISIONS)
            .collect()
    };
    if !standing.is_empty() {
        lines.push("Standing decisions:".into());
        for r in &standing {
            let supersede_tag = supersedes
                .get(r.entry.id.as_str())
                .map(|old| format!(" (supersedes `{}`)", short_id(old)))
                .unwrap_or_default();
            lines.push(format!(
                "- {} `{}` ({}){supersede_tag}",
                clean_line(&r.entry.title),
                short_id(&r.entry.id),
                format_age(now, r.entry.timestamp)
            ));
            cite(&mut source_ids, &r.entry.id);
        }
    }

    // Corrections / gotchas — the user's "no, do it this way" signals.
    let corrections = top_by_effective(&pool, MemoryType::Feedback, MAX_CORRECTIONS, now);
    if !corrections.is_empty() {
        lines.push("Corrections (do not repeat):".into());
        for r in &corrections {
            lines.push(format!(
                "- {} `{}`",
                clean_line(&r.entry.title),
                short_id(&r.entry.id)
            ));
            cite(&mut source_ids, &r.entry.id);
        }
    }

    // Open follow-ups — forward-looking NOTES, newest first. Same guards
    // as the Journal (review points): corrections are never TODOs (a
    // Russian "не надо X" — "do not do X" — contains the "надо " marker),
    // decisions are never TODOs (a completed decision whose title mentions
    // follow-up words is not open work), the marker is matched against the
    // cleaned FIRST line only (multi-line bodies carry markers that don't
    // describe the memory), and nothing already cited above repeats here.
    let followups: Vec<&RankedEntry> = pool
        .iter()
        .filter(|r| {
            if r.entry.memory_type != MemoryType::Note
                || source_ids.iter().any(|id| id == &r.entry.id)
            {
                return false;
            }
            let t = clean_line(&r.entry.title).to_lowercase();
            !t.contains("не надо") && FOLLOWUP_MARKERS.iter().any(|m| t.contains(m))
        })
        .take(MAX_FOLLOWUPS)
        .collect();
    if !followups.is_empty() {
        lines.push("Next / open:".into());
        for r in &followups {
            lines.push(format!(
                "- {} `{}`",
                clean_line(&r.entry.title),
                short_id(&r.entry.id)
            ));
            cite(&mut source_ids, &r.entry.id);
        }
    }

    // Latest activity — the freshest few titles regardless of type, skipping
    // anything already shown above and anything the lint reversed (a dead
    // decision is history, not current state).
    let recent: Vec<&RankedEntry> = pool
        .iter()
        .filter(|r| {
            !source_ids.iter().any(|id| id == &r.entry.id)
                && !reversed_old.contains(r.entry.id.as_str())
        })
        .take(MAX_RECENT)
        .collect();
    if !recent.is_empty() {
        lines.push("Latest:".into());
        for r in &recent {
            // Cited like every other section: a memory that appears ONLY
            // here still needs a source id to refetch/verify it.
            lines.push(format!(
                "- {} `{}` ({})",
                clean_line(&r.entry.title),
                short_id(&r.entry.id),
                format_age(now, r.entry.timestamp)
            ));
            cite(&mut source_ids, &r.entry.id);
        }
    }

    Ok(Some(ProjectDigest {
        project: project_name.to_string(),
        markdown: lines.join("\n"),
        source_ids,
    }))
}

/// Digests for the currently most-active projects (weighted 14-day window),
/// ready for the context's Projects section.
pub fn top_project_digests(
    storage: &Storage,
    max_projects: usize,
    now: DateTime<Utc>,
) -> Result<Vec<ProjectDigest>> {
    // The noise floor is applied IN the candidate query, and candidates
    // are consumed in PAGES until enough digests exist (review points: any
    // fixed over-fetch — 2x, 25, whatever — can still be exhausted by
    // projects whose memories all turn out noise-titled at build time).
    // The page loop terminates when the query runs dry.
    let page_size = (max_projects * 2).max(10);
    let mut out = Vec::new();
    let mut offset = 0usize;
    loop {
        let names = storage.active_projects_weighted(14, page_size, MIN_PROJECT_MEMS, offset)?;
        let page_len = names.len();
        for name in names {
            if out.len() >= max_projects {
                return Ok(out);
            }
            if let Some(d) = build_project_digest(storage, &name, now)? {
                out.push(d);
            }
        }
        if page_len < page_size {
            break; // query ran dry
        }
        offset += page_size;
    }
    Ok(out)
}

fn top_by_effective(
    pool: &[RankedEntry],
    memory_type: MemoryType,
    limit: usize,
    now: DateTime<Utc>,
) -> Vec<&RankedEntry> {
    let mut filtered: Vec<&RankedEntry> = pool
        .iter()
        .filter(|r| r.entry.memory_type == memory_type)
        .collect();
    filtered.sort_by(|a, b| {
        b.effective(now)
            .partial_cmp(&a.effective(now))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    filtered.into_iter().take(limit).collect()
}

fn short_id(id: &str) -> &str {
    id.get(..8).unwrap_or(id)
}

/// All chain-terminal replacements reachable from `start` by following
/// confirmed old→new edges (an old decision can have SEVERAL confirmed
/// replacements, so the graph is multi-valued). A terminal is a node with
/// no outgoing edge; `start` itself never qualifies. A cycle — possible if
/// the judge confirms both directions of a pair — contributes no terminal,
/// so resolution degrades to "no replacement" instead of looping.
pub(crate) fn chain_terminals<'a>(
    old_to_new: &'a std::collections::HashMap<String, Vec<String>>,
    start: &'a str,
) -> Vec<&'a str> {
    let mut terminals: Vec<&str> = Vec::new();
    let mut visited: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let mut stack: Vec<&str> = vec![start];
    while let Some(cur) = stack.pop() {
        if !visited.insert(cur) {
            continue;
        }
        match old_to_new.get(cur) {
            Some(nexts) => stack.extend(nexts.iter().map(String::as_str)),
            None if cur != start => terminals.push(cur),
            None => {}
        }
    }
    terminals
}

fn format_age(now: DateTime<Utc>, ts: DateTime<Utc>) -> String {
    let days = (now - ts).num_days();
    match days {
        0 => "today".into(),
        1 => "1d ago".into(),
        d if d < 30 => format!("{d}d ago"),
        d => format!("{}mo ago", d / 30),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{EventSource, MemoryEntry};
    use crate::graph::{Entity, EntityType};
    use std::sync::Arc;

    fn tmp_storage() -> Arc<Storage> {
        let dir = std::env::temp_dir().join(format!("mn-digest-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        Arc::new(Storage::open(&dir.join("memory.db")).unwrap())
    }

    fn save_linked(storage: &Storage, entity_id: &str, title: &str, mt: MemoryType) -> String {
        let e = MemoryEntry::new(title, "body", mt, EventSource::Socket);
        storage.save(&e).unwrap();
        storage.link_memory_entity(&e.id, entity_id).unwrap();
        e.id.clone()
    }

    fn project_entity(storage: &Storage, name: &str) -> String {
        storage
            .upsert_entity(&Entity {
                name: name.into(),
                entity_type: EntityType::Project,
            })
            .unwrap()
    }

    #[test]
    fn digest_cites_sources_and_sections() {
        let storage = tmp_storage();
        let eid = project_entity(&storage, "demoapp");
        let d1 = save_linked(
            &storage,
            &eid,
            "Use SQLite for the store",
            MemoryType::Decision,
        );
        let f1 = save_linked(
            &storage,
            &eid,
            "не делай batch, только по одному",
            MemoryType::Feedback,
        );
        save_linked(&storage, &eid, "осталось: добить виджет", MemoryType::Note);

        let d = build_project_digest(&storage, "demoapp", Utc::now())
            .unwrap()
            .expect("digest for active project");
        assert!(d.markdown.contains("### demoapp"));
        assert!(d.markdown.contains("Standing decisions:"));
        assert!(d.markdown.contains("Use SQLite for the store"));
        assert!(d.markdown.contains(&d1[..8]), "decision cited");
        assert!(d.markdown.contains("Corrections (do not repeat):"));
        assert!(d.markdown.contains(&f1[..8]), "feedback cited");
        assert!(
            d.markdown.contains("Next / open:"),
            "followup marker caught"
        );
        assert!(d.source_ids.contains(&d1));
        assert!(d.source_ids.contains(&f1));
    }

    #[test]
    fn digest_excludes_superseded_session_summaries_and_noise() {
        let storage = tmp_storage();
        let eid = project_entity(&storage, "demoapp");
        let dead = save_linked(&storage, &eid, "Old dead decision", MemoryType::Decision);
        let live = save_linked(&storage, &eid, "Live decision", MemoryType::Decision);
        save_linked(&storage, &eid, "Session recap", MemoryType::SessionSummary);
        save_linked(&storage, &eid, "Conversation decision", MemoryType::Note); // noise title
        // A second clean memory so the project stays above the noise floor
        // after the excluded rows are filtered out.
        save_linked(&storage, &eid, "Plain working note", MemoryType::Note);
        // Mark the old one superseded by the live one.
        {
            let conn = storage.conn.lock().unwrap();
            conn.execute(
                "UPDATE memories SET superseded_by = ?2 WHERE id = ?1",
                rusqlite::params![dead, live],
            )
            .unwrap();
        }

        let d = build_project_digest(&storage, "demoapp", Utc::now())
            .unwrap()
            .expect("digest");
        assert!(!d.markdown.contains("Old dead decision"), "superseded gone");
        assert!(!d.markdown.contains("Session recap"), "summaries excluded");
        assert!(
            !d.markdown.contains("Conversation decision"),
            "noise excluded"
        );
        assert!(d.markdown.contains("Live decision"));
    }

    #[test]
    fn below_noise_floor_yields_none() {
        let storage = tmp_storage();
        let eid = project_entity(&storage, "tiny");
        save_linked(&storage, &eid, "Only one memory", MemoryType::Note);
        assert!(
            build_project_digest(&storage, "tiny", Utc::now())
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn caps_hold_for_busy_projects() {
        let storage = tmp_storage();
        let eid = project_entity(&storage, "busy");
        for i in 0..20 {
            save_linked(
                &storage,
                &eid,
                &format!("Decision number {i}"),
                MemoryType::Decision,
            );
        }
        let d = build_project_digest(&storage, "busy", Utc::now())
            .unwrap()
            .unwrap();
        // Count decision bullets in the Standing section only (everything
        // before "Latest:") — Latest bullets share the same titles here.
        let standing_section = d.markdown.split("Latest:").next().unwrap();
        let decision_lines = standing_section
            .lines()
            .filter(|l| l.starts_with("- Decision number"))
            .count();
        assert!(decision_lines <= MAX_STANDING_DECISIONS);
        assert!(d.markdown.lines().count() <= 20, "digest stays short");
        // Every Latest bullet is source-cited too (traceability contract).
        if let Some(latest) = d.markdown.split("Latest:").nth(1) {
            assert!(
                latest
                    .lines()
                    .filter(|l| l.starts_with("- "))
                    .all(|l| l.contains('`')),
                "Latest bullets must carry a short source id: {latest}"
            );
        }
    }

    /// A completed decision whose title mentions follow-up words is not
    /// open work: only NOTES can be follow-ups.
    #[test]
    fn decisions_never_become_followups() {
        let storage = tmp_storage();
        let eid = project_entity(&storage, "demoapp");
        save_linked(
            &storage,
            &eid,
            "осталось решено: перенесли сканер на воркер",
            MemoryType::Decision,
        );
        save_linked(&storage, &eid, "plain working note", MemoryType::Note);

        let d = build_project_digest(&storage, "demoapp", Utc::now())
            .unwrap()
            .expect("digest");
        assert!(
            !d.markdown.contains("Next / open:"),
            "a decision must not render as a TODO: {}",
            d.markdown
        );
    }

    /// A correction like "не надо X" (do not do X) must never surface as an
    /// open TODO: feedback is excluded from Next/open, and so is anything
    /// already cited in an earlier section.
    #[test]
    fn corrections_never_become_followups() {
        let storage = tmp_storage();
        let eid = project_entity(&storage, "demoapp");
        save_linked(
            &storage,
            &eid,
            "не надо использовать batch генерацию",
            MemoryType::Feedback,
        );
        save_linked(&storage, &eid, "plain working note", MemoryType::Note);

        let d = build_project_digest(&storage, "demoapp", Utc::now())
            .unwrap()
            .expect("digest");
        assert!(
            !d.markdown.contains("Next / open:"),
            "a prohibition must not render as a TODO: {}",
            d.markdown
        );
        assert!(d.markdown.contains("Corrections (do not repeat):"));
    }

    /// A busy project with hundreds of fresh notes must not evict its old
    /// standing decisions from the digest pool (recency limit applies to
    /// the note tail only).
    #[test]
    fn old_decisions_survive_a_flood_of_recent_notes() {
        let storage = tmp_storage();
        let eid = project_entity(&storage, "busy");
        let old_decision =
            save_linked(&storage, &eid, "Adopt SQLite forever", MemoryType::Decision);
        // Push the decision far into the past.
        {
            let conn = storage.conn.lock().unwrap();
            conn.execute(
                "UPDATE memories SET timestamp = '2024-01-01T00:00:00+00:00' WHERE id = ?1",
                [&old_decision],
            )
            .unwrap();
        }
        for i in 0..250 {
            save_linked(&storage, &eid, &format!("fresh note {i}"), MemoryType::Note);
        }

        let d = build_project_digest(&storage, "busy", Utc::now())
            .unwrap()
            .expect("digest");
        assert!(
            d.markdown.contains("Adopt SQLite forever"),
            "old standing decision must survive the note flood"
        );
    }

    /// A low-ranked replacement is FORCE-included into Standing decisions:
    /// hiding the old decision while the replacement falls below the top-K
    /// would leave the project with no guidance at all.
    #[test]
    fn weak_replacement_is_forced_into_standing() {
        let storage = tmp_storage();
        let eid = project_entity(&storage, "busy");
        let old = save_linked(&storage, &eid, "Use Postgres", MemoryType::Decision);
        let new = save_linked(&storage, &eid, "Switch to Redis", MemoryType::Decision);
        // Age the replacement far below every filler decision's rank.
        {
            let conn = storage.conn.lock().unwrap();
            conn.execute(
                "UPDATE memories SET timestamp = '2023-01-01T00:00:00+00:00',
                                     importance = 0.1 WHERE id = ?1",
                [&new],
            )
            .unwrap();
        }
        for i in 0..8 {
            save_linked(
                &storage,
                &eid,
                &format!("Strong call {i}"),
                MemoryType::Decision,
            );
        }
        storage
            .upsert_conflict(&old, &new, "busy", "confirmed", Some(0.9), Some("r"))
            .unwrap();

        let d = build_project_digest(&storage, "busy", Utc::now())
            .unwrap()
            .expect("digest");
        assert!(
            d.markdown.contains("Switch to Redis"),
            "replacement must be rendered despite its low rank: {}",
            d.markdown
        );
        assert!(
            !d.markdown.contains("Use Postgres"),
            "old decision hidden only because its replacement is shown"
        );
    }

    /// Conflicts are project-scoped: a memory linked to TWO projects whose
    /// replacement lives only in one of them keeps standing in the other —
    /// hiding it there would erase guidance with nothing to show instead.
    #[test]
    fn conflicts_only_act_where_the_replacement_is_visible() {
        let storage = tmp_storage();
        let shared_eid = project_entity(&storage, "shared");
        let other_eid = project_entity(&storage, "other");

        // The old decision is linked to BOTH projects.
        let old = save_linked(&storage, &shared_eid, "Use Postgres", MemoryType::Decision);
        storage.link_memory_entity(&old, &other_eid).unwrap();
        // The replacement exists only in "shared".
        let new = save_linked(
            &storage,
            &shared_eid,
            "Switch to Redis",
            MemoryType::Decision,
        );
        // Keep "other" above the noise floor.
        save_linked(&storage, &other_eid, "other note", MemoryType::Note);
        storage
            .upsert_conflict(&old, &new, "shared", "confirmed", Some(0.9), Some("r"))
            .unwrap();

        // In "shared" the pair acts: old hidden, replacement tagged.
        let d_shared = build_project_digest(&storage, "shared", Utc::now())
            .unwrap()
            .expect("shared digest");
        assert!(!d_shared.markdown.contains("Use Postgres"));
        assert!(d_shared.markdown.contains("Switch to Redis"));

        // In "other" the replacement is invisible → old keeps standing.
        let d_other = build_project_digest(&storage, "other", Utc::now())
            .unwrap()
            .expect("other digest");
        assert!(
            d_other.markdown.contains("Use Postgres"),
            "old decision must keep standing where its replacement is not \
             linked: {}",
            d_other.markdown
        );
    }

    /// A→B and B→C: only the chain terminal C stands. The intermediate B
    /// is a replacement of A, but it is itself reversed — forcing it in
    /// would resurface obsolete guidance (review point).
    #[test]
    fn conflict_chain_shows_only_the_terminal_decision() {
        let storage = tmp_storage();
        let eid = project_entity(&storage, "demoapp");
        let a = save_linked(
            &storage,
            &eid,
            "Use Postgres for events",
            MemoryType::Decision,
        );
        let b = save_linked(&storage, &eid, "Move events to Redis", MemoryType::Decision);
        let c = save_linked(
            &storage,
            &eid,
            "Settle events on Kafka",
            MemoryType::Decision,
        );
        storage
            .upsert_conflict(&a, &b, "demoapp", "confirmed", Some(0.9), Some("r"))
            .unwrap();
        storage
            .upsert_conflict(&b, &c, "demoapp", "confirmed", Some(0.9), Some("r"))
            .unwrap();

        let d = build_project_digest(&storage, "demoapp", Utc::now())
            .unwrap()
            .expect("digest");
        assert!(
            d.markdown.contains("Settle events on Kafka"),
            "chain terminal must stand: {}",
            d.markdown
        );
        assert!(
            !d.markdown.contains("Move events to Redis"),
            "intermediate replacement must not be forced: {}",
            d.markdown
        );
        assert!(
            !d.markdown.contains("Use Postgres for events"),
            "chain root must stay hidden while the terminal renders: {}",
            d.markdown
        );
    }

    /// An old decision with SEVERAL confirmed replacements stays hidden as
    /// long as ANY of them renders — alternate edges must not be lost even
    /// when one replacement path dead-ends in a judge-confirmed cycle
    /// (review point).
    #[test]
    fn old_with_multiple_replacements_hides_when_any_renders() {
        let storage = tmp_storage();
        let eid = project_entity(&storage, "demoapp");
        let a = save_linked(
            &storage,
            &eid,
            "Store events as flat files",
            MemoryType::Decision,
        );
        let b = save_linked(
            &storage,
            &eid,
            "Store events in SQLite",
            MemoryType::Decision,
        );
        let c = save_linked(
            &storage,
            &eid,
            "Store events in LevelDB",
            MemoryType::Decision,
        );
        let d = save_linked(
            &storage,
            &eid,
            "Store events in RocksDB",
            MemoryType::Decision,
        );
        // A was reversed twice; the C-path is a cycle (judge confirmed both
        // directions of C vs D), so only the B-path yields a terminal.
        storage
            .upsert_conflict(&a, &b, "demoapp", "confirmed", Some(0.9), Some("r"))
            .unwrap();
        storage
            .upsert_conflict(&a, &c, "demoapp", "confirmed", Some(0.9), Some("r"))
            .unwrap();
        storage
            .upsert_conflict(&c, &d, "demoapp", "confirmed", Some(0.9), Some("r"))
            .unwrap();
        storage
            .upsert_conflict(&d, &c, "demoapp", "confirmed", Some(0.9), Some("r"))
            .unwrap();

        let dig = build_project_digest(&storage, "demoapp", Utc::now())
            .unwrap()
            .expect("digest");
        assert!(
            dig.markdown.contains("Store events in SQLite"),
            "the terminal replacement must render: {}",
            dig.markdown
        );
        assert!(
            !dig.markdown.contains("Store events as flat files"),
            "old must stay hidden while ANY of its replacements renders: {}",
            dig.markdown
        );
        // The cycle pair resolves to no terminal: both stay visible rather
        // than hiding guidance with nothing rendered to replace it.
        assert!(dig.markdown.contains("Store events in LevelDB"));
        assert!(dig.markdown.contains("Store events in RocksDB"));
    }

    /// A decision the lint confirmed as reversed disappears from the
    /// standing list; its replacement is tagged and the memory row itself
    /// stays untouched.
    #[test]
    fn confirmed_conflict_hides_old_decision_and_tags_new() {
        let storage = tmp_storage();
        let eid = project_entity(&storage, "demoapp");
        let old = save_linked(&storage, &eid, "Use Postgres", MemoryType::Decision);
        let new = save_linked(&storage, &eid, "Switch to Redis", MemoryType::Decision);
        // Keeps the project above the noise floor after `new` dies below.
        save_linked(&storage, &eid, "plain working note", MemoryType::Note);
        storage
            .upsert_conflict(&old, &new, "demoapp", "confirmed", Some(0.9), Some("r"))
            .unwrap();

        let d = build_project_digest(&storage, "demoapp", Utc::now())
            .unwrap()
            .expect("digest");
        assert!(!d.markdown.contains("Use Postgres"), "reversed hidden");
        assert!(d.markdown.contains("Switch to Redis"));
        assert!(
            d.markdown
                .contains(&format!("(supersedes `{}`)", &old[..8])),
            "replacement is tagged: {}",
            d.markdown
        );
        // Audit-only: the memory row is untouched.
        {
            let conn = storage.conn.lock().unwrap();
            let n: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM memories WHERE id = ?1 AND superseded_by IS NULL",
                    [&old],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(n, 1);
        }

        // If the REPLACEMENT later dies (forgotten / superseded), the pair
        // stops acting — suppressing the old decision would erase the only
        // remaining guidance (review point).
        {
            let conn = storage.conn.lock().unwrap();
            conn.execute(
                "UPDATE memories SET superseded_by = 'gone' WHERE id = ?1",
                [&new],
            )
            .unwrap();
        }
        let d2 = build_project_digest(&storage, "demoapp", Utc::now())
            .unwrap()
            .expect("digest after replacement died");
        assert!(
            d2.markdown.contains("Use Postgres"),
            "old decision must resurface once its replacement is gone: {}",
            d2.markdown
        );
    }

    /// Projects that pass the SQL floor but turn out all-noise at build
    /// time must not exhaust the candidate window either: the loop pages
    /// until enough digests are built (or the query runs dry).
    #[test]
    fn paging_reaches_projects_past_noise_only_candidates() {
        let storage = tmp_storage();
        // The buildable project is created FIRST (oldest activity), so
        // every noise project outranks it on the recency tie-break.
        let real = project_entity(&storage, "real");
        save_linked(&storage, &real, "First real note", MemoryType::Note);
        save_linked(&storage, &real, "Second real note", MemoryType::Note);
        // 12 projects that pass the SQL floor (2 memories each) but whose
        // titles are all noise → build_project_digest returns None. That
        // is more than the first page (max_projects*2 = 6, min 10).
        for i in 0..12 {
            let eid = project_entity(&storage, &format!("noiseonly{i}"));
            save_linked(&storage, &eid, "Conversation decision", MemoryType::Note);
            save_linked(&storage, &eid, "User correction", MemoryType::Note);
        }

        let ds = top_project_digests(&storage, 3, Utc::now()).unwrap();
        assert!(
            ds.iter().any(|d| d.project == "real"),
            "paging must reach the buildable project behind 12 noise-only \
             candidates, got: {:?}",
            ds.iter().map(|d| d.project.clone()).collect::<Vec<_>>()
        );
    }

    /// Sub-floor (one-memory) projects can outrank a buildable project by
    /// weighted activity, but must never crowd it out of the digest list —
    /// the noise floor is applied in the candidate query itself.
    #[test]
    fn one_memory_projects_cannot_starve_buildable_ones() {
        let storage = tmp_storage();
        // Eight noisy projects, each with a single decision (weight 3).
        for i in 0..8 {
            let eid = project_entity(&storage, &format!("noise{i}"));
            save_linked(
                &storage,
                &eid,
                &format!("Lone decision {i}"),
                MemoryType::Decision,
            );
        }
        // One buildable project with two plain notes (weight 2 — ranked
        // BELOW every noisy project).
        let eid = project_entity(&storage, "real");
        save_linked(&storage, &eid, "First real note", MemoryType::Note);
        save_linked(&storage, &eid, "Second real note", MemoryType::Note);

        let ds = top_project_digests(&storage, 3, Utc::now()).unwrap();
        assert!(
            ds.iter().any(|d| d.project == "real"),
            "buildable project must get a digest despite 8 higher-ranked \
             sub-floor projects, got: {:?}",
            ds.iter().map(|d| d.project.clone()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn top_project_digests_ranks_recent_weighted_activity() {
        let storage = tmp_storage();
        let hot = project_entity(&storage, "hot");
        let cold = project_entity(&storage, "cold");
        for i in 0..3 {
            save_linked(
                &storage,
                &hot,
                &format!("Hot decision {i}"),
                MemoryType::Decision,
            );
        }
        // cold: two plain notes (weight 1 each, still above floor).
        save_linked(&storage, &cold, "Cold note one", MemoryType::Note);
        save_linked(&storage, &cold, "Cold note two", MemoryType::Note);

        let ds = top_project_digests(&storage, 2, Utc::now()).unwrap();
        assert_eq!(ds.len(), 2);
        assert_eq!(ds[0].project, "hot", "weighted ranking puts hot first");
        assert_eq!(ds[1].project, "cold");
    }
}
