pub mod daemon_client;

use anyhow::Result;
use std::collections::HashMap;
use std::sync::OnceLock;

/// Fixed-size embedding vector (256-dim hash-based)
pub const EMBED_DIMS: usize = 256;
pub type Embedding = Vec<f32>;

/// Trait for embedding providers — swap to neural in v2
pub trait Embedder: Send + Sync {
    /// Embed a stored document / passage. For asymmetric retrieval models
    /// (e5) implementations prepend a "passage:" instruction.
    fn embed(&self, text: &str) -> Result<Embedding>;

    /// Embed a search QUERY. Asymmetric models (e5) prepend "query:" here;
    /// symmetric models (paraphrase) and the hash fallback just reuse
    /// `embed`. Retrieval call sites use this; storage/dedup use `embed`.
    fn embed_query(&self, text: &str) -> Result<Embedding> {
        self.embed(text)
    }

    /// Stable identifier of the underlying model, surfaced over the daemon's
    /// /embed and /status endpoints so clients can detect split-brain (daemon
    /// and MCP built against different models) instead of silently mixing
    /// incompatible vectors.
    fn model_id(&self) -> &'static str {
        "unknown"
    }

    /// Output dimension when statically known (None when it would require
    /// loading the model to find out).
    fn dim_hint(&self) -> Option<usize> {
        None
    }
}

/// Hash-based embedder using weighted SimHash + TF-IDF-like features.
/// Zero dependencies, <1ms per call, good enough for dedup (cosine > 0.92).
pub struct HashEmbedder;

impl HashEmbedder {
    pub fn new() -> Self {
        Self
    }
}

impl Embedder for HashEmbedder {
    fn embed(&self, text: &str) -> Result<Embedding> {
        Ok(hash_embed(text))
    }

    fn model_id(&self) -> &'static str {
        "hash-simhash-v1"
    }

    fn dim_hint(&self) -> Option<usize> {
        Some(EMBED_DIMS)
    }
}

/// Neural embedder using multilingual-e5-base via fastembed (ONNX Runtime).
/// 768-dim, ~100 languages incl. Russian — SOTA-tier multilingual retrieval.
///
/// e5 is ASYMMETRIC: it must be fed "query: {text}" for search queries and
/// "passage: {text}" for stored documents. We apply those prefixes in
/// `embed` (passage) and `embed_query` (query). Dropping the prefixes
/// noticeably degrades recall, so call sites must pick the right method:
/// storage/dedup → `embed`, retrieval → `embed_query`.
///
/// Replaces all-MiniLM-L6-v2 (English-only, 384-dim) which was effectively
/// blind to Russian queries (RU recall@5 = 0 on the eval set).
/// Enable with: cargo build --features neural
#[cfg(feature = "neural")]
pub struct NeuralEmbedder {
    model: fastembed::TextEmbedding,
}

#[cfg(feature = "neural")]
impl NeuralEmbedder {
    pub fn new() -> Result<Self> {
        use fastembed::{EmbeddingModel, InitOptions};
        let model = fastembed::TextEmbedding::try_new(
            InitOptions::new(EmbeddingModel::MultilingualE5Base).with_show_download_progress(true),
        )?;
        Ok(Self { model })
    }

    /// Run the model on a string that already carries its e5 instruction
    /// prefix ("query: " or "passage: ").
    fn embed_prefixed(&self, prefixed: String) -> Result<Embedding> {
        let results = self.model.embed(vec![prefixed], None)?;
        results
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("No embedding returned"))
    }
}

#[cfg(feature = "neural")]
impl Embedder for NeuralEmbedder {
    fn embed(&self, text: &str) -> Result<Embedding> {
        self.embed_prefixed(format!("passage: {text}"))
    }

    fn embed_query(&self, text: &str) -> Result<Embedding> {
        self.embed_prefixed(format!("query: {text}"))
    }

    fn model_id(&self) -> &'static str {
        "intfloat/multilingual-e5-base"
    }

    fn dim_hint(&self) -> Option<usize> {
        Some(768)
    }
}

/// Create the best available embedder based on compiled features.
/// With --features neural: NeuralEmbedder (768-dim, multilingual semantic).
/// Without: HashEmbedder (256-dim, hash-based).
pub fn create_embedder() -> Result<Box<dyn Embedder>> {
    #[cfg(feature = "neural")]
    {
        match NeuralEmbedder::new() {
            Ok(e) => {
                tracing::info!("Using neural embedder (multilingual-e5-base, 768-dim)");
                return Ok(Box::new(e));
            }
            Err(e) => {
                tracing::warn!(
                    "Neural embedder failed to load: {e}. Falling back to hash embedder."
                );
            }
        }
    }

    tracing::info!("Using hash embedder (256-dim)");
    Ok(Box::new(HashEmbedder::new()))
}

/// An `Embedder` that defers model construction until the first embed call.
///
/// The neural model (multilingual-e5-base via ONNX Runtime) costs ~1.4 GB of
/// transient RSS to load. The MCP server spawns one process per Claude Code
/// session, and most tool calls (context, recent, status, graph) never embed,
/// so loading eagerly at startup made every session spike ~1.4 GB for nothing.
/// Wrapping the embedder here loads the model only when a search or save
/// actually needs it, once, and never for embed-free sessions.
type EmbedderBuilder = Box<dyn Fn() -> Result<Box<dyn Embedder>> + Send + Sync>;

pub struct LazyEmbedder {
    inner: OnceLock<Box<dyn Embedder>>,
    builder: EmbedderBuilder,
}

impl LazyEmbedder {
    /// Defers to `create_embedder` on first use.
    pub fn new() -> Self {
        Self::from_builder(create_embedder)
    }

    /// Construct with a custom builder (used by tests to inject a cheap
    /// embedder and observe when construction actually happens).
    pub(crate) fn from_builder(
        builder: impl Fn() -> Result<Box<dyn Embedder>> + Send + Sync + 'static,
    ) -> Self {
        Self {
            inner: OnceLock::new(),
            builder: Box::new(builder),
        }
    }

    /// Get the real embedder, building it on first use. Idempotent and
    /// race-safe: if two callers init at once, one wins and both see it.
    fn get(&self) -> Result<&dyn Embedder> {
        if let Some(e) = self.inner.get() {
            return Ok(e.as_ref());
        }
        let built = (self.builder)()?;
        // set() fails only if another thread already initialised it — fine,
        // we re-read below either way.
        let _ = self.inner.set(built);
        Ok(self
            .inner
            .get()
            .expect("embedder was just initialised")
            .as_ref())
    }
}

impl Default for LazyEmbedder {
    fn default() -> Self {
        Self::new()
    }
}

impl Embedder for LazyEmbedder {
    fn embed(&self, text: &str) -> Result<Embedding> {
        self.get()?.embed(text)
    }

    fn embed_query(&self, text: &str) -> Result<Embedding> {
        self.get()?.embed_query(text)
    }

    /// Delegates when already built; never forces a model load just to
    /// answer metadata.
    fn model_id(&self) -> &'static str {
        self.inner
            .get()
            .map(|e| e.model_id())
            .unwrap_or("lazy-unloaded")
    }

    fn dim_hint(&self) -> Option<usize> {
        self.inner.get().and_then(|e| e.dim_hint())
    }
}

/// Generate a 256-dim embedding from text using feature hashing (SimHash-style).
///
/// Algorithm:
/// 1. Tokenize into words + bigrams
/// 2. Hash each token to a set of dimensions
/// 3. Accumulate +weight/-weight per dimension based on hash bits
/// 4. Normalize to unit vector
///
/// This preserves word overlap semantics well enough for dedup.
fn hash_embed(text: &str) -> Embedding {
    let mut vector = vec![0.0f32; EMBED_DIMS];

    let lower = text.to_lowercase();
    let words: Vec<&str> = lower
        .split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-')
        .filter(|w| w.len() >= 2)
        .collect();

    if words.is_empty() {
        return vector;
    }

    // Count term frequencies for TF weighting
    let mut tf: HashMap<&str, f32> = HashMap::new();
    for w in &words {
        *tf.entry(w).or_default() += 1.0;
    }

    let total = words.len() as f32;

    // Unigrams
    for (word, count) in &tf {
        let weight = count / total; // TF weight
        hash_project(word, weight, &mut vector);
    }

    // Bigrams (capture word order / phrases)
    for pair in words.windows(2) {
        let bigram = format!("{} {}", pair[0], pair[1]);
        hash_project(&bigram, 0.5 / total, &mut vector);
    }

    // Normalize to unit vector
    let norm: f32 = vector.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for v in &mut vector {
            *v /= norm;
        }
    }

    vector
}

/// Project a token into the embedding space using multiple hash functions.
/// Each token affects ~8 dimensions (like a sparse random projection).
fn hash_project(token: &str, weight: f32, vector: &mut [f32]) {
    // Use multiple hash seeds for better distribution
    for seed in 0u64..4 {
        let h = fnv_hash(token, seed);

        // Pick 2 dimensions per hash
        let dim1 = (h as usize) % EMBED_DIMS;
        let dim2 = ((h >> 16) as usize) % EMBED_DIMS;

        // Sign from different bits
        let sign1: f32 = if (h >> 8) & 1 == 0 { 1.0 } else { -1.0 };
        let sign2: f32 = if (h >> 24) & 1 == 0 { 1.0 } else { -1.0 };

        vector[dim1] += sign1 * weight;
        vector[dim2] += sign2 * weight;
    }
}

/// FNV-1a hash with seed
fn fnv_hash(s: &str, seed: u64) -> u64 {
    let mut h: u64 = 14695981039346656037u64.wrapping_add(seed.wrapping_mul(6364136223846793005));
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(1099511628211);
    }
    h
}

/// Cosine similarity between two vectors
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }

    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();

    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }

    dot / (norm_a * norm_b)
}

/// Serialize embedding to bytes (for SQLite BLOB)
pub fn embedding_to_bytes(embedding: &[f32]) -> Vec<u8> {
    embedding.iter().flat_map(|f| f.to_le_bytes()).collect()
}

/// Deserialize embedding from bytes
pub fn embedding_from_bytes(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing::debug;

    #[test]
    fn test_identical_texts() {
        let a = hash_embed("Add JWT token refresh for authentication");
        let b = hash_embed("Add JWT token refresh for authentication");
        assert!((cosine_similarity(&a, &b) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_similar_texts() {
        let a = hash_embed("Add JWT token refresh for authentication");
        let b = hash_embed("Add JWT token refresh for auth module");
        let sim = cosine_similarity(&a, &b);
        debug!("Similar texts cosine: {sim:.4}");
        assert!(sim > 0.7, "Expected > 0.7, got {sim}");
    }

    #[test]
    fn test_different_texts() {
        let a = hash_embed("Add JWT token refresh for authentication");
        let b = hash_embed("Fix database migration script for PostgreSQL");
        let sim = cosine_similarity(&a, &b);
        debug!("Different texts cosine: {sim:.4}");
        assert!(sim < 0.5, "Expected < 0.5, got {sim}");
    }

    #[test]
    fn test_exact_duplicate_detection() {
        let a = hash_embed("feat(auth): Add JWT token refresh");
        let b = hash_embed("feat(auth): Add JWT token refresh");
        assert!(cosine_similarity(&a, &b) >= 0.92);
    }

    #[test]
    fn test_embedding_roundtrip() {
        let original = vec![0.1, -0.5, 3.75, 0.0, -1.0];
        let bytes = embedding_to_bytes(&original);
        let restored = embedding_from_bytes(&bytes);
        assert_eq!(original, restored);
    }

    #[test]
    fn test_empty_text() {
        let emb = hash_embed("");
        let norm: f32 = emb.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!(norm < 1e-6); // zero vector for empty text
    }

    #[test]
    fn test_cosine_orthogonal() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![0.0, 1.0, 0.0];
        assert!(cosine_similarity(&a, &b).abs() < 1e-6);
    }

    #[test]
    fn lazy_embedder_builds_once_on_first_use() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        let lazy = LazyEmbedder::from_builder(move || {
            c.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(HashEmbedder::new()) as Box<dyn Embedder>)
        });

        // Constructing the wrapper must NOT build the underlying embedder.
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "embedder must not be built until first embed"
        );

        let a = lazy.embed("hello world").unwrap();
        assert_eq!(a.len(), EMBED_DIMS);
        assert_eq!(calls.load(Ordering::SeqCst), 1, "built on first embed");

        // Second call (and embed_query) reuses the same instance.
        let _ = lazy.embed_query("hello world").unwrap();
        let _ = lazy.embed("again").unwrap();
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "embedder built once and reused"
        );
    }
}
