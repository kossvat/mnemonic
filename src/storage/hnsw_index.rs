use std::collections::{HashMap, HashSet};

use anndists::dist::DistCosine;
use hnsw_rs::prelude::*;
use tracing::{debug, warn};

use crate::embedding::{Embedding, cosine_similarity};

/// HNSW index for fast approximate nearest neighbor search.
/// Wraps hnsw_rs with cosine distance for f32 embeddings.
///
/// Uses 'static lifetime — all inserted vectors are copied into the index.
///
/// hnsw_rs has no delete: removed/replaced vectors are tombstoned instead.
/// Search overfetches by the tombstone count and filters dead slots, so
/// `forget`/`supersede` immediately stop a vector from matching (previously
/// ghosts survived until the next daemon restart and made the dedup gate
/// silently drop re-saves of forgotten memories).
pub struct HnswIndex {
    hnsw: Hnsw<'static, f32, DistCosine>,
    /// Keeps owned copies of vectors so they live as long as the index
    vectors: Vec<Vec<f32>>,
    /// Maps HNSW internal DataId → memory UUID string
    id_map: Vec<String>,
    /// memory UUID → live data_id (latest insert wins)
    live: HashMap<String, usize>,
    /// data_ids whose memory was forgotten/superseded/replaced
    tombstones: HashSet<usize>,
    /// Dimension locked by the first inserted vector. anndists panics on
    /// mixed dimensions (hash=256 vs neural=384), so mismatched inserts
    /// and searches are rejected here instead.
    dim: Option<usize>,
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
        let hnsw =
            Hnsw::<f32, DistCosine>::new(m, max_elements, max_layer, ef_construction, DistCosine);
        Self {
            hnsw,
            vectors: Vec::new(),
            id_map: Vec::new(),
            live: HashMap::new(),
            tombstones: HashSet::new(),
            dim: None,
        }
    }

    /// Insert a vector with associated memory ID.
    /// The vector is cloned and owned by the index.
    ///
    /// Re-inserting a known ID tombstones the previous vector so updates
    /// don't leave a stale copy matching old content. Vectors whose
    /// dimension differs from the index are skipped (would panic inside
    /// anndists) — happens transiently while a reembed migration runs.
    pub fn insert(&mut self, memory_id: &str, embedding: &Embedding) {
        match self.dim {
            None => self.dim = Some(embedding.len()),
            Some(d) if d != embedding.len() => {
                warn!(
                    "HNSW insert skipped for {memory_id}: dim {} ≠ index dim {d}. \
                     Run `mnemonic reembed` to migrate.",
                    embedding.len()
                );
                return;
            }
            _ => {}
        }
        if let Some(&old) = self.live.get(memory_id) {
            self.tombstones.insert(old);
        }
        let data_id = self.id_map.len();
        self.id_map.push(memory_id.to_string());
        self.vectors.push(embedding.clone());
        self.live.insert(memory_id.to_string(), data_id);

        // Safety: the vector lives in self.vectors for the lifetime of the index.
        // We transmute the slice lifetime to 'static since we guarantee the data
        // won't be moved or dropped while the index exists.
        let slice: &[f32] = &self.vectors[data_id];
        let static_slice: &'static [f32] = unsafe { std::mem::transmute(slice) };
        self.hnsw.insert((static_slice, data_id));
        debug!("HNSW insert: data_id={data_id}, memory={memory_id}");
    }

    /// Tombstone a memory's vector so it stops matching searches.
    /// Returns true if the memory had a live vector.
    pub fn remove(&mut self, memory_id: &str) -> bool {
        if let Some(data_id) = self.live.remove(memory_id) {
            self.tombstones.insert(data_id);
            debug!("HNSW remove: data_id={data_id}, memory={memory_id}");
            true
        } else {
            false
        }
    }

    /// Search for K nearest neighbors. Returns Vec<(memory_id, similarity)>.
    /// Similarity is 1.0 - cosine_distance (1.0 = identical).
    pub fn search(&self, query: &Embedding, k: usize) -> Vec<(String, f32)> {
        if self.live.is_empty() {
            return Vec::new();
        }
        if let Some(d) = self.dim
            && d != query.len()
        {
            debug!(
                "HNSW search skipped: query dim {} ≠ index dim {d}",
                query.len()
            );
            return Vec::new();
        }

        // Tombstoned slots still occupy result positions inside hnsw_rs —
        // overfetch by the tombstone count so k live results stay reachable.
        let fetch = (k + self.tombstones.len()).min(self.id_map.len()).max(1);
        let ef_search = (fetch * 3).max(30);
        let neighbours = self.hnsw.search(query, fetch, ef_search);

        let hits: Vec<(String, f32)> = neighbours
            .into_iter()
            .filter_map(|n| {
                let data_id = n.d_id;
                if data_id >= self.id_map.len() || self.tombstones.contains(&data_id) {
                    return None;
                }
                let similarity = 1.0 - n.distance;
                Some((self.id_map[data_id].clone(), similarity))
            })
            .take(k)
            .collect();

        let live_target = k.min(self.live.len());
        if hits.len() < live_target {
            debug!(
                "HNSW returned {}/{} live hits after tombstone filtering; filling via brute force",
                hits.len(),
                live_target
            );
            return self.search_bruteforce(query, k);
        }

        hits
    }

    fn search_bruteforce(&self, query: &Embedding, k: usize) -> Vec<(String, f32)> {
        let mut hits: Vec<(String, f32)> = self
            .live
            .iter()
            .map(|(id, data_id)| {
                (
                    id.clone(),
                    cosine_similarity(query, &self.vectors[*data_id]),
                )
            })
            .collect();
        hits.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.cmp(&b.0))
        });
        hits.truncate(k);
        hits
    }

    /// Number of live (non-tombstoned) vectors in the index
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.live.len()
    }

    pub fn is_empty(&self) -> bool {
        self.live.is_empty()
    }

    /// Dimension the index locked to (set by the first inserted vector), or
    /// None while empty. Lets storage reject mismatched-dimension queries
    /// after an embedding-model swap instead of silently scoring them 0.
    pub fn dim(&self) -> Option<usize> {
        self.dim
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit_vec(dims: usize, hot: usize) -> Embedding {
        let mut v = vec![0.0f32; dims];
        v[hot] = 1.0;
        v
    }

    #[test]
    fn removed_vector_stops_matching() {
        let mut idx = HnswIndex::new(10);
        idx.insert("a", &unit_vec(8, 0));
        idx.insert("b", &unit_vec(8, 1));
        idx.insert("c", &unit_vec(8, 2));

        let hits = idx.search(&unit_vec(8, 0), 3);
        assert_eq!(hits[0].0, "a");

        assert!(idx.remove("a"));
        assert!(!idx.remove("a"), "second remove is a no-op");
        assert_eq!(idx.len(), 2);

        let hits = idx.search(&unit_vec(8, 0), 3);
        assert!(
            hits.iter().all(|(id, _)| id != "a"),
            "tombstoned vector must not match: {hits:?}"
        );
        // The other two are still reachable despite the tombstone.
        assert_eq!(hits.len(), 2);
    }

    #[test]
    fn reinsert_replaces_old_vector() {
        let mut idx = HnswIndex::new(10);
        idx.insert("a", &unit_vec(8, 0));
        idx.insert("a", &unit_vec(8, 3));
        assert_eq!(idx.len(), 1);

        // Old content no longer matches as "a" with perfect similarity…
        let hits = idx.search(&unit_vec(8, 0), 2);
        assert!(hits.iter().all(|(_, sim)| *sim < 0.99), "{hits:?}");
        // …new content does.
        let hits = idx.search(&unit_vec(8, 3), 2);
        assert_eq!(hits[0].0, "a");
        assert!(hits[0].1 > 0.99);
    }

    #[test]
    fn dim_mismatch_is_skipped_not_panicking() {
        let mut idx = HnswIndex::new(10);
        idx.insert("a", &unit_vec(256, 0));
        // Different dimension (hash → neural migration): skipped, no panic.
        idx.insert("b", &unit_vec(384, 0));
        assert_eq!(idx.len(), 1);
        // Query in the wrong dimension: empty, no panic.
        assert!(idx.search(&unit_vec(384, 0), 5).is_empty());
        assert_eq!(idx.search(&unit_vec(256, 0), 5).len(), 1);
    }

    #[test]
    fn empty_index_is_safe() {
        let idx = HnswIndex::new(10);
        assert!(idx.is_empty());
        assert!(idx.search(&unit_vec(8, 0), 5).is_empty());
    }
}
