//! Separate storage for curated shared context and untrusted agent observations.
//! No private memory, embedding, network, configuration, or output sink is used.

use std::fs::{self, OpenOptions};
use std::path::Path;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail, ensure};
use chrono::Utc;
use rusqlite::{Connection, OpenFlags, OptionalExtension, Row, TransactionBehavior, params};

use super::curation::{NewRecord, check_expected, validate_actor, write_record};
use super::types::{
    Expected, Observation, SharedContext, SharedRecord, validate_identifier, validate_limit,
    validate_payload, validate_slug, validate_text,
};

const MAX_PENDING_PER_WRITER: i64 = 100;
pub(super) const RECORD_COLUMNS: &str = "project_id, key, title, content, source, revision, \
     updated_at, published_by, origin_observation_id, origin_writer_id";
pub(super) const OBSERVATION_COLUMNS: &str = "id, project_id, writer_id, request_id, title, \
     content, source, status, created_at, principal_id, promoted_key";

pub struct SharedStore {
    conn: Mutex<Connection>,
}

impl SharedStore {
    /// Validate the existing parent and all alias routes as service/root-owned.
    /// Broad write permissions are rejected; private temp children are supported.
    pub fn open(path: &Path) -> Result<Self> {
        ensure!(
            path.file_name().is_some(),
            "shared database needs a file path"
        );
        ensure!(
            path != Path::new(":memory:"),
            "shared database must be a persistent file"
        );
        let resolved = super::filesystem::trusted_file_path(path)?;
        let path = resolved.as_path();
        check_file(path)?;
        for suffix in ["-wal", "-shm", "-journal"] {
            let mut sidecar = path.as_os_str().to_os_string();
            sidecar.push(suffix);
            check_file(Path::new(&sidecar))?;
        }
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        match options.open(path) {
            Ok(_) => (),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => (),
            Err(error) => return Err(error).context("cannot create shared database"),
        }
        check_file(path)?;
        let mut conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )?;
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "trusted_schema", "OFF")?;
        {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            super::schema::ensure_schema(&tx)?;
            tx.commit()?;
        }
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "FULL")?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Single-curator shorthand: last writer wins, no attribution.
    #[allow(dead_code, reason = "library API; the binary uses publish_checked")]
    pub fn publish(
        &self,
        project_id: &str,
        key: &str,
        title: &str,
        content: &str,
        source: &str,
    ) -> Result<SharedRecord> {
        self.publish_checked(project_id, key, title, content, source, Expected::Any, None)
    }

    /// Publish only if the key is still what the curator last saw. A stale
    /// expectation returns [`Conflict`] and writes nothing.
    #[allow(clippy::too_many_arguments)]
    pub fn publish_checked(
        &self,
        project_id: &str,
        key: &str,
        title: &str,
        content: &str,
        source: &str,
        expected: Expected,
        actor: Option<&str>,
    ) -> Result<SharedRecord> {
        validate_slug(project_id, "project_id")?;
        validate_identifier(key, "key")?;
        validate_payload(title, content, source)?;
        validate_actor(actor)?;
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        check_pin(&tx, project_id)?;
        let existing = get_record(&tx, project_id, key)?;
        // An identical retry is a no-op whatever the caller expected: the
        // first attempt already moved the key to this exact record. A new
        // actor is a real change and goes through the revision check below.
        if let Some(existing) = &existing
            && existing.title == title
            && existing.content == content
            && existing.source == source
            && existing.published_by.as_deref() == actor
        {
            return Ok(existing.clone());
        }
        check_expected(key, existing.as_ref(), expected)?;
        let record = write_record(
            &tx,
            &NewRecord {
                project_id,
                key,
                title,
                content,
                source,
                published_by: actor,
                origin_observation_id: None,
                origin_writer_id: None,
            },
        )?;
        tx.commit()?;
        Ok(record)
    }

    /// Removes live content. The project's revision remains to invalidate caches.
    #[allow(dead_code, reason = "library API; the binary uses revoke_checked")]
    pub fn revoke(&self, project_id: &str, key: &str) -> Result<bool> {
        self.revoke_checked(project_id, key, Expected::Any)
    }

    pub fn revoke_checked(&self, project_id: &str, key: &str, expected: Expected) -> Result<bool> {
        validate_slug(project_id, "project_id")?;
        validate_identifier(key, "key")?;
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        check_pin(&tx, project_id)?;
        if expected != Expected::Any {
            check_expected(key, get_record(&tx, project_id, key)?.as_ref(), expected)?;
        }
        let changed = tx.execute(
            "DELETE FROM shared_records WHERE project_id = ?1 AND key = ?2",
            params![project_id, key],
        )? > 0;
        if changed {
            next_revision(&tx, project_id)?;
        }
        tx.commit()?;
        Ok(changed)
    }

    pub fn context(&self, project_id: &str, limit: usize) -> Result<SharedContext> {
        self.read_context(project_id, None, limit)
    }

    /// Literal substring search; SQL/FTS syntax from callers has no special meaning.
    pub fn search(&self, project_id: &str, query: &str, limit: usize) -> Result<SharedContext> {
        validate_text(query, "query", 512, false)?;
        self.read_context(project_id, Some(query), limit)
    }

    fn read_context(
        &self,
        project_id: &str,
        query: Option<&str>,
        limit: usize,
    ) -> Result<SharedContext> {
        validate_slug(project_id, "project_id")?;
        validate_limit(limit)?;
        let mut conn = self.lock()?;
        let tx = conn.transaction()?;
        check_pin(&tx, project_id)?;
        let revision = project_revision(&tx, project_id)?;
        let needle = query.map(str::to_lowercase);
        let mut records = {
            let mut stmt = tx.prepare(&format!(
                "SELECT {RECORD_COLUMNS} FROM shared_records WHERE project_id = ?1
                 ORDER BY revision DESC, key ASC"
            ))?;
            // Scope in SQL before reading content. Rust lowercasing handles Russian
            // and other Unicode text, unlike SQLite's ASCII-only lower(). Stream
            // records to retain at most limit + 1 matches in memory.
            let rows = stmt.query_map([project_id], record_row)?;
            let mut matches = Vec::new();
            for row in rows {
                let record = row?;
                if needle.as_ref().is_none_or(|needle| {
                    format!("{}\n{}", record.title, record.content)
                        .to_lowercase()
                        .contains(needle)
                }) {
                    matches.push(record);
                    if matches.len() > limit {
                        break;
                    }
                }
            }
            matches
        };
        let truncated = records.len() > limit;
        records.truncate(limit);
        tx.commit()?;
        Ok(SharedContext {
            project_id: project_id.to_owned(),
            revision,
            records,
            truncated,
        })
    }

    pub fn get(&self, project_id: &str, key: &str) -> Result<Option<SharedRecord>> {
        validate_slug(project_id, "project_id")?;
        validate_identifier(key, "key")?;
        let conn = self.lock()?;
        check_pin(&conn, project_id)?;
        get_record(&conn, project_id, key)
    }

    /// Quarantine an observation with no named principal.
    #[allow(clippy::too_many_arguments)]
    #[allow(dead_code, reason = "library API; the binary uses observe_as")]
    pub fn observe(
        &self,
        project_id: &str,
        writer_id: &str,
        request_id: &str,
        title: &str,
        content: &str,
        source: &str,
    ) -> Result<Observation> {
        self.observe_as(
            project_id, None, writer_id, request_id, title, content, source,
        )
    }

    /// Quarantine an observation on behalf of `principal_id`, the person the
    /// writing agent acts for.
    #[allow(clippy::too_many_arguments)]
    pub fn observe_as(
        &self,
        project_id: &str,
        principal_id: Option<&str>,
        writer_id: &str,
        request_id: &str,
        title: &str,
        content: &str,
        source: &str,
    ) -> Result<Observation> {
        validate_slug(project_id, "project_id")?;
        validate_slug(writer_id, "writer_id")?;
        if let Some(principal_id) = principal_id {
            validate_slug(principal_id, "principal_id")?;
        }
        validate_identifier(request_id, "request_id")?;
        validate_payload(title, content, source)?;
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        check_pin(&tx, project_id)?;
        let existing = tx
            .query_row(
                &format!(
                    "SELECT {OBSERVATION_COLUMNS} FROM shared_observations
                     WHERE project_id=?1 AND writer_id=?2 AND request_id=?3"
                ),
                params![project_id, writer_id, request_id],
                observation_row,
            )
            .optional()?;
        if let Some(existing) = existing {
            ensure!(
                existing.principal_id.as_deref() == principal_id,
                "writer_id already belongs to another principal"
            );
            ensure!(
                existing.title == title && existing.content == content && existing.source == source,
                "request_id already exists with different observation content"
            );
            return Ok(existing);
        }
        // A writer id belongs to one person for good, whatever a policy says.
        if let Some(principal_id) = principal_id {
            let claimed: Option<Option<String>> = tx
                .query_row(
                    "SELECT principal_id FROM shared_observations
                     WHERE project_id=?1 AND writer_id=?2 LIMIT 1",
                    params![project_id, writer_id],
                    |row| row.get(0),
                )
                .optional()?;
            ensure!(
                claimed.is_none_or(|owner| owner.as_deref() == Some(principal_id)),
                "writer_id already belongs to another principal"
            );
        }
        let pending: i64 = tx.query_row(
            "SELECT count(*) FROM shared_observations
             WHERE project_id=?1 AND writer_id=?2 AND status='pending'",
            params![project_id, writer_id],
            |row| row.get(0),
        )?;
        ensure!(
            pending < MAX_PENDING_PER_WRITER,
            "writer pending observation limit reached"
        );
        ensure_project(&tx, project_id)?;
        let observation = Observation {
            id: uuid::Uuid::new_v4().to_string(),
            project_id: project_id.to_owned(),
            writer_id: writer_id.to_owned(),
            request_id: request_id.to_owned(),
            title: title.to_owned(),
            content: content.to_owned(),
            source: source.to_owned(),
            status: "pending".to_owned(),
            created_at: Utc::now().to_rfc3339(),
            principal_id: principal_id.map(str::to_owned),
            promoted_key: None,
        };
        tx.execute(
            "INSERT INTO shared_observations
                (id, project_id, writer_id, request_id, title, content, source, status,
                 created_at, principal_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'pending', ?8, ?9)",
            params![
                observation.id,
                project_id,
                writer_id,
                request_id,
                title,
                content,
                source,
                observation.created_at,
                principal_id
            ],
        )?;
        tx.commit()?;
        Ok(observation)
    }

    pub fn inbox(&self, project_id: &str, limit: usize) -> Result<Vec<Observation>> {
        validate_slug(project_id, "project_id")?;
        validate_limit(limit)?;
        let conn = self.lock()?;
        check_pin(&conn, project_id)?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {OBSERVATION_COLUMNS} FROM shared_observations
             WHERE project_id=?1 AND status='pending'
             ORDER BY created_at ASC, id ASC LIMIT ?2"
        ))?;
        Ok(stmt
            .query_map(params![project_id, limit as i64], observation_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn review(&self, project_id: &str, observation_id: &str, status: &str) -> Result<bool> {
        validate_slug(project_id, "project_id")?;
        validate_identifier(observation_id, "observation_id")?;
        ensure!(
            matches!(status, "reviewed" | "rejected"),
            "invalid review status"
        );
        let conn = self.lock()?;
        check_pin(&conn, project_id)?;
        Ok(conn.execute(
            "UPDATE shared_observations SET status=?3
             WHERE project_id=?1 AND id=?2 AND status='pending'",
            params![project_id, observation_id, status],
        )? > 0)
    }

    pub(super) fn lock(&self) -> Result<MutexGuard<'_, Connection>> {
        self.conn
            .lock()
            .map_err(|_| anyhow!("shared database lock poisoned"))
    }
}

fn check_file(path: &Path) -> Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).context("cannot inspect shared database path"),
    };
    ensure!(
        metadata.is_file(),
        "shared database and sidecars must be regular files, not symlinks"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        ensure!(
            metadata.mode() & 0o077 == 0,
            "shared database and sidecars must have private permissions (0600)"
        );
        ensure!(
            metadata.nlink() == 1,
            "shared database and sidecars must not have hard links"
        );
        // This check does not alter permissions of unrelated or shared directories.
        ensure!(
            metadata.uid() == unsafe { libc::geteuid() },
            "shared database must belong to the current user"
        );
    }
    Ok(())
}

/// A pinned database serves exactly one project. A typo in a policy or a
/// command then fails instead of quietly creating a second, empty project.
pub(super) fn check_pin(conn: &Connection, project_id: &str) -> Result<()> {
    let pinned: Option<String> = conn
        .query_row(
            "SELECT pinned_project FROM shared_meta WHERE id = 1",
            [],
            |r| r.get(0),
        )
        .optional()?;
    ensure!(
        pinned.as_deref().is_none_or(|pinned| pinned == project_id),
        "this shared database is pinned to another project"
    );
    Ok(())
}

fn ensure_project(conn: &Connection, project_id: &str) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO shared_projects(project_id) VALUES (?1)",
        [project_id],
    )?;
    Ok(())
}

fn project_revision(conn: &Connection, project_id: &str) -> Result<i64> {
    Ok(conn
        .query_row(
            "SELECT revision FROM shared_projects WHERE project_id=?1",
            [project_id],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or(0))
}

pub(super) fn next_revision(conn: &Connection, project_id: &str) -> Result<i64> {
    let current = project_revision(conn, project_id)?;
    let Some(next) = current.checked_add(1) else {
        bail!("project revision exhausted")
    };
    conn.execute(
        "UPDATE shared_projects SET revision=?2 WHERE project_id=?1",
        params![project_id, next],
    )?;
    Ok(next)
}

pub(super) fn get_record(
    conn: &Connection,
    project_id: &str,
    key: &str,
) -> Result<Option<SharedRecord>> {
    Ok(conn
        .query_row(
            &format!("SELECT {RECORD_COLUMNS} FROM shared_records WHERE project_id=?1 AND key=?2"),
            params![project_id, key],
            record_row,
        )
        .optional()?)
}

fn record_row(row: &Row<'_>) -> rusqlite::Result<SharedRecord> {
    Ok(SharedRecord {
        project_id: row.get(0)?,
        key: row.get(1)?,
        title: row.get(2)?,
        content: row.get(3)?,
        source: row.get(4)?,
        revision: row.get(5)?,
        updated_at: row.get(6)?,
        published_by: row.get(7)?,
        origin_observation_id: row.get(8)?,
        origin_writer_id: row.get(9)?,
    })
}

pub(super) fn observation_row(row: &Row<'_>) -> rusqlite::Result<Observation> {
    Ok(Observation {
        id: row.get(0)?,
        project_id: row.get(1)?,
        writer_id: row.get(2)?,
        request_id: row.get(3)?,
        title: row.get(4)?,
        content: row.get(5)?,
        source: row.get(6)?,
        status: row.get(7)?,
        created_at: row.get(8)?,
        principal_id: row.get(9)?,
        promoted_key: row.get(10)?,
    })
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
