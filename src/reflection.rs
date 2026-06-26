//! Reflection / consolidation — clusters near-duplicate memories and (in
//! apply mode) creates a canonical memory that supersedes its sources.
//!
//! Safety properties (Phase 5 contract):
//!   - Source memories are NEVER deleted. They're marked `superseded_by`
//!     and remain queryable for audit via `Storage::sources_for_canonical`.
//!   - Apply mode is atomic per cluster (single SQLite transaction).
//!   - Dry-run produces a `ReflectionPlan` with no DB writes.
//!   - Retrieval (`recent`, `search`, `find_similar`, `recent_ranked`)
//!     filters out superseded memories by default.
//!
//! Clustering is union-find over pairwise cosine similarity above
//! `threshold`. O(N²) — fine for N ≤ a few thousand active memories.
//!
//! Canonical synthesis is rule-based by default: longest content wins,
//! titles concatenated. An LLM synthesizer is a Phase 5+ swap behind
//! the same `Synthesizer` trait.

use anyhow::Result;
use serde::Serialize;
use std::sync::Arc;
use tracing::{debug, info};

use crate::config::Config;
use crate::embedding::{
    Embedder, Embedding, cosine_similarity, create_embedder, embedding_from_bytes,
};
use crate::event::{EventSource, MemoryEntry, MemoryType};
use crate::storage::Storage;

/// Default cosine threshold for clustering — empirically near-duplicate
/// memories on MiniLM 384-dim sit around 0.85+.
pub const DEFAULT_THRESHOLD: f32 = 0.85;

/// Hard cap on cluster size. Bigger clusters usually indicate a too-low
/// threshold; refuse to silently collapse a hundred entries into one.
pub const MAX_CLUSTER_SIZE: usize = 12;

/// Maximum number of active memories examined per run. O(N²) clustering
/// stays cheap up to ~2k entries.
pub const MAX_POOL: usize = 2_000;

#[derive(Debug, Clone, Serialize)]
pub struct ReflectionOptions {
    pub mode: Mode,
    pub threshold: f32,
    pub limit: Option<usize>,
    pub since_days: Option<i64>,
}

impl Default for ReflectionOptions {
    fn default() -> Self {
        Self {
            mode: Mode::DryRun,
            threshold: DEFAULT_THRESHOLD,
            limit: None,
            since_days: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Mode {
    DryRun,
    Apply,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReflectionPlan {
    pub run_id: String,
    pub mode: Mode,
    pub threshold: f32,
    pub pool_size: usize,
    pub clusters: Vec<PlannedCluster>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlannedCluster {
    /// Source memory ids in cluster, ordered by cosine to centroid DESC.
    pub source_ids: Vec<String>,
    /// Cosine of each source to the proposed canonical embedding.
    pub cosines: Vec<f32>,
    /// Title the apply phase would create.
    pub draft_title: String,
    /// Synthesized content the apply phase would create.
    pub draft_content: String,
    /// Did apply mode actually write? Always false in dry-run.
    #[serde(default)]
    pub applied: bool,
    /// id of the canonical memory that was inserted (only in apply mode).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub canonical_id: Option<String>,
}

/// Trait so we can swap rule-based synthesis for an LLM call later
/// without changing the orchestration.
pub trait Synthesizer: Send + Sync {
    fn synthesize(&self, members: &[MemoryEntry]) -> (String, String);
    fn name(&self) -> &'static str;
}

/// Rule-based default: title is the most-mentioned project token from
/// member titles, fallback to longest source title. Content concatenates
/// distinct paragraphs with a `→` separator and lists source ids.
pub struct RuleSynthesizer;

impl Synthesizer for RuleSynthesizer {
    fn name(&self) -> &'static str {
        "rule"
    }
    fn synthesize(&self, members: &[MemoryEntry]) -> (String, String) {
        // Title: longest unique title (proxy for "most descriptive").
        let title = members
            .iter()
            .map(|m| m.title.clone())
            .max_by_key(|t| t.chars().count())
            .unwrap_or_else(|| "Consolidated memory".into());

        // Content: dedup paragraphs by first 60 chars, preserve order of
        // first appearance, append source provenance footer.
        use std::collections::HashSet;
        let mut seen = HashSet::new();
        let mut parts: Vec<String> = Vec::new();
        for m in members {
            for paragraph in m.content.split("\n\n") {
                let key: String = paragraph.chars().take(60).collect();
                if !key.trim().is_empty() && seen.insert(key) {
                    parts.push(paragraph.trim().to_string());
                }
            }
        }
        let body = parts.join("\n\n");
        let sources: Vec<&str> = members.iter().map(|m| m.id.as_str()).collect();
        let content = format!(
            "{body}\n\n---\n_Consolidated from {} memories: {}_",
            members.len(),
            sources
                .iter()
                .map(|s| s.get(..8).unwrap_or(s))
                .collect::<Vec<_>>()
                .join(", ")
        );
        (title, content)
    }
}

/// Top-level entry point. Pulls active memories with embeddings, runs
/// clustering, builds a plan. In apply mode, writes canonicals and
/// supersedes sources.
pub fn run_reflection(
    storage: &Arc<Storage>,
    config: &Config,
    opts: &ReflectionOptions,
) -> Result<ReflectionPlan> {
    let synthesizer: Box<dyn Synthesizer> = Box::new(RuleSynthesizer);

    // Dry-run must not write to the DB (Codex P2a). We synthesize a
    // synthetic run id and skip the reflection_runs INSERT entirely;
    // finalize_reflection_run at the end is also gated by mode.
    let run_id = match opts.mode {
        Mode::Apply => storage.begin_reflection_run("apply", opts.threshold, synthesizer.name())?,
        Mode::DryRun => format!("dry-run-{}", uuid::Uuid::new_v4()),
    };

    // Pull active embeddings. Filter by recency/limit if requested.
    let pool = active_pool(storage, opts.limit.unwrap_or(MAX_POOL), opts.since_days)?;
    let pool_size = pool.len();
    info!(
        "Reflection {:?}: scanning {pool_size} active memories @ threshold {:.2}",
        opts.mode, opts.threshold
    );

    let clusters_idx = cluster_by_cosine(&pool, opts.threshold);
    let mut planned = Vec::new();
    let mut applied = 0usize;

    let embedder: Option<Box<dyn Embedder>> = match opts.mode {
        Mode::Apply => create_embedder().ok(),
        Mode::DryRun => None,
    };
    let _ = config; // reserved for future LLM-backed synthesizer

    for cluster_idxs in &clusters_idx {
        if cluster_idxs.len() < 2 || cluster_idxs.len() > MAX_CLUSTER_SIZE {
            continue;
        }
        let members: Vec<&ActiveEntry> = cluster_idxs.iter().map(|&i| &pool[i]).collect();
        let centroid = mean_embedding(&members);
        let mut cosines: Vec<f32> = members
            .iter()
            .map(|m| cosine_similarity(&m.embedding, &centroid))
            .collect();

        // Sort members by cosine DESC for stable serialization.
        let mut ordered: Vec<(&&ActiveEntry, f32)> =
            members.iter().zip(cosines.drain(..)).collect();
        ordered.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        let member_entries: Vec<MemoryEntry> =
            ordered.iter().map(|(m, _)| m.entry.clone()).collect();
        let source_ids: Vec<String> = ordered.iter().map(|(m, _)| m.entry.id.clone()).collect();
        let cosines: Vec<f32> = ordered.iter().map(|(_, c)| *c).collect();

        let (title, content) = synthesizer.synthesize(&member_entries);

        let mut cluster_record = PlannedCluster {
            source_ids: source_ids.clone(),
            cosines: cosines.clone(),
            draft_title: title.clone(),
            draft_content: content.clone(),
            applied: false,
            canonical_id: None,
        };

        if matches!(opts.mode, Mode::Apply) {
            let canonical_id = uuid::Uuid::new_v4().to_string();
            // Borrow the highest-importance memory type from the cluster so
            // consolidated decisions stay decisions.
            let memory_type = member_entries
                .iter()
                .map(|m| m.memory_type.clone())
                .max_by_key(type_priority)
                .unwrap_or(MemoryType::Note);

            let canonical_entry = MemoryEntry {
                id: canonical_id.clone(),
                timestamp: chrono::Utc::now(),
                title,
                content: content.clone(),
                memory_type,
                tags: collect_tags(&member_entries),
                source: EventSource::Manual,
                importance: member_entries
                    .iter()
                    .map(|m| m.importance)
                    .fold(0.0_f32, f32::max)
                    .max(0.7),
                metadata: serde_json::json!({
                    "consolidated_from": source_ids,
                    "run_id": run_id,
                }),
            };

            let canonical_embedding = embedder
                .as_ref()
                .and_then(|e| {
                    e.embed(&format!("{}\n{}", canonical_entry.title, content))
                        .ok()
                })
                .unwrap_or(centroid);

            let cluster_pairs: Vec<(String, f32)> = source_ids
                .iter()
                .zip(cosines.iter())
                .map(|(id, c)| (id.clone(), *c))
                .collect();

            let canonical_id_written = storage.apply_reflection(
                &run_id,
                &canonical_entry,
                Some(&canonical_embedding),
                &cluster_pairs,
            )?;
            cluster_record.applied = true;
            cluster_record.canonical_id = Some(canonical_id_written);
            applied += 1;
        }

        planned.push(cluster_record);
    }

    if matches!(opts.mode, Mode::Apply) {
        storage.finalize_reflection_run(&run_id, planned.len(), applied)?;
    }
    debug!(
        "Reflection plan: {} clusters, {applied} applied",
        planned.len()
    );

    Ok(ReflectionPlan {
        run_id,
        mode: opts.mode,
        threshold: opts.threshold,
        pool_size,
        clusters: planned,
    })
}

struct ActiveEntry {
    entry: MemoryEntry,
    embedding: Embedding,
}

fn active_pool(
    storage: &Storage,
    limit: usize,
    since_days: Option<i64>,
) -> Result<Vec<ActiveEntry>> {
    use rusqlite::params_from_iter;

    let conn = storage
        .conn
        .lock()
        .map_err(|e| anyhow::anyhow!("lock: {e}"))?;

    let mut sql = String::from(
        "SELECT id, timestamp, title, content, memory_type, tags, source, importance, metadata, embedding
         FROM memories
         WHERE embedding IS NOT NULL AND superseded_by IS NULL",
    );
    let mut params_vec: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    if let Some(days) = since_days {
        sql.push_str(" AND timestamp >= datetime('now', ?1)");
        params_vec.push(Box::new(format!("-{days} days")));
    }
    sql.push_str(" ORDER BY timestamp DESC LIMIT ?");
    sql.push_str(&(params_vec.len() + 1).to_string());
    params_vec.push(Box::new(limit as i64));

    let mut stmt = conn.prepare(&sql)?;
    let param_refs: Vec<&dyn rusqlite::ToSql> = params_vec.iter().map(|b| b.as_ref()).collect();

    let entries: Vec<ActiveEntry> = stmt
        .query_map(params_from_iter(param_refs.iter()), |row| {
            let id: String = row.get(0)?;
            let timestamp: String = row.get(1)?;
            let title: String = row.get(2)?;
            let content: String = row.get(3)?;
            let memory_type: String = row.get(4)?;
            let tags: String = row.get(5)?;
            let source: String = row.get(6)?;
            let importance: f64 = row.get(7)?;
            let metadata: String = row.get(8)?;
            let blob: Vec<u8> = row.get(9)?;
            Ok((
                id,
                timestamp,
                title,
                content,
                memory_type,
                tags,
                source,
                importance,
                metadata,
                blob,
            ))
        })?
        .filter_map(|r| r.ok())
        .filter_map(|t| {
            let memory_type = match t.4.as_str() {
                "decision" => MemoryType::Decision,
                "feedback" => MemoryType::Feedback,
                "session_summary" => MemoryType::SessionSummary,
                "security" => MemoryType::Security,
                _ => MemoryType::Note,
            };
            let source: EventSource = serde_json::from_str(&t.6).unwrap_or(EventSource::Manual);
            let tags: Vec<String> = serde_json::from_str(&t.5).unwrap_or_default();
            let metadata = serde_json::from_str(&t.8).unwrap_or(serde_json::Value::Null);
            let timestamp = chrono::DateTime::parse_from_rfc3339(&t.1)
                .map(|dt| dt.with_timezone(&chrono::Utc))
                .ok()?;
            Some(ActiveEntry {
                entry: MemoryEntry {
                    id: t.0,
                    timestamp,
                    title: t.2,
                    content: t.3,
                    memory_type,
                    tags,
                    source,
                    importance: t.7 as f32,
                    metadata,
                },
                embedding: embedding_from_bytes(&t.9),
            })
        })
        .collect();

    Ok(entries)
}

/// Union-find clustering on pairwise cosine. Returns clusters as index lists.
fn cluster_by_cosine(pool: &[ActiveEntry], threshold: f32) -> Vec<Vec<usize>> {
    let n = pool.len();
    let mut parent: Vec<usize> = (0..n).collect();

    fn find(parent: &mut [usize], i: usize) -> usize {
        let mut root = i;
        while parent[root] != root {
            root = parent[root];
        }
        let mut cur = i;
        while parent[cur] != root {
            let next = parent[cur];
            parent[cur] = root;
            cur = next;
        }
        root
    }
    fn union(parent: &mut [usize], a: usize, b: usize) {
        let ra = find(parent, a);
        let rb = find(parent, b);
        if ra != rb {
            parent[ra] = rb;
        }
    }

    // Skip same-dimension check; all embeddings are produced by the same
    // embedder in steady-state. If dimensions mismatch (mid-migration),
    // cosine_similarity already returns 0 in the embedding module.
    for i in 0..n {
        for j in (i + 1)..n {
            let sim = cosine_similarity(&pool[i].embedding, &pool[j].embedding);
            if sim >= threshold {
                union(&mut parent, i, j);
            }
        }
    }

    let mut groups: std::collections::HashMap<usize, Vec<usize>> = std::collections::HashMap::new();
    for i in 0..n {
        let root = find(&mut parent, i);
        groups.entry(root).or_default().push(i);
    }
    groups.into_values().filter(|g| g.len() >= 2).collect()
}

/// Mean of N embeddings (assumed same dimension). Returns the first
/// embedding if computation fails — better than panicking.
fn mean_embedding(members: &[&ActiveEntry]) -> Embedding {
    if members.is_empty() {
        return Vec::new();
    }
    let dim = members[0].embedding.len();
    let mut acc = vec![0.0_f32; dim];
    let mut counted = 0usize;
    for m in members {
        if m.embedding.len() != dim {
            continue;
        }
        for (i, v) in m.embedding.iter().enumerate() {
            acc[i] += v;
        }
        counted += 1;
    }
    if counted == 0 {
        return members[0].embedding.clone();
    }
    let inv = 1.0 / counted as f32;
    for v in &mut acc {
        *v *= inv;
    }
    // Re-normalize to unit length so cosine math stays consistent.
    let norm: f32 = acc.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for v in &mut acc {
            *v /= norm;
        }
    }
    acc
}

fn type_priority(t: &MemoryType) -> u8 {
    match t {
        MemoryType::Feedback => 4,
        MemoryType::Security => 3,
        MemoryType::Decision => 2,
        MemoryType::SessionSummary => 1,
        MemoryType::Note => 0,
    }
}

fn collect_tags(members: &[MemoryEntry]) -> Vec<String> {
    use std::collections::BTreeSet;
    let mut tags: BTreeSet<String> = BTreeSet::new();
    for m in members {
        for t in &m.tags {
            tags.insert(t.clone());
        }
    }
    tags.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{EventSource, MemoryType};
    use chrono::Utc;

    /// Codex P2a: dry-run must not write to reflection_runs. We can't
    /// inspect the table directly here without exposing internals, so we
    /// verify the contract indirectly: the run_id of a dry-run starts
    /// with "dry-run-" (synthetic, not a UUID from begin_reflection_run).
    #[test]
    fn dry_run_run_id_is_synthetic_not_persisted() {
        let dir = std::env::temp_dir().join(format!(
            "mnemonic-reflect-dry-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let storage = Arc::new(crate::storage::Storage::open(&dir.join("memory.db")).unwrap());
        let config = crate::config::Config::default();
        let opts = ReflectionOptions::default();
        let plan = run_reflection(&storage, &config, &opts).unwrap();
        assert!(
            plan.run_id.starts_with("dry-run-"),
            "dry-run id must be synthetic, got '{}'",
            plan.run_id
        );
    }

    fn entry(id: &str, title: &str) -> ActiveEntry {
        ActiveEntry {
            entry: MemoryEntry {
                id: id.to_string(),
                timestamp: Utc::now(),
                title: title.to_string(),
                content: format!("content of {title}"),
                memory_type: MemoryType::Note,
                tags: vec![],
                source: EventSource::Manual,
                importance: 0.5,
                metadata: serde_json::Value::Null,
            },
            embedding: vec![0.0; 4],
        }
    }

    #[test]
    fn cluster_by_cosine_groups_similar() {
        let mut pool = vec![entry("a", "a"), entry("b", "b"), entry("c", "c")];
        // a and b nearly parallel; c orthogonal.
        pool[0].embedding = vec![1.0, 0.0, 0.0, 0.0];
        pool[1].embedding = vec![0.98, 0.05, 0.05, 0.05];
        pool[2].embedding = vec![0.0, 1.0, 0.0, 0.0];

        let groups = cluster_by_cosine(&pool, 0.9);
        assert_eq!(groups.len(), 1, "expected one cluster, got {groups:?}");
        let g = &groups[0];
        assert_eq!(g.len(), 2);
        // Check that the cluster is {0, 1}.
        let ids: std::collections::HashSet<_> = g.iter().copied().collect();
        assert!(ids.contains(&0) && ids.contains(&1));
    }

    #[test]
    fn cluster_by_cosine_chains_transitively() {
        // a-b similar, b-c similar, a-c NOT. Union-find should still
        // merge them all via b.
        let mut pool = vec![entry("a", "a"), entry("b", "b"), entry("c", "c")];
        pool[0].embedding = vec![1.0, 0.0, 0.0, 0.0];
        pool[1].embedding = vec![0.95, 0.32, 0.0, 0.0];
        pool[2].embedding = vec![0.6, 0.8, 0.0, 0.0];
        // sim(a,b) ≈ 0.95; sim(b,c) ≈ 0.83; sim(a,c) ≈ 0.6
        let groups = cluster_by_cosine(&pool, 0.8);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].len(), 3);
    }

    #[test]
    fn cluster_threshold_isolates_unrelated() {
        let mut pool = vec![entry("a", "a"), entry("b", "b"), entry("c", "c")];
        pool[0].embedding = vec![1.0, 0.0, 0.0, 0.0];
        pool[1].embedding = vec![0.0, 1.0, 0.0, 0.0];
        pool[2].embedding = vec![0.0, 0.0, 1.0, 0.0];
        let groups = cluster_by_cosine(&pool, 0.9);
        assert!(groups.is_empty(), "orthogonal vectors must not cluster");
    }

    #[test]
    fn rule_synthesizer_picks_longest_title_and_dedups_paragraphs() {
        let make = |id: &str, title: &str, content: &str| MemoryEntry {
            id: id.into(),
            timestamp: Utc::now(),
            title: title.into(),
            content: content.into(),
            memory_type: MemoryType::Note,
            tags: vec![],
            source: EventSource::Manual,
            importance: 0.5,
            metadata: serde_json::Value::Null,
        };
        let members = vec![
            make("a", "Short", "Same intro\n\nUnique a"),
            make(
                "b",
                "This is the longest title in the cluster",
                "Same intro\n\nUnique b",
            ),
        ];
        let s = RuleSynthesizer;
        let (title, content) = s.synthesize(&members);
        assert_eq!(title, "This is the longest title in the cluster");
        assert!(content.contains("Same intro"));
        assert!(content.contains("Unique a"));
        assert!(content.contains("Unique b"));
        assert!(content.contains("Consolidated from 2 memories"));
    }

    #[test]
    fn type_priority_orders_feedback_over_note() {
        assert!(type_priority(&MemoryType::Feedback) > type_priority(&MemoryType::Decision));
        assert!(type_priority(&MemoryType::Decision) > type_priority(&MemoryType::Note));
    }

    #[test]
    fn mean_embedding_normalizes_to_unit() {
        let mut a = entry("a", "a");
        let mut b = entry("b", "b");
        a.embedding = vec![3.0, 0.0, 0.0, 0.0];
        b.embedding = vec![0.0, 4.0, 0.0, 0.0];
        let m = mean_embedding(&[&a, &b]);
        let norm: f32 = m.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-5, "expected unit norm, got {norm}");
    }
}
