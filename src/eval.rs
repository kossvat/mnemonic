//! Retrieval eval harness.
//!
//! Loads a hand-curated set of (query, expected_relevant) pairs and scores
//! the current hybrid retriever against them. The point isn't to chase a
//! benchmark — it's to have a stable baseline so changes to retrieval
//! (RRF weights, graph hop, new embedders) can be checked against
//! recall@5 / recall@20 / MRR instead of vibes.
//!
//! Two ways to specify what's relevant per query:
//!
//! 1. `expected_ids` — explicit memory ids. Recall = fraction of expected
//!    ids that appear in the top-k. Treats the query as set-retrieval.
//! 2. `expected_title_contains` — case-insensitive substrings. The query
//!    is treated as presence-retrieval: did *any* result whose title
//!    contains any of the substrings show up in the top-k? Useful when
//!    you don't have stable ids (DB regenerated, reflection moved things).
//!
//! Both can coexist on the same query — a result is relevant if either
//! check passes.
//!
//! File format is JSONL (one JSON object per line). Lines starting with
//! `#` or blank lines are skipped so the seed file can carry comments.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// One row of the eval seed file.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct EvalQuery {
    pub query: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub expected_ids: Vec<String>,
    #[serde(default)]
    pub expected_title_contains: Vec<String>,
}

impl EvalQuery {
    /// True if there's at least one relevance criterion to score against.
    /// Queries with no expected_* lists are skipped by `evaluate`.
    pub fn has_expectations(&self) -> bool {
        !self.expected_ids.is_empty() || !self.expected_title_contains.is_empty()
    }
}

/// Decision on whether a single result is relevant to the query. Pulled
/// out so the CLI can show "matched by id" vs "matched by title" if
/// useful, and so it can be unit-tested without instantiating a hit.
pub fn is_hit(query: &EvalQuery, id: &str, title: &str) -> bool {
    if query.expected_ids.iter().any(|x| x == id) {
        return true;
    }
    let title_lc = title.to_lowercase();
    query
        .expected_title_contains
        .iter()
        .any(|s| !s.is_empty() && title_lc.contains(&s.to_lowercase()))
}

/// Read a JSONL eval file. Blank lines and `#`-prefixed comment lines are
/// allowed. Each non-comment line must parse as an `EvalQuery`.
pub fn load_jsonl(path: &Path) -> Result<Vec<EvalQuery>> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("opening eval file {}", path.display()))?;
    let mut out = Vec::new();
    for (i, line) in raw.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let q: EvalQuery = serde_json::from_str(trimmed)
            .with_context(|| format!("parsing {}:{}", path.display(), i + 1))?;
        out.push(q);
    }
    Ok(out)
}

/// Recall@k for ONE query.
///
/// - `relevance` — relevance flag per hit, in retrieval order. Length is
///   the number of hits returned (may be < k if the retriever ran out).
/// - `k`         — cutoff. We look at the first `min(k, relevance.len())`.
/// - `total_relevant` — denominator. For `expected_ids` it's `expected_ids.len()`.
///   For `expected_title_contains` callers should pass 1 (presence test).
///
/// Returns 0.0 if `total_relevant == 0`, NOT NaN — keeps aggregate clean.
pub fn recall_at_k(relevance: &[bool], k: usize, total_relevant: usize) -> f32 {
    if total_relevant == 0 {
        return 0.0;
    }
    let hits = relevance.iter().take(k).filter(|r| **r).count();
    (hits as f32 / total_relevant as f32).min(1.0)
}

/// Reciprocal rank (1 / rank of first relevant hit). 0.0 if no relevant
/// hit ever shows up in `relevance`. Used to compute mean reciprocal
/// rank (MRR) over a query set by averaging.
pub fn reciprocal_rank(relevance: &[bool]) -> f32 {
    for (i, hit) in relevance.iter().enumerate() {
        if *hit {
            return 1.0 / (i as f32 + 1.0);
        }
    }
    0.0
}

/// "Number of relevant docs" denominator for recall.
///
/// Convention: when only `expected_title_contains` is provided, the query
/// is a presence check — `total_relevant` collapses to 1 ("at least one
/// title-matching doc should be in top-k"). Otherwise it's the count of
/// expected ids. Mixed mode (both lists present) falls back to id-count
/// because that's the only well-defined denominator.
pub fn expected_relevant_count(query: &EvalQuery) -> usize {
    if !query.expected_ids.is_empty() {
        query.expected_ids.len()
    } else if !query.expected_title_contains.is_empty() {
        1
    } else {
        0
    }
}

/// Per-query metric row, for both the CLI table and the JSON output.
#[derive(Debug, Clone, Serialize)]
pub struct QueryResult {
    pub query: String,
    pub tags: Vec<String>,
    pub returned: usize,
    pub recall_at_5: f32,
    pub recall_at_20: f32,
    pub reciprocal_rank: f32,
    /// First few returned (id, title) for eyeballing failures.
    pub top_ids: Vec<(String, String)>,
}

/// Aggregate over a query set.
#[derive(Debug, Clone, Serialize)]
pub struct EvalSummary {
    pub queries: usize,
    pub skipped: usize,
    pub mean_recall_at_5: f32,
    pub mean_recall_at_20: f32,
    pub mrr: f32,
    pub per_query: Vec<QueryResult>,
}

/// Compute the aggregate metrics from a list of per-query relevance
/// vectors. Pure function — no IO, no retriever calls. Used by the CLI
/// after running the retriever, and by tests with hand-crafted vectors.
pub fn aggregate(per_query: Vec<QueryResult>) -> EvalSummary {
    let n = per_query.len().max(1) as f32;
    let mean_5 = per_query.iter().map(|r| r.recall_at_5).sum::<f32>() / n;
    let mean_20 = per_query.iter().map(|r| r.recall_at_20).sum::<f32>() / n;
    let mrr = per_query.iter().map(|r| r.reciprocal_rank).sum::<f32>() / n;
    EvalSummary {
        queries: per_query.len(),
        skipped: 0,
        mean_recall_at_5: mean_5,
        mean_recall_at_20: mean_20,
        mrr,
        per_query,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(ids: &[&str], titles: &[&str]) -> EvalQuery {
        EvalQuery {
            query: "test".into(),
            tags: vec![],
            expected_ids: ids.iter().map(|s| s.to_string()).collect(),
            expected_title_contains: titles.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn is_hit_matches_by_id() {
        let query = q(&["abc"], &[]);
        assert!(is_hit(&query, "abc", "some title"));
        assert!(!is_hit(&query, "def", "some title"));
    }

    #[test]
    fn is_hit_matches_by_title_case_insensitive() {
        let query = q(&[], &["Inventory Labeler"]);
        assert!(is_hit(&query, "any-id", "inventory labeler pricing"));
        assert!(is_hit(&query, "any-id", "INVENTORY LABELER retrospective"));
        assert!(!is_hit(&query, "any-id", "unrelated content"));
    }

    #[test]
    fn is_hit_ignores_empty_substrings() {
        // Stray empty string in the substring list mustn't match everything.
        let query = q(&[], &[""]);
        assert!(!is_hit(&query, "any-id", "literally anything"));
    }

    #[test]
    fn recall_at_k_basic() {
        // 3 relevant docs expected. Top-5 has 2 of them.
        let rel = vec![true, false, true, false, false];
        assert!((recall_at_k(&rel, 5, 3) - 2.0 / 3.0).abs() < 1e-6);
        // Top-1: 1 hit.
        assert!((recall_at_k(&rel, 1, 3) - 1.0 / 3.0).abs() < 1e-6);
    }

    #[test]
    fn recall_at_k_caps_at_one() {
        // Edge case: more hits than expected (shouldn't happen with proper
        // seeds but the math shouldn't return >1).
        let rel = vec![true, true, true];
        assert!((recall_at_k(&rel, 5, 2) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn recall_at_k_zero_relevant_returns_zero_not_nan() {
        let rel = vec![true, false];
        assert_eq!(recall_at_k(&rel, 5, 0), 0.0);
    }

    #[test]
    fn reciprocal_rank_first_hit() {
        assert!((reciprocal_rank(&[true, false, false]) - 1.0).abs() < 1e-6);
        assert!((reciprocal_rank(&[false, true, false]) - 0.5).abs() < 1e-6);
        assert!((reciprocal_rank(&[false, false, true]) - 1.0 / 3.0).abs() < 1e-6);
        assert_eq!(reciprocal_rank(&[false, false, false]), 0.0);
        assert_eq!(reciprocal_rank(&[]), 0.0);
    }

    #[test]
    fn expected_relevant_count_modes() {
        assert_eq!(expected_relevant_count(&q(&["a", "b", "c"], &[])), 3);
        assert_eq!(expected_relevant_count(&q(&[], &["foo"])), 1);
        // Mixed mode → id-count denominator (substring acts as bonus relevance signal).
        assert_eq!(expected_relevant_count(&q(&["a"], &["foo"])), 1);
        assert_eq!(expected_relevant_count(&q(&[], &[])), 0);
    }

    #[test]
    fn aggregate_means_what_it_says() {
        let qr = |r5: f32, r20: f32, rr: f32| QueryResult {
            query: "x".into(),
            tags: vec![],
            returned: 20,
            recall_at_5: r5,
            recall_at_20: r20,
            reciprocal_rank: rr,
            top_ids: vec![],
        };
        let s = aggregate(vec![qr(1.0, 1.0, 1.0), qr(0.0, 0.5, 0.5)]);
        assert_eq!(s.queries, 2);
        assert!((s.mean_recall_at_5 - 0.5).abs() < 1e-6);
        assert!((s.mean_recall_at_20 - 0.75).abs() < 1e-6);
        assert!((s.mrr - 0.75).abs() < 1e-6);
    }
}
