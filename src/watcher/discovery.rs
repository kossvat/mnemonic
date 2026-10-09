//! What a tick needs to know of the transcripts without opening them.
//!
//! A tick runs every ten seconds over thousands of transcripts, nearly all
//! of them unchanged. It stats every known transcript and every folder that
//! can receive a new one: a folder lists again only when its change time
//! moved, and a transcript is opened only when its identity, length or
//! change time moved since its last successful read. A full pass, which
//! lists every folder and reads every transcript as every tick did before,
//! runs at start and every ten minutes (live-projects, design 5.1).
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

/// How often every folder is listed again and every transcript read, whatever
/// their stat says: the catch for a change that keeps all three values.
pub(super) const FULL_PASS_EVERY: Duration = Duration::from_secs(600);

/// When the next full pass is due: the first tick, then every `every`.
pub(super) struct FullPass {
    every: Duration,
    last: Option<Instant>,
}

impl FullPass {
    pub fn new(every: Duration) -> Self {
        Self { every, last: None }
    }

    /// Whether the tick at `now` is a full pass. Asking moves the schedule.
    pub fn due(&mut self, now: Instant) -> bool {
        let due = self
            .last
            .is_none_or(|last| now.saturating_duration_since(last) >= self.every);
        if due {
            self.last = Some(now);
        }
        due
    }
}

/// The three values of a file that say whether it may have changed: its
/// identity (device and inode), its length and its change time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FileStamp {
    device: u64,
    inode: u64,
    len: u64,
    modified: Option<SystemTime>,
}

impl FileStamp {
    pub fn of(meta: &std::fs::Metadata) -> Self {
        #[cfg(unix)]
        let (device, inode) = {
            use std::os::unix::fs::MetadataExt;
            (MetadataExt::dev(meta), MetadataExt::ino(meta))
        };
        #[cfg(not(unix))]
        let (device, inode) = (0, 0);
        Self {
            device,
            inode,
            len: meta.len(),
            modified: meta.modified().ok(),
        }
    }

    /// The stamp of the file at `path` now, without opening it.
    pub fn now(path: &Path) -> Option<Self> {
        std::fs::metadata(path).ok().map(|meta| Self::of(&meta))
    }
}

/// The entries of a folder as of its last listing.
struct Listing {
    modified: Option<SystemTime>,
    files: Vec<PathBuf>,
    folders: Vec<PathBuf>,
}

/// The listing of each folder a watcher reads, kept with the folder's change
/// time. A folder whose change time did not move holds the same entries.
#[derive(Default)]
pub(super) struct Discovery {
    listings: HashMap<PathBuf, Arc<Listing>>,
    /// Folders listed since this was made, for the tests of the tick.
    #[cfg(test)]
    pub listed: usize,
}

impl Discovery {
    /// Forget every listing: the next ticks list every folder again. A
    /// full pass begins with this, which also drops folders that are gone.
    pub fn forget(&mut self) {
        self.listings.clear();
    }

    /// The listing of `folder`, made again when its change time moved;
    /// `None` when it cannot be read. A folder is told from a file
    /// the way the watchers always told them: by following a link.
    fn list(&mut self, folder: &Path) -> Option<Arc<Listing>> {
        let Ok(meta) = std::fs::metadata(folder) else {
            self.listings.remove(folder);
            return None;
        };
        let modified = meta.modified().ok();
        if let Some(listing) = self.listings.get(folder)
            && modified.is_some()
            && listing.modified == modified
        {
            return Some(listing.clone());
        }
        let Ok(entries) = std::fs::read_dir(folder) else {
            self.listings.remove(folder);
            return None;
        };
        let (mut files, mut folders) = (Vec::new(), Vec::new());
        let mut complete = true;
        for entry in entries {
            let Ok(entry) = entry else {
                complete = false;
                continue;
            };
            let path = entry.path();
            if path.is_dir() {
                folders.push(path);
            } else {
                files.push(path);
            }
        }
        #[cfg(test)]
        {
            self.listed += 1;
        }
        let listing = Arc::new(Listing {
            modified,
            files,
            folders,
        });
        // A listing that lost an entry is used for this tick and not kept:
        // the folder's change time would not move for what it missed.
        if complete {
            self.listings.insert(folder.to_path_buf(), listing.clone());
        } else {
            self.listings.remove(folder);
        }
        Some(listing)
    }

    /// Claude Code keeps a transcript in a folder per project: the `.jsonl`
    /// files one folder below `root`.
    pub fn claude_transcripts(&mut self, root: &Path) -> Vec<PathBuf> {
        let Some(top) = self.list(root) else {
            return Vec::new();
        };
        let mut found = Vec::new();
        for project in &top.folders {
            if let Some(listing) = self.list(project) {
                found.extend(
                    listing
                        .files
                        .iter()
                        .filter(|path| path.extension().is_some_and(|e| e == "jsonl"))
                        .cloned(),
                );
            }
        }
        found
    }

    /// Codex keeps its rollouts in folders by date: every rollout at any
    /// depth below `root`.
    pub fn codex_rollouts(&mut self, root: &Path, found: &mut Vec<PathBuf>) {
        let Some(listing) = self.list(root) else {
            return;
        };
        found.extend(
            listing
                .files
                .iter()
                .filter(|path| is_rollout(path))
                .cloned(),
        );
        for folder in &listing.folders {
            self.codex_rollouts(folder, found);
        }
    }
}

/// A Codex rollout: `rollout-*.jsonl`.
pub(super) fn is_rollout(path: &Path) -> bool {
    path.extension().is_some_and(|e| e == "jsonl")
        && path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("rollout-"))
}
