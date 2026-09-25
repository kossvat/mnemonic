use anyhow::Result;
use git2::Repository;
use std::path::PathBuf;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::event::{Event, EventKind, EventSource};

pub struct GitWatcher {
    repo_path: PathBuf,
    poll_interval_secs: u64,
}

impl GitWatcher {
    pub fn new(repo_path: PathBuf) -> Self {
        Self {
            repo_path,
            poll_interval_secs: 5,
        }
    }

    fn get_head_commit_id(repo: &Repository) -> Option<String> {
        repo.head()
            .ok()?
            .peel_to_commit()
            .ok()
            .map(|c| c.id().to_string())
    }

    fn extract_commit_info(repo: &Repository) -> Option<CommitInfo> {
        let head = repo.head().ok()?;
        let commit = head.peel_to_commit().ok()?;
        let message = commit.message().unwrap_or("").to_string();
        let id = commit.id().to_string();

        // Get diff stats
        let parent = commit.parent(0).ok();
        let diff = repo
            .diff_tree_to_tree(
                parent.as_ref().and_then(|p| p.tree().ok()).as_ref(),
                commit.tree().ok().as_ref(),
                None,
            )
            .ok()?;

        let stats = diff.stats().ok()?;

        Some(CommitInfo {
            id,
            message,
            files_changed: stats.files_changed(),
            insertions: stats.insertions(),
            deletions: stats.deletions(),
        })
    }
}

struct CommitInfo {
    id: String,
    message: String,
    files_changed: usize,
    insertions: usize,
    deletions: usize,
}

impl super::Watcher for GitWatcher {
    async fn start(self, tx: mpsc::Sender<Event>) -> Result<()> {
        info!("Git watcher starting for: {}", self.repo_path.display());

        let repo_path = self.repo_path.clone();
        let interval = self.poll_interval_secs;

        tokio::spawn(async move {
            // Get initial HEAD to track changes
            let mut last_commit_id = match Repository::open(&repo_path) {
                Ok(repo) => {
                    let id = Self::get_head_commit_id(&repo);
                    info!("Git watcher tracking HEAD: {:?}", id);
                    id
                }
                Err(e) => {
                    warn!("Cannot open git repo at {}: {e}", repo_path.display());
                    return;
                }
            };

            let mut tick = tokio::time::interval(tokio::time::Duration::from_secs(interval));

            loop {
                tick.tick().await;

                // Re-open repo each poll to see new commits
                // (git2 caches internal state, won't see external changes otherwise)
                let repo = match Repository::open(&repo_path) {
                    Ok(r) => r,
                    Err(e) => {
                        warn!("Git repo reopen error: {e}");
                        continue;
                    }
                };

                let current_id = Self::get_head_commit_id(&repo);

                if current_id != last_commit_id {
                    if let Some(info) = Self::extract_commit_info(&repo) {
                        let content = format!(
                            "Git commit: {} (+{} -{} in {} files)",
                            info.message.trim(),
                            info.insertions,
                            info.deletions,
                            info.files_changed,
                        );

                        let event =
                            Event::new(EventSource::GitWatcher, EventKind::GitCommit, &content)
                                .with_metadata(serde_json::json!({
                                    "commit_id": info.id,
                                    "message": info.message.trim(),
                                    "files_changed": info.files_changed,
                                    "insertions": info.insertions,
                                    "deletions": info.deletions,
                                }));

                        debug!("New commit detected: {}", info.message.trim());

                        if tx.send(event).await.is_err() {
                            return; // Channel closed
                        }
                    }

                    last_commit_id = current_id;
                }
            }
        });

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_head_and_diff_stats_follow_two_commits() {
        let tmp = crate::test_support::temp_dir("mnemonic-git-");
        let dir = tmp.path();
        {
            let mut options = git2::RepositoryInitOptions::new();
            options.initial_head("main").external_template(false);
            let repo = Repository::init_opts(dir, &options).unwrap();
            assert!(GitWatcher::get_head_commit_id(&repo).is_none());
            // Explicit identities/times avoid dependence on the user's Git
            // identity, signing settings or any network transport.
            let signature = git2::Signature::new(
                "Mnemonic Test",
                "test@example.invalid",
                &git2::Time::new(1_700_000_000, 0),
            )
            .unwrap();
            std::fs::write(dir.join("notes.txt"), "first line\n").unwrap();
            let mut index = repo.index().unwrap();
            index.add_path(std::path::Path::new("notes.txt")).unwrap();
            index.write().unwrap();
            let tree_id = index.write_tree().unwrap();
            let tree = repo.find_tree(tree_id).unwrap();
            let first = repo
                .commit(
                    Some("HEAD"),
                    &signature,
                    &signature,
                    "feat: seed notes",
                    &tree,
                    &[],
                )
                .unwrap();
            assert_eq!(
                GitWatcher::get_head_commit_id(&repo),
                Some(first.to_string())
            );
            let info = GitWatcher::extract_commit_info(&repo).unwrap();
            assert_eq!(info.id, first.to_string());
            assert_eq!(info.message, "feat: seed notes");
            assert_eq!(
                (info.files_changed, info.insertions, info.deletions),
                (1, 1, 0)
            );

            std::fs::write(dir.join("notes.txt"), "replacement line\n").unwrap();
            index.add_path(std::path::Path::new("notes.txt")).unwrap();
            index.write().unwrap();
            let tree_id = index.write_tree().unwrap();
            let tree = repo.find_tree(tree_id).unwrap();
            let parent = repo.find_commit(first).unwrap();
            let second = repo
                .commit(
                    Some("HEAD"),
                    &signature,
                    &signature,
                    "fix: revise notes",
                    &tree,
                    &[&parent],
                )
                .unwrap();
            // The production watcher reopens the repo on each poll.
            let reopened = Repository::open(dir).unwrap();
            assert_eq!(
                GitWatcher::get_head_commit_id(&reopened),
                Some(second.to_string())
            );
            let info = GitWatcher::extract_commit_info(&reopened).unwrap();
            assert_eq!(info.id, second.to_string());
            assert_eq!(info.message, "fix: revise notes");
            assert_eq!(
                (info.files_changed, info.insertions, info.deletions),
                (1, 1, 1)
            );
        }
    }
}
