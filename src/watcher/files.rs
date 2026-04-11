use anyhow::Result;
use notify::{Event as NotifyEvent, EventKind, RecommendedWatcher, RecursiveMode, Watcher as _};
use std::path::Path;
use std::sync::mpsc as std_mpsc;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::config::WatcherConfig;
use crate::event::{Event, EventKind as MnemonicEventKind, EventSource};

pub struct FileWatcher {
    config: WatcherConfig,
}

impl FileWatcher {
    pub fn new(config: WatcherConfig) -> Self {
        Self { config }
    }

    fn should_ignore(path: &Path, ignore_patterns: &[String], extensions: &[String]) -> bool {
        let path_str = path.to_string_lossy();

        for pattern in ignore_patterns {
            if pattern.ends_with('/') {
                if path_str.contains(pattern.trim_end_matches('/')) {
                    return true;
                }
            } else if pattern.starts_with("*.") {
                let ext = pattern.trim_start_matches("*.");
                if path.extension().is_some_and(|e| e == ext) {
                    return true;
                }
            } else if path_str.contains(pattern.as_str()) {
                return true;
            }
        }

        // Check extension allowlist
        if let Some(ext) = path.extension() {
            let ext_str = ext.to_string_lossy().to_string();
            if !extensions.contains(&ext_str) {
                return true;
            }
        }

        false
    }

    fn classify_event(kind: &EventKind, path: &Path) -> Option<MnemonicEventKind> {
        let path_str = path.to_string_lossy();

        match kind {
            EventKind::Create(_) => {
                if path_str.ends_with("Cargo.toml")
                    || path_str.ends_with("package.json")
                    || path_str.ends_with("requirements.txt")
                {
                    Some(MnemonicEventKind::DependencyAdded)
                } else {
                    Some(MnemonicEventKind::FileCreated)
                }
            }
            EventKind::Modify(_) => {
                if path_str.ends_with("Cargo.toml")
                    || path_str.ends_with("package.json")
                    || path_str.ends_with("requirements.txt")
                {
                    Some(MnemonicEventKind::DependencyAdded)
                } else {
                    Some(MnemonicEventKind::FileModified)
                }
            }
            EventKind::Remove(_) => Some(MnemonicEventKind::FileDeleted),
            _ => None,
        }
    }
}

impl super::Watcher for FileWatcher {
    async fn start(self, tx: mpsc::Sender<Event>) -> Result<()> {
        let (sync_tx, sync_rx) = std_mpsc::channel::<notify::Result<NotifyEvent>>();

        let mut watcher: RecommendedWatcher = notify::recommended_watcher(sync_tx)?;

        for path in &self.config.watch_paths {
            let resolved = if path.to_string_lossy() == "." {
                std::env::current_dir()?
            } else {
                path.clone()
            };

            if resolved.exists() {
                watcher.watch(&resolved, RecursiveMode::Recursive)?;
                info!("Watching: {}", resolved.display());
            } else {
                warn!("Watch path does not exist: {}", resolved.display());
            }
        }

        let debounce_ms = self.config.debounce_ms;
        let ignore_patterns = self.config.ignore_patterns.clone();
        let extensions = self.config.extensions.clone();

        tokio::task::spawn_blocking(move || {
            let _watcher = watcher; // keep alive
            let mut last_event_time = std::time::Instant::now();

            loop {
                match sync_rx.recv() {
                    Ok(Ok(notify_event)) => {
                        let now = std::time::Instant::now();
                        if now.duration_since(last_event_time).as_millis()
                            < debounce_ms as u128
                        {
                            continue;
                        }
                        last_event_time = now;

                        for path in &notify_event.paths {
                            // Apply ignore filter
                            if Self::should_ignore(path, &ignore_patterns, &extensions) {
                                continue;
                            }

                            if let Some(kind) =
                                Self::classify_event(&notify_event.kind, path)
                            {
                                let content = format!(
                                    "{}: {}",
                                    match &kind {
                                        MnemonicEventKind::FileCreated => "File created",
                                        MnemonicEventKind::FileModified => "File modified",
                                        MnemonicEventKind::FileDeleted => "File deleted",
                                        MnemonicEventKind::DependencyAdded =>
                                            "Dependency changed",
                                        _ => "File event",
                                    },
                                    path.display()
                                );

                                let event = Event::new(
                                    EventSource::FileWatcher,
                                    kind,
                                    &content,
                                )
                                .with_metadata(serde_json::json!({
                                    "path": path.to_string_lossy(),
                                    "extension": path.extension()
                                        .map(|e| e.to_string_lossy().to_string()),
                                }));

                                debug!("File event: {content}");

                                if tx.blocking_send(event).is_err() {
                                    return;
                                }
                            }
                        }
                    }
                    Ok(Err(e)) => {
                        warn!("File watch error: {e}");
                    }
                    Err(_) => {
                        return;
                    }
                }
            }
        });

        Ok(())
    }
}
