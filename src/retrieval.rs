//! Hybrid retrieval: BM25 (FTS5) + vector (HNSW) + graph hop, fused via
//! Reciprocal Rank Fusion (RRF).
//!
//! Why RRF: rank-based fusion is robust to score scale mismatch between
//! retrievers (BM25 raw score vs cosine similarity vs graph weight). The
//! classic constant k=60 dampens the head of each list so a single retriever
//! can't dominate the merged ranking.
//!
//! Pipeline:
//! 1. FTS5 returns `limit_per_retriever` results ordered by BM25.
//! 2. HNSW vector search returns same.
//! 3. Optional graph hop: extract seed entities from top vector hits,
//!    expand 1-hop along edges with weight >= `min_edge_weight`, fetch
//!    memories linked to seed+neighbor entities.
//! 4. RRF merge → fused rank list.
//! 5. `touch_access` every returned id so usage feeds into decay scoring.

use anyhow::Result;
use std::collections::{HashMap, HashSet};

use crate::embedding::Embedder;
use crate::event::MemoryEntry;
use crate::storage::Storage;

/// Default fusion constant — industry-standard RRF k=60.
pub const RRF_K: f32 = 60.0;

/// How many results each retriever is asked for before fusion.
pub const DEFAULT_PER_RETRIEVER: usize = 20;

/// Edge weight threshold for graph hop expansion.
pub const DEFAULT_MIN_EDGE_WEIGHT: f32 = 1.0;

/// Which retrievers contributed to a hit. Used for source citations.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HitSources {
    pub fts: bool,
    pub vector: bool,
    pub graph: bool,
}

impl HitSources {
    fn label(&self) -> String {
        let mut parts: Vec<&str> = Vec::new();
        if self.fts {
            parts.push("bm25");
        }
        if self.vector {
            parts.push("vector");
        }
        if self.graph {
            parts.push("graph");
        }
        parts.join("+")
    }
}

/// One fused retrieval result.
#[derive(Debug, Clone)]
pub struct HybridHit {
    pub entry: MemoryEntry,
    pub score: f32,
    pub sources: HitSources,
}

impl HybridHit {
    /// Human-readable provenance tag, e.g. "bm25+vector".
    pub fn source_label(&self) -> String {
        self.sources.label()
    }
}

/// Default weight of the graph-hop retriever in RRF fusion.
/// Lowering from 1.0 (equal to fts/vector) because eval surfaced that
/// short queries get drowned by graph expansion otherwise. Measured on
/// the 12-query seed against ~165 memories:
///
/// | weight             | recall@5 | recall@20 | MRR   |
/// |--------------------|----------|-----------|-------|
/// | 1.0 (old)          | 0.917    | 1.000     | 0.790 |
/// | 0.4 (this default) | 0.917    | 1.000     | 0.887 |
/// | 0.0 (no-graph-hop) | 1.000    | 1.000     | 0.944 |
///
/// 0.4 improves MRR and reduces graph noise enough that good results
/// climb to the top, but doesn't fully close the remaining recall@5 gap
/// — that's still ~1 query out of 12 where graph contribution drowns
/// the right answer. Lowering further is on the table once the seed
/// grows past 25–30 queries and the regression is statistically real.
pub const DEFAULT_GRAPH_WEIGHT: f32 = 0.4;

/// How many fused candidates to re-score with the cross-encoder before
/// truncating to `limit`. Bigger = more chance the right doc gets pulled
/// up from rank 6-10 into the top-5, but each candidate is one model
/// forward pass.
///
/// MEASURED on M1 Mac, fastembed 4.9.1 CPU ONNX (no CoreML accel) with
/// jina-reranker-v2-base-multilingual: ~25s for 30 docs, ~8s for 10.
/// That's MUCH slower than the docs claim (which I parroted earlier and
/// shouldn't have). At top-10 the latency is tolerable for batch eval
/// but still too high for interactive retrieval, so:
///
/// - Default `rerank_top_n` is 10 — usable for `mnemonic eval --rerank`.
/// - Default `HybridOptions::rerank` stays `false` everywhere. Opt in
///   per call. Don't wire this into the daemon's hot path until we
///   either (a) ship Metal-accelerated ONNX, or (b) swap to a smaller
///   reranker (jina-v1-turbo-en) for English-mostly users.
pub const DEFAULT_RERANK_TOP_N: usize = 10;

/// Options for `hybrid_search`.
#[derive(Debug, Clone)]
pub struct HybridOptions {
    pub limit: usize,
    pub per_retriever: usize,
    pub with_graph_hop: bool,
    pub min_edge_weight: f32,
    /// Multiplier on graph-hop's RRF contribution. fts and vector each
    /// contribute `1.0 / (k + rank)`; graph contributes
    /// `graph_weight * 1.0 / (k + rank)`. Keep ≤ 1.0.
    pub graph_weight: f32,
    /// Bump `access_count` / `last_accessed_at` on every returned memory.
    /// Default true for production retrieval (feeds the decay scorer).
    /// Set false for eval / debugging / read-only inspection — otherwise
    /// re-running `mnemonic eval` would shift rankings just by measuring.
    pub touch_access: bool,
    /// If `true`, run a cross-encoder rerank pass over the top
    /// `rerank_top_n` fused candidates before truncating to `limit`.
    /// Requires a reranker passed via `hybrid_search_with_rerank`; the
    /// plain `hybrid_search` ignores this flag.
    pub rerank: bool,
    /// Window size handed to the reranker. Ignored when `rerank` is false
    /// or no reranker is available.
    pub rerank_top_n: usize,
}

impl Default for HybridOptions {
    fn default() -> Self {
        Self {
            limit: 10,
            per_retriever: DEFAULT_PER_RETRIEVER,
            with_graph_hop: true,
            min_edge_weight: DEFAULT_MIN_EDGE_WEIGHT,
            graph_weight: DEFAULT_GRAPH_WEIGHT,
            touch_access: true,
            rerank: false,
            rerank_top_n: DEFAULT_RERANK_TOP_N,
        }
    }
}

/// Run hybrid retrieval without cross-encoder rerank. Embedding is computed
/// once and shared between vector and (indirectly) graph stages.
///
/// If `opts.rerank` is set on this entry point, it's silently ignored — use
/// `hybrid_search_with_rerank` and pass an actual reranker. This split lets
/// callers that don't want to pay reranker init cost (e.g. tests, internal
/// dedup checks) keep using the simpler signature.
pub fn hybrid_search(
    storage: &Storage,
    embedder: &dyn Embedder,
    query: &str,
    opts: &HybridOptions,
) -> Result<Vec<HybridHit>> {
    // --- Retriever 1: FTS5 / BM25 ---
    // FTS5 treats unquoted tokens with `-` as NOT; sanitize before MATCH.
    let fts_query = sanitize_fts_query(query);
    let fts_results = if fts_query.is_empty() {
        Vec::new()
    } else if opts.touch_access {
        storage
            .search(&fts_query, opts.per_retriever)
            .unwrap_or_default()
    } else {
        storage
            .search_no_touch(&fts_query, opts.per_retriever)
            .unwrap_or_default()
    };

    // --- Retriever 2: vector / HNSW ---
    let vec_results: Vec<MemoryEntry> = match embedder.embed_query(query) {
        Ok(emb) => {
            let raw = if opts.touch_access {
                storage.find_similar(&emb, opts.per_retriever)
            } else {
                storage.find_similar_no_touch(&emb, opts.per_retriever)
            };
            raw.map(|hits| hits.into_iter().map(|(e, _)| e).collect())
                .unwrap_or_default()
        }
        Err(_) => Vec::new(),
    };

    // --- Retriever 3 (optional): graph hop seeded from vector top-K ---
    let graph_results: Vec<MemoryEntry> = if opts.with_graph_hop {
        graph_hop(storage, &vec_results, opts).unwrap_or_default()
    } else {
        Vec::new()
    };

    // --- RRF fusion ---
    let fused = rrf_fuse(
        &fts_results,
        &vec_results,
        &graph_results,
        opts.graph_weight,
    );

    // Take top-N.
    let top: Vec<HybridHit> = fused.into_iter().take(opts.limit).collect();

    // Touch every fused winner so usage feeds the decay scorer. Skipped
    // for read-only callers (eval / debugging) — otherwise measuring
    // retrieval would change retrieval.
    if opts.touch_access {
        let ids: Vec<&str> = top.iter().map(|h| h.entry.id.as_str()).collect();
        let _ = storage.touch_access(&ids);
    }

    Ok(top)
}

/// Same as `hybrid_search`, but inserts a cross-encoder rerank pass between
/// RRF fusion and final truncation. When `opts.rerank` is false, this is
/// identical to `hybrid_search`. When true:
///
/// 1. Fuse retrievers via RRF, take top `opts.rerank_top_n` candidates.
/// 2. Build (query, doc-text) pairs from those candidates.
/// 3. Score each pair via the reranker (e.g. Jina v2 multilingual ONNX).
/// 4. Sort by reranker score, take top `opts.limit`.
///
/// Re-ordering happens entirely on the rerank window; candidates beyond
/// `rerank_top_n` are kept at their fused rank in case the window doesn't
/// fill `limit`. The `HybridHit.score` field is rewritten to the rerank
/// score on documents that were reranked, so downstream callers can show
/// a meaningful confidence number; `sources` is preserved.
///
/// Doc text fed to the reranker is `"<title>\n<content>"` truncated to
/// 2KB — the reranker's tokenizer handles longer inputs but truncation
/// keeps the forward pass tight and matches what the embedder sees.
pub fn hybrid_search_with_rerank(
    storage: &Storage,
    embedder: &dyn Embedder,
    reranker: Option<&dyn crate::reranker::Reranker>,
    query: &str,
    opts: &HybridOptions,
) -> Result<Vec<HybridHit>> {
    // Short-circuit to the simpler path whenever rerank can't or won't run.
    // Keeps test surface small and avoids an extra Vec allocation on the
    // common path.
    if !opts.rerank || reranker.is_none() {
        return hybrid_search(storage, embedder, query, opts);
    }
    let reranker = reranker.expect("checked above");

    // Run retrievers with a `rerank_top_n`-sized limit instead of `limit`,
    // so the reranker has enough candidates to actually move things around.
    let stage1_opts = HybridOptions {
        limit: opts.rerank_top_n.max(opts.limit),
        // Don't touch_access during the stage-1 fuse — we'll do it once at
        // the end on the final winners. Otherwise we'd bump 30 memories'
        // counters every time the user runs a single query, drowning the
        // decay signal.
        touch_access: false,
        // Same rerank=false flag so we don't recurse.
        rerank: false,
        ..opts.clone()
    };
    let candidates = hybrid_search(storage, embedder, query, &stage1_opts)?;
    if candidates.len() <= 1 {
        // Nothing meaningful to rerank.
        if opts.touch_access {
            let ids: Vec<&str> = candidates.iter().map(|h| h.entry.id.as_str()).collect();
            let _ = storage.touch_access(&ids);
        }
        return Ok(candidates);
    }

    // Build the (id-stable) text view of each candidate for scoring.
    const RERANK_DOC_TRUNCATE: usize = 2048;
    let doc_strings: Vec<String> = candidates
        .iter()
        .map(|h| {
            let raw = format!("{}\n{}", h.entry.title, h.entry.content);
            if raw.len() > RERANK_DOC_TRUNCATE {
                // Char-boundary-safe truncation. Slicing by byte would
                // panic mid-UTF-8.
                raw.chars().take(RERANK_DOC_TRUNCATE).collect()
            } else {
                raw
            }
        })
        .collect();
    let doc_refs: Vec<&str> = doc_strings.iter().map(|s| s.as_str()).collect();

    let scored = match reranker.rerank(query, &doc_refs) {
        Ok(s) => s,
        Err(e) => {
            // A rerank failure shouldn't poison the query. Fall back to
            // the RRF order, truncate to `limit`.
            tracing::warn!("Reranker failed mid-query, falling back to RRF order: {e}");
            let top: Vec<HybridHit> = candidates.into_iter().take(opts.limit).collect();
            if opts.touch_access {
                let ids: Vec<&str> = top.iter().map(|h| h.entry.id.as_str()).collect();
                let _ = storage.touch_access(&ids);
            }
            return Ok(top);
        }
    };

    // Re-thread scored indices back to candidate hits, in scored order.
    // `candidates[scored[i].index]` is the i-th reranker pick.
    let mut reranked: Vec<HybridHit> = Vec::with_capacity(scored.len());
    for s in scored {
        if let Some(mut hit) = candidates.get(s.index).cloned() {
            // Replace fused RRF score with the reranker's so downstream
            // callers see a real signal. We keep `sources` untouched —
            // a user looking at provenance still wants to know it was a
            // BM25+vector match originally, not just "reranker liked it".
            hit.score = s.score;
            reranked.push(hit);
        }
    }
    let top: Vec<HybridHit> = reranked.into_iter().take(opts.limit).collect();

    if opts.touch_access {
        let ids: Vec<&str> = top.iter().map(|h| h.entry.id.as_str()).collect();
        let _ = storage.touch_access(&ids);
    }
    Ok(top)
}

/// 1-hop weighted graph expansion seeded by entities from `seed_memories`.
fn graph_hop(
    storage: &Storage,
    seed_memories: &[MemoryEntry],
    opts: &HybridOptions,
) -> Result<Vec<MemoryEntry>> {
    if seed_memories.is_empty() {
        return Ok(Vec::new());
    }

    // Take top-3 seeds — diminishing returns past that, and graph can explode.
    let seeds: Vec<&MemoryEntry> = seed_memories.iter().take(3).collect();

    // Collect entity names linked to seed memories.
    let mut seed_entities: HashSet<String> = HashSet::new();
    for m in &seeds {
        if let Ok(names) = storage.entity_names_for_memory(&m.id) {
            seed_entities.extend(names.into_iter().map(|n| n.to_lowercase()));
        }
    }
    if seed_entities.is_empty() {
        return Ok(Vec::new());
    }

    // 1-hop expand along edges above weight threshold.
    let seed_vec: Vec<&str> = seed_entities.iter().map(|s| s.as_str()).collect();
    let neighbors = storage
        .weighted_neighbors(&seed_vec, opts.min_edge_weight)
        .unwrap_or_default();

    // Union of seed + neighbor entity names.
    let mut all_entities: Vec<String> = seed_entities.into_iter().collect();
    all_entities.extend(neighbors);
    let all_refs: Vec<&str> = all_entities.iter().map(|s| s.as_str()).collect();

    // Memories linked to any of those entities.
    let memory_ids = storage
        .memory_ids_for_entities(&all_refs, opts.per_retriever)
        .unwrap_or_default();

    let mut out = Vec::with_capacity(memory_ids.len());
    for id in &memory_ids {
        if let Ok(Some(entry)) = storage.get_by_id(id) {
            out.push(entry);
        }
    }
    Ok(out)
}

/// Pure RRF fusion. Each retriever provides a ranked list (rank 0 = top).
/// Score per doc = Σ weight * 1 / (k + rank). Higher = better.
/// FTS and vector contribute at weight 1.0; graph contributes at
/// `graph_weight` (default 0.4 — see `DEFAULT_GRAPH_WEIGHT`).
fn rrf_fuse(
    fts: &[MemoryEntry],
    vector: &[MemoryEntry],
    graph: &[MemoryEntry],
    graph_weight: f32,
) -> Vec<HybridHit> {
    let mut scores: HashMap<String, f32> = HashMap::new();
    let mut sources: HashMap<String, HitSources> = HashMap::new();
    let mut entries: HashMap<String, MemoryEntry> = HashMap::new();

    let mut add = |list: &[MemoryEntry], weight: f32, mark: fn(&mut HitSources)| {
        for (rank, entry) in list.iter().enumerate() {
            let contribution = weight / (RRF_K + rank as f32);
            *scores.entry(entry.id.clone()).or_insert(0.0) += contribution;
            entries
                .entry(entry.id.clone())
                .or_insert_with(|| entry.clone());
            let s = sources.entry(entry.id.clone()).or_default();
            mark(s);
        }
    };

    add(fts, 1.0, |s| s.fts = true);
    add(vector, 1.0, |s| s.vector = true);
    add(graph, graph_weight, |s| s.graph = true);

    let mut hits: Vec<HybridHit> = scores
        .into_iter()
        .filter_map(|(id, score)| {
            let entry = entries.remove(&id)?;
            let srcs = sources.remove(&id).unwrap_or_default();
            Some(HybridHit {
                entry,
                score,
                sources: srcs,
            })
        })
        .collect();

    hits.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    hits
}

/// FTS5 has reserved tokens (`-`, `"`, `:`, AND/OR/NOT). The safest minimal
/// sanitizer for free-text queries is to quote each whitespace-separated
/// token, escaping embedded quotes. This loses operator support but is
/// the right default for context-injection retrieval.
pub(crate) fn sanitize_fts_query(q: &str) -> String {
    q.split_whitespace()
        .filter(|t| t.chars().any(|c| c.is_alphanumeric()))
        .map(|t| {
            let escaped = t.replace('"', "\"\"");
            format!("\"{escaped}\"")
        })
        .collect::<Vec<_>>()
        .join(" OR ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{EventSource, MemoryType};
    use chrono::Utc;

    fn mk(id: &str, title: &str) -> MemoryEntry {
        MemoryEntry {
            id: id.to_string(),
            timestamp: Utc::now(),
            title: title.into(),
            content: title.into(),
            memory_type: MemoryType::Note,
            tags: vec![],
            source: EventSource::Manual,
            importance: 0.5,
            metadata: serde_json::Value::Null,
        }
    }

    #[test]
    fn rrf_promotes_docs_found_by_multiple_retrievers() {
        let a = mk("a", "alpha");
        let b = mk("b", "bravo");
        let c = mk("c", "charlie");

        // a appears in all three retrievers at the BOTTOM (rank 1) of each.
        // b is rank 0 in FTS only. c is rank 0 in vector only.
        // RRF should promote `a` over single-retriever winners.
        let fts = vec![b.clone(), a.clone()];
        let vec = vec![c.clone(), a.clone()];
        let graph = vec![a.clone()];

        // Use weight 1.0 here to keep the test focused on RRF mechanics —
        // the weighted variant is exercised separately below.
        let fused = rrf_fuse(&fts, &vec, &graph, 1.0);
        let top_id = &fused.first().unwrap().entry.id;
        assert_eq!(top_id, "a", "doc seen by all 3 retrievers must win");

        let a_hit = fused.iter().find(|h| h.entry.id == "a").unwrap();
        assert!(a_hit.sources.fts);
        assert!(a_hit.sources.vector);
        assert!(a_hit.sources.graph);
    }

    #[test]
    fn rrf_handles_empty_retrievers() {
        let fused = rrf_fuse(&[], &[], &[], 1.0);
        assert!(fused.is_empty());
    }

    #[test]
    fn rrf_dedupes_same_id_across_retrievers() {
        let a = mk("a", "alpha");
        let one = std::slice::from_ref(&a);
        let fused = rrf_fuse(one, one, one, 1.0);
        assert_eq!(fused.len(), 1, "same id must collapse to one HybridHit");
    }

    #[test]
    fn rrf_k_constant_dampens_head() {
        // At k=60, rank-0 contribution = 1/60 ≈ 0.0167.
        // Rank-1 = 1/61 ≈ 0.0164. Very small gap — that's the point.
        // A doc at rank-0 in ONE retriever should lose to a doc at rank-1 in TWO.
        let a = mk("a", "alpha");
        let b = mk("b", "bravo");

        let fts = vec![a.clone()]; // a at rank 0
        let vec = vec![b.clone(), a.clone()]; // a at rank 1, b at rank 0
        let graph = vec![b.clone(), a.clone()]; // a at rank 1, b at rank 0

        // a: 1/60 + 1/61 + 1/61 ≈ 0.0495
        // b:        1/60 + 1/60 ≈ 0.0333
        let fused = rrf_fuse(&fts, &vec, &graph, 1.0);
        assert_eq!(fused[0].entry.id, "a");
    }

    /// Lowering graph weight must reduce graph's contribution. The mechanism:
    /// at weight 1.0 graph contributes the same per-rank as fts/vector;
    /// at weight 0.4 a graph-only hit is worth ~60% less. This is the exact
    /// dial behind the graph_hop recall regression the eval harness surfaced.
    #[test]
    fn rrf_graph_weight_dampens_graph_only_hits() {
        let a = mk("a", "alpha"); // graph-only hit (potential noise)
        let b = mk("b", "bravo"); // in fts at rank 0, vector at rank 1
        let c = mk("c", "c-filler"); // pushes b down in vector

        let fts = [b.clone()];
        let vec_results = [c, b.clone()];
        let graph_results = [a.clone()];

        // Weight 1.0:
        //   a: 1/60                 ≈ 0.01667
        //   b: 1/60 + 1/61          ≈ 0.03306  → gap b/a ≈ 1.98x
        // Weight 0.4:
        //   a: 0.4/60               ≈ 0.00667
        //   b: 1/60 + 1/61          ≈ 0.03306  → gap b/a ≈ 4.96x
        let fused_full = rrf_fuse(&fts, &vec_results, &graph_results, 1.0);
        let fused_damped = rrf_fuse(&fts, &vec_results, &graph_results, 0.4);

        let gap = |hits: &[HybridHit]| {
            let b_score = hits.iter().find(|h| h.entry.id == "b").unwrap().score;
            let a_score = hits.iter().find(|h| h.entry.id == "a").unwrap().score;
            b_score / a_score
        };
        let full_gap = gap(&fused_full);
        let damped_gap = gap(&fused_damped);
        assert!(
            damped_gap > full_gap,
            "lower graph_weight should widen the gap between non-graph and graph-only hits: \
             full_gap={full_gap:.3}, damped_gap={damped_gap:.3}"
        );
    }

    /// HybridOptions::default() must pick up the tuned constant — if someone
    /// adds a new field and forgets to default-set graph_weight, this catches it.
    #[test]
    fn default_graph_weight_wired_into_options() {
        assert_eq!(HybridOptions::default().graph_weight, DEFAULT_GRAPH_WEIGHT);
    }

    #[test]
    fn sanitize_fts_strips_operators() {
        let out = sanitize_fts_query("inventory-labeler pricing OR billing");
        // Each token quoted, dash preserved inside quotes (FTS5 treats quoted
        // tokens as literal — no NOT operator hazard).
        assert!(out.contains("\"inventory-labeler\""));
        assert!(out.contains("\"pricing\""));
        assert!(out.contains("\"OR\""), "OR is quoted, not a keyword: {out}");
    }

    #[test]
    fn sanitize_fts_empty_for_pure_punctuation() {
        assert_eq!(sanitize_fts_query("--- ??? !!!"), "");
        assert_eq!(sanitize_fts_query(""), "");
    }

    #[test]
    fn hit_source_label_lists_contributors() {
        let s = HitSources {
            fts: true,
            graph: true,
            ..Default::default()
        };
        assert_eq!(s.label(), "bm25+graph");
    }
}
