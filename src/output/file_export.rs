use anyhow::{Context, Result, bail};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;

use crate::event::MemoryEntry;

/// Keep the full memory identity: titles (including truncated slugs) are not keys.
/// Percent encoding also keeps older, non-UUID imported IDs inside one filename.
pub(super) fn filename_for(entry: &MemoryEntry, slug: &str) -> Result<String> {
    if entry.id.is_empty() {
        bail!("Cannot export a memory without an ID");
    }
    let id: String = entry
        .id
        .bytes()
        .map(|b| {
            // Encode uppercase as well so distinct imported IDs remain
            // distinct on case-insensitive filesystems.
            if b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' {
                char::from(b).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect();
    let date = entry.timestamp.format("%Y-%m-%d").to_string();
    // Filesystem limits count bytes, not Unicode characters. Leave room for
    // date, separators, the complete ID and extension on common filesystems.
    let slug_budget = 240usize
        .checked_sub(date.len() + id.len() + 5)
        .filter(|budget| *budget >= "memory".len())
        .context("Memory ID is too long for a safe export filename")?;
    let mut safe_slug = String::new();
    for ch in slug.chars() {
        if safe_slug.len() + ch.len_utf8() > slug_budget {
            break;
        }
        safe_slug.push(ch);
    }
    if safe_slug.is_empty() {
        safe_slug.push_str("memory");
    }
    Ok(format!("{date}-{safe_slug}-{id}.md"))
}

/// Publish complete files atomically, without following a destination symlink
/// or exposing a partially written note to file watchers.
pub(super) fn write_atomic(path: &Path, content: &str) -> Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.is_file() => {
            bail!(
                "Export destination is not a regular file: {}",
                path.display()
            );
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let parent = path
        .parent()
        .context("Export path has no parent directory")?;
    let temp = parent.join(format!(".mnemonic-export-{}.tmp", uuid::Uuid::new_v4()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    // Only clean up after create_new succeeds: a collision must never remove
    // a temporary file created by another process.
    let mut file = options.open(&temp)?;
    let result = (|| -> Result<()> {
        file.write_all(content.as_bytes())?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temp, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{EventSource, MemoryType};

    #[test]
    fn unsafe_imported_ids_are_encoded_without_colliding() {
        let mut entry = MemoryEntry::new("Note", "Body", MemoryType::Note, EventSource::Manual);
        entry.id = "../../outside".into();
        let unsafe_name = filename_for(&entry, "note").unwrap();
        assert!(!unsafe_name.contains('/'));
        entry.id = "%2E%2E%2F%2E%2E%2Foutside".into();
        assert_ne!(unsafe_name, filename_for(&entry, "note").unwrap());
        entry.id = "ENTRY".into();
        let uppercase = filename_for(&entry, "note").unwrap();
        entry.id = "entry".into();
        assert_ne!(
            uppercase.to_lowercase(),
            filename_for(&entry, "note").unwrap()
        );
        entry.id.clear();
        assert!(filename_for(&entry, "note").is_err());
        entry.id = "x".repeat(240);
        assert!(filename_for(&entry, "note").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn replacement_is_complete_private_and_leaves_no_temp_file() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = crate::test_support::temp_dir("mnemonic-export-");
        let dir = tmp.path();
        let path = dir.join("note.md");
        write_atomic(&path, &"previous contents".repeat(4096)).unwrap();
        write_atomic(&path, "replacement").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "replacement");
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(std::fs::read_dir(dir).unwrap().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn atomic_write_refuses_symlinks_without_touching_the_target() {
        let tmp = crate::test_support::temp_dir("mnemonic-export-");
        let dir = tmp.path();
        let target = dir.join("original.md");
        let link = dir.join("export.md");
        std::fs::write(&target, "original").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(write_atomic(&link, "replacement").is_err());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "original");
        assert!(std::fs::symlink_metadata(&link).unwrap().is_symlink());
    }
}
