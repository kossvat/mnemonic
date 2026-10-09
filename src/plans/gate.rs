//! Local receipts and project trees, read without starting a process.

use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use git2::{ObjectType, Oid, Repository};
use serde::Serialize;
use serde_json::Value;

#[path = "git_detail.rs"]
mod git_detail;
pub use git_detail::Detail;
pub(crate) use git_detail::DetailCache;

const MAX_RECEIPT_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GateStatus {
    Passed,
    Failed,
    Stale,
    #[default]
    None,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct Gate {
    pub status: GateStatus,
    pub at: Option<i64>,
    pub failed_runs: usize,
}

#[derive(Clone, PartialEq, Eq)]
struct Stamp {
    len: u64,
    modified: Option<SystemTime>,
}

impl Stamp {
    fn of(metadata: &fs::Metadata) -> Self {
        Self {
            len: metadata.len(),
            modified: metadata.modified().ok(),
        }
    }
}

#[derive(Clone)]
struct Receipt {
    change: String,
    passed: Option<bool>,
    subtree: Option<String>,
    subtree_recorded: bool,
    commit: Option<String>,
    at: Option<u128>,
}

struct CachedReceipt {
    stamp: Stamp,
    value: Option<Receipt>,
}

struct CachedDirectory {
    stamp: Stamp,
    paths: Vec<PathBuf>,
}

struct Run {
    order: u128,
    at: Option<i64>,
    receipt: Option<Receipt>,
    configured: bool,
}

/// Paths and source receipt text are kept only in this in-memory reader.
#[derive(Default)]
pub struct Reader {
    receipts: HashMap<PathBuf, CachedReceipt>,
    directories: HashMap<PathBuf, CachedDirectory>,
    #[cfg(test)]
    parsed: usize,
}

impl Reader {
    fn runs(&mut self, project: &Path, unit: &str, receipt: Option<&Path>) -> Vec<Run> {
        let Some(project) = project.canonicalize().ok().filter(|path| path.is_dir()) else {
            return Vec::new();
        };
        if !valid_unit(unit) {
            return Vec::new();
        }
        let mut runs = Vec::new();
        if let Some(receipt) = receipt {
            let path = project.join(receipt);
            if let Some(run) = self.run(&project, &path, unit, None, true) {
                runs.push(run);
            }
        } else {
            let directory = project.join("target").join("checks").join(unit);
            for path in self.entries(&project, &directory) {
                if !path.is_dir() || !contained(&project, &path) {
                    continue;
                }
                let at = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .and_then(|name| name.parse::<u128>().ok())
                    .map(epoch_nanos);
                let path = path.join("verification.json");
                if let Some(run) = self.run(&project, &path, unit, at, false) {
                    runs.push(run);
                }
            }
        }
        // Equal timestamps favor a malformed or failed result over a pass.
        runs.sort_by_key(|run| {
            (
                std::cmp::Reverse(run.order),
                match run.receipt.as_ref().and_then(|receipt| receipt.passed) {
                    None => 0,
                    Some(false) => 1,
                    Some(true) => 2,
                },
            )
        });
        runs
    }

    /// Standalone library call. The binary shares scans through `detail_cached`.
    #[allow(dead_code)]
    pub fn detail(&mut self, project: &Path, unit: &str, receipt: Option<&Path>) -> Detail {
        self.detail_cached(project, unit, receipt, &mut DetailCache::default())
    }

    pub(crate) fn detail_cached(
        &mut self,
        project: &Path,
        unit: &str,
        receipt: Option<&Path>,
        cache: &mut DetailCache,
    ) -> Detail {
        let runs = self.runs(project, unit, receipt);
        let commit = runs
            .first()
            .and_then(|run| run.receipt.as_ref())
            .and_then(|receipt| receipt.commit.as_deref());
        git_detail::read(project, commit, cache)
    }

    pub fn read(&mut self, project: &Path, unit: &str, receipt: Option<&Path>) -> Gate {
        let runs = self.runs(project, unit, receipt);
        let Some(newest) = runs.first() else {
            return Gate::default();
        };
        let failed_runs = runs
            .iter()
            .take(3)
            .take_while(|run| {
                run.receipt
                    .as_ref()
                    .is_some_and(|receipt| receipt.passed == Some(false))
            })
            .count();
        let status = match &newest.receipt {
            None => GateStatus::Stale,
            Some(receipt) if receipt.passed == Some(false) => GateStatus::Failed,
            Some(receipt) if receipt.passed.is_none() => GateStatus::Stale,
            Some(receipt) => {
                let fresh = if let Some(subtree) = &receipt.subtree {
                    head_tree(project).as_ref() == Some(subtree)
                } else if newest.configured && !receipt.subtree_recorded {
                    receipt.commit.as_ref().is_some_and(|commit| {
                        project_repository(project)
                            .and_then(|(repo, _)| head_id(&repo))
                            .as_ref()
                            == Some(commit)
                    })
                } else {
                    // Legacy source_tree is the repository's candidate tree,
                    // not the tested project subtree, even for a root project.
                    false
                };
                if fresh {
                    GateStatus::Passed
                } else {
                    GateStatus::Stale
                }
            }
        };
        Gate {
            status,
            at: newest.at,
            failed_runs,
        }
    }

    fn entries(&mut self, project: &Path, directory: &Path) -> Vec<PathBuf> {
        if !contained(project, directory) {
            self.directories.remove(directory);
            return Vec::new();
        }
        let Ok(metadata) = fs::metadata(directory) else {
            self.directories.remove(directory);
            return Vec::new();
        };
        let stamp = Stamp::of(&metadata);
        if let Some(cached) = self.directories.get(directory)
            && cached.stamp == stamp
        {
            return cached.paths.clone();
        }
        let Ok(entries) = fs::read_dir(directory) else {
            self.directories.remove(directory);
            return Vec::new();
        };
        let paths: Vec<_> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .collect();
        self.directories.insert(
            directory.to_path_buf(),
            CachedDirectory {
                stamp,
                paths: paths.clone(),
            },
        );
        paths
    }

    fn run(
        &mut self,
        project: &Path,
        path: &Path,
        unit: &str,
        folder_at: Option<u128>,
        configured: bool,
    ) -> Option<Run> {
        if !contained(project, path) {
            self.receipts.remove(path);
            return None;
        }
        let metadata = fs::metadata(path)
            .ok()
            .filter(|metadata| metadata.is_file())?;
        let stamp = Stamp::of(&metadata);
        if self
            .receipts
            .get(path)
            .is_none_or(|cached| cached.stamp != stamp)
        {
            let value = parse_receipt(path, metadata.len());
            self.receipts
                .insert(path.to_path_buf(), CachedReceipt { stamp, value });
            #[cfg(test)]
            {
                self.parsed += 1;
            }
        }
        let value = self.receipts.get(path)?.value.clone();
        if value.as_ref().is_some_and(|receipt| receipt.change != unit) {
            return None;
        }
        let time = folder_at
            .or_else(|| value.as_ref().and_then(|receipt| receipt.at))
            .or_else(|| {
                metadata
                    .modified()
                    .ok()?
                    .duration_since(UNIX_EPOCH)
                    .ok()
                    .map(|time| time.as_nanos())
            });
        Some(Run {
            order: time.unwrap_or_default(),
            at: time.and_then(|time| i64::try_from(time / 1_000_000_000).ok()),
            receipt: value,
            configured,
        })
    }

    #[cfg(test)]
    fn parsed_count(&self) -> usize {
        self.parsed
    }
}

fn parse_receipt(path: &Path, len: u64) -> Option<Receipt> {
    if len > MAX_RECEIPT_BYTES {
        return None;
    }
    let mut bytes = Vec::new();
    fs::File::open(path)
        .ok()?
        .take(MAX_RECEIPT_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > MAX_RECEIPT_BYTES {
        return None;
    }
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    let change = value.get("change")?.as_str()?.to_owned();
    let passed = value.get("passed").and_then(Value::as_bool);
    let subtree_recorded = value.get("candidate_subtree").is_some();
    let subtree = value.get("candidate_subtree").and_then(valid_hash);
    let commit = value
        .get("commit")
        .or_else(|| value.get("head"))
        .and_then(valid_hash);
    let at = [
        "at",
        "timestamp",
        "completed_at",
        "finished_at",
        "started_at",
    ]
    .into_iter()
    .find_map(|key| value.get(key).and_then(receipt_time));
    Some(Receipt {
        change,
        passed,
        subtree,
        subtree_recorded,
        commit,
        at,
    })
}

fn valid_hash(value: &Value) -> Option<String> {
    let value = value.as_str()?;
    (value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| value.to_ascii_lowercase())
}

fn receipt_time(value: &Value) -> Option<u128> {
    if let Some(value) = value.as_u64() {
        return Some(epoch_nanos(u128::from(value)));
    }
    let value = value.as_str()?;
    if let Ok(value) = value.parse::<u128>() {
        return Some(epoch_nanos(value));
    }
    let time = chrono::DateTime::parse_from_rfc3339(value).ok()?;
    u128::try_from(time.timestamp_nanos_opt()?).ok()
}

fn epoch_nanos(time: u128) -> u128 {
    // Modern epoch timestamps expressed as seconds, milliseconds,
    // microseconds or nanoseconds. The recipe uses time_ns for run folders.
    let factor = match time {
        0..1_000_000_000_000 => 1_000_000_000,
        1_000_000_000_000..1_000_000_000_000_000 => 1_000_000,
        1_000_000_000_000_000..1_000_000_000_000_000_000 => 1_000,
        _ => 1,
    };
    time.saturating_mul(factor)
}

fn valid_unit(unit: &str) -> bool {
    !unit.is_empty()
        && unit.len() <= 160
        && unit != "."
        && unit != ".."
        && unit
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
}

fn contained(project: &Path, path: &Path) -> bool {
    let safe = |path: &Path| {
        !path.components().any(|component| {
            component == Component::ParentDir
                || component
                    .as_os_str()
                    .to_str()
                    .is_some_and(|name| name == ".env" || name.starts_with(".env."))
        })
    };
    safe(path)
        && path
            .canonicalize()
            .is_ok_and(|path| path.starts_with(project) && safe(&path))
}

fn project_repository(project: &Path) -> Option<(Repository, PathBuf)> {
    let project = project.canonicalize().ok()?;
    if !project.is_dir() {
        return None;
    }
    // Explicit discovery ignores GIT_DIR and never falls back to the
    // caller's current directory. Worktrees resolve their own HEAD.
    let repo = Repository::discover(&project).ok()?;
    let root = repo.workdir()?.canonicalize().ok()?;
    let relative = project.strip_prefix(root).ok()?.to_path_buf();
    Some((repo, relative))
}

fn head_id(repo: &Repository) -> Option<String> {
    Some(repo.head().ok()?.peel_to_commit().ok()?.id().to_string())
}

pub fn head_tree(project: &Path) -> Option<String> {
    let (repo, relative) = project_repository(project)?;
    let tree = repo.head().ok()?.peel_to_tree().ok()?;
    let id: Oid = if relative.as_os_str().is_empty() {
        tree.id()
    } else {
        let entry = tree.get_path(&relative).ok()?;
        if entry.kind() != Some(ObjectType::Tree) {
            return None;
        }
        entry.id()
    };
    Some(id.to_string())
}

#[cfg(test)]
#[path = "gate_tests.rs"]
mod tests;
