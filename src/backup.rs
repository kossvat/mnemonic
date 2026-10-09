//! A whole copy of the stores, taken before an installation: the way back
//! (live-projects, design 20).
//!
//! The stores are opened as they are. Nothing is migrated, so a binary
//! newer than the one installed can take the copy of a store before that
//! store is moved to its generation. Each copy is taken through SQLite, so
//! it is whole while a daemon writes, and it is checked before it counts.
use anyhow::{Context, Result, bail, ensure};
use rusqlite::{Connection, OpenFlags};
use std::path::{Path, PathBuf};

use crate::config::Config;

/// Room left free on the volume after the copies.
const MARGIN_BYTES: u64 = 64 << 20;

/// One copy that was taken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Copied {
    pub file: PathBuf,
    pub bytes: u64,
}

/// The bytes a store holds: its file and its journal.
fn size_of(db: &Path) -> u64 {
    let len = |path: PathBuf| std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0);
    let mut journal = db.as_os_str().to_owned();
    journal.push("-wal");
    len(db.to_path_buf()) + len(PathBuf::from(journal))
}

/// The bytes free for this user on the volume of `path`.
#[cfg(unix)]
fn free_bytes(path: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut stat = std::mem::MaybeUninit::<libc::statvfs>::zeroed();
    // SAFETY: `path` is a C string that lives through the call, and
    // `stat` is a place for one `statvfs`, read only when the call says
    // it filled it.
    let stat = unsafe {
        if libc::statvfs(path.as_ptr(), stat.as_mut_ptr()) != 0 {
            return None;
        }
        stat.assume_init()
    };
    Some(stat.f_bavail as u64 * stat.f_frsize as u64)
}

#[cfg(not(unix))]
fn free_bytes(_path: &Path) -> Option<u64> {
    None
}

/// A new folder only the owner can enter. It fails when the folder is
/// there already.
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

fn written_over(target: &Path) -> String {
    format!(
        "{} is there already: a backup is never written over",
        target.display()
    )
}

/// Copy one store into `staging`, check the copy, and publish it as
/// `target`, which must not exist. The copy is whole and checked before
/// it has that name, and it never takes the place of a file that has it.
fn copy(source: &Path, staging: &Path, target: &Path) -> Result<Copied> {
    ensure!(!target.exists(), written_over(target));
    let staged = staging.join(target.file_name().context("a copy has a file name")?);
    // Read and write, never create: the store is copied as it is, and one
    // that is not there is an error, not a new empty store.
    let conn = Connection::open_with_flags(source, OpenFlags::SQLITE_OPEN_READ_WRITE)
        .with_context(|| format!("cannot open {}", source.display()))?;
    conn.pragma_update(None, "busy_timeout", 30_000)?;
    conn.execute(
        &format!(
            "VACUUM INTO '{}'",
            staged.to_string_lossy().replace('\'', "''")
        ),
        [],
    )
    .with_context(|| format!("cannot copy {}", source.display()))?;
    drop(conn);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o600))?;
    }
    let copy = Connection::open_with_flags(&staged, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let checked: String = copy.pragma_query_value(None, "quick_check", |row| row.get(0))?;
    drop(copy);
    ensure!(
        checked == "ok",
        "the copy of {} did not pass its check",
        source.display()
    );
    publish(&staged, target)?;
    Ok(Copied {
        file: target.to_path_buf(),
        bytes: std::fs::metadata(target)?.len(),
    })
}

/// Give a finished copy its name. A link fails when the name is taken, so
/// a copy another run finished first is never written over; and what is
/// removed on a failure is this run's own file in its own folder, never
/// the file that has the name.
fn publish(staged: &Path, target: &Path) -> Result<()> {
    match std::fs::hard_link(staged, target) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => bail!(written_over(target)),
        Err(e) => Err(e).with_context(|| format!("cannot write {}", target.display())),
    }
}

/// Copy the memory store and, when there is one, the activity store into
/// `dir`, made when it is not there. Each copy is written in a folder of
/// this run that only the owner can enter, and gets its name in `dir`
/// when it is whole, checked and readable by the owner alone: `dir` may
/// be a folder others can look into.
pub fn backup(config: &Config, dir: &Path) -> Result<Vec<Copied>> {
    let memory = &config.storage.db_path;
    ensure!(memory.exists(), "no store at {}", memory.display());
    let activity = config.activity_db_path();
    let sources: Vec<&Path> = [
        Some(memory.as_path()),
        activity.exists().then_some(&*activity),
    ]
    .into_iter()
    .flatten()
    .collect();
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(dir)?;
    let needed: u64 = sources.iter().map(|source| size_of(source)).sum();
    if let Some(free) = free_bytes(dir) {
        ensure!(
            free >= needed + MARGIN_BYTES,
            "not enough room for the backup: {} MB free, {} MB to copy",
            free >> 20,
            needed >> 20
        );
    }
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let staging = dir.join(format!(".staging-{}-{nanos}", std::process::id()));
    private_dir(&staging)?;
    let copied = (|| {
        let mut copied = Vec::new();
        for source in sources {
            let name = source.file_name().context("a store has a file name")?;
            copied.push(copy(source, &staging, &dir.join(name))?);
        }
        Ok(copied)
    })();
    let _ = std::fs::remove_dir_all(&staging);
    copied
}

#[cfg(test)]
#[path = "backup_tests.rs"]
mod tests;
