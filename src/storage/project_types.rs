//! Names the configured registry allows a model to type as a project.
use anyhow::Result;
use std::collections::HashSet;

use super::Storage;
use crate::graph::canonical::canonicalize_name;

impl Storage {
    /// Current names, slugs and name aliases, including old names retained
    /// by merged rows. Scope aliases are paths and never admit a project
    /// name. States affect visibility, not whether a name is registered.
    pub fn project_type_names(&self) -> Result<HashSet<String>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        let mut stmt = conn.prepare(
            "SELECT slug FROM projects UNION SELECT name FROM projects
             UNION SELECT a.alias FROM project_aliases a
             JOIN projects p ON p.id = a.project_id WHERE a.kind = 'name'",
        )?;
        let raw = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(raw
            .iter()
            .map(|name| canonicalize_name(name))
            .filter(|name| !name.is_empty())
            .collect())
    }
}
