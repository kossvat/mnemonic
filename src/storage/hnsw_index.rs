use hnsw_rs::prelude::*;
use anndists::dist::DistCosine;
use tracing::debug;

use crate::embedding::Embedding;

/// HNSW index for fast approximate nearest neighbor search.
/// Wraps hnsw_rs with cosine distance for f32 embeddings.
///
/// Uses 'static lifetime — all inserted vectors are copied into the index.
pub struct HnswIndex {
    hnsw: Hnsw<'static, f32, DistCosine>,
    /// Keeps owned copies of vectors so they live as long as the index
    vectors: Vec<Vec<f32>>,
    /// Maps HNSW internal DataId → memory UUID string
    id_map: Vec<String>,
}

// Safety: Hnsw uses Arc internally for shared data, vectors are owned
unsafe impl Send for HnswIndex {}
unsafe impl Sync for HnswIndex {}

impl HnswIndex {
    /// Create a new empty index.
    /// - max_elements: expected max number of vectors
    pub fn new(max_elements: usize) -> Self {
        let m = 16; // max connections per node
        let max_layer = 16;
        let ef_construction = 200;
        let hnsw = Hnsw::<f32, DistCosine>::new(
            m, max_elements, max_layer, ef_construction, DistCosine,
        );
        Self {
            hnsw,
            vectors: Vec::new(),
            id_map: Vec::new(),
        }
    }

    /// Insert a vector with associated memory ID.
    /// The vector is cloned and owned by the index.
    pub fn insert(&mut self, memory_id: &str, embedding: &Embedding) {
        let data_id = self.id_map.len();
        self.id_map.push(memory_id.to_string());
        self.vectors.push(embedding.clone());

        // Safety: the vector lives in self.vectors for the lifetime of the index.
        // We transmute the slice lifetime to 'static since we guarantee the data
        // won't be moved or dropped while the index exists.
        let slice: &[f32] = &self.vectors[data_id];
        let static_slice: &'static [f32] = unsafe {
            std::mem::transmute(slice)
        };
        self.hnsw.insert((static_slice, data_id));
        debug!("HNSW insert: data_id={data_id}, memory={memory_id}");
    }

    /// Search for K nearest neighbors. Returns Vec<(memory_id, similarity)>.
    /// Similarity is 1.0 - cosine_distance (1.0 = identical).
    pub fn search(&self, query: &Embedding, k: usize) -> Vec<(String, f32)> {
        if self.id_map.is_empty() {
            return Vec::new();
        }

        let ef_search = (k * 3).max(30);
        let neighbours = self.hnsw.search(query, k, ef_search);

        neighbours
            .into_iter()
            .filter_map(|n| {
                let data_id = n.d_id;
                if data_id < self.id_map.len() {
                    let similarity = 1.0 - n.distance;
                    Some((self.id_map[data_id].clone(), similarity))
                } else {
                    None
                }
            })
            .collect()
    }

    /// Number of vectors in the index
    pub fn len(&self) -> usize {
        self.id_map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.id_map.is_empty()
    }
}
