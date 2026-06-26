//! Cross-encoder reranking for retrieval.
//!
//! RRF fusion (in `retrieval`) merges BM25 / vector / graph candidates by
//! rank. It's robust and fast, but it can't tell whether the doc *actually*
//! answers the query — only that it ranked highly in at least one retriever.
//! A cross-encoder pass takes the top-N fused candidates, scores each
//! (query, doc) pair with a model that's seen both jointly, and re-sorts.
//!
//! ## Why local-first?
//!
//! Mnemonic runs entirely on the user's Mac — no cloud retrieval. We use
//! the same `fastembed` runtime that already ships embeddings (ONNX +
//! HuggingFace cache) and load the multilingual Jina-v2-base reranker
//! (~278MB, downloads once on first use into `~/.fastembed_cache`).
//!
//! ## Latency (measured, not claimed)
//!
//! On M1 Mac, fastembed 4.9.1 CPU ONNX (no Metal/CoreML acceleration):
//!   - 10-doc batch: ~8s per query
//!   - 30-doc batch: ~25-34s per query
//!
//! That's much slower than the marketing-grade "sub-100ms" cross-encoder
//! claims. It's tolerable for batch eval (`mnemonic eval --rerank` on
//! 12 queries = ~2-4min), unusable for interactive retrieval today.
//! Path to faster: build fastembed with `metal` feature, or swap to a
//! smaller English-only reranker, or quantize. None of those are wired
//! up yet — for now, this module is opt-in and CLI-only.
//!
//! ## Behavior
//!
//! - First call constructs and warms the model (downloads if missing).
//!   Subsequent calls use the cached instance.
//! - If model load fails (no network, disk full, etc.), `try_new` returns
//!   `Err` and the caller is expected to fall back to RRF-only ranking.
//!   This module never panics on absent infrastructure.
//! - Reranking is opt-in via `HybridOptions::rerank` so eval can A/B with
//!   it on and off, and so the daemon can start without paying the model
//!   download cost until a reranked query is actually issued.

use anyhow::Result;
use tracing::warn;

/// One reranker score for a candidate document. `index` is the position
/// in the input `documents` slice the score corresponds to — the reranker
/// returns results sorted by score, so the caller can use `index` to
/// re-thread back to the original `HybridHit` it came from.
#[derive(Debug, Clone)]
pub struct RerankScore {
    pub index: usize,
    pub score: f32,
}

/// Cross-encoder reranker trait. Implementations score (query, doc) pairs
/// jointly — fundamentally more accurate than bi-encoder vector cosine for
/// short queries, at the cost of being O(N) per query rather than indexable.
pub trait Reranker: Send + Sync {
    /// Score each document against the query and return all candidates
    /// sorted by score, highest first. Length of return is `documents.len()`.
    fn rerank(&self, query: &str, documents: &[&str]) -> Result<Vec<RerankScore>>;
}

// ── Neural implementation (fastembed / ONNX) ────────────────────────────

/// Jina v2 multilingual reranker (base, ~278MB ONNX).
///
/// Chosen because Mnemonic memories often mix English and Russian within
/// the same body. English-only rerankers (Jina v1 turbo, bge-reranker-base
/// before v2-m3) underperform on mixed-script titles. v2-base is the
/// smallest multilingual model fastembed-rs ships out of the box.
#[cfg(feature = "neural")]
pub struct JinaReranker {
    model: fastembed::TextRerank,
}

#[cfg(feature = "neural")]
impl JinaReranker {
    pub fn new() -> Result<Self> {
        use fastembed::{RerankInitOptions, RerankerModel, TextRerank};
        // First-call cost is real and confusing — print to stderr BEFORE
        // calling try_new so the user sees something is happening, then log
        // total init time on completion. fastembed's own progress bar
        // covers the HTTP download itself.
        eprintln!(
            "Loading cross-encoder reranker (jina-reranker-v2-base-multilingual, ~278MB). \
             First run downloads weights to ~/.fastembed_cache; later runs reuse the cache."
        );
        let t0 = std::time::Instant::now();
        let model = TextRerank::try_new(
            RerankInitOptions::new(RerankerModel::JINARerankerV2BaseMultiligual)
                .with_show_download_progress(true),
        )?;
        let dt = t0.elapsed();
        if dt > std::time::Duration::from_secs(5) {
            tracing::info!(
                "Reranker ready in {:.1}s (download + ONNX init)",
                dt.as_secs_f32()
            );
        } else {
            tracing::debug!("Reranker ready in {:.1}s (cache hit)", dt.as_secs_f32());
        }
        Ok(Self { model })
    }
}

#[cfg(feature = "neural")]
impl Reranker for JinaReranker {
    fn rerank(&self, query: &str, documents: &[&str]) -> Result<Vec<RerankScore>> {
        if documents.is_empty() {
            return Ok(Vec::new());
        }
        // Measure each rerank call so we have data, not docs claims. Print
        // a one-time warning if the call exceeded 1s — far over the
        // sub-100ms target and a signal something is off (CPU contention,
        // missing accelerator, ORT thread starvation under load).
        let t0 = std::time::Instant::now();

        // fastembed's generic S must unify across query AND docs, so we
        // pass everything as &str. return_documents=false: we'll re-thread
        // by index ourselves and don't need the doc text back.
        // batch_size=None lets fastembed pick a default suited to the host
        // (ORT manages thread pool).
        let docs_vec: Vec<&str> = documents.to_vec();
        let results = self.model.rerank(query, docs_vec, false, None)?;

        let dt = t0.elapsed();
        tracing::debug!(
            "Reranked {} docs in {:.0}ms",
            documents.len(),
            dt.as_secs_f32() * 1000.0
        );
        // Observed baseline on M1 CPU ONNX: ~0.8s per doc. Warn at >2x
        // the linear expectation so we catch real slowdowns (CPU
        // contention, missing accelerator) without spamming the log on
        // every healthy call.
        let expected_ms = documents.len() as f32 * 800.0;
        let actual_ms = dt.as_secs_f32() * 1000.0;
        if actual_ms > expected_ms * 2.0 && actual_ms > 5000.0 {
            tracing::warn!(
                "Reranker took {:.1}s for {} docs ({:.0}ms/doc) — about 2x slower than the \
                 ~800ms/doc baseline observed on M1 CPU. Suspect CPU contention or missing \
                 accelerator backend.",
                dt.as_secs_f32(),
                documents.len(),
                actual_ms / documents.len() as f32,
            );
        }

        Ok(results
            .into_iter()
            .map(|r| RerankScore {
                index: r.index,
                score: r.score,
            })
            .collect())
    }
}

/// Factory: build the best available reranker, or `Ok(None)` if reranking
/// isn't viable (missing feature flag, model download failure). Caller
/// treats absence as a signal to skip reranking, not as an error.
///
/// Splitting this from `JinaReranker::new` keeps the retrieval module from
/// having to know about feature flags.
pub fn try_create_reranker() -> Result<Option<Box<dyn Reranker>>> {
    #[cfg(feature = "neural")]
    {
        match JinaReranker::new() {
            Ok(r) => {
                tracing::info!(
                    "Reranker ready (jina-reranker-v2-base-multilingual, cross-encoder)"
                );
                Ok(Some(Box::new(r) as Box<dyn Reranker>))
            }
            Err(e) => {
                warn!("Reranker init failed: {e}. Retrieval will fall back to RRF-only ranking.");
                Ok(None)
            }
        }
    }
    #[cfg(not(feature = "neural"))]
    {
        // No neural feature → no reranker. Hash-embedder builds skip this
        // entirely and rely on BM25 + hash-vector + graph.
        let _ = warn;
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tiny mock reranker for tests that need to exercise the retrieval
    /// integration without loading 280MB of weights. Score is the length
    /// of the longest substring of `query` found in the doc — crude, but
    /// deterministic and zero-dependency.
    pub struct MockReranker;
    impl Reranker for MockReranker {
        fn rerank(&self, query: &str, documents: &[&str]) -> Result<Vec<RerankScore>> {
            let q = query.to_lowercase();
            let mut scored: Vec<RerankScore> = documents
                .iter()
                .enumerate()
                .map(|(i, doc)| {
                    let d = doc.to_lowercase();
                    // crude overlap score: 1.0 for substring match, else
                    // ratio of shared word tokens
                    let s = if d.contains(&q) {
                        1.0
                    } else {
                        let q_tokens: std::collections::HashSet<&str> =
                            q.split_whitespace().collect();
                        let d_tokens: std::collections::HashSet<&str> =
                            d.split_whitespace().collect();
                        if q_tokens.is_empty() {
                            0.0
                        } else {
                            q_tokens.intersection(&d_tokens).count() as f32 / q_tokens.len() as f32
                        }
                    };
                    RerankScore { index: i, score: s }
                })
                .collect();
            scored.sort_by(|a, b| {
                b.score
                    .partial_cmp(&a.score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            Ok(scored)
        }
    }

    #[test]
    fn mock_reranker_orders_by_substring_match() {
        let r = MockReranker;
        let docs = [
            "something else",
            "watch label pricing",
            "rust async runtime",
        ];
        let scored = r.rerank("watch label", &docs).unwrap();
        // The doc containing "watch label" should be first.
        assert_eq!(scored[0].index, 1);
        assert!(scored[0].score >= scored[1].score);
        assert!(scored[1].score >= scored[2].score);
    }

    #[test]
    fn empty_docs_returns_empty() {
        let r = MockReranker;
        let docs: Vec<&str> = vec![];
        assert!(r.rerank("anything", &docs).unwrap().is_empty());
    }

    #[test]
    fn preserves_all_indices() {
        let r = MockReranker;
        let docs = ["a one", "b two three", "c four"];
        let scored = r.rerank("two", &docs).unwrap();
        assert_eq!(scored.len(), 3);
        // Every input index must appear exactly once in the output.
        let mut seen = [false; 3];
        for s in &scored {
            assert!(!seen[s.index], "index {} appeared twice", s.index);
            seen[s.index] = true;
        }
        assert!(seen.iter().all(|&v| v));
    }

    /// `#[ignore]` because actually constructing the reranker may trigger
    /// a ~280MB model download into `~/.fastembed_cache`. Run explicitly
    /// with `cargo test --release -- --ignored` when validating a real
    /// model load locally. The contract being pinned: the factory never
    /// returns Err — it returns `Ok(None)` on any failure path.
    #[test]
    #[ignore]
    fn try_create_reranker_returns_ok_either_way() {
        let result = try_create_reranker();
        if let Err(e) = &result {
            panic!("factory must not error, got: {e}");
        }
    }
}
