//! The registry of projects (live-projects, design 6).
//!
//! A project is a row, not an entity of the graph that happened to get a
//! type. The rows come from the folders of the configured workspace
//! (`crate::projects`); the tables hold names and ids and never where a
//! project lives. A project is renamed or merged without moving a row of
//! history: `merged_into` names the canonical row, and every reader
//! resolves a project through it. Nothing here deletes a row, so a slot,
//! the project's place in the graph, is never given twice.
use anyhow::{Result, bail};
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::BTreeMap;

use super::Storage;

pub(super) const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS projects (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        slug TEXT NOT NULL UNIQUE,
        name TEXT NOT NULL,
        parent_id INTEGER REFERENCES projects(id),
        entity_id TEXT,
        state TEXT NOT NULL DEFAULT 'active'
            CHECK (state IN ('active', 'archived', 'ignored')),
        slot INTEGER NOT NULL UNIQUE,
        merged_into INTEGER REFERENCES projects(id),
        prev_entity_type TEXT,
        first_seen TEXT NOT NULL DEFAULT (datetime('now')),
        last_active_at TEXT
    );
    CREATE TABLE IF NOT EXISTS project_aliases (
        alias TEXT PRIMARY KEY,
        project_id INTEGER NOT NULL REFERENCES projects(id),
        kind TEXT NOT NULL CHECK (kind IN ('name', 'scope'))
    );
    CREATE INDEX IF NOT EXISTS idx_project_aliases_project
        ON project_aliases(project_id);
";

/// The state of a project: `active`, `archived` by the owner, or `ignored`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectState {
    Active,
    Archived,
    Ignored,
}

impl ProjectState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Archived => "archived",
            Self::Ignored => "ignored",
        }
    }

    fn parse(text: &str) -> Self {
        match text {
            "archived" => Self::Archived,
            "ignored" => Self::Ignored,
            _ => Self::Active,
        }
    }
}

/// What an alias is: a second `name` of a project, or a `scope`, a name
/// its folder had before.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AliasKind {
    Name,
    Scope,
}

impl AliasKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::Scope => "scope",
        }
    }
}

/// One row of the registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectRow {
    pub id: i64,
    pub slug: String,
    pub name: String,
    pub parent_id: Option<i64>,
    pub entity_id: Option<String>,
    pub state: ProjectState,
    pub slot: i64,
    pub merged_into: Option<i64>,
    pub prev_entity_type: Option<String>,
}

const COLUMNS: &str =
    "id, slug, name, parent_id, entity_id, state, slot, merged_into, prev_entity_type";

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ProjectRow> {
    Ok(ProjectRow {
        id: r.get(0)?,
        slug: r.get(1)?,
        name: r.get(2)?,
        parent_id: r.get(3)?,
        entity_id: r.get(4)?,
        state: ProjectState::parse(&r.get::<_, String>(5)?),
        slot: r.get(6)?,
        merged_into: r.get(7)?,
        prev_entity_type: r.get(8)?,
    })
}

/// The row a project resolves to: its own, or the one its merges lead to,
/// however long the chain. A chain that comes back to a row it passed (it
/// cannot be written through `project_merge`) stops where it began.
fn canonical(conn: &Connection, id: i64) -> Result<i64> {
    let mut at = id;
    let mut passed = std::collections::HashSet::from([id]);
    loop {
        let next: Option<i64> = conn
            .query_row(
                "SELECT merged_into FROM projects WHERE id = ?1",
                params![at],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        match next {
            Some(next) if passed.insert(next) => at = next,
            Some(_) => return Ok(id),
            None => return Ok(at),
        }
    }
}

/// A transaction that holds the write lock from its start. These writers
/// read before they write, and a lock taken to read is refused, not waited
/// for, when it is raised after another process committed in between.
fn write_tx(conn: &Connection) -> Result<rusqlite::Transaction<'_>> {
    Ok(rusqlite::Transaction::new_unchecked(
        conn,
        rusqlite::TransactionBehavior::Immediate,
    )?)
}

/// Mark the days of the segments of `id` and of every project merged into
/// it: the split of the owner's time on those days is made again.
fn mark_days_of_members(conn: &Connection, id: i64) -> Result<()> {
    let all: Vec<i64> = conn
        .prepare("SELECT id FROM projects")?
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    for member in all {
        // After a merge `id` resolves further on, so ask who reaches it.
        if member == id || reaches(conn, member, id)? {
            super::owner_time::mark_days_of_project(conn, member)?;
        }
    }
    Ok(())
}

/// Whether the redirects of `from` pass through `through`.
fn reaches(conn: &Connection, from: i64, through: i64) -> Result<bool> {
    let mut at = from;
    let mut passed = std::collections::HashSet::from([from]);
    loop {
        let next: Option<i64> = conn
            .query_row(
                "SELECT merged_into FROM projects WHERE id = ?1",
                params![at],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        match next {
            Some(next) if next == through => return Ok(true),
            Some(next) if passed.insert(next) => at = next,
            _ => return Ok(false),
        }
    }
}

impl Storage {
    /// Make the row of a project found in the workspace, or bring its name
    /// and parent up to date. A new row takes the next slot. Returns its id.
    ///
    /// The names are judged by the identity policy as they arrived and in
    /// the forms they are stored in; a refused name writes nothing.
    pub fn project_upsert(&self, slug: &str, name: &str, parent_id: Option<i64>) -> Result<i64> {
        crate::redaction::state::admit_identities("project", [slug, name])?;
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let tx = write_tx(&conn)?;
        let existing: Option<i64> = tx
            .query_row(
                "SELECT id FROM projects WHERE slug = ?1",
                params![slug],
                |r| r.get(0),
            )
            .optional()?;
        let id = match existing {
            Some(id) => {
                tx.execute(
                    "UPDATE projects SET name = ?1, parent_id = ?2
                      WHERE id = ?3 AND (name IS NOT ?1 OR parent_id IS NOT ?2)",
                    params![name, parent_id, id],
                )?;
                id
            }
            None => {
                tx.execute(
                    "INSERT INTO projects (slug, name, parent_id, slot)
                     VALUES (?1, ?2, ?3, (SELECT COALESCE(MAX(slot), -1) + 1 FROM projects))",
                    params![slug, name, parent_id],
                )?;
                tx.last_insert_rowid()
            }
        };
        tx.commit()?;
        Ok(id)
    }

    /// Tie a project to the entity of the graph that carries `entity_name`,
    /// when there is one, and make that entity a project. The type it had
    /// is kept the first time, so the change can be read back. This is the
    /// one write the registry makes to entities. Whether an entity was
    /// found.
    pub fn project_link_entity(&self, id: i64, entity_name: &str) -> Result<bool> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let tx = write_tx(&conn)?;
        let entity: Option<(String, String)> = tx
            .query_row(
                "SELECT id, entity_type FROM entities WHERE name = ?1",
                params![entity_name],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let Some((entity_id, entity_type)) = entity else {
            return Ok(false);
        };
        if entity_type != "project" {
            tx.execute(
                "UPDATE entities SET entity_type = 'project' WHERE id = ?1",
                params![entity_id],
            )?;
            tx.execute(
                "UPDATE projects SET prev_entity_type = ?1
                  WHERE id = ?2 AND prev_entity_type IS NULL",
                params![entity_type, id],
            )?;
        }
        tx.execute(
            "UPDATE projects SET entity_id = ?1 WHERE id = ?2 AND entity_id IS NOT ?1",
            params![entity_id, id],
        )?;
        tx.commit()?;
        Ok(true)
    }

    /// Give a project a second name. A name that is the slug of a project,
    /// or the alias of another one, is left as it is. Whether it was set.
    pub fn project_alias_set(&self, alias: &str, project_id: i64, kind: AliasKind) -> Result<bool> {
        crate::redaction::state::admit_identities("project alias", [alias])?;
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let taken: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM projects WHERE slug = ?1)
                 OR EXISTS(SELECT 1 FROM project_aliases
                            WHERE alias = ?1 AND project_id IS NOT ?2)",
            params![alias, project_id],
            |r| r.get(0),
        )?;
        if taken {
            return Ok(false);
        }
        conn.execute(
            "INSERT INTO project_aliases (alias, project_id, kind) VALUES (?1, ?2, ?3)
             ON CONFLICT(alias) DO UPDATE SET kind = excluded.kind",
            params![alias, project_id, kind.as_str()],
        )?;
        Ok(true)
    }

    /// Every row of the registry, in the order of their slots.
    pub fn projects(&self) -> Result<Vec<ProjectRow>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let mut stmt = conn.prepare(&format!("SELECT {COLUMNS} FROM projects ORDER BY slot"))?;
        let rows = stmt.query_map([], row)?.collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    /// The row a key names: a slug or an alias, as it is written. Not
    /// followed through a merge; `project_canonical` does that.
    pub fn project_by_key(&self, key: &str) -> Result<Option<ProjectRow>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let found = conn
            .query_row(
                &format!(
                    "SELECT {COLUMNS} FROM projects
                      WHERE slug = ?1
                         OR id = (SELECT project_id FROM project_aliases WHERE alias = ?1)
                      ORDER BY slug = ?1 DESC LIMIT 1"
                ),
                params![key],
                row,
            )
            .optional()?;
        Ok(found)
    }

    /// The id a project resolves to: its own, or the one it was merged into.
    pub fn project_canonical(&self, id: i64) -> Result<i64> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        canonical(&conn, id)
    }

    /// The canonical projects with the ids that resolve to each, itself
    /// included: what a reader sums over.
    pub fn project_groups(&self) -> Result<BTreeMap<i64, Vec<i64>>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let ids: Vec<i64> = conn
            .prepare("SELECT id FROM projects ORDER BY id")?
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        let mut groups: BTreeMap<i64, Vec<i64>> = BTreeMap::new();
        for id in ids {
            groups.entry(canonical(&conn, id)?).or_default().push(id);
        }
        Ok(groups)
    }

    /// Merge `from` into `into`: a redirect, written on the one row. A
    /// merge that would lead back to its own start is refused.
    pub fn project_merge(&self, from: i64, into: i64) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let tx = write_tx(&conn)?;
        let target = canonical(&tx, into)?;
        if target == from || canonical(&tx, from)? == target {
            bail!("a project cannot be merged into itself");
        }
        let changed = tx.execute(
            "UPDATE projects SET merged_into = ?1
              WHERE id = ?2 AND EXISTS(SELECT 1 FROM projects WHERE id = ?1)",
            params![target, from],
        )?;
        if changed == 0 {
            bail!("no such project");
        }
        mark_days_of_members(&tx, from)?;
        tx.commit()?;
        Ok(())
    }

    /// Undo a merge: the project resolves to itself again. Whether it was
    /// merged.
    pub fn project_unmerge(&self, id: i64) -> Result<bool> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let tx = write_tx(&conn)?;
        let changed = tx.execute(
            "UPDATE projects SET merged_into = NULL WHERE id = ?1 AND merged_into IS NOT NULL",
            params![id],
        )?;
        if changed > 0 {
            mark_days_of_members(&tx, id)?;
        }
        tx.commit()?;
        Ok(changed > 0)
    }

    /// Tie every project that has no entity yet to the entity that carries
    /// its name, when one has appeared since. How many were tied.
    pub fn projects_link_missing(&self, entity_name: impl Fn(&str) -> String) -> Result<usize> {
        let unlinked: Vec<(i64, String)> = {
            let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
            conn.prepare("SELECT id, name FROM projects WHERE entity_id IS NULL")?
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?
        };
        let mut tied = 0;
        for (id, name) in unlinked {
            tied += usize::from(self.project_link_entity(id, &entity_name(&name))?);
        }
        Ok(tied)
    }

    /// A value that moves when a merge is made, undone or pointed
    /// elsewhere: every row that redirects with where it redirects to, in
    /// the order of the rows.
    pub fn project_redirects_stamp(&self) -> Result<String> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        Ok(conn.query_row(
            "SELECT COALESCE(group_concat(id || '>' || merged_into, ','), '')
               FROM (SELECT id, merged_into FROM projects
                      WHERE merged_into IS NOT NULL ORDER BY id)",
            [],
            |r| r.get(0),
        )?)
    }

    /// Set the state of a project. Whether it changed.
    pub fn project_set_state(&self, id: i64, state: ProjectState) -> Result<bool> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let changed = conn.execute(
            "UPDATE projects SET state = ?1 WHERE id = ?2 AND state IS NOT ?1",
            params![state.as_str(), id],
        )?;
        Ok(changed > 0)
    }
}
