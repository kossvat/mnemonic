//! A read-only entry to the store, for a dry run.
//!
//! A normal open initialises the schema, catches up old facts, expires
//! ingress and refreshes the backup: four writes. This entry does none of
//! them and creates nothing. A store nobody has open is taken as a file
//! that cannot change, so SQLite opens no journal and takes no lock; a
//! store in use is read through its journal, as any reader reads it.
use anyhow::Result;
use rusqlite::{Connection, OpenFlags};
use std::collections::HashMap;
use std::path::Path;

/// The files SQLite keeps beside a store while it is in use.
const SIDECARS: [&str; 3] = ["-wal", "-shm", "-journal"];

pub struct ReadOnlyStore {
    conn: Connection,
}

impl ReadOnlyStore {
    /// Open the store at `path` for reading. `None` when there is no store.
    pub fn open(path: &Path) -> Result<Option<Self>> {
        if !path.is_file() {
            return Ok(None);
        }
        let text = path.to_string_lossy();
        let in_use = SIDECARS.iter().any(|suffix| {
            let mut beside = path.as_os_str().to_owned();
            beside.push(suffix);
            Path::new(&beside).exists()
        });
        let mode = if in_use {
            "mode=ro"
        } else {
            "mode=ro&immutable=1"
        };
        let uri = format!("file:{}?{mode}", urlencoding::encode(&text));
        let flags = OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let conn = Connection::open_with_flags(uri, flags)?;
        conn.pragma_update(None, "query_only", "ON")?;
        conn.pragma_update(None, "temp_store", "MEMORY")?;
        Ok(Some(Self { conn }))
    }

    /// For every project that was merged, the key of the project it
    /// resolves to. Empty for a store that has no registry yet.
    pub fn project_redirects(&self) -> Result<HashMap<String, String>> {
        let has_registry: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'projects')",
            [],
            |r| r.get(0),
        )?;
        if !has_registry {
            return Ok(HashMap::new());
        }
        let rows: Vec<(i64, String, Option<i64>)> = self
            .conn
            .prepare("SELECT id, slug, merged_into FROM projects")?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<_>>()?;
        let by_id: HashMap<i64, (&str, Option<i64>)> = rows
            .iter()
            .map(|(id, slug, into)| (*id, (slug.as_str(), *into)))
            .collect();
        let mut redirects = HashMap::new();
        for (id, slug, into) in &rows {
            if into.is_none() {
                continue;
            }
            // The chain to its end, however long; one that comes back to
            // a row it passed redirects nothing.
            let mut at = *id;
            let mut passed = std::collections::HashSet::from([*id]);
            let ended = loop {
                match by_id.get(&at).and_then(|(_, into)| *into) {
                    Some(next) if passed.insert(next) => at = next,
                    Some(_) => break false,
                    None => break true,
                }
            };
            if let (true, Some((canonical, _))) = (ended, by_id.get(&at)) {
                redirects.insert(slug.clone(), canonical.to_string());
            }
        }
        Ok(redirects)
    }
}
