use anyhow::Result;
use chrono::Utc;
use std::path::PathBuf;

use crate::embedding::create_embedder;
use crate::storage::{RankedEntry, Storage};

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

        // Header
        sections.push(format!(
            "# Mnemonic Context\n\n_Auto-generated: {}_\n",
            now.format("%Y-%m-%d %H:%M UTC")
        ));

        // Pull a larger pool than we need so the effective-score ranker
        // has something to chew on. recent_ranked() returns DESC by timestamp.
        let pool = storage.recent_ranked(200)?;

        // Decisions, ranked by effective importance.
        let decisions = top_by_effective(
            &pool,
            crate::event::MemoryType::Decision,
            self.max_decisions,
            now,
        );

        if !decisions.is_empty() {
            sections.push("## Key Decisions\n".into());
            for ranked in &decisions {
                let entry = &ranked.entry;
                let age = format_age(now, entry.timestamp);
                sections.push(format!(
                    "- **{}** ({}) — {}",
                    entry.title,
                    age,
                    truncate(&entry.content, 120),
                ));
                if !entry.tags.is_empty() {
                    sections.push(format!("  _tags: {}_", entry.tags.join(", ")));
                }
            }
            sections.push(String::new());
        }

        // User feedback / corrections (must not repeat mistakes).
        // Ranked by effective importance — fresh corrections beat stale ones.
        let feedback = top_by_effective(
            &pool,
            crate::event::MemoryType::Feedback,
            self.max_feedback,
            now,
        );

        if !feedback.is_empty() {
            sections.push("## User Feedback (DO NOT repeat these mistakes)\n".into());
            for ranked in &feedback {
                let entry = &ranked.entry;
                let age = format_age(now, entry.timestamp);
                sections.push(format!(
                    "- **{}** ({}) — {}",
                    entry.title,
                    age,
                    truncate(&entry.content, 120)
                ));
            }
            sections.push(String::new());
        }

        // Recent activity: notes + session summaries, ranked by effective score.
        let mut other: Vec<&RankedEntry> = pool
            .iter()
            .filter(|r| {
                r.entry.memory_type != crate::event::MemoryType::Decision
                    && r.entry.memory_type != crate::event::MemoryType::Feedback
            })
            .collect();
        other.sort_by(|a, b| {
            b.effective(now)
                .partial_cmp(&a.effective(now))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let recent: Vec<&RankedEntry> = other.into_iter().take(self.max_recent).collect();

        if !recent.is_empty() {
            sections.push("## Recent Activity\n".into());
            for ranked in &recent {
                let entry = &ranked.entry;
                let age = format_age(now, entry.timestamp);
                sections.push(format!(
                    "- [{}] **{}** ({}, importance: {:.1})",
                    entry.memory_type,
                    entry.title,
                    age,
                    ranked.effective(now)
                ));
            }
            sections.push(String::new());
        }

        // Knowledge Graph summary (top entities with connections)
        if let Ok((entity_count, edge_count)) = storage.graph_stats()
            && entity_count > 0
        {
            sections.push("## Knowledge Graph\n".into());
            if let Ok(top_entities) = storage.list_entities(10) {
                for (name, etype, count) in &top_entities {
                    if *count >= 2 {
                        // Only show entities mentioned 2+ times
                        sections.push(format!("- **{}** ({}, {} mentions)", name, etype, count));

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
                            sections
                                .push(format!("  → connected to: {}", neighbor_names.join(", ")));
                        }
                    }
                }
            }
            sections.push(format!(
                "\n_Graph: {} entities, {} edges_\n",
                entity_count, edge_count
            ));
        }

        // Stats
        if let Ok(stats) = storage.stats() {
            sections.push(format!(
                "---\n_Total memories: {} | Generated by mnemonic_",
                stats.total
            ));
        }

        let content = sections.join("\n");

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
