use anyhow::Result;
use rusqlite::{Connection, params};
use std::path::Path;
use std::sync::Mutex;
use tracing::{debug, info};

use crate::embedding::{Embedding, cosine_similarity, embedding_from_bytes, embedding_to_bytes};
use crate::event::MemoryEntry;

/// SQLite-backed memory storage (thread-safe via Mutex)
pub struct Storage {
    pub(crate) conn: Mutex<Connection>,
}

impl Storage {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let conn = Connection::open(path)?;
        let storage = Self {
            conn: Mutex::new(conn),
        };
        storage.init_schema()?;
        Ok(storage)
    }

    fn init_schema(&self) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS memories (
                id TEXT PRIMARY KEY,
                timestamp TEXT NOT NULL,
                title TEXT NOT NULL,
                content TEXT NOT NULL,
                memory_type TEXT NOT NULL,
                tags TEXT NOT NULL DEFAULT '[]',
                source TEXT NOT NULL,
                importance REAL NOT NULL DEFAULT 0.5,
                metadata TEXT NOT NULL DEFAULT '{}',
                embedding BLOB,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );

            CREATE INDEX IF NOT EXISTS idx_memories_type ON memories(memory_type);
            CREATE INDEX IF NOT EXISTS idx_memories_timestamp ON memories(timestamp);
            CREATE INDEX IF NOT EXISTS idx_memories_importance ON memories(importance);

            -- Full-text search
            CREATE VIRTUAL TABLE IF NOT EXISTS memories_fts USING fts5(
                title, content, tags,
                content='memories',
                content_rowid='rowid'
            );

            -- Triggers to keep FTS in sync
            CREATE TRIGGER IF NOT EXISTS memories_ai AFTER INSERT ON memories BEGIN
                INSERT INTO memories_fts(rowid, title, content, tags)
                VALUES (new.rowid, new.title, new.content, new.tags);
            END;

            CREATE TRIGGER IF NOT EXISTS memories_ad AFTER DELETE ON memories BEGIN
                INSERT INTO memories_fts(memories_fts, rowid, title, content, tags)
                VALUES ('delete', old.rowid, old.title, old.content, old.tags);
            END;

            CREATE TRIGGER IF NOT EXISTS memories_au AFTER UPDATE ON memories BEGIN
                INSERT INTO memories_fts(memories_fts, rowid, title, content, tags)
                VALUES ('delete', old.rowid, old.title, old.content, old.tags);
                INSERT INTO memories_fts(rowid, title, content, tags)
                VALUES (new.rowid, new.title, new.content, new.tags);
            END;
            ",
        )?;

        // Migration: add embedding column to existing databases
        Self::migrate_add_column(&conn, "memories", "embedding", "BLOB");

        info!("Storage initialized at {:?}", conn.path());
        Ok(())
    }

    /// Safe column migration — ignores "duplicate column" errors
    fn migrate_add_column(conn: &Connection, table: &str, column: &str, col_type: &str) {
        let sql = format!("ALTER TABLE {table} ADD COLUMN {column} {col_type}");
        if conn.execute(&sql, []).is_ok() {
            info!("Migration: added column {table}.{column}");
        }
        // Column already exists — fine
    }

    pub fn save(&self, entry: &MemoryEntry) -> Result<()> {
        self.save_with_embedding(entry, None)
    }

    pub fn save_with_embedding(
        &self,
        entry: &MemoryEntry,
        embedding: Option<&Embedding>,
    ) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let blob = embedding.map(|e| embedding_to_bytes(e));
        conn.execute(
            "INSERT OR REPLACE INTO memories (id, timestamp, title, content, memory_type, tags, source, importance, metadata, embedding)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                entry.id,
                entry.timestamp.to_rfc3339(),
                entry.title,
                entry.content,
                entry.memory_type.to_string(),
                serde_json::to_string(&entry.tags)?,
                serde_json::to_string(&entry.source)?,
                entry.importance,
                entry.metadata.to_string(),
                blob,
            ],
        )?;
        Ok(())
    }

    /// Check if a similar memory already exists (cosine > threshold).
    /// Returns Some(similarity) if duplicate found, None if unique.
    pub fn is_duplicate(&self, embedding: &Embedding, threshold: f32) -> Result<Option<f32>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let mut stmt = conn.prepare(
            "SELECT embedding FROM memories WHERE embedding IS NOT NULL ORDER BY timestamp DESC LIMIT 200",
        )?;

        let rows: Vec<Vec<u8>> = stmt
            .query_map([], |row| row.get::<_, Vec<u8>>(0))?
            .filter_map(|r| r.ok())
            .collect();

        for blob in &rows {
            let existing = embedding_from_bytes(blob);
            let sim = cosine_similarity(embedding, &existing);
            if sim >= threshold {
                debug!("Duplicate found: cosine={sim:.4} >= threshold={threshold:.4}");
                return Ok(Some(sim));
            }
        }

        Ok(None)
    }

    /// Find memories most similar to a given embedding
    pub fn find_similar(
        &self,
        embedding: &Embedding,
        limit: usize,
    ) -> Result<Vec<(MemoryEntry, f32)>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let mut stmt = conn.prepare(
            "SELECT id, timestamp, title, content, memory_type, tags, source, importance, metadata, embedding
             FROM memories WHERE embedding IS NOT NULL",
        )?;

        let mut scored: Vec<(StorageRow, Vec<u8>, f32)> = stmt
            .query_map([], |row| {
                Ok((
                    StorageRow {
                        id: row.get(0)?,
                        timestamp: row.get(1)?,
                        title: row.get(2)?,
                        content: row.get(3)?,
                        memory_type: row.get(4)?,
                        tags: row.get(5)?,
                        source: row.get(6)?,
                        importance: row.get(7)?,
                        metadata: row.get(8)?,
                    },
                    row.get::<_, Vec<u8>>(9)?,
                ))
            })?
            .filter_map(|r| r.ok())
            .map(|(row, blob)| {
                let existing = embedding_from_bytes(&blob);
                let sim = cosine_similarity(embedding, &existing);
                (row, blob, sim)
            })
            .collect();

        // Sort by similarity descending
        scored.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(limit);

        let results: Vec<(MemoryEntry, f32)> = scored
            .into_iter()
            .filter_map(|(row, _, sim)| row.into_memory_entry().ok().map(|e| (e, sim)))
            .collect();

        Ok(results)
    }

    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<MemoryEntry>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let mut stmt = conn.prepare(
            "SELECT m.id, m.timestamp, m.title, m.content, m.memory_type, m.tags, m.source, m.importance, m.metadata
             FROM memories_fts fts
             JOIN memories m ON m.rowid = fts.rowid
             WHERE memories_fts MATCH ?1
             ORDER BY rank
             LIMIT ?2",
        )?;

        let entries = stmt
            .query_map(params![query, limit as i64], |row| {
                Ok(StorageRow {
                    id: row.get(0)?,
                    timestamp: row.get(1)?,
                    title: row.get(2)?,
                    content: row.get(3)?,
                    memory_type: row.get(4)?,
                    tags: row.get(5)?,
                    source: row.get(6)?,
                    importance: row.get(7)?,
                    metadata: row.get(8)?,
                })
            })?
            .filter_map(|r| r.ok())
            .filter_map(|row| row.into_memory_entry().ok())
            .collect();

        Ok(entries)
    }

    pub fn recent(&self, limit: usize) -> Result<Vec<MemoryEntry>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let mut stmt = conn.prepare(
            "SELECT id, timestamp, title, content, memory_type, tags, source, importance, metadata
             FROM memories
             ORDER BY timestamp DESC
             LIMIT ?1",
        )?;

        let entries = stmt
            .query_map(params![limit as i64], |row| {
                Ok(StorageRow {
                    id: row.get(0)?,
                    timestamp: row.get(1)?,
                    title: row.get(2)?,
                    content: row.get(3)?,
                    memory_type: row.get(4)?,
                    tags: row.get(5)?,
                    source: row.get(6)?,
                    importance: row.get(7)?,
                    metadata: row.get(8)?,
                })
            })?
            .filter_map(|r| r.ok())
            .filter_map(|row| row.into_memory_entry().ok())
            .collect();

        Ok(entries)
    }

    pub fn count(&self) -> Result<usize> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM memories", [], |row| row.get(0))?;
        Ok(count as usize)
    }

    pub fn stats(&self) -> Result<StorageStats> {
        let total = self.count()?;
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;

        let mut stmt =
            conn.prepare("SELECT memory_type, COUNT(*) FROM memories GROUP BY memory_type")?;

        let by_type: Vec<(String, usize)> = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as usize))
            })?
            .filter_map(|r| r.ok())
            .collect();

        Ok(StorageStats { total, by_type })
    }

    /// Export all memories as JSON array
    pub fn export_all(&self) -> Result<Vec<serde_json::Value>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let mut stmt = conn.prepare(
            "SELECT id, timestamp, title, content, memory_type, tags, source, importance, metadata
             FROM memories ORDER BY timestamp ASC",
        )?;

        let mut entries = Vec::new();
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            entries.push(serde_json::json!({
                "id": row.get::<_, String>(0)?,
                "timestamp": row.get::<_, String>(1)?,
                "title": row.get::<_, String>(2)?,
                "content": row.get::<_, String>(3)?,
                "memory_type": row.get::<_, String>(4)?,
                "tags": row.get::<_, String>(5)?,
                "source": row.get::<_, String>(6)?,
                "importance": row.get::<_, f64>(7)?,
                "metadata": row.get::<_, String>(8)?,
            }));
        }

        Ok(entries)
    }

    /// Import memories from JSON array (skips duplicates by id)
    pub fn import_entries(&self, entries: &[serde_json::Value]) -> Result<(usize, usize)> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let mut imported = 0;
        let mut skipped = 0;

        for entry in entries {
            let id = entry["id"].as_str().unwrap_or_default();
            let exists: bool = conn
                .query_row(
                    "SELECT COUNT(*) > 0 FROM memories WHERE id = ?1",
                    params![id],
                    |row| row.get(0),
                )
                .unwrap_or(false);

            if exists {
                skipped += 1;
                continue;
            }

            conn.execute(
                "INSERT INTO memories (id, timestamp, title, content, memory_type, tags, source, importance, metadata)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    id,
                    entry["timestamp"].as_str().unwrap_or_default(),
                    entry["title"].as_str().unwrap_or_default(),
                    entry["content"].as_str().unwrap_or_default(),
                    entry["memory_type"].as_str().unwrap_or("note"),
                    entry["tags"].as_str().unwrap_or("[]"),
                    entry["source"].as_str().unwrap_or("\"manual\""),
                    entry["importance"].as_f64().unwrap_or(0.5),
                    entry["metadata"].as_str().unwrap_or("{}"),
                ],
            )?;
            imported += 1;
        }

        Ok((imported, skipped))
    }

    /// Cleanup old low-importance memories.
    /// Keeps: decisions (forever), feedback (forever), high-importance (>= threshold).
    /// Removes: notes older than max_age_days with importance < threshold.
    pub fn cleanup(&self, max_age_days: i64, importance_threshold: f32) -> Result<usize> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let cutoff = chrono::Utc::now() - chrono::Duration::days(max_age_days);
        let cutoff_str = cutoff.to_rfc3339();

        let deleted = conn.execute(
            "DELETE FROM memories
             WHERE memory_type NOT IN ('decision', 'feedback')
             AND importance < ?1
             AND timestamp < ?2",
            params![importance_threshold as f64, cutoff_str],
        )?;

        if deleted > 0 {
            conn.execute(
                "INSERT INTO memories_fts(memories_fts) VALUES('rebuild')",
                [],
            )?;
            info!("Cleanup: removed {deleted} old low-importance memories");
        }

        Ok(deleted)
    }

    /// Daily memory counts for the last N days (for sparkline graphs)
    pub fn daily_counts(&self, days: usize) -> Result<Vec<(String, usize)>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let mut stmt = conn.prepare(
            "SELECT date(timestamp) as d, COUNT(*) as c
             FROM memories
             WHERE timestamp >= datetime('now', ?1)
             GROUP BY d
             ORDER BY d ASC",
        )?;
        let offset = format!("-{days} days");
        let rows: Vec<(String, usize)> = stmt
            .query_map([&offset], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as usize))
            })?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    }

    /// Timestamp of the most recent memory entry
    pub fn last_activity(&self) -> Result<Option<String>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let result: Option<String> = conn
            .query_row(
                "SELECT timestamp FROM memories ORDER BY timestamp DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .ok();
        Ok(result)
    }

    /// Dedup stats: how many entries were saved vs have embeddings
    pub fn dedup_estimate(&self) -> Result<(usize, usize)> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let saved: i64 = conn.query_row("SELECT COUNT(*) FROM memories", [], |row| row.get(0))?;
        // embeddings table may not exist in older DBs
        let with_emb: i64 = conn
            .query_row("SELECT COUNT(*) FROM embeddings", [], |row| row.get(0))
            .unwrap_or(0);
        Ok((saved as usize, with_emb as usize))
    }

    /// Get database file size in bytes
    pub fn db_size(&self) -> Result<u64> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        match conn.path() {
            Some(path) => Ok(std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)),
            None => Ok(0),
        }
    }
}

struct StorageRow {
    id: String,
    timestamp: String,
    title: String,
    content: String,
    memory_type: String,
    tags: String,
    source: String,
    importance: f64,
    metadata: String,
}

impl StorageRow {
    fn into_memory_entry(self) -> Result<MemoryEntry, anyhow::Error> {
        use crate::event::{EventSource, MemoryType};

        let memory_type = match self.memory_type.as_str() {
            "decision" => MemoryType::Decision,
            "feedback" => MemoryType::Feedback,
            "session_summary" => MemoryType::SessionSummary,
            "security" => MemoryType::Security,
            _ => MemoryType::Note,
        };

        let source: EventSource = serde_json::from_str(&self.source).unwrap_or(EventSource::Manual);
        let tags: Vec<String> = serde_json::from_str(&self.tags).unwrap_or_default();
        let metadata: serde_json::Value =
            serde_json::from_str(&self.metadata).unwrap_or(serde_json::Value::Null);
        let timestamp = chrono::DateTime::parse_from_rfc3339(&self.timestamp)
            .map(|dt| dt.with_timezone(&chrono::Utc))
            .unwrap_or_else(|_| chrono::Utc::now());

        Ok(MemoryEntry {
            id: self.id,
            timestamp,
            title: self.title,
            content: self.content,
            memory_type,
            tags,
            source,
            importance: self.importance as f32,
            metadata,
        })
    }
}

#[derive(Debug)]
pub struct StorageStats {
    pub total: usize,
    pub by_type: Vec<(String, usize)>,
}

impl std::fmt::Display for StorageStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "Total memories: {}", self.total)?;
        for (t, count) in &self.by_type {
            writeln!(f, "  {t}: {count}")?;
        }
        Ok(())
    }
}

/// Trait for output sinks — extensible for Whisper (Phase 2), Obsidian, etc.
pub trait OutputSink: Send + Sync {
    fn write(&self, entry: &MemoryEntry) -> Result<()>;
    fn name(&self) -> &str;
}
