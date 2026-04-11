use anyhow::Result;
use rusqlite::{params, Connection};
use std::path::Path;
use std::sync::Mutex;
use tracing::info;

use crate::event::MemoryEntry;

/// SQLite-backed memory storage (thread-safe via Mutex)
pub struct Storage {
    conn: Mutex<Connection>,
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

        info!("Storage initialized at {:?}", conn.path());
        Ok(())
    }

    pub fn save(&self, entry: &MemoryEntry) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        conn.execute(
            "INSERT OR REPLACE INTO memories (id, timestamp, title, content, memory_type, tags, source, importance, metadata)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
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
            ],
        )?;
        Ok(())
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
        let count: i64 =
            conn.query_row("SELECT COUNT(*) FROM memories", [], |row| row.get(0))?;
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

        let source: EventSource =
            serde_json::from_str(&self.source).unwrap_or(EventSource::Manual);
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
