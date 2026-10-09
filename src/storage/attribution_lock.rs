//! Serialize attribution computation and cache writes across store handles.
//! A persistent sibling sidecar avoids interfering with SQLite's own locks.
use super::Storage;
use anyhow::{Context, Result};
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

pub(super) struct AttributionWriteLock {
    path: Option<PathBuf>,
    file: Mutex<Option<File>>,
}

impl AttributionWriteLock {
    pub(super) fn new(database_path: Option<&str>) -> Result<Self> {
        let path = database_path
            .filter(|path| !path.is_empty())
            .map(|path| -> Result<PathBuf> {
                // Resolve symlink aliases without opening another descriptor
                // on SQLite's inode. Closing such a descriptor can release
                // SQLite's process-scoped record locks on Unix.
                let database = std::fs::canonicalize(path)?;
                let mut name = database
                    .file_name()
                    .context("attribution database filename unavailable")?
                    .to_os_string();
                name.push(".attribution-lock");
                Ok(database.with_file_name(name))
            })
            .transpose()?;
        Ok(Self {
            path,
            file: Mutex::new(None),
        })
    }

    fn acquire(&self) -> Result<AttributionWriteGuard<'_>> {
        let mut file = self
            .file
            .lock()
            .map_err(|_| anyhow::anyhow!("attribution write lock unavailable"))?;
        // Only cache writers reach this path. Opening Storage or serving a
        // read never creates a sidecar. Keep the descriptor for later writes.
        if file.is_none()
            && let Some(path) = &self.path
        {
            *file = Some(open_sidecar(path)?);
        }
        if let Some(handle) = file.as_ref() {
            handle.lock().context("attribution file lock unavailable")?;
        }
        Ok(AttributionWriteGuard { file })
    }
}

fn open_sidecar(path: &Path) -> Result<File> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => validate_sidecar(&metadata)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // O_NOFOLLOW closes the symlink race; O_NONBLOCK prevents a planted
        // FIFO from hanging open before its descriptor can be rejected.
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path).context("attribution lock open failed")?;
    validate_sidecar(&file.metadata()?)?;
    Ok(file)
}

fn validate_sidecar(metadata: &std::fs::Metadata) -> Result<()> {
    anyhow::ensure!(
        metadata.is_file() && metadata.len() == 0,
        "attribution lock must be an empty regular file"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        anyhow::ensure!(
            metadata.nlink() == 1,
            "attribution lock must not be hardlinked"
        );
    }
    Ok(())
}

pub(crate) struct AttributionWriteGuard<'a> {
    file: MutexGuard<'a, Option<File>>,
}

impl Drop for AttributionWriteGuard<'_> {
    fn drop(&mut self) {
        // Drop runs before the MutexGuard field is released. Never let the
        // next local writer inherit an OS lock still held by this guard.
        if let Some(file) = self.file.as_ref()
            && file.unlock().is_err()
        {
            tracing::warn!("attribution write lock release failed");
        }
    }
}

impl Storage {
    pub(crate) fn attribution_write_lock(&self) -> Result<AttributionWriteGuard<'_>> {
        self.attribution_write.acquire()
    }
}

#[cfg(test)]
#[path = "attribution_lock_tests.rs"]
mod tests;
