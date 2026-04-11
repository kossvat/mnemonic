use anyhow::Result;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{error, info, warn};

use crate::api::ApiServer;
use crate::classifier::rules::RuleClassifier;
use crate::classifier::Classifier;
use crate::config::Config;
use crate::event::Event;
use crate::output::memory_files::MemoryFileSink;
use crate::storage::{OutputSink, Storage};
use crate::watcher::files::FileWatcher;
use crate::watcher::git::GitWatcher;
use crate::watcher::Watcher;

pub struct Daemon {
    config: Config,
}

impl Daemon {
    pub fn new(config: Config) -> Self {
        Self { config }
    }

    pub async fn run(&self) -> Result<()> {
        self.write_pid()?;

        // Storage (shared with API server)
        let storage = Arc::new(Storage::open(&self.config.storage.db_path)?);
        info!("Storage ready: {} memories", storage.count()?);

        // Classifier
        let classifier = RuleClassifier::new(self.config.classifier.clone());

        // Output sinks
        let mut sinks: Vec<Box<dyn OutputSink>> = Vec::new();
        if self.config.output.memory_files_enabled {
            sinks.push(Box::new(MemoryFileSink::new(
                self.config.output.memory_files_path.clone(),
            )));
        }
        // Phase 2: WhisperSink, ObsidianSink

        info!(
            "Output sinks: {}",
            sinks.iter().map(|s| s.name()).collect::<Vec<_>>().join(", ")
        );

        // Start API server
        let api = ApiServer::new(
            self.config.daemon.socket_path.clone(),
            storage.clone(),
        );
        tokio::spawn(async move {
            if let Err(e) = api.start().await {
                error!("API server error: {e}");
            }
        });

        // Event channel
        let (tx, mut rx) = mpsc::channel::<Event>(256);

        // Start watchers
        let file_watcher = FileWatcher::new(self.config.watchers.clone());
        file_watcher.start(tx.clone()).await?;

        let cwd = std::env::current_dir()?;
        if cwd.join(".git").exists() {
            let git_watcher = GitWatcher::new(cwd);
            git_watcher.start(tx.clone()).await?;
        } else {
            warn!("No .git directory found, git watcher disabled");
        }

        info!("mnemonic daemon running. Watching for events...");

        // Event processing loop
        let mut batch: Vec<Event> = Vec::new();
        let batch_interval =
            tokio::time::Duration::from_secs(self.config.output.batch_interval_secs);
        let mut batch_timer = tokio::time::interval(batch_interval);

        loop {
            tokio::select! {
                Some(event) = rx.recv() => {
                    // Urgent events bypass batching
                    if event.kind == crate::event::EventKind::UserCorrection {
                        if let Some(entry) = classifier.classify(&event) {
                            if let Err(e) = storage.save(&entry) {
                                error!("Storage save error: {e}");
                            }
                            for sink in &sinks {
                                if let Err(e) = sink.write(&entry) {
                                    warn!("Sink {} error: {e}", sink.name());
                                }
                            }
                            info!("URGENT saved: {} [{}]", entry.title, entry.memory_type);
                        }
                    } else {
                        batch.push(event);
                    }
                }
                _ = batch_timer.tick() => {
                    if !batch.is_empty() {
                        self.process_batch(&batch, &classifier, &storage, &sinks);
                        batch.clear();
                    }
                }
                _ = tokio::signal::ctrl_c() => {
                    info!("Shutting down...");
                    if !batch.is_empty() {
                        self.process_batch(&batch, &classifier, &storage, &sinks);
                    }
                    self.cleanup();
                    return Ok(());
                }
            }
        }
    }

    fn process_batch(
        &self,
        batch: &[Event],
        classifier: &impl Classifier,
        storage: &Storage,
        sinks: &[Box<dyn OutputSink>],
    ) {
        let mut saved = 0;
        let mut skipped = 0;

        for event in batch {
            match classifier.classify(event) {
                Some(entry) => {
                    if let Err(e) = storage.save(&entry) {
                        error!("Storage save error: {e}");
                        continue;
                    }
                    for sink in sinks {
                        if let Err(e) = sink.write(&entry) {
                            warn!("Sink {} error: {e}", sink.name());
                        }
                    }
                    saved += 1;
                }
                None => {
                    skipped += 1;
                }
            }
        }

        if saved > 0 {
            info!("Batch: {saved} saved, {skipped} skipped");
        }
    }

    fn write_pid(&self) -> Result<()> {
        let pid_path = &self.config.daemon.pid_file;
        if let Some(parent) = pid_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(pid_path, std::process::id().to_string())?;
        Ok(())
    }

    fn cleanup(&self) {
        let _ = std::fs::remove_file(&self.config.daemon.pid_file);
        let _ = std::fs::remove_file(&self.config.daemon.socket_path);
        info!("Cleanup complete");
    }

    pub fn is_running(config: &Config) -> Option<u32> {
        let pid_path = &config.daemon.pid_file;
        if !pid_path.exists() {
            return None;
        }

        let pid_str = std::fs::read_to_string(pid_path).ok()?;
        let pid: u32 = pid_str.trim().parse().ok()?;

        let output = std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .output()
            .ok()?;

        if output.status.success() {
            Some(pid)
        } else {
            let _ = std::fs::remove_file(pid_path);
            None
        }
    }
}
