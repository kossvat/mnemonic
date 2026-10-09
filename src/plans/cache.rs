use super::{ParsedUnit, Provider, providers};
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Stamp {
    pub len: u64,
    pub modified: SystemTime,
}

pub(super) fn stamp(path: &Path) -> Option<Stamp> {
    let metadata = std::fs::metadata(path).ok()?;
    metadata.is_file().then(|| Stamp {
        len: metadata.len(),
        modified: metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
    })
}

/// Configuration cannot turn the reader into a reader of an arbitrary
/// location. Canonical containment also rejects a link outside a checkout.
pub(super) fn within(root: &Path, relative: &Path) -> Option<PathBuf> {
    if relative.as_os_str().is_empty()
        || relative.is_absolute()
        || relative
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
        || relative
            .components()
            .any(|part| part.as_os_str().to_string_lossy().starts_with(".env"))
    {
        return None;
    }
    let path = root.join(relative);
    let canonical_root = root.canonicalize().ok()?;
    if canonical_root
        .components()
        .any(|part| part.as_os_str().to_string_lossy().starts_with(".env"))
    {
        return None;
    }
    // Ordinary components need only a link check. Canonicalize a link
    // when encountered, rather than resolving the whole ancestor chain
    // again for every absent discovery file.
    let mut resolved = canonical_root.clone();
    for component in relative.components() {
        resolved.push(component);
        match resolved.symlink_metadata() {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                resolved = resolved.canonicalize().ok()?;
                if !resolved.starts_with(&canonical_root)
                    || resolved
                        .components()
                        .any(|part| part.as_os_str().to_string_lossy().starts_with(".env"))
                {
                    return None;
                }
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Some(path);
            }
            Err(_) => return None,
        }
    }
    Some(path)
}

#[derive(Clone)]
struct File {
    stamp: Stamp,
    text: Result<String, String>,
    parsed: Option<(Provider, String, bool, i64, Vec<ParsedUnit>)>,
}

#[derive(Default)]
pub(super) struct Cache {
    files: HashMap<PathBuf, File>,
    dirs: HashMap<PathBuf, (SystemTime, Vec<PathBuf>)>,
    pub parsed: usize,
}

impl Cache {
    pub fn text(&mut self, path: &Path) -> Option<(Result<String, String>, i64)> {
        let Some(now) = stamp(path) else {
            self.files.remove(path);
            return None;
        };
        if self.files.get(path).is_none_or(|file| file.stamp != now) {
            let text = if now.len > MAX_FILE_BYTES {
                Err("plan file exceeds limit".into())
            } else {
                std::fs::File::open(path)
                    .and_then(|file| {
                        let mut bytes = Vec::new();
                        file.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes)?;
                        if bytes.len() as u64 > MAX_FILE_BYTES {
                            return Err(std::io::ErrorKind::InvalidData.into());
                        }
                        String::from_utf8(bytes).map_err(|_| std::io::ErrorKind::InvalidData.into())
                    })
                    .map_err(|_| "plan file unreadable".into())
            };
            self.files.insert(
                path.to_path_buf(),
                File {
                    stamp: now,
                    text,
                    parsed: None,
                },
            );
        }
        let file = &self.files[path];
        let modified = file
            .stamp
            .modified
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        Some((file.text.clone(), modified))
    }

    pub fn units(
        &mut self,
        path: &Path,
        provider: Provider,
        unit: &str,
        sections: bool,
        now: DateTime<Utc>,
    ) -> Option<(Vec<ParsedUnit>, i64)> {
        let (text, modified) = self.text(path)?;
        // Funnel weekly counts depend on the date as well as on bytes.
        let day = if provider == Provider::Funnel {
            now.timestamp().div_euclid(86400)
        } else {
            0
        };
        let file = self.files.get_mut(path)?;
        if let Some((kind, id, split, at, parsed)) = &file.parsed
            && (*kind, id.as_str(), *split, *at) == (provider, unit, sections, day)
        {
            return Some((parsed.clone(), modified));
        }
        let result = text.and_then(|text| match provider {
            Provider::Registry => providers::registry(&text),
            Provider::Checkbox => Ok(providers::checkboxes(&text, unit, sections)),
            Provider::Funnel => providers::funnel(&text, unit, now),
        });
        let parsed = result.unwrap_or_else(|message| {
            vec![ParsedUnit {
                id: unit.into(),
                title: unit.into(),
                error: Some(crate::redaction::state::prepare_capped(&message, 160).value),
                ..ParsedUnit::default()
            }]
        });
        self.parsed += 1;
        file.parsed = Some((provider, unit.into(), sections, day, parsed.clone()));
        Some((parsed, modified))
    }

    pub fn dirs(&mut self, path: &Path) -> Vec<PathBuf> {
        let Some(modified) = std::fs::metadata(path)
            .ok()
            .filter(|meta| meta.is_dir())
            .and_then(|meta| meta.modified().ok())
        else {
            self.dirs.remove(path);
            return Vec::new();
        };
        if self
            .dirs
            .get(path)
            .is_none_or(|(before, _)| *before != modified)
        {
            let Ok(entries) = std::fs::read_dir(path) else {
                return Vec::new();
            };
            let mut children = Vec::new();
            for entry in entries {
                // A failed listing is not cached.
                let Ok(entry) = entry else {
                    return Vec::new();
                };
                if entry.file_type().is_ok_and(|kind| kind.is_dir())
                    && !entry.file_name().to_string_lossy().starts_with('.')
                {
                    children.push(entry.path());
                }
            }
            children.sort();
            self.dirs.insert(path.into(), (modified, children));
        }
        self.dirs[path].1.clone()
    }
}

#[cfg(all(test, unix))]
#[test]
fn plan_path_containment_checks_existing_missing_and_retargeted_links() {
    use std::os::unix::fs::symlink;

    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("project");
    let inside = root.join("plans");
    let outside = temp.path().join("outside");
    std::fs::create_dir_all(&inside).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(inside.join("tasks.md"), "- [ ] T01: Pending\n").unwrap();
    assert!(within(&root, Path::new("plans/tasks.md")).is_some());
    assert!(within(&root, Path::new("plans/later/tasks.md")).is_some());
    assert!(within(&root, Path::new("../outside/tasks.md")).is_none());
    assert!(within(&root, &outside.join("tasks.md")).is_none());

    let link = root.join("linked");
    symlink(&inside, &link).unwrap();
    assert!(within(&root, Path::new("linked/tasks.md")).is_some());
    assert!(within(&root, Path::new("linked/later/tasks.md")).is_some());
    std::fs::remove_file(&link).unwrap();
    symlink(&outside, &link).unwrap();
    assert!(within(&root, Path::new("linked/tasks.md")).is_none());
    assert!(within(&root, Path::new("linked/later/tasks.md")).is_none());
    std::fs::remove_file(&link).unwrap();
    symlink(outside.join("missing"), &link).unwrap();
    assert!(within(&root, Path::new("linked")).is_none());
    assert!(within(&root, Path::new("linked/tasks.md")).is_none());

    let hidden = root.join(".env-plans");
    std::fs::create_dir(&hidden).unwrap();
    std::fs::remove_file(&link).unwrap();
    symlink(&hidden, &link).unwrap();
    assert!(within(&root, Path::new(".env-plans/tasks.md")).is_none());
    assert!(within(&root, Path::new("linked/tasks.md")).is_none());
}
