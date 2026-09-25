//! Shared store schema and its forward-only migrations.
//!
//! Version 2 adds what two independent people need on one project: who
//! published a record and which observation it came from, which person an
//! observation belongs to, revocable access per person, and an optional pin
//! that binds the database to a single project. Migration only adds columns
//! and tables, so every version 1 row and the revision counter survive. An
//! older binary refuses a version 2 file: it fails closed.

use anyhow::{Result, ensure};
use rusqlite::Transaction;

pub(super) const APPLICATION_ID: i64 = 0x4d4e5348; // MNSH; never attach this schema to private memory.db.
pub(super) const SCHEMA_VERSION: i64 = 2;

const V1: &str = "CREATE TABLE IF NOT EXISTS shared_projects (
        project_id TEXT PRIMARY KEY, revision INTEGER NOT NULL DEFAULT 0
            CHECK (revision >= 0)
     );
     CREATE TABLE IF NOT EXISTS shared_records (
        project_id TEXT NOT NULL REFERENCES shared_projects(project_id),
        key TEXT NOT NULL, title TEXT NOT NULL, content TEXT NOT NULL,
        source TEXT NOT NULL, revision INTEGER NOT NULL CHECK (revision > 0),
        updated_at TEXT NOT NULL, PRIMARY KEY (project_id, key)
     );
     CREATE TABLE IF NOT EXISTS shared_observations (
        id TEXT PRIMARY KEY, project_id TEXT NOT NULL
            REFERENCES shared_projects(project_id),
        writer_id TEXT NOT NULL, request_id TEXT NOT NULL,
        title TEXT NOT NULL, content TEXT NOT NULL, source TEXT NOT NULL,
        status TEXT NOT NULL DEFAULT 'pending'
            CHECK (status IN ('pending', 'reviewed', 'rejected')),
        created_at TEXT NOT NULL,
        UNIQUE (project_id, writer_id, request_id)
     );
     CREATE INDEX IF NOT EXISTS shared_record_order
        ON shared_records(project_id, revision DESC);
     CREATE INDEX IF NOT EXISTS shared_observation_inbox
        ON shared_observations(project_id, status, created_at);";

const V2: &str = "ALTER TABLE shared_records ADD COLUMN published_by TEXT;
     ALTER TABLE shared_records ADD COLUMN origin_observation_id TEXT;
     ALTER TABLE shared_records ADD COLUMN origin_writer_id TEXT;
     ALTER TABLE shared_observations ADD COLUMN principal_id TEXT;
     ALTER TABLE shared_observations ADD COLUMN reviewed_by TEXT;
     ALTER TABLE shared_observations ADD COLUMN reviewed_at TEXT;
     ALTER TABLE shared_observations ADD COLUMN promoted_key TEXT;
     ALTER TABLE shared_observations ADD COLUMN promoted_revision INTEGER;
     CREATE TABLE shared_revocations (
        project_id TEXT NOT NULL, principal_id TEXT NOT NULL,
        revoked_at TEXT NOT NULL, revoked_by TEXT,
        PRIMARY KEY (project_id, principal_id)
     );
     CREATE TABLE shared_meta (
        id INTEGER PRIMARY KEY CHECK (id = 1), pinned_project TEXT NOT NULL
     );";

/// Create or upgrade the schema inside the caller's write transaction.
pub(super) fn ensure_schema(tx: &Transaction<'_>) -> Result<()> {
    let app_id: i64 = tx.pragma_query_value(None, "application_id", |r| r.get(0))?;
    let version: i64 = tx.pragma_query_value(None, "user_version", |r| r.get(0))?;
    let objects: i64 = tx.query_row(
        "SELECT count(*) FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'",
        [],
        |r| r.get(0),
    )?;
    let fresh = app_id == 0 && version == 0 && objects == 0;
    ensure!(
        fresh || (app_id == APPLICATION_ID && (1..=SCHEMA_VERSION).contains(&version)),
        "refusing unrelated database or unsupported shared schema version"
    );
    if fresh {
        tx.execute_batch(V1)?;
    }
    if fresh || version == 1 {
        tx.execute_batch(V2)?;
    }
    tx.pragma_update(None, "application_id", APPLICATION_ID)?;
    tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::store::SharedStore;
    use super::super::types::Expected;
    use super::*;
    use rusqlite::Connection;

    fn private_dir(tag: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("mnemonic-schema-{tag}-{}", uuid::Uuid::new_v4()));
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&dir).unwrap();
        dir
    }

    fn write_version(path: &std::path::Path, version: i64) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(V1).unwrap();
        conn.execute_batch(
            "INSERT INTO shared_projects(project_id, revision) VALUES ('alpha', 7);
             INSERT INTO shared_records VALUES
                ('alpha', 'brief', 'Brief', 'kept text', 'owner:v1', 7, '2026-09-01T00:00:00Z');
             INSERT INTO shared_observations VALUES
                ('obs-1', 'alpha', 'scout', 'request-1', 'T', 'old draft', 's:1', 'pending',
                 '2026-09-01T00:00:00Z');",
        )
        .unwrap();
        conn.pragma_update(None, "application_id", APPLICATION_ID)
            .unwrap();
        conn.pragma_update(None, "user_version", version).unwrap();
        drop(conn);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
    }

    #[test]
    fn version_one_database_upgrades_in_place_and_keeps_everything() {
        let dir = private_dir("v1");
        let path = dir.join("shared.db");
        write_version(&path, 1);

        let store = SharedStore::open(&path).unwrap();
        let record = store.get("alpha", "brief").unwrap().unwrap();
        assert_eq!((record.content.as_str(), record.revision), ("kept text", 7));
        assert_eq!(record.published_by, None);
        let inbox = store.inbox("alpha", 10).unwrap();
        assert_eq!(inbox[0].principal_id, None);

        // The revision counter continues, and version 2 operations work on old rows.
        let promoted = store
            .promote("alpha", "obs-1", "draft", Expected::Absent, "ann")
            .unwrap();
        assert_eq!(promoted.revision, 8);
        drop(store);
        // Opening again is a no-op migration.
        assert!(
            SharedStore::open(&path)
                .unwrap()
                .get("alpha", "draft")
                .unwrap()
                .is_some()
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_migrated_row_cannot_publish_text_a_direct_publish_would_refuse() {
        let dir = private_dir("legacy");
        let path = dir.join("shared.db");
        write_version(&path, 1);
        // Written before the rule existed: an invisible tag character that a
        // human reviewing the inbox cannot see, but a model reads.
        let conn = Connection::open(&path).unwrap();
        conn.execute(
            "UPDATE shared_observations SET content = ?1 WHERE id = 'obs-1'",
            ["ship it\u{e0041}\u{e0042} on Friday"],
        )
        .unwrap();
        drop(conn);

        let store = SharedStore::open(&path).unwrap();
        // It is visible in the inbox, so the curator can reject it.
        assert!(
            store.inbox("alpha", 10).unwrap()[0]
                .content
                .contains("ship it")
        );
        // Promoting it must fail exactly like publishing the same text.
        assert!(
            store
                .promote("alpha", "obs-1", "draft", Expected::Absent, "ann")
                .is_err()
        );
        assert!(store.get("alpha", "draft").unwrap().is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_newer_schema_is_refused() {
        let dir = private_dir("v3");
        let path = dir.join("shared.db");
        write_version(&path, SCHEMA_VERSION + 1);
        assert!(SharedStore::open(&path).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
