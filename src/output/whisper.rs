use anyhow::Result;
use chrono::Utc;
use std::path::PathBuf;

use crate::embedding::create_embedder;
use crate::storage::{RankedEntry, Storage};

/// Hard ceiling for the generated context, enforced mechanically (design
/// review): adding project digests must make the injection SMARTER, not
/// BIGGER. Measured from the pre-digest output (~9-10k chars on a live DB).
const MAX_CONTEXT_CHARS: usize = 10_000;
/// How many project state digests lead the context.
const MAX_DIGEST_PROJECTS: usize = 3;

/// Character budget the section builders draw from. Once a line doesn't
/// fit, it (and anything after it in that section) is dropped — later,
/// lower-priority sections simply get whatever remains.
struct Budget {
    remaining: usize,
}

impl Budget {
    fn new(cap: usize) -> Self {
        Self { remaining: cap }
    }

    /// Reserve space for `s` (+1 for the joining newline). True = pushed.
    fn push(&mut self, out: &mut Vec<String>, s: String) -> bool {
        let cost = s.chars().count() + 1;
        if cost > self.remaining {
            return false;
        }
        self.remaining -= cost;
        out.push(s);
        true
    }
}

/// Whisper — context injection engine.
/// Generates a CONTEXT.md file with the most relevant memories for the current project.
/// Claude Code picks this up via memory files or CLAUDE.md includes.
pub struct Whisper {
    output_path: PathBuf,
    max_recent: usize,
    max_decisions: usize,
    max_feedback: usize,
}

impl Whisper {
    pub fn new(output_path: PathBuf) -> Self {
        Self {
            output_path,
            max_recent: 10,
            max_decisions: 10,
            max_feedback: 5,
        }
    }

    /// Generate context file from current memory state.
    /// Called on session start and periodically.
    ///
    /// Sections are ranked by `decay::effective_score`, so frequently-accessed
    /// or recently-touched memories outrank stale ones — even if the stale
    /// ones have a higher static `importance`.
    pub fn generate(&self, storage: &Storage) -> Result<String> {
        let now = Utc::now();
        let mut sections: Vec<String> = Vec::new();
        let mut budget = Budget::new(MAX_CONTEXT_CHARS);

        // Header (always fits — the cap is far above it).
        budget.push(
            &mut sections,
            format!(
                "# Mnemonic Context\n\n_Auto-generated: {}_\n",
                now.format("%Y-%m-%d %H:%M UTC")
            ),
        );

        // Project state digests FIRST — the highest-signal section: what
        // each active project is, its standing decisions, corrections and
        // open items, every bullet citing its source memory. Decisions and
        // feedback shown here are excluded from the flat sections below so
        // the digest saves tokens instead of duplicating them.
        let mut digest_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
        match crate::digest::top_project_digests(storage, MAX_DIGEST_PROJECTS, now) {
            Ok(digests) if !digests.is_empty() => {
                budget.push(&mut sections, "## Projects (state digests)\n".into());
                for d in &digests {
                    if !budget.push(&mut sections, format!("{}\n", d.markdown)) {
                        tracing::debug!("context: budget exhausted before digest of {}", d.project);
                        break;
                    }
                    digest_ids.extend(d.source_ids.iter().cloned());
                }
            }
            Ok(_) => {}
            Err(e) => tracing::warn!("context: project digests failed: {e}"),
        }

        // Pull a larger pool than we need so the effective-score ranker
        // has something to chew on. recent_ranked() returns DESC by timestamp.
        let pool = storage.recent_ranked(200)?;

        // User feedback / corrections (must not repeat mistakes) — before
        // decisions per design review: corrections are the rarest signal.
        let feedback: Vec<&RankedEntry> = top_by_effective(
            &pool,
            crate::event::MemoryType::Feedback,
            self.max_feedback + digest_ids.len(),
            now,
        )
        .into_iter()
        .filter(|r| !digest_ids.contains(&r.entry.id))
        .take(self.max_feedback)
        .collect();

        if !feedback.is_empty() {
            budget.push(
                &mut sections,
                "## User Feedback (DO NOT repeat these mistakes)\n".into(),
            );
            for ranked in &feedback {
                let entry = &ranked.entry;
                let age = format_age(now, entry.timestamp);
                if !budget.push(
                    &mut sections,
                    format!(
                        "- **{}** ({}) — {}",
                        entry.title,
                        age,
                        truncate(&entry.content, 120)
                    ),
                ) {
                    break;
                }
            }
            budget.push(&mut sections, String::new());
        }

        // Decisions, ranked by effective importance — minus the ones the
        // digests above already carry. Lint verdicts are applied with the
        // "never hide guidance" invariant (review point): a reversed-old
        // decision is DROPPED only when its replacement is already emitted
        // (in a digest or in this very list), SWAPPED for the replacement
        // when that is available in the pool, and KEPT as-is otherwise —
        // the context must never end up with neither side of a decision.
        // Multi-valued like the digest's graph: an old decision can carry
        // several confirmed replacements, and losing the alternate edges
        // would let the old row survive while one replacement renders.
        let mut replacement_of: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        for (old, new) in storage.confirmed_conflicts().unwrap_or_default() {
            replacement_of.entry(old).or_default().push(new);
        }
        let edge_count: usize = replacement_of.values().map(Vec::len).sum();
        let pool_by_id: std::collections::HashMap<&str, &RankedEntry> =
            pool.iter().map(|r| (r.entry.id.as_str(), r)).collect();
        // Conflicts are resolved BEFORE the max_decisions cap (review point):
        // dropped rows are backfilled from lower-ranked live decisions, so a
        // handful of superseded top entries can't shrink the section.
        let ranked: Vec<&RankedEntry> = top_by_effective(
            &pool,
            crate::event::MemoryType::Decision,
            self.max_decisions + digest_ids.len() + edge_count,
            now,
        )
        .into_iter()
        .filter(|r| !digest_ids.contains(&r.entry.id))
        .collect();
        let mut chosen_ids: std::collections::HashSet<&str> = std::collections::HashSet::new();
        let mut decisions: Vec<&RankedEntry> = Vec::with_capacity(self.max_decisions);
        for r in ranked {
            if decisions.len() >= self.max_decisions {
                break;
            }
            if chosen_ids.contains(r.entry.id.as_str()) {
                continue; // already swapped in for an earlier obsolete row
            }
            let terminals = crate::digest::chain_terminals(&replacement_of, &r.entry.id);
            if terminals.is_empty() {
                // Current decision (or a judge-confirmed cycle): keep it.
                chosen_ids.insert(r.entry.id.as_str());
                decisions.push(r);
            } else if terminals
                .iter()
                .any(|t| digest_ids.contains(*t) || chosen_ids.contains(*t))
            {
                // A replacement is already visible — the old row is history.
            } else if let Some(replacement) = terminals
                .iter()
                .filter_map(|t| pool_by_id.get(*t))
                .max_by(|a, b| {
                    a.effective(now)
                        .partial_cmp(&b.effective(now))
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
            {
                // Swap the obsolete decision for its strongest chain-final
                // replacement available in the pool.
                chosen_ids.insert(replacement.entry.id.as_str());
                decisions.push(replacement);
            } else {
                // Replacements outside the context window: keep the old
                // decision rather than showing nothing.
                chosen_ids.insert(r.entry.id.as_str());
                decisions.push(r);
            }
        }

        if !decisions.is_empty() {
            budget.push(&mut sections, "## Key Decisions\n".into());
            for ranked in &decisions {
                let entry = &ranked.entry;
                let age = format_age(now, entry.timestamp);
                if !budget.push(
                    &mut sections,
                    format!(
                        "- **{}** ({}) — {}",
                        entry.title,
                        age,
                        truncate(&entry.content, 120)
                    ),
                ) {
                    break;
                }
                if !entry.tags.is_empty() {
                    budget.push(
                        &mut sections,
                        format!("  _tags: {}_", entry.tags.join(", ")),
                    );
                }
            }
            budget.push(&mut sections, String::new());
        }

        // Recent activity: notes + session summaries, ranked by effective score.
        let mut other: Vec<&RankedEntry> = pool
            .iter()
            .filter(|r| {
                r.entry.memory_type != crate::event::MemoryType::Decision
                    && r.entry.memory_type != crate::event::MemoryType::Feedback
                    && !digest_ids.contains(&r.entry.id)
            })
            .collect();
        other.sort_by(|a, b| {
            b.effective(now)
                .partial_cmp(&a.effective(now))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let recent: Vec<&RankedEntry> = other.into_iter().take(self.max_recent).collect();

        if !recent.is_empty() {
            budget.push(&mut sections, "## Recent Activity\n".into());
            for ranked in &recent {
                let entry = &ranked.entry;
                let age = format_age(now, entry.timestamp);
                if !budget.push(
                    &mut sections,
                    format!(
                        "- [{}] **{}** ({}, importance: {:.1})",
                        entry.memory_type,
                        entry.title,
                        age,
                        ranked.effective(now)
                    ),
                ) {
                    break;
                }
            }
            budget.push(&mut sections, String::new());
        }

        // Knowledge Graph summary (top entities with connections)
        if let Ok((entity_count, edge_count)) = storage.graph_stats()
            && entity_count > 0
        {
            budget.push(&mut sections, "## Knowledge Graph\n".into());
            if let Ok(top_entities) = storage.list_entities(10) {
                'graph: for (name, etype, count) in &top_entities {
                    if *count >= 2 {
                        // Only show entities mentioned 2+ times
                        if !budget.push(
                            &mut sections,
                            format!("- **{}** ({}, {} mentions)", name, etype, count),
                        ) {
                            break 'graph;
                        }

                        // Show neighbors for high-mention entities
                        if let Ok(graph) = storage.graph_query(name)
                            && graph.found
                            && !graph.neighbors.is_empty()
                        {
                            let neighbor_names: Vec<String> = graph
                                .neighbors
                                .iter()
                                .take(5)
                                .map(|n| n.name.clone())
                                .collect();
                            budget.push(
                                &mut sections,
                                format!("  → connected to: {}", neighbor_names.join(", ")),
                            );
                        }
                    }
                }
            }
            budget.push(
                &mut sections,
                format!(
                    "\n_Graph: {} entities, {} edges_\n",
                    entity_count, edge_count
                ),
            );
        }

        // Stats
        if let Ok(stats) = storage.stats() {
            budget.push(
                &mut sections,
                format!(
                    "---\n_Total memories: {} | Generated by mnemonic_",
                    stats.total
                ),
            );
        }

        let content = sections.join("\n");
        debug_assert!(content.chars().count() <= MAX_CONTEXT_CHARS);

        // Write to file
        self.write_file(&content)?;

        Ok(content)
    }

    /// Generate context relevant to a specific topic.
    ///
    /// Uses hybrid retrieval (BM25 + vector + 1-hop graph) fused via RRF.
    /// Each hit carries a provenance label (bm25 / vector / graph / combos)
    /// so the agent can cite which retriever surfaced it.
    pub fn generate_for_topic(
        &self,
        storage: &Storage,
        topic: &str,
        limit: usize,
    ) -> Result<String> {
        let now = Utc::now();
        let embedder = create_embedder()?;
        let opts = crate::retrieval::HybridOptions {
            limit,
            ..Default::default()
        };
        let hits = crate::retrieval::hybrid_search(storage, &*embedder, topic, &opts)?;

        let mut sections: Vec<String> = Vec::new();
        sections.push(format!(
            "# Mnemonic Context: \"{}\"\n\n_Auto-generated: {}_\n",
            topic,
            now.format("%Y-%m-%d %H:%M UTC")
        ));

        if hits.is_empty() {
            sections.push("No relevant memories found.".into());
        } else {
            sections.push(format!(
                "## Relevant Memories ({} found, fused via RRF)\n",
                hits.len()
            ));
            for hit in &hits {
                let entry = &hit.entry;
                let age = format_age(now, entry.timestamp);
                sections.push(format!(
                    "- [{}] **{}** ({}, rrf: {:.3}, via: {}) — {}",
                    entry.memory_type,
                    entry.title,
                    age,
                    hit.score,
                    hit.source_label(),
                    truncate(&entry.content, 150),
                ));
                if !entry.tags.is_empty() {
                    sections.push(format!("  _tags: {}_", entry.tags.join(", ")));
                }
            }

            // Source citations: short id list so an agent can re-fetch the
            // exact memories that backed the answer.
            sections.push(String::new());
            sections.push("## Sources".into());
            for hit in &hits {
                sections.push(format!(
                    "- `{}` — {} ({})",
                    short_id(&hit.entry.id),
                    hit.entry.title,
                    hit.source_label()
                ));
            }
        }

        let content = sections.join("\n");
        Ok(content)
    }

    fn write_file(&self, content: &str) -> Result<()> {
        if let Some(parent) = self.output_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&self.output_path, content)?;
        Ok(())
    }
}

/// Filter `pool` by memory type and return the top-N by effective score.
fn top_by_effective(
    pool: &[RankedEntry],
    memory_type: crate::event::MemoryType,
    limit: usize,
    now: chrono::DateTime<Utc>,
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

/// Short-form id for the Sources section — first 8 hex chars of the UUID.
/// Keeps citations readable while still uniquely addressable in practice.
fn short_id(id: &str) -> &str {
    id.get(..8).unwrap_or(id)
}

fn format_age(now: chrono::DateTime<Utc>, timestamp: chrono::DateTime<Utc>) -> String {
    let diff = now - timestamp;
    let mins = diff.num_minutes();
    if mins < 60 {
        format!("{mins}m ago")
    } else if mins < 1440 {
        format!("{}h ago", mins / 60)
    } else {
        format!("{}d ago", mins / 1440)
    }
}

fn truncate(s: &str, max_chars: usize) -> String {
    let first_line = s.lines().next().unwrap_or(s);
    if first_line.chars().count() <= max_chars {
        first_line.to_string()
    } else {
        let truncated: String = first_line.chars().take(max_chars).collect();
        format!("{truncated}...")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{EventSource, MemoryEntry, MemoryType};
    use crate::graph::{Entity, EntityType};

    fn tmp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mn-whisper-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn save_linked(storage: &Storage, entity_id: &str, title: &str, mt: MemoryType) -> String {
        let e = MemoryEntry::new(title, "body of the memory", mt, EventSource::Socket);
        storage.save(&e).unwrap();
        storage.link_memory_entity(&e.id, entity_id).unwrap();
        e.id.clone()
    }

    /// The context must stay under the hard cap, lead with project digests,
    /// and never repeat a digest-cited decision in Key Decisions.
    #[test]
    fn context_has_digests_no_duplicates_and_respects_budget() {
        let dir = tmp_dir();
        let storage = Storage::open(&dir.join("memory.db")).unwrap();
        let eid = storage
            .upsert_entity(&Entity {
                name: "demoapp".into(),
                entity_type: EntityType::Project,
            })
            .unwrap();
        save_linked(
            &storage,
            &eid,
            "Adopt SQLite as the store",
            MemoryType::Decision,
        );
        save_linked(
            &storage,
            &eid,
            "не делай batch, только по одному",
            MemoryType::Feedback,
        );
        save_linked(&storage, &eid, "regular working note", MemoryType::Note);
        // Plenty of unlinked noise so the budget has something to trim.
        for i in 0..80 {
            let e = MemoryEntry::new(
                format!("Unlinked filler note {i} with a reasonably long title to eat budget"),
                "filler content that is long enough to occupy characters in the context output",
                MemoryType::Note,
                EventSource::Socket,
            );
            storage.save(&e).unwrap();
        }

        let whisper = Whisper::new(dir.join("CONTEXT.md"));
        let content = whisper.generate(&storage).unwrap();

        assert!(
            content.chars().count() <= MAX_CONTEXT_CHARS,
            "budget violated: {} chars",
            content.chars().count()
        );
        assert!(content.contains("## Projects (state digests)"));
        assert!(content.contains("### demoapp"));
        // The digest carries the decision; Key Decisions must not repeat it.
        let occurrences = content.matches("Adopt SQLite as the store").count();
        assert_eq!(occurrences, 1, "digest decision must appear exactly once");
        let fb = content.matches("не делай batch, только по одному").count();
        assert_eq!(fb, 1, "digest feedback must appear exactly once");
    }

    /// Key Decisions never loses guidance to a conflict verdict: when the
    /// old decision ranks in and its low-ranked replacement does not, the
    /// list SWAPS in the replacement instead of showing neither.
    #[test]
    fn key_decisions_swap_reversed_for_their_replacement() {
        let dir = tmp_dir();
        let storage = Storage::open(&dir.join("memory.db")).unwrap();
        // No project entities: the pair lives outside any digest.
        let old = MemoryEntry::new(
            "Use Postgres everywhere",
            "storage decision",
            MemoryType::Decision,
            EventSource::Socket,
        );
        storage.save(&old).unwrap();
        let new = MemoryEntry::new(
            "Switch storage to Redis",
            "reversal",
            MemoryType::Decision,
            EventSource::Socket,
        );
        storage.save(&new).unwrap();
        // Age the replacement below the top-10 while old ranks first.
        {
            let conn = storage.conn.lock().unwrap();
            conn.execute(
                "UPDATE memories SET timestamp = '2023-01-01T00:00:00+00:00',
                                     importance = 0.1 WHERE id = ?1",
                rusqlite::params![new.id],
            )
            .unwrap();
            conn.execute(
                "UPDATE memories SET importance = 0.99 WHERE id = ?1",
                rusqlite::params![old.id],
            )
            .unwrap();
        }
        for i in 0..12 {
            let filler = MemoryEntry::new(
                format!("Strong unrelated call {i}"),
                "filler decision",
                MemoryType::Decision,
                EventSource::Socket,
            );
            storage.save(&filler).unwrap();
        }
        storage
            .upsert_conflict(&old.id, &new.id, "p", "confirmed", Some(0.9), Some("r"))
            .unwrap();

        let whisper = Whisper::new(dir.join("CONTEXT.md"));
        let content = whisper.generate(&storage).unwrap();
        assert!(
            !content.contains("Use Postgres everywhere"),
            "reversed decision must not surface: {content}"
        );
        assert!(
            content.contains("Switch storage to Redis"),
            "its replacement must be swapped in: {content}"
        );
    }

    /// Dropping a superseded top decision (replacement already in a digest)
    /// must not shrink Key Decisions: a lower-ranked live decision backfills
    /// the slot (conflicts resolve BEFORE the cap, review point).
    #[test]
    fn dropped_superseded_decisions_are_backfilled() {
        let dir = tmp_dir();
        let storage = Storage::open(&dir.join("memory.db")).unwrap();
        let eid = storage
            .upsert_entity(&Entity {
                name: "demoapp".into(),
                entity_type: EntityType::Project,
            })
            .unwrap();
        // The replacement lives in the project digest.
        let new_id = save_linked(&storage, &eid, "Switch to Redis", MemoryType::Decision);
        save_linked(&storage, &eid, "plain working note", MemoryType::Note);
        // The reversed decision ranks FIRST among flat candidates.
        let old = MemoryEntry::new(
            "Use Postgres everywhere",
            "storage decision",
            MemoryType::Decision,
            EventSource::Socket,
        );
        storage.save(&old).unwrap();
        {
            let conn = storage.conn.lock().unwrap();
            conn.execute(
                "UPDATE memories SET importance = 0.99 WHERE id = ?1",
                rusqlite::params![old.id],
            )
            .unwrap();
        }
        storage
            .upsert_conflict(
                &old.id,
                &new_id,
                "demoapp",
                "confirmed",
                Some(0.9),
                Some("r"),
            )
            .unwrap();
        // Exactly max_decisions live decisions besides `old`: without
        // backfill the section would come out one short.
        for i in 0..9 {
            let filler = MemoryEntry::new(
                format!("Strong unrelated call {i}"),
                "filler decision",
                MemoryType::Decision,
                EventSource::Socket,
            );
            storage.save(&filler).unwrap();
        }
        let tail = MemoryEntry::new(
            "Backfill decision at the tail",
            "lowest ranked live decision",
            MemoryType::Decision,
            EventSource::Socket,
        );
        storage.save(&tail).unwrap();
        {
            let conn = storage.conn.lock().unwrap();
            conn.execute(
                "UPDATE memories SET importance = 0.05 WHERE id = ?1",
                rusqlite::params![tail.id],
            )
            .unwrap();
        }

        let whisper = Whisper::new(dir.join("CONTEXT.md"));
        let content = whisper.generate(&storage).unwrap();
        assert!(
            !content.contains("Use Postgres everywhere"),
            "superseded decision must not surface: {content}"
        );
        assert!(
            content.contains("Backfill decision at the tail"),
            "dropped slot must be backfilled from lower-ranked decisions: {content}"
        );
    }

    /// With no project entities at all, the section is simply absent and
    /// generation still succeeds (fresh installs).
    #[test]
    fn context_without_projects_omits_digest_section() {
        let dir = tmp_dir();
        let storage = Storage::open(&dir.join("memory.db")).unwrap();
        let e = MemoryEntry::new(
            "solo note",
            "content",
            MemoryType::Note,
            EventSource::Socket,
        );
        storage.save(&e).unwrap();

        let whisper = Whisper::new(dir.join("CONTEXT.md"));
        let content = whisper.generate(&storage).unwrap();
        assert!(!content.contains("## Projects (state digests)"));
        assert!(content.contains("solo note"));
    }
}
