//! Filesystem boundary for the trusted hub process, not a worker sandbox.
//!
//! Every traversed directory (including alias routes) must remain controlled by
//! root or the service UID. Sticky shared ancestors may contain a trusted child,
//! but cannot directly contain the DB/sidecars or policy. Root, the service UID,
//! mount administration and any ACL grants are trusted deployment concerns.

use anyhow::{Context, Result, ensure};
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};

/// Resolve only the parent, leaving the leaf for NOFOLLOW file opens. Checking
/// only canonical ancestors would miss an attacker-controlled alias route.
pub(super) fn trusted_file_path(path: &Path) -> Result<PathBuf> {
    let name = path.file_name().context("shared path needs a file name")?;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    #[cfg(unix)]
    let parent = trusted_directory(parent)?;
    #[cfg(not(unix))]
    let parent = parent.canonicalize()?;
    Ok(parent.join(name))
}

#[cfg(unix)]
fn trusted_directory(path: &Path) -> Result<PathBuf> {
    use std::collections::VecDeque;
    use std::os::unix::fs::MetadataExt;
    use std::path::Component;

    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    // Owned one-component paths allow a symlink target to be prepended without
    // normalizing away an unsafe traversal such as writable/../trusted.
    let mut pending: VecDeque<PathBuf> = absolute
        .components()
        .map(|c| PathBuf::from(c.as_os_str()))
        .collect();
    let mut resolved = PathBuf::from("/");
    let service_uid = unsafe { libc::geteuid() };
    check_directory(&fs::symlink_metadata(&resolved)?, service_uid, true)?;
    let mut links = 0;
    while let Some(part) = pending.pop_front() {
        match part.components().next().context("empty path component")? {
            Component::RootDir => resolved = PathBuf::from("/"),
            Component::CurDir => (),
            Component::ParentDir => {
                resolved.pop();
            }
            Component::Normal(name) => {
                let next = resolved.join(name);
                let meta = fs::symlink_metadata(&next).with_context(|| {
                    format!("cannot inspect shared directory {}", next.display())
                })?;
                if meta.file_type().is_symlink() {
                    check_owner(meta.uid(), service_uid)?;
                    links += 1;
                    ensure!(links <= 40, "too many shared directory symlinks");
                    let target = fs::read_link(&next)?;
                    for component in target.components().rev() {
                        pending.push_front(PathBuf::from(component.as_os_str()));
                    }
                } else {
                    check_directory(&meta, service_uid, true).with_context(|| {
                        format!("untrusted shared directory {}", next.display())
                    })?;
                    resolved = next;
                }
            }
            Component::Prefix(_) => anyhow::bail!("unsupported shared path prefix"),
        }
    }
    // Even a sticky directory permits precreation of new DB or sidecar names.
    check_directory(&fs::symlink_metadata(&resolved)?, service_uid, false)
        .with_context(|| format!("untrusted shared parent {}", resolved.display()))?;
    Ok(resolved)
}

#[cfg(unix)]
fn check_owner(owner: u32, service_uid: u32) -> Result<()> {
    ensure!(
        owner == service_uid || owner == 0,
        "shared path must be owned by the service user or root"
    );
    Ok(())
}

#[cfg(unix)]
fn check_directory(meta: &fs::Metadata, service_uid: u32, allow_sticky: bool) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    ensure!(
        meta.is_dir(),
        "shared parent components must be directories"
    );
    check_owner(meta.uid(), service_uid)?;
    ensure!(
        meta.mode() & 0o022 == 0 || (allow_sticky && meta.mode() & 0o1000 != 0),
        "shared directory must not be group/world writable (sticky ancestors only)"
    );
    Ok(())
}

pub(super) fn open_policy(path: &Path) -> Result<File> {
    let path = trusted_file_path(path)?;
    // Validate the descriptor actually read, instead of stat(path) followed by
    // reopening the original path. NONBLOCK avoids waiting on a substituted FIFO.
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    ensure!(
        fs::symlink_metadata(&path)?.is_file(),
        "policy must be a regular file, not a symlink"
    );
    let file = options.open(&path).context("cannot open shared policy")?;
    let meta = file.metadata()?;
    ensure!(meta.is_file(), "policy must be a regular file");
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        ensure!(
            meta.mode() & 0o022 == 0,
            "policy must not be group/world writable"
        );
        check_owner(meta.uid(), unsafe { libc::geteuid() })?;
    }
    Ok(file)
}

#[cfg(all(test, unix))]
#[path = "filesystem_tests.rs"]
mod tests;
