//! Schema generation of `memory.db` (`PRAGMA user_version`) and the one-time
//! snapshot taken before a store first moves to a new generation.
//!
//! Every schema change is additive, so a store never NEEDS the snapshot to
//! keep working; it is the way back if a new generation misbehaves. Taking
//! it is therefore best-effort: a failure is logged, the open goes on, the
//! generation is not bumped, and the next open tries again.
//!
//! Many processes open one store (the daemon, one MCP server per agent
//! session, the CLI). SQLite's own write lock decides which of them takes
//! the snapshot; the copy is written under a unique temporary name and
//! published with a hard link, which never overwrites, so a crash can leave
//! only a stale temporary file, never a torn snapshot.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{Result, ensure};
use rusqlite::Connection;
use tracing::{info, warn};

/// Bumped by each schema change old readers must not mistake for their own:
/// 1 update links, 2 facts v2.
pub const MEMORY_SCHEMA_GENERATION: i64 = 2;

/// A temporary snapshot older than this belongs to a process that died.
const STALE_TMP: Duration = Duration::from_secs(3600);

pub fn generation(conn: &Connection) -> Result<i64> {
    Ok(conn.pragma_query_value(None, "user_version", |r| r.get(0))?)
}

/// Refuse a store a newer binary has already moved on.
pub(super) fn refuse_newer(conn: &Connection) -> Result<()> {
    let found = generation(conn)?;
    ensure!(
        found <= MEMORY_SCHEMA_GENERATION,
        "memory.db schema generation {found} is newer than this binary knows \
         ({MEMORY_SCHEMA_GENERATION}); upgrade mnemonic"
    );
    Ok(())
}

/// A store with no memories table yet has nothing to protect.
pub(super) fn is_fresh(conn: &Connection) -> Result<bool> {
    let tables: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'memories'",
        [],
        |r| r.get(0),
    )?;
    Ok(tables == 0)
}

pub fn snapshot_path(db_path: &Path) -> PathBuf {
    sibling(db_path, &format!(".pre-v{MEMORY_SCHEMA_GENERATION}.bak"))
}

fn sibling(db_path: &Path, suffix: &str) -> PathBuf {
    let name = db_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "memory.db".into());
    db_path.with_file_name(format!("{name}{suffix}"))
}

#[cfg(test)]
thread_local! {
    /// Makes the next snapshot attempt on this thread fail.
    pub(super) static FAIL_NEXT_SNAPSHOT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Take the pre-upgrade snapshot if this store needs one. Returns whether
/// the store may move to the current generation: it is fresh, already there,
/// or its snapshot exists.
pub(super) fn snapshot_before_upgrade(conn: &Connection, db_path: &Path, fresh: bool) -> bool {
    match generation(conn) {
        Ok(found) if found >= MEMORY_SCHEMA_GENERATION => return true,
        Ok(_) => {}
        Err(e) => {
            warn!("Schema generation unreadable, snapshot skipped: {e}");
            return false;
        }
    }
    let target = snapshot_path(db_path);
    if fresh || target.exists() {
        return true;
    }
    remove_stale_temporaries(db_path);
    match take(conn, db_path, &target) {
        Ok(()) => true,
        Err(e) => {
            warn!("Pre-upgrade snapshot not taken, will retry on the next open: {e}");
            false
        }
    }
}

fn take(conn: &Connection, db_path: &Path, target: &Path) -> Result<()> {
    #[cfg(test)]
    if FAIL_NEXT_SNAPSHOT.with(|f| f.replace(false)) {
        anyhow::bail!("injected snapshot failure");
    }
    // Wait long for the lock: every opener arrives here at once after a
    // deploy, and only one of them copies.
    conn.pragma_update(None, "busy_timeout", 30_000)?;
    let outcome = (|| -> Result<()> {
        conn.execute_batch("BEGIN IMMEDIATE")?;
        let copied = (|| -> Result<()> {
            if target.exists() {
                return Ok(());
            }
            let nanos = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            // A directory only the owner can enter holds the copy from its
            // first byte: SQLite creates the file with the umask's mode, and
            // the store may sit in a directory others can read.
            let private = sibling(
                db_path,
                &format!(
                    ".pre-v{MEMORY_SCHEMA_GENERATION}.bak.tmp.{}.{nanos}",
                    std::process::id()
                ),
            );
            let result = private_dir(&private).and_then(|()| {
                let tmp = private.join("snapshot.db");
                copy_scrubbed(db_path, &tmp)?;
                match std::fs::hard_link(&tmp, target) {
                    Ok(()) => Ok(()),
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
                    Err(e) => Err(e.into()),
                }
            });
            let _ = std::fs::remove_dir_all(&private);
            result
        })();
        match copied {
            Ok(()) => {
                conn.execute_batch("COMMIT")?;
                info!("Pre-upgrade snapshot: {}", target.display());
                Ok(())
            }
            Err(e) => {
                let _ = conn.execute_batch("ROLLBACK");
                Err(e)
            }
        }
    })();
    conn.pragma_update(None, "busy_timeout", 5_000)?;
    outcome
}

/// `VACUUM INTO` through a second connection (it cannot run inside the lock
/// holder's own transaction), then scrub raw transcript payloads out of the
/// copy the same way the daily backup does, and make it owner-only.
fn copy_scrubbed(db_path: &Path, tmp: &Path) -> Result<()> {
    let source = Connection::open(db_path)?;
    source.execute(
        &format!(
            "VACUUM INTO '{}'",
            tmp.to_string_lossy().replace('\'', "''")
        ),
        [],
    )?;
    drop(source);
    let copy = Connection::open(tmp)?;
    copy.pragma_update(None, "secure_delete", "ON")?;
    let has_ingress: i64 = copy.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'ingest_events'",
        [],
        |r| r.get(0),
    )?;
    if has_ingress > 0 {
        copy.execute("UPDATE ingest_events SET payload = NULL", [])?;
    }
    drop(copy);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(tmp, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

/// A new directory only the owner can enter.
fn private_dir(path: &Path) -> Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)?;
    Ok(())
}

/// Temporary copies left by a process that died mid-snapshot.
fn remove_stale_temporaries(db_path: &Path) {
    let (Some(dir), Some(name)) = (db_path.parent(), db_path.file_name()) else {
        return;
    };
    let prefix = format!("{}.pre-v", name.to_string_lossy());
    let Ok(entries) = std::fs::read_dir(if dir.as_os_str().is_empty() {
        Path::new(".")
    } else {
        dir
    }) else {
        return;
    };
    for entry in entries.flatten() {
        let file = entry.file_name().to_string_lossy().into_owned();
        let stale = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|modified| SystemTime::now().duration_since(modified).ok())
            .is_some_and(|age| age > STALE_TMP);
        if file.starts_with(&prefix) && file.contains(".bak.tmp.") && stale {
            let path = entry.path();
            let _ = if path.is_dir() {
                std::fs::remove_dir_all(&path)
            } else {
                std::fs::remove_file(&path)
            };
        }
    }
}

/// Move the store to the current generation once it may.
pub(super) fn bump(conn: &Connection, allowed: bool) -> Result<()> {
    if allowed && generation(conn)? < MEMORY_SCHEMA_GENERATION {
        conn.pragma_update(None, "user_version", MEMORY_SCHEMA_GENERATION)?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "versioning_tests.rs"]
mod tests;
