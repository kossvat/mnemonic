//! Curator operations for a project shared by more than one person: turning a
//! reviewed observation into published text, revoking a person's access, and
//! pinning a database to its one project. None of this is reachable from the
//! restricted agent MCP.

use anyhow::{Context, Result, bail, ensure};
use chrono::Utc;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::Serialize;

use super::store::{OBSERVATION_COLUMNS, SharedStore, check_pin, get_record, observation_row};
use super::types::{Conflict, Expected, SharedRecord, validate_identifier, validate_slug};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Revocation {
    pub project_id: String,
    pub principal_id: String,
    pub revoked_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revoked_by: Option<String>,
}

pub(super) struct NewRecord<'a> {
    pub project_id: &'a str,
    pub key: &'a str,
    pub title: &'a str,
    pub content: &'a str,
    pub source: &'a str,
    pub published_by: Option<&'a str>,
    pub origin_observation_id: Option<&'a str>,
    pub origin_writer_id: Option<&'a str>,
}

/// Attribution is a label, so it gets the same hygiene as any identifier.
pub(super) fn validate_actor(actor: Option<&str>) -> Result<()> {
    match actor {
        Some(actor) => validate_slug(actor, "actor"),
        None => Ok(()),
    }
}

pub(super) fn check_expected(
    key: &str,
    existing: Option<&SharedRecord>,
    expected: Expected,
) -> Result<()> {
    let current_revision = existing.map(|record| record.revision);
    let holds = match expected {
        Expected::Any => true,
        Expected::Absent => current_revision.is_none(),
        Expected::Revision(revision) => current_revision == Some(revision),
    };
    if holds {
        return Ok(());
    }
    Err(Conflict {
        key: key.to_owned(),
        current_revision,
    }
    .into())
}

/// Insert or replace one key at the project's next revision.
pub(super) fn write_record(conn: &Connection, new: &NewRecord<'_>) -> Result<SharedRecord> {
    conn.execute(
        "INSERT OR IGNORE INTO shared_projects(project_id) VALUES (?1)",
        [new.project_id],
    )?;
    let revision = super::store::next_revision(conn, new.project_id)?;
    conn.execute(
        "INSERT INTO shared_records
            (project_id, key, title, content, source, revision, updated_at,
             published_by, origin_observation_id, origin_writer_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT(project_id, key) DO UPDATE SET title=excluded.title,
            content=excluded.content, source=excluded.source,
            revision=excluded.revision, updated_at=excluded.updated_at,
            published_by=excluded.published_by,
            origin_observation_id=excluded.origin_observation_id,
            origin_writer_id=excluded.origin_writer_id",
        params![
            new.project_id,
            new.key,
            new.title,
            new.content,
            new.source,
            revision,
            Utc::now().to_rfc3339(),
            new.published_by,
            new.origin_observation_id,
            new.origin_writer_id
        ],
    )?;
    get_record(conn, new.project_id, new.key)?.context("published record missing")
}

impl SharedStore {
    /// Publish the exact text of one observation under `key`.
    ///
    /// This is the only path from agent input to shared truth, and a human
    /// runs it for one observation at a time. `expected` may not be
    /// [`Expected::Any`]: the curator must say what the key holds now, so a
    /// stale or poisoned draft cannot silently replace newer text. The record
    /// keeps where it came from. A retry returns the same record.
    pub fn promote(
        &self,
        project_id: &str,
        observation_id: &str,
        key: &str,
        expected: Expected,
        actor: &str,
    ) -> Result<SharedRecord> {
        validate_slug(project_id, "project_id")?;
        validate_identifier(observation_id, "observation_id")?;
        validate_identifier(key, "key")?;
        validate_slug(actor, "actor")?;
        ensure!(
            expected != Expected::Any,
            "promote needs --expect-absent or --expect-revision"
        );
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        check_pin(&tx, project_id)?;
        let observation = tx
            .query_row(
                &format!(
                    "SELECT {OBSERVATION_COLUMNS} FROM shared_observations
                     WHERE project_id=?1 AND id=?2"
                ),
                params![project_id, observation_id],
                observation_row,
            )
            .optional()?
            .context("no such observation in this project")?;
        let existing = get_record(&tx, project_id, key)?;
        if let Some(promoted_key) = &observation.promoted_key {
            // Same request again: answer with what it produced the first time.
            if promoted_key == key
                && let Some(record) = existing
                && record.origin_observation_id.as_deref() == Some(observation_id)
            {
                return Ok(record);
            }
            bail!("observation was already promoted to key {promoted_key}");
        }
        ensure!(
            observation.status != "rejected",
            "a rejected observation cannot be promoted"
        );
        // A migrated row predates the current rules; publishing it must not
        // slip text past a check that a direct publish would fail.
        super::types::validate_payload(
            &observation.title,
            &observation.content,
            &observation.source,
        )?;
        check_expected(key, existing.as_ref(), expected)?;
        let record = write_record(
            &tx,
            &NewRecord {
                project_id,
                key,
                title: &observation.title,
                content: &observation.content,
                source: &observation.source,
                published_by: Some(actor),
                origin_observation_id: Some(observation_id),
                origin_writer_id: Some(&observation.writer_id),
            },
        )?;
        tx.execute(
            "UPDATE shared_observations SET status='reviewed', reviewed_by=?3, reviewed_at=?4,
                promoted_key=?5, promoted_revision=?6
             WHERE project_id=?1 AND id=?2",
            params![
                project_id,
                observation_id,
                actor,
                Utc::now().to_rfc3339(),
                key,
                record.revision
            ],
        )?;
        tx.commit()?;
        Ok(record)
    }

    /// Bind this database to one project for good. Refused when it already
    /// holds another project, so an existing multi-project hub stays as is.
    pub fn pin_project(&self, project_id: &str) -> Result<()> {
        validate_slug(project_id, "project_id")?;
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        check_pin(&tx, project_id)?;
        let others: i64 = tx.query_row(
            "SELECT count(*) FROM shared_projects WHERE project_id <> ?1",
            [project_id],
            |row| row.get(0),
        )?;
        ensure!(
            others == 0,
            "cannot pin: this database already holds {others} other project(s)"
        );
        tx.execute(
            "INSERT OR IGNORE INTO shared_projects(project_id) VALUES (?1)",
            [project_id],
        )?;
        tx.execute(
            "INSERT OR IGNORE INTO shared_meta(id, pinned_project) VALUES (1, ?1)",
            [project_id],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn pinned_project(&self) -> Result<Option<String>> {
        Ok(self
            .lock()?
            .query_row(
                "SELECT pinned_project FROM shared_meta WHERE id = 1",
                [],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// Cut off every agent of one person. Live sessions notice on their next
    /// request; text they already read cannot be recalled.
    pub fn revoke_principal(
        &self,
        project_id: &str,
        principal_id: &str,
        actor: Option<&str>,
    ) -> Result<bool> {
        validate_slug(project_id, "project_id")?;
        validate_slug(principal_id, "principal_id")?;
        validate_actor(actor)?;
        let conn = self.lock()?;
        check_pin(&conn, project_id)?;
        Ok(conn.execute(
            "INSERT OR IGNORE INTO shared_revocations
                (project_id, principal_id, revoked_at, revoked_by) VALUES (?1, ?2, ?3, ?4)",
            params![project_id, principal_id, Utc::now().to_rfc3339(), actor],
        )? > 0)
    }

    pub fn restore_principal(&self, project_id: &str, principal_id: &str) -> Result<bool> {
        validate_slug(project_id, "project_id")?;
        validate_slug(principal_id, "principal_id")?;
        Ok(self.lock()?.execute(
            "DELETE FROM shared_revocations WHERE project_id=?1 AND principal_id=?2",
            params![project_id, principal_id],
        )? > 0)
    }

    pub fn is_principal_revoked(&self, project_id: &str, principal_id: &str) -> Result<bool> {
        Ok(self
            .lock()?
            .query_row(
                "SELECT 1 FROM shared_revocations WHERE project_id=?1 AND principal_id=?2",
                params![project_id, principal_id],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    pub fn revocations(&self, project_id: &str) -> Result<Vec<Revocation>> {
        validate_slug(project_id, "project_id")?;
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            "SELECT project_id, principal_id, revoked_at, revoked_by FROM shared_revocations
             WHERE project_id=?1 ORDER BY revoked_at ASC, principal_id ASC",
        )?;
        Ok(stmt
            .query_map([project_id], |row| {
                Ok(Revocation {
                    project_id: row.get(0)?,
                    principal_id: row.get(1)?,
                    revoked_at: row.get(2)?,
                    revoked_by: row.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }
}

#[cfg(test)]
#[path = "curation_tests.rs"]
mod tests;
