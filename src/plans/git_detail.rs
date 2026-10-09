//! Conservative checkout diagnostics. No Git filters or hooks are run.
//! A skipped tracked file, attributes, filters or a bounded read produce an
//! unknown result unless another inspected change proves the checkout dirty.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use git2::{DiffOptions, IndexEntryExtendedFlag, ObjectType, Oid, Repository};
use serde::Serialize;

const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_ENTRIES: usize = 10_000;

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct Detail {
    pub dirty: Option<bool>,
    pub commits_since: Option<usize>,
}

/// One detail request's checkout scans. A canonical project path distinguishes
/// both linked worktrees and separate projects in the same repository.
/// Do not retain this across requests: worktree edits must be seen next time.
#[derive(Default)]
pub(crate) struct DetailCache {
    dirty: HashMap<PathBuf, Option<bool>>,
    #[cfg(test)]
    scans: usize,
}

#[cfg(test)]
impl DetailCache {
    pub(crate) fn scan_count(&self) -> usize {
        self.scans
    }
}

/// Locations are explicit so tests can supply closed synthetic defaults
/// without changing process-wide environment or libgit2 search paths.
#[derive(Default)]
struct GlobalFiles {
    attributes: Option<PathBuf>,
    ignore: Option<PathBuf>,
}

impl GlobalFiles {
    fn from_environment() -> Self {
        Self {
            attributes: default_git_file("attributes"),
            ignore: default_git_file("ignore"),
        }
    }
}

pub(super) fn read(project: &Path, commit: Option<&str>, cache: &mut DetailCache) -> Detail {
    let Some((repo, relative)) = super::project_repository(project) else {
        return Detail::default();
    };
    inspect(
        &repo,
        &relative,
        commit,
        &GlobalFiles::from_environment(),
        cache,
    )
}

fn inspect(
    repo: &Repository,
    relative: &Path,
    commit: Option<&str>,
    globals: &GlobalFiles,
    cache: &mut DetailCache,
) -> Detail {
    let commits_since = commit.and_then(|commit| {
        let before = Oid::from_str(commit).ok()?;
        let head = repo.head().ok()?.peel_to_commit().ok()?.id();
        let (ahead, behind) = repo.graph_ahead_behind(head, before).ok()?;
        (behind == 0).then_some(ahead)
    });
    let dirty = repo.workdir().and_then(|root| {
        let key = root.canonicalize().ok()?.join(relative);
        if let Some(known) = cache.dirty.get(&key) {
            return *known;
        }
        #[cfg(test)]
        {
            cache.scans += 1;
        }
        let found = dirty(repo, relative, globals);
        cache.dirty.insert(key, found);
        found
    });
    Detail {
        dirty,
        commits_since,
    }
}

fn dirty(repo: &Repository, relative: &Path, globals: &GlobalFiles) -> Option<bool> {
    let root = repo.workdir()?.canonicalize().ok()?;
    let project = root.join(relative);
    let index = repo.index().ok()?;
    let tree = repo.head().ok()?.peel_to_tree().ok()?;
    // The tree/index comparison uses object ids only, not file contents or
    // configured diff programs. Check path components after diffing.
    let diff = repo
        .diff_tree_to_index(Some(&tree), Some(&index), Some(&mut DiffOptions::new()))
        .ok()?;
    if diff.deltas().any(|delta| {
        [delta.old_file().path(), delta.new_file().path()]
            .into_iter()
            .flatten()
            .any(|path| path.starts_with(relative))
    }) {
        return Some(true);
    }
    let mut uncertain = attributes_or_filters(repo, &root, &project, globals);
    #[cfg(unix)]
    let tracks_mode = repo
        .config()
        .ok()
        .and_then(|config| config.get_bool("core.filemode").ok())
        .unwrap_or(true);
    let mut tracked = HashSet::new();
    for entry in index.iter() {
        let Ok(path) = std::str::from_utf8(&entry.path) else {
            uncertain = true;
            continue;
        };
        let path = PathBuf::from(path);
        if !path.starts_with(relative) {
            continue;
        }
        tracked.insert(path.clone());
        if tracked.len() > MAX_ENTRIES {
            return None;
        }
        let full = root.join(&path);
        if skipped(&path) {
            uncertain = true;
            continue;
        }
        if IndexEntryExtendedFlag::from_bits_truncate(entry.flags_extended).is_skip_worktree() {
            // Sparse checkout deliberately leaves these ordinary files out.
            // A sparse-directory entry or a materialized skip-worktree file
            // needs rules this raw comparison does not implement: unknown.
            let absent = fs::symlink_metadata(&full)
                .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound);
            if !absent || entry.mode & 0o170000 != 0o100000 {
                uncertain = true;
            }
            continue;
        }
        let metadata = match fs::symlink_metadata(&full) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Some(true),
            Err(_) => {
                uncertain = true;
                continue;
            }
        };
        if !metadata.is_file()
            || metadata.len() > MAX_FILE_BYTES
            || !super::contained(&project, &full)
        {
            uncertain = true;
            continue;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if tracks_mode
                && (metadata.permissions().mode() & 0o111 != 0) != (entry.mode & 0o111 != 0)
            {
                return Some(true);
            }
        }
        // Every path component must be a real directory: following a link
        // in the middle could otherwise reach an excluded file.
        if linked_ancestor(&project, &full) {
            uncertain = true;
            continue;
        }
        if full
            .parent()
            .is_some_and(|parent| has_attributes(&root, parent))
        {
            uncertain = true;
        }
        if uncertain {
            continue;
        }
        let mut bytes = Vec::new();
        if fs::File::open(&full)
            .ok()?
            .take(MAX_FILE_BYTES + 1)
            .read_to_end(&mut bytes)
            .is_err()
            || bytes.len() as u64 > MAX_FILE_BYTES
        {
            uncertain = true;
            continue;
        }
        if Oid::hash_object(ObjectType::Blob, &bytes).ok()? != entry.id {
            return Some(true);
        }
    }
    let mut directories = vec![project];
    if !safe_ignore_sources(repo, globals) {
        return None;
    }
    let mut seen = 0;
    while let Some(directory) = directories.pop() {
        for entry in fs::read_dir(directory).ok()? {
            let entry = entry.ok()?;
            seen += 1;
            if seen > MAX_ENTRIES {
                return None;
            }
            let path = entry.path();
            let relative = path.strip_prefix(&root).ok()?;
            if skipped(relative) {
                continue;
            }
            let kind = entry.file_type().ok()?;
            if kind.is_symlink() {
                uncertain = true;
                continue;
            }
            if tracked.contains(relative) {
                continue;
            }
            if !safe_ignore_ancestors(&root, &path) {
                uncertain = true;
                continue;
            }
            if repo.is_path_ignored(relative).ok()? {
                continue;
            }
            if kind.is_dir() {
                directories.push(path);
            } else {
                return Some(true);
            }
        }
    }
    (!uncertain).then_some(false)
}

fn skipped(path: &Path) -> bool {
    path.components().any(|part| {
        part.as_os_str().to_str().is_some_and(|part| {
            part == ".git" || part == "target" || part == ".env" || part.starts_with(".env.")
        })
    })
}

fn linked_ancestor(project: &Path, path: &Path) -> bool {
    path.ancestors()
        .take_while(|parent| *parent != project)
        .any(|parent| fs::symlink_metadata(parent).map_or(true, |metadata| metadata.is_symlink()))
}

fn safe_ignore_ancestors(root: &Path, path: &Path) -> bool {
    path.ancestors()
        .skip(1)
        .take_while(|parent| parent.starts_with(root))
        .all(|parent| {
            fs::symlink_metadata(parent.join(".gitignore")).map_or_else(
                |error| error.kind() == std::io::ErrorKind::NotFound,
                |metadata| metadata.is_file(),
            )
        })
}

fn default_git_file(name: &str) -> Option<PathBuf> {
    let directory = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(directory.join("git").join(name))
}

fn safe_ignore_sources(repo: &Repository, globals: &GlobalFiles) -> bool {
    let Ok(config) = repo.config() else {
        return false;
    };
    let global = config
        .get_path("core.excludesfile")
        .ok()
        .or_else(|| globals.ignore.clone());
    let secret = |path: &Path| {
        path.components().any(|part| {
            part.as_os_str()
                .to_str()
                .is_some_and(|part| part == ".env" || part.starts_with(".env."))
        })
    };
    [Some(repo.commondir().join("info").join("exclude")), global]
        .into_iter()
        .flatten()
        .all(|path| {
            !secret(&path)
                && match fs::symlink_metadata(&path) {
                    Ok(metadata) => {
                        metadata.is_file() && path.canonicalize().is_ok_and(|path| !secret(&path))
                    }
                    Err(error) => error.kind() == std::io::ErrorKind::NotFound,
                }
        })
}

fn has_attributes(root: &Path, project: &Path) -> bool {
    project
        .ancestors()
        .take_while(|path| path.starts_with(root))
        .any(|path| path.join(".gitattributes").exists())
}

fn attributes_or_filters(
    repo: &Repository,
    root: &Path,
    project: &Path,
    globals: &GlobalFiles,
) -> bool {
    // Attributes are never loaded or applied. Their presence makes a raw
    // file/blob comparison insufficient, so the result stays unknown.
    if has_attributes(root, project)
        || globals
            .attributes
            .as_ref()
            .is_some_and(|path| path.exists())
        || repo.path().join("info").join("attributes").exists()
        || repo.commondir().join("info").join("attributes").exists()
    {
        return true;
    }
    let Ok(config) = repo.config() else {
        return true;
    };
    if ["core.attributesfile", "core.eol", "filter.*"]
        .iter()
        .any(|key| {
            config
                .entries(Some(key))
                .is_ok_and(|mut entries| entries.next().is_some())
        })
    {
        return true;
    }
    config
        .get_string("core.autocrlf")
        .is_ok_and(|value| value != "false")
}

#[cfg(test)]
#[path = "git_detail_tests.rs"]
mod tests;
