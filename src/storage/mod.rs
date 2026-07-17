pub mod hnsw_index;

use anyhow::Result;
use rusqlite::{Connection, params};
use std::path::Path;
use std::sync::Mutex;
use tracing::{debug, info, warn};

use crate::embedding::{Embedding, cosine_similarity, embedding_from_bytes, embedding_to_bytes};
use crate::event::MemoryEntry;
use crate::graph::{Edge, Entity};

use self::hnsw_index::HnswIndex;

/// SQLite-backed memory storage (thread-safe via Mutex)
/// Vector search uses HNSW index for O(log n) approximate nearest neighbor.
pub struct Storage {
    pub(crate) conn: Mutex<Connection>,
    hnsw: Mutex<HnswIndex>,
}

impl Storage {
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_with_options(path, true)
    }

    /// Open storage without building the HNSW index. Used for bulk operations
    /// like `mnemonic reembed` where the embedding dimension may change and
    /// rebuilding HNSW mid-operation would panic on dimension mismatch.
    /// Caller should re-open the DB normally after the bulk op completes.
    pub fn open_no_hnsw(path: &Path) -> Result<Self> {
        Self::open_with_options(path, false)
    }

    fn open_with_options(path: &Path, build_hnsw: bool) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
            tighten_default_data_dir(parent);
        }

        let conn = Connection::open(path)?;
        // Defence in depth: the DB holds verbatim memory content, so lock it to
        // 0600 rather than relying solely on the 0700 parent dir. Best-effort.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
        // WAL mode: concurrent readers don't block on a single writer (daemon).
        // busy_timeout: if locked, retry for up to 5s instead of hanging forever.
        // wal_autocheckpoint: auto-checkpoint WAL every 1000 pages (~4MB) so the
        //   WAL file doesn't grow unbounded. Prevents multi-MB WAL files observed
        //   in production (memory.db-wal growing 3x the size of memory.db).
        // Without WAL+busy_timeout, CLI invocations during daemon writes hang in
        // UE state on macOS (see tests::reader_does_not_hang_during_writer).
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "busy_timeout", 5000)?;
        conn.pragma_update(None, "wal_autocheckpoint", 1000)?;
        // SQLite ignores FOREIGN KEY constraints by default — they're parsed
        // but not enforced unless this pragma is on. Without it, orphan
        // memory_peers / sessions rows can land without anyone noticing.
        // Setting it BEFORE init_schema so any FK-bearing DDL is validated
        // from the first connection on.
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let hnsw = HnswIndex::new(50_000);
        let storage = Self {
            conn: Mutex::new(conn),
            hnsw: Mutex::new(hnsw),
        };
        storage.init_schema()?;
        if build_hnsw {
            storage.rebuild_hnsw_index()?;
        }
        // Refresh backup snapshot if stale (>24h) or missing.
        // Uses SQLite VACUUM INTO for a consistent copy (WAL-safe).
        let _ = storage.refresh_backup_if_stale(path);
        Ok(storage)
    }

    /// Create or refresh `memory.db.bak` next to the main DB if the existing
    /// backup is missing or older than 24h. Uses `VACUUM INTO` for a
    /// WAL-consistent snapshot — safe even while the daemon is running.
    /// Silently no-ops on error: backups are best-effort, not critical-path.
    fn refresh_backup_if_stale(&self, db_path: &Path) -> Result<()> {
        let bak_path = {
            let mut p = db_path.to_path_buf();
            let name = p
                .file_name()
                .map(|n| format!("{}.bak", n.to_string_lossy()))
                .unwrap_or_else(|| "memory.db.bak".to_string());
            p.set_file_name(name);
            p
        };

        // Skip if backup is fresh (< 24h old).
        if let Ok(meta) = std::fs::metadata(&bak_path)
            && let Ok(modified) = meta.modified()
            && let Ok(age) = std::time::SystemTime::now().duration_since(modified)
            && age < std::time::Duration::from_secs(24 * 3600)
        {
            return Ok(());
        }

        // VACUUM INTO requires a non-existent target path.
        let tmp_path = {
            let mut p = bak_path.clone();
            p.set_extension("bak.tmp");
            p
        };
        let _ = std::fs::remove_file(&tmp_path);

        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        conn.execute(
            &format!(
                "VACUUM INTO '{}'",
                tmp_path.to_string_lossy().replace('\'', "''")
            ),
            [],
        )?;
        drop(conn);

        std::fs::rename(&tmp_path, &bak_path)?;
        info!("Backup refreshed: {}", bak_path.display());
        Ok(())
    }

    /// Rebuild HNSW index from all embeddings in SQLite.
    /// Called once on startup — O(n) scan, then O(log n) searches.
    fn rebuild_hnsw_index(&self) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        // Superseded memories are filtered out of every retrieval path, so
        // indexing them only creates ghost hits that eat result slots and
        // trip the dedup gate. Newest-first so the index dimension locks to
        // the ACTIVE embedder's output (the most recent write), not whatever
        // dimension an ancient row happens to have — a hash↔neural switch
        // otherwise builds an index the live embedder can't query.
        let mut stmt = conn.prepare(
            "SELECT id, embedding FROM memories
             WHERE embedding IS NOT NULL AND superseded_by IS NULL
             ORDER BY timestamp DESC",
        )?;

        let rows: Vec<(String, Vec<u8>)> = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
            })?
            .filter_map(|r| r.ok())
            .collect();

        let count = rows.len();
        if count == 0 {
            return Ok(());
        }

        let mut hnsw = self.hnsw.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        // The dimension is set by the first vector we insert. Subsequent vectors
        // of different dimension would panic inside anndists — skip them with a
        // warning. This can happen transiently during a reembed migration.
        let mut dim: Option<usize> = None;
        let mut indexed = 0usize;
        let mut skipped = 0usize;
        for (id, blob) in &rows {
            let embedding = embedding_from_bytes(blob);
            let d = embedding.len();
            match dim {
                None => {
                    dim = Some(d);
                }
                Some(expected) if expected != d => {
                    tracing::warn!(
                        "Skipping memory {id}: embedding dim {d} ≠ expected {expected}. \
                         Run `mnemonic reembed` to migrate."
                    );
                    skipped += 1;
                    continue;
                }
                _ => {}
            }
            hnsw.insert(id, &embedding);
            indexed += 1;
        }

        if skipped > 0 {
            info!(
                "HNSW index rebuilt: {indexed} vectors indexed, {skipped} skipped (dim mismatch)"
            );
        } else {
            info!("HNSW index rebuilt: {indexed} vectors indexed");
        }
        Ok(())
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
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                -- Session this memory was captured in. Nullable: memories
                -- saved via `mnemonic save` or imported from non-session
                -- sources have no session. SQLite allows forward FK refs
                -- so the `sessions` table doesn't need to exist yet at
                -- CREATE TABLE time. ON DELETE SET NULL means deleting
                -- a session preserves the memories but clears the link.
                session_id TEXT REFERENCES sessions(id) ON DELETE SET NULL
            );

            CREATE INDEX IF NOT EXISTS idx_memories_type ON memories(memory_type);
            CREATE INDEX IF NOT EXISTS idx_memories_timestamp ON memories(timestamp);
            CREATE INDEX IF NOT EXISTS idx_memories_importance ON memories(importance);
            -- NOTE: idx_memories_session is intentionally NOT created here.
            -- On legacy DBs the memories table already exists, so the
            -- CREATE TABLE above is a no-op and the session_id column
            -- has not been added yet — running CREATE INDEX on the column
            -- here would crash with a missing-column error. The migration
            -- block at the end of init_schema adds the column AND creates
            -- this index, so both fresh and legacy DBs converge there.

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

            -- UPDATE OF: only retokenize when indexed text actually
            -- changes. A bare AFTER UPDATE also fired on touch_access
            -- (access_count / last_accessed_at bumps on every search hit),
            -- re-tokenizing up to a few dozen rows per query for nothing.
            -- Legacy DBs holding the old broad trigger are healed by the
            -- conditional rebuild right below.
            CREATE TRIGGER IF NOT EXISTS memories_au AFTER UPDATE OF title, content, tags ON memories BEGIN
                INSERT INTO memories_fts(memories_fts, rowid, title, content, tags)
                VALUES ('delete', old.rowid, old.title, old.content, old.tags);
                INSERT INTO memories_fts(rowid, title, content, tags)
                VALUES (new.rowid, new.title, new.content, new.tags);
            END;
            ",
        )?;

        // Heal the legacy broad AFTER UPDATE trigger exactly once. Probe
        // sqlite_master first: an unconditional DROP+CREATE on every open
        // takes a write lock even in steady state, and a concurrent writer
        // turns that into spurious SQLITE_BUSY (flaky CI caught it — every
        // CLI invocation was running DDL against the daemon's live DB).
        {
            use rusqlite::OptionalExtension;
            let au_sql: Option<String> = conn
                .query_row(
                    "SELECT sql FROM sqlite_master WHERE type = 'trigger' AND name = 'memories_au'",
                    [],
                    |row| row.get(0),
                )
                .optional()?;
            let legacy = au_sql
                .as_deref()
                .is_some_and(|sql| !sql.contains("AFTER UPDATE OF"));
            if legacy {
                conn.execute_batch(
                    "DROP TRIGGER IF EXISTS memories_au;
                     CREATE TRIGGER memories_au AFTER UPDATE OF title, content, tags ON memories BEGIN
                        INSERT INTO memories_fts(memories_fts, rowid, title, content, tags)
                        VALUES ('delete', old.rowid, old.title, old.content, old.tags);
                        INSERT INTO memories_fts(rowid, title, content, tags)
                        VALUES (new.rowid, new.title, new.content, new.tags);
                     END;",
                )?;
            }
        }

        // Migration: add embedding column to existing databases
        Self::migrate_add_column(&conn, "memories", "embedding", "BLOB");

        // Migration: usage tracking for dynamic effective importance.
        // - access_count: how many times retrieval has touched this entry.
        // - last_accessed_at: timestamp of most recent access (null = never).
        Self::migrate_add_column(
            &conn,
            "memories",
            "access_count",
            "INTEGER NOT NULL DEFAULT 0",
        );
        Self::migrate_add_column(&conn, "memories", "last_accessed_at", "TEXT");

        // Reflection / consolidation columns (Phase 5).
        // - superseded_by: NULL = active; otherwise points at the canonical
        //   memory that replaced this one. Memory is NEVER deleted.
        // - canonical_memory_id: convenience pointer so we can quickly group
        //   a canonical with its sources (mirrors reflection_sources for
        //   single-hop reverse lookup).
        Self::migrate_add_column(&conn, "memories", "superseded_by", "TEXT");
        Self::migrate_add_column(&conn, "memories", "canonical_memory_id", "TEXT");

        // Reflection run log + provenance trail. Audit-only — nothing else
        // reads them at retrieval time, but they make every consolidation
        // reversible by hand and explainable in the UI.
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS reflection_runs (
                id TEXT PRIMARY KEY,
                run_at TEXT NOT NULL DEFAULT (datetime('now')),
                mode TEXT NOT NULL,
                threshold REAL NOT NULL,
                clusters_found INTEGER NOT NULL DEFAULT 0,
                applied_count INTEGER NOT NULL DEFAULT 0,
                synthesizer TEXT NOT NULL DEFAULT 'rule'
            );

            CREATE TABLE IF NOT EXISTS reflection_sources (
                canonical_id TEXT NOT NULL,
                source_id TEXT NOT NULL,
                run_id TEXT NOT NULL,
                cosine REAL NOT NULL,
                position INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (canonical_id, source_id)
            );

            CREATE INDEX IF NOT EXISTS idx_reflection_sources_canonical ON reflection_sources(canonical_id);
            CREATE INDEX IF NOT EXISTS idx_reflection_sources_run ON reflection_sources(run_id);
            CREATE INDEX IF NOT EXISTS idx_memories_superseded ON memories(superseded_by);",
        )?;

        // LLM extraction cache. Keyed by sha256(title|content|extractor_id);
        // value is the JSON-encoded ExtractionResult so repeated identical
        // memories (or full re-imports) don't re-burn LLM calls.
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS llm_extraction_cache (
                content_hash TEXT NOT NULL,
                extractor_id TEXT NOT NULL,
                result_json TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                PRIMARY KEY (content_hash, extractor_id)
            );",
        )?;

        // Pending LLM extractions. When the backend (Ollama) is unreachable
        // or returns malformed JSON, we silently fell back to rule-based only —
        // and that loss was permanent. Now we enqueue the memory id with an
        // exponential-backoff next_attempt_at and let `mnemonic reextract
        // --pending` drain the queue when the model is healthy again.
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS pending_extractions (
                memory_id TEXT PRIMARY KEY,
                attempts INTEGER NOT NULL DEFAULT 0,
                last_error TEXT,
                last_attempt_at TEXT,
                next_attempt_at TEXT NOT NULL DEFAULT (datetime('now')),
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE INDEX IF NOT EXISTS idx_pending_next_attempt
                ON pending_extractions(next_attempt_at);",
        )?;

        // Async extraction queue. Distinct from `pending_extractions`:
        //   - `extraction_queue` = first-attempt work scheduled at save time.
        //     The daemon writes a memory row here so the save path stays
        //     under 100ms; a background worker picks it up and runs the
        //     real entity extractor (rule-based + optional LLM) without
        //     blocking ingestion.
        //   - `pending_extractions` = retry-after-failure queue with
        //     exponential backoff. LlmExtractor moves rows there when
        //     its backend call errors.
        // Keeping the two separate avoids tangling "never been tried" and
        // "failed N times" into the same row.
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS extraction_queue (
                memory_id TEXT PRIMARY KEY,
                enqueued_at TEXT NOT NULL DEFAULT (datetime('now')),
                attempts INTEGER NOT NULL DEFAULT 0
            );
            CREATE INDEX IF NOT EXISTS idx_extraction_queue_enqueued
                ON extraction_queue(enqueued_at);",
        )?;
        // Legacy DBs predate the attempts counter (head-of-line fix).
        Self::migrate_add_column(
            &conn,
            "extraction_queue",
            "attempts",
            "INTEGER NOT NULL DEFAULT 0",
        );

        // Temporal facts. Edges in the knowledge graph say "X uses Y" — fine
        // for static relations. But many "facts" Mnemonic ingests have a
        // value AND a timestamp: a project's price changes over time, a
        // deadline gets pushed, a stack swaps out a dependency. Asking
        // "what's the *current* price of inventory-labeler?" against the
        // edges table is ambiguous — multiple memories from different dates
        // each have their own "costs" edge.
        //
        // The facts table treats this explicitly. Each row is one assertion
        // (`subject`, `predicate`, `value`) sourced from one memory. When a
        // new fact with the same (subject, predicate) lands, the previous
        // current fact's `valid_to` is set to the new fact's `valid_from`
        // and the new one becomes current (`valid_to = NULL`). The full
        // chain stays in the table — never deleted — so "what did we used
        // to think before?" queries work too.
        //
        // Confidence stays at 1.0 for now (rule-based + LLM extraction
        // produce binary assertions). When we add inductive conclusions
        // ("User usually prefers TS in frontend"), confidence will start
        // doing real work.
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS facts (
                id TEXT PRIMARY KEY,
                subject TEXT NOT NULL,
                predicate TEXT NOT NULL,
                value TEXT NOT NULL,
                valid_from TEXT NOT NULL,
                valid_to TEXT,
                confidence REAL NOT NULL DEFAULT 1.0,
                source_memory_id TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE INDEX IF NOT EXISTS idx_facts_subject_predicate
                ON facts(subject, predicate);
            -- UNIQUE partial index enforces 'at most one current fact per
            -- (subject, predicate)' at the DB level. add_fact already
            -- guarantees this via its supersede transaction, but a
            -- DB-level constraint catches any future code path that
            -- bypasses add_fact (raw SQL imports, schema migrations,
            -- direct INSERTs in tests).
            CREATE UNIQUE INDEX IF NOT EXISTS idx_facts_current_unique
                ON facts(subject, predicate) WHERE valid_to IS NULL;
            CREATE INDEX IF NOT EXISTS idx_facts_source
                ON facts(source_memory_id);",
        )?;

        // Inductive conclusions. Facts are atomic point-in-time observations
        // (subject/predicate/value). Conclusions are the next layer up:
        // higher-level patterns, preferences, or trends induced from a
        // cluster of memories. Example facts: "user uses_editor neovim",
        // "user language_pref rust". Example conclusion: "user prefers
        // low-overhead developer tooling" — a claim that no single memory
        // states explicitly but that emerges from many.
        //
        // v1 is foundation only: schema + storage helpers + CLI for manual
        // entry (useful for testing and curated insights). v2 will plug
        // an LLM-driven generator into the existing async extraction worker
        // to mine conclusions from memory clusters.
        //
        // `subject` is the canonical entity name (e.g. "user") or the
        // sentinel "_global" for conclusions that aren't about a specific
        // entity. `kind` is free-form text ("pattern" | "preference" |
        // "trend" | "observation") so we don't need a migration when a
        // new category appears. `support_count` is denormalized cache of
        // conclusion_sources row count — kept in sync by the storage
        // helpers, useful for cheap ranking.
        //
        // Supersede semantics mirror facts: `superseded_by` points at the
        // replacement conclusion. ON DELETE SET NULL so deleting a
        // replacement doesn't cascade-wipe the original. `current_conclusions`
        // helper filters `superseded_by IS NULL`.
        //
        // `conclusion_sources` is the M:N link to the memories that
        // support a conclusion — provides traceable evidence and enables
        // cascade cleanup if a memory is forgotten.
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS conclusions (
                id TEXT PRIMARY KEY,
                subject TEXT NOT NULL,
                kind TEXT NOT NULL DEFAULT 'pattern',
                statement TEXT NOT NULL,
                confidence REAL NOT NULL DEFAULT 0.5,
                support_count INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                last_evaluated_at TEXT NOT NULL DEFAULT (datetime('now')),
                superseded_by TEXT,
                FOREIGN KEY (superseded_by) REFERENCES conclusions(id) ON DELETE SET NULL
            );
            CREATE INDEX IF NOT EXISTS idx_conclusions_subject
                ON conclusions(subject);
            CREATE INDEX IF NOT EXISTS idx_conclusions_current
                ON conclusions(subject) WHERE superseded_by IS NULL;
            CREATE TABLE IF NOT EXISTS conclusion_sources (
                conclusion_id TEXT NOT NULL,
                memory_id TEXT NOT NULL,
                PRIMARY KEY (conclusion_id, memory_id),
                FOREIGN KEY (conclusion_id) REFERENCES conclusions(id) ON DELETE CASCADE,
                FOREIGN KEY (memory_id) REFERENCES memories(id) ON DELETE CASCADE
            );
            CREATE INDEX IF NOT EXISTS idx_conclusion_sources_memory
                ON conclusion_sources(memory_id);
            -- Keep `conclusions.support_count` consistent with the real
            -- row count in `conclusion_sources`. Codex caught that the
            -- denormalized cache drifts when a supporting memory is
            -- forgotten — ON DELETE CASCADE removes the link but the
            -- cached count stayed stale. Triggers fire on every
            -- INSERT/DELETE (including those caused by cascade), so the
            -- cache is always equal to COUNT(*) of the link table.
            CREATE TRIGGER IF NOT EXISTS conclusion_sources_after_insert
                AFTER INSERT ON conclusion_sources
            BEGIN
                UPDATE conclusions
                   SET support_count = support_count + 1
                 WHERE id = NEW.conclusion_id;
            END;
            CREATE TRIGGER IF NOT EXISTS conclusion_sources_after_delete
                AFTER DELETE ON conclusion_sources
            BEGIN
                UPDATE conclusions
                   SET support_count = support_count - 1
                 WHERE id = OLD.conclusion_id;
            END;

            -- Semantic contradiction lint (design-reviewed): AUDIT table,
            -- deliberately separate from memories.superseded_by — that
            -- column means retrieval-hidden dedup consolidation, while a
            -- confirmed conflict is a historically-reversed decision that
            -- must stay visible in history and only be excluded from
            -- standing-decision surfaces (digests / Key Decisions).
            CREATE TABLE IF NOT EXISTS decision_conflicts (
                old_id TEXT NOT NULL,
                new_id TEXT NOT NULL,
                project TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'candidate',
                confidence REAL,
                reason TEXT,
                checked_at TEXT NOT NULL DEFAULT (datetime('now')),
                checker_version INTEGER NOT NULL DEFAULT 1,
                PRIMARY KEY (old_id, new_id)
            );
            CREATE INDEX IF NOT EXISTS idx_decision_conflicts_status
                ON decision_conflicts(status);",
        )?;

        // Peers and sessions. Today every memory is anonymous: the daemon
        // sees a chat-transcript event, classifies it, and saves a row with
        // a `source` enum (Manual / Watcher / etc.) — but no record of WHO
        // said it. User works with Claude AND Codex in parallel; their
        // decisions land in the same store with no way to attribute or
        // filter. Same problem for client conversations, paired-programming
        // sessions, multi-agent setups.
        //
        // Peers are first-class identities (User, Claude, Codex, a client,
        // a teammate). Sessions group memories from one logical thread —
        // a Claude Code JSONL session, a phone call, a workday.
        // `memory_peers` is the M:N link: a memory can have multiple
        // peers attached, each with a role ("speaker", "subject",
        // "mentioned"). Role is free-form text so we don't have to grow
        // an enum every time a new use-case appears.
        //
        // Names are lowercased + trimmed for matching ("User" → "user")
        // so case-different references find the same peer. Display case
        // stays in `display_name`.
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS peers (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL UNIQUE,
                display_name TEXT,
                kind TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                last_seen_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE INDEX IF NOT EXISTS idx_peers_kind ON peers(kind);

            CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY,
                peer_id TEXT NOT NULL,
                label TEXT,
                started_at TEXT NOT NULL DEFAULT (datetime('now')),
                ended_at TEXT,
                source TEXT NOT NULL,
                -- Stable external identifier so a restart of the daemon
                -- can re-discover the same logical session for the same
                -- JSONL file. Without it, every restart starts a new
                -- session per JSONL and leaves the old one open
                -- forever — breaks the one-JSONL = one-session
                -- invariant the moment the daemon cycles. Nullable
                -- because manually-opened sessions (CLI) don't have one.
                external_key TEXT,
                -- Wall-clock timestamp of the most recent activity on
                -- this session. Persisted so idle-expiry checks survive
                -- daemon restarts AND so `ended_at` can be backdated to
                -- the real end-of-activity instead of when the next
                -- event happened to fire and trigger expiry. Default to
                -- started_at on legacy rows via the migration block.
                last_activity_at TEXT,
                FOREIGN KEY (peer_id) REFERENCES peers(id) ON DELETE CASCADE
            );
            CREATE INDEX IF NOT EXISTS idx_sessions_peer ON sessions(peer_id);
            CREATE INDEX IF NOT EXISTS idx_sessions_open
                ON sessions(peer_id) WHERE ended_at IS NULL;
            -- NOTE: idx_sessions_open_key (partial UNIQUE on external_key)
            -- is intentionally NOT created here. On legacy DBs the
            -- sessions table already exists without the external_key
            -- column, so CREATE INDEX would crash with a missing-column
            -- error. The migration block at the end of init_schema adds
            -- the column AND creates the index — both fresh and legacy
            -- DBs converge there.

            CREATE TABLE IF NOT EXISTS memory_peers (
                memory_id TEXT NOT NULL,
                peer_id TEXT NOT NULL,
                role TEXT NOT NULL DEFAULT 'speaker',
                PRIMARY KEY (memory_id, peer_id, role),
                FOREIGN KEY (memory_id) REFERENCES memories(id) ON DELETE CASCADE,
                FOREIGN KEY (peer_id) REFERENCES peers(id) ON DELETE CASCADE
            );
            CREATE INDEX IF NOT EXISTS idx_memory_peers_peer
                ON memory_peers(peer_id);",
        )?;

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

            CREATE TABLE IF NOT EXISTS entity_aliases (
                alias TEXT NOT NULL,
                canonical TEXT NOT NULL,
                merged_at TEXT NOT NULL DEFAULT (datetime('now')),
                PRIMARY KEY (alias, canonical)
            );

            CREATE INDEX IF NOT EXISTS idx_aliases_canonical ON entity_aliases(canonical);

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

        // Backfill FK constraints on `memory_peers` for databases created
        // before the FK clauses landed in the schema. `CREATE TABLE IF NOT
        // EXISTS` is a no-op when the table already exists, so the FK
        // declarations above don't retrofit the live table — we have to
        // do the standard SQLite rebuild dance: new table, copy valid
        // rows, drop, rename.
        Self::migrate_memory_peers_foreign_keys(&conn)?;

        // Role rename: addressee → participant for agent peers in
        // conversation memories. The auto-tag code emitted `addressee`
        // initially; that role name assumed user-message JSONL only.
        // Current code emits `participant` (neutral wrt turn direction).
        // Migrate any leftover rows so role-based queries don't see two
        // semantics for the same logical link.
        Self::migrate_addressee_to_participant(&conn)?;

        // Two-phase migration for `memories.session_id`:
        //
        // Phase 1: ensure the column exists. `CREATE TABLE IF NOT EXISTS`
        // above is a no-op when the table exists, so legacy DBs need
        // ALTER TABLE ADD COLUMN. We can't add the FK here because
        // SQLite rejects `ADD COLUMN ... REFERENCES` when foreign_keys
        // is ON. This call is also a no-op on fresh DBs (column already
        // present from CREATE TABLE).
        Self::migrate_add_column(&conn, "memories", "session_id", "TEXT");
        // The partial index doesn't depend on FK presence; create it
        // here so legacy DBs (which skipped CREATE TABLE) get it too.
        // On fresh DBs the index creation is idempotent.
        conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_memories_session
                ON memories(session_id) WHERE session_id IS NOT NULL;",
        )?;

        // Phase 2: retrofit the FK constraint on legacy DBs. Detection
        // checks `PRAGMA foreign_key_list(memories)` for a row pointing
        // at `sessions`; if present, fresh DBs (CREATE TABLE path) and
        // already-migrated legacy DBs both return early. Without the FK
        // a DELETE on sessions leaves stale `memories.session_id` values
        // that look valid but reference nothing — Codex caught this with
        // a live smoke-test repro. App-level enforcement in
        // `set_memory_session` catches forward writes but can't fix raw
        // DELETEs; only the DB-level cascade can. So we do the standard
        // SQLite rebuild dance: foreign_keys OFF, drop FTS triggers +
        // virtual table, recreate `memories` with the FK, copy rows
        // (nulling out any references to sessions that no longer exist),
        // recreate FTS + triggers + indexes, foreign_keys ON.
        Self::migrate_memories_session_fk(&conn)?;

        // Persistent session reuse columns on `sessions`. Codex caught
        // that the SessionTracker's in-RAM cache loses identity on
        // daemon restart — same JSONL re-opens a new session, old stays
        // open forever. Fix: store the JSONL path (or any caller-chosen
        // key) as `external_key` and the wall-clock activity timestamp
        // as `last_activity_at` so reuse + idle-expiry survive restarts.
        Self::migrate_add_column(&conn, "sessions", "external_key", "TEXT");
        Self::migrate_add_column(&conn, "sessions", "last_activity_at", "TEXT");
        // Backfill: legacy rows had no last_activity_at — start them at
        // their own started_at so idle-expiry math has a real value.
        // Idempotent: WHERE clause skips already-populated rows.
        conn.execute(
            "UPDATE sessions SET last_activity_at = started_at WHERE last_activity_at IS NULL",
            [],
        )?;
        // Indexes (declared above in CREATE TABLE for fresh DBs; idempotent here for legacy).
        conn.execute_batch(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_sessions_open_key
                ON sessions(external_key) WHERE ended_at IS NULL AND external_key IS NOT NULL;",
        )?;

        // Atomic-canonical guard: at most ONE canonical
        // (open_at_summary_time=false-or-NULL) session_summary per
        // session. Codex flagged the race: dream_worker's
        // lookup-then-save and the CLI's `dream batch --apply` could
        // both see "no canonical yet" in a 1-second window and both
        // insert, producing duplicates. SQLite supports expression
        // indexes via json_extract; the partial WHERE narrows the
        // uniqueness to the rows that actually represent canonical
        // summaries (snapshots are allowed to coexist as before).
        //
        // The index is created here in init_schema (not in CREATE
        // TABLE memories) because the `memories` table is built in
        // a single execute_batch that runs before the metadata
        // semantics are settled — keeping the index migration here
        // mirrors the idx_memories_session pattern and is safe to
        // re-run on every open.
        //
        // First-time creation will fail if the live DB already has
        // duplicate canonical summaries for some session. The
        // helper below dedups defensively before creating the
        // index, keeping the most-recently-touched row.
        Self::dedupe_canonical_session_summaries(&conn)?;
        conn.execute_batch(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_session_summary_canonical
                ON memories(json_extract(metadata, '$.summary_of_session'))
              WHERE memory_type = 'session_summary'
                AND json_extract(metadata, '$.summary_of_session') IS NOT NULL
                AND COALESCE(json_extract(metadata, '$.open_at_summary_time'), 0) = 0;",
        )?;

        info!("Storage initialized at {:?}", conn.path());
        Ok(())
    }

    /// Rebuild `memories` with `session_id` carrying an actual FK on
    /// `sessions(id) ON DELETE SET NULL`. Skipped if the FK is already
    /// present (fresh DBs via CREATE TABLE; legacy DBs already migrated).
    ///
    /// SQLite has no `ALTER TABLE ADD CONSTRAINT`, so the only path to
    /// retrofit a FK is rebuild-and-swap. The wrinkle here vs the
    /// memory_peers migration: `memories` has an FTS5 virtual table
    /// (`memories_fts`) with `content='memories'` that tracks rowids,
    /// plus three triggers (`memories_ai/ad/au`) that propagate writes.
    /// Rebuilding `memories` changes the table's rowid backing, so we
    /// drop FTS + triggers, rebuild `memories`, then recreate FTS and
    /// repopulate it from the new table.
    ///
    /// `foreign_keys` PRAGMA cannot change inside a transaction, so it's
    /// flipped OFF before the transaction begins and ON in the cleanup
    /// arm regardless of outcome.
    ///
    /// Stale `session_id` values (pointing at sessions that no longer
    /// exist) are nulled out during the row copy. Without that the new
    /// FK constraint would still pass (FK is checked on writes, not
    /// reads) but `PRAGMA foreign_key_check` would flag them — better
    /// to clean them up while we're rebuilding anyway.
    fn migrate_memories_session_fk(conn: &Connection) -> Result<()> {
        // Detection: is there already a FK on memories → sessions?
        let has_fk = {
            let mut stmt = conn.prepare("PRAGMA foreign_key_list(memories)")?;
            stmt.query_map([], |row| row.get::<_, String>(2))?
                .filter_map(|r| r.ok())
                .any(|table| table == "sessions")
        };
        if has_fk {
            return Ok(());
        }

        conn.pragma_update(None, "foreign_keys", "OFF")?;

        let result: Result<()> = (|| {
            let tx = conn.unchecked_transaction()?;

            // Drop FTS shim — triggers first (they reference the FTS
            // table), then the virtual table itself. FTS data is
            // rebuildable from `memories` so dropping it is safe.
            tx.execute_batch(
                "DROP TRIGGER IF EXISTS memories_ai;
                 DROP TRIGGER IF EXISTS memories_ad;
                 DROP TRIGGER IF EXISTS memories_au;
                 DROP TABLE IF EXISTS memories_fts;",
            )?;

            // Rebuild memories with the FK and full current column set
            // (the schema accreted via migrate_add_column over time;
            // listing every column explicitly so the rebuild stays in
            // sync with the live shape).
            tx.execute_batch(
                "CREATE TABLE memories_new (
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
                    created_at TEXT NOT NULL DEFAULT (datetime('now')),
                    access_count INTEGER NOT NULL DEFAULT 0,
                    last_accessed_at TEXT,
                    superseded_by TEXT,
                    canonical_memory_id TEXT,
                    session_id TEXT REFERENCES sessions(id) ON DELETE SET NULL
                );",
            )?;

            // Copy rows. Build the SELECT dynamically: legacy DBs may
            // be missing optional columns that accreted via migrate_add_column
            // over time (and `created_at` which never got a migration
            // because its CREATE TABLE default uses `datetime('now')` —
            // SQLite forbids that in ALTER TABLE ADD COLUMN). For any
            // missing column we project a sensible default so the
            // INSERT INTO ... SELECT always has the right shape.
            //
            // The CASE expression on session_id nulls out stale references
            // — cleans up dangling ids Codex's live-smoke surfaced.
            let live_cols: std::collections::HashSet<String> = {
                let mut stmt = tx.prepare("PRAGMA table_info(memories)")?;
                stmt.query_map([], |row| row.get::<_, String>(1))?
                    .filter_map(|r| r.ok())
                    .collect()
            };
            // (column_name, default_expr_when_missing). Defaults mirror
            // the CREATE TABLE shape above so legacy rows land in a
            // valid state for the NOT NULL / DEFAULT constraints on the
            // new table.
            let col_specs: &[(&str, &str)] = &[
                ("id", "id"),
                ("timestamp", "timestamp"),
                ("title", "title"),
                ("content", "content"),
                ("memory_type", "memory_type"),
                ("tags", "'[]'"),
                ("source", "source"),
                ("importance", "0.5"),
                ("metadata", "'{}'"),
                ("embedding", "NULL"),
                ("created_at", "datetime('now')"),
                ("access_count", "0"),
                ("last_accessed_at", "NULL"),
                ("superseded_by", "NULL"),
                ("canonical_memory_id", "NULL"),
                // session_id needs the stale-id null-out wrapped around it.
                (
                    "session_id",
                    "CASE WHEN session_id IS NULL \
                            OR EXISTS(SELECT 1 FROM sessions s WHERE s.id = memories.session_id) \
                          THEN session_id ELSE NULL END",
                ),
            ];
            let select_exprs: Vec<String> = col_specs
                .iter()
                .map(|(name, default_expr)| {
                    if live_cols.contains(*name) {
                        // Column present: use its current value (the
                        // session_id slot still gets the CASE wrapper
                        // because *default_expr references session_id
                        // directly).
                        if *name == "session_id" {
                            (*default_expr).to_string()
                        } else {
                            (*name).to_string()
                        }
                    } else {
                        // Column missing on this legacy DB: project the
                        // default. session_id would always be NULL here
                        // because the CASE references a non-existent
                        // column — collapse to plain NULL.
                        if *name == "session_id" {
                            "NULL".to_string()
                        } else {
                            (*default_expr).to_string()
                        }
                    }
                })
                .collect();
            let insert_sql = format!(
                "INSERT INTO memories_new
                    (id, timestamp, title, content, memory_type, tags, source,
                     importance, metadata, embedding, created_at, access_count,
                     last_accessed_at, superseded_by, canonical_memory_id, session_id)
                 SELECT {} FROM memories",
                select_exprs.join(", ")
            );
            let copied = tx.execute(&insert_sql, [])?;

            tx.execute("DROP TABLE memories", [])?;
            tx.execute("ALTER TABLE memories_new RENAME TO memories", [])?;

            // Indexes (the ones we lost when we dropped memories).
            tx.execute_batch(
                "CREATE INDEX IF NOT EXISTS idx_memories_type ON memories(memory_type);
                 CREATE INDEX IF NOT EXISTS idx_memories_timestamp ON memories(timestamp);
                 CREATE INDEX IF NOT EXISTS idx_memories_importance ON memories(importance);
                 CREATE INDEX IF NOT EXISTS idx_memories_session
                    ON memories(session_id) WHERE session_id IS NOT NULL;",
            )?;

            // FTS rebuild: recreate the virtual table, then populate
            // from the new memories rowids. Triggers go last so they
            // don't fire during the bulk INSERT (would double-up FTS
            // entries with the explicit INSERT above).
            tx.execute_batch(
                "CREATE VIRTUAL TABLE memories_fts USING fts5(
                    title, content, tags,
                    content='memories',
                    content_rowid='rowid'
                 );
                 INSERT INTO memories_fts(rowid, title, content, tags)
                    SELECT rowid, title, content, tags FROM memories;
                 CREATE TRIGGER memories_ai AFTER INSERT ON memories BEGIN
                    INSERT INTO memories_fts(rowid, title, content, tags)
                    VALUES (new.rowid, new.title, new.content, new.tags);
                 END;
                 CREATE TRIGGER memories_ad AFTER DELETE ON memories BEGIN
                    INSERT INTO memories_fts(memories_fts, rowid, title, content, tags)
                    VALUES ('delete', old.rowid, old.title, old.content, old.tags);
                 END;
                 CREATE TRIGGER memories_au AFTER UPDATE OF title, content, tags ON memories BEGIN
                    INSERT INTO memories_fts(memories_fts, rowid, title, content, tags)
                    VALUES ('delete', old.rowid, old.title, old.content, old.tags);
                    INSERT INTO memories_fts(rowid, title, content, tags)
                    VALUES (new.rowid, new.title, new.content, new.tags);
                 END;",
            )?;

            tx.commit()?;
            info!(
                "Migrated memories: FK on session_id retrofitted ({copied} rows kept; FTS rebuilt)"
            );
            Ok(())
        })();

        // Restore foreign_keys regardless of outcome — if the rebuild
        // failed, the rest of the connection's queries still need FK
        // enforcement back on.
        conn.pragma_update(None, "foreign_keys", "ON")?;
        result
    }

    /// Rewrite leftover `addressee` rows to `participant` ONLY for
    /// rows pointing at agent peers. The old auto-tag wrote
    /// `addressee` for the agent on conversation memories — that's the
    /// only artifact this migration is supposed to clean up.
    ///
    /// `addressee` for a human peer (or any non-agent kind) is a
    /// legitimate hand-applied role — a user might link "claude said
    /// X to User" with User as addressee, and we must NOT silently
    /// rewrite that. Codex called this out: scope the rewrite by
    /// `peers.kind = 'agent'`.
    ///
    /// Pre-index dedup: if the live DB has more than one CANONICAL
    /// session_summary for the same session (race between worker
    /// and CLI batch in the pre-fix window), the upcoming
    /// `idx_session_summary_canonical` UNIQUE index would fail to
    /// create. Keep the most-recently-touched row per session and
    /// delete the rest before the index lands.
    ///
    /// "Canonical" = `open_at_summary_time IS NULL OR = 0`.
    /// Snapshots (= 1) are exempt — multiple snapshots per session
    /// are allowed by design.
    ///
    /// Idempotent: no duplicates → no-op. Logs at info when it
    /// removes rows so unexpected dedup activity is visible in the
    /// daemon log.
    fn dedupe_canonical_session_summaries(conn: &Connection) -> Result<()> {
        // Detect: any session with >1 canonical summary?
        let dupes: Vec<(String, i64)> = {
            let mut stmt = conn.prepare(
                "SELECT json_extract(metadata, '$.summary_of_session') AS sid,
                        COUNT(*) AS n
                   FROM memories
                  WHERE memory_type = 'session_summary'
                    AND json_extract(metadata, '$.summary_of_session') IS NOT NULL
                    AND COALESCE(json_extract(metadata, '$.open_at_summary_time'), 0) = 0
                  GROUP BY sid
                 HAVING n > 1",
            )?;
            stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        if dupes.is_empty() {
            return Ok(());
        }

        let mut total_removed = 0usize;
        let tx = conn.unchecked_transaction()?;
        for (session_id, n) in &dupes {
            // Keep the most-recently-touched row (use timestamp as
            // proxy for "freshest"). Delete the rest. Without the
            // `OFFSET 1` semantics, `LIMIT N-1` against a subquery
            // — easier to write the dual: pick the WINNER, delete
            // everyone-else.
            let winner: String = tx.query_row(
                "SELECT id FROM memories
                  WHERE memory_type = 'session_summary'
                    AND json_extract(metadata, '$.summary_of_session') = ?1
                    AND COALESCE(json_extract(metadata, '$.open_at_summary_time'), 0) = 0
                  ORDER BY timestamp DESC, created_at DESC
                  LIMIT 1",
                params![session_id],
                |r| r.get::<_, String>(0),
            )?;
            let removed = tx.execute(
                "DELETE FROM memories
                  WHERE memory_type = 'session_summary'
                    AND json_extract(metadata, '$.summary_of_session') = ?1
                    AND COALESCE(json_extract(metadata, '$.open_at_summary_time'), 0) = 0
                    AND id != ?2",
                params![session_id, winner],
            )?;
            total_removed += removed;
            info!(
                "dedupe_canonical_session_summaries: session {} had {} duplicates → kept {}, removed {}",
                &session_id[..8.min(session_id.len())],
                n,
                &winner[..8.min(winner.len())],
                removed
            );
        }
        tx.commit()?;
        info!(
            "dedupe_canonical_session_summaries: removed {total_removed} duplicate rows across {} sessions",
            dupes.len()
        );
        Ok(())
    }

    /// Idempotent — early-returns when no qualifying rows exist.
    /// Handles PK collisions (`participant` row already present for
    /// the same memory+peer): keeps participant, drops addressee.
    fn migrate_addressee_to_participant(conn: &Connection) -> Result<()> {
        // Detect: any addressee rows pointing at AGENT peers?
        let stale_count: i64 = conn
            .query_row(
                "SELECT COUNT(*)
                   FROM memory_peers mp
                   JOIN peers p ON p.id = mp.peer_id
                  WHERE mp.role = 'addressee' AND p.kind = 'agent'",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);
        if stale_count == 0 {
            return Ok(());
        }

        let tx = conn.unchecked_transaction()?;
        // INSERT OR IGNORE: if (memory_id, peer_id, 'participant') already
        // exists (we ran the new code on the same memory), the collision
        // is silent — old addressee row will be dropped next.
        tx.execute(
            "INSERT OR IGNORE INTO memory_peers (memory_id, peer_id, role)
             SELECT mp.memory_id, mp.peer_id, 'participant'
               FROM memory_peers mp
               JOIN peers p ON p.id = mp.peer_id
              WHERE mp.role = 'addressee' AND p.kind = 'agent'",
            [],
        )?;
        let removed = tx.execute(
            "DELETE FROM memory_peers
              WHERE role = 'addressee'
                AND peer_id IN (SELECT id FROM peers WHERE kind = 'agent')",
            [],
        )?;
        tx.commit()?;
        info!(
            "Migrated memory_peers roles: addressee → participant for agent peers \
             ({removed} rows; collisions silently merged)"
        );
        Ok(())
    }

    /// Idempotent FK retrofit for `memory_peers`. Checks
    /// `PRAGMA foreign_key_list(memory_peers)`; if it already lists at
    /// least one FK, returns early. Otherwise rebuilds the table inside
    /// a transaction with `foreign_keys = OFF` (the rebuild references
    /// peers/memories that exist; orphan rows are dropped explicitly via
    /// the COPY's WHERE clause).
    ///
    /// Why an explicit migration and not just CREATE TABLE? Because
    /// `CREATE TABLE IF NOT EXISTS` is a no-op when the table exists,
    /// and SQLite has no `ALTER TABLE ... ADD CONSTRAINT`. The only path
    /// to retrofit FKs is rebuild-and-swap.
    fn migrate_memory_peers_foreign_keys(conn: &Connection) -> Result<()> {
        // Detect: does memory_peers already have BOTH expected FKs?
        // Codex flagged that `fk_count > 0` would skip rebuild on a
        // partially-broken schema (one FK present, one missing). Check
        // explicitly that we have exactly the two FKs we expect
        // (memory_id → memories, peer_id → peers).
        let referenced_tables: std::collections::HashSet<String> = {
            let mut stmt = conn.prepare(
                // PRAGMA foreign_key_list columns: id, seq, table, from, to, on_update, on_delete, match
                "PRAGMA foreign_key_list(memory_peers)",
            )?;
            stmt.query_map([], |row| row.get::<_, String>(2))?
                .filter_map(|r| r.ok())
                .collect()
        };
        let has_memories_fk = referenced_tables.contains("memories");
        let has_peers_fk = referenced_tables.contains("peers");
        if has_memories_fk && has_peers_fk {
            // Schema already up to date — both expected FKs are present.
            return Ok(());
        }

        // PRAGMA foreign_keys can't change inside a transaction; flip it
        // off for the rebuild and back on afterwards. Without this the
        // ALTER chain below would fail trying to enforce FKs against the
        // intermediate state.
        conn.pragma_update(None, "foreign_keys", "OFF")?;

        let result: Result<()> = (|| {
            let tx = conn.unchecked_transaction()?;
            tx.execute_batch(
                "CREATE TABLE memory_peers_new (
                    memory_id TEXT NOT NULL,
                    peer_id TEXT NOT NULL,
                    role TEXT NOT NULL DEFAULT 'speaker',
                    PRIMARY KEY (memory_id, peer_id, role),
                    FOREIGN KEY (memory_id) REFERENCES memories(id) ON DELETE CASCADE,
                    FOREIGN KEY (peer_id) REFERENCES peers(id) ON DELETE CASCADE
                );",
            )?;
            // Copy only the rows that reference live memories AND live peers.
            // Orphan rows that would violate the new FKs are dropped here.
            let copied = tx.execute(
                "INSERT INTO memory_peers_new (memory_id, peer_id, role)
                 SELECT mp.memory_id, mp.peer_id, mp.role
                   FROM memory_peers mp
                  WHERE EXISTS(SELECT 1 FROM memories m WHERE m.id = mp.memory_id)
                    AND EXISTS(SELECT 1 FROM peers p   WHERE p.id = mp.peer_id)",
                [],
            )?;
            tx.execute("DROP TABLE memory_peers", [])?;
            tx.execute("ALTER TABLE memory_peers_new RENAME TO memory_peers", [])?;
            tx.execute(
                "CREATE INDEX IF NOT EXISTS idx_memory_peers_peer
                    ON memory_peers(peer_id)",
                [],
            )?;
            tx.commit()?;
            info!("Migrated memory_peers: FK constraints retrofitted ({copied} rows kept)");
            Ok(())
        })();

        // Restore the pragma regardless of outcome. If the rebuild failed,
        // the FK enforcement state has to come back on for the next
        // connection's queries to behave correctly.
        conn.pragma_update(None, "foreign_keys", "ON")?;
        result
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

        // Canonical session_summary rows need plain INSERT so that
        // `idx_session_summary_canonical` (UNIQUE partial index) can
        // actually reject a concurrent second canonical for the same
        // session. With `INSERT OR REPLACE` SQLite would silently
        // delete the existing canonical and slot the new id in its
        // place — no constraint error, the race winner just steamrolls
        // the loser. All other writes keep OR REPLACE because they're
        // either id-keyed updates or have their own dedup paths
        // (memory_peers, edges, llm_extraction_cache, …).
        let is_canonical_session_summary =
            matches!(entry.memory_type, crate::event::MemoryType::SessionSummary)
                && entry
                    .metadata
                    .get("summary_of_session")
                    .and_then(|v| v.as_str())
                    .is_some()
                && !entry
                    .metadata
                    .get("open_at_summary_time")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);

        let sql = if is_canonical_session_summary {
            "INSERT INTO memories (id, timestamp, title, content, memory_type, tags, source, importance, metadata, embedding)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)"
        } else {
            "INSERT OR REPLACE INTO memories (id, timestamp, title, content, memory_type, tags, source, importance, metadata, embedding)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)"
        };
        conn.execute(
            sql,
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
        drop(conn);

        // Add to HNSW index
        if let Some(emb) = embedding
            && let Ok(mut hnsw) = self.hnsw.lock()
        {
            hnsw.insert(&entry.id, emb);
        }
        Ok(())
    }

    /// Update ONLY the embedding column for an existing memory — does not
    /// touch HNSW. Used for bulk re-embedding when the embedder dimension
    /// changes; caller is responsible for triggering a fresh HNSW rebuild
    /// (e.g. by restarting the daemon / re-opening Storage).
    pub fn update_embedding(&self, id: &str, embedding: &Embedding) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let blob = embedding_to_bytes(embedding);
        conn.execute(
            "UPDATE memories SET embedding = ?1 WHERE id = ?2",
            params![blob, id],
        )?;
        Ok(())
    }

    /// Check if a similar memory already exists (cosine > threshold).
    /// Returns Some(similarity) if duplicate found, None if unique.
    /// Uses HNSW for fast check, falls back to brute-force.
    /// Reject a vector query whose dimension differs from the dimension the
    /// in-memory index locked to — i.e. the embedding model was swapped
    /// without a `mnemonic reembed`. Without this, HNSW returns nothing and
    /// the brute-force fallback scores every old row 0, silently breaking
    /// search and (via `is_duplicate`) dedup. An empty index has no stored
    /// vectors yet, so there is nothing to mismatch.
    fn guard_query_dim(&self, embedding: &Embedding) -> Result<()> {
        let stored = self
            .hnsw
            .lock()
            .map_err(|e| anyhow::anyhow!("lock: {e}"))?
            .dim();
        if let Some(d) = stored
            && d != embedding.len()
        {
            anyhow::bail!(
                "Embedding dimension mismatch (query={}, stored={d}): the embedding model \
                 changed without a reembed. Run `mnemonic reembed`.",
                embedding.len()
            );
        }
        Ok(())
    }

    pub fn is_duplicate(&self, embedding: &Embedding, threshold: f32) -> Result<Option<f32>> {
        self.guard_query_dim(embedding)?;
        // Try HNSW first — but verify candidates against SQLite before
        // trusting them. The index is in-memory and per-process: a forget
        // or supersede issued by ANOTHER process (CLI `mnemonic forget`,
        // MCP server) leaves a ghost vector here, and an unverified ghost
        // match would make the dedup gate silently drop a brand-new save.
        let candidates = match self.hnsw.lock() {
            Ok(hnsw) if !hnsw.is_empty() => Some(hnsw.search(embedding, 3)),
            _ => None,
        };
        if let Some(candidates) = candidates {
            let mut stale: Vec<String> = Vec::new();
            let mut verdict: Option<f32> = None;
            {
                let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
                for (id, similarity) in &candidates {
                    if *similarity < threshold {
                        break; // sorted by similarity desc
                    }
                    let live = conn
                        .query_row(
                            "SELECT 1 FROM memories WHERE id = ?1 AND superseded_by IS NULL",
                            params![id],
                            |_| Ok(()),
                        )
                        .is_ok();
                    if live {
                        debug!(
                            "HNSW duplicate found: cosine={similarity:.4} >= threshold={threshold:.4}"
                        );
                        verdict = Some(*similarity);
                        break;
                    }
                    stale.push(id.clone());
                }
            }
            // Self-heal: evict ghosts so they stop occupying result slots.
            if !stale.is_empty()
                && let Ok(mut hnsw) = self.hnsw.lock()
            {
                for id in &stale {
                    hnsw.remove(id);
                }
            }
            return Ok(verdict);
        }

        // Fallback: brute-force scan
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let mut stmt = conn.prepare(
            "SELECT embedding FROM memories
             WHERE embedding IS NOT NULL AND superseded_by IS NULL
             ORDER BY timestamp DESC LIMIT 200",
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

    /// Find memories most similar to a given embedding.
    /// Uses HNSW index for O(log n) approximate nearest neighbor search.
    /// Falls back to brute-force scan if HNSW index is empty.
    ///
    /// Side effect: bumps `access_count` / `last_accessed_at` on every hit
    /// so usage feeds back into `decay::effective_score` rankings.
    pub fn find_similar(
        &self,
        embedding: &Embedding,
        limit: usize,
    ) -> Result<Vec<(MemoryEntry, f32)>> {
        let results = self.find_similar_inner(embedding, limit)?;
        let ids: Vec<&str> = results.iter().map(|(e, _)| e.id.as_str()).collect();
        if let Err(e) = self.touch_access(&ids) {
            debug!("touch_access failed (find_similar): {e}");
        }
        Ok(results)
    }

    /// Pure read vector / HNSW similarity — no `access_count` /
    /// `last_accessed_at` bump. Mirror of `search_no_touch`. See it for
    /// rationale.
    pub fn find_similar_no_touch(
        &self,
        embedding: &Embedding,
        limit: usize,
    ) -> Result<Vec<(MemoryEntry, f32)>> {
        self.find_similar_inner(embedding, limit)
    }

    fn find_similar_inner(
        &self,
        embedding: &Embedding,
        limit: usize,
    ) -> Result<Vec<(MemoryEntry, f32)>> {
        self.guard_query_dim(embedding)?;
        // Try HNSW first. Overfetch: the SQL hydration below re-checks
        // liveness (superseded / cross-process-forgotten rows drop out),
        // so without spare candidates the result set shrinks below `limit`
        // — "ask 20, get 12".
        let hnsw_results = self
            .hnsw
            .lock()
            .map_err(|e| anyhow::anyhow!("lock: {e}"))?
            .search(embedding, limit * 2 + 8);

        if !hnsw_results.is_empty() {
            debug!("HNSW search returned {} results", hnsw_results.len());
            let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
            let mut results = Vec::new();
            let mut stale: Vec<&str> = Vec::new();

            for (memory_id, similarity) in &hnsw_results {
                if results.len() >= limit {
                    break;
                }
                let row = conn.query_row(
                    "SELECT id, timestamp, title, content, memory_type, tags, source, importance, metadata
                     FROM memories WHERE id = ?1 AND superseded_by IS NULL",
                    params![memory_id],
                    |row| {
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
                    },
                );

                if let Ok(row) = row
                    && let Ok(entry) = row.into_memory_entry()
                {
                    results.push((entry, *similarity));
                } else {
                    // Row vanished or got superseded behind the index's
                    // back (cross-process forget) — self-heal the ghost.
                    stale.push(memory_id);
                }
            }
            drop(conn);
            if !stale.is_empty()
                && let Ok(mut hnsw) = self.hnsw.lock()
            {
                for id in stale {
                    hnsw.remove(id);
                }
            }

            return Ok(results);
        }

        // Fallback: brute-force scan (for empty index or edge cases)
        debug!("HNSW empty, falling back to brute-force scan");
        self.find_similar_bruteforce(embedding, limit)
    }

    /// Brute-force similarity scan — fallback when HNSW is empty
    fn find_similar_bruteforce(
        &self,
        embedding: &Embedding,
        limit: usize,
    ) -> Result<Vec<(MemoryEntry, f32)>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let mut stmt = conn.prepare(
            "SELECT id, timestamp, title, content, memory_type, tags, source, importance, metadata, embedding
             FROM memories WHERE embedding IS NOT NULL AND superseded_by IS NULL",
        )?;

        let mut scored: Vec<(StorageRow, f32)> = stmt
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
                (row, sim)
            })
            .collect();

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(limit);

        let results: Vec<(MemoryEntry, f32)> = scored
            .into_iter()
            .filter_map(|(row, sim)| row.into_memory_entry().ok().map(|e| (e, sim)))
            .collect();

        Ok(results)
    }

    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<MemoryEntry>> {
        let entries = self.search_no_touch(query, limit)?;
        // Touch access for usage-aware ranking. Errors here are non-fatal — we
        // still return the search results even if the bookkeeping write fails.
        let ids: Vec<&str> = entries.iter().map(|e| e.id.as_str()).collect();
        if let Err(e) = self.touch_access(&ids) {
            debug!("touch_access failed (search): {e}");
        }
        Ok(entries)
    }

    /// Pure read FTS5 search — no `access_count` / `last_accessed_at` bump.
    /// Use for eval, debugging tools, anything that mustn't perturb the
    /// decay scoring just by looking. Production retrieval should call
    /// `search`.
    pub fn search_no_touch(&self, query: &str, limit: usize) -> Result<Vec<MemoryEntry>> {
        // Quote every token so FTS5's mini-syntax (", *, :, NEAR, AND, ...) in a
        // raw user query can't make MATCH raise a parse error. The HTTP path
        // already sanitizes via hybrid_search; this covers the socket/MCP/CLI
        // callers that hit search() directly. Empty after sanitize → no rows.
        let query = crate::retrieval::sanitize_fts_query(query);
        if query.is_empty() {
            return Ok(Vec::new());
        }
        let query = query.as_str();
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let mut stmt = conn.prepare(
            "SELECT m.id, m.timestamp, m.title, m.content, m.memory_type, m.tags, m.source, m.importance, m.metadata
             FROM memories_fts fts
             JOIN memories m ON m.rowid = fts.rowid
             WHERE memories_fts MATCH ?1
               AND m.superseded_by IS NULL
             ORDER BY rank
             LIMIT ?2",
        )?;
        let entries: Vec<MemoryEntry> = stmt
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

    /// Fetch a single memory by id. Returns None if no row matches.
    pub fn get_by_id(&self, id: &str) -> Result<Option<MemoryEntry>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let row = conn
            .query_row(
                "SELECT id, timestamp, title, content, memory_type, tags, source, importance, metadata
                 FROM memories WHERE id = ?1",
                params![id],
                |row| {
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
                },
            )
            .ok();
        Ok(row.and_then(|r| r.into_memory_entry().ok()))
    }

    /// Distinct embedding dimensions (in f32s) among ACTIVE, non-superseded
    /// rows. Empty if nothing is embedded yet; more than one entry means a
    /// mixed / partly-migrated store. Used at startup to detect an embedding-
    /// model dimension change (a swap without `mnemonic reembed`) so the
    /// daemon can refuse to run instead of silently degrading search/dedup.
    /// Superseded rows are excluded so a retired old-dim vector can't trip it.
    pub fn active_embedding_dims(&self) -> Vec<usize> {
        let Ok(conn) = self.conn.lock() else {
            return Vec::new();
        };
        let Ok(mut stmt) = conn.prepare(
            "SELECT DISTINCT length(embedding) FROM memories \
             WHERE embedding IS NOT NULL AND superseded_by IS NULL",
        ) else {
            return Vec::new();
        };
        stmt.query_map([], |row| row.get::<_, i64>(0))
            .map(|rows| {
                rows.filter_map(|r| r.ok())
                    .map(|bytes| (bytes as usize) / 4)
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn recent(&self, limit: usize) -> Result<Vec<MemoryEntry>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let mut stmt = conn.prepare(
            "SELECT id, timestamp, title, content, memory_type, tags, source, importance, metadata
             FROM memories
             WHERE superseded_by IS NULL
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

    /// Active memories whose timestamp falls in `[start, end)`, oldest first.
    /// The journal's deterministic fact-collector reads a local day's window
    /// through this.
    pub fn memories_in_window(
        &self,
        start: chrono::DateTime<chrono::Utc>,
        end: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<MemoryEntry>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let mut stmt = conn.prepare(
            "SELECT id, timestamp, title, content, memory_type, tags, source, importance, metadata
             FROM memories
             WHERE superseded_by IS NULL
               AND timestamp >= ?1 AND timestamp < ?2
             ORDER BY timestamp ASC",
        )?;
        let entries = stmt
            .query_map(params![start.to_rfc3339(), end.to_rfc3339()], |row| {
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

    /// For each **significant** project (>= `min_mems` real memories, same
    /// noise floor as attribution), the active memories it links to in
    /// `[start, end)`. Drives the journal's per-project bullets. Keyed by
    /// project entity id; value is `(name, memories oldest-first)`.
    pub fn project_memories_in_window(
        &self,
        start: chrono::DateTime<chrono::Utc>,
        end: chrono::DateTime<chrono::Utc>,
        min_mems: i64,
    ) -> Result<Vec<(String, String, Vec<MemoryEntry>)>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let mut stmt = conn.prepare(
            "SELECT e.id, e.name, m.id, m.timestamp, m.title, m.content, m.memory_type,
                    m.tags, m.source, m.importance, m.metadata
             FROM memories m
             JOIN memory_entities me ON me.memory_id = m.id
             JOIN entities e ON e.id = me.entity_id AND e.entity_type = 'project'
             WHERE m.superseded_by IS NULL
               AND m.memory_type != 'session_summary'
               AND m.timestamp >= ?1 AND m.timestamp < ?2
               AND (
                 SELECT COUNT(*) FROM memory_entities me2
                 JOIN memories m2 ON m2.id = me2.memory_id
                 WHERE me2.entity_id = e.id
                   AND m2.superseded_by IS NULL
                   AND m2.memory_type != 'session_summary'
               ) >= ?3
             ORDER BY e.id, m.timestamp ASC",
        )?;
        let rows = stmt
            .query_map(
                params![start.to_rfc3339(), end.to_rfc3339(), min_mems],
                |row| {
                    let key: String = row.get(0)?;
                    let name: String = row.get(1)?;
                    let sr = StorageRow {
                        id: row.get(2)?,
                        timestamp: row.get(3)?,
                        title: row.get(4)?,
                        content: row.get(5)?,
                        memory_type: row.get(6)?,
                        tags: row.get(7)?,
                        source: row.get(8)?,
                        importance: row.get(9)?,
                        metadata: row.get(10)?,
                    };
                    Ok((key, name, sr))
                },
            )?
            .filter_map(|r| r.ok());

        // Group consecutive rows by project id (query is ORDER BY e.id).
        let mut out: Vec<(String, String, Vec<MemoryEntry>)> = Vec::new();
        for (key, name, sr) in rows {
            let Ok(entry) = sr.into_memory_entry() else {
                continue;
            };
            match out.last_mut() {
                Some((k, _, mems)) if *k == key => mems.push(entry),
                _ => out.push((key, name, vec![entry])),
            }
        }
        Ok(out)
    }

    /// Reference pool for k-NN attribution: every clean (non-session_summary,
    /// non-noise) project-linked memory with an embedding, keyed by its
    /// CANONICAL project name. Only canonical projects with at least `min_mems`
    /// such memories are kept, so a stray link can't form a one-memory project.
    pub fn project_reference_pool(
        &self,
        min_mems: i64,
    ) -> Result<Vec<crate::semantic_attribution::PoolItem>> {
        use std::collections::HashMap;
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let mut stmt = conn.prepare(
            "SELECT e.name, m.title, m.embedding
             FROM memories m
             JOIN memory_entities me ON me.memory_id = m.id
             JOIN entities e ON e.id = me.entity_id AND e.entity_type = 'project'
             WHERE m.superseded_by IS NULL
               AND m.memory_type != 'session_summary'
               AND m.embedding IS NOT NULL",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Vec<u8>>(2)?,
                ))
            })?
            .filter_map(|r| r.ok());

        let mut items: Vec<(String, crate::embedding::Embedding)> = Vec::new();
        let mut counts: HashMap<String, i64> = HashMap::new();
        for (name, title, blob) in rows {
            if crate::journal::is_noise_title(&title) {
                continue;
            }
            let canon = crate::semantic_attribution::canonical_project(&name);
            *counts.entry(canon.clone()).or_insert(0) += 1;
            items.push((canon, crate::embedding::embedding_from_bytes(&blob)));
        }
        Ok(items
            .into_iter()
            .filter(|(canon, _)| counts.get(canon).copied().unwrap_or(0) >= min_mems)
            .map(|(canon, embedding)| crate::semantic_attribution::PoolItem {
                project_name: canon.clone(),
                project_key: canon,
                embedding,
            })
            .collect())
    }

    /// Active memories in `[start, end)` that carry an embedding, each with its
    /// hard-linked project keys (all project entities). Feeds the semantic
    /// attribution dry-run / engine.
    pub fn window_memories_with_embeddings(
        &self,
        start: chrono::DateTime<chrono::Utc>,
        end: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<WindowMemory>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let mut stmt = conn.prepare(
            "SELECT m.id, m.title, m.content, m.timestamp, m.embedding,
                    GROUP_CONCAT(e.name)
             FROM memories m
             LEFT JOIN memory_entities me ON me.memory_id = m.id
             LEFT JOIN entities e ON e.id = me.entity_id AND e.entity_type = 'project'
             WHERE m.superseded_by IS NULL
               AND m.embedding IS NOT NULL
               AND m.timestamp >= ?1 AND m.timestamp < ?2
             GROUP BY m.id
             ORDER BY m.timestamp ASC",
        )?;
        let rows = stmt
            .query_map(params![start.to_rfc3339(), end.to_rfc3339()], |r| {
                let ts: String = r.get(3)?;
                let blob: Vec<u8> = r.get(4)?;
                let links: Option<String> = r.get(5)?;
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    ts,
                    blob,
                    links,
                ))
            })?
            .filter_map(|r| r.ok());

        let mut out = Vec::new();
        for (id, title, content, ts, blob, links) in rows {
            let timestamp = chrono::DateTime::parse_from_rfc3339(&ts)
                .map(|d| d.with_timezone(&chrono::Utc))
                .unwrap_or_else(|_| chrono::Utc::now());
            let linked_projects: Vec<String> = links
                .map(|s| {
                    s.split(',')
                        .filter(|x| !x.is_empty())
                        .map(String::from)
                        .collect()
                })
                .unwrap_or_default();
            out.push(WindowMemory {
                id,
                title,
                content,
                timestamp,
                linked_projects,
                embedding: crate::embedding::embedding_from_bytes(&blob),
            });
        }
        Ok(out)
    }

    /// Projects (project-type graph entities) ranked by mention count,
    /// each with its memory count and latest memories. Drives the widget's
    /// Projects page. Time is NOT attributed here (that's a separate
    /// feature) — callers mark `tracking = false` so the UI shows
    /// "tracking soon" for the hours.
    pub fn projects_overview(
        &self,
        max_projects: usize,
        mems_per: usize,
    ) -> Result<Vec<ProjectOverview>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let projs: Vec<(String, String)> = {
            let mut estmt = conn.prepare(
                "SELECT id, name FROM entities WHERE entity_type = 'project'
                 ORDER BY mention_count DESC, last_seen DESC LIMIT ?1",
            )?;
            estmt
                .query_map([max_projects as i64], |r| Ok((r.get(0)?, r.get(1)?)))?
                .filter_map(|r| r.ok())
                .collect()
        };

        let mut out = Vec::with_capacity(projs.len());
        for (eid, name) in projs {
            // Count the same set the list shows (exclude session_summary)
            // so the "N mem" badge can't exceed the visible memories.
            let mem_count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM memory_entities me
                     JOIN memories m ON m.id = me.memory_id
                     WHERE me.entity_id = ?1 AND m.superseded_by IS NULL
                       AND m.memory_type != 'session_summary'",
                    [&eid],
                    |r| r.get(0),
                )
                .unwrap_or(0);

            let mut mstmt = conn.prepare(
                "SELECT m.id, m.timestamp, m.title, m.content, m.memory_type, m.tags, m.source, m.importance, m.metadata
                 FROM memories m JOIN memory_entities me ON me.memory_id = m.id
                 WHERE me.entity_id = ?1 AND m.superseded_by IS NULL
                   AND m.memory_type != 'session_summary'
                 ORDER BY m.timestamp DESC LIMIT ?2",
            )?;
            let mems: Vec<MemoryEntry> = mstmt
                .query_map(params![eid, mems_per as i64], |row| {
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
                .filter_map(|r| r.into_memory_entry().ok())
                .collect();

            out.push(ProjectOverview {
                key: eid,
                name,
                mem_count,
                mems,
            });
        }
        Ok(out)
    }

    /// Build a single project's overview by entity id — used to union in
    /// attributed-but-unlisted projects so no tracked time is dropped from the
    /// payload. Returns None if the entity no longer exists.
    pub fn project_overview_by_id(
        &self,
        eid: &str,
        mems_per: usize,
    ) -> Result<Option<ProjectOverview>> {
        use rusqlite::OptionalExtension;
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let name: Option<String> = conn
            .query_row(
                "SELECT name FROM entities WHERE id = ?1 AND entity_type = 'project'",
                [eid],
                |r| r.get(0),
            )
            .optional()?;
        let Some(name) = name else { return Ok(None) };

        let mem_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM memory_entities me
                 JOIN memories m ON m.id = me.memory_id
                 WHERE me.entity_id = ?1 AND m.superseded_by IS NULL
                   AND m.memory_type != 'session_summary'",
                [eid],
                |r| r.get(0),
            )
            .unwrap_or(0);

        let mut mstmt = conn.prepare(
            "SELECT m.id, m.timestamp, m.title, m.content, m.memory_type, m.tags, m.source, m.importance, m.metadata
             FROM memories m JOIN memory_entities me ON me.memory_id = m.id
             WHERE me.entity_id = ?1 AND m.superseded_by IS NULL
               AND m.memory_type != 'session_summary'
             ORDER BY m.timestamp DESC LIMIT ?2",
        )?;
        let mems: Vec<MemoryEntry> = mstmt
            .query_map(params![eid, mems_per as i64], |row| {
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
            .filter_map(|r| r.into_memory_entry().ok())
            .collect();

        Ok(Some(ProjectOverview {
            key: eid.to_string(),
            name,
            mem_count,
            mems,
        }))
    }

    /// Canonical JSON array for the Projects page — shared by CLI + HTTP.
    /// `time` (from `ActivityStore::project_time`) merges attributed hours by
    /// project key; when absent or a project has no time, the row stays
    /// `tracking:false` so the UI shows "tracking soon". The top-level
    /// `unattributed` block is added by the caller from the same `time`.
    pub fn projects_value(
        &self,
        max_projects: usize,
        mems_per: usize,
        time: Option<&crate::activity::ProjectTimeData>,
    ) -> Result<serde_json::Value> {
        use std::collections::HashMap;
        let by_key: HashMap<&str, &crate::activity::ProjectTimeRow> = time
            .map(|t| t.rows.iter().map(|r| (r.project_key.as_str(), r)).collect())
            .unwrap_or_default();

        let mut ps = self.projects_overview(max_projects, mems_per)?;
        // Union in any project that has attributed time but ranks outside the
        // overview's top-N display cap — otherwise its hours would silently
        // vanish from the payload (shown ≠ accounted). The signal filter
        // already guarantees these keys are real projects, so they're safe to
        // surface. This makes the invariant exact: Σ(shown time) + unattributed
        // == total tracked time.
        {
            let shown: std::collections::HashSet<&str> =
                ps.iter().map(|p| p.key.as_str()).collect();
            let extra_keys: Vec<String> = by_key
                .iter()
                .filter(|(k, r)| {
                    (r.week_seconds > 0.0 || r.today_seconds > 0.0) && !shown.contains(*k)
                })
                .map(|(k, _)| k.to_string())
                .collect();
            for k in extra_keys {
                if let Some(p) = self.project_overview_by_id(&k, mems_per)? {
                    ps.push(p);
                }
            }
        }
        let arr: Vec<serde_json::Value> = ps
            .iter()
            .map(|p| {
                let t = by_key.get(p.key.as_str());
                let has_time = t
                    .map(|r| r.week_seconds > 0.0 || r.today_seconds > 0.0)
                    .unwrap_or(false);
                serde_json::json!({
                    "key": p.key,
                    "name": p.name,
                    "today_seconds": t.map(|r| r.today_seconds.round() as i64),
                    "week_seconds": t.map(|r| r.week_seconds.round() as i64),
                    "week": t.map(|r| r.week.iter().map(|s| s.round() as i64).collect::<Vec<_>>())
                        .unwrap_or_default(),
                    "mem_count": p.mem_count,
                    "tracking": has_time,
                    "confidence": t.and_then(|r| r.confidence.clone()),
                    "mems": p.mems.iter().map(|m| serde_json::json!({
                        "id": m.id,
                        "type": m.memory_type.to_string(),
                        "title": m.title,
                        "content": m.content,
                        "timestamp": m.timestamp.to_rfc3339(),
                    })).collect::<Vec<_>>(),
                })
            })
            .collect();
        Ok(serde_json::Value::Array(arr))
    }

    /// Project signals in a time window: project-type entities that were
    /// touched by memories whose timestamp falls in [start, end], weighted by
    /// memory count. This is the attribution signal — reuses the graph's
    /// memory↔project links rather than parsing file paths.
    ///
    /// `min_mems` filters out **noise entities**: the LLM extractor tags many
    /// feature/throwaway names as `entity_type='project'` (e.g. `light-card`,
    /// `project-entity`, `activity-tracker`, each linked to a single memory).
    /// Only projects with at least `min_mems` real (non-session_summary)
    /// memories generate signal — everything below that is below the noise
    /// floor and its time honestly falls into the Unattributed bucket instead
    /// of inventing a pseudo-project. This keeps the attribution universe the
    /// same as the one the Projects page displays.
    pub fn project_signals_in_window(
        &self,
        start: chrono::DateTime<chrono::Utc>,
        end: chrono::DateTime<chrono::Utc>,
        min_mems: i64,
    ) -> Result<Vec<crate::attribution::ProjectSignal>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let mut stmt = conn.prepare(
            "SELECT e.id, COUNT(*) AS w
             FROM memories m
             JOIN memory_entities me ON me.memory_id = m.id
             JOIN entities e ON e.id = me.entity_id AND e.entity_type = 'project'
             WHERE m.superseded_by IS NULL
               AND m.timestamp >= ?1 AND m.timestamp < ?2
               AND (
                 SELECT COUNT(*) FROM memory_entities me2
                 JOIN memories m2 ON m2.id = me2.memory_id
                 WHERE me2.entity_id = e.id
                   AND m2.superseded_by IS NULL
                   AND m2.memory_type != 'session_summary'
               ) >= ?3
             GROUP BY e.id",
        )?;
        let rows = stmt.query_map(
            rusqlite::params![start.to_rfc3339(), end.to_rfc3339(), min_mems],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)),
        )?;
        Ok(rows
            .filter_map(|r| r.ok())
            .map(|(key, w)| crate::attribution::ProjectSignal {
                project_key: key,
                weight: w as f64,
            })
            .collect())
    }

    /// Resolve an entity id to its display name (e.g. the project key UUID used
    /// in attribution back to "project-alpha"). None if the id is unknown.
    pub fn entity_name(&self, id: &str) -> Result<Option<String>> {
        use rusqlite::OptionalExtension;
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let name = conn
            .query_row("SELECT name FROM entities WHERE id = ?1", [id], |r| {
                r.get::<_, String>(0)
            })
            .optional()?;
        Ok(name)
    }

    /// `(project_key, timestamp)` for every project-linked memory in a window
    /// whose project clears the same `min_mems` floor as
    /// `project_signals_in_window`. Carry-forward uses these timestamps to test
    /// whether a no-signal session sits within its window of real project
    /// activity. session_summary rows are excluded — they're markers, not work.
    pub fn project_mem_times_in_window(
        &self,
        start: chrono::DateTime<chrono::Utc>,
        end: chrono::DateTime<chrono::Utc>,
        min_mems: i64,
    ) -> Result<Vec<(String, chrono::DateTime<chrono::Utc>)>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let mut stmt = conn.prepare(
            "SELECT e.id, m.timestamp
             FROM memories m
             JOIN memory_entities me ON me.memory_id = m.id
             JOIN entities e ON e.id = me.entity_id AND e.entity_type = 'project'
             WHERE m.superseded_by IS NULL
               AND m.memory_type != 'session_summary'
               -- Defensive meta guard (mirrors is_meta_memory): conversation /
               -- correction chatter shouldn't anchor a project's work window,
               -- even if a stray link survived reconcile.
               AND m.memory_type != 'feedback'
               AND COALESCE(m.tags, '') NOT LIKE '%conversation%'
               AND COALESCE(m.tags, '') NOT LIKE '%correction%'
               AND m.timestamp >= ?1 AND m.timestamp < ?2
               AND (
                 SELECT COUNT(*) FROM memory_entities me2
                 JOIN memories m2 ON m2.id = me2.memory_id
                 WHERE me2.entity_id = e.id
                   AND m2.superseded_by IS NULL
                   AND m2.memory_type != 'session_summary'
               ) >= ?3",
        )?;
        let rows = stmt.query_map(
            rusqlite::params![start.to_rfc3339(), end.to_rfc3339(), min_mems],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        )?;
        let mut out = Vec::new();
        for (key, ts) in rows.filter_map(|r| r.ok()) {
            if let Ok(t) = chrono::DateTime::parse_from_rfc3339(&ts) {
                out.push((key, t.with_timezone(&chrono::Utc)));
            }
        }
        Ok(out)
    }

    /// Full Projects payload `{ projects:[...], unattributed:{...} }` shared by
    /// CLI + HTTP. `time` carries the attributed hours (None → all "tracking
    /// soon").
    pub fn projects_payload(
        &self,
        max_projects: usize,
        mems_per: usize,
        time: Option<&crate::activity::ProjectTimeData>,
    ) -> Result<serde_json::Value> {
        let projects = self.projects_value(max_projects, mems_per, time)?;
        let unattributed = time.map(|t| {
            serde_json::json!({
                "today_seconds": t.unattributed_today.round() as i64,
                "week_seconds": t.unattributed_week.round() as i64,
            })
        });
        Ok(serde_json::json!({
            "projects": projects,
            "unattributed": unattributed,
        }))
    }

    /// Bump access_count and update last_accessed_at for the given IDs.
    /// Called on retrieval paths (search, find_similar, context) so that
    /// frequently-touched memories rank higher via `decay::effective_score`.
    /// Silently no-ops if `ids` is empty.
    pub fn touch_access(&self, ids: &[&str]) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let now = chrono::Utc::now().to_rfc3339();
        let tx = conn.unchecked_transaction()?;
        {
            let mut stmt = tx.prepare(
                "UPDATE memories
                 SET access_count = COALESCE(access_count, 0) + 1,
                     last_accessed_at = ?1
                 WHERE id = ?2",
            )?;
            for id in ids {
                stmt.execute(params![now, id])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Memory-centric graph payload: every active memory + the set of
    /// entity names it links to. Filterable so the UI can ask "decisions
    /// in the last 30 days mentioning inventory labeler", not the whole DB.
    ///
    /// Skips superseded memories — those got rolled into canonicals already.
    pub fn memory_graph_nodes(
        &self,
        limit: usize,
        since_days: Option<i64>,
        memory_type: Option<&str>,
        query: Option<&str>,
    ) -> Result<Vec<(MemoryEntry, Vec<String>)>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;

        // Build the WHERE clause with positional placeholders. Order matters:
        // we'll push parameters into params_vec in the same order.
        let mut where_parts: Vec<String> = vec!["m.superseded_by IS NULL".into()];
        let mut params_vec: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        let mut idx = 0;

        if let Some(days) = since_days {
            idx += 1;
            where_parts.push(format!("m.timestamp >= datetime('now', ?{idx})"));
            params_vec.push(Box::new(format!("-{days} days")));
        }
        if let Some(t) = memory_type
            && !t.is_empty()
            && t != "all"
        {
            idx += 1;
            where_parts.push(format!("m.memory_type = ?{idx}"));
            params_vec.push(Box::new(t.to_string()));
        }
        if let Some(q) = query
            && !q.trim().is_empty()
        {
            idx += 1;
            // Escape LIKE metachars (`%`, `_`, `\`) so a user search of "100%"
            // or "foo_bar" doesn't degenerate into a wildcard scan. The `\`
            // escape character is declared in the SQL via `ESCAPE '\'`.
            let pattern = format!("%{}%", escape_like(q.trim()));
            where_parts.push(format!(
                "(m.title LIKE ?{idx} ESCAPE '\\' OR m.content LIKE ?{idx} ESCAPE '\\')"
            ));
            params_vec.push(Box::new(pattern));
        }

        idx += 1;
        let limit_idx = idx;
        params_vec.push(Box::new(limit as i64));

        let sql = format!(
            "SELECT m.id, m.timestamp, m.title, m.content, m.memory_type, m.tags,
                    m.source, m.importance, m.metadata,
                    COALESCE(GROUP_CONCAT(e.name, '\u{1f}'), '') AS entity_names
             FROM memories m
             LEFT JOIN memory_entities me ON me.memory_id = m.id
             LEFT JOIN entities e ON e.id = me.entity_id
             WHERE {}
             GROUP BY m.id
             ORDER BY m.timestamp DESC
             LIMIT ?{limit_idx}",
            where_parts.join(" AND ")
        );

        let mut stmt = conn.prepare(&sql)?;
        let param_refs: Vec<&dyn rusqlite::ToSql> = params_vec.iter().map(|b| b.as_ref()).collect();

        let rows: Vec<(StorageRow, String)> = stmt
            .query_map(rusqlite::params_from_iter(param_refs.iter()), |row| {
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
                    row.get::<_, String>(9)?,
                ))
            })?
            .filter_map(|r| r.ok())
            .collect();

        let mut out = Vec::with_capacity(rows.len());
        for (row, ent_csv) in rows {
            let entry = row.into_memory_entry()?;
            // Unit-separator splits names safely even if an entity contains commas.
            let entities: Vec<String> = ent_csv
                .split('\u{1f}')
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .collect();
            out.push((entry, entities));
        }
        Ok(out)
    }

    /// Recent memories with usage stats for effective-score ranking.
    /// `last_active` falls back to `timestamp` when never accessed.
    pub fn recent_ranked(&self, limit: usize) -> Result<Vec<RankedEntry>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let mut stmt = conn.prepare(
            "SELECT id, timestamp, title, content, memory_type, tags, source, importance, metadata,
                    COALESCE(access_count, 0), last_accessed_at
             FROM memories
             WHERE superseded_by IS NULL
             ORDER BY timestamp DESC
             LIMIT ?1",
        )?;

        let rows: Vec<RankedEntry> = stmt
            .query_map(params![limit as i64], |row| {
                let storage_row = StorageRow {
                    id: row.get(0)?,
                    timestamp: row.get(1)?,
                    title: row.get(2)?,
                    content: row.get(3)?,
                    memory_type: row.get(4)?,
                    tags: row.get(5)?,
                    source: row.get(6)?,
                    importance: row.get(7)?,
                    metadata: row.get(8)?,
                };
                let access_count: i64 = row.get(9)?;
                let last_accessed_at: Option<String> = row.get(10)?;
                Ok((storage_row, access_count, last_accessed_at))
            })?
            .filter_map(|r| r.ok())
            .filter_map(|(row, access_count, last_accessed_at)| {
                let entry = row.into_memory_entry().ok()?;
                let last_active = last_accessed_at
                    .as_deref()
                    .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                    .map(|dt| dt.with_timezone(&chrono::Utc))
                    .unwrap_or(entry.timestamp);
                Some(RankedEntry {
                    entry,
                    access_count: access_count.max(0) as u32,
                    last_active,
                })
            })
            .collect();

        Ok(rows)
    }

    /// Active (non-superseded, non-session-summary) memories linked to a
    /// project entity by NAME, newest first, with access data for
    /// decay-effective ranking. Feeds the project digest builder — name (not
    /// entity UUID) keys the lookup so graph rebuilds can't orphan it.
    ///
    /// Decisions and feedback are NEVER truncated by the recency limit
    /// (review point): a busy project with hundreds of fresh notes must not
    /// evict the durable old decisions that still govern it — `limit` only
    /// bounds the note/other tail.
    pub fn project_digest_pool(
        &self,
        project_name: &str,
        limit: usize,
    ) -> Result<Vec<RankedEntry>> {
        let mut rows = self.project_pool_query(
            project_name,
            "AND m.memory_type IN ('decision', 'feedback')",
            10_000,
        )?;
        rows.extend(self.project_pool_query(
            project_name,
            "AND m.memory_type NOT IN ('decision', 'feedback', 'session_summary')",
            limit,
        )?);
        // Callers rely on newest-first ordering (the digest's Latest section).
        rows.sort_by_key(|r| std::cmp::Reverse(r.entry.timestamp));
        Ok(rows)
    }

    fn project_pool_query(
        &self,
        project_name: &str,
        type_clause: &str,
        limit: usize,
    ) -> Result<Vec<RankedEntry>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let sql = format!(
            "SELECT m.id, m.timestamp, m.title, m.content, m.memory_type, m.tags, m.source,
                    m.importance, m.metadata, COALESCE(m.access_count, 0), m.last_accessed_at
             FROM memories m
             JOIN memory_entities me ON me.memory_id = m.id
             JOIN entities e ON e.id = me.entity_id
             WHERE e.name = ?1 AND e.entity_type = 'project'
               AND m.superseded_by IS NULL
               {type_clause}
             ORDER BY m.timestamp DESC
             LIMIT ?2"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows: Vec<RankedEntry> = stmt
            .query_map(params![project_name, limit as i64], |row| {
                let storage_row = StorageRow {
                    id: row.get(0)?,
                    timestamp: row.get(1)?,
                    title: row.get(2)?,
                    content: row.get(3)?,
                    memory_type: row.get(4)?,
                    tags: row.get(5)?,
                    source: row.get(6)?,
                    importance: row.get(7)?,
                    metadata: row.get(8)?,
                };
                let access_count: i64 = row.get(9)?;
                let last_accessed_at: Option<String> = row.get(10)?;
                Ok((storage_row, access_count, last_accessed_at))
            })?
            .filter_map(|r| r.ok())
            .filter_map(|(row, access_count, last_accessed_at)| {
                let entry = row.into_memory_entry().ok()?;
                let last_active = last_accessed_at
                    .as_deref()
                    .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                    .map(|dt| dt.with_timezone(&chrono::Utc))
                    .unwrap_or(entry.timestamp);
                Some(RankedEntry {
                    entry,
                    access_count: access_count.max(0) as u32,
                    last_active,
                })
            })
            .collect();
        Ok(rows)
    }

    /// Project names ranked by WEIGHTED recent activity: decisions and
    /// feedback linked in the last `days` count 3, notes 1, session
    /// summaries 0. Recency breaks ties. Design-review point: lifetime
    /// mention_count alone would pin long-dead projects to the top.
    ///
    /// `min_real_mems` applies the caller's noise floor IN SQL (lifetime
    /// count of active non-summary memories), so sub-floor projects can
    /// never crowd buildable ones out of the LIMIT (review point).
    pub fn active_projects_weighted(
        &self,
        days: i64,
        limit: usize,
        min_real_mems: usize,
        offset: usize,
    ) -> Result<Vec<String>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let cutoff = (chrono::Utc::now() - chrono::Duration::days(days)).to_rfc3339();
        let mut stmt = conn.prepare(
            "SELECT e.name,
                    SUM(CASE m.memory_type
                        WHEN 'decision' THEN 3
                        WHEN 'feedback' THEN 3
                        WHEN 'session_summary' THEN 0
                        ELSE 1 END) AS w,
                    MAX(m.timestamp) AS latest
             FROM entities e
             JOIN memory_entities me ON me.entity_id = e.id
             JOIN memories m ON m.id = me.memory_id
             WHERE e.entity_type = 'project'
               AND m.superseded_by IS NULL
               AND m.timestamp >= ?1
             GROUP BY e.id, e.name
             HAVING w > 0
                AND (SELECT COUNT(*)
                     FROM memory_entities me2
                     JOIN memories m2 ON m2.id = me2.memory_id
                     WHERE me2.entity_id = e.id
                       AND m2.superseded_by IS NULL
                       AND m2.memory_type != 'session_summary') >= ?3
             ORDER BY w DESC, latest DESC
             LIMIT ?2 OFFSET ?4",
        )?;
        let names: Vec<String> = stmt
            .query_map(
                params![cutoff, limit as i64, min_real_mems as i64, offset as i64],
                |r| r.get::<_, String>(0),
            )?
            .filter_map(|r| r.ok())
            .collect();
        Ok(names)
    }

    /// Projects that actually have something for the contradiction lint to
    /// compare: at least two active decisions WITH embeddings. Ranked by
    /// freshest decision so projects with new (possibly reversing)
    /// decisions are checked first — a weighted-activity ranking would let
    /// note-heavy projects starve them out of the per-pass limit.
    pub fn projects_with_decision_pairs(&self, limit: usize) -> Result<Vec<String>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let mut stmt = conn.prepare(
            "SELECT e.name
             FROM entities e
             JOIN memory_entities me ON me.entity_id = e.id
             JOIN memories m ON m.id = me.memory_id
             WHERE e.entity_type = 'project'
               AND m.memory_type = 'decision'
               AND m.superseded_by IS NULL
               AND m.embedding IS NOT NULL
             GROUP BY e.id, e.name
             HAVING COUNT(*) >= 2
             ORDER BY MAX(m.timestamp) DESC
             LIMIT ?1",
        )?;
        let names: Vec<String> = stmt
            .query_map([limit as i64], |r| r.get::<_, String>(0))?
            .filter_map(|r| r.ok())
            .collect();
        Ok(names)
    }

    /// Active decisions of a project WITH their stored embeddings — the
    /// contradiction lint compares these pairwise. Content rides along
    /// (review point): titles can be generic ("Dependency change: ...")
    /// while the actual reversal lives in the body, and a judge deciding
    /// from the title alone would confirm/dismiss from incomplete text.
    pub fn project_decisions_with_embeddings(
        &self,
        project_name: &str,
        limit: usize,
    ) -> Result<Vec<DecisionRow>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let mut stmt = conn.prepare(
            "SELECT m.id, m.title, m.content, m.timestamp, m.embedding
             FROM memories m
             JOIN memory_entities me ON me.memory_id = m.id
             JOIN entities e ON e.id = me.entity_id
             WHERE e.name = ?1 AND e.entity_type = 'project'
               AND m.memory_type = 'decision'
               AND m.superseded_by IS NULL
               AND m.embedding IS NOT NULL
             ORDER BY m.timestamp DESC
             LIMIT ?2",
        )?;
        let rows: Vec<DecisionRow> = stmt
            .query_map(params![project_name, limit as i64], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, Vec<u8>>(4)?,
                ))
            })?
            .filter_map(|r| r.ok())
            .map(|(id, title, content, ts, blob)| DecisionRow {
                id,
                title,
                content,
                timestamp: ts,
                embedding: embedding_from_bytes(&blob),
            })
            .collect();
        Ok(rows)
    }

    /// Status of a recorded conflict pair, if any ('candidate' /
    /// 'confirmed' / 'dismissed'). Used to avoid re-judging pairs.
    pub fn conflict_status(&self, old_id: &str, new_id: &str) -> Result<Option<String>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let status = conn
            .query_row(
                "SELECT status FROM decision_conflicts WHERE old_id = ?1 AND new_id = ?2",
                params![old_id, new_id],
                |r| r.get::<_, String>(0),
            )
            .ok();
        Ok(status)
    }

    /// Record / update a conflict pair verdict.
    pub fn upsert_conflict(
        &self,
        old_id: &str,
        new_id: &str,
        project: &str,
        status: &str,
        confidence: Option<f32>,
        reason: Option<&str>,
    ) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        conn.execute(
            "INSERT INTO decision_conflicts
                 (old_id, new_id, project, status, confidence, reason, checked_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, datetime('now'))
             ON CONFLICT(old_id, new_id) DO UPDATE SET
                 status = excluded.status,
                 confidence = excluded.confidence,
                 reason = excluded.reason,
                 checked_at = excluded.checked_at",
            params![old_id, new_id, project, status, confidence, reason],
        )?;
        Ok(())
    }

    /// All CONFIRMED contradictions as (old_id, new_id) — surfaces consult
    /// this to hide reversed decisions from standing-decision lists while
    /// the memories themselves stay untouched.
    ///
    /// A pair only acts while BOTH sides are still alive (review point): if
    /// the replacement is later forgotten or superseded, suppressing the old
    /// decision would erase the only remaining guidance — the joins drop
    /// such stale audit rows from the result automatically.
    pub fn confirmed_conflicts(&self) -> Result<Vec<(String, String)>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let mut stmt = conn.prepare(
            "SELECT dc.old_id, dc.new_id FROM decision_conflicts dc
             JOIN memories mo ON mo.id = dc.old_id AND mo.superseded_by IS NULL
             JOIN memories mn ON mn.id = dc.new_id AND mn.superseded_by IS NULL
             WHERE dc.status = 'confirmed'",
        )?;
        let rows: Vec<(String, String)> = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    }

    pub fn count(&self) -> Result<usize> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM memories", [], |row| row.get(0))?;
        Ok(count as usize)
    }

    /// Wipe the entire knowledge graph — entities, edges, memory↔entity links
    /// and aliases. Used by `reextract --clean-graph` so a rebuild reflects
    /// updated extractor rules (reclassified types, newly-denied nodes)
    /// instead of upserting on top of stale rows with INSERT OR IGNORE.
    /// Memories themselves are untouched. Returns the entity count removed.
    pub fn clear_graph(&self) -> Result<usize> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let removed: i64 = conn
            .query_row("SELECT COUNT(*) FROM entities", [], |r| r.get(0))
            .unwrap_or(0);
        // Children before parents (FK-safe): edges/links/aliases reference entities.
        conn.execute_batch(
            "BEGIN;
             DELETE FROM edges;
             DELETE FROM memory_entities;
             DELETE FROM entity_aliases;
             DELETE FROM entities;
             COMMIT;",
        )?;
        Ok(removed as usize)
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

    /// Begin a reflection run, persist its metadata, return the run id.
    /// `mode` is "dry-run" or "apply"; threshold is the cosine cutoff used
    /// for clustering. `synthesizer` is "rule" or the LLM model id.
    pub fn begin_reflection_run(
        &self,
        mode: &str,
        threshold: f32,
        synthesizer: &str,
    ) -> Result<String> {
        let id = uuid::Uuid::new_v4().to_string();
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        conn.execute(
            "INSERT INTO reflection_runs (id, mode, threshold, synthesizer) VALUES (?1, ?2, ?3, ?4)",
            params![id, mode, threshold as f64, synthesizer],
        )?;
        Ok(id)
    }

    /// Update a reflection run's counters after the run finishes.
    pub fn finalize_reflection_run(
        &self,
        run_id: &str,
        clusters_found: usize,
        applied_count: usize,
    ) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        conn.execute(
            "UPDATE reflection_runs SET clusters_found = ?1, applied_count = ?2 WHERE id = ?3",
            params![clusters_found as i64, applied_count as i64, run_id],
        )?;
        Ok(())
    }

    /// Apply a reflection: create the canonical memory, mark sources as
    /// superseded, persist the provenance trail. Atomic — either everything
    /// lands or nothing does.
    ///
    /// `cluster` is a list of (source_memory_id, cosine_to_centroid) pairs;
    /// the canonical itself is NOT in this list. `canonical_embedding`
    /// becomes the canonical's vector. Returns the new canonical id.
    pub fn apply_reflection(
        &self,
        run_id: &str,
        canonical_entry: &MemoryEntry,
        canonical_embedding: Option<&Embedding>,
        cluster: &[(String, f32)],
    ) -> Result<String> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let tx = conn.unchecked_transaction()?;
        let blob = canonical_embedding.map(|e| embedding_to_bytes(e));

        // 1) Insert canonical memory with self-referencing canonical_memory_id.
        tx.execute(
            "INSERT INTO memories (id, timestamp, title, content, memory_type, tags, source, importance, metadata, embedding, canonical_memory_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?1)",
            params![
                canonical_entry.id,
                canonical_entry.timestamp.to_rfc3339(),
                canonical_entry.title,
                canonical_entry.content,
                canonical_entry.memory_type.to_string(),
                serde_json::to_string(&canonical_entry.tags)?,
                serde_json::to_string(&canonical_entry.source)?,
                canonical_entry.importance,
                canonical_entry.metadata.to_string(),
                blob,
            ],
        )?;

        // 2) Mark each source as superseded + record provenance.
        for (pos, (source_id, cosine)) in cluster.iter().enumerate() {
            tx.execute(
                "UPDATE memories SET superseded_by = ?1, canonical_memory_id = ?1 WHERE id = ?2",
                params![canonical_entry.id, source_id],
            )?;
            tx.execute(
                "INSERT OR REPLACE INTO reflection_sources (canonical_id, source_id, run_id, cosine, position)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![canonical_entry.id, source_id, run_id, *cosine as f64, pos as i64],
            )?;
        }

        // 3) Inherit graph links: canonical points at the union of entities
        // from all sources. Without this, graph-hop retrieval can no longer
        // surface the canonical via any entity it should belong to, since
        // sources are now superseded and filtered out everywhere.
        tx.execute(
            "INSERT OR IGNORE INTO memory_entities (memory_id, entity_id)
             SELECT ?1, entity_id FROM memory_entities WHERE memory_id IN (
                SELECT source_id FROM reflection_sources WHERE canonical_id = ?1
             )",
            params![canonical_entry.id],
        )?;

        tx.commit()?;
        drop(conn);

        // Index the new canonical embedding and evict the superseded
        // sources. SQL-side filters hide superseded rows from results, but
        // their vectors would still occupy HNSW result slots (shrinking
        // recall) and answer dedup probes (dropping new saves that resemble
        // a source the canonical replaced).
        if let Ok(mut hnsw) = self.hnsw.lock() {
            if let Some(emb) = canonical_embedding {
                hnsw.insert(&canonical_entry.id, emb);
            }
            for (source_id, _) in cluster {
                hnsw.remove(source_id);
            }
        }
        Ok(canonical_entry.id.clone())
    }

    /// Return the source ids that were consolidated into a canonical memory.
    /// Empty Vec if the id is not a canonical (or has no sources yet).
    pub fn sources_for_canonical(&self, canonical_id: &str) -> Result<Vec<(String, f32)>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let mut stmt = conn.prepare(
            "SELECT source_id, cosine FROM reflection_sources
             WHERE canonical_id = ?1 ORDER BY position ASC",
        )?;
        let rows = stmt
            .query_map(params![canonical_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)? as f32))
            })?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    }

    /// Forget a single memory by id. Cascade-cleans memory_entities and edges,
    /// FTS via the existing AFTER DELETE trigger. Returns true if a row was
    /// removed.
    pub fn forget_by_id(&self, id: &str) -> Result<bool> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let tx = conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM memory_entities WHERE memory_id = ?1",
            params![id],
        )?;
        tx.execute("DELETE FROM edges WHERE memory_id = ?1", params![id])?;
        let removed = tx.execute("DELETE FROM memories WHERE id = ?1", params![id])?;
        tx.commit()?;
        drop(conn);
        // Evict the vector too. Without this the ghost keeps matching until
        // the next restart — worst case the dedup gate sees a forgotten
        // memory as a "duplicate" and silently drops its replacement.
        if removed > 0
            && let Ok(mut hnsw) = self.hnsw.lock()
        {
            hnsw.remove(id);
        }
        Ok(removed > 0)
    }

    /// Cleanup old low-importance memories.
    /// Keeps: decisions (forever), feedback (forever), high-importance
    /// (>= threshold), and memories referenced by reflection_sources
    /// (the "NEVER deleted" reflection contract — sources must outlive
    /// cleanup so /api/memories/{id}/sources stays consistent).
    /// Removes: notes older than max_age_days with importance < threshold.
    pub fn cleanup(&self, max_age_days: i64, importance_threshold: f32) -> Result<usize> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let cutoff = chrono::Utc::now() - chrono::Duration::days(max_age_days);
        let cutoff_str = cutoff.to_rfc3339();

        // Collect the ids first so the HNSW vectors can be tombstoned after
        // the SQL delete — otherwise the ghosts keep matching until restart.
        let doomed: Vec<String> = {
            let mut stmt = conn.prepare(
                "SELECT id FROM memories
                 WHERE memory_type NOT IN ('decision', 'feedback')
                 AND importance < ?1
                 AND timestamp < ?2
                 AND id NOT IN (SELECT source_id FROM reflection_sources)
                 AND id NOT IN (SELECT canonical_id FROM reflection_sources)",
            )?;
            stmt.query_map(params![importance_threshold as f64, cutoff_str], |row| {
                row.get::<_, String>(0)
            })?
            .filter_map(|r| r.ok())
            .collect()
        };

        if doomed.is_empty() {
            return Ok(0);
        }

        let tx = conn.unchecked_transaction()?;
        let mut deleted = 0usize;
        for id in &doomed {
            deleted += tx.execute("DELETE FROM memories WHERE id = ?1", params![id])?;
        }
        tx.execute(
            "INSERT INTO memories_fts(memories_fts) VALUES('rebuild')",
            [],
        )?;
        tx.commit()?;
        drop(conn);

        if let Ok(mut hnsw) = self.hnsw.lock() {
            for id in &doomed {
                hnsw.remove(id);
            }
        }
        info!("Cleanup: removed {deleted} old low-importance memories");

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
        let with_emb: i64 = conn.query_row(
            "SELECT COUNT(*) FROM memories WHERE embedding IS NOT NULL",
            [],
            |row| row.get(0),
        )?;
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

    /// Link a memory to every EXISTING project entity whose name appears in its
    /// title. The rule extractor only knows a hardcoded `KNOWN_PROJECTS` list
    /// (mnemonic + demo stubs), so memories about real projects — "project-beta
    /// work item", "project-forge overhaul" — were never linked and stayed invisible
    /// to attribution/Journal. This matches against the projects that actually
    /// exist in the graph, so mnemonic self-learns: any project you've touched
    /// (>=3 memories, to skip generic noise like "backend") is recognised in
    /// future titles. Title-only + word-boundary so it doesn't over-link on a
    /// stray mention deep in a body. Returns the number of links added.
    ///
    /// Skips META memories — user corrections and conversation captures that
    /// merely *mention* a project ("но если project-beta сохраняет…") rather than being
    /// work on it; linking those would inflate that project's time with
    /// discussion. Each new link also bumps the entity's `mention_count` so the
    /// graph stays consistent with `replace_graph`'s decrement on reextract.
    pub fn backlink_memory_projects(
        &self,
        memory_id: &str,
        title: &str,
        memory_type: &str,
        tags_csv: &str,
    ) -> Result<usize> {
        const MIN_PROJECT_MEMS: i64 = 3;
        if is_meta_memory(memory_type, tags_csv) {
            return Ok(0);
        }
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let projects: Vec<(String, String)> = {
            let mut stmt = conn.prepare(
                "SELECT e.id, e.name FROM entities e
                 WHERE e.entity_type = 'project'
                   AND (SELECT COUNT(*) FROM memory_entities me
                        JOIN memories m ON m.id = me.memory_id
                        WHERE me.entity_id = e.id AND m.superseded_by IS NULL
                          AND m.memory_type != 'session_summary') >= ?1",
            )?;
            stmt.query_map([MIN_PROJECT_MEMS], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?
            .filter_map(|r| r.ok())
            .collect()
        };
        let head = title
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("")
            .to_lowercase();
        if head.is_empty() {
            return Ok(0);
        }
        let mut added = 0;
        for (id, name) in projects {
            // Match the project's own name OR a known alias in the title, so a
            // note titled "project-gamma dashboard deployed" links to project-alpha
            // (project-gamma/rendergen/mediagen/… → project-alpha). Meta memories are
            // already excluded above, so this only tags real work notes.
            let matched = project_name_in_text(&head, &name)
                || crate::semantic_attribution::aliases_for_project(&name)
                    .iter()
                    .any(|a| project_name_in_text(&head, a));
            if matched {
                let n = conn.execute(
                    "INSERT OR IGNORE INTO memory_entities (memory_id, entity_id) VALUES (?1, ?2)",
                    params![memory_id, id],
                )?;
                if n > 0 {
                    // Keep mention_count in step with the link we just added —
                    // replace_graph decrements it for every link it clears.
                    conn.execute(
                        "UPDATE entities SET mention_count = mention_count + 1 WHERE id = ?1",
                        params![id],
                    )?;
                    added += n;
                }
            }
        }
        Ok(added)
    }

    /// Full-DB repair behind `mnemonic backlink-projects`: re-reconcile every
    /// active memory — meta gets its project links stripped, conventional
    /// commits get pinned to their scope project, plain notes get alias-
    /// backlinked. Returns the number of memories scanned.
    pub fn reconcile_all_projects(&self) -> Result<usize> {
        let rows: Vec<(String, String, String, String)> = {
            let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
            let mut stmt = conn.prepare(
                "SELECT id, title, memory_type, COALESCE(tags, '') FROM memories
                 WHERE superseded_by IS NULL AND memory_type != 'session_summary'",
            )?;
            stmt.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                ))
            })?
            .filter_map(|r| r.ok())
            .collect()
        };
        let scanned = rows.len();
        for (id, title, mtype, tags) in rows {
            self.reconcile_memory_projects(&id, &title, &mtype, &tags)?;
        }
        Ok(scanned)
    }

    /// Strip a single memory's PROJECT associations — both the
    /// memory→project links (with `mention_count` decrement) and the project
    /// edges this memory created. Used for META memories so a project the
    /// extractor pulled out of a discussion ("но если project-beta сохраняет…") can't
    /// attribute time or pollute the graph. Returns (links removed, edges removed).
    pub fn strip_memory_project_associations(&self, memory_id: &str) -> Result<(usize, usize)> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let projects: Vec<(String, String)> = {
            let mut stmt =
                conn.prepare("SELECT id, name FROM entities WHERE entity_type = 'project'")?;
            stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
                .filter_map(|r| r.ok())
                .collect()
        };
        let (mut links, mut edges) = (0, 0);
        for (id, name) in &projects {
            let n = conn.execute(
                "DELETE FROM memory_entities WHERE memory_id = ?1 AND entity_id = ?2",
                params![memory_id, id],
            )?;
            if n > 0 {
                conn.execute(
                    "UPDATE entities SET mention_count = MAX(0, mention_count - 1) WHERE id = ?1",
                    params![id],
                )?;
                links += n;
            }
            edges += conn.execute(
                "DELETE FROM edges WHERE memory_id = ?1 AND (source_entity = ?2 OR target_entity = ?2)",
                params![memory_id, name],
            )?;
        }
        Ok((links, edges))
    }

    /// Force a memory's project links to EXACTLY `canonical_name` — a
    /// Conventional Commit's scope project. Adds that link and strips every
    /// OTHER project link + edge the extractor pulled from the subject/body, so
    /// `fix(mnemonic): … project-alpha …` counts as mnemonic only. Returns
    /// false (caller falls back to normal backlink) if the scope isn't a known
    /// project entity.
    pub fn set_memory_single_project(&self, memory_id: &str, canonical_name: &str) -> Result<bool> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let keep_id: Option<String> = conn
            .query_row(
                "SELECT id FROM entities WHERE name = ?1 AND entity_type = 'project'",
                params![canonical_name],
                |r| r.get(0),
            )
            .ok();
        let Some(keep_id) = keep_id else {
            return Ok(false);
        };
        let others: Vec<(String, String)> = {
            let mut stmt = conn.prepare(
                "SELECT e.id, e.name FROM memory_entities me
                 JOIN entities e ON e.id = me.entity_id AND e.entity_type = 'project'
                 WHERE me.memory_id = ?1 AND e.id != ?2",
            )?;
            stmt.query_map(params![memory_id, keep_id], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?
            .filter_map(|r| r.ok())
            .collect()
        };
        for (eid, ename) in others {
            let n = conn.execute(
                "DELETE FROM memory_entities WHERE memory_id = ?1 AND entity_id = ?2",
                params![memory_id, eid],
            )?;
            if n > 0 {
                conn.execute(
                    "UPDATE entities SET mention_count = MAX(0, mention_count - 1) WHERE id = ?1",
                    params![eid],
                )?;
            }
            conn.execute(
                "DELETE FROM edges WHERE memory_id = ?1 AND (source_entity = ?2 OR target_entity = ?2)",
                params![memory_id, ename],
            )?;
        }
        let n = conn.execute(
            "INSERT OR IGNORE INTO memory_entities (memory_id, entity_id) VALUES (?1, ?2)",
            params![memory_id, keep_id],
        )?;
        if n > 0 {
            conn.execute(
                "UPDATE entities SET mention_count = mention_count + 1 WHERE id = ?1",
                params![keep_id],
            )?;
        }
        Ok(true)
    }

    /// Reconcile a memory's project associations after extraction. For a META
    /// memory (correction / conversation), STRIP project links+edges the
    /// extractor may have pulled from a mere mention; otherwise BACKLINK to the
    /// projects its title names. Called by the extraction worker after
    /// `replace_graph`, so meta leakage is closed at the source — not just by
    /// the manual `backlink-projects` cleanup.
    pub fn reconcile_memory_projects(
        &self,
        memory_id: &str,
        title: &str,
        memory_type: &str,
        tags_csv: &str,
    ) -> Result<()> {
        if is_meta_memory(memory_type, tags_csv) {
            self.strip_memory_project_associations(memory_id)?;
        } else if let Some(scope) = commit_scope(title) {
            // Conventional commit: its scope IS the project. Pin to the scope and
            // drop any other project the extractor pulled from the subject/body.
            // Fall back to alias-backlink if the scope isn't a known project.
            if !self.set_memory_single_project(memory_id, &scope)? {
                self.backlink_memory_projects(memory_id, title, memory_type, tags_csv)?;
            }
        } else {
            self.backlink_memory_projects(memory_id, title, memory_type, tags_csv)?;
        }
        Ok(())
    }

    /// Canonical graph write path for re-extraction/backfill-style callers.
    ///
    /// `replace_graph` keeps mention counts/idempotency honest; the follow-up
    /// reconcile step removes project links/edges from meta memories or adds
    /// project backlinks for real work memories. Keep manual reextract, HTTP
    /// reextract, pending retries, and the worker on this same path.
    pub fn replace_graph_and_reconcile_projects(
        &self,
        entry: &MemoryEntry,
        entities: &[Entity],
        edges: &[Edge],
    ) -> Result<()> {
        self.replace_graph(&entry.id, entities, edges)?;
        self.reconcile_memory_projects(
            &entry.id,
            &entry.title,
            &entry.memory_type.to_string(),
            &entry.tags.join(","),
        )?;
        Ok(())
    }

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
    pub fn save_graph(&self, memory_id: &str, entities: &[Entity], edges: &[Edge]) -> Result<()> {
        for entity in entities {
            let entity_id = self.upsert_entity(entity)?;
            self.link_memory_entity(memory_id, &entity_id)?;
        }
        for edge in edges {
            self.save_edge(edge)?;
        }
        Ok(())
    }

    /// Idempotent graph replace for a single memory — runs in ONE transaction.
    ///
    /// `save_graph` is *additive* — each call upserts entities and bumps
    /// `mention_count` by one regardless of whether the memory was linked
    /// before. That's fine the first time a memory is extracted, but if
    /// the async worker drains a row that was already in `extraction_queue`,
    /// or if `reextract --pending` retries an LLM-failed memory whose rule
    /// graph was already saved, the same memory ends up bumping counts
    /// twice. The graph stops being a fair "how many memories mention this
    /// entity" reading.
    ///
    /// All work — decrement old counts, delete old links/edges, upsert new
    /// entities, link new memory_entities, insert new edges — happens
    /// inside a single SQLite transaction. If ANY step fails, the
    /// transaction is rolled back and the memory's pre-call graph state is
    /// preserved (no half-replaced state, no silent enrichment loss).
    ///
    /// Entities whose mention_count drops to zero are NOT deleted — they
    /// may still be referenced by aliases or reflection_sources.
    pub fn replace_graph(
        &self,
        memory_id: &str,
        entities: &[Entity],
        edges: &[Edge],
    ) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let tx = conn.unchecked_transaction()?;
        let now = chrono::Utc::now().to_rfc3339();

        // --- Phase 1: drop the old footprint inside this transaction ---
        tx.execute(
            "UPDATE entities
                SET mention_count = MAX(0, mention_count - 1)
              WHERE id IN (SELECT entity_id FROM memory_entities WHERE memory_id = ?1)",
            params![memory_id],
        )?;
        tx.execute(
            "DELETE FROM memory_entities WHERE memory_id = ?1",
            params![memory_id],
        )?;
        tx.execute("DELETE FROM edges WHERE memory_id = ?1", params![memory_id])?;

        // --- Phase 2: write the new graph inside the SAME transaction ---
        // We can't reuse the public save_graph helpers because they each
        // re-lock `self.conn` and would deadlock against the lock we already
        // hold for `tx`. The SQL is the same; just inlined against `tx`.
        //
        // Defensive dedup: if a caller hands us the same entity name twice
        // in one call (RuleExtractor already does this; CompositeExtractor
        // and any future LLM extractor *should* but might not), the naive
        // loop would SELECT then UPDATE the same row twice and inflate
        // mention_count by 2 instead of 1. Same risk for edges that share
        // (source, target, relation) within one call. Storage is the
        // chokepoint where every extractor's output lands — better to
        // enforce uniqueness here than to trust every upstream.
        //
        // Empty-name entities are dropped — they're meaningless graph
        // nodes and the canonical pass upstream usually filters them, but
        // belt-and-suspenders.
        let mut seen_entity_names: std::collections::HashSet<&str> =
            std::collections::HashSet::new();
        for entity in entities {
            let name = entity.name.trim();
            if name.is_empty() || !seen_entity_names.insert(name) {
                continue;
            }
            // Upsert entity (logic mirrors upsert_entity but reuses tx).
            let existing: Option<String> = tx
                .query_row(
                    "SELECT id FROM entities WHERE name = ?1",
                    params![name],
                    |row| row.get(0),
                )
                .ok();
            let entity_id = if let Some(id) = existing {
                tx.execute(
                    "UPDATE entities SET mention_count = mention_count + 1, last_seen = ?1
                     WHERE id = ?2",
                    params![now, id],
                )?;
                id
            } else {
                let id = uuid::Uuid::new_v4().to_string();
                tx.execute(
                    "INSERT INTO entities (id, name, entity_type, mention_count, first_seen, last_seen)
                     VALUES (?1, ?2, ?3, 1, ?4, ?4)",
                    params![id, name, entity.entity_type.to_string(), now],
                )?;
                id
            };
            tx.execute(
                "INSERT OR IGNORE INTO memory_entities (memory_id, entity_id) VALUES (?1, ?2)",
                params![memory_id, entity_id],
            )?;
        }

        let mut seen_edges: std::collections::HashSet<(&str, &str, &str)> =
            std::collections::HashSet::new();
        for edge in edges {
            // Collapse repeated (source, target, relation) triples within
            // this call. memory_id is implicit — every edge in a single
            // replace_graph call belongs to the function's `memory_id`
            // argument, NOT to whatever the caller stuffed into
            // `edge.memory_id`. Trusting the field would let a malformed
            // extractor write edges under another memory's name; pinning
            // it here makes the schema invariant unambiguous.
            if !seen_edges.insert((&edge.source, &edge.target, &edge.relation)) {
                continue;
            }
            let id = uuid::Uuid::new_v4().to_string();
            tx.execute(
                "INSERT OR IGNORE INTO edges (id, source_entity, target_entity, relation, memory_id, weight, timestamp)
                 VALUES (?1, ?2, ?3, ?4, ?5, 1.0, ?6)",
                params![id, edge.source, edge.target, edge.relation, memory_id, now],
            )?;
            tx.execute(
                "UPDATE edges SET weight = weight + 0.5
                 WHERE source_entity = ?1 AND target_entity = ?2 AND relation = ?3 AND memory_id != ?4",
                params![edge.source, edge.target, edge.relation, memory_id],
            )?;
        }

        // One commit at the very end — if any step above errored, `?`
        // bubbles up and `tx` is dropped without commit, so SQLite rolls
        // back. Pre-call state is preserved.
        tx.commit()?;
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

        let aliases = Self::aliases_for_canonical_conn(&conn, &name_lower)?;

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
            for edge in rows.flatten() {
                edges.push(edge);
            }
        }

        // Find related memories
        let mut memories = Vec::new();
        {
            let mut stmt = conn.prepare(
                "SELECT m.id, m.title, m.memory_type, m.importance, m.timestamp
                 FROM memories m
                 JOIN memory_entities me ON me.memory_id = m.id
                 WHERE me.entity_id = ?1 AND m.superseded_by IS NULL
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
            for mem in rows.flatten() {
                memories.push(mem);
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
            for n in rows.flatten() {
                neighbors.push(n);
            }
        }

        Ok(GraphResult {
            entity_name: name_lower,
            entity_type,
            mention_count,
            first_seen,
            last_seen,
            aliases,
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

    /// Full `MemoryEntry` rows for memories mentioning a specific
    /// entity name (case-insensitive lookup, canonicalized). Newest
    /// first, capped at `limit`. Excludes superseded memories so the
    /// LLM conclusion generator doesn't synthesize patterns from
    /// stale duplicates that were already rolled into canonicals.
    ///
    /// Returns empty Vec when the entity doesn't exist (silent
    /// degradation: future LLM caller surfaces "no memories" to the
    /// user rather than crashing on a typo'd subject).
    pub fn memories_for_entity_name(&self, name: &str, limit: usize) -> Result<Vec<MemoryEntry>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let mut stmt = conn.prepare(
            "SELECT m.id, m.timestamp, m.title, m.content, m.memory_type, m.tags,
                    m.source, m.importance, m.metadata
               FROM memories m
               JOIN memory_entities me ON me.memory_id = m.id
               JOIN entities e ON e.id = me.entity_id
              WHERE LOWER(e.name) = LOWER(?1)
                AND m.superseded_by IS NULL
              ORDER BY m.timestamp DESC
              LIMIT ?2",
        )?;
        let rows: Vec<MemoryEntry> = stmt
            .query_map(params![name, limit as i64], |row| {
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
            .collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .filter_map(|raw| raw.into_memory_entry().ok())
            .collect();
        Ok(rows)
    }

    /// Memory IDs linked to any of the given entity names (case-insensitive).
    /// Returns deduplicated IDs ordered by entity mention frequency × edge weight,
    /// then memory recency.
    pub fn memory_ids_for_entities(&self, names: &[&str], limit: usize) -> Result<Vec<String>> {
        if names.is_empty() {
            return Ok(Vec::new());
        }
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        // Lowercase + dedup the input.
        let mut lowered: Vec<String> = names.iter().map(|s| s.to_lowercase()).collect();
        lowered.sort();
        lowered.dedup();

        let placeholders = (0..lowered.len())
            .map(|i| format!("?{}", i + 1))
            .collect::<Vec<_>>()
            .join(",");
        let limit_idx = lowered.len() + 1;
        let sql = format!(
            "SELECT DISTINCT m.id
             FROM memory_entities me
             JOIN entities e ON e.id = me.entity_id
             JOIN memories m ON m.id = me.memory_id
             WHERE lower(e.name) IN ({placeholders})
               AND m.superseded_by IS NULL
             ORDER BY e.mention_count DESC, m.timestamp DESC
             LIMIT ?{limit_idx}"
        );
        let mut stmt = conn.prepare(&sql)?;

        let mut params_vec: Vec<&dyn rusqlite::ToSql> =
            lowered.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
        let limit_val: i64 = limit as i64;
        params_vec.push(&limit_val);

        let ids: Vec<String> = stmt
            .query_map(rusqlite::params_from_iter(params_vec.iter()), |row| {
                row.get::<_, String>(0)
            })?
            .filter_map(|r| r.ok())
            .collect();
        Ok(ids)
    }

    /// Entity names directly linked to a memory (no transitive walk).
    pub fn entity_names_for_memory(&self, memory_id: &str) -> Result<Vec<String>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let mut stmt = conn.prepare(
            "SELECT e.name
             FROM memory_entities me
             JOIN entities e ON e.id = me.entity_id
             WHERE me.memory_id = ?1",
        )?;
        let names: Vec<String> = stmt
            .query_map(params![memory_id], |row| row.get::<_, String>(0))?
            .filter_map(|r| r.ok())
            .collect();
        Ok(names)
    }

    /// Graph neighbors of given entity names, filtered by minimum edge weight.
    /// Returns unique neighbor names (lowercased), ordered by mention count.
    /// Used by K-hop subgraph expansion in `crate::retrieval`.
    pub fn weighted_neighbors(&self, names: &[&str], min_weight: f32) -> Result<Vec<String>> {
        if names.is_empty() {
            return Ok(Vec::new());
        }
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let mut lowered: Vec<String> = names.iter().map(|s| s.to_lowercase()).collect();
        lowered.sort();
        lowered.dedup();

        let placeholders = (0..lowered.len())
            .map(|i| format!("?{}", i + 1))
            .collect::<Vec<_>>()
            .join(",");
        let weight_idx = lowered.len() + 1;
        let sql = format!(
            "SELECT DISTINCT
                CASE WHEN lower(ed.source_entity) IN ({placeholders})
                     THEN ed.target_entity ELSE ed.source_entity END AS neighbor
             FROM edges ed
             WHERE (lower(ed.source_entity) IN ({placeholders})
                 OR lower(ed.target_entity) IN ({placeholders}))
               AND ed.weight >= ?{weight_idx}"
        );
        // Build SQL with three identical placeholder lists — that takes 3*N + 1 params.
        // Easier: use a single placeholder list (lowered) repeated three times via params.
        let mut stmt = conn.prepare(&sql)?;

        // Build params: lowered ×3 + min_weight
        let mut params_vec: Vec<&dyn rusqlite::ToSql> = Vec::new();
        for s in &lowered {
            params_vec.push(s as &dyn rusqlite::ToSql);
        }
        // NOTE: with the same placeholder names ?1..?N, SQLite reuses them across
        // multiple occurrences — so we DON'T pass them three times. Just lowered + weight.
        let min_weight_f64 = min_weight as f64;
        params_vec.push(&min_weight_f64);

        let names: Vec<String> = stmt
            .query_map(rusqlite::params_from_iter(params_vec.iter()), |row| {
                row.get::<_, String>(0)
            })?
            .filter_map(|r| r.ok())
            .map(|n| n.to_lowercase())
            .filter(|n| !lowered.contains(n))
            .collect();
        Ok(names)
    }

    /// Merge `alias_name` into `canonical_name` atomically.
    /// Reassigns edges and memory_entities, sums mention_count, deletes the
    /// alias row. Idempotent — if `alias_name` doesn't exist, no-op.
    ///
    /// The canonical entity must already exist (caller's responsibility);
    /// for "promote alias to canonical" use `rename_entity` instead.
    pub fn merge_entities(&self, canonical_name: &str, alias_name: &str) -> Result<MergeReport> {
        if canonical_name == alias_name {
            return Ok(MergeReport::default());
        }
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let tx = conn.unchecked_transaction()?;

        let canonical_id: Option<String> = tx
            .query_row(
                "SELECT id FROM entities WHERE name = ?1",
                params![canonical_name],
                |row| row.get(0),
            )
            .ok();
        let (alias_id, alias_mentions): (Option<String>, i64) = tx
            .query_row(
                "SELECT id, mention_count FROM entities WHERE name = ?1",
                params![alias_name],
                |row| Ok((Some(row.get::<_, String>(0)?), row.get::<_, i64>(1)?)),
            )
            .unwrap_or((None, 0));

        let (canonical_id, alias_id) = match (canonical_id, alias_id) {
            (Some(c), Some(a)) => (c, a),
            // Alias doesn't exist — nothing to merge.
            (_, None) => return Ok(MergeReport::default()),
            // Canonical missing — refuse silently. Caller should create it first.
            (None, Some(_)) => return Ok(MergeReport::default()),
        };

        // 1) memory_entities: redirect, drop dupes via INSERT OR IGNORE.
        tx.execute(
            "INSERT OR IGNORE INTO memory_entities (memory_id, entity_id)
             SELECT memory_id, ?1 FROM memory_entities WHERE entity_id = ?2",
            params![canonical_id, alias_id],
        )?;
        let me_redirected = tx.execute(
            "DELETE FROM memory_entities WHERE entity_id = ?1",
            params![alias_id],
        )?;

        // 2) edges: rename source/target alias → canonical via UPDATE OR IGNORE,
        // then drop conflict survivors. Done for source and target separately.
        let _ = tx.execute(
            "UPDATE OR IGNORE edges SET source_entity = ?1 WHERE source_entity = ?2",
            params![canonical_name, alias_name],
        )?;
        let _ = tx.execute(
            "DELETE FROM edges WHERE source_entity = ?1",
            params![alias_name],
        )?;
        let _ = tx.execute(
            "UPDATE OR IGNORE edges SET target_entity = ?1 WHERE target_entity = ?2",
            params![canonical_name, alias_name],
        )?;
        let edges_redirected = tx.execute(
            "DELETE FROM edges WHERE target_entity = ?1",
            params![alias_name],
        )?;

        // 3) mention_count rollup.
        tx.execute(
            "UPDATE entities SET mention_count = mention_count + ?1, last_seen = datetime('now')
             WHERE id = ?2",
            params![alias_mentions, canonical_id],
        )?;

        // 4) Drop the alias row.
        let alias_dropped = tx.execute("DELETE FROM entities WHERE id = ?1", params![alias_id])?;

        // 5) Preserve merge provenance so future extraction can map the old
        // name directly to the canonical entity and avoid recreating duplicates.
        tx.execute(
            "UPDATE entity_aliases SET canonical = ?1 WHERE canonical = ?2",
            params![canonical_name, alias_name],
        )?;
        tx.execute(
            "INSERT OR IGNORE INTO entity_aliases (alias, canonical) VALUES (?1, ?2)",
            params![alias_name, canonical_name],
        )?;
        tx.execute("DELETE FROM entity_aliases WHERE alias = canonical", [])?;

        tx.commit()?;

        Ok(MergeReport {
            memory_links_redirected: me_redirected,
            edges_redirected,
            mentions_summed: alias_mentions as usize,
            alias_dropped: alias_dropped > 0,
        })
    }

    /// Rename an entity row in place. Used to promote an alias to canonical
    /// when the canonical name doesn't yet exist.
    /// Returns true if a row was renamed.
    pub fn rename_entity(&self, old_name: &str, new_name: &str) -> Result<bool> {
        if old_name == new_name {
            return Ok(false);
        }
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let tx = conn.unchecked_transaction()?;
        // If new_name already exists, refuse — caller should use merge_entities.
        let new_exists: bool = tx
            .query_row(
                "SELECT 1 FROM entities WHERE name = ?1",
                params![new_name],
                |_| Ok(true),
            )
            .unwrap_or(false);
        if new_exists {
            return Ok(false);
        }
        let renamed = tx.execute(
            "UPDATE entities SET name = ?1 WHERE name = ?2",
            params![new_name, old_name],
        )?;
        if renamed == 0 {
            return Ok(false);
        }
        // Edges store names directly, so they must follow.
        tx.execute(
            "UPDATE OR IGNORE edges SET source_entity = ?1 WHERE source_entity = ?2",
            params![new_name, old_name],
        )?;
        tx.execute(
            "DELETE FROM edges WHERE source_entity = ?1",
            params![old_name],
        )?;
        tx.execute(
            "UPDATE OR IGNORE edges SET target_entity = ?1 WHERE target_entity = ?2",
            params![new_name, old_name],
        )?;
        tx.execute(
            "DELETE FROM edges WHERE target_entity = ?1",
            params![old_name],
        )?;
        tx.commit()?;
        Ok(true)
    }

    /// All entity names + ids, used by `mnemonic dedupe-graph` to compute
    /// canonical groupings without holding the conn lock the whole time.
    pub fn list_entity_names(&self) -> Result<Vec<String>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let mut stmt = conn.prepare("SELECT name FROM entities ORDER BY mention_count DESC")?;
        let names: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .filter_map(|r| r.ok())
            .collect();
        Ok(names)
    }

    /// All memory ids ordered by timestamp DESC, used by reextract.
    pub fn list_memory_ids(
        &self,
        since_days: Option<i64>,
        limit: Option<usize>,
        include_superseded: bool,
    ) -> Result<Vec<String>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let mut sql = String::from("SELECT id FROM memories");
        let mut params_vec: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        let mut where_parts: Vec<String> = Vec::new();
        if !include_superseded {
            where_parts.push("superseded_by IS NULL".into());
        }
        if let Some(days) = since_days {
            params_vec.push(Box::new(format!("-{days} days")));
            where_parts.push(format!(
                "timestamp >= datetime('now', ?{})",
                params_vec.len()
            ));
        }
        if !where_parts.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&where_parts.join(" AND "));
        }
        sql.push_str(" ORDER BY timestamp DESC");
        if let Some(n) = limit {
            sql.push_str(" LIMIT ?");
            sql.push_str(&(params_vec.len() + 1).to_string());
            params_vec.push(Box::new(n as i64));
        }
        let mut stmt = conn.prepare(&sql)?;
        let param_refs: Vec<&dyn rusqlite::ToSql> = params_vec.iter().map(|b| b.as_ref()).collect();
        let ids: Vec<String> = stmt
            .query_map(rusqlite::params_from_iter(param_refs.iter()), |row| {
                row.get::<_, String>(0)
            })?
            .filter_map(|r| r.ok())
            .collect();
        Ok(ids)
    }

    /// Get a cached LLM extraction result by content hash + extractor id.
    pub fn llm_cache_get(&self, content_hash: &str, extractor_id: &str) -> Result<Option<String>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let result: Option<String> = conn
            .query_row(
                "SELECT result_json FROM llm_extraction_cache
                 WHERE content_hash = ?1 AND extractor_id = ?2",
                params![content_hash, extractor_id],
                |row| row.get(0),
            )
            .ok();
        Ok(result)
    }

    /// Store an LLM extraction result. UPSERT semantics — same key replaces.
    pub fn llm_cache_put(
        &self,
        content_hash: &str,
        extractor_id: &str,
        result_json: &str,
    ) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        conn.execute(
            "INSERT OR REPLACE INTO llm_extraction_cache
             (content_hash, extractor_id, result_json, created_at)
             VALUES (?1, ?2, ?3, datetime('now'))",
            params![content_hash, extractor_id, result_json],
        )?;
        Ok(())
    }

    // ───────────────────── pending extractions queue ─────────────────────

    /// Record that a memory's LLM extraction failed and should be retried.
    /// Idempotent on re-enqueue (last_error refreshed, attempts/next_attempt
    /// untouched until `mark_pending_attempted` bumps them).
    ///
    /// First enqueue → `next_attempt_at = now + 60s`, so a flapping Ollama
    /// gets a chance to come back before the first retry.
    pub fn enqueue_pending_extraction(&self, memory_id: &str, error: &str) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        conn.execute(
            "INSERT INTO pending_extractions (memory_id, attempts, last_error, next_attempt_at)
                 VALUES (?1, 0, ?2, datetime('now', '+60 seconds'))
             ON CONFLICT(memory_id) DO UPDATE SET
                 last_error = excluded.last_error",
            params![memory_id, error],
        )?;
        Ok(())
    }

    /// Drop a pending row — call this when extraction finally succeeds.
    pub fn drop_pending_extraction(&self, memory_id: &str) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        conn.execute(
            "DELETE FROM pending_extractions WHERE memory_id = ?1",
            params![memory_id],
        )?;
        Ok(())
    }

    /// Bump attempts + push `next_attempt_at` further out using exponential
    /// backoff: 5m, 30m, 2h, 6h, 24h. Returns `true` if the row was kept,
    /// `false` (and deletes the row) once 6 failed attempts have accumulated.
    /// Rule-based extraction is good enough on its own for the long tail —
    /// at ~32h of cumulative backoff a memory that's still not extractable
    /// almost certainly won't recover from another retry.
    pub fn mark_pending_attempted(&self, memory_id: &str, error: &str) -> Result<bool> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let attempts: i64 = conn
            .query_row(
                "SELECT attempts FROM pending_extractions WHERE memory_id = ?1",
                params![memory_id],
                |row| row.get(0),
            )
            .unwrap_or(0);
        let next_attempts = attempts + 1;
        // Backoff schedule indexed by *new* attempts count.
        let backoff = match next_attempts {
            1 => "+5 minutes",
            2 => "+30 minutes",
            3 => "+2 hours",
            4 => "+6 hours",
            _ => "+24 hours",
        };
        let max_attempts: i64 = 5;
        if next_attempts > max_attempts {
            conn.execute(
                "DELETE FROM pending_extractions WHERE memory_id = ?1",
                params![memory_id],
            )?;
            return Ok(false);
        }
        let next_at_sql = format!("datetime('now', '{backoff}')");
        conn.execute(
            &format!(
                "UPDATE pending_extractions
                    SET attempts = ?1,
                        last_error = ?2,
                        last_attempt_at = datetime('now'),
                        next_attempt_at = {next_at_sql}
                  WHERE memory_id = ?3"
            ),
            params![next_attempts, error, memory_id],
        )?;
        Ok(true)
    }

    /// Return up to `limit` pending memory_ids whose `next_attempt_at` is
    /// in the past. Ordered oldest-due first so a long-stuck queue drains
    /// fairly. Used by `mnemonic reextract --pending`.
    pub fn pending_due_for_retry(&self, limit: usize) -> Result<Vec<String>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let mut stmt = conn.prepare(
            "SELECT memory_id FROM pending_extractions
              WHERE next_attempt_at <= datetime('now')
              ORDER BY next_attempt_at ASC
              LIMIT ?1",
        )?;
        let ids: Vec<String> = stmt
            .query_map(params![limit as i64], |row| row.get::<_, String>(0))?
            .filter_map(|r| r.ok())
            .collect();
        Ok(ids)
    }

    /// Total pending count. Surfaced via `mnemonic status` / dashboard so
    /// the user sees "12 extractions waiting" before they wonder why their
    /// graph looks sparse.
    pub fn pending_extractions_count(&self) -> Result<usize> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM pending_extractions", [], |row| {
            row.get(0)
        })?;
        Ok(n as usize)
    }

    /// Look up the metadata for one pending row — used by tests and the
    /// status command. Returns (attempts, last_error, next_attempt_at).
    pub fn pending_row(&self, memory_id: &str) -> Result<Option<(i64, Option<String>, String)>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let row = conn
            .query_row(
                "SELECT attempts, last_error, next_attempt_at
                   FROM pending_extractions WHERE memory_id = ?1",
                params![memory_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .ok();
        Ok(row)
    }

    // ─────────────── temporal facts ───────────────
    //
    // See the `facts` table migration comment for purpose. These helpers
    // implement the "supersede on conflict" policy: adding a new fact
    // with the same (subject, predicate) closes out the previous current
    // fact by setting its valid_to to the new fact's valid_from. Nothing
    // is ever deleted — the full chain stays queryable.

    /// Add a new fact. If a current fact exists for the same
    /// (subject, predicate), it gets superseded (valid_to = `valid_from`
    /// of the new fact) inside the same transaction so a concurrent
    /// reader can't observe two current facts for the same key.
    ///
    /// `valid_from` defaults to now if `None`. Caller-controllable so
    /// import paths can preserve original timestamps from a source
    /// memory's `timestamp` field.
    ///
    /// Returns the new fact's id.
    pub fn add_fact(
        &self,
        subject: &str,
        predicate: &str,
        value: &str,
        source_memory_id: &str,
        confidence: f32,
        valid_from: Option<&str>,
    ) -> Result<String> {
        let subject_lc = subject.trim().to_lowercase();
        let predicate_lc = predicate.trim().to_lowercase();
        if subject_lc.is_empty() || predicate_lc.is_empty() {
            anyhow::bail!("add_fact: subject and predicate must be non-empty");
        }
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let tx = conn.unchecked_transaction()?;
        let now = chrono::Utc::now().to_rfc3339();
        let from = valid_from.unwrap_or(&now);

        // Close any currently-valid fact for this (subject, predicate).
        // There should be at most one — the unique-current invariant is
        // enforced by always calling this method and never inserting raw.
        tx.execute(
            "UPDATE facts
                SET valid_to = ?1
              WHERE subject = ?2 AND predicate = ?3 AND valid_to IS NULL",
            params![from, subject_lc, predicate_lc],
        )?;

        let id = uuid::Uuid::new_v4().to_string();
        tx.execute(
            "INSERT INTO facts
                (id, subject, predicate, value, valid_from, valid_to, confidence, source_memory_id)
             VALUES (?1, ?2, ?3, ?4, ?5, NULL, ?6, ?7)",
            params![
                id,
                subject_lc,
                predicate_lc,
                value,
                from,
                confidence,
                source_memory_id
            ],
        )?;
        tx.commit()?;
        Ok(id)
    }

    /// Fetch the current fact (valid_to IS NULL) for one
    /// (subject, predicate). At most one row by invariant. Returns None
    /// if nothing has ever been asserted for this pair.
    pub fn latest_fact(&self, subject: &str, predicate: &str) -> Result<Option<Fact>> {
        use rusqlite::OptionalExtension;
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        // `OptionalExtension::optional()` distinguishes "no rows" (Ok(None))
        // from a real DB error (`Err`). The previous `.ok()` swallowed
        // every error class as None — a DB lock or row-decode failure
        // looked identical to "subject not found", which Codex caught.
        let row = conn
            .query_row(
                "SELECT id, subject, predicate, value, valid_from, valid_to,
                        confidence, source_memory_id, created_at
                   FROM facts
                  WHERE subject = ?1 AND predicate = ?2 AND valid_to IS NULL
                  LIMIT 1",
                params![
                    subject.trim().to_lowercase(),
                    predicate.trim().to_lowercase()
                ],
                row_to_fact,
            )
            .optional()?;
        Ok(row)
    }

    /// All currently-valid facts for a subject. Useful for context
    /// generation: "what do we currently know about inventory-labeler?"
    pub fn current_facts_for_subject(&self, subject: &str) -> Result<Vec<Fact>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let mut stmt = conn.prepare(
            "SELECT id, subject, predicate, value, valid_from, valid_to,
                    confidence, source_memory_id, created_at
               FROM facts
              WHERE subject = ?1 AND valid_to IS NULL
              ORDER BY predicate ASC",
        )?;
        // collect::<Result<_, _>>() propagates row-decode errors instead
        // of dropping bad rows on the floor like filter_map(|r| r.ok())
        // did. If a corrupted row shows up, the caller hears about it.
        let rows: Vec<Fact> = stmt
            .query_map(params![subject.trim().to_lowercase()], row_to_fact)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Full fact history for a subject — current + superseded — newest
    /// first by `valid_from`. Used for "what did we used to think before?"
    /// queries and for the eventual audit / timeline UI.
    pub fn facts_for_subject(&self, subject: &str) -> Result<Vec<Fact>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let mut stmt = conn.prepare(
            "SELECT id, subject, predicate, value, valid_from, valid_to,
                    confidence, source_memory_id, created_at
               FROM facts
              WHERE subject = ?1
              ORDER BY valid_from DESC, created_at DESC",
        )?;
        let rows: Vec<Fact> = stmt
            .query_map(params![subject.trim().to_lowercase()], row_to_fact)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Total number of fact rows (current + superseded). Surfaced via
    /// `mnemonic status` so the user knows the table is being used.
    pub fn facts_count(&self) -> Result<usize> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM facts", [], |row| row.get(0))?;
        Ok(n as usize)
    }

    // ─────────────── inductive conclusions ───────────────
    //
    // Higher-level patterns induced from clusters of memories. v1 supports
    // manual entry (CLI) and storage-level supersede semantics; v2 will
    // wire an LLM generator into the async extraction worker. See the
    // `conclusions` migration block above for design notes.

    /// Insert a new conclusion. `subject` is trimmed + lowercased for
    /// matching ("_global" is reserved for non-entity-specific claims).
    /// `kind` defaults to "pattern" if empty. `confidence` must be in
    /// [0.0, 1.0] — validation lives here, not just in the CLI, because
    /// the v2 LLM generator will call this directly and must hit the
    /// same gate. `support_count` is maintained automatically by the
    /// INSERT/DELETE triggers on `conclusion_sources`, so this method
    /// inserts the conclusion row with support=0 then lets the triggers
    /// bump it as each link lands.
    ///
    /// Does NOT auto-supersede prior conclusions — unlike `add_fact` there
    /// can legitimately be many concurrent conclusions for the same subject
    /// ("prefers rust", "favors low-overhead tooling", "ships incrementally"
    /// are all true simultaneously). Use `supersede_conclusion` explicitly
    /// when a new conclusion replaces a specific old one.
    ///
    /// Returns the new conclusion's id.
    pub fn add_conclusion(
        &self,
        subject: &str,
        kind: &str,
        statement: &str,
        confidence: f32,
        source_memory_ids: &[String],
    ) -> Result<String> {
        let subject_lc = subject.trim().to_lowercase();
        if subject_lc.is_empty() {
            anyhow::bail!(
                "add_conclusion: subject must be non-empty (use \"_global\" for non-entity claims)"
            );
        }
        if statement.trim().is_empty() {
            anyhow::bail!("add_conclusion: statement must be non-empty");
        }
        // Storage-side confidence gate. CLI already validates, but the
        // future LLM generator (and any other caller — tests, importers)
        // will hit this path directly. NaN is rejected via the !is_finite
        // check; otherwise `-5.0` would pass a naive `contains` check
        // that some refactor accidentally drops.
        if !confidence.is_finite() || !(0.0..=1.0).contains(&confidence) {
            anyhow::bail!(
                "add_conclusion: confidence must be a finite number in [0.0, 1.0], got {confidence}"
            );
        }
        let kind_trim = kind.trim();
        let kind_final = if kind_trim.is_empty() {
            "pattern"
        } else {
            kind_trim
        };

        // Dedup the source list up front so a sloppy caller passing the
        // same memory twice doesn't blow the composite PK on
        // `conclusion_sources`. The `support_count` cache stays
        // consistent automatically via the INSERT/DELETE triggers — we
        // no longer set it manually here.
        let unique_sources: Vec<&str> = {
            let mut seen = std::collections::HashSet::new();
            source_memory_ids
                .iter()
                .filter_map(|m| {
                    if seen.insert(m.as_str()) {
                        Some(m.as_str())
                    } else {
                        None
                    }
                })
                .collect()
        };

        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let tx = conn.unchecked_transaction()?;
        let id = uuid::Uuid::new_v4().to_string();
        tx.execute(
            "INSERT INTO conclusions
                (id, subject, kind, statement, confidence)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, subject_lc, kind_final, statement, confidence],
        )?;
        for mem_id in &unique_sources {
            // The AFTER INSERT trigger on conclusion_sources bumps
            // support_count by 1 per row landed.
            tx.execute(
                "INSERT INTO conclusion_sources (conclusion_id, memory_id)
                 VALUES (?1, ?2)",
                params![id, mem_id],
            )?;
        }
        tx.commit()?;
        Ok(id)
    }

    /// All current (not superseded) conclusions for a subject, newest
    /// first. The "_global" subject collects non-entity-specific claims;
    /// pass it explicitly to read those.
    pub fn current_conclusions_for_subject(&self, subject: &str) -> Result<Vec<Conclusion>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let mut stmt = conn.prepare(
            "SELECT id, subject, kind, statement, confidence, support_count,
                    created_at, last_evaluated_at, superseded_by
               FROM conclusions
              WHERE subject = ?1 AND superseded_by IS NULL
              ORDER BY confidence DESC, support_count DESC, created_at DESC",
        )?;
        let rows: Vec<Conclusion> = stmt
            .query_map(params![subject.trim().to_lowercase()], row_to_conclusion)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Full conclusion history for a subject (current + superseded),
    /// newest first. Useful for "how has our view of X evolved?" queries.
    pub fn conclusions_for_subject(&self, subject: &str) -> Result<Vec<Conclusion>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let mut stmt = conn.prepare(
            "SELECT id, subject, kind, statement, confidence, support_count,
                    created_at, last_evaluated_at, superseded_by
               FROM conclusions
              WHERE subject = ?1
              ORDER BY created_at DESC",
        )?;
        let rows: Vec<Conclusion> = stmt
            .query_map(params![subject.trim().to_lowercase()], row_to_conclusion)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Memory ids supporting a conclusion. Provides traceable evidence:
    /// "this pattern was inferred from these 12 memories".
    pub fn conclusion_sources(&self, conclusion_id: &str) -> Result<Vec<String>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let mut stmt = conn.prepare(
            "SELECT memory_id FROM conclusion_sources
              WHERE conclusion_id = ?1
              ORDER BY memory_id ASC",
        )?;
        let rows: Vec<String> = stmt
            .query_map(params![conclusion_id], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Mark `old_id` as superseded by `new_id` and bump the replacement's
    /// `last_evaluated_at`. Both rows must exist; no-op-friendly if `old`
    /// is already superseded (UPDATE … WHERE superseded_by IS NULL).
    ///
    /// Rejects `old_id == new_id` — the FK technically allows
    /// self-reference, but that would make a conclusion supersede
    /// itself and silently vanish from the current view. Codex caught
    /// this; the guard is explicit so the storage layer enforces it
    /// regardless of caller (CLI, tests, future LLM generator).
    #[allow(dead_code)]
    pub fn supersede_conclusion(&self, old_id: &str, new_id: &str) -> Result<()> {
        if old_id == new_id {
            anyhow::bail!(
                "supersede_conclusion: cannot supersede a conclusion with itself ({old_id})"
            );
        }
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let tx = conn.unchecked_transaction()?;
        let updated = tx.execute(
            "UPDATE conclusions
                SET superseded_by = ?1
              WHERE id = ?2 AND superseded_by IS NULL",
            params![new_id, old_id],
        )?;
        if updated == 0 {
            anyhow::bail!("supersede_conclusion: id {old_id} not found or already superseded");
        }
        tx.execute(
            "UPDATE conclusions
                SET last_evaluated_at = datetime('now')
              WHERE id = ?1",
            params![new_id],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Total number of conclusion rows (current + superseded). Surfaced
    /// via `mnemonic status` so the user knows the table is being used.
    pub fn conclusions_count(&self) -> Result<usize> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM conclusions", [], |row| row.get(0))?;
        Ok(n as usize)
    }

    /// Look up a single conclusion by id. Used by the CLI's
    /// `conclusion delete` / `conclusion supersede` to preview what
    /// the user is about to mutate, and to validate that the id
    /// exists before reporting success. Returns `None` for missing
    /// ids rather than erroring — the caller decides how to react.
    pub fn conclusion_by_id(&self, id: &str) -> Result<Option<Conclusion>> {
        use rusqlite::OptionalExtension;
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let row = conn
            .query_row(
                "SELECT id, subject, kind, statement, confidence, support_count,
                        created_at, last_evaluated_at, superseded_by
                   FROM conclusions
                  WHERE id = ?1",
                params![id],
                row_to_conclusion,
            )
            .optional()?;
        Ok(row)
    }

    /// Find conclusion ids whose UUID starts with `prefix`. Used by
    /// the CLI's `conclusion delete <prefix>` / `supersede <prefix>`
    /// to support short ids the way `session show` does. Same
    /// safety contract: caller enforces minimum length, this
    /// function does the lookup. Returns at most N matches so
    /// ambiguous prefixes can be surfaced rather than silently
    /// picking one.
    pub fn find_conclusion_ids_by_prefix(&self, prefix: &str) -> Result<Vec<String>> {
        // Defense in depth — reject obviously-bad prefixes before
        // hitting SQL. UUID alphabet only.
        if !prefix.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
            anyhow::bail!(
                "find_conclusion_ids_by_prefix: prefix must contain only hex digits and '-'"
            );
        }
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let mut stmt = conn.prepare("SELECT id FROM conclusions WHERE id LIKE ?1 LIMIT 5")?;
        let pattern = format!("{prefix}%");
        let rows: Vec<String> = stmt
            .query_map(params![pattern], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Delete a conclusion by id. Cascades through `conclusion_sources`
    /// via the existing ON DELETE CASCADE FK — link rows go with it,
    /// the source memories themselves stay intact. Returns true when
    /// a row was actually removed, false when the id was unknown.
    /// Idempotent: deleting an already-gone id is `Ok(false)`.
    ///
    /// Codex flagged the gap: `conclusion generate --apply` can pile
    /// duplicates and the workaround was the general-purpose
    /// `mnemonic forget <id>` (which operates on the `memories`
    /// table — wrong table for conclusions). This is the dedicated
    /// helper the CLI now exposes.
    pub fn delete_conclusion(&self, id: &str) -> Result<bool> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let removed = conn.execute("DELETE FROM conclusions WHERE id = ?1", params![id])?;
        Ok(removed > 0)
    }

    // ─────────────── peers & sessions ───────────────
    //
    // First-class identities (User, Claude, Codex, clients) and the
    // sessions that group their memories. See the `peers` migration for
    // motivation. Today these tables are populated only by explicit CLI
    // calls — the conversation watcher etc. don't auto-tag yet. That
    // wiring is a separate commit on top of this foundation.

    /// Upsert a peer. The `name` is trimmed + lowercased for matching;
    /// if a peer with that name already exists, its `last_seen_at` is
    /// touched and the existing id returned. `display_name` and `kind`
    /// on subsequent upserts are only applied when their current value
    /// is NULL / empty — explicit edits go through dedicated setters
    /// (not yet present, on purpose: this is foundation-only).
    ///
    /// Returns the peer's id.
    pub fn upsert_peer(
        &self,
        name: &str,
        display_name: Option<&str>,
        kind: &str,
    ) -> Result<String> {
        let lc = name.trim().to_lowercase();
        if lc.is_empty() {
            anyhow::bail!("upsert_peer: name must be non-empty");
        }
        let kind = kind.trim();
        if kind.is_empty() {
            anyhow::bail!("upsert_peer: kind must be non-empty");
        }
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let now = chrono::Utc::now().to_rfc3339();

        if let Ok((existing_id, existing_display, existing_kind)) = conn.query_row(
            "SELECT id, display_name, kind FROM peers WHERE name = ?1",
            params![lc],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        ) {
            // Backfill empty/missing fields from this call.
            //
            // Promised semantic: if a peer was first created without a
            // display_name (e.g. the watcher saw "claude" but didn't know
            // the casing), a later `mnemonic peer add claude --display
            // Claude` should populate it. We only overwrite when the
            // current value is NULL or whitespace-only — explicit edits
            // belong in a dedicated setter (not yet present, intentionally
            // small surface).
            //
            // Same for kind: if a future code path created a peer with a
            // generic "unknown" placeholder, the next caller passing a
            // real kind ("agent", "human") wins. The current schema has
            // NOT NULL on kind so today this branch backfills the
            // placeholder, but keeping the logic in place future-proofs
            // it against schema changes.
            let mut fields = Vec::new();
            let mut params_dyn: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

            fields.push("last_seen_at = ?".to_string());
            params_dyn.push(Box::new(now.clone()));

            if let Some(new_display) = display_name {
                let new_display_trimmed = new_display.trim();
                let current_empty = existing_display
                    .as_deref()
                    .map(|s| s.trim().is_empty())
                    .unwrap_or(true);
                if !new_display_trimmed.is_empty() && current_empty {
                    fields.push("display_name = ?".to_string());
                    params_dyn.push(Box::new(new_display.to_string()));
                }
            }
            if !kind.is_empty() && existing_kind.trim().is_empty() {
                fields.push("kind = ?".to_string());
                params_dyn.push(Box::new(kind.to_string()));
            }

            let set_clause = fields
                .iter()
                .enumerate()
                .map(|(i, f)| f.replacen('?', &format!("?{}", i + 1), 1))
                .collect::<Vec<_>>()
                .join(", ");
            params_dyn.push(Box::new(existing_id.clone()));
            let sql = format!(
                "UPDATE peers SET {set_clause} WHERE id = ?{}",
                params_dyn.len()
            );
            let refs: Vec<&dyn rusqlite::ToSql> = params_dyn.iter().map(|b| b.as_ref()).collect();
            conn.execute(&sql, rusqlite::params_from_iter(refs.iter()))?;
            return Ok(existing_id);
        }

        let id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO peers (id, name, display_name, kind, created_at, last_seen_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
            params![id, lc, display_name, kind, now],
        )?;
        Ok(id)
    }

    /// Fetch one peer by lowercased name. Returns None if no match.
    pub fn peer_by_name(&self, name: &str) -> Result<Option<Peer>> {
        use rusqlite::OptionalExtension;
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let p = conn
            .query_row(
                "SELECT id, name, display_name, kind, created_at, last_seen_at
                   FROM peers WHERE name = ?1",
                params![name.trim().to_lowercase()],
                row_to_peer,
            )
            .optional()?;
        Ok(p)
    }

    /// Fetch one peer by id. Returns None if no match. Used by the
    /// `mnemonic session list/show` CLI to resolve session.peer_id back
    /// to a human-readable label.
    pub fn peer_by_id(&self, peer_id: &str) -> Result<Option<Peer>> {
        use rusqlite::OptionalExtension;
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let p = conn
            .query_row(
                "SELECT id, name, display_name, kind, created_at, last_seen_at
                   FROM peers WHERE id = ?1",
                params![peer_id],
                row_to_peer,
            )
            .optional()?;
        Ok(p)
    }

    /// All known peers, ordered by `last_seen_at` desc (most recently
    /// active first). Bounded by `limit` — pass usize::MAX for "all".
    pub fn list_peers(&self, limit: usize) -> Result<Vec<Peer>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let mut stmt = conn.prepare(
            "SELECT id, name, display_name, kind, created_at, last_seen_at
               FROM peers
              ORDER BY last_seen_at DESC
              LIMIT ?1",
        )?;
        let rows = stmt
            .query_map(params![limit as i64], row_to_peer)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn peers_count(&self) -> Result<usize> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM peers", [], |r| r.get(0))?;
        Ok(n as usize)
    }

    /// Open a new session for `peer_id`. Returns the session id.
    /// Caller is responsible for ending it later via `end_session`.
    /// Multiple open sessions per peer are allowed (e.g. User has two
    /// Claude Code windows open simultaneously).
    ///
    /// `allow(dead_code)`: not yet called from the bin — wired in the
    /// follow-up conversation-watcher auto-tagging commit. Tests cover it.
    #[allow(dead_code)]
    pub fn open_session(&self, peer_id: &str, label: Option<&str>, source: &str) -> Result<String> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let id = uuid::Uuid::new_v4().to_string();
        // last_activity_at is set to NOW at open so subsequent idle
        // checks have a real anchor; without it the column would stay
        // NULL until the first touch and idle-expiry math would break.
        conn.execute(
            "INSERT INTO sessions (id, peer_id, label, source, last_activity_at)
             VALUES (?1, ?2, ?3, ?4, datetime('now'))",
            params![id, peer_id, label, source],
        )?;
        Ok(id)
    }

    /// Close a session by id. No-op if already ended or the id doesn't
    /// exist — call sites that depend on the close having happened can
    /// query `session_by_id` themselves.
    #[allow(dead_code)]
    pub fn end_session(&self, session_id: &str) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        conn.execute(
            "UPDATE sessions SET ended_at = datetime('now')
              WHERE id = ?1 AND ended_at IS NULL",
            params![session_id],
        )?;
        Ok(())
    }

    /// Close a session and stamp `ended_at` with a caller-supplied
    /// timestamp (RFC3339 / SQLite datetime string). Used by the
    /// SessionTracker on idle-expiry so the ended_at reflects the real
    /// end-of-activity (last_activity + idle_timeout) rather than the
    /// "moment the next event happened to fire and notice the gap".
    /// Codex caught the previous behavior: an overnight gap closed
    /// sessions with the morning timestamp, distorting session windows
    /// for dream/summary features.
    ///
    /// `open_or_reuse_session_for_key` handles the expiry path inline
    /// so this helper isn't called from the daemon loop today, but it
    /// stays public because the future `mnemonic session close <id>
    /// --at <ts>` CLI (backlog) and explicit end-of-day cleanup paths
    /// need a way to backdate.
    #[allow(dead_code)]
    pub fn end_session_at(&self, session_id: &str, ended_at: &str) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        conn.execute(
            "UPDATE sessions SET ended_at = ?1
              WHERE id = ?2 AND ended_at IS NULL",
            params![ended_at, session_id],
        )?;
        Ok(())
    }

    /// Close every open session whose last activity is older than
    /// `idle_timeout_secs`. Stamps `ended_at = last_activity_at + idle`
    /// — the same convention as SessionTracker's idle-expiry, so the
    /// recorded end reflects when work actually stopped, not when the
    /// sweep happened to run.
    ///
    /// SessionTracker only closes a session when the SAME key produces a
    /// later event. Keys that never fire again (one-off sessions, daemon
    /// restarts) stayed open forever — and every consumer that waits for
    /// closed sessions (dream consolidation, session summaries) starved.
    /// The daemon runs this on a periodic sweep. Returns closed count.
    pub fn close_idle_sessions(&self, idle_timeout_secs: u64) -> Result<usize> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let n = conn.execute(
            "UPDATE sessions
                SET ended_at = datetime(last_activity_at, '+' || ?1 || ' seconds')
              WHERE ended_at IS NULL
                AND last_activity_at IS NOT NULL
                AND last_activity_at < datetime('now', '-' || ?1 || ' seconds')",
            params![idle_timeout_secs],
        )?;
        Ok(n)
    }

    /// Bump `last_activity_at` on an open session to NOW. Called by the
    /// SessionTracker on every fast-path reuse (cache hit within idle
    /// window) so the persisted timestamp stays current — critical for
    /// restart survival because a tracker that loses its in-RAM cache
    /// reads `last_activity_at` from the DB to decide whether the
    /// previously-open session is still fresh or already idle-expired.
    ///
    /// No-op on closed sessions or unknown ids — the WHERE clause
    /// matches nothing and SQLite's UPDATE silently affects 0 rows,
    /// which is the right behavior (caller doesn't need to handle
    /// "session got closed by another path" as an error).
    pub fn touch_session_activity(&self, session_id: &str) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        conn.execute(
            "UPDATE sessions SET last_activity_at = datetime('now')
              WHERE id = ?1 AND ended_at IS NULL",
            params![session_id],
        )?;
        Ok(())
    }

    /// Atomic open-or-reuse keyed on `external_key`. The session
    /// identity helper that makes restart survival work: same key
    /// always resolves to the same session id until that session
    /// idle-expires.
    ///
    /// Three cases inside one transaction:
    ///
    /// 1. **Open session for this key, still fresh** (now - last_activity <
    ///    idle): bump `last_activity_at` to now, return existing id.
    /// 2. **Open session for this key, idle-expired**: close it with
    ///    `ended_at = last_activity_at + idle_secs` (the real end-of-
    ///    activity), then fall through to open a fresh session.
    /// 3. **No open session for this key**: open a new one.
    ///
    /// The unique partial index `idx_sessions_open_key` prevents two
    /// open sessions for the same key from coexisting — a defensive
    /// guard against bugs or racing callers; the transaction here
    /// ensures we never trip it on the happy path.
    pub fn open_or_reuse_session_for_key(
        &self,
        peer_id: &str,
        external_key: &str,
        label: Option<&str>,
        source: &str,
        idle_secs: u64,
    ) -> Result<String> {
        use rusqlite::OptionalExtension;
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let tx = conn.unchecked_transaction()?;

        // 1. Probe for an existing open session under this key.
        let existing: Option<(String, String)> = tx
            .query_row(
                "SELECT id, COALESCE(last_activity_at, started_at)
                   FROM sessions
                  WHERE external_key = ?1 AND ended_at IS NULL
                  LIMIT 1",
                params![external_key],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;

        if let Some((existing_id, last_activity_str)) = existing {
            // Parse the stored timestamp; if it's malformed, treat as
            // ancient (force expiry) rather than crashing — defensive
            // for any hand-edited rows or pre-migration data.
            let last_activity = chrono::DateTime::parse_from_rfc3339(&last_activity_str)
                .map(|dt| dt.with_timezone(&chrono::Utc))
                .or_else(|_| {
                    // SQLite datetime() format ("YYYY-MM-DD HH:MM:SS")
                    // isn't strict RFC3339; try a permissive parse.
                    chrono::NaiveDateTime::parse_from_str(&last_activity_str, "%Y-%m-%d %H:%M:%S")
                        .map(|ndt| ndt.and_utc())
                });
            let now = chrono::Utc::now();
            let idle = chrono::Duration::seconds(idle_secs as i64);

            match last_activity {
                Ok(la) if now.signed_duration_since(la) < idle => {
                    // Case 1: fresh — bump activity, reuse id, and re-home to
                    // the current owner peer. An open session carried over
                    // from an older build (e.g. a Codex session previously
                    // opened under the Claude peer) must move to its correct
                    // owner instead of staying mislabeled until idle expiry.
                    // For same-owner reuse this writes the same peer_id, a
                    // harmless no-op.
                    tx.execute(
                        "UPDATE sessions SET last_activity_at = ?1, peer_id = ?2 WHERE id = ?3",
                        params![now.to_rfc3339(), peer_id, existing_id],
                    )?;
                    tx.commit()?;
                    return Ok(existing_id);
                }
                Ok(la) => {
                    // Case 2: expired — close at the real end-of-activity
                    // (last_activity + idle), then fall through to open
                    // a fresh session for this key.
                    let ended_at = (la + idle).to_rfc3339();
                    tx.execute(
                        "UPDATE sessions SET ended_at = ?1 WHERE id = ?2",
                        params![ended_at, existing_id],
                    )?;
                }
                Err(_) => {
                    // Malformed timestamp — close with now and reopen.
                    // Logged so we notice if this path fires unexpectedly.
                    warn!(
                        "open_or_reuse_session_for_key: malformed last_activity_at \
                         `{last_activity_str}` on session {existing_id}, closing and reopening"
                    );
                    tx.execute(
                        "UPDATE sessions SET ended_at = datetime('now') WHERE id = ?1",
                        params![existing_id],
                    )?;
                }
            }
        }

        // Case 3 (or fallthrough from case 2): open a new session.
        let new_id = uuid::Uuid::new_v4().to_string();
        tx.execute(
            "INSERT INTO sessions (id, peer_id, label, source, external_key, last_activity_at)
             VALUES (?1, ?2, ?3, ?4, ?5, datetime('now'))",
            params![new_id, peer_id, label, source, external_key],
        )?;
        tx.commit()?;
        Ok(new_id)
    }

    /// Open (un-ended) sessions for a single peer, newest first.
    /// Dedicated helper because the CLI's `session list --peer X --open
    /// --limit N` previously did `sessions_for_peer(N).filter(is_open)`
    /// — if the most recent N rows for that peer were all closed, the
    /// open ones beyond the window vanished silently. Codex caught this.
    /// The filter belongs inside the SQL so LIMIT N applies AFTER the
    /// open check.
    pub fn open_sessions_for_peer(&self, peer_id: &str, limit: usize) -> Result<Vec<Session>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let sql = format!(
            "SELECT {SESSION_COLUMNS}
               FROM sessions
              WHERE peer_id = ?1 AND ended_at IS NULL
              ORDER BY started_at DESC
              LIMIT ?2"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt
            .query_map(params![peer_id, limit as i64], row_to_session)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// All sessions for a peer, newest first. Useful for "show me the last
    /// 5 things this agent worked on".
    pub fn sessions_for_peer(&self, peer_id: &str, limit: usize) -> Result<Vec<Session>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let sql = format!(
            "SELECT {SESSION_COLUMNS}
               FROM sessions
              WHERE peer_id = ?1
              ORDER BY started_at DESC
              LIMIT ?2"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt
            .query_map(params![peer_id, limit as i64], row_to_session)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Open (un-ended) sessions across all peers. Returns the actual
    /// rows (capped at `limit`). For a true count use
    /// `open_sessions_count` — that one isn't bounded.
    /// `allow(dead_code)`: foundation for v2 watcher wiring; tested.
    #[allow(dead_code)]
    pub fn open_sessions(&self, limit: usize) -> Result<Vec<Session>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let sql = format!(
            "SELECT {SESSION_COLUMNS}
               FROM sessions
              WHERE ended_at IS NULL
              ORDER BY started_at DESC
              LIMIT ?1"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt
            .query_map(params![limit as i64], row_to_session)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    #[allow(dead_code)]
    pub fn sessions_count(&self) -> Result<usize> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get(0))?;
        Ok(n as usize)
    }

    /// Count of unfinished sessions across all peers — dedicated COUNT(*)
    /// rather than `open_sessions(N).len()` because that one capped at N
    /// and would silently misreport ("1 session open" when 12 are open
    /// because the caller asked for limit=1). Surfaced in `mnemonic status`.
    pub fn open_sessions_count(&self) -> Result<usize> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sessions WHERE ended_at IS NULL",
            [],
            |r| r.get(0),
        )?;
        Ok(n as usize)
    }

    /// Fetch a session by id. Returns `None` if the id doesn't exist.
    /// Used by the CLI `session show` and by the future watcher when
    /// it needs to verify a cached session id still points at a row.
    pub fn session_by_id(&self, session_id: &str) -> Result<Option<Session>> {
        use rusqlite::OptionalExtension;
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let sql = format!(
            "SELECT {SESSION_COLUMNS}
               FROM sessions
              WHERE id = ?1"
        );
        let row = conn
            .query_row(&sql, params![session_id], row_to_session)
            .optional()?;
        Ok(row)
    }

    /// Longest COMPLETED sessions, newest-format-agnostic. Durations are
    /// computed in Rust via `parse_session_timestamp` (the live DB carries
    /// mixed SQLite `datetime('now')` and RFC3339 rows — SQL-side julianday
    /// math would silently misorder any format it can't parse). Open
    /// sessions (`ended_at IS NULL`), malformed timestamps, and negative
    /// durations (clock skew / hand-edited rows) are dropped, not guessed.
    ///
    /// `top_project` comes from `project_signals_in_window` over the padded
    /// session window — the SAME signal the attribution worker uses. It is
    /// deliberately NOT derived from `memories.session_id` joins (Codex P1):
    /// in the watcher flow the only memories carrying a session_id are
    /// corrections/conversation decisions, and `reconcile_memory_projects`
    /// classifies those as meta and strips their project links, so that
    /// join is empty on production data.
    pub fn longest_sessions(&self, limit: usize) -> Result<Vec<SessionRecordRow>> {
        // Mirror the attribution worker's window constants: memories often
        // land a few minutes after the work burst, and a project below the
        // real-memory floor is extractor noise, not a leaderboard winner.
        const SIGNAL_PAD_MINUTES: i64 = 10;
        const MIN_PROJECT_MEMS: i64 = 2;

        if limit == 0 {
            return Ok(Vec::new());
        }
        // A session's parsed time span. ALL sessions are collected (open
        // included) — open ones can't rank, but their start still bounds
        // a neighbour's signal pad below.
        struct SessionSpan {
            id: String,
            started_at_raw: String,
            start: chrono::DateTime<chrono::Utc>,
            end: Option<chrono::DateTime<chrono::Utc>>,
        }
        let all: Vec<SessionSpan> = {
            let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
            let mut stmt = conn.prepare("SELECT id, started_at, ended_at FROM sessions")?;
            let raw = stmt
                .query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, Option<String>>(2)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            raw.into_iter()
                .filter_map(|(id, started_at, ended_at)| {
                    let start = parse_session_timestamp(&started_at)?;
                    // Genuinely OPEN rows stay (their start bounds a
                    // neighbour's pad), but a COMPLETED row with a
                    // malformed or skewed end is dropped entirely — its
                    // bogus timestamps must not win prev_end/next_start
                    // and truncate a valid session's signal window
                    // (review point).
                    let end = match ended_at.as_deref() {
                        None => None,
                        Some(raw_end) => {
                            let e = parse_session_timestamp(raw_end)?;
                            if e < start {
                                return None;
                            }
                            Some(e)
                        }
                    };
                    Some(SessionSpan {
                        id,
                        started_at_raw: started_at,
                        start,
                        end,
                    })
                })
                .collect()
        };
        // (row, parsed start, parsed end) — completed sessions only.
        let mut rows: Vec<(
            SessionRecordRow,
            chrono::DateTime<chrono::Utc>,
            chrono::DateTime<chrono::Utc>,
        )> = all
            .iter()
            .filter_map(|span| {
                let end = span.end?;
                let secs = end.signed_duration_since(span.start).num_seconds();
                if secs < 0 {
                    return None;
                }
                Some((
                    SessionRecordRow {
                        session_id: span.id.clone(),
                        started_at: span.started_at_raw.clone(),
                        duration_seconds: secs,
                        top_project: None,
                    },
                    span.start,
                    end,
                ))
            })
            .collect();
        // Longest first; started_at DESC breaks ties deterministically.
        rows.sort_by(|a, b| {
            b.0.duration_seconds
                .cmp(&a.0.duration_seconds)
                .then_with(|| b.0.started_at.cmp(&a.0.started_at))
        });
        rows.truncate(limit);

        // Fill top_project per row (bounded by the clamped limit). The
        // signal pad is clamped to the midpoint toward the nearest
        // neighbouring session on each side — the same idea the
        // attribution worker applies to work-session pads — so a memory
        // saved between two sessions counts toward the closer one only,
        // and back-to-back sessions can't leak signal into each other
        // (review point). Core overlaps (truly concurrent sessions) keep
        // window semantics: the label is the window's dominant project.
        // Signal failures degrade to "no project", never an error — the
        // ranking itself must not depend on the graph being healthy.
        let pad = chrono::Duration::minutes(SIGNAL_PAD_MINUTES);
        let mut out = Vec::with_capacity(rows.len());
        for (mut row, start, end) in rows {
            let prev_end = all
                .iter()
                .filter(|span| span.id != row.session_id)
                .filter_map(|span| span.end)
                .filter(|e| *e <= start)
                .max();
            let next_start = all
                .iter()
                .filter(|span| span.id != row.session_id)
                .map(|span| span.start)
                .filter(|s| *s >= end)
                .min();
            let mut lo = start - pad;
            if let Some(pe) = prev_end {
                lo = lo.max(pe + (start - pe) / 2);
            }
            let mut hi = end + pad;
            if let Some(ns) = next_start {
                hi = hi.min(end + (ns - end) / 2);
            }
            // The pad may shrink to nothing, but never inverts the core.
            lo = lo.min(start);
            hi = hi.max(end);
            let mut signals = self
                .project_signals_in_window(lo, hi, MIN_PROJECT_MEMS)
                .unwrap_or_default();
            signals.sort_by(|a, b| {
                b.weight
                    .partial_cmp(&a.weight)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| a.project_key.cmp(&b.project_key))
            });
            row.top_project = signals
                .first()
                .and_then(|s| self.entity_name(&s.project_key).ok().flatten());
            out.push(row);
        }
        Ok(out)
    }

    /// All session ids whose UUID starts with `prefix`. Used by the CLI
    /// `session show` to support short ids (first 8+ chars) without
    /// silently grabbing the first match — the caller checks `.len() ==
    /// 1` before consuming the result.
    ///
    /// LIKE 'prefix%' is safe here because session ids are UUIDs — no
    /// user-controlled SQL wildcards in the input space. The trailing
    /// `%` is a real wildcard; if a future caller passes a prefix that
    /// contains `%` or `_`, results would be unexpected — guarded by
    /// rejecting non-hex/dash characters explicitly.
    pub fn find_session_ids_by_prefix(&self, prefix: &str) -> Result<Vec<String>> {
        if prefix.is_empty() {
            anyhow::bail!("find_session_ids_by_prefix: prefix must be non-empty");
        }
        if !prefix.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
            anyhow::bail!(
                "find_session_ids_by_prefix: prefix must contain only hex digits and '-'"
            );
        }
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let pattern = format!("{prefix}%");
        let mut stmt = conn.prepare("SELECT id FROM sessions WHERE id LIKE ?1 LIMIT 50")?;
        let rows: Vec<String> = stmt
            .query_map(params![pattern], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Attach a memory to a session. Idempotent. Passing `None` for
    /// `session_id` clears the link (returns the memory to "session-less"
    /// state — useful for tests and the eventual `session backfill` CLI).
    ///
    /// Validates `session_id` exists when non-NULL. On fresh DBs the FK
    /// on `memories.session_id` enforces this at the DB level too;
    /// legacy DBs lack the FK (SQLite forbids `ALTER TABLE ADD COLUMN`
    /// with a REFERENCES clause when foreign_keys is ON), so the
    /// app-level check is the chokepoint that catches typos there.
    /// `allow(dead_code)`: wired in the follow-up watcher commit.
    #[allow(dead_code)]
    pub fn set_memory_session(&self, memory_id: &str, session_id: Option<&str>) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        if let Some(sid) = session_id {
            let exists: i64 = conn.query_row(
                "SELECT COUNT(*) FROM sessions WHERE id = ?1",
                params![sid],
                |r| r.get(0),
            )?;
            if exists == 0 {
                anyhow::bail!("set_memory_session: session id {sid} does not exist");
            }
        }
        let updated = conn.execute(
            "UPDATE memories SET session_id = ?1 WHERE id = ?2",
            params![session_id, memory_id],
        )?;
        if updated == 0 {
            anyhow::bail!("set_memory_session: memory id {memory_id} does not exist");
        }
        Ok(())
    }

    /// All memories captured in a session, oldest first. Oldest-first is
    /// deliberate: a session is a chronological thread, and the CLI
    /// `session show` reads top-to-bottom like a conversation transcript.
    pub fn memories_for_session(&self, session_id: &str) -> Result<Vec<MemoryEntry>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let mut stmt = conn.prepare(
            "SELECT id, timestamp, title, content, memory_type, tags, source,
                    importance, metadata
               FROM memories
              WHERE session_id = ?1
              ORDER BY timestamp ASC, created_at ASC",
        )?;
        let rows: Vec<MemoryEntry> = stmt
            .query_map(params![session_id], |row| {
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
            // collect::<Result<_,_>>() instead of filter_map(|r| r.ok())
            // so that a row-decode error from rusqlite surfaces as an
            // error from this helper rather than silently dropping the row.
            // The subsequent into_memory_entry() can still tolerate
            // best-effort parsing of individual fields (timestamps, etc.)
            // because the row decode succeeded.
            .collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .filter_map(|raw| raw.into_memory_entry().ok())
            .collect();
        Ok(rows)
    }

    /// Top entities by mention count across all memories in a session.
    /// Joins `memory_entities` → `entities` filtered to memory ids
    /// owned by the session, GROUP BY entity name, ORDER BY count DESC.
    /// Used by the dream consolidation summarizer to surface the
    /// topics that dominated a work window.
    pub fn top_entities_for_session(
        &self,
        session_id: &str,
        limit: usize,
    ) -> Result<Vec<(String, i64)>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let mut stmt = conn.prepare(
            "SELECT e.name, COUNT(*) AS mentions
               FROM memories m
               JOIN memory_entities me ON me.memory_id = m.id
               JOIN entities e ON e.id = me.entity_id
              WHERE m.session_id = ?1
              GROUP BY e.name
              ORDER BY mentions DESC, e.name ASC
              LIMIT ?2",
        )?;
        let rows: Vec<(String, i64)> = stmt
            .query_map(params![session_id, limit as i64], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Find a CANONICAL `session_summary` memory for a session —
    /// i.e., a summary that was generated AFTER the session was
    /// closed (`metadata.open_at_summary_time = false` or absent).
    /// Returns `None` if only snapshots-of-open-sessions exist,
    /// which is what the dream batch path needs so a `--allow-open`
    /// snapshot doesn't freeze the session from getting a real
    /// summary after it closes. Codex P2 caught the gap.
    ///
    /// Old-format summaries (pre-fix) don't carry the
    /// `open_at_summary_time` key at all — those are treated as
    /// canonical too, because in the pre-fix world only closed
    /// sessions were summarized in practice. Avoids re-summarizing
    /// every old session after this upgrade.
    ///
    /// To find ALL summaries including snapshots (e.g., for an
    /// audit / history view), use
    /// `session_summary_lookup_including_snapshots`.
    ///
    /// Uses SQLite's `json_extract` against the metadata column.
    /// Today there's no index on metadata; cost is O(N) over rows
    /// of type session_summary. That's bounded — summary counts grow
    /// linearly with sessions, not memories. Add an index when the
    /// count starts hurting (probably never).
    pub fn session_summary_lookup(&self, session_id: &str) -> Result<Option<MemoryEntry>> {
        self.session_summary_lookup_impl(session_id, false)
    }

    /// Find ANY session_summary for the given session, including
    /// snapshots produced via `--allow-open`. Used for audit and
    /// debugging. The dream module uses the non-snapshot variant
    /// (`session_summary_lookup`) for its idempotency check so
    /// snapshots don't block canonical summaries.
    #[allow(dead_code)]
    pub fn session_summary_lookup_including_snapshots(
        &self,
        session_id: &str,
    ) -> Result<Option<MemoryEntry>> {
        self.session_summary_lookup_impl(session_id, true)
    }

    fn session_summary_lookup_impl(
        &self,
        session_id: &str,
        include_snapshots: bool,
    ) -> Result<Option<MemoryEntry>> {
        use rusqlite::OptionalExtension;
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        // The snapshot filter: `open_at_summary_time IS NULL` for
        // pre-fix rows (no key), or `= 0` for fix-era closed-session
        // summaries. `json_extract` returns SQL NULL when the key
        // is absent, so `IS NOT 1` does the right thing for both.
        // SQLite booleans are stored as integers; serde_json::json!
        // emits `false` which `json_extract` returns as integer 0.
        let snapshot_filter = if include_snapshots {
            ""
        } else {
            "AND COALESCE(json_extract(metadata, '$.open_at_summary_time'), 0) = 0"
        };
        let sql = format!(
            "SELECT id, timestamp, title, content, memory_type, tags, source,
                    importance, metadata
               FROM memories
              WHERE memory_type = 'session_summary'
                AND json_extract(metadata, '$.summary_of_session') = ?1
                {snapshot_filter}
              ORDER BY timestamp DESC
              LIMIT 1"
        );
        let row = conn
            .query_row(&sql, params![session_id], |row| {
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
            })
            .optional()?;
        Ok(row.and_then(|raw| raw.into_memory_entry().ok()))
    }

    /// Closed sessions whose `ended_at` is no older than `since_hours`
    /// hours from now, ordered newest-ended first. The dream batch
    /// CLI uses this to pick recently-finished sessions for
    /// summarization (skipping ancient ones avoids re-summarizing
    /// historical data on every run).
    pub fn closed_sessions_since(&self, since_hours: u64, limit: usize) -> Result<Vec<Session>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        // SQLite `datetime('now', '-N hours')` returns the cutoff
        // timestamp; rows with ended_at >= cutoff qualify. RFC3339
        // and SQLite's own format are both string-comparable for
        // ISO-8601 — but we're conservative and use SQLite's date
        // math against `ended_at` directly.
        let sql = format!(
            "SELECT {SESSION_COLUMNS}
               FROM sessions
              WHERE ended_at IS NOT NULL
                AND datetime(ended_at) >= datetime('now', ?1)
              ORDER BY ended_at DESC
              LIMIT ?2"
        );
        let mut stmt = conn.prepare(&sql)?;
        let offset = format!("-{since_hours} hours");
        let rows = stmt
            .query_map(params![offset, limit as i64], row_to_session)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Attach a peer to a memory with a role. Idempotent on (memory, peer,
    /// role) by primary key. Role is free-form: "speaker" (default —
    /// who originated the memory), "subject" (who it's about),
    /// "mentioned", "addressee", etc.
    #[allow(dead_code)]
    pub fn link_memory_peer(&self, memory_id: &str, peer_id: &str, role: &str) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        conn.execute(
            "INSERT OR IGNORE INTO memory_peers (memory_id, peer_id, role)
             VALUES (?1, ?2, ?3)",
            params![memory_id, peer_id, role],
        )?;
        Ok(())
    }

    /// All peers attached to a memory, each with their role.
    /// Returned as (Peer, role) pairs ordered by role then peer name.
    #[allow(dead_code)]
    pub fn peers_for_memory(&self, memory_id: &str) -> Result<Vec<(Peer, String)>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let mut stmt = conn.prepare(
            "SELECT p.id, p.name, p.display_name, p.kind, p.created_at, p.last_seen_at,
                    mp.role
               FROM memory_peers mp
               JOIN peers p ON p.id = mp.peer_id
              WHERE mp.memory_id = ?1
              ORDER BY mp.role ASC, p.name ASC",
        )?;
        let rows: Vec<(Peer, String)> = stmt
            .query_map(params![memory_id], |row| {
                Ok((
                    Peer {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        display_name: row.get(2)?,
                        kind: row.get(3)?,
                        created_at: row.get(4)?,
                        last_seen_at: row.get(5)?,
                    },
                    row.get::<_, String>(6)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Admin op: merge two peers — moves all memory_peers rows from
    /// `src` to `dst`, then deletes `src`. Useful when a default was
    /// changed and an old peer needs to be folded into a new one
    /// (e.g. `user → user` after the default rename).
    ///
    /// Both peers must already exist by canonical name. Returns the
    /// number of links that were re-pointed (some collide on the new
    /// PK and are silently merged via `INSERT OR IGNORE`).
    pub fn merge_peers(&self, src_name: &str, dst_name: &str) -> Result<usize> {
        let src_name = src_name.trim().to_lowercase();
        let dst_name = dst_name.trim().to_lowercase();
        if src_name == dst_name {
            anyhow::bail!("merge_peers: src and dst are the same ({src_name})");
        }

        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let tx = conn.unchecked_transaction()?;

        let src_id: String = tx
            .query_row(
                "SELECT id FROM peers WHERE name = ?1",
                params![src_name],
                |row| row.get(0),
            )
            .map_err(|_| anyhow::anyhow!("merge_peers: source peer '{src_name}' not found"))?;
        let dst_id: String = tx
            .query_row(
                "SELECT id FROM peers WHERE name = ?1",
                params![dst_name],
                |row| row.get(0),
            )
            .map_err(|_| anyhow::anyhow!("merge_peers: dest peer '{dst_name}' not found"))?;

        // Re-point memory_peers from src → dst. PK is (memory_id, peer_id,
        // role); a row that already exists for dst with the same role
        // would collide. Use INSERT OR IGNORE + DELETE rather than UPDATE
        // so collisions are handled cleanly.
        let moved = tx.execute(
            "INSERT OR IGNORE INTO memory_peers (memory_id, peer_id, role)
             SELECT memory_id, ?1, role FROM memory_peers WHERE peer_id = ?2",
            params![dst_id, src_id],
        )?;
        tx.execute(
            "DELETE FROM memory_peers WHERE peer_id = ?1",
            params![src_id],
        )?;

        // sessions also FK to peers via ON DELETE CASCADE — repoint
        // those explicitly so an active session's history isn't lost.
        tx.execute(
            "UPDATE sessions SET peer_id = ?1 WHERE peer_id = ?2",
            params![dst_id, src_id],
        )?;

        // Finally drop the source peer. With FKs now pointing at dst,
        // the CASCADE on the now-empty src is a no-op but kept for
        // shape.
        tx.execute("DELETE FROM peers WHERE id = ?1", params![src_id])?;
        tx.commit()?;

        Ok(moved)
    }

    // ─────────────── extraction_queue: first-attempt async work ───────────────
    //
    // See the `extraction_queue` migration comment for why this lives in a
    // separate table from `pending_extractions`. TL;DR: this queue is
    // "scheduled but never tried", `pending_extractions` is "tried, failed,
    // backing off."

    /// Enqueue a freshly-saved memory for background entity extraction.
    /// Idempotent — `INSERT OR IGNORE` so repeated enqueues for the same id
    /// (e.g. a memory that got reflected or merged) don't pile up.
    pub fn enqueue_extraction(&self, memory_id: &str) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        conn.execute(
            "INSERT OR IGNORE INTO extraction_queue (memory_id) VALUES (?1)",
            params![memory_id],
        )?;
        Ok(())
    }

    /// Fetch up to `limit` memory ids from the queue. Does NOT delete —
    /// caller is responsible for `dequeue_extraction(id)` once the work
    /// succeeds, so a crashed worker doesn't lose unprocessed rows.
    ///
    /// Never-tried rows go first (attempts ASC), then oldest-first.
    /// Plain oldest-first let a handful of poisoned rows camp at the head:
    /// with `batch_size` of them failing every tick, fresh saves behind
    /// them never got a first attempt (head-of-line blocking).
    pub fn next_extraction_batch(&self, limit: usize) -> Result<Vec<String>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let mut stmt = conn.prepare(
            "SELECT memory_id FROM extraction_queue
              ORDER BY attempts ASC, enqueued_at ASC
              LIMIT ?1",
        )?;
        let ids: Vec<String> = stmt
            .query_map(params![limit as i64], |row| row.get::<_, String>(0))?
            .filter_map(|r| r.ok())
            .collect();
        Ok(ids)
    }

    /// Drop a row from the queue. Called after the worker has processed it,
    /// regardless of whether extraction yielded entities or fell through to
    /// rule-only — the queue just tracks "needs first attempt".
    pub fn dequeue_extraction(&self, memory_id: &str) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        conn.execute(
            "DELETE FROM extraction_queue WHERE memory_id = ?1",
            params![memory_id],
        )?;
        Ok(())
    }

    /// Record a failed first-attempt extraction. Bumps `attempts`; once
    /// they reach `max_attempts` the row is dead-lettered into
    /// `pending_extractions` (the backoff retry queue, visible in
    /// `mnemonic status` and drained by `mnemonic reextract --pending`)
    /// instead of looping in `extraction_queue` forever. Returns `true`
    /// when the row was dead-lettered.
    pub fn fail_extraction(&self, memory_id: &str, error: &str, max_attempts: i64) -> Result<bool> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let attempts: i64 = conn
            .query_row(
                "SELECT attempts FROM extraction_queue WHERE memory_id = ?1",
                params![memory_id],
                |row| row.get(0),
            )
            .unwrap_or(0)
            + 1;
        if attempts >= max_attempts {
            let tx = conn.unchecked_transaction()?;
            tx.execute(
                "DELETE FROM extraction_queue WHERE memory_id = ?1",
                params![memory_id],
            )?;
            // Same upsert as enqueue_pending_extraction — inlined because
            // that helper takes the conn lock this fn already holds.
            tx.execute(
                "INSERT INTO pending_extractions (memory_id, attempts, last_error, next_attempt_at)
                     VALUES (?1, 0, ?2, datetime('now', '+60 seconds'))
                 ON CONFLICT(memory_id) DO UPDATE SET
                     last_error = excluded.last_error",
                params![memory_id, error],
            )?;
            tx.commit()?;
            return Ok(true);
        }
        conn.execute(
            "UPDATE extraction_queue SET attempts = ?1 WHERE memory_id = ?2",
            params![attempts, memory_id],
        )?;
        Ok(false)
    }

    /// Queue depth — surfaced via `mnemonic status` so the user sees when
    /// the async worker is falling behind.
    pub fn extraction_queue_count(&self) -> Result<usize> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM extraction_queue", [], |row| {
            row.get(0)
        })?;
        Ok(n as usize)
    }

    /// Look up the canonical name for a known alias.
    ///
    /// Chain merges are followed defensively even though merge_entities also
    /// rewrites old alias rows to point at the newest canonical name.
    pub fn canonical_for_alias(&self, name: &str) -> Result<Option<String>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        Self::canonical_for_alias_conn(&conn, name)
    }

    fn canonical_for_alias_conn(conn: &Connection, name: &str) -> Result<Option<String>> {
        let mut current = name.to_lowercase();
        let mut resolved = None;
        let mut seen = std::collections::HashSet::new();

        for _ in 0..16 {
            if !seen.insert(current.clone()) {
                break;
            }

            let next: Option<String> = conn
                .query_row(
                    "SELECT canonical FROM entity_aliases WHERE lower(alias) = ?1
                     ORDER BY merged_at DESC LIMIT 1",
                    params![current],
                    |row| row.get::<_, String>(0),
                )
                .ok()
                .map(|s| s.to_lowercase());

            match next {
                Some(canonical) if canonical != current => {
                    resolved = Some(canonical.clone());
                    current = canonical;
                }
                Some(canonical) => {
                    resolved = Some(canonical);
                    break;
                }
                None => break,
            }
        }

        Ok(resolved)
    }

    fn aliases_for_canonical_conn(conn: &Connection, canonical: &str) -> Result<Vec<String>> {
        let mut stmt = conn
            .prepare("SELECT alias FROM entity_aliases WHERE canonical = ?1 ORDER BY alias ASC")?;
        let aliases = stmt
            .query_map(params![canonical], |row| row.get::<_, String>(0))?
            .filter_map(|r| r.ok())
            .collect();
        Ok(aliases)
    }

    /// Count entities and edges
    pub fn graph_stats(&self) -> Result<(usize, usize)> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let entities: i64 =
            conn.query_row("SELECT COUNT(*) FROM entities", [], |row| row.get(0))?;
        let edges: i64 = conn.query_row("SELECT COUNT(*) FROM edges", [], |row| row.get(0))?;
        Ok((entities as usize, edges as usize))
    }
}

/// Escape SQL `LIKE` metacharacters in a user-supplied substring.
///
/// SQLite `LIKE` treats `%` (any run) and `_` (any single char) as wildcards.
/// Without escaping, a search of `100%` becomes a wildcard scan, and a
/// search of `___` matches *everything* of length 3 — turning a substring
/// search into a degenerate full-table scan. Use together with
/// `LIKE ?n ESCAPE '\\'` in the SQL.
/// If `title`'s first line is a Conventional Commit `type(scope): …`, return the
/// canonical project for that scope. For such a memory the scope IS the project
/// it's work on; other project names in the subject/body are just description
/// and must not pull links.
fn commit_scope(title: &str) -> Option<String> {
    let head = title.lines().find(|l| !l.trim().is_empty())?.trim();
    let lower = head.to_lowercase();
    const TYPES: &[&str] = &[
        "feat", "fix", "docs", "style", "refactor", "perf", "test", "chore", "build", "ci",
        "revert", "security",
    ];
    let typ = TYPES.iter().find(|t| lower.starts_with(**t))?;
    let rest = &head[typ.len()..];
    let open = rest.find('(')?;
    let close = rest.find(')')?;
    if close <= open + 1 {
        return None;
    }
    // The scope group must be immediately followed by ':'.
    if !rest[close + 1..].trim_start().starts_with(':') {
        return None;
    }
    let scope = rest[open + 1..close].trim();
    if scope.is_empty() {
        return None;
    }
    Some(crate::semantic_attribution::canonical_project(scope))
}

/// True when `text` (already lowercased) names `project` as a whole word —
/// matching the name and its hyphen/space/joined variants, on word boundaries
/// so "project-beta" matches it as a whole word but not inside "project-betamax",
/// and hyphen/space variants ("project beta") also match.
/// True for memories that merely DISCUSS a project rather than being work on
/// it — user corrections and conversation-watcher captures. Backlinking those
/// would attribute discussion time to the project they happen to name.
pub(crate) fn is_meta_memory(memory_type: &str, tags: &str) -> bool {
    if memory_type.trim().eq_ignore_ascii_case("feedback") {
        return true;
    }
    let t = tags.to_lowercase();
    t.contains("conversation") || t.contains("correction")
}

fn project_name_in_text(text: &str, project: &str) -> bool {
    let base = project.trim().to_lowercase();
    if base.len() < 3 {
        return false;
    }
    let variants = [base.clone(), base.replace('-', " "), base.replace('-', "")];
    variants
        .iter()
        .filter(|v| v.len() >= 3)
        .any(|v| contains_word(text, v))
}

/// Whole-word (non-alphanumeric boundary) containment.
fn contains_word(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return false;
    }
    let mut from = 0;
    while let Some(rel) = haystack[from..].find(needle) {
        let i = from + rel;
        let before = haystack[..i].chars().next_back();
        let after = haystack[i + needle.len()..].chars().next();
        let left_ok = before.is_none_or(|c| !c.is_alphanumeric());
        let right_ok = after.is_none_or(|c| !c.is_alphanumeric());
        if left_ok && right_ok {
            return true;
        }
        from = i + needle.len();
    }
    false
}

fn escape_like(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '\\' | '%' | '_' => {
                out.push('\\');
                out.push(ch);
            }
            _ => out.push(ch),
        }
    }
    out
}

// === Graph Result Types ===

/// A project (project-type graph entity) with its memory count and
/// latest memories, for the widget's Projects page.
#[derive(Debug, Clone)]
pub struct ProjectOverview {
    pub key: String,
    pub name: String,
    pub mem_count: i64,
    pub mems: Vec<MemoryEntry>,
}

/// One embedded memory in a time window, with its hard-linked project keys —
/// the unit the semantic attribution engine reasons over.
#[derive(Debug, Clone)]
pub struct WindowMemory {
    pub id: String,
    pub title: String,
    pub content: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub linked_projects: Vec<String>,
    pub embedding: crate::embedding::Embedding,
}

#[derive(Debug, serde::Serialize)]
pub struct GraphResult {
    pub entity_name: String,
    pub entity_type: String,
    pub mention_count: i64,
    pub first_seen: String,
    pub last_seen: String,
    pub aliases: Vec<String>,
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
            aliases: Vec::new(),
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

/// One row from the temporal `facts` table. See the migration comment for
/// the table's purpose. Returned by `current_facts_for_subject`,
/// `facts_for_subject` (full history), and `latest_fact`.
///
/// `valid_to == None` means the fact is currently true. A non-None value
/// is the timestamp when it was superseded by a newer fact with the same
/// (subject, predicate).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct Fact {
    pub id: String,
    pub subject: String,
    pub predicate: String,
    pub value: String,
    pub valid_from: String,
    pub valid_to: Option<String>,
    pub confidence: f32,
    pub source_memory_id: String,
    pub created_at: String,
}

impl Fact {
    /// True if this fact is currently in effect (no superseding fact has
    /// been recorded yet).
    pub fn is_current(&self) -> bool {
        self.valid_to.is_none()
    }
}

/// Row mapper for the canonical `facts` projection used by every fact
/// helper. Lifted to a free function so each helper can pass it to
/// `query_row` / `query_map` without duplicating the column-by-column
/// destructure.
fn row_to_fact(row: &rusqlite::Row<'_>) -> rusqlite::Result<Fact> {
    Ok(Fact {
        id: row.get(0)?,
        subject: row.get(1)?,
        predicate: row.get(2)?,
        value: row.get(3)?,
        valid_from: row.get(4)?,
        valid_to: row.get(5)?,
        confidence: row.get(6)?,
        source_memory_id: row.get(7)?,
        created_at: row.get(8)?,
    })
}

/// A higher-level pattern induced from a cluster of memories. Sits one
/// layer above atomic `Fact`s — facts say "user uses_editor neovim",
/// conclusions say "user prefers low-overhead developer tooling".
///
/// `subject` is the canonical entity name or the sentinel "_global" for
/// non-entity-specific claims. `kind` is free-form ("pattern" |
/// "preference" | "trend" | "observation"). `support_count` is a
/// denormalized cache of conclusion_sources row count, maintained by
/// `add_conclusion`. `superseded_by == None` means the conclusion is
/// currently in effect.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct Conclusion {
    pub id: String,
    pub subject: String,
    pub kind: String,
    pub statement: String,
    pub confidence: f32,
    pub support_count: i64,
    pub created_at: String,
    pub last_evaluated_at: String,
    pub superseded_by: Option<String>,
}

impl Conclusion {
    /// True if this conclusion is currently in effect (no superseding
    /// conclusion has been recorded).
    #[allow(dead_code)]
    pub fn is_current(&self) -> bool {
        self.superseded_by.is_none()
    }
}

fn row_to_conclusion(row: &rusqlite::Row<'_>) -> rusqlite::Result<Conclusion> {
    Ok(Conclusion {
        id: row.get(0)?,
        subject: row.get(1)?,
        kind: row.get(2)?,
        statement: row.get(3)?,
        confidence: row.get(4)?,
        support_count: row.get(5)?,
        created_at: row.get(6)?,
        last_evaluated_at: row.get(7)?,
        superseded_by: row.get(8)?,
    })
}

/// First-class identity for an actor that participates in memories —
/// the user, an AI agent (Claude, Codex), a client, a teammate. See the
/// `peers` table migration for purpose and naming rules.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct Peer {
    pub id: String,
    /// Lowercased canonical name used for lookups. Always present.
    pub name: String,
    /// Original casing for display. None falls back to `name`.
    pub display_name: Option<String>,
    /// "human" | "agent" | "system". Free-form so future kinds (e.g.
    /// "tool", "service") don't require a schema change.
    pub kind: String,
    pub created_at: String,
    pub last_seen_at: String,
}

impl Peer {
    /// What to show in UI / CLI output. Falls back to `name` when
    /// `display_name` isn't set.
    pub fn label(&self) -> &str {
        self.display_name.as_deref().unwrap_or(&self.name)
    }
}

/// One leaderboard row from `longest_sessions` — a completed session with
/// its computed duration and the project that dominated it (if any).
#[derive(Debug, Clone, serde::Serialize)]
pub struct SessionRecordRow {
    pub session_id: String,
    pub started_at: String,
    pub duration_seconds: i64,
    pub top_project: Option<String>,
}

/// Parse a session timestamp string into UTC. Sessions opened by
/// the storage helpers use SQLite's `datetime('now')` which produces
/// `YYYY-MM-DD HH:MM:SS` (space delimiter, no timezone) and is
/// implicitly UTC. Sessions touched via `open_or_reuse_session_for_key`
/// use chrono's `to_rfc3339()`, producing the standard form with
/// timezone. This helper accepts both so duration math and date
/// extraction don't silently fall over when the live DB has mixed
/// timestamp formats — Codex caught this exact gap on the live DB
/// where `Duration:` was missing from summaries because the rows
/// were in SQLite format and only RFC3339 was being tried.
pub fn parse_session_timestamp(s: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    // RFC3339 first (newer rows + manual code paths via to_rfc3339).
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        return Some(dt.with_timezone(&chrono::Utc));
    }
    // SQLite default: "%Y-%m-%d %H:%M:%S" in UTC.
    if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S") {
        return Some(naive.and_utc());
    }
    // SQLite sub-second variant (rarer but possible after a UPDATE
    // with datetime('now', 'subsec')).
    if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S%.f") {
        return Some(naive.and_utc());
    }
    None
}

/// Render a non-negative second count the way session summaries do
/// ("42s" / "12m 3s" / "3h 40m"). Shared by dream's session summaries and
/// the session leaderboard endpoint so durations read the same everywhere.
pub fn human_duration(secs: i64) -> String {
    let secs = secs.max(0);
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else {
        format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
    }
}

/// A logical thread that groups memories from one continuous interaction
/// — a Claude Code JSONL session, a meeting, a workday. See the
/// `sessions` table migration for purpose.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct Session {
    pub id: String,
    pub peer_id: String,
    pub label: Option<String>,
    pub started_at: String,
    pub ended_at: Option<String>,
    /// Which watcher / system opened the session. Free-form string:
    /// "claude-code", "codex", "manual", "telegram", etc.
    pub source: String,
    /// Caller-provided stable identifier (e.g. JSONL path for the
    /// conversation watcher). Used by `open_or_reuse_session_for_key`
    /// to find the right existing session after a daemon restart.
    /// `None` for manually-opened sessions that don't need restart
    /// survival.
    #[serde(default)]
    pub external_key: Option<String>,
    /// Wall-clock timestamp of the most recent activity on this
    /// session. Persisted so idle-expiry checks survive restart.
    /// `None` only on very old rows that pre-date the migration AND
    /// somehow escaped the backfill (shouldn't happen in practice).
    #[serde(default)]
    pub last_activity_at: Option<String>,
}

impl Session {
    /// True if the session hasn't been closed yet (ended_at IS NULL).
    pub fn is_open(&self) -> bool {
        self.ended_at.is_none()
    }
}

fn row_to_peer(row: &rusqlite::Row<'_>) -> rusqlite::Result<Peer> {
    Ok(Peer {
        id: row.get(0)?,
        name: row.get(1)?,
        display_name: row.get(2)?,
        kind: row.get(3)?,
        created_at: row.get(4)?,
        last_seen_at: row.get(5)?,
    })
}

fn row_to_session(row: &rusqlite::Row<'_>) -> rusqlite::Result<Session> {
    Ok(Session {
        id: row.get(0)?,
        peer_id: row.get(1)?,
        label: row.get(2)?,
        started_at: row.get(3)?,
        ended_at: row.get(4)?,
        source: row.get(5)?,
        external_key: row.get(6)?,
        last_activity_at: row.get(7)?,
    })
}

/// Canonical column projection for `sessions` SELECTs that feed
/// `row_to_session`. Centralized as a const so every reader stays in
/// sync if the column set grows again — Codex's review caught that the
/// two new columns (external_key, last_activity_at) would have been
/// easy to forget at one of the four SELECT sites if each handler
/// inlined its own projection.
const SESSION_COLUMNS: &str =
    "id, peer_id, label, started_at, ended_at, source, external_key, last_activity_at";

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

/// Summary of an entity merge operation. Useful for dry-run reporting.
#[derive(Debug, Default, Clone)]
pub struct MergeReport {
    pub memory_links_redirected: usize,
    pub edges_redirected: usize,
    #[allow(dead_code)] // surfaced via Debug + reserved for future progress UI
    pub mentions_summed: usize,
    pub alias_dropped: bool,
}

/// One active project decision with its embedding — the unit the
/// contradiction lint compares pairwise. `timestamp` stays a raw RFC3339
/// string; the lint parses it for chronological ordering.
#[derive(Debug, Clone)]
pub struct DecisionRow {
    pub id: String,
    pub title: String,
    pub content: String,
    pub timestamp: String,
    pub embedding: Embedding,
}

/// Memory entry plus the bookkeeping needed to compute its effective score.
/// `last_active` is `last_accessed_at` if present, else `timestamp`.
#[derive(Debug, Clone)]
pub struct RankedEntry {
    pub entry: crate::event::MemoryEntry,
    pub access_count: u32,
    pub last_active: chrono::DateTime<chrono::Utc>,
}

impl RankedEntry {
    /// Compute effective importance using `crate::decay::effective_score`.
    pub fn effective(&self, now: chrono::DateTime<chrono::Utc>) -> f32 {
        crate::decay::effective_score(
            self.entry.importance,
            self.last_active,
            self.access_count,
            &self.entry.memory_type,
            now,
        )
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

#[cfg(unix)]
fn tighten_default_data_dir(path: &Path) {
    if path
        .file_name()
        .is_some_and(|name| name == std::ffi::OsStr::new(".mnemonic"))
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700));
    }
}

#[cfg(not(unix))]
fn tighten_default_data_dir(_path: &Path) {}

/// Trait for output sinks — extensible for Whisper (Phase 2), Obsidian, etc.
pub trait OutputSink: Send + Sync {
    fn write(&self, entry: &MemoryEntry) -> Result<()>;
    fn name(&self) -> &str;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{EventSource, MemoryType};
    use chrono::Utc;
    use std::sync::Arc;
    use std::thread;
    use std::time::{Duration, Instant};

    #[test]
    fn meta_memories_are_skipped() {
        // User corrections + conversation captures only discuss a project.
        assert!(is_meta_memory("feedback", "[\"feedback\",\"correction\"]"));
        assert!(is_meta_memory(
            "decision",
            "[\"decision\",\"conversation\"]"
        ));
        // Real work memories pass.
        assert!(!is_meta_memory("note", "[\"mnemonic\",\"attribution\"]"));
        assert!(!is_meta_memory("decision", "[\"feature\",\"mnemonic\"]"));
    }

    #[test]
    fn replace_graph_and_reconcile_strips_meta_project_links_and_edges() {
        use crate::graph::{Edge, Entity, EntityType};

        let storage = Storage::open(&tmp_db()).unwrap();
        let mut mem = make_entry("User correction");
        mem.memory_type = MemoryType::Feedback;
        mem.tags = vec!["feedback".into(), "correction".into()];
        storage.save(&mem).unwrap();

        let entities = vec![
            Entity {
                name: "project-alpha".into(),
                entity_type: EntityType::Project,
            },
            Entity {
                name: "grok".into(),
                entity_type: EntityType::Concept,
            },
        ];
        let edges = vec![Edge {
            source: "project-alpha".into(),
            target: "grok".into(),
            relation: "uses".into(),
            memory_id: mem.id.clone(),
        }];

        storage
            .replace_graph_and_reconcile_projects(&mem, &entities, &edges)
            .unwrap();

        let (project_links, project_edges): (i64, i64) = {
            let conn = storage.conn.lock().unwrap();
            let links = conn
                .query_row(
                    "SELECT COUNT(*)
                     FROM memory_entities me
                     JOIN entities e ON e.id = me.entity_id
                     WHERE me.memory_id = ?1 AND e.entity_type = 'project'",
                    params![&mem.id],
                    |row| row.get(0),
                )
                .unwrap();
            let edges = conn
                .query_row(
                    "SELECT COUNT(*) FROM edges
                     WHERE memory_id = ?1
                       AND (source_entity = 'project-alpha'
                            OR target_entity = 'project-alpha')",
                    params![&mem.id],
                    |row| row.get(0),
                )
                .unwrap();
            (links, edges)
        };

        assert_eq!(project_links, 0, "meta memories keep no project links");
        assert_eq!(project_edges, 0, "meta memories keep no project edges");
        assert_eq!(
            storage.graph_query("project-alpha").unwrap().mention_count,
            0,
            "stripping the project link must decrement mention_count"
        );
    }

    #[cfg(unix)]
    #[test]
    fn open_tightens_default_mnemonic_dir_and_db_file() {
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!("mnemonic-data-{}", uuid::Uuid::new_v4()));
        let dir = root.join(".mnemonic");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        let db = dir.join("memory.db");

        let _storage = Storage::open(&db).unwrap();

        let dir_mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        let db_mode = std::fs::metadata(&db).unwrap().permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o700);
        assert_eq!(db_mode, 0o600);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn commit_scope_pins_to_scope_project() {
        // Scope is the project — other mentions in the subject are ignored.
        assert_eq!(
            commit_scope("fix(mnemonic): backlink aliases (project-gamma → project-alpha)")
                .as_deref(),
            Some("mnemonic")
        );
        // Scope aliases canonicalize.
        assert_eq!(
            commit_scope("feat(rendergen): batch render").as_deref(),
            Some("project-alpha")
        );
        // No scope / not a commit → None (caller alias-backlinks instead).
        assert_eq!(commit_scope("fix: tidy"), None);
        assert_eq!(
            commit_scope("project-gamma dashboard deployed on Vercel"),
            None
        );
        assert_eq!(commit_scope("project-beta Gmail send — cron"), None);
    }

    #[test]
    fn project_name_matches_on_word_boundary() {
        // Names the project → match.
        assert!(project_name_in_text(
            "project-beta gmail send — cron",
            "project-beta"
        ));
        assert!(project_name_in_text(
            "project-forge site overhaul deployed",
            "project-forge"
        ));
        // Hyphen/space variants: "project-alpha" also matches "project alpha".
        assert!(project_name_in_text(
            "notes on project alpha batch",
            "project-alpha"
        ));
        // Substring but not a word → no false positive.
        assert!(!project_name_in_text(
            "project-betamax recorder review",
            "project-beta"
        ));
        // Unrelated.
        assert!(!project_name_in_text("fix the door hinge", "project-beta"));
        // Too-short names are ignored.
        assert!(!project_name_in_text("a b c", "ab"));
    }

    #[test]
    fn escape_like_escapes_metachars() {
        assert_eq!(escape_like("plain"), "plain");
        assert_eq!(escape_like("100%"), r"100\%");
        assert_eq!(escape_like("foo_bar"), r"foo\_bar");
        assert_eq!(escape_like(r"a\b"), r"a\\b");
        assert_eq!(escape_like("___"), r"\_\_\_");
        // Mixed: literal non-ASCII + wildcard should preserve text, escape only `%`.
        assert_eq!(escape_like("Анна 50%"), r"Анна 50\%");
    }

    #[test]
    fn pending_enqueue_is_idempotent() {
        let storage = Storage::open(&tmp_db()).unwrap();
        let entry = make_entry("foo");
        storage.save(&entry).unwrap();

        storage
            .enqueue_pending_extraction(&entry.id, "backend: connection refused")
            .unwrap();
        let row1 = storage.pending_row(&entry.id).unwrap().unwrap();
        assert_eq!(row1.0, 0, "first enqueue → attempts=0");

        // Re-enqueue (e.g. extractor ran again, still failed at save-time).
        // last_error refreshes but attempts/next_attempt stay put — the
        // backoff schedule should be controlled by the *retry* path, not by
        // how many times we slammed enqueue.
        storage
            .enqueue_pending_extraction(&entry.id, "backend: timeout")
            .unwrap();
        let row2 = storage.pending_row(&entry.id).unwrap().unwrap();
        assert_eq!(row2.0, 0, "re-enqueue must not bump attempts");
        assert_eq!(row2.1.as_deref(), Some("backend: timeout"));
    }

    #[test]
    fn pending_mark_attempted_drops_on_sixth_attempt() {
        let storage = Storage::open(&tmp_db()).unwrap();
        let entry = make_entry("foo");
        storage.save(&entry).unwrap();
        storage
            .enqueue_pending_extraction(&entry.id, "first failure")
            .unwrap();

        // 5 attempts allowed → returns true.
        for i in 1..=5 {
            let kept = storage
                .mark_pending_attempted(&entry.id, &format!("attempt {i}"))
                .unwrap();
            assert!(kept, "attempt {i} should keep the row");
        }
        // 6th attempt → row dropped, returns false.
        let kept = storage
            .mark_pending_attempted(&entry.id, "attempt 6")
            .unwrap();
        assert!(!kept, "6th attempt must drop the row");
        assert!(storage.pending_row(&entry.id).unwrap().is_none());
    }

    #[test]
    fn pending_due_returns_only_past_next_attempt() {
        let storage = Storage::open(&tmp_db()).unwrap();
        let entry = make_entry("foo");
        storage.save(&entry).unwrap();
        storage
            .enqueue_pending_extraction(&entry.id, "err")
            .unwrap();

        // Initial enqueue → next_attempt_at = now + 60s. Not due yet.
        let due_now = storage.pending_due_for_retry(10).unwrap();
        assert!(
            !due_now.contains(&entry.id),
            "freshly enqueued row should NOT be due immediately"
        );

        // Force next_attempt into the past.
        {
            let conn = storage.conn.lock().unwrap();
            conn.execute(
                "UPDATE pending_extractions SET next_attempt_at = datetime('now','-1 minute')
                 WHERE memory_id = ?1",
                params![entry.id],
            )
            .unwrap();
        }
        let due = storage.pending_due_for_retry(10).unwrap();
        assert!(due.contains(&entry.id), "row past next_attempt must be due");
    }

    #[test]
    fn pending_drop_clears_the_row() {
        let storage = Storage::open(&tmp_db()).unwrap();
        let entry = make_entry("foo");
        storage.save(&entry).unwrap();
        storage
            .enqueue_pending_extraction(&entry.id, "err")
            .unwrap();
        assert_eq!(storage.pending_extractions_count().unwrap(), 1);

        storage.drop_pending_extraction(&entry.id).unwrap();
        assert_eq!(storage.pending_extractions_count().unwrap(), 0);
        assert!(storage.pending_row(&entry.id).unwrap().is_none());
    }

    #[test]
    fn dedup_estimate_counts_memory_embedding_column() {
        let storage = Storage::open(&tmp_db()).unwrap();
        let with_embedding = make_entry("embedded");
        let without_embedding = make_entry("plain");
        let emb = vec![1.0_f32, 0.0, 0.0, 0.0];

        storage
            .save_with_embedding(&with_embedding, Some(&emb))
            .unwrap();
        storage.save(&without_embedding).unwrap();

        assert_eq!(storage.dedup_estimate().unwrap(), (2, 1));
    }

    /// Searching for "100%" must not match unrelated rows just because `%`
    /// is a wildcard.
    #[test]
    fn memory_graph_search_does_not_wildcard_on_percent() {
        let storage = Storage::open(&tmp_db()).unwrap();
        let mut a = make_entry("budget hit 100% of plan");
        a.title = "budget".into();
        let mut b = make_entry("totally unrelated content about cats");
        b.title = "cats".into();
        storage.save(&a).unwrap();
        storage.save(&b).unwrap();

        let hits = storage
            .memory_graph_nodes(50, None, None, Some("100%"))
            .unwrap();
        let ids: Vec<&str> = hits.iter().map(|(m, _)| m.id.as_str()).collect();
        assert!(
            ids.contains(&a.id.as_str()),
            "should match the literal 100%"
        );
        assert!(
            !ids.contains(&b.id.as_str()),
            "must NOT match unrelated row via wildcard"
        );
    }

    /// UUID-suffixed temp path so parallel tests can't collide on the
    /// same nanosecond bucket. `SystemTime::now().as_nanos()` resolution
    /// on macOS is coarser than `cargo test` scheduling, which produced
    /// flaky failures in extraction_worker tests on adjacent runs.
    fn tmp_db() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("mnemonic-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("memory.db")
    }

    fn make_entry(title: &str) -> MemoryEntry {
        MemoryEntry {
            id: uuid::Uuid::new_v4().to_string(),
            timestamp: Utc::now(),
            memory_type: MemoryType::Note,
            title: title.to_string(),
            content: format!("test content for {title}"),
            tags: vec!["test".to_string()],
            source: EventSource::Manual,
            importance: 0.5,
            metadata: serde_json::Value::Null,
        }
    }

    /// Regression test for the 2026-04-16 hang bug.
    /// Before WAL + busy_timeout, a CLI process opening the DB while the
    /// daemon held a write lock would hang indefinitely in UE state.
    #[test]
    fn pragma_wal_and_busy_timeout_are_set() {
        let path = tmp_db();
        let storage = Storage::open(&path).unwrap();

        let conn = storage.conn.lock().unwrap();
        let journal_mode: String = conn
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        let busy_timeout: i64 = conn
            .query_row("PRAGMA busy_timeout", [], |r| r.get(0))
            .unwrap();

        let wal_autocheckpoint: i64 = conn
            .query_row("PRAGMA wal_autocheckpoint", [], |r| r.get(0))
            .unwrap();

        assert_eq!(
            journal_mode.to_lowercase(),
            "wal",
            "WAL mode must be enabled"
        );
        assert!(
            busy_timeout >= 1000,
            "busy_timeout must be at least 1s (got {busy_timeout}ms)"
        );
        assert!(
            wal_autocheckpoint > 0,
            "wal_autocheckpoint must be enabled to prevent WAL bloat (got {wal_autocheckpoint})"
        );
    }

    /// Storage::open should create memory.db.bak next to the main DB.
    /// Refresh logic should skip if the backup is less than 24h old.
    #[test]
    fn backup_created_on_open_and_skipped_when_fresh() {
        let path = tmp_db();
        let bak_path = {
            let mut p = path.clone();
            p.set_file_name(format!("{}.bak", p.file_name().unwrap().to_string_lossy()));
            p
        };

        // First open creates the backup.
        {
            let s = Storage::open(&path).unwrap();
            s.save(&make_entry("first")).unwrap();
        }
        assert!(bak_path.exists(), "backup should exist after Storage::open");
        let first_mtime = std::fs::metadata(&bak_path).unwrap().modified().unwrap();

        // Second open within 24h must NOT touch the existing backup.
        std::thread::sleep(std::time::Duration::from_millis(50));
        {
            let s = Storage::open(&path).unwrap();
            s.save(&make_entry("second")).unwrap();
        }
        let second_mtime = std::fs::metadata(&bak_path).unwrap().modified().unwrap();
        assert_eq!(
            first_mtime, second_mtime,
            "fresh backup (<24h) must not be overwritten"
        );
    }

    /// Concurrent writers must not deadlock. Before WAL, two processes writing
    /// at the same time with busy_timeout=0 would block one of them forever.
    #[test]
    fn concurrent_writes_do_not_hang() {
        let path = Arc::new(tmp_db());
        let s1 = Arc::new(Storage::open(&path).unwrap());
        let s2 = Arc::new(Storage::open(&path).unwrap());

        let start = Instant::now();
        let t1 = {
            let s = s1.clone();
            thread::spawn(move || {
                for i in 0..20 {
                    s.save(&make_entry(&format!("from-s1-{i}"))).unwrap();
                }
            })
        };
        let t2 = {
            let s = s2.clone();
            thread::spawn(move || {
                for i in 0..20 {
                    s.save(&make_entry(&format!("from-s2-{i}"))).unwrap();
                }
            })
        };

        t1.join().unwrap();
        t2.join().unwrap();

        // If this takes more than 10s, WAL/busy_timeout is broken.
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "concurrent writes took too long — possible lock regression"
        );

        let total = s1.count().unwrap();
        assert_eq!(total, 40, "both writers should have committed all entries");
    }

    /// Apply reflection must inherit graph links: the canonical memory
    /// gains entries in memory_entities for every entity any source was
    /// linked to (Codex P1b).
    #[test]
    fn apply_reflection_inherits_source_entity_links() {
        use crate::graph::{Entity, EntityType};
        let path = tmp_db();
        let storage = Storage::open(&path).unwrap();

        // Two near-duplicate sources, each linked to an entity.
        let s1 = make_entry("source-1");
        let s2 = make_entry("source-2");
        storage.save(&s1).unwrap();
        storage.save(&s2).unwrap();
        storage
            .save_graph(
                &s1.id,
                &[Entity {
                    name: "inventory-labeler".into(),
                    entity_type: EntityType::Project,
                }],
                &[],
            )
            .unwrap();
        storage
            .save_graph(
                &s2.id,
                &[Entity {
                    name: "acme-devices".into(),
                    entity_type: EntityType::Person,
                }],
                &[],
            )
            .unwrap();

        let run_id = storage.begin_reflection_run("apply", 0.9, "rule").unwrap();
        let canonical = make_entry("canonical");
        let cluster = vec![(s1.id.clone(), 0.99), (s2.id.clone(), 0.97)];
        storage
            .apply_reflection(&run_id, &canonical, None, &cluster)
            .unwrap();

        // Canonical must now appear under BOTH entities.
        let g1 = storage.graph_query("inventory-labeler").unwrap();
        let g2 = storage.graph_query("acme-devices").unwrap();
        assert!(g1.memories.iter().any(|m| m.id == canonical.id));
        assert!(g2.memories.iter().any(|m| m.id == canonical.id));
    }

    /// Cleanup must not delete memories that are reflection sources or
    /// canonicals (Codex P2b). "Sources are NEVER deleted" is part of
    /// the reflection contract.
    #[test]
    fn cleanup_preserves_reflection_sources() {
        let path = tmp_db();
        let storage = Storage::open(&path).unwrap();

        let mut s1 = make_entry("old low-importance note");
        s1.importance = 0.1;
        s1.timestamp = Utc::now() - chrono::Duration::days(180);
        storage.save(&s1).unwrap();
        let mut s2 = make_entry("another");
        s2.importance = 0.1;
        s2.timestamp = Utc::now() - chrono::Duration::days(180);
        storage.save(&s2).unwrap();

        let run_id = storage.begin_reflection_run("apply", 0.9, "rule").unwrap();
        let canonical = make_entry("canon");
        let cluster = vec![(s1.id.clone(), 0.99), (s2.id.clone(), 0.97)];
        storage
            .apply_reflection(&run_id, &canonical, None, &cluster)
            .unwrap();

        // Aggressive cleanup that would normally nuke both sources.
        let deleted = storage.cleanup(30, 0.5).unwrap();
        assert_eq!(deleted, 0, "no rows must be deleted while protected");
        assert!(storage.get_by_id(&s1.id).unwrap().is_some());
        assert!(storage.get_by_id(&s2.id).unwrap().is_some());
        assert!(storage.get_by_id(&canonical.id).unwrap().is_some());
    }

    /// merge_entities reassigns edges + memory_entities and drops the
    /// alias row. Sum mention_count goes to canonical.
    #[test]
    fn merge_entities_reassigns_and_drops_alias() {
        use crate::graph::{Edge, Entity, EntityType};

        let path = tmp_db();
        let storage = Storage::open(&path).unwrap();
        let mem = make_entry("m1");
        storage.save(&mem).unwrap();

        // Create two entities and an edge between them.
        let canonical = Entity {
            name: "acme-devices".into(),
            entity_type: EntityType::Person,
        };
        let alias = Entity {
            name: "acme-devices-co".into(),
            entity_type: EntityType::Person,
        };
        storage
            .save_graph(
                &mem.id,
                &[canonical.clone(), alias.clone()],
                &[Edge {
                    source: "inventory-labeler".into(),
                    target: alias.name.clone(),
                    relation: "owns".into(),
                    memory_id: mem.id.clone(),
                }],
            )
            .unwrap();

        let report = storage
            .merge_entities(&canonical.name, &alias.name)
            .unwrap();
        assert!(report.alias_dropped);
        // edges_redirected may be 0 if UPDATE OR IGNORE skipped — but the
        // edge SHOULD now point to canonical. Verify by query.
        let g = storage.graph_query(&canonical.name).unwrap();
        assert!(g.found);
        let to_canonical = g
            .edges
            .iter()
            .any(|e| e.target.eq_ignore_ascii_case(&canonical.name));
        let stale_alias = g
            .edges
            .iter()
            .any(|e| e.target == alias.name || e.source == alias.name);
        assert!(
            to_canonical,
            "edge should now target canonical: {:?}",
            g.edges
        );
        assert!(
            !stale_alias,
            "no edge should still reference alias: {:?}",
            g.edges
        );

        // Alias entity must be gone.
        let alias_q = storage.graph_query(&alias.name).unwrap();
        assert!(!alias_q.found, "alias entity should be deleted");
    }

    // ─────────────── temporal facts ───────────────

    /// Core supersede semantics: a new fact with the same (subject,
    /// predicate) must close out the previous current fact AND become
    /// the new current. The full chain stays queryable.
    #[test]
    fn facts_supersede_previous_current_on_add() {
        let storage = Storage::open(&tmp_db()).unwrap();
        let mem1 = make_entry("m1");
        let mem2 = make_entry("m2");
        storage.save(&mem1).unwrap();
        storage.save(&mem2).unwrap();

        // First assertion.
        storage
            .add_fact(
                "inventory-labeler",
                "has-price",
                "$2k flat",
                &mem1.id,
                1.0,
                Some("2026-05-01T00:00:00Z"),
            )
            .unwrap();
        let current_after_first = storage
            .latest_fact("inventory-labeler", "has-price")
            .unwrap()
            .expect("first add must have a current fact");
        assert_eq!(current_after_first.value, "$2k flat");
        assert!(current_after_first.is_current());

        // Second assertion supersedes the first.
        storage
            .add_fact(
                "inventory-labeler",
                "has-price",
                "$6k phase 1 + $1.5k scoping",
                &mem2.id,
                1.0,
                Some("2026-05-09T00:00:00Z"),
            )
            .unwrap();

        // latest_fact returns the new one only.
        let current = storage
            .latest_fact("inventory-labeler", "has-price")
            .unwrap()
            .expect("post-supersede should have one current fact");
        assert!(current.value.starts_with("$6k"));
        assert!(current.is_current());

        // current_facts returns exactly one.
        let currents = storage
            .current_facts_for_subject("inventory-labeler")
            .unwrap();
        assert_eq!(currents.len(), 1, "exactly one current fact per predicate");

        // facts_for_subject returns the full chain — newest first.
        let history = storage.facts_for_subject("inventory-labeler").unwrap();
        assert_eq!(history.len(), 2);
        assert!(history[0].value.starts_with("$6k"));
        assert!(history[0].is_current());
        assert_eq!(history[1].value, "$2k flat");
        assert!(
            !history[1].is_current(),
            "old fact must be marked superseded"
        );
        assert_eq!(
            history[1].valid_to.as_deref(),
            Some("2026-05-09T00:00:00Z"),
            "old fact's valid_to must equal new fact's valid_from"
        );
    }

    /// Distinct predicates for the same subject DON'T supersede each
    /// other — both remain current.
    #[test]
    fn facts_different_predicates_coexist_for_same_subject() {
        let storage = Storage::open(&tmp_db()).unwrap();
        let mem = make_entry("m1");
        storage.save(&mem).unwrap();

        storage
            .add_fact("project-x", "has-price", "$5k", &mem.id, 1.0, None)
            .unwrap();
        storage
            .add_fact("project-x", "deadline", "2026-08-01", &mem.id, 1.0, None)
            .unwrap();

        let currents = storage.current_facts_for_subject("project-x").unwrap();
        assert_eq!(currents.len(), 2);
        assert!(currents.iter().all(|f| f.is_current()));
        let predicates: std::collections::HashSet<&str> =
            currents.iter().map(|f| f.predicate.as_str()).collect();
        assert!(predicates.contains("has-price"));
        assert!(predicates.contains("deadline"));
    }

    /// Subject + predicate get lowercased on insert so case-different
    /// queries find the same fact.
    #[test]
    fn facts_subject_and_predicate_are_lowercased() {
        let storage = Storage::open(&tmp_db()).unwrap();
        let mem = make_entry("m1");
        storage.save(&mem).unwrap();

        storage
            .add_fact("MixedCase", "Has-Price", "$1", &mem.id, 1.0, None)
            .unwrap();
        // Query with any casing finds it.
        let f1 = storage.latest_fact("mixedcase", "has-price").unwrap();
        let f2 = storage.latest_fact("MIXEDCASE", "HAS-PRICE").unwrap();
        let f3 = storage.latest_fact("MixedCase", "Has-Price").unwrap();
        assert!(f1.is_some());
        assert_eq!(f1, f2);
        assert_eq!(f1, f3);
    }

    /// DB-level UNIQUE invariant: even if a bug ever lets two current
    /// rows land for the same (subject, predicate), the partial UNIQUE
    /// index rejects it. We bypass add_fact and INSERT raw to confirm
    /// the index does its job — add_fact's own supersede transaction is
    /// already covered by `facts_supersede_previous_current_on_add`,
    /// this test pins the schema-level safety net.
    #[test]
    fn facts_unique_index_blocks_two_current_per_predicate() {
        let storage = Storage::open(&tmp_db()).unwrap();
        let mem = make_entry("m1");
        storage.save(&mem).unwrap();

        // First current fact: legitimate via add_fact.
        storage
            .add_fact("subj", "pred", "v1", &mem.id, 1.0, None)
            .unwrap();

        // Try to insert ANOTHER current fact (valid_to NULL) for the same
        // (subject, predicate) directly via raw SQL, bypassing the
        // supersede transaction. The partial UNIQUE index should refuse.
        let conn = storage.conn.lock().unwrap();
        let raw_insert = conn.execute(
            "INSERT INTO facts (id, subject, predicate, value, valid_from,
                                valid_to, confidence, source_memory_id)
             VALUES (?1, 'subj', 'pred', 'v2', datetime('now'), NULL, 1.0, ?2)",
            params![uuid::Uuid::new_v4().to_string(), mem.id],
        );
        assert!(
            raw_insert.is_err(),
            "raw insert of a duplicate current fact must fail the UNIQUE \
             partial index, got Ok({raw_insert:?})"
        );
    }

    // ─────────────── peers & sessions ───────────────

    /// Regression: legacy DBs created during the brief window when
    /// auto-tag wrote `addressee` for agent peers must have those rows
    /// rewritten to `participant` so role-based queries see one
    /// semantic. Idempotent — re-running init on a clean DB is a no-op.
    /// Collisions (memory+peer already has `participant` from new code)
    /// silently merge via INSERT OR IGNORE; old row is then deleted.
    #[test]
    fn migrate_addressee_to_participant_rewrites_legacy_rows() {
        let storage = Storage::open(&tmp_db()).unwrap();
        let mem = make_entry("m1");
        storage.save(&mem).unwrap();
        let agent = storage.upsert_peer("claude", None, "agent").unwrap();

        // Inject a legacy `addressee` row via the raw conn (mirrors a
        // DB written by the older auto-tag code).
        {
            let conn = storage.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO memory_peers (memory_id, peer_id, role)
                 VALUES (?1, ?2, 'addressee')",
                params![mem.id, agent],
            )
            .unwrap();
        }

        // Sanity: the row is there with the old role.
        let before = storage.peers_for_memory(&mem.id).unwrap();
        assert!(before.iter().any(|(_, r)| r == "addressee"));

        // Trigger the migration by re-opening the same DB path.
        let path: std::path::PathBuf = {
            let conn = storage.conn.lock().unwrap();
            conn.path().expect("disk-backed DB").into()
        };
        drop(storage);
        let storage2 = Storage::open(&path).unwrap();

        // After re-open: no addressee rows, one participant row instead.
        let after = storage2.peers_for_memory(&mem.id).unwrap();
        assert!(
            !after.iter().any(|(_, r)| r == "addressee"),
            "addressee rows must be gone after migration, got {after:?}"
        );
        assert!(
            after.iter().any(|(_, r)| r == "participant"),
            "migration must produce a participant row, got {after:?}"
        );
    }

    /// Scope check: migration must NOT rewrite `addressee` rows when
    /// the peer is non-agent (human / system / etc). Codex flagged
    /// that `addressee` is a legitimate hand-applied role for human
    /// peers — e.g. "Claude said X to User" with User as addressee.
    /// The migration's purpose is cleanup of the old auto-tag's
    /// agent-side artifact only.
    #[test]
    fn migrate_addressee_to_participant_leaves_human_peers_alone() {
        let storage = Storage::open(&tmp_db()).unwrap();
        let mem = make_entry("m1");
        storage.save(&mem).unwrap();
        let human = storage.upsert_peer("alice", None, "human").unwrap();

        // Manually attach an addressee row to a HUMAN peer — a real
        // use-case for the role.
        storage
            .link_memory_peer(&mem.id, &human, "addressee")
            .unwrap();
        assert!(
            storage
                .peers_for_memory(&mem.id)
                .unwrap()
                .iter()
                .any(|(_, r)| r == "addressee"),
            "test setup: addressee row must exist before migration"
        );

        // Re-open to trigger migration.
        let path: std::path::PathBuf = {
            let conn = storage.conn.lock().unwrap();
            conn.path().expect("disk-backed DB").into()
        };
        drop(storage);
        let storage2 = Storage::open(&path).unwrap();

        // Human-peer addressee must SURVIVE the migration.
        let after = storage2.peers_for_memory(&mem.id).unwrap();
        assert!(
            after
                .iter()
                .any(|(p, r)| p.name == "alice" && r == "addressee"),
            "human-peer addressee row was wrongly rewritten: {after:?}"
        );
        assert!(
            !after.iter().any(|(_, r)| r == "participant"),
            "no participant rows should appear for human-only addressee: {after:?}"
        );
    }

    /// Migration is idempotent: collision when (memory, peer) already
    /// has a `participant` row from the new code path AND a leftover
    /// `addressee` from the old. INSERT OR IGNORE keeps participant,
    /// DELETE drops the addressee, no PK error.
    #[test]
    fn migrate_addressee_to_participant_handles_collisions() {
        let storage = Storage::open(&tmp_db()).unwrap();
        let mem = make_entry("m1");
        storage.save(&mem).unwrap();
        let agent = storage.upsert_peer("claude", None, "agent").unwrap();

        // BOTH roles on same (memory, peer) — the legacy addressee row
        // from old auto-tag, AND a participant row from a later new-code
        // save of the same memory id.
        storage
            .link_memory_peer(&mem.id, &agent, "participant")
            .unwrap();
        {
            let conn = storage.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO memory_peers (memory_id, peer_id, role)
                 VALUES (?1, ?2, 'addressee')",
                params![mem.id, agent],
            )
            .unwrap();
        }

        let path: std::path::PathBuf = {
            let conn = storage.conn.lock().unwrap();
            conn.path().expect("disk-backed DB").into()
        };
        drop(storage);
        let storage2 = Storage::open(&path).unwrap();

        let after = storage2.peers_for_memory(&mem.id).unwrap();
        assert_eq!(
            after.len(),
            1,
            "collision must collapse to one row, got {after:?}"
        );
        assert_eq!(after[0].1, "participant");
    }

    /// Regression for live-DB blocker: databases created BEFORE the FK
    /// clauses landed in the schema have a `memory_peers` table without
    /// FKs. `CREATE TABLE IF NOT EXISTS` doesn't retrofit. The migration
    /// in `init_schema` must rebuild the table so existing installs get
    /// FK enforcement on the next daemon start.
    ///
    /// Simulates the legacy state by opening a raw connection, manually
    /// creating memory_peers without FKs, inserting an orphan row, then
    /// closing and re-opening via Storage::open. Post-reopen we assert:
    /// memory_peers has FK constraints, the orphan row was dropped
    /// during migration, and attempts to insert new orphan rows now
    /// fail at the FK level.
    #[test]
    fn migrate_memory_peers_retrofits_foreign_keys_on_legacy_db() {
        use rusqlite::Connection;
        let path = tmp_db();

        // --- Simulate a legacy DB ---
        // Open a raw connection, build the OLD memory_peers schema (no FKs),
        // and insert an orphan row that the new schema would reject.
        {
            let conn = Connection::open(&path).unwrap();
            conn.pragma_update(None, "foreign_keys", "OFF").unwrap();
            // Minimal subset of the surrounding schema so the test rebuild
            // has tables to reference. memories + peers must exist for
            // the new memory_peers FKs to validate.
            conn.execute_batch(
                "CREATE TABLE memories (
                    id TEXT PRIMARY KEY,
                    timestamp TEXT NOT NULL,
                    title TEXT NOT NULL,
                    content TEXT NOT NULL,
                    memory_type TEXT NOT NULL,
                    tags TEXT NOT NULL,
                    source TEXT NOT NULL,
                    importance REAL NOT NULL,
                    metadata TEXT NOT NULL
                );
                CREATE TABLE peers (
                    id TEXT PRIMARY KEY,
                    name TEXT NOT NULL UNIQUE,
                    display_name TEXT,
                    kind TEXT NOT NULL,
                    created_at TEXT NOT NULL DEFAULT (datetime('now')),
                    last_seen_at TEXT NOT NULL DEFAULT (datetime('now'))
                );
                -- LEGACY memory_peers: no FK constraints.
                CREATE TABLE memory_peers (
                    memory_id TEXT NOT NULL,
                    peer_id TEXT NOT NULL,
                    role TEXT NOT NULL DEFAULT 'speaker',
                    PRIMARY KEY (memory_id, peer_id, role)
                );",
            )
            .unwrap();
            // Seed one VALID link (memory + peer both exist) and one ORPHAN
            // (references ids that don't).
            conn.execute(
                "INSERT INTO memories (id, timestamp, title, content, memory_type,
                    tags, source, importance, metadata)
                 VALUES ('mem-real', datetime('now'), 'real', '', 'note',
                    '[]', '\"Manual\"', 0.5, 'null')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO peers (id, name, kind)
                 VALUES ('peer-real', 'user', 'human')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO memory_peers (memory_id, peer_id, role)
                 VALUES ('mem-real', 'peer-real', 'speaker')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO memory_peers (memory_id, peer_id, role)
                 VALUES ('orphan-memory-id', 'orphan-peer-id', 'speaker')",
                [],
            )
            .unwrap();

            // Confirm: no FKs in this legacy state.
            let mut stmt = conn
                .prepare("PRAGMA foreign_key_list(memory_peers)")
                .unwrap();
            let fk_count = stmt
                .query_map([], |_| Ok(()))
                .unwrap()
                .filter_map(|r| r.ok())
                .count();
            assert_eq!(fk_count, 0, "test setup: legacy table must have no FKs");
        }

        // --- Run the migration via Storage::open ---
        let storage = Storage::open(&path).unwrap();

        // 1. FK constraints are now declared.
        {
            let conn = storage.conn.lock().unwrap();
            let mut stmt = conn
                .prepare("PRAGMA foreign_key_list(memory_peers)")
                .unwrap();
            let fk_count = stmt
                .query_map([], |_| Ok(()))
                .unwrap()
                .filter_map(|r| r.ok())
                .count();
            assert_eq!(
                fk_count, 2,
                "post-migration memory_peers must have 2 FKs (memory_id + peer_id), got {fk_count}"
            );

            // 2. Orphan row was dropped; valid row survives.
            let n_real: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM memory_peers
                      WHERE memory_id = 'mem-real' AND peer_id = 'peer-real'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            let n_orphan: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM memory_peers
                      WHERE memory_id = 'orphan-memory-id'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(n_real, 1, "valid row must survive migration");
            assert_eq!(n_orphan, 0, "orphan row must be dropped by migration");
        }

        // 3. New orphan inserts are rejected by enforced FKs.
        let bad_link = storage.link_memory_peer("some-bogus-id", "another-bogus-id", "speaker");
        assert!(
            bad_link.is_err(),
            "post-migration FK enforcement must reject orphan links, got Ok"
        );

        // 4. Migration is idempotent — re-opening doesn't break anything.
        let _storage2 = Storage::open(&path).unwrap();
    }

    /// Regression: SQLite ignores FK constraints unless
    /// `PRAGMA foreign_keys = ON`. Storage::open() sets it; this test
    /// pins the behavior so a future refactor that drops the pragma
    /// fails loudly instead of silently creating orphan rows.
    #[test]
    fn foreign_keys_pragma_is_enforced() {
        let storage = Storage::open(&tmp_db()).unwrap();

        // Direct check: PRAGMA foreign_keys should report 1 (ON).
        {
            let conn = storage.conn.lock().unwrap();
            let on: i64 = conn
                .query_row("PRAGMA foreign_keys", [], |r| r.get(0))
                .unwrap();
            assert_eq!(on, 1, "foreign_keys pragma must be ON, got {on}");
        }

        // Behavioral check #1: session with bogus peer_id is rejected.
        let bogus_session = storage.open_session("no-such-peer", Some("x"), "test");
        assert!(
            bogus_session.is_err(),
            "open_session with bogus peer_id must fail, got Ok"
        );

        // Behavioral check #2: memory_peers with bogus FKs is rejected.
        let mem = make_entry("m1");
        storage.save(&mem).unwrap();
        let real_peer = storage.upsert_peer("user", None, "human").unwrap();

        // Bogus memory_id — link should fail at the FK.
        let r1 = storage.link_memory_peer("no-such-memory", &real_peer, "speaker");
        assert!(r1.is_err(), "link with bogus memory_id must fail");

        // Bogus peer_id — same.
        let r2 = storage.link_memory_peer(&mem.id, "no-such-peer", "speaker");
        assert!(r2.is_err(), "link with bogus peer_id must fail");

        // Sanity: a fully-valid link still works.
        storage
            .link_memory_peer(&mem.id, &real_peer, "speaker")
            .expect("valid link must succeed");
    }

    /// upsert_peer should fill in an empty display_name on a later call,
    /// not just touch last_seen_at. Previously the conversation watcher
    /// would create a peer with no display, then an explicit
    /// `mnemonic peer add --display X` wouldn't update it.
    #[test]
    fn upsert_peer_backfills_empty_display_name() {
        let storage = Storage::open(&tmp_db()).unwrap();
        // First call — no display_name (simulates watcher auto-tagging).
        let id1 = storage.upsert_peer("claude", None, "agent").unwrap();
        let after_first = storage.peer_by_name("claude").unwrap().unwrap();
        assert_eq!(after_first.display_name, None);

        // Second call — explicit display. Must populate the empty field.
        let id2 = storage
            .upsert_peer("claude", Some("Claude"), "agent")
            .unwrap();
        assert_eq!(id1, id2);
        let after_second = storage.peer_by_name("claude").unwrap().unwrap();
        assert_eq!(
            after_second.display_name.as_deref(),
            Some("Claude"),
            "second call with explicit display should backfill the empty field"
        );

        // Third call — DIFFERENT display. Must NOT overwrite (only empty
        // fields are backfilled; explicit renames go through a dedicated
        // setter, intentionally not present yet).
        storage
            .upsert_peer("claude", Some("Claude-2"), "agent")
            .unwrap();
        let after_third = storage.peer_by_name("claude").unwrap().unwrap();
        assert_eq!(
            after_third.display_name.as_deref(),
            Some("Claude"),
            "subsequent upserts must NOT clobber an existing display_name"
        );
    }

    /// `open_sessions_count()` returns the true count, not the limit
    /// capped value `open_sessions(1).len()` used to give.
    #[test]
    fn open_sessions_count_returns_actual_count() {
        let storage = Storage::open(&tmp_db()).unwrap();
        let peer_id = storage.upsert_peer("claude", None, "agent").unwrap();

        // Open five sessions.
        let mut ids = Vec::new();
        for i in 0..5 {
            ids.push(
                storage
                    .open_session(&peer_id, Some(&format!("s{i}")), "claude-code")
                    .unwrap(),
            );
        }
        assert_eq!(
            storage.open_sessions_count().unwrap(),
            5,
            "open_sessions_count must return all 5, not capped at any limit"
        );

        // Close two, count drops to 3.
        storage.end_session(&ids[0]).unwrap();
        storage.end_session(&ids[1]).unwrap();
        assert_eq!(storage.open_sessions_count().unwrap(), 3);
    }

    /// `upsert_peer` is idempotent on name: calling twice with the same
    /// name returns the same id, and `last_seen_at` advances.
    #[test]
    fn upsert_peer_idempotent_and_touches_last_seen() {
        let storage = Storage::open(&tmp_db()).unwrap();
        let id1 = storage.upsert_peer("User", Some("User"), "human").unwrap();
        let p1 = storage
            .peer_by_name("user")
            .unwrap()
            .expect("peer must exist after upsert");
        // Spin briefly so the second upsert's timestamp moves forward.
        std::thread::sleep(std::time::Duration::from_millis(15));
        let id2 = storage.upsert_peer("USER", None, "human").unwrap();
        assert_eq!(id1, id2, "same canonical name → same id");

        let p2 = storage.peer_by_name("user").unwrap().unwrap();
        assert!(
            p2.last_seen_at > p1.last_seen_at,
            "last_seen_at must advance on re-upsert: {:?} vs {:?}",
            p1.last_seen_at,
            p2.last_seen_at,
        );
        // display_name from the FIRST upsert is preserved (we don't
        // overwrite on subsequent upserts).
        assert_eq!(p2.display_name.as_deref(), Some("User"));
    }

    /// Empty name or kind is a programmer error; upsert must reject.
    #[test]
    fn upsert_peer_rejects_empty_inputs() {
        let storage = Storage::open(&tmp_db()).unwrap();
        assert!(storage.upsert_peer("", None, "human").is_err());
        assert!(storage.upsert_peer("   ", None, "human").is_err());
        assert!(storage.upsert_peer("user", None, "").is_err());
    }

    /// Sessions open and close with the expected timestamps. Multiple
    /// open sessions per peer are allowed (e.g. two Claude Code windows).
    #[test]
    fn sessions_lifecycle() {
        let storage = Storage::open(&tmp_db()).unwrap();
        let peer_id = storage.upsert_peer("claude", None, "agent").unwrap();

        let s1 = storage
            .open_session(&peer_id, Some("project-x"), "claude-code")
            .unwrap();
        let s2 = storage
            .open_session(&peer_id, Some("debug-session"), "claude-code")
            .unwrap();
        assert_ne!(s1, s2);

        let sessions = storage.sessions_for_peer(&peer_id, 10).unwrap();
        assert_eq!(sessions.len(), 2);
        assert!(sessions.iter().all(|s| s.is_open()));

        // End one, verify state.
        storage.end_session(&s1).unwrap();
        let after_close = storage.sessions_for_peer(&peer_id, 10).unwrap();
        let s1_after = after_close.iter().find(|s| s.id == s1).unwrap();
        let s2_after = after_close.iter().find(|s| s.id == s2).unwrap();
        assert!(!s1_after.is_open(), "s1 should be closed");
        assert!(s2_after.is_open(), "s2 should still be open");

        // open_sessions returns only the still-open ones.
        let open = storage.open_sessions(10).unwrap();
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].id, s2);

        // end_session on an already-ended id is a no-op (idempotent).
        storage.end_session(&s1).unwrap();
        let again = storage.sessions_for_peer(&peer_id, 10).unwrap();
        let s1_again = again.iter().find(|s| s.id == s1).unwrap();
        assert_eq!(
            s1_after.ended_at, s1_again.ended_at,
            "ended_at must not move on a double-close"
        );
    }

    /// Leaderboard: longest completed sessions first, mixed timestamp
    /// formats both parsed, open / clock-skewed rows excluded, and
    /// top_project = the strongest attribution-window signal (NOT
    /// memories.session_id joins — those are empty in the watcher flow,
    /// Codex P1). A single-memory project stays under the noise floor.
    #[test]
    fn longest_sessions_orders_filters_and_attributes() {
        let storage = Storage::open(&tmp_db()).unwrap();
        let peer_id = storage.upsert_peer("claude", None, "agent").unwrap();
        let mk = |label: &str| {
            storage
                .open_session(&peer_id, Some(label), "claude-code")
                .unwrap()
        };
        let long = mk("long"); // 3h, SQLite format
        let mid = mk("mid"); // 40m, RFC3339 format
        let short = mk("short"); // 90s
        let _open = mk("open"); // stays open — excluded
        let skew = mk("skew"); // ends before it starts — excluded
        {
            let conn = storage.conn.lock().unwrap();
            for (id, start, end) in [
                (&long, "2026-07-01 10:00:00", "2026-07-01 13:00:00"),
                (
                    &mid,
                    "2026-07-01T10:00:00+00:00",
                    "2026-07-01T10:40:00+00:00",
                ),
                (&short, "2026-07-01 10:00:00", "2026-07-01 10:01:30"),
                (&skew, "2026-07-02 10:00:00", "2026-07-02 09:00:00"),
            ] {
                conn.execute(
                    "UPDATE sessions SET started_at = ?2, ended_at = ?3 WHERE id = ?1",
                    params![id, start, end],
                )
                .unwrap();
            }
        }
        // Project signal INSIDE the long session's window (11:00-11:30 —
        // also outside the mid session's padded 09:50-10:50 window):
        // demoapp gets 2 memories (clears the floor), sideproj only 1
        // (extractor-noise floor keeps it out). No session_id links on
        // purpose — production memories don't carry usable ones.
        let demo = storage
            .upsert_entity(&crate::graph::Entity {
                name: "demoapp".into(),
                entity_type: crate::graph::EntityType::Project,
            })
            .unwrap();
        let side = storage
            .upsert_entity(&crate::graph::Entity {
                name: "sideproj".into(),
                entity_type: crate::graph::EntityType::Project,
            })
            .unwrap();
        let link = |title: &str, ts: &str, entity: &str| {
            let m = crate::event::MemoryEntry::new(
                title,
                "body",
                MemoryType::Note,
                EventSource::Socket,
            );
            storage.save(&m).unwrap();
            storage.link_memory_entity(&m.id, entity).unwrap();
            let conn = storage.conn.lock().unwrap();
            conn.execute(
                "UPDATE memories SET timestamp = ?2 WHERE id = ?1",
                params![m.id, ts],
            )
            .unwrap();
        };
        link("work note 1", "2026-07-01T11:00:00+00:00", &demo);
        link("work note 2", "2026-07-01T11:30:00+00:00", &demo);
        link("side note", "2026-07-01T11:15:00+00:00", &side);

        let rows = storage.longest_sessions(10).unwrap();
        let ids: Vec<&str> = rows.iter().map(|r| r.session_id.as_str()).collect();
        assert_eq!(
            ids,
            vec![long.as_str(), mid.as_str(), short.as_str()],
            "longest first; open + skewed rows excluded"
        );
        assert_eq!(rows[0].duration_seconds, 3 * 3600);
        assert_eq!(rows[1].duration_seconds, 40 * 60);
        assert_eq!(rows[2].duration_seconds, 90);
        assert_eq!(rows[0].top_project.as_deref(), Some("demoapp"));
        assert!(
            rows[1].top_project.is_none(),
            "no project memories → no top_project"
        );
        assert_eq!(storage.longest_sessions(1).unwrap().len(), 1);
        assert!(storage.longest_sessions(0).unwrap().is_empty());
    }

    /// The signal pad is clamped to the midpoint toward the neighbouring
    /// session: a memory saved shortly AFTER session A ends counts toward
    /// A only — session B starting 10 minutes later must not inherit it
    /// through its own pre-pad (Codex P1, cross-session contamination).
    #[test]
    fn longest_sessions_signal_pad_clamps_to_neighbour_midpoint() {
        let storage = Storage::open(&tmp_db()).unwrap();
        let peer_id = storage.upsert_peer("claude", None, "agent").unwrap();
        let a = storage
            .open_session(&peer_id, Some("a"), "claude-code")
            .unwrap();
        let b = storage
            .open_session(&peer_id, Some("b"), "claude-code")
            .unwrap();
        let ghost = storage
            .open_session(&peer_id, Some("ghost"), "claude-code")
            .unwrap();
        {
            let conn = storage.conn.lock().unwrap();
            // A 10:00-11:00, B 11:10-11:20 — gap midpoint is 11:05.
            conn.execute(
                "UPDATE sessions SET started_at = '2026-07-01 10:00:00',
                                     ended_at   = '2026-07-01 11:00:00' WHERE id = ?1",
                params![a],
            )
            .unwrap();
            conn.execute(
                "UPDATE sessions SET started_at = '2026-07-01 11:10:00',
                                     ended_at   = '2026-07-01 11:20:00' WHERE id = ?1",
                params![b],
            )
            .unwrap();
            // An INVALID completed span (ends before it starts) sitting in
            // the gap. If it survived into the neighbour set, its 11:03
            // start would clamp A's post-pad to 11:01:30 and steal the
            // wrap-up memories below (review point: invalid completed
            // spans must not bound valid sessions).
            conn.execute(
                "UPDATE sessions SET started_at = '2026-07-01 11:03:00',
                                     ended_at   = '2026-07-01 09:00:00' WHERE id = ?1",
                params![ghost],
            )
            .unwrap();
        }
        // Two project memories land 1-2 minutes after A ends: inside A's
        // clamped post-pad (up to 11:05), OUTSIDE B's clamped pre-pad
        // (from 11:05). An unclamped ±10min pad would hand them to BOTH.
        let proj = storage
            .upsert_entity(&crate::graph::Entity {
                name: "gapproj".into(),
                entity_type: crate::graph::EntityType::Project,
            })
            .unwrap();
        for (title, ts) in [
            ("wrapup note 1", "2026-07-01T11:01:00+00:00"),
            ("wrapup note 2", "2026-07-01T11:02:00+00:00"),
        ] {
            let m = crate::event::MemoryEntry::new(
                title,
                "body",
                MemoryType::Note,
                EventSource::Socket,
            );
            storage.save(&m).unwrap();
            storage.link_memory_entity(&m.id, &proj).unwrap();
            let conn = storage.conn.lock().unwrap();
            conn.execute(
                "UPDATE memories SET timestamp = ?2 WHERE id = ?1",
                params![m.id, ts],
            )
            .unwrap();
        }

        let rows = storage.longest_sessions(10).unwrap();
        let row_a = rows.iter().find(|r| r.session_id == a).unwrap();
        let row_b = rows.iter().find(|r| r.session_id == b).unwrap();
        assert_eq!(
            row_a.top_project.as_deref(),
            Some("gapproj"),
            "the closer session owns the wrap-up memories"
        );
        assert!(
            row_b.top_project.is_none(),
            "the later session must not inherit its neighbour's signal"
        );
    }

    /// Round-trip: link a memory to a session, read it back via
    /// `memories_for_session`, clear with `None`, verify it disappears.
    /// Also exercises ordering: oldest-first by timestamp.
    #[test]
    fn set_memory_session_round_trip_and_clears_on_none() {
        let storage = Storage::open(&tmp_db()).unwrap();
        let peer_id = storage.upsert_peer("claude", None, "agent").unwrap();
        let sid = storage
            .open_session(&peer_id, Some("dev-session"), "claude-code")
            .unwrap();

        // Two memories saved out-of-order; helper must return them
        // oldest-first by `timestamp` regardless of insertion order.
        let mut m_late = make_entry("late");
        m_late.timestamp = chrono::Utc::now();
        let mut m_early = make_entry("early");
        m_early.timestamp = m_late.timestamp - chrono::Duration::minutes(5);
        storage.save(&m_late).unwrap();
        storage.save(&m_early).unwrap();

        storage.set_memory_session(&m_late.id, Some(&sid)).unwrap();
        storage.set_memory_session(&m_early.id, Some(&sid)).unwrap();

        let in_session = storage.memories_for_session(&sid).unwrap();
        assert_eq!(in_session.len(), 2);
        assert_eq!(in_session[0].id, m_early.id, "oldest-first ordering");
        assert_eq!(in_session[1].id, m_late.id);

        // Clear the link on `m_late` — it must drop out of the session
        // view, but the memory row itself stays.
        storage.set_memory_session(&m_late.id, None).unwrap();
        let after_clear = storage.memories_for_session(&sid).unwrap();
        assert_eq!(after_clear.len(), 1);
        assert_eq!(after_clear[0].id, m_early.id);
        assert!(storage.get_by_id(&m_late.id).unwrap().is_some());
    }

    /// `set_memory_session` rejects non-existent session ids and
    /// non-existent memory ids loudly. On fresh DBs the FK on
    /// `memories.session_id` would catch the bad session id at the DB
    /// level too, but the app-level check fires first and gives a
    /// clearer error.
    #[test]
    fn set_memory_session_rejects_unknown_ids() {
        let storage = Storage::open(&tmp_db()).unwrap();
        let m = make_entry("m");
        storage.save(&m).unwrap();

        assert!(
            storage
                .set_memory_session(&m.id, Some("00000000-aaaa-bbbb-cccc-000000000000"))
                .is_err(),
            "unknown session id must error"
        );
        assert!(
            storage
                .set_memory_session("not-a-real-memory-id", None)
                .is_err(),
            "unknown memory id must error"
        );
    }

    /// Deleting a session sets the FK-linked memories' `session_id` to
    /// NULL (ON DELETE SET NULL) on fresh DBs. The memory rows survive
    /// — historical record stays, just loses the session link.
    #[test]
    fn deleting_session_clears_memory_session_id_via_fk() {
        let storage = Storage::open(&tmp_db()).unwrap();
        let peer_id = storage.upsert_peer("claude", None, "agent").unwrap();
        let sid = storage
            .open_session(&peer_id, Some("session"), "claude-code")
            .unwrap();
        let m = make_entry("m");
        storage.save(&m).unwrap();
        storage.set_memory_session(&m.id, Some(&sid)).unwrap();

        // Raw DELETE on the sessions row triggers ON DELETE SET NULL
        // on memories.session_id (fresh DB has the FK from CREATE TABLE).
        {
            let conn = storage.conn.lock().unwrap();
            conn.execute("DELETE FROM sessions WHERE id = ?1", params![sid])
                .unwrap();
        }

        // Memory row still exists; session_id is now NULL so it doesn't
        // appear in memories_for_session for the (now-gone) id.
        assert!(storage.get_by_id(&m.id).unwrap().is_some());
        assert!(storage.memories_for_session(&sid).unwrap().is_empty());
    }

    /// Prefix lookup returns just matching ids. Empty/invalid prefixes
    /// are rejected. A real-world prefix length (8 chars) is the CLI's
    /// common path.
    #[test]
    fn find_session_ids_by_prefix_matches_and_rejects_garbage() {
        let storage = Storage::open(&tmp_db()).unwrap();
        let peer_id = storage.upsert_peer("claude", None, "agent").unwrap();
        let s1 = storage.open_session(&peer_id, Some("a"), "src").unwrap();
        let _s2 = storage.open_session(&peer_id, Some("b"), "src").unwrap();

        let prefix = &s1[..8];
        let matches = storage.find_session_ids_by_prefix(prefix).unwrap();
        assert!(matches.contains(&s1), "prefix must find its own id");

        // Empty prefix is an error (LIKE '%' would match everything,
        // silently breaking the "be specific" CLI contract).
        assert!(storage.find_session_ids_by_prefix("").is_err());
        // Non-hex / non-dash characters are rejected — guards against
        // LIKE pattern injection via `%` or `_` if a future caller
        // passes raw user input.
        assert!(storage.find_session_ids_by_prefix("ab%cd").is_err());
        assert!(storage.find_session_ids_by_prefix("not_hex").is_err());
    }

    /// Migration idempotency: opening the same DB path twice in a row
    /// must not error or duplicate the `session_id` column. The
    /// `migrate_add_column` helper swallows the "duplicate column" SQLite
    /// error; this test makes sure it really does so for our column.
    #[test]
    fn memories_session_id_migration_is_idempotent() {
        let path = tmp_db();
        let _s1 = Storage::open(&path).unwrap();
        // Second open hits the migration code path again — must not panic.
        let s2 = Storage::open(&path).unwrap();
        // Sanity: column exists and is queryable.
        let conn = s2.conn.lock().unwrap();
        let _: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM memories WHERE session_id IS NULL",
                [],
                |r| r.get(0),
            )
            .expect("session_id column must exist after migration");
    }

    /// Fresh DBs end up with a FK on memories → sessions and the
    /// idempotent rebuild migration is a no-op for them. Codex flagged
    /// that legacy DBs were missing the FK; this test pins the fresh
    /// path AND verifies a second open doesn't re-rebuild.
    #[test]
    fn memories_session_id_fk_present_on_fresh_db() {
        let path = tmp_db();
        let storage = Storage::open(&path).unwrap();
        let conn = storage.conn.lock().unwrap();
        let fk_target: Option<String> = conn
            .prepare("PRAGMA foreign_key_list(memories)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(2))
            .unwrap()
            .filter_map(|r| r.ok())
            .find(|t| t == "sessions");
        assert!(
            fk_target.is_some(),
            "fresh DB must have FK memories.session_id → sessions(id)"
        );
    }

    /// Legacy retrofit: simulate an old DB where memories has
    /// session_id without a FK, then run the migration via Storage::open
    /// and verify (1) the FK is now present, (2) row count is preserved,
    /// (3) stale session_id values are nulled out, (4) FTS still works
    /// after the rebuild.
    #[test]
    fn migrate_memories_session_fk_retrofits_legacy_db_and_rebuilds_fts() {
        use rusqlite::Connection;
        let path = tmp_db();

        // --- Simulate legacy: build memories WITHOUT the session_id FK,
        // plus the FTS scaffolding the production schema has. Add one
        // memory with a valid (but unreferenced) session_id-shaped
        // string, and one with NULL.
        {
            let conn = Connection::open(&path).unwrap();
            conn.pragma_update(None, "foreign_keys", "OFF").unwrap();
            conn.execute_batch(
                "CREATE TABLE memories (
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
                    created_at TEXT NOT NULL DEFAULT (datetime('now')),
                    access_count INTEGER NOT NULL DEFAULT 0,
                    last_accessed_at TEXT,
                    superseded_by TEXT,
                    canonical_memory_id TEXT,
                    session_id TEXT
                );
                CREATE TABLE peers (
                    id TEXT PRIMARY KEY,
                    name TEXT NOT NULL UNIQUE,
                    display_name TEXT,
                    kind TEXT NOT NULL,
                    created_at TEXT NOT NULL DEFAULT (datetime('now')),
                    last_seen_at TEXT NOT NULL DEFAULT (datetime('now'))
                );
                CREATE TABLE sessions (
                    id TEXT PRIMARY KEY,
                    peer_id TEXT NOT NULL,
                    label TEXT,
                    started_at TEXT NOT NULL DEFAULT (datetime('now')),
                    ended_at TEXT,
                    source TEXT NOT NULL
                );",
            )
            .unwrap();

            // Insert a real session so the survivor's session_id resolves.
            conn.execute(
                "INSERT INTO peers (id, name, kind)
                 VALUES ('peer-claude', 'claude', 'agent')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO sessions (id, peer_id, source)
                 VALUES ('real-session-id', 'peer-claude', 'jsonl')",
                [],
            )
            .unwrap();

            // Three memories: one with a valid session_id, one with a
            // STALE session_id (the migration must null it out), one
            // with NULL session_id (passes through unchanged).
            conn.execute(
                "INSERT INTO memories (id, timestamp, title, content, memory_type,
                    tags, source, importance, metadata, session_id)
                 VALUES ('m-valid', datetime('now'), 'valid', '', 'note',
                    '[]', '\"Manual\"', 0.5, 'null', 'real-session-id')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO memories (id, timestamp, title, content, memory_type,
                    tags, source, importance, metadata, session_id)
                 VALUES ('m-stale', datetime('now'), 'stale', '', 'note',
                    '[]', '\"Manual\"', 0.5, 'null', 'gone-session-id')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO memories (id, timestamp, title, content, memory_type,
                    tags, source, importance, metadata, session_id)
                 VALUES ('m-null', datetime('now'), 'null', '', 'note',
                    '[]', '\"Manual\"', 0.5, 'null', NULL)",
                [],
            )
            .unwrap();

            // Confirm: no FK on memories in this legacy state.
            let mut stmt = conn.prepare("PRAGMA foreign_key_list(memories)").unwrap();
            let cnt = stmt
                .query_map([], |_| Ok(()))
                .unwrap()
                .filter_map(|r| r.ok())
                .count();
            assert_eq!(cnt, 0, "legacy memories must have no FK");
        }

        // --- Run the migration via Storage::open ---
        let storage = Storage::open(&path).unwrap();

        // 1. FK is now present and points at sessions.
        {
            let conn = storage.conn.lock().unwrap();
            let fk_target: Option<String> = conn
                .prepare("PRAGMA foreign_key_list(memories)")
                .unwrap()
                .query_map([], |row| row.get::<_, String>(2))
                .unwrap()
                .filter_map(|r| r.ok())
                .find(|t| t == "sessions");
            assert!(
                fk_target.is_some(),
                "post-migration memories must have FK → sessions"
            );

            // 2. Row count preserved.
            let n: i64 = conn
                .query_row("SELECT COUNT(*) FROM memories", [], |r| r.get(0))
                .unwrap();
            assert_eq!(n, 3, "all 3 rows must survive migration");

            // 3. Stale session_id was nulled out; valid one kept; NULL stayed.
            let valid: Option<String> = conn
                .query_row(
                    "SELECT session_id FROM memories WHERE id = 'm-valid'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(valid.as_deref(), Some("real-session-id"));
            let stale: Option<String> = conn
                .query_row(
                    "SELECT session_id FROM memories WHERE id = 'm-stale'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert!(stale.is_none(), "stale session_id must be nulled out");
            let null_row: Option<String> = conn
                .query_row(
                    "SELECT session_id FROM memories WHERE id = 'm-null'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert!(null_row.is_none());

            // 4. FTS table exists and was repopulated — search the
            // rebuilt index for the survivor's title.
            let fts_hits: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM memories_fts WHERE title MATCH 'valid'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert!(fts_hits > 0, "FTS must be rebuilt after table swap");
        }

        // 5. Now that the FK is enforced, deleting the session cascades
        //    SET NULL on the linked memory.
        {
            let conn = storage.conn.lock().unwrap();
            conn.execute("DELETE FROM sessions WHERE id = 'real-session-id'", [])
                .unwrap();
            let after: Option<String> = conn
                .query_row(
                    "SELECT session_id FROM memories WHERE id = 'm-valid'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert!(
                after.is_none(),
                "DELETE on sessions must cascade SET NULL via the new FK"
            );
        }

        // 6. Idempotency: a second open is a no-op (FK already present;
        //    detection short-circuits).
        let _ = Storage::open(&path).unwrap();
    }

    /// `open_sessions_for_peer` filters inside the SQL so LIMIT applies
    /// AFTER the open check — Codex caught that the previous CLI path
    /// (sessions_for_peer + client-side filter) could return 0 results
    /// when the most recent N peer sessions were closed and older open
    /// ones existed beyond the window.
    #[test]
    fn open_sessions_for_peer_filters_inside_sql_not_after_limit() {
        let storage = Storage::open(&tmp_db()).unwrap();
        let peer_id = storage.upsert_peer("claude", None, "agent").unwrap();

        // Three closed sessions, then one open. With limit=3, the old
        // sessions_for_peer(3).filter(open) returned 0 — the open one
        // sat at position 4 outside the LIMIT window.
        let s_open = {
            let c1 = storage.open_session(&peer_id, Some("c1"), "jsonl").unwrap();
            storage.end_session(&c1).unwrap();
            let c2 = storage.open_session(&peer_id, Some("c2"), "jsonl").unwrap();
            storage.end_session(&c2).unwrap();
            let c3 = storage.open_session(&peer_id, Some("c3"), "jsonl").unwrap();
            storage.end_session(&c3).unwrap();
            storage
                .open_session(&peer_id, Some("live"), "jsonl")
                .unwrap()
        };

        let opens = storage.open_sessions_for_peer(&peer_id, 3).unwrap();
        assert_eq!(
            opens.len(),
            1,
            "open-filter must run inside SQL, not after LIMIT"
        );
        assert_eq!(opens[0].id, s_open);
    }

    /// `closed_sessions_since` drives the `dream batch` CLI:
    /// (a) excludes open sessions (ended_at IS NULL),
    /// (b) excludes sessions whose ended_at is older than the
    ///     `since_hours` cutoff,
    /// (c) returns rows newest-ended first.
    /// Codex P2 caught that this helper was untested even though
    /// it's the gate for which sessions get summarized at all.
    #[test]
    fn closed_sessions_since_filters_window_and_excludes_open() {
        let storage = Storage::open(&tmp_db()).unwrap();
        let peer_id = storage.upsert_peer("claude", None, "agent").unwrap();

        // Open session — must be excluded regardless of cutoff.
        let _s_open = storage
            .open_session(&peer_id, Some("open"), "jsonl")
            .unwrap();

        // Recent closed session (ended a few seconds ago).
        let s_recent = storage
            .open_session(&peer_id, Some("recent"), "jsonl")
            .unwrap();
        storage.end_session(&s_recent).unwrap();

        // Ancient closed session — set ended_at to 100 hours ago so
        // the 24-hour window misses it. Going through raw SQL because
        // there's no public helper that accepts a custom ended_at.
        let s_old = storage
            .open_session(&peer_id, Some("old"), "jsonl")
            .unwrap();
        {
            let conn = storage.conn.lock().unwrap();
            conn.execute(
                "UPDATE sessions SET ended_at = datetime('now', '-100 hours') WHERE id = ?1",
                rusqlite::params![s_old],
            )
            .unwrap();
        }

        // 24-hour window should pick up only the recent closed row.
        let results = storage.closed_sessions_since(24, 10).unwrap();
        let ids: Vec<String> = results.iter().map(|s| s.id.clone()).collect();
        assert_eq!(
            ids,
            vec![s_recent.clone()],
            "must include recent closed, exclude open AND >24h-old"
        );

        // Widening the window to 200 hours pulls in the ancient one
        // too, AND newest-first ordering puts the recent one first.
        let wide = storage.closed_sessions_since(200, 10).unwrap();
        let wide_ids: Vec<String> = wide.iter().map(|s| s.id.clone()).collect();
        assert_eq!(
            wide_ids,
            vec![s_recent, s_old],
            "wider window pulls older row but recent stays first"
        );
        // Open session never appears.
        assert!(
            wide.iter().all(|s| s.ended_at.is_some()),
            "closed_sessions_since must never return open sessions"
        );
    }

    /// Codex P2 follow-up: `session_summary_lookup` is the idempotency
    /// gate for `dream batch`. A snapshot generated via `--allow-open`
    /// must NOT block a future canonical summary after the session
    /// closes. The lookup filters `open_at_summary_time = true` rows
    /// by default; the explicit `_including_snapshots` variant returns
    /// them for audit purposes.
    #[test]
    fn session_summary_lookup_excludes_snapshots_but_finds_canonical() {
        let storage = Storage::open(&tmp_db()).unwrap();
        let peer_id = storage.upsert_peer("claude", None, "agent").unwrap();
        let session_id = storage
            .open_session(&peer_id, Some("test"), "jsonl")
            .unwrap();

        // Save a snapshot summary (open_at_summary_time = true).
        let snapshot = MemoryEntry {
            id: uuid::Uuid::new_v4().to_string(),
            timestamp: chrono::Utc::now(),
            title: "snapshot".into(),
            content: "snap body".into(),
            memory_type: crate::event::MemoryType::SessionSummary,
            tags: vec![],
            source: crate::event::EventSource::Manual,
            importance: 0.7,
            metadata: serde_json::json!({
                "summary_of_session": session_id,
                "open_at_summary_time": true,
            }),
        };
        storage.save(&snapshot).unwrap();

        // Default lookup must NOT find the snapshot — that's what
        // unfreezes the batch path so a real summary can be made
        // after the session closes.
        let default = storage.session_summary_lookup(&session_id).unwrap();
        assert!(
            default.is_none(),
            "snapshot must not satisfy canonical lookup (would freeze batch)"
        );

        // _including_snapshots DOES find it — needed for audit / debug.
        let audit = storage
            .session_summary_lookup_including_snapshots(&session_id)
            .unwrap();
        assert_eq!(audit.unwrap().id, snapshot.id);

        // Now save a canonical summary (open_at_summary_time = false).
        let canonical = MemoryEntry {
            id: uuid::Uuid::new_v4().to_string(),
            timestamp: chrono::Utc::now(),
            title: "canonical".into(),
            content: "final body".into(),
            memory_type: crate::event::MemoryType::SessionSummary,
            tags: vec![],
            source: crate::event::EventSource::Manual,
            importance: 0.7,
            metadata: serde_json::json!({
                "summary_of_session": session_id,
                "open_at_summary_time": false,
            }),
        };
        storage.save(&canonical).unwrap();

        // Default lookup now finds the canonical one.
        let found = storage.session_summary_lookup(&session_id).unwrap();
        assert_eq!(
            found.unwrap().id,
            canonical.id,
            "canonical summary must satisfy lookup once it exists"
        );
    }

    /// Pre-fix summaries don't carry the `open_at_summary_time` key
    /// at all. They were only made for closed sessions (the strict
    /// default existed implicitly), so the lookup must treat them as
    /// canonical to avoid re-summarizing every session after upgrade.
    /// `COALESCE(..., 0) = 0` handles the NULL-from-missing-key case.
    #[test]
    fn session_summary_lookup_treats_legacy_summaries_without_key_as_canonical() {
        let storage = Storage::open(&tmp_db()).unwrap();
        let peer_id = storage.upsert_peer("claude", None, "agent").unwrap();
        let session_id = storage
            .open_session(&peer_id, Some("legacy"), "jsonl")
            .unwrap();
        let legacy = MemoryEntry {
            id: uuid::Uuid::new_v4().to_string(),
            timestamp: chrono::Utc::now(),
            title: "old".into(),
            content: "no open_at_summary_time field".into(),
            memory_type: crate::event::MemoryType::SessionSummary,
            tags: vec![],
            source: crate::event::EventSource::Manual,
            importance: 0.7,
            metadata: serde_json::json!({
                "summary_of_session": session_id,
                // Deliberately no open_at_summary_time key — mimics
                // pre-fix data.
            }),
        };
        storage.save(&legacy).unwrap();

        let found = storage.session_summary_lookup(&session_id).unwrap();
        assert_eq!(
            found.unwrap().id,
            legacy.id,
            "pre-fix summaries (no key) must be treated as canonical"
        );
    }

    /// Codex backlog #1: the unique partial index
    /// `idx_session_summary_canonical` prevents a worker/CLI race
    /// from producing two canonical summaries for one session.
    /// Trying to INSERT a second canonical for the same session
    /// must hit a UNIQUE constraint at the DB level — the previous
    /// in-Rust lookup-then-save check had a 1-second race window.
    #[test]
    fn unique_partial_index_blocks_duplicate_canonical_session_summary() {
        let storage = Storage::open(&tmp_db()).unwrap();
        let peer_id = storage.upsert_peer("claude", None, "agent").unwrap();
        let session_id = storage.open_session(&peer_id, Some("s"), "jsonl").unwrap();

        // First canonical summary lands cleanly.
        let s1 = MemoryEntry {
            id: uuid::Uuid::new_v4().to_string(),
            timestamp: chrono::Utc::now(),
            title: "first canonical".into(),
            content: "body".into(),
            memory_type: crate::event::MemoryType::SessionSummary,
            tags: vec![],
            source: crate::event::EventSource::Manual,
            importance: 0.7,
            metadata: serde_json::json!({
                "summary_of_session": session_id,
                "open_at_summary_time": false,
            }),
        };
        storage.save(&s1).unwrap();

        // Second canonical for the SAME session — must fail at
        // the DB level (UNIQUE constraint). Pre-fix code would
        // accept this and leave the lookup ambiguous.
        let s2 = MemoryEntry {
            id: uuid::Uuid::new_v4().to_string(),
            timestamp: chrono::Utc::now(),
            title: "second canonical (race)".into(),
            content: "body".into(),
            memory_type: crate::event::MemoryType::SessionSummary,
            tags: vec![],
            source: crate::event::EventSource::Manual,
            importance: 0.7,
            metadata: serde_json::json!({
                "summary_of_session": session_id,
                "open_at_summary_time": false,
            }),
        };
        let result = storage.save(&s2);
        assert!(
            result.is_err(),
            "second canonical for same session must be rejected by unique partial index"
        );
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.to_lowercase().contains("unique") || err_msg.contains("constraint"),
            "error should mention the unique constraint: {err_msg}"
        );
    }

    /// Snapshots (`open_at_summary_time = true`) are exempt from
    /// the unique constraint by design — multiple per-session
    /// snapshots are allowed. Pin this so a future tweak to the
    /// index doesn't accidentally lock them down.
    #[test]
    fn unique_partial_index_allows_multiple_snapshots_per_session() {
        let storage = Storage::open(&tmp_db()).unwrap();
        let peer_id = storage.upsert_peer("claude", None, "agent").unwrap();
        let session_id = storage.open_session(&peer_id, Some("s"), "jsonl").unwrap();

        for i in 0..3 {
            let snap = MemoryEntry {
                id: uuid::Uuid::new_v4().to_string(),
                timestamp: chrono::Utc::now(),
                title: format!("snapshot {i}"),
                content: "body".into(),
                memory_type: crate::event::MemoryType::SessionSummary,
                tags: vec![],
                source: crate::event::EventSource::Manual,
                importance: 0.7,
                metadata: serde_json::json!({
                    "summary_of_session": session_id,
                    "open_at_summary_time": true,
                }),
            };
            storage
                .save(&snap)
                .expect("snapshots must not collide on the canonical index");
        }
    }

    /// Pre-index dedup removes duplicate canonicals before the
    /// UNIQUE index is created. Simulates a legacy DB that ran
    /// pre-fix worker/CLI race: two canonicals for one session.
    /// After `Storage::open` walks the migration chain, exactly
    /// one canonical survives (newest by timestamp).
    #[test]
    fn dedupe_canonical_session_summaries_keeps_newest_on_legacy_db() {
        use rusqlite::Connection;
        let path = tmp_db();

        // Stage 1: build a minimal legacy schema and insert two
        // canonical summaries for the same session.
        {
            let conn = Connection::open(&path).unwrap();
            conn.pragma_update(None, "foreign_keys", "OFF").unwrap();
            conn.execute_batch(
                "CREATE TABLE memories (
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
                    created_at TEXT NOT NULL DEFAULT (datetime('now')),
                    access_count INTEGER NOT NULL DEFAULT 0,
                    last_accessed_at TEXT,
                    superseded_by TEXT,
                    canonical_memory_id TEXT,
                    session_id TEXT
                );",
            )
            .unwrap();
            // Older row — timestamp earlier.
            conn.execute(
                "INSERT INTO memories (id, timestamp, title, content, memory_type,
                    tags, source, importance, metadata)
                 VALUES ('older', '2026-05-25T00:00:00Z', 'old', '', 'session_summary',
                    '[]', '\"Manual\"', 0.7, '{\"summary_of_session\":\"sess\",\"open_at_summary_time\":false}')",
                [],
            )
            .unwrap();
            // Newer row — should win.
            conn.execute(
                "INSERT INTO memories (id, timestamp, title, content, memory_type,
                    tags, source, importance, metadata)
                 VALUES ('newer', '2026-05-26T00:00:00Z', 'new', '', 'session_summary',
                    '[]', '\"Manual\"', 0.7, '{\"summary_of_session\":\"sess\",\"open_at_summary_time\":false}')",
                [],
            )
            .unwrap();
        }

        // Stage 2: Storage::open runs the migration chain
        // including dedupe_canonical_session_summaries.
        let storage = Storage::open(&path).unwrap();
        let conn = storage.conn.lock().unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM memories
                  WHERE memory_type = 'session_summary'
                    AND json_extract(metadata, '$.summary_of_session') = 'sess'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "duplicates must be deduped before index creation");

        let survivor: String = conn
            .query_row(
                "SELECT id FROM memories
                  WHERE memory_type = 'session_summary'
                    AND json_extract(metadata, '$.summary_of_session') = 'sess'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(survivor, "newer", "most recent timestamp wins");
    }

    /// `merge_peers` moves links from src → dst and deletes src.
    /// Verifies the common case (cleanly re-point) plus collision case
    /// (src and dst BOTH have a link for the same memory with the same
    /// role → INSERT OR IGNORE collapses it).
    #[test]
    fn merge_peers_moves_links_and_handles_collisions() {
        let storage = Storage::open(&tmp_db()).unwrap();
        let m1 = make_entry("m1");
        let m2 = make_entry("m2");
        storage.save(&m1).unwrap();
        storage.save(&m2).unwrap();

        let src = storage.upsert_peer("alice", None, "human").unwrap();
        let dst = storage.upsert_peer("user", None, "human").unwrap();

        // src has links for both memories; dst has a colliding link for m1.
        storage.link_memory_peer(&m1.id, &src, "speaker").unwrap();
        storage.link_memory_peer(&m2.id, &src, "speaker").unwrap();
        storage.link_memory_peer(&m1.id, &dst, "speaker").unwrap();

        // src also owns a session — merge must re-point this to dst,
        // otherwise dropping src would CASCADE-delete the session's
        // history. Codex flagged the original test didn't cover this.
        let src_session = storage
            .open_session(&src, Some("src-session"), "test")
            .unwrap();

        let moved = storage.merge_peers("alice", "user").unwrap();
        // INSERT OR IGNORE: m1 link collided (already exists for dst),
        // m2 link successfully moved. So count = 1.
        assert_eq!(moved, 1, "INSERT OR IGNORE collapses the colliding row");

        // src peer must be gone.
        assert!(storage.peer_by_name("alice").unwrap().is_none());

        // dst should have both memories linked now.
        let m1_peers = storage.peers_for_memory(&m1.id).unwrap();
        let m2_peers = storage.peers_for_memory(&m2.id).unwrap();
        assert_eq!(m1_peers.len(), 1);
        assert_eq!(m1_peers[0].0.name, "user");
        assert_eq!(m2_peers.len(), 1);
        assert_eq!(m2_peers[0].0.name, "user");

        // Session must have been re-pointed to dst — survived the merge.
        let dst_sessions = storage.sessions_for_peer(&dst, 10).unwrap();
        assert!(
            dst_sessions.iter().any(|s| s.id == src_session),
            "session originally owned by src must now belong to dst, got {dst_sessions:?}"
        );
        // And it should still be the only session for dst — no duplication.
        assert_eq!(dst_sessions.len(), 1);
    }

    /// merge_peers rejects self-merge and non-existent peers.
    #[test]
    fn merge_peers_validates_inputs() {
        let storage = Storage::open(&tmp_db()).unwrap();
        storage.upsert_peer("a", None, "human").unwrap();
        storage.upsert_peer("b", None, "human").unwrap();

        // Same peer.
        assert!(storage.merge_peers("a", "a").is_err());
        // Missing src.
        assert!(storage.merge_peers("ghost", "a").is_err());
        // Missing dst.
        assert!(storage.merge_peers("a", "ghost").is_err());

        // Sanity: valid merge still works.
        let _ = storage.merge_peers("a", "b").unwrap();
        assert!(storage.peer_by_name("a").unwrap().is_none());
        assert!(storage.peer_by_name("b").unwrap().is_some());
    }

    /// memory_peers links accept multiple peers per memory, multiple
    /// roles per peer, and stay idempotent on re-link of (memory, peer,
    /// role).
    #[test]
    fn memory_peer_linking_is_role_aware_and_idempotent() {
        let storage = Storage::open(&tmp_db()).unwrap();
        let mem = make_entry("m1");
        storage.save(&mem).unwrap();

        let user = storage.upsert_peer("user", None, "human").unwrap();
        let claude = storage.upsert_peer("claude", None, "agent").unwrap();

        storage.link_memory_peer(&mem.id, &user, "speaker").unwrap();
        storage
            .link_memory_peer(&mem.id, &claude, "addressee")
            .unwrap();
        // Repeat link — must collapse via PRIMARY KEY conflict.
        storage.link_memory_peer(&mem.id, &user, "speaker").unwrap();
        // Same peer, different role — must be a new row.
        storage.link_memory_peer(&mem.id, &user, "subject").unwrap();

        let pairs = storage.peers_for_memory(&mem.id).unwrap();
        assert_eq!(pairs.len(), 3, "got {} pairs: {:?}", pairs.len(), pairs);
        // Sorted by role asc, then name. So order is: addressee/claude,
        // speaker/user, subject/user.
        let roles: Vec<&str> = pairs.iter().map(|(_, r)| r.as_str()).collect();
        assert_eq!(roles, vec!["addressee", "speaker", "subject"]);
    }

    /// Empty subject or predicate is a programmer error; add_fact must
    /// reject it explicitly rather than silently storing junk.
    #[test]
    fn facts_reject_empty_subject_or_predicate() {
        let storage = Storage::open(&tmp_db()).unwrap();
        let mem = make_entry("m1");
        storage.save(&mem).unwrap();

        assert!(storage.add_fact("", "p", "v", &mem.id, 1.0, None).is_err());
        assert!(
            storage
                .add_fact("s", "   ", "v", &mem.id, 1.0, None)
                .is_err()
        );
    }

    /// Conclusion basics: insert, dedup of duplicate source ids inside one
    /// call (cache must equal the unique count, not the input count), and
    /// the M:N link round-trip.
    #[test]
    fn add_conclusion_stores_unique_sources_and_count() {
        let storage = Storage::open(&tmp_db()).unwrap();
        let m1 = make_entry("m1");
        let m2 = make_entry("m2");
        storage.save(&m1).unwrap();
        storage.save(&m2).unwrap();

        // Pass m1 twice on purpose — sloppy caller must not blow the PK.
        let cid = storage
            .add_conclusion(
                "User",
                "preference",
                "prefers low-overhead developer tooling",
                0.7,
                &[m1.id.clone(), m1.id.clone(), m2.id.clone()],
            )
            .unwrap();

        let sources = storage.conclusion_sources(&cid).unwrap();
        assert_eq!(sources.len(), 2, "dedup must collapse the repeat");
        assert!(sources.contains(&m1.id));
        assert!(sources.contains(&m2.id));

        let current = storage.current_conclusions_for_subject("user").unwrap();
        assert_eq!(current.len(), 1);
        assert_eq!(current[0].subject, "user", "subject is lowercased");
        assert_eq!(current[0].kind, "preference");
        assert_eq!(
            current[0].support_count, 2,
            "support_count cache mirrors unique sources"
        );
        assert!(current[0].is_current());
    }

    /// Empty subject / statement must be rejected; kind defaults to
    /// "pattern" when blank.
    #[test]
    fn add_conclusion_validates_inputs_and_defaults_kind() {
        let storage = Storage::open(&tmp_db()).unwrap();
        let m = make_entry("m1");
        storage.save(&m).unwrap();

        assert!(
            storage
                .add_conclusion("", "pattern", "x", 0.5, std::slice::from_ref(&m.id))
                .is_err()
        );
        assert!(
            storage
                .add_conclusion("user", "pattern", "  ", 0.5, std::slice::from_ref(&m.id))
                .is_err()
        );

        let cid = storage
            .add_conclusion(
                "user",
                "  ",
                "ships incrementally",
                0.6,
                std::slice::from_ref(&m.id),
            )
            .unwrap();
        let current = storage.current_conclusions_for_subject("user").unwrap();
        assert_eq!(current.len(), 1);
        assert_eq!(current[0].id, cid);
        assert_eq!(current[0].kind, "pattern", "blank kind → default 'pattern'");
    }

    /// Supersede flips the old row's `superseded_by`, removes it from
    /// `current_conclusions_for_subject`, but keeps it visible in the
    /// full-history `conclusions_for_subject`.
    #[test]
    fn supersede_conclusion_hides_old_from_current_view() {
        let storage = Storage::open(&tmp_db()).unwrap();
        let m = make_entry("m1");
        storage.save(&m).unwrap();

        let old_id = storage
            .add_conclusion(
                "user",
                "pattern",
                "old claim",
                0.5,
                std::slice::from_ref(&m.id),
            )
            .unwrap();
        let new_id = storage
            .add_conclusion(
                "user",
                "pattern",
                "refined claim",
                0.8,
                std::slice::from_ref(&m.id),
            )
            .unwrap();

        storage.supersede_conclusion(&old_id, &new_id).unwrap();

        let current = storage.current_conclusions_for_subject("user").unwrap();
        assert_eq!(current.len(), 1);
        assert_eq!(current[0].id, new_id);

        let history = storage.conclusions_for_subject("user").unwrap();
        assert_eq!(history.len(), 2, "history keeps both rows");

        // Re-superseding the same row is rejected.
        assert!(storage.supersede_conclusion(&old_id, &new_id).is_err());
    }

    /// Deleting the supporting memory cascades into `conclusion_sources`
    /// (foreign key ON DELETE CASCADE), but the conclusion itself
    /// survives — historical record stays, just loses an evidence link.
    #[test]
    fn deleting_memory_clears_conclusion_source_link() {
        let storage = Storage::open(&tmp_db()).unwrap();
        let m1 = make_entry("m1");
        let m2 = make_entry("m2");
        storage.save(&m1).unwrap();
        storage.save(&m2).unwrap();

        let cid = storage
            .add_conclusion(
                "user",
                "pattern",
                "uses many tools",
                0.5,
                &[m1.id.clone(), m2.id.clone()],
            )
            .unwrap();
        assert_eq!(storage.conclusion_sources(&cid).unwrap().len(), 2);

        storage.forget_by_id(&m1.id).unwrap();

        let sources = storage.conclusion_sources(&cid).unwrap();
        assert_eq!(sources.len(), 1, "cascade removed the m1 link");
        assert_eq!(sources[0], m2.id);

        // The conclusion row still exists, AND its support_count cache
        // was updated by the AFTER DELETE trigger — without the trigger
        // this would still report 2 and lie about the evidence count.
        // Codex caught this drift explicitly.
        assert_eq!(storage.conclusions_count().unwrap(), 1);
        let current = storage.current_conclusions_for_subject("user").unwrap();
        assert_eq!(current.len(), 1);
        assert_eq!(
            current[0].support_count, 1,
            "trigger must decrement support_count on cascade delete"
        );
    }

    /// Storage-side confidence gate. CLI validation is not enough — the
    /// future LLM generator will call this helper directly and must hit
    /// the same gate. Rejects out-of-range, NaN, and infinity.
    #[test]
    fn add_conclusion_validates_confidence_range_in_storage() {
        let storage = Storage::open(&tmp_db()).unwrap();
        let m = make_entry("m1");
        storage.save(&m).unwrap();

        // Below 0.0
        assert!(
            storage
                .add_conclusion("user", "pattern", "stmt", -0.1, std::slice::from_ref(&m.id))
                .is_err()
        );
        // Above 1.0
        assert!(
            storage
                .add_conclusion("user", "pattern", "stmt", 1.5, std::slice::from_ref(&m.id))
                .is_err()
        );
        // NaN and infinity (a naive `contains` check passes these — the
        // `is_finite` guard catches them).
        assert!(
            storage
                .add_conclusion(
                    "user",
                    "pattern",
                    "stmt",
                    f32::NAN,
                    std::slice::from_ref(&m.id)
                )
                .is_err()
        );
        assert!(
            storage
                .add_conclusion(
                    "user",
                    "pattern",
                    "stmt",
                    f32::INFINITY,
                    std::slice::from_ref(&m.id)
                )
                .is_err()
        );

        // Boundaries pass.
        assert!(
            storage
                .add_conclusion("user", "pattern", "low", 0.0, &[])
                .is_ok()
        );
        assert!(
            storage
                .add_conclusion("user", "pattern", "high", 1.0, &[])
                .is_ok()
        );
    }

    /// `supersede_conclusion(id, id)` must be rejected — otherwise a
    /// conclusion can supersede itself and silently vanish from the
    /// current view via the WHERE `superseded_by IS NULL` filter.
    /// Codex caught this; the storage layer is the right place to
    /// enforce it because no caller has a legitimate use case for
    /// self-supersede.
    #[test]
    fn supersede_conclusion_rejects_self_reference() {
        let storage = Storage::open(&tmp_db()).unwrap();
        let m = make_entry("m1");
        storage.save(&m).unwrap();

        let id = storage
            .add_conclusion("user", "pattern", "stmt", 0.5, std::slice::from_ref(&m.id))
            .unwrap();

        assert!(storage.supersede_conclusion(&id, &id).is_err());

        // Row is untouched — still current.
        let current = storage.current_conclusions_for_subject("user").unwrap();
        assert_eq!(current.len(), 1);
        assert_eq!(current[0].id, id);
        assert!(current[0].is_current());
    }

    /// `delete_conclusion` removes the row and cascades through
    /// `conclusion_sources` via the existing ON DELETE CASCADE FK.
    /// Source memories themselves are NOT touched — we're just
    /// cleaning up the inductive claim. Returns true on actual
    /// removal, false on unknown id (idempotent).
    #[test]
    fn delete_conclusion_removes_row_and_cascades_sources_but_keeps_memories() {
        let storage = Storage::open(&tmp_db()).unwrap();
        let m1 = make_entry("source one");
        let m2 = make_entry("source two");
        storage.save(&m1).unwrap();
        storage.save(&m2).unwrap();

        let cid = storage
            .add_conclusion(
                "user",
                "pattern",
                "stmt",
                0.6,
                &[m1.id.clone(), m2.id.clone()],
            )
            .unwrap();
        assert_eq!(storage.conclusion_sources(&cid).unwrap().len(), 2);

        // First delete: row gone, sources cascaded.
        let removed = storage.delete_conclusion(&cid).unwrap();
        assert!(removed, "delete must report true when a row was removed");
        assert!(storage.conclusion_by_id(&cid).unwrap().is_none());
        assert_eq!(storage.conclusion_sources(&cid).unwrap().len(), 0);

        // Source memories untouched — conclusion deletion isn't a
        // way to delete underlying memories.
        assert!(storage.get_by_id(&m1.id).unwrap().is_some());
        assert!(storage.get_by_id(&m2.id).unwrap().is_some());

        // Idempotent: deleting an already-gone id is Ok(false), not
        // an error. Lets retry loops stay simple.
        let removed_again = storage.delete_conclusion(&cid).unwrap();
        assert!(!removed_again);
    }

    /// `conclusion_by_id` returns None for missing ids without
    /// erroring, and returns the full row for live ids.
    #[test]
    fn conclusion_by_id_round_trip() {
        let storage = Storage::open(&tmp_db()).unwrap();
        assert!(storage.conclusion_by_id("no-such-id").unwrap().is_none());

        let cid = storage
            .add_conclusion("user", "pattern", "claim", 0.5, &[])
            .unwrap();
        let row = storage
            .conclusion_by_id(&cid)
            .unwrap()
            .expect("must find by id");
        assert_eq!(row.id, cid);
        assert_eq!(row.subject, "user");
        assert_eq!(row.statement, "claim");
    }

    /// `find_conclusion_ids_by_prefix` supports the short-id CLI
    /// pattern (mirror of session prefix lookup). Returns up to 5
    /// matches; ambiguous prefixes can be surfaced rather than
    /// silently picked. Rejects non-hex/dash chars defensively.
    #[test]
    fn find_conclusion_ids_by_prefix_matches_and_rejects_garbage() {
        let storage = Storage::open(&tmp_db()).unwrap();
        let cid = storage
            .add_conclusion("user", "pattern", "claim", 0.5, &[])
            .unwrap();

        // Full id matches.
        let hits = storage.find_conclusion_ids_by_prefix(&cid).unwrap();
        assert_eq!(hits, vec![cid.clone()]);

        // 8-char prefix matches.
        let prefix = &cid[..8];
        let hits = storage.find_conclusion_ids_by_prefix(prefix).unwrap();
        assert!(hits.contains(&cid), "8-char prefix must find the row");

        // Unknown prefix returns empty (no error).
        let hits = storage
            .find_conclusion_ids_by_prefix("ffffffff-ffff-ffff")
            .unwrap();
        assert!(hits.is_empty());

        // Non-hex chars rejected loudly — defensive against
        // injection-shaped input. Mirrors the session helper.
        assert!(storage.find_conclusion_ids_by_prefix("not-hex!").is_err());
    }

    /// Trigger-based `support_count` maintenance: after every INSERT and
    /// DELETE on `conclusion_sources` the cached count on the parent
    /// conclusion row must equal the real `COUNT(*)` of links. This
    /// closes the drift Codex caught — previously the count was set
    /// once at insert time and never updated again.
    #[test]
    fn support_count_trigger_stays_in_sync_with_link_table() {
        let storage = Storage::open(&tmp_db()).unwrap();
        let m1 = make_entry("m1");
        let m2 = make_entry("m2");
        let m3 = make_entry("m3");
        storage.save(&m1).unwrap();
        storage.save(&m2).unwrap();
        storage.save(&m3).unwrap();

        // Start with 2 links.
        let cid = storage
            .add_conclusion(
                "user",
                "pattern",
                "uses many tools",
                0.5,
                &[m1.id.clone(), m2.id.clone()],
            )
            .unwrap();
        let after_insert = storage
            .current_conclusions_for_subject("user")
            .unwrap()
            .into_iter()
            .find(|c| c.id == cid)
            .unwrap();
        assert_eq!(
            after_insert.support_count, 2,
            "trigger fired on initial inserts"
        );

        // Cascade-delete one link by forgetting its memory; count drops.
        storage.forget_by_id(&m2.id).unwrap();
        let after_cascade = storage
            .current_conclusions_for_subject("user")
            .unwrap()
            .into_iter()
            .find(|c| c.id == cid)
            .unwrap();
        assert_eq!(after_cascade.support_count, 1, "trigger fired on cascade");

        // Cascade-delete the conclusion itself — link table entries
        // disappear, but the conclusion row is also gone so we don't
        // need to verify the count. We're really checking the trigger
        // doesn't crash on cascade-from-parent.
        storage.forget_by_id(&m1.id).unwrap();
        let cleaned = storage
            .current_conclusions_for_subject("user")
            .unwrap()
            .into_iter()
            .find(|c| c.id == cid)
            .unwrap();
        assert_eq!(
            cleaned.support_count, 0,
            "all evidence gone, cache reflects it"
        );

        // Add a new link to a fresh memory; trigger increments.
        let cid2 = storage
            .add_conclusion(
                "user",
                "pattern",
                "another",
                0.5,
                std::slice::from_ref(&m3.id),
            )
            .unwrap();
        let with_one = storage
            .current_conclusions_for_subject("user")
            .unwrap()
            .into_iter()
            .find(|c| c.id == cid2)
            .unwrap();
        assert_eq!(with_one.support_count, 1);
    }

    /// Defensive dedup: passing the same entity name twice to `replace_graph`
    /// must NOT double-bump `mention_count`. Storage is the chokepoint
    /// where every extractor's output lands; this guarantee can't depend
    /// on callers behaving.
    #[test]
    fn replace_graph_dedupes_duplicate_entities_within_one_call() {
        use crate::graph::{Entity, EntityType};
        let storage = Storage::open(&tmp_db()).unwrap();
        let mem = make_entry("m1");
        storage.save(&mem).unwrap();

        // Same name three times — a misbehaving extractor.
        let dupes = vec![
            Entity {
                name: "alpha".into(),
                entity_type: EntityType::Concept,
            },
            Entity {
                name: "alpha".into(),
                entity_type: EntityType::Concept,
            },
            Entity {
                name: "alpha".into(),
                entity_type: EntityType::Concept,
            },
        ];
        storage.replace_graph(&mem.id, &dupes, &[]).unwrap();
        assert_eq!(
            storage.graph_query("alpha").unwrap().mention_count,
            1,
            "three identical entities in one extraction must collapse to one mention"
        );
    }

    /// Same defensive dedup for edges: repeated (source, target, relation)
    /// within one call must not inflate edge weight either.
    #[test]
    fn replace_graph_dedupes_duplicate_edges_within_one_call() {
        use crate::graph::{Edge, Entity, EntityType};
        let storage = Storage::open(&tmp_db()).unwrap();
        let mem = make_entry("m1");
        storage.save(&mem).unwrap();

        let entities = [
            Entity {
                name: "a".into(),
                entity_type: EntityType::Concept,
            },
            Entity {
                name: "b".into(),
                entity_type: EntityType::Concept,
            },
        ];
        // Three copies of the same edge.
        let mk_edge = || Edge {
            source: "a".into(),
            target: "b".into(),
            relation: "uses".into(),
            memory_id: mem.id.clone(),
        };
        let dupes = vec![mk_edge(), mk_edge(), mk_edge()];
        storage.replace_graph(&mem.id, &entities, &dupes).unwrap();
        let g = storage.graph_query("a").unwrap();
        // Exactly one edge in either direction.
        assert_eq!(
            g.edges.len(),
            1,
            "duplicate edges must collapse: {:?}",
            g.edges
        );
        // Weight should be the baseline 1.0 — UPDATE branch fires for OTHER
        // memory_ids only, so 3 identical edges from the same memory don't
        // bump weight even without our dedup. But the test still pins the
        // edge count to exactly 1.
    }

    /// Empty-name entities must be silently dropped — meaningless graph
    /// nodes (canonicalization upstream usually filters them, but storage
    /// is the chokepoint).
    #[test]
    fn replace_graph_drops_empty_entity_names() {
        use crate::graph::{Entity, EntityType};
        let storage = Storage::open(&tmp_db()).unwrap();
        let mem = make_entry("m1");
        storage.save(&mem).unwrap();

        let mixed = [
            Entity {
                name: "".into(),
                entity_type: EntityType::Concept,
            },
            Entity {
                name: "real".into(),
                entity_type: EntityType::Concept,
            },
            Entity {
                name: "   ".into(), // whitespace-only also empty after trim
                entity_type: EntityType::Concept,
            },
        ];
        storage.replace_graph(&mem.id, &mixed, &[]).unwrap();
        assert!(storage.graph_query("real").unwrap().found);
        assert!(
            !storage.graph_query("").unwrap().found,
            "empty entity name should not be stored"
        );
    }

    /// Atomicity regression: a failure inside replace_graph's transaction
    /// must roll the whole thing back, leaving the memory's pre-call graph
    /// state intact. Otherwise a partial commit would wipe existing edges
    /// without saving new ones — silent enrichment loss.
    ///
    /// Failure injection: open a second SQLite connection, take an
    /// IMMEDIATE write lock, and pin `busy_timeout = 0` on the storage
    /// connection so it fails fast instead of waiting. replace_graph's
    /// very first UPDATE/DELETE hits SQLITE_BUSY, `?` bubbles up, the
    /// transaction is dropped without commit, SQLite rolls back. No
    /// test-only seam in the production code — the failure is induced
    /// entirely from the outside.
    #[test]
    fn replace_graph_rolls_back_when_phase_2_fails() {
        use crate::graph::{Entity, EntityType};

        let path = tmp_db();
        let storage = Storage::open(&path).unwrap();
        let mem = make_entry("m1");
        storage.save(&mem).unwrap();

        // Seed: memory M links to "kept-entity" (count=1).
        let seed = [Entity {
            name: "kept-entity".into(),
            entity_type: EntityType::Concept,
        }];
        storage.replace_graph(&mem.id, &seed, &[]).unwrap();
        let count_before = storage.graph_query("kept-entity").unwrap().mention_count;
        assert_eq!(count_before, 1);

        // Lock the DB from a second connection with an IMMEDIATE write
        // lock that won't release. The original storage's replace_graph
        // will hit SQLITE_BUSY on its first INSERT/UPDATE because the
        // pragma busy_timeout fires the failure path.
        let lock_conn = rusqlite::Connection::open(&path).unwrap();
        lock_conn
            .execute_batch("PRAGMA busy_timeout = 0; BEGIN IMMEDIATE;")
            .unwrap();
        // Touch a row inside that BEGIN so it actually holds a write lock,
        // not just a reserved one.
        lock_conn
            .execute(
                "INSERT INTO memories (id, timestamp, title, content, memory_type,
                    tags, source, importance, metadata)
                 VALUES (?1, datetime('now'), 'lock-holder', '', 'note', '[]', '\"Manual\"', 0.1, 'null')",
                rusqlite::params![uuid::Uuid::new_v4().to_string()],
            )
            .unwrap();

        // Set storage's busy_timeout to 0 too so it fails fast instead of
        // waiting. Without this, the test would block for the storage
        // default timeout (5s) on every conflicting write.
        storage
            .conn
            .lock()
            .unwrap()
            .execute_batch("PRAGMA busy_timeout = 0;")
            .unwrap();

        // Now attempt the replace. Phase 1's DELETEs / UPDATEs will hit
        // the lock and the function should return Err.
        let new_entities = [Entity {
            name: "would-have-replaced".into(),
            entity_type: EntityType::Concept,
        }];
        let res = storage.replace_graph(&mem.id, &new_entities, &[]);
        assert!(res.is_err(), "expected SQLITE_BUSY error, got {res:?}");

        // Release the lock so the verification queries work.
        lock_conn.execute_batch("ROLLBACK;").unwrap();
        // Restore busy_timeout on the main connection too (other tests
        // share the process, even if they have separate DB files).
        storage
            .conn
            .lock()
            .unwrap()
            .execute_batch("PRAGMA busy_timeout = 5000;")
            .unwrap();

        // The CRITICAL assertion: pre-call state must be intact. If
        // replace_graph had committed phase 1 before phase 2 failed,
        // "kept-entity" would be gone or its count would have decremented
        // permanently. Atomic tx → rollback → pre-call state preserved.
        let kept = storage.graph_query("kept-entity").unwrap();
        assert!(
            kept.found,
            "kept-entity vanished — phase 1 committed without phase 2"
        );
        assert_eq!(
            kept.mention_count, count_before,
            "kept-entity mention_count drifted from {count_before} to {} — partial commit",
            kept.mention_count
        );
        // And the new entity must not exist either.
        assert!(
            !storage.graph_query("would-have-replaced").unwrap().found,
            "phase 2 was never supposed to succeed, but its entity exists"
        );
    }

    /// `replace_graph` must be idempotent: calling it twice with the same
    /// (entities, edges) for the same memory must leave mention_count
    /// equal to its single-call value, not double. This is the core
    /// invariant the async worker depends on.
    #[test]
    fn replace_graph_is_idempotent_on_mention_count() {
        use crate::graph::{Entity, EntityType};
        let path = tmp_db();
        let storage = Storage::open(&path).unwrap();
        let mem = make_entry("m1");
        storage.save(&mem).unwrap();

        let entities = vec![Entity {
            name: "alpha".into(),
            entity_type: EntityType::Project,
        }];
        let edges: Vec<crate::graph::Edge> = vec![];

        storage.replace_graph(&mem.id, &entities, &edges).unwrap();
        let count_once = storage.graph_query("alpha").unwrap().mention_count;
        assert_eq!(
            count_once, 1,
            "first replace_graph should register one mention"
        );

        // Call again with identical input. Without the decrement step
        // inside replace_graph, the entity's mention_count would jump to 2.
        storage.replace_graph(&mem.id, &entities, &edges).unwrap();
        let count_twice = storage.graph_query("alpha").unwrap().mention_count;
        assert_eq!(
            count_twice, 1,
            "second replace_graph must not inflate mention_count (got {count_twice})"
        );

        // And a third time with a DIFFERENT entity set should drop the
        // first entity to zero and bring beta to 1.
        let new_entities = vec![Entity {
            name: "beta".into(),
            entity_type: EntityType::Project,
        }];
        storage
            .replace_graph(&mem.id, &new_entities, &edges)
            .unwrap();
        let alpha_after = storage.graph_query("alpha").unwrap();
        let beta_after = storage.graph_query("beta").unwrap();
        assert_eq!(
            alpha_after.mention_count, 0,
            "alpha should drop to 0 after re-extraction picked a different entity set"
        );
        assert_eq!(beta_after.mention_count, 1);
    }

    #[test]
    fn merge_records_alias() {
        use crate::graph::{Entity, EntityType};

        let path = tmp_db();
        let storage = Storage::open(&path).unwrap();
        let mem = make_entry("m1");
        storage.save(&mem).unwrap();

        storage
            .save_graph(
                &mem.id,
                &[
                    Entity {
                        name: "acme-devices".into(),
                        entity_type: EntityType::Person,
                    },
                    Entity {
                        name: "acme-devices-co".into(),
                        entity_type: EntityType::Person,
                    },
                ],
                &[],
            )
            .unwrap();

        storage
            .merge_entities("acme-devices", "acme-devices-co")
            .unwrap();

        let conn = storage.conn.lock().unwrap();
        let alias_rows: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM entity_aliases WHERE alias = ?1 AND canonical = ?2",
                params!["acme-devices-co", "acme-devices"],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(alias_rows, 1);
    }

    #[test]
    fn canonical_for_alias_returns_canonical() {
        use crate::graph::{Entity, EntityType};

        let path = tmp_db();
        let storage = Storage::open(&path).unwrap();
        let mem = make_entry("m1");
        storage.save(&mem).unwrap();

        storage
            .save_graph(
                &mem.id,
                &[
                    Entity {
                        name: "acme-devices".into(),
                        entity_type: EntityType::Person,
                    },
                    Entity {
                        name: "acme-devices-co".into(),
                        entity_type: EntityType::Person,
                    },
                ],
                &[],
            )
            .unwrap();

        storage
            .merge_entities("acme-devices", "acme-devices-co")
            .unwrap();

        assert_eq!(
            storage.canonical_for_alias("ACME-DEVICES-CO").unwrap(),
            Some("acme-devices".to_string())
        );
        assert_eq!(storage.canonical_for_alias("acme-devices").unwrap(), None);
    }

    #[test]
    fn chain_merge_redirects_old_aliases() {
        use crate::graph::{Entity, EntityType};

        let path = tmp_db();
        let storage = Storage::open(&path).unwrap();
        let mem = make_entry("m1");
        storage.save(&mem).unwrap();

        storage
            .save_graph(
                &mem.id,
                &[
                    Entity {
                        name: "a".into(),
                        entity_type: EntityType::Concept,
                    },
                    Entity {
                        name: "b".into(),
                        entity_type: EntityType::Concept,
                    },
                    Entity {
                        name: "c".into(),
                        entity_type: EntityType::Concept,
                    },
                ],
                &[],
            )
            .unwrap();

        storage.merge_entities("b", "a").unwrap();
        assert_eq!(storage.canonical_for_alias("a").unwrap(), Some("b".into()));

        storage.merge_entities("c", "b").unwrap();
        assert_eq!(storage.canonical_for_alias("a").unwrap(), Some("c".into()));
        assert_eq!(storage.canonical_for_alias("b").unwrap(), Some("c".into()));

        let conn = storage.conn.lock().unwrap();
        let stale_rows: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM entity_aliases WHERE canonical = ?1",
                params!["b"],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(stale_rows, 0);
    }

    #[test]
    fn graph_query_returns_aliases() {
        use crate::graph::{Entity, EntityType};

        let path = tmp_db();
        let storage = Storage::open(&path).unwrap();
        let mem = make_entry("m1");
        storage.save(&mem).unwrap();

        storage
            .save_graph(
                &mem.id,
                &[
                    Entity {
                        name: "acme-devices".into(),
                        entity_type: EntityType::Person,
                    },
                    Entity {
                        name: "acme-devices-co".into(),
                        entity_type: EntityType::Person,
                    },
                    Entity {
                        name: "client-acme-devices".into(),
                        entity_type: EntityType::Person,
                    },
                ],
                &[],
            )
            .unwrap();

        storage
            .merge_entities("acme-devices", "acme-devices-co")
            .unwrap();
        storage
            .merge_entities("acme-devices", "client-acme-devices")
            .unwrap();

        let graph = storage.graph_query("acme-devices").unwrap();
        assert!(graph.found);
        // SQLite doesn't guarantee order without ORDER BY — compare as a
        // sorted set so the test stays stable regardless of insertion order
        // or merged_at timestamps colliding.
        let mut got = graph.aliases.clone();
        got.sort();
        assert_eq!(
            got,
            vec![
                "acme-devices-co".to_string(),
                "client-acme-devices".to_string()
            ]
        );
    }

    /// merge on a non-existent alias is a silent no-op.
    #[test]
    fn merge_entities_missing_alias_is_noop() {
        let path = tmp_db();
        let storage = Storage::open(&path).unwrap();
        let report = storage.merge_entities("foo", "ghost").unwrap();
        assert!(!report.alias_dropped);
    }

    /// rename_entity must move the row and rewrite edge endpoints.
    #[test]
    fn rename_entity_updates_edges() {
        use crate::graph::{Edge, Entity, EntityType};

        let path = tmp_db();
        let storage = Storage::open(&path).unwrap();
        let mem = make_entry("m1");
        storage.save(&mem).unwrap();
        storage
            .save_graph(
                &mem.id,
                &[Entity {
                    name: "old-name".into(),
                    entity_type: EntityType::Project,
                }],
                &[Edge {
                    source: "old-name".into(),
                    target: "rust".into(),
                    relation: "uses".into(),
                    memory_id: mem.id.clone(),
                }],
            )
            .unwrap();

        assert!(storage.rename_entity("old-name", "new-name").unwrap());
        assert!(storage.graph_query("new-name").unwrap().found);
        assert!(!storage.graph_query("old-name").unwrap().found);
    }

    /// touch_access must bump access_count and set last_accessed_at,
    /// and recent_ranked must surface those stats so callers can compute
    /// effective scores without a second round-trip.
    #[test]
    fn touch_access_bumps_count_and_timestamp() {
        let path = tmp_db();
        let storage = Storage::open(&path).unwrap();
        let entry = make_entry("touch-target");
        storage.save(&entry).unwrap();

        // Before: never touched.
        let before = storage.recent_ranked(10).unwrap();
        let r0 = before.iter().find(|r| r.entry.id == entry.id).unwrap();
        assert_eq!(r0.access_count, 0);
        // last_active falls back to timestamp when never accessed.
        assert_eq!(r0.last_active, r0.entry.timestamp);

        // Touch twice.
        storage.touch_access(&[entry.id.as_str()]).unwrap();
        storage.touch_access(&[entry.id.as_str()]).unwrap();

        let after = storage.recent_ranked(10).unwrap();
        let r1 = after.iter().find(|r| r.entry.id == entry.id).unwrap();
        assert_eq!(r1.access_count, 2, "two touches should yield count=2");
        assert!(
            r1.last_active >= r1.entry.timestamp,
            "last_active must be >= original timestamp"
        );
    }

    /// touch_access on a non-existent id must not error — UPDATE with no
    /// matching row is a silent no-op in SQLite, which is what we want.
    #[test]
    fn touch_access_missing_id_is_noop() {
        let path = tmp_db();
        let storage = Storage::open(&path).unwrap();
        assert!(storage.touch_access(&["does-not-exist"]).is_ok());
        assert!(storage.touch_access(&[]).is_ok());
    }

    /// search() must touch every returned hit so frequently-searched
    /// memories rise in effective-score ranking.
    #[test]
    fn search_touches_returned_entries() {
        let path = tmp_db();
        let storage = Storage::open(&path).unwrap();
        // No dashes — FTS5 treats `-` as a NOT operator.
        let entry = make_entry("findme zorbleforge");
        storage.save(&entry).unwrap();

        let hits = storage.search("zorbleforge", 5).unwrap();
        assert!(!hits.is_empty(), "FTS5 should match the term");

        let ranked = storage.recent_ranked(10).unwrap();
        let r = ranked.iter().find(|r| r.entry.id == entry.id).unwrap();
        assert!(
            r.access_count >= 1,
            "search hit should bump access_count, got {}",
            r.access_count
        );
    }

    /// Regression: `search_no_touch` must NOT bump access_count.
    /// Existed first as eval-side concern (re-running eval shouldn't shift
    /// production rankings); this guards the contract from future refactor.
    #[test]
    fn search_no_touch_does_not_bump_access_count() {
        let path = tmp_db();
        let storage = Storage::open(&path).unwrap();
        let entry = make_entry("findme zorbleforge");
        storage.save(&entry).unwrap();

        let before = storage
            .recent_ranked(10)
            .unwrap()
            .iter()
            .find(|r| r.entry.id == entry.id)
            .unwrap()
            .access_count;

        // Hit it 3x via no_touch — each call returns the entry but must
        // not write to access_count or last_accessed_at.
        for _ in 0..3 {
            let hits = storage.search_no_touch("zorbleforge", 5).unwrap();
            assert!(!hits.is_empty(), "FTS5 should match");
        }

        let after = storage
            .recent_ranked(10)
            .unwrap()
            .iter()
            .find(|r| r.entry.id == entry.id)
            .unwrap()
            .access_count;
        assert_eq!(
            before, after,
            "search_no_touch must leave access_count untouched; before={before}, after={after}"
        );
    }

    /// Sibling regression for the vector path. Different code path
    /// (HNSW vs FTS) — different chance to forget the no-touch guarantee
    /// in a refactor. Uses a hand-rolled embedding (not the ONNX model)
    /// so the test stays fast and offline.
    #[test]
    fn find_similar_no_touch_does_not_bump_access_count() {
        let path = tmp_db();
        let storage = Storage::open(&path).unwrap();
        let entry = make_entry("zorbleforge target");

        // Deterministic synthetic embedding — dimensionality matches what
        // the embedder produces (384). All ones, normalized. We're not
        // testing relevance ranking here, just whether find_similar
        // bumps access bookkeeping.
        let dim = 384usize;
        let mut emb = vec![1.0_f32 / (dim as f32).sqrt(); dim];
        // Add tiny perturbation so HNSW indexes it as distinct from a
        // hypothetical zero vector.
        emb[0] += 1e-3;
        storage.save_with_embedding(&entry, Some(&emb)).unwrap();

        let before = storage
            .recent_ranked(10)
            .unwrap()
            .iter()
            .find(|r| r.entry.id == entry.id)
            .map(|r| r.access_count)
            .unwrap_or(0);

        for _ in 0..3 {
            let _ = storage.find_similar_no_touch(&emb, 5).unwrap();
        }

        let after = storage
            .recent_ranked(10)
            .unwrap()
            .iter()
            .find(|r| r.entry.id == entry.id)
            .map(|r| r.access_count)
            .unwrap_or(0);
        assert_eq!(
            before, after,
            "find_similar_no_touch must leave access_count untouched; \
             before={before}, after={after}"
        );
    }

    /// After an embedding-model swap (dimension change) without a reembed, a
    /// query whose dimension differs from the stored vectors must fail loudly
    /// instead of silently scoring every old row 0 — which would gut search
    /// and (via `is_duplicate`) dedup. See `Storage::guard_query_dim`.
    #[test]
    fn mismatched_query_dimension_is_rejected() {
        let path = tmp_db();
        let storage = Storage::open(&path).unwrap();
        let entry = make_entry("dimension guard target");

        let dim = 384usize;
        let mut emb = vec![1.0_f32 / (dim as f32).sqrt(); dim];
        emb[0] += 1e-3;
        storage.save_with_embedding(&entry, Some(&emb)).unwrap();

        // The store reports exactly the one active dimension.
        assert_eq!(storage.active_embedding_dims(), vec![dim]);

        // Same dimension as the stored vectors: both paths work.
        assert!(storage.find_similar_no_touch(&emb, 5).is_ok());
        assert!(storage.is_duplicate(&emb, 0.9).is_ok());

        // Model swapped without reembed (768 vs stored 384): rejected loudly.
        let wrong = vec![0.1_f32; 768];
        assert!(
            storage.find_similar_no_touch(&wrong, 5).is_err(),
            "find_similar must reject a mismatched-dimension query"
        );
        assert!(
            storage.is_duplicate(&wrong, 0.9).is_err(),
            "is_duplicate must reject a mismatched-dimension query"
        );
    }

    /// A reader opening the DB while a writer holds the lock must not hang.
    /// This is the exact scenario that caused the CLI to freeze in the
    /// original bug (daemon writing batch + user running `mnemonic query ...`).
    #[test]
    fn reader_does_not_hang_during_writer() {
        let path = Arc::new(tmp_db());
        let writer = Arc::new(Storage::open(&path).unwrap());

        // Saturate the writer.
        let writer_path = path.clone();
        let writer_thread = {
            let s = writer.clone();
            thread::spawn(move || {
                for i in 0..50 {
                    s.save(&make_entry(&format!("w-{i}"))).unwrap();
                }
                drop(writer_path);
            })
        };

        // Meanwhile a "CLI" process opens the same DB and reads.
        let reader_start = Instant::now();
        let reader = Storage::open(&path).unwrap();
        let _ = reader.recent(10).unwrap();
        let reader_elapsed = reader_start.elapsed();

        writer_thread.join().unwrap();

        assert!(
            reader_elapsed < Duration::from_secs(5),
            "reader took {reader_elapsed:?} — should complete quickly under WAL"
        );
    }

    /// P0 regression: a forgotten memory's HNSW vector must stop answering
    /// dedup probes immediately. Before tombstoning, the ghost survived
    /// until the next restart and `is_duplicate` made the daemon silently
    /// drop any re-save of similar content — "forget, save corrected
    /// version, corrected version lost".
    #[test]
    fn forget_evicts_vector_so_resave_is_not_deduped() {
        let path = tmp_db();
        let storage = Storage::open(&path).unwrap();

        let dim = 384usize;
        let mut emb_a = vec![0.0_f32; dim];
        emb_a[0] = 1.0;
        let mut emb_b = vec![0.0_f32; dim];
        emb_b[1] = 1.0;

        let a = make_entry("forget-me alpha");
        let b = make_entry("keeper beta");
        storage.save_with_embedding(&a, Some(&emb_a)).unwrap();
        storage.save_with_embedding(&b, Some(&emb_b)).unwrap();

        // Sanity: while A is alive, identical content IS a duplicate.
        assert!(storage.is_duplicate(&emb_a, 0.95).unwrap().is_some());

        assert!(storage.forget_by_id(&a.id).unwrap());

        // The ghost must not answer dedup probes anymore…
        assert!(
            storage.is_duplicate(&emb_a, 0.95).unwrap().is_none(),
            "forgotten memory still answers dedup — re-saves would be silently dropped"
        );
        // …or eat result slots in similarity search.
        let hits = storage.find_similar_no_touch(&emb_a, 2).unwrap();
        assert!(hits.iter().all(|(e, _)| e.id != a.id));
        assert!(hits.iter().any(|(e, _)| e.id == b.id));
    }

    /// Same P0 from the other side: deletes can happen in ANOTHER process
    /// (CLI `mnemonic forget`, MCP server) — this process's in-memory HNSW
    /// never hears about them. The dedup gate must verify candidates
    /// against SQLite instead of trusting a ghost vector.
    #[test]
    fn cross_process_delete_does_not_dedup_new_saves() {
        let path = tmp_db();
        let storage = Storage::open(&path).unwrap();
        let dim = 384usize;
        let mut emb = vec![0.0_f32; dim];
        emb[5] = 1.0;
        let a = make_entry("ghost candidate");
        storage.save_with_embedding(&a, Some(&emb)).unwrap();
        assert!(storage.is_duplicate(&emb, 0.95).unwrap().is_some());

        // Delete behind the index's back — simulates another process.
        {
            let conn = storage.conn.lock().unwrap();
            conn.execute("DELETE FROM memories WHERE id = ?1", params![a.id])
                .unwrap();
        }

        assert!(
            storage.is_duplicate(&emb, 0.95).unwrap().is_none(),
            "ghost HNSW hit must be verified against SQLite"
        );
    }

    /// Recall regression: ghosts must not shrink the result set. 12 live +
    /// 8 superseded behind the index's back; asking for 10 must return 10
    /// live memories, not 10-minus-ghosts.
    #[test]
    fn ghosts_do_not_shrink_recall() {
        let path = tmp_db();
        let storage = Storage::open(&path).unwrap();
        let dim = 384usize;
        let mut ids = Vec::new();
        for i in 0..20 {
            let mut emb = vec![0.0_f32; dim];
            emb[i] = 1.0;
            emb[63] = 0.5; // shared component so one query matches them all
            let e = make_entry(&format!("recall-{i}"));
            storage.save_with_embedding(&e, Some(&emb)).unwrap();
            ids.push(e.id.clone());
        }
        {
            let conn = storage.conn.lock().unwrap();
            for id in ids.iter().take(8) {
                conn.execute(
                    "UPDATE memories SET superseded_by = 'x' WHERE id = ?1",
                    params![id],
                )
                .unwrap();
            }
        }
        let mut q = vec![0.0_f32; dim];
        q[63] = 1.0;
        let hits = storage.find_similar_no_touch(&q, 10).unwrap();
        assert_eq!(hits.len(), 10, "ghosts ate result slots");
        assert!(hits.iter().all(|(e, _)| !ids[..8].contains(&e.id)));
    }

    /// Idle sweep: sessions whose key never fires again must still get
    /// closed once `last_activity_at + idle_timeout` passes — dream
    /// consolidation and session summaries only consume CLOSED sessions.
    #[test]
    fn close_idle_sessions_sweeps_stale_open_sessions() {
        let path = tmp_db();
        let storage = Storage::open(&path).unwrap();
        let peer = storage.upsert_peer("sweeper", None, "agent").unwrap();
        let sid = storage.open_session(&peer, Some("s"), "test").unwrap();

        // Fresh session: sweep must not touch it.
        assert_eq!(storage.close_idle_sessions(600).unwrap(), 0);

        // Backdate activity by 2h — simulates a session orphaned by a
        // daemon restart whose JSONL never gets another event.
        {
            let conn = storage.conn.lock().unwrap();
            conn.execute(
                "UPDATE sessions SET last_activity_at = datetime('now', '-7200 seconds')
                 WHERE id = ?1",
                params![sid],
            )
            .unwrap();
        }

        assert_eq!(storage.close_idle_sessions(600).unwrap(), 1);
        // Idempotent: already-closed sessions are left alone.
        assert_eq!(storage.close_idle_sessions(600).unwrap(), 0);

        let s = storage.session_by_id(&sid).unwrap().unwrap();
        let ended = s.ended_at.expect("session must be closed");
        let ended = chrono::NaiveDateTime::parse_from_str(&ended, "%Y-%m-%d %H:%M:%S")
            .unwrap()
            .and_utc();
        // ended_at = last_activity + idle (not "now") — matches the
        // SessionTracker idle-expiry convention.
        let expected = chrono::Utc::now() - chrono::Duration::seconds(7200 - 600);
        let drift = (ended - expected).num_seconds().abs();
        assert!(
            drift < 15,
            "ended_at must be last_activity + idle, drift {drift}s"
        );
    }

    /// The legacy broad AFTER UPDATE FTS trigger must be rebuilt to the
    /// narrow AFTER UPDATE OF form on open — but only when it IS legacy.
    /// (The first version dropped+recreated unconditionally, which made
    /// every open take a write lock and flake against concurrent writers.)
    #[test]
    fn legacy_broad_fts_trigger_is_healed_on_open() {
        let path = tmp_db();
        {
            let storage = Storage::open(&path).unwrap();
            let conn = storage.conn.lock().unwrap();
            conn.execute_batch(
                "DROP TRIGGER memories_au;
                 CREATE TRIGGER memories_au AFTER UPDATE ON memories BEGIN
                    INSERT INTO memories_fts(memories_fts, rowid, title, content, tags)
                    VALUES ('delete', old.rowid, old.title, old.content, old.tags);
                    INSERT INTO memories_fts(rowid, title, content, tags)
                    VALUES (new.rowid, new.title, new.content, new.tags);
                 END;",
            )
            .unwrap();
        }
        let storage = Storage::open(&path).unwrap();
        let conn = storage.conn.lock().unwrap();
        let sql: String = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'trigger' AND name = 'memories_au'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            sql.contains("AFTER UPDATE OF"),
            "legacy trigger must be rebuilt on open, got: {sql}"
        );
    }
}
