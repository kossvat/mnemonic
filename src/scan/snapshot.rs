//! The file a scan is given: one regular file, named by the caller, with
//! no journal beside it, opened so that SQLite writes nothing and takes
//! no lock. What the file and its directory were before the scan is kept,
//! to say after it that nothing changed.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::hash::Hasher;
use std::io::Read;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};

use super::report::Reason;

/// What SQLite keeps beside a database that is open or was not closed.
const SIDECARS: [&str; 3] = ["-wal", "-shm", "-journal"];
const HEADER: &[u8; 16] = b"SQLite format 3\0";

#[derive(Debug, PartialEq, Eq)]
struct State {
    len: u64,
    hash: u64,
    /// The names in the file's directory.
    beside: BTreeSet<OsString>,
}

pub struct Snapshot {
    path: PathBuf,
    before: State,
}

fn beside(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

fn state(path: &Path) -> Result<State, Reason> {
    let mut file = std::fs::File::open(path).map_err(|_| Reason::ReadFailed)?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    let mut buffer = vec![0u8; 1 << 16];
    let mut len = 0u64;
    loop {
        let n = file.read(&mut buffer).map_err(|_| Reason::ReadFailed)?;
        if n == 0 {
            break;
        }
        hasher.write(&buffer[..n]);
        len += n as u64;
    }
    let parent = path.parent().ok_or(Reason::InvalidPath)?;
    let beside = std::fs::read_dir(parent)
        .map_err(|_| Reason::ReadFailed)?
        .map(|entry| entry.map(|e| e.file_name()))
        .collect::<Result<_, _>>()
        .map_err(|_| Reason::ReadFailed)?;
    Ok(State {
        len,
        hash: hasher.finish(),
        beside,
    })
}

impl Snapshot {
    /// Check the file and open it for reading. Nothing is created: not
    /// the file, not a journal, not a lock.
    pub fn open(path: &Path) -> Result<(Self, Connection), Reason> {
        let text = path.to_str().ok_or(Reason::InvalidPath)?;
        if text.is_empty() || text.contains('\0') {
            return Err(Reason::InvalidPath);
        }
        let meta = std::fs::symlink_metadata(path).map_err(|_| Reason::NotFound)?;
        if !meta.file_type().is_file() {
            return Err(Reason::NotARegularFile);
        }
        // The directory as it is, with no link in its path: the file is
        // opened by that path, and SQLite is told to follow no link.
        let path = std::path::absolute(path).map_err(|_| Reason::InvalidPath)?;
        let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
            return Err(Reason::InvalidPath);
        };
        let path = parent
            .canonicalize()
            .map_err(|_| Reason::NotFound)?
            .join(name);
        for suffix in SIDECARS {
            if std::fs::symlink_metadata(beside(&path, suffix)).is_ok() {
                return Err(Reason::SidecarPresent);
            }
        }
        let mut header = [0u8; 16];
        let read = std::fs::File::open(&path)
            .and_then(|mut file| file.read_exact(&mut header))
            .is_ok();
        if !read || &header != HEADER {
            return Err(Reason::NotADatabase);
        }
        let before = state(&path)?;

        // `immutable`: SQLite takes the file as one that cannot change,
        // so it opens no journal and takes no lock, whatever journal mode
        // the file was written in.
        let text = path.to_str().ok_or(Reason::InvalidPath)?;
        let uri = format!("file:{}?mode=ro&immutable=1", urlencoding::encode(text));
        let flags = OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NOFOLLOW
            | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let conn = Connection::open_with_flags(uri, flags).map_err(|_| Reason::ReadFailed)?;
        conn.pragma_update(None, "query_only", "ON")
            .map_err(|_| Reason::ReadFailed)?;
        conn.pragma_update(None, "trusted_schema", "OFF")
            .map_err(|_| Reason::ReadFailed)?;
        // What SQLite sorts or keeps aside while it answers is kept in
        // memory: a file of its own, in a directory of its choice, would
        // be a copy of what the snapshot holds.
        conn.pragma_update(None, "temp_store", "MEMORY")
            .map_err(|_| Reason::ReadFailed)?;
        Ok((Self { path, before }, conn))
    }

    /// Whether the file and its directory are as they were when the scan
    /// began.
    pub fn unchanged(&self) -> bool {
        state(&self.path).is_ok_and(|now| now == self.before)
    }
}
