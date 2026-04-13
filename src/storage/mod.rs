use anyhow::Result;
use rusqlite::{Connection, params};
use std::path::Path;
use std::sync::Mutex;
use tracing::{debug, info};

use crate::embedding::{Embedding, cosine_similarity, embedding_from_bytes, embedding_to_bytes};
use crate::event::MemoryEntry;
use crate::graph::{Edge, Entity};

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

        // Knowledge graph tables
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS entities (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL UNIQUE,
                entity_type TEXT NOT NULL DEFAULT 'concept',
                mention_count INTEGER NOT NULL DEFAULT 1,
                first_seen TEXT NOT NULL DEFAULT (datetime('now')),
                last_seen TEXT NOT NULL DEFAULT (datetime('now'))
            );

            CREATE INDEX IF NOT EXISTS idx_entities_name ON entities(name);
            CREATE INDEX IF NOT EXISTS idx_entities_type ON entities(entity_type);

            CREATE TABLE IF NOT EXISTS edges (
                id TEXT PRIMARY KEY,
                source_entity TEXT NOT NULL,
                target_entity TEXT NOT NULL,
                relation TEXT NOT NULL,
                memory_id TEXT,
                weight REAL NOT NULL DEFAULT 1.0,
                timestamp TEXT NOT NULL DEFAULT (datetime('now')),
                UNIQUE(source_entity, target_entity, relation, memory_id)
            );

            CREATE INDEX IF NOT EXISTS idx_edges_source ON edges(source_entity);
            CREATE INDEX IF NOT EXISTS idx_edges_target ON edges(target_entity);

            CREATE TABLE IF NOT EXISTS memory_entities (
                memory_id TEXT NOT NULL,
                entity_id TEXT NOT NULL,
                PRIMARY KEY (memory_id, entity_id)
            );

            CREATE INDEX IF NOT EXISTS idx_me_entity ON memory_entities(entity_id);
            ",
        )?;

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

    // === Knowledge Graph Methods ===

    /// Upsert an entity — create if new, bump mention_count if exists
    pub fn upsert_entity(&self, entity: &Entity) -> Result<String> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let now = chrono::Utc::now().to_rfc3339();

        // Try to find existing entity by name
        let existing_id: Option<String> = conn
            .query_row(
                "SELECT id FROM entities WHERE name = ?1",
                params![entity.name],
                |row| row.get(0),
            )
            .ok();

        if let Some(id) = existing_id {
            conn.execute(
                "UPDATE entities SET mention_count = mention_count + 1, last_seen = ?1 WHERE id = ?2",
                params![now, id],
            )?;
            Ok(id)
        } else {
            let id = uuid::Uuid::new_v4().to_string();
            conn.execute(
                "INSERT INTO entities (id, name, entity_type, mention_count, first_seen, last_seen)
                 VALUES (?1, ?2, ?3, 1, ?4, ?4)",
                params![id, entity.name, entity.entity_type.to_string(), now],
            )?;
            Ok(id)
        }
    }

    /// Save an edge between two entities
    pub fn save_edge(&self, edge: &Edge) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();

        // INSERT OR IGNORE to skip duplicate edges
        conn.execute(
            "INSERT OR IGNORE INTO edges (id, source_entity, target_entity, relation, memory_id, weight, timestamp)
             VALUES (?1, ?2, ?3, ?4, ?5, 1.0, ?6)",
            params![id, edge.source, edge.target, edge.relation, edge.memory_id, now],
        )?;

        // If edge already existed, bump weight
        conn.execute(
            "UPDATE edges SET weight = weight + 0.5
             WHERE source_entity = ?1 AND target_entity = ?2 AND relation = ?3 AND memory_id != ?4",
            params![edge.source, edge.target, edge.relation, edge.memory_id],
        )?;

        Ok(())
    }

    /// Link a memory to an entity
    pub fn link_memory_entity(&self, memory_id: &str, entity_id: &str) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        conn.execute(
            "INSERT OR IGNORE INTO memory_entities (memory_id, entity_id) VALUES (?1, ?2)",
            params![memory_id, entity_id],
        )?;
        Ok(())
    }

    /// Save extraction results: entities, edges, and links to memory
    pub fn save_graph(
        &self,
        memory_id: &str,
        entities: &[Entity],
        edges: &[Edge],
    ) -> Result<()> {
        for entity in entities {
            let entity_id = self.upsert_entity(entity)?;
            self.link_memory_entity(memory_id, &entity_id)?;
        }
        for edge in edges {
            self.save_edge(edge)?;
        }
        Ok(())
    }

    /// Query the graph: find all connections for an entity
    pub fn graph_query(&self, entity_name: &str) -> Result<GraphResult> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let name_lower = entity_name.to_lowercase();

        // Find the entity
        let entity_row: Option<(String, String, i64, String, String)> = conn
            .query_row(
                "SELECT id, entity_type, mention_count, first_seen, last_seen FROM entities WHERE lower(name) = ?1",
                params![name_lower],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
            )
            .ok();

        let (entity_id, entity_type, mention_count, first_seen, last_seen) = match entity_row {
            Some(r) => r,
            None => return Ok(GraphResult::not_found(entity_name)),
        };

        // Find all edges where this entity is source or target
        let mut edges = Vec::new();
        {
            let mut stmt = conn.prepare(
                "SELECT source_entity, target_entity, relation, weight FROM edges
                 WHERE source_entity = ?1 OR target_entity = ?1
                 ORDER BY weight DESC",
            )?;
            let rows = stmt.query_map(params![name_lower], |row| {
                Ok(GraphEdgeResult {
                    source: row.get(0)?,
                    target: row.get(1)?,
                    relation: row.get(2)?,
                    weight: row.get(3)?,
                })
            })?;
            for row in rows {
                if let Ok(edge) = row {
                    edges.push(edge);
                }
            }
        }

        // Find related memories
        let mut memories = Vec::new();
        {
            let mut stmt = conn.prepare(
                "SELECT m.id, m.title, m.memory_type, m.importance, m.timestamp
                 FROM memories m
                 JOIN memory_entities me ON me.memory_id = m.id
                 WHERE me.entity_id = ?1
                 ORDER BY m.timestamp DESC
                 LIMIT 20",
            )?;
            let rows = stmt.query_map(params![entity_id], |row| {
                Ok(GraphMemoryResult {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    memory_type: row.get(2)?,
                    importance: row.get(3)?,
                    timestamp: row.get(4)?,
                })
            })?;
            for row in rows {
                if let Ok(mem) = row {
                    memories.push(mem);
                }
            }
        }

        // Find connected entities (neighbors)
        let mut neighbors = Vec::new();
        {
            let mut stmt = conn.prepare(
                "SELECT DISTINCT e.name, e.entity_type, e.mention_count
                 FROM entities e
                 JOIN edges ed ON (ed.source_entity = e.name OR ed.target_entity = e.name)
                 WHERE (ed.source_entity = ?1 OR ed.target_entity = ?1)
                   AND e.name != ?1
                 ORDER BY e.mention_count DESC
                 LIMIT 20",
            )?;
            let rows = stmt.query_map(params![name_lower], |row| {
                Ok(GraphNeighbor {
                    name: row.get(0)?,
                    entity_type: row.get(1)?,
                    mention_count: row.get(2)?,
                })
            })?;
            for row in rows {
                if let Ok(n) = row {
                    neighbors.push(n);
                }
            }
        }

        Ok(GraphResult {
            entity_name: name_lower,
            entity_type,
            mention_count,
            first_seen,
            last_seen,
            edges,
            memories,
            neighbors,
            found: true,
        })
    }

    /// List all entities, sorted by mention count
    pub fn list_entities(&self, limit: usize) -> Result<Vec<(String, String, i64)>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let mut stmt = conn.prepare(
            "SELECT name, entity_type, mention_count FROM entities ORDER BY mention_count DESC LIMIT ?1",
        )?;
        let rows: Vec<(String, String, i64)> = stmt
            .query_map(params![limit as i64], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    }

    /// Count entities and edges
    pub fn graph_stats(&self) -> Result<(usize, usize)> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let entities: i64 =
            conn.query_row("SELECT COUNT(*) FROM entities", [], |row| row.get(0))?;
        let edges: i64 =
            conn.query_row("SELECT COUNT(*) FROM edges", [], |row| row.get(0))?;
        Ok((entities as usize, edges as usize))
    }
}

// === Graph Result Types ===

#[derive(Debug, serde::Serialize)]
pub struct GraphResult {
    pub entity_name: String,
    pub entity_type: String,
    pub mention_count: i64,
    pub first_seen: String,
    pub last_seen: String,
    pub edges: Vec<GraphEdgeResult>,
    pub memories: Vec<GraphMemoryResult>,
    pub neighbors: Vec<GraphNeighbor>,
    pub found: bool,
}

impl GraphResult {
    fn not_found(name: &str) -> Self {
        Self {
            entity_name: name.to_string(),
            entity_type: String::new(),
            mention_count: 0,
            first_seen: String::new(),
            last_seen: String::new(),
            edges: Vec::new(),
            memories: Vec::new(),
            neighbors: Vec::new(),
            found: false,
        }
    }
}

#[derive(Debug, serde::Serialize)]
pub struct GraphEdgeResult {
    pub source: String,
    pub target: String,
    pub relation: String,
    pub weight: f64,
}

#[derive(Debug, serde::Serialize)]
pub struct GraphMemoryResult {
    pub id: String,
    pub title: String,
    pub memory_type: String,
    pub importance: f64,
    pub timestamp: String,
}

#[derive(Debug, serde::Serialize)]
pub struct GraphNeighbor {
    pub name: String,
    pub entity_type: String,
    pub mention_count: i64,
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
