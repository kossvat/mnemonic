use crate::embedding::{Embedding, cosine_similarity, embedding_from_bytes};
use crate::event::{EventKind, MemoryType};
use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::Connection;
use std::sync::Mutex;
use tracing::debug;

/// Dynamic importance scoring: frequency × 0.3 + recency × 0.3 + signal × 0.4
///
/// - **frequency**: how often similar content appears (more = more important pattern)
/// - **recency**: time decay — recent topics matter more
/// - **signal**: event type strength (user correction > decision > note)
pub struct ImportanceScorer {
    /// Weight for frequency component
    pub w_frequency: f32,
    /// Weight for recency component
    pub w_recency: f32,
    /// Weight for signal component
    pub w_signal: f32,
    /// Similarity threshold to count as "related" for frequency
    pub similarity_threshold: f32,
}

impl Default for ImportanceScorer {
    fn default() -> Self {
        Self {
            w_frequency: 0.3,
            w_recency: 0.3,
            w_signal: 0.4,
            similarity_threshold: 0.5,
        }
    }
}

impl ImportanceScorer {
    /// Calculate dynamic importance score (0.0 - 1.0)
    pub fn score(
        &self,
        embedding: &Embedding,
        event_kind: &EventKind,
        memory_type: &MemoryType,
        conn: &Mutex<Connection>,
    ) -> Result<f32> {
        let freq = self.frequency_score(embedding, conn)?;
        let rec = self.recency_score(embedding, conn)?;
        let sig = self.signal_score(event_kind, memory_type);

        let mut score = self.w_frequency * freq + self.w_recency * rec + self.w_signal * sig;

        // Floor: new unique events (never seen before) get at least signal strength as score.
        // Without this, first occurrence of any topic scores ~0.16 and gets dropped.
        if freq == 0.0 && rec == 0.0 {
            score = score.max(sig * 0.75);
        }

        let score = score.clamp(0.0, 1.0);

        debug!("Importance: freq={freq:.2} rec={rec:.2} sig={sig:.2} → {score:.2}");

        Ok(score)
    }

    /// How often similar content has been seen before.
    /// More occurrences of a topic = more important pattern.
    /// Returns 0.0 (never seen) to 1.0 (seen 5+ times)
    fn frequency_score(&self, embedding: &Embedding, conn: &Mutex<Connection>) -> Result<f32> {
        let conn = conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let mut stmt = conn.prepare(
            "SELECT embedding FROM memories WHERE embedding IS NOT NULL ORDER BY timestamp DESC LIMIT 100",
        )?;

        let blobs: Vec<Vec<u8>> = stmt
            .query_map([], |row| row.get::<_, Vec<u8>>(0))?
            .filter_map(|r| r.ok())
            .collect();

        let similar_count = blobs
            .iter()
            .filter(|blob| {
                let existing = embedding_from_bytes(blob);
                cosine_similarity(embedding, &existing) >= self.similarity_threshold
            })
            .count();

        // Normalize: 0 similar = 0.0, 5+ similar = 1.0
        Ok((similar_count as f32 / 5.0).min(1.0))
    }

    /// How recently similar content was last seen.
    /// Recent = more relevant context, old = less important.
    /// Returns 1.0 (within last hour) decaying to 0.0 (>7 days ago)
    fn recency_score(&self, embedding: &Embedding, conn: &Mutex<Connection>) -> Result<f32> {
        let conn = conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let mut stmt = conn.prepare(
            "SELECT timestamp, embedding FROM memories WHERE embedding IS NOT NULL ORDER BY timestamp DESC LIMIT 50",
        )?;

        let now = Utc::now();
        let mut best_recency: f32 = 0.0;

        let rows: Vec<(String, Vec<u8>)> = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
            })?
            .filter_map(|r| r.ok())
            .collect();

        for (ts_str, blob) in &rows {
            let existing = embedding_from_bytes(blob);
            let sim = cosine_similarity(embedding, &existing);

            if sim >= self.similarity_threshold
                && let Ok(ts) = DateTime::parse_from_rfc3339(ts_str)
            {
                let hours_ago = (now - ts.with_timezone(&Utc)).num_minutes() as f32 / 60.0;
                // Exponential decay: half-life of 24 hours
                let recency = (-hours_ago / 24.0).exp();
                best_recency = best_recency.max(recency);
            }
        }

        Ok(best_recency)
    }

    /// Event type signal strength.
    /// User corrections are always critical, decisions are important, notes are moderate.
    fn signal_score(&self, kind: &EventKind, memory_type: &MemoryType) -> f32 {
        // First check event kind (strongest signals)
        match kind {
            EventKind::UserCorrection => return 1.0,
            EventKind::ErrorFixed => return 0.8,
            // Manual save = user explicitly wanted this saved → high signal
            EventKind::Custom(s) if s == "manual" => return 0.8,
            // Git commits are explicit developer actions → decent signal
            EventKind::GitCommit => return 0.6,
            _ => {}
        }

        // Then check classified memory type
        match memory_type {
            MemoryType::Feedback => 1.0,
            MemoryType::Security => 0.9,
            MemoryType::Decision => 0.7,
            MemoryType::SessionSummary => 0.5,
            MemoryType::Note => 0.4,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_signal_scores() {
        let scorer = ImportanceScorer::default();

        assert_eq!(
            scorer.signal_score(&EventKind::UserCorrection, &MemoryType::Note),
            1.0
        );
        assert_eq!(
            scorer.signal_score(&EventKind::GitCommit, &MemoryType::Decision),
            0.6
        );
        assert_eq!(
            scorer.signal_score(&EventKind::FileCreated, &MemoryType::Note),
            0.4
        );
        assert_eq!(
            scorer.signal_score(&EventKind::ErrorFixed, &MemoryType::Note),
            0.8
        );
    }

    #[test]
    fn test_weights_sum_to_one() {
        let scorer = ImportanceScorer::default();
        let total = scorer.w_frequency + scorer.w_recency + scorer.w_signal;
        assert!((total - 1.0).abs() < 1e-6);
    }
}
