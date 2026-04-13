use anyhow::Result;
use std::collections::HashMap;

/// Fixed-size embedding vector (256-dim hash-based)
pub const EMBED_DIMS: usize = 256;
pub type Embedding = Vec<f32>;

/// Trait for embedding providers — swap to neural in v2
pub trait Embedder: Send + Sync {
    fn embed(&self, text: &str) -> Result<Embedding>;
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
}

/// Neural embedder using all-MiniLM-L6-v2 via fastembed (ONNX Runtime).
/// 384-dim real semantic embeddings. ~5ms per call.
/// Enable with: cargo build --features neural
#[cfg(feature = "neural")]
pub struct NeuralEmbedder {
    model: fastembed::TextEmbedding,
}

#[cfg(feature = "neural")]
impl NeuralEmbedder {
    pub fn new() -> Result<Self> {
        use fastembed::{InitOptions, EmbeddingModel};
        let model = fastembed::TextEmbedding::try_new(
            InitOptions::new(EmbeddingModel::AllMiniLML6V2)
                .with_show_download_progress(true),
        )?;
        Ok(Self { model })
    }
}

#[cfg(feature = "neural")]
impl Embedder for NeuralEmbedder {
    fn embed(&self, text: &str) -> Result<Embedding> {
        let results = self.model.embed(vec![text], None)?;
        results
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("No embedding returned"))
    }
}

/// Create the best available embedder based on compiled features.
/// With --features neural: NeuralEmbedder (384-dim, semantic).
/// Without: HashEmbedder (256-dim, hash-based).
pub fn create_embedder() -> Result<Box<dyn Embedder>> {
    #[cfg(feature = "neural")]
    {
        match NeuralEmbedder::new() {
            Ok(e) => {
                tracing::info!("Using neural embedder (all-MiniLM-L6-v2, 384-dim)");
                return Ok(Box::new(e));
            }
            Err(e) => {
                tracing::warn!("Neural embedder failed to load: {e}. Falling back to hash embedder.");
            }
        }
    }

    tracing::info!("Using hash embedder (256-dim)");
    Ok(Box::new(HashEmbedder::new()))
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
        let original = vec![0.1, -0.5, 3.14, 0.0, -1.0];
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
}
