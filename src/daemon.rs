use anyhow::Result;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{error, info, warn};

use crate::api::ApiServer;
use crate::classifier::Classifier;
use crate::classifier::rules::RuleClassifier;
use crate::config::Config;
use crate::embedding::Embedder;
use crate::event::Event;
use crate::graph::extractor::{EntityExtractor, RuleExtractor};
use crate::scoring::ImportanceScorer;
use crate::storage::{OutputSink, Storage};
use crate::watcher::Watcher;
use crate::watcher::codex::CodexWatcher;
use crate::watcher::conversation::ConversationWatcher;
use crate::watcher::files::FileWatcher;
use crate::watcher::git::GitWatcher;

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

        // Embedder — auto-selects neural (384-dim) or hash (256-dim) based on features
        let embedder: Arc<dyn Embedder> = Arc::from(crate::embedding::create_embedder()?);
        let dedup_threshold = self.config.classifier.dedup_threshold;
        info!("Embedder ready (dedup threshold: {:.2})", dedup_threshold);

        // Fail fast, not silently broken: if stored embeddings were written by
        // a model of a different dimension (e.g. the neural model was swapped
        // without a reembed), every query vector mismatches — HNSW returns
        // nothing, brute-force scores 0, and dedup (is_duplicate) treats every
        // save as new, so duplicates pile up. Refuse to run on a mismatched
        // store; migration is a deliberate `stop -> reembed -> start` step.
        if let Ok(probe) = embedder.embed("probe") {
            let model = probe.len();
            let dims = storage.active_embedding_dims();
            if dims.iter().any(|&d| d != model) {
                let _ = std::fs::remove_file(&self.config.daemon.pid_file);
                anyhow::bail!(
                    "Active embeddings have dimension(s) {dims:?} but the current model produces \
                     {model}-dim vectors. Vector search and dedup would be broken on this store. \
                     Run `mnemonic reembed` before starting the daemon."
                );
            }
        }

        // Dynamic importance scorer
        let scorer = ImportanceScorer::default();

        // Entity extractor for knowledge graph. Rule-based is always on;
        // when llm.enabled=true we wrap it in a CompositeExtractor that also
        // calls an Ollama-compatible endpoint and merges results.
        let graph_extractor: Arc<dyn EntityExtractor> = if self.config.llm.enabled {
            match crate::graph::extractor_llm::OllamaBackend::new(&self.config.llm) {
                Ok(backend) => {
                    let llm = crate::graph::extractor_llm::LlmExtractor::new(
                        Box::new(backend),
                        storage.clone(),
                        &self.config.llm,
                    );
                    info!(
                        "Graph extractor: composite (rule-based + LLM via {} {})",
                        self.config.llm.endpoint, self.config.llm.model
                    );
                    Arc::new(crate::graph::extractor_llm::CompositeExtractor::new(
                        Box::new(RuleExtractor::new()),
                        Box::new(llm),
                    ))
                }
                Err(e) => {
                    warn!("LLM extractor disabled, backend init failed: {e}");
                    Arc::new(RuleExtractor::new())
                }
            }
        } else {
            info!("Graph extractor: rule-based only (set llm.enabled=true to add LLM)");
            Arc::new(RuleExtractor::new())
        };

        let importance_threshold = self.config.classifier.importance_threshold;
        info!("Scorer ready (threshold: {:.2})", importance_threshold);

        // Output sinks — shared builder with the MCP server so both save
        // paths write through the same set.
        let sinks: Vec<Box<dyn OutputSink>> = crate::output::build_sinks(&self.config);

        info!(
            "Output sinks: {}",
            sinks
                .iter()
                .map(|s| s.name())
                .collect::<Vec<_>>()
                .join(", ")
        );

        // Async extraction worker. When enabled (default), the daemon's
        // save path enqueues each new memory into `extraction_queue` and
        // returns in <100ms; this worker drains the queue on a tick and
        // runs the real entity extractor (rule-based + optional LLM)
        // without blocking ingestion. Set extraction.async_enabled=false
        // in config to fall back to the sync legacy path below.
        let async_extraction = self.config.extraction.async_enabled;
        if async_extraction {
            crate::extraction_worker::spawn_worker(
                storage.clone(),
                graph_extractor.clone(),
                self.config.extraction.worker_interval_secs,
                self.config.extraction.worker_batch_size,
            );
        }

        // Dream-consolidation worker. Periodically summarizes
        // recently-closed sessions into `session_summary` memories
        // so retrieval can surface high-level "what happened in
        // that session" results. Default cadence is hourly with
        // the heuristic summarizer (no LLM); users opt into LLM
        // prose via `[dream] use_llm = true`. Disabled entirely
        // via `[dream] enabled = false` — the CLI `mnemonic dream
        // batch` keeps working in either case.
        if self.config.dream.enabled {
            crate::dream_worker::spawn_worker(
                storage.clone(),
                self.config.dream.clone(),
                self.config.llm.clone(),
            );
        } else {
            info!("Dream worker disabled ([dream] enabled = false)");
        }

        // Contradiction lint — periodic flag-only pass over each active
        // project's decisions (see src/lint.rs). Deliberately its own
        // worker + config, NOT hidden inside dream (which defaults off):
        // the candidate layer is pure embedding math and safe everywhere;
        // LLM confirmation engages only when [llm] is enabled.
        if self.config.lint.enabled {
            let lint_storage = storage.clone();
            let lint_cfg = self.config.lint.clone();
            let llm_cfg = self.config.llm.clone();
            info!(
                "Lint worker starting (interval={}s, similarity={:.2}, llm={})",
                lint_cfg.interval_secs, lint_cfg.similarity, llm_cfg.enabled
            );
            tokio::spawn(async move {
                let mut ticker = tokio::time::interval(std::time::Duration::from_secs(
                    lint_cfg.interval_secs.max(60),
                ));
                loop {
                    ticker.tick().await;
                    let storage = lint_storage.clone();
                    let llm_cfg = llm_cfg.clone();
                    let sim = lint_cfg.similarity;
                    let res = tokio::task::spawn_blocking(move || {
                        let backend = if llm_cfg.enabled {
                            crate::graph::extractor_llm::OllamaBackend::new(&llm_cfg).ok()
                        } else {
                            None
                        };
                        crate::lint::run_lint_pass(
                            &storage,
                            backend
                                .as_ref()
                                .map(|b| b as &dyn crate::graph::extractor_llm::LlmBackend),
                            sim,
                        )
                    })
                    .await;
                    match res {
                        Ok(Ok(stats)) if stats.confirmed > 0 || stats.candidates_new > 0 => {
                            info!(
                                "Lint: {} confirmed, {} new candidates, {} dismissed",
                                stats.confirmed, stats.candidates_new, stats.dismissed
                            );
                        }
                        Ok(Ok(_)) => {}
                        Ok(Err(e)) => tracing::warn!("Lint pass failed: {e}"),
                        Err(e) => tracing::warn!("Lint join error: {e}"),
                    }
                }
            });
        } else {
            info!("Lint worker disabled ([lint] enabled = false)");
        }

        // Work-activity sampler. Reads system idle time on a tick and
        // accumulates accurate daily "time worked" into a separate
        // activity.db (never touches memory.db). Disabled via
        // `[activity] enabled = false`. Failure to open the activity DB
        // is non-fatal — the rest of the daemon runs regardless.
        if self.config.activity.enabled {
            match crate::activity::ActivityStore::open_for_daemon(
                &self.config.activity_db_path(),
                self.config.activity.min_session_secs,
            ) {
                Ok(store) => {
                    let activity_store = std::sync::Arc::new(store);
                    crate::activity_worker::spawn_worker(
                        activity_store.clone(),
                        self.config.activity.clone(),
                    );
                    // Project-time attribution: correlate sessions with the
                    // memory graph to assign hours per project. Shares the same
                    // ActivityStore + the memory Storage. Backfills 14 days on
                    // startup, recomputes today every 10 min.
                    crate::attribution_worker::spawn_worker(
                        activity_store,
                        storage.clone(),
                        600,
                        14,
                    );
                }
                Err(e) => warn!("Activity store open failed, sampler off this run: {e}"),
            }
        } else {
            info!("Activity worker disabled ([activity] enabled = false)");
        }

        // Peer attribution. When `peers.auto_tag = true` (default), every
        // saved memory gets linked to a "user" peer as speaker, and
        // memories from the conversation watcher additionally get linked
        // to an "agent" peer as participant. Both peers are upserted now
        // so the first save can rely on them existing. If upsert fails
        // (DB locked, etc.), we degrade gracefully — attributor stays
        // None and save paths skip the linking step.
        let attributor = if self.config.peers.auto_tag {
            match PeerAttributor::init(&storage, &self.config.peers) {
                Ok(a) => {
                    info!(
                        "Peer auto-tagging on: user={} ({}), agent={} ({})",
                        a.user_name, a.user_peer_id, a.agent_name, a.agent_peer_id
                    );
                    Some(a)
                }
                Err(e) => {
                    warn!("Peer attributor init failed, continuing without auto-tagging: {e}");
                    None
                }
            }
        } else {
            info!("Peer auto-tagging disabled (peers.auto_tag = false)");
            None
        };

        // Session tracker — groups conversation-watcher memories into
        // sessions keyed by JSONL file path with `sessions.idle_timeout_secs`
        // expiration. Only created when peer auto-tagging is on, because
        // it depends on the agent peer id; without auto-tag, conversation
        // memories don't get session attribution either.
        let session_tracker = attributor.as_ref().map(|a| {
            std::sync::Mutex::new(SessionTracker::new(
                a.agent_peer_id.clone(),
                std::time::Duration::from_secs(self.config.sessions.idle_timeout_secs),
            ))
        });
        if session_tracker.is_some() {
            info!(
                "Session tracker on: idle timeout {}s",
                self.config.sessions.idle_timeout_secs
            );
        }

        // Idle-session sweeper. The tracker above closes a session only
        // when the same key emits a LATER event; sessions whose key never
        // fires again (daemon restart, one-off conversations) stayed open
        // forever, starving everything that consumes CLOSED sessions
        // (dream consolidation, session summaries). First tick fires
        // immediately, so sessions orphaned by a previous run are closed
        // right at startup.
        {
            let sweep_storage = storage.clone();
            let idle_secs = self.config.sessions.idle_timeout_secs;
            tokio::spawn(async move {
                let mut iv = tokio::time::interval(std::time::Duration::from_secs(300));
                loop {
                    iv.tick().await;
                    match sweep_storage.close_idle_sessions(idle_secs) {
                        Ok(0) => {}
                        Ok(n) => info!("Session sweep: closed {n} idle sessions"),
                        Err(e) => warn!("Session sweep failed: {e}"),
                    }
                }
            });
        }

        // Start API server (unix socket — for MCP and CLI clients). Shares
        // the daemon's embedder so /embed serves MCP processes from the ONE
        // resident model copy instead of each loading its own.
        let api = ApiServer::new(
            self.config.daemon.socket_path.clone(),
            storage.clone(),
            embedder.clone(),
        );
        tokio::spawn(async move {
            if let Err(e) = api.start().await {
                error!("API server error: {e}");
            }
        });

        // Optional HTTP dashboard API. Opt-in via [ui] enabled=true.
        // Shares the daemon's embedder so /api/search doesn't reload the
        // ONNX model per request.
        if self.config.ui.enabled {
            let cfg = self.config.clone();
            let st = storage.clone();
            let emb = embedder.clone();
            tokio::spawn(async move {
                if let Err(e) = crate::http::serve(cfg, st, emb).await {
                    error!("Dashboard HTTP server error: {e}");
                }
            });
        }

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

        // Start conversation watcher (Claude Code JSONL sessions)
        if self.config.watchers.conversation_enabled {
            let sessions_dir = self
                .config
                .watchers
                .conversation_sessions_dir
                .clone()
                .unwrap_or_else(|| {
                    dirs::home_dir()
                        .unwrap_or_default()
                        .join(".claude/projects")
                });
            if sessions_dir.exists() {
                // Offsets persist next to the DB so a restart resumes
                // reading session JSONLs where the previous run stopped
                // instead of skipping everything written while down.
                let mut conv_watcher = ConversationWatcher::new(sessions_dir.clone());
                if let Some(dir) = self.config.storage.db_path.parent() {
                    conv_watcher = conv_watcher.with_state_path(dir.join("watcher_offsets.json"));
                }
                let conv_tx = tx.clone();
                tokio::spawn(async move {
                    if let Err(e) = conv_watcher.start(conv_tx).await {
                        error!("Conversation watcher error: {e}");
                    }
                });
                info!(
                    "Conversation watcher monitoring: {}",
                    sessions_dir.display()
                );
            } else {
                warn!(
                    "Sessions dir not found: {}, conversation watcher disabled",
                    sessions_dir.display()
                );
            }
        }

        // Start Codex watcher (Codex CLI rollout transcripts). Mirrors the
        // conversation watcher: offsets persist next to the DB so restarts
        // resume instead of skipping to EOF.
        if self.config.watchers.codex_enabled {
            let codex_sessions_dir = self
                .config
                .watchers
                .codex_sessions_dir
                .clone()
                .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".codex/sessions"));
            if codex_sessions_dir.exists() {
                let mut codex_watcher = CodexWatcher::new(codex_sessions_dir.clone());
                // Archived sessions live in a sibling flat dir next to the
                // sessions root (e.g. ~/.codex/archived_sessions next to
                // ~/.codex/sessions). Derive it from the configured root so
                // a non-default codex_sessions_dir finds its own archives
                // instead of the real home's.
                if let Some(parent) = codex_sessions_dir.parent() {
                    let archived = parent.join("archived_sessions");
                    if archived.exists() {
                        codex_watcher = codex_watcher.with_archived_dir(archived);
                    }
                }
                if let Some(dir) = self.config.storage.db_path.parent() {
                    codex_watcher =
                        codex_watcher.with_state_path(dir.join("codex_watcher_offsets.json"));
                }
                let codex_tx = tx.clone();
                tokio::spawn(async move {
                    if let Err(e) = codex_watcher.start(codex_tx).await {
                        error!("Codex watcher error: {e}");
                    }
                });
                info!("Codex watcher monitoring: {}", codex_sessions_dir.display());
            } else {
                warn!(
                    "Codex sessions dir not found: {}, Codex watcher disabled",
                    codex_sessions_dir.display()
                );
            }
        }

        info!("mnemonic daemon running. Watching for events...");

        // Event processing loop
        let mut batch: Vec<Event> = Vec::new();
        let batch_interval =
            tokio::time::Duration::from_secs(self.config.output.batch_interval_secs);
        let mut batch_timer = tokio::time::interval(batch_interval);

        // `mnemonic stop` and launchd terminate the daemon with SIGTERM
        // (stop_daemon: SIGTERM → 5s poll → SIGKILL); only an interactive
        // Ctrl-C delivers SIGINT. Listening to SIGINT alone meant every
        // normal stop killed the process mid-batch — up to
        // batch_interval_secs of classified events vanished while the CLI
        // reported a graceful shutdown.
        let mut sigterm =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;

        loop {
            tokio::select! {
                Some(event) = rx.recv() => {
                    // Urgent events bypass batching
                    if event.kind == crate::event::EventKind::UserCorrection {
                        if let Some(entry) = classifier.classify(&event) {
                            let emb = {
                                let text = format!("{} {}", entry.title, entry.content);
                                embedder.embed(&text).ok()
                            };
                            if let Err(e) = storage.save_with_embedding(&entry, emb.as_ref()) {
                                error!("Storage save error: {e}");
                                // Don't enqueue extraction or invoke sinks
                                // for a row that didn't make it into the DB.
                                // The worker would just see Ok(None) and
                                // dequeue silently; sinks would write to
                                // disk referencing a non-existent memory.
                                continue;
                            }
                            // Peer attribution + session link. Turn-aware
                            // when the memory carries `role` metadata
                            // from the conversation watcher; jsonl_path
                            // metadata also routes to SessionTracker.
                            // Errors logged at warn; never block save flow.
                            if let Some(att) = attributor.as_ref() {
                                att.attribute(&storage, &entry, session_tracker.as_ref());
                            }
                            // Knowledge-graph extraction. Async path (default)
                            // just enqueues the id and lets the background
                            // worker do the LLM round-trip. Sync path retains
                            // the legacy in-line extract for users who turn
                            // async off or run without an event loop.
                            if async_extraction {
                                if let Err(e) = storage.enqueue_extraction(&entry.id) {
                                    warn!("Enqueue extraction error: {e}");
                                }
                            } else {
                                let extraction = graph_extractor.extract(&entry);
                                if (!extraction.entities.is_empty() || !extraction.edges.is_empty())
                                    && let Err(e) = storage.save_graph(&entry.id, &extraction.entities, &extraction.edges) {
                                        warn!("Graph save error: {e}");
                                    }
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
                        self.process_batch(&batch, &classifier, &storage, &sinks, &*embedder, dedup_threshold, &scorer, importance_threshold, &*graph_extractor, async_extraction, attributor.as_ref(), session_tracker.as_ref());
                        batch.clear();
                    }
                }
                _ = tokio::signal::ctrl_c() => {
                    info!("Shutting down (SIGINT)...");
                    break;
                }
                _ = sigterm.recv() => {
                    info!("Shutting down (SIGTERM)...");
                    break;
                }
            }
        }

        // Reached from both SIGINT and SIGTERM: flush the in-flight batch
        // before exiting so a routine stop/restart never drops events.
        if !batch.is_empty() {
            self.process_batch(
                &batch,
                &classifier,
                &storage,
                &sinks,
                &*embedder,
                dedup_threshold,
                &scorer,
                importance_threshold,
                &*graph_extractor,
                async_extraction,
                attributor.as_ref(),
                session_tracker.as_ref(),
            );
        }
        self.cleanup();
        // Exit here instead of returning: unwinding back through main drops
        // the tokio runtime, and runtime drop WAITS for in-flight
        // spawn_blocking work — a single LLM extraction call (multi-second
        // qwen round-trip) overshoots `mnemonic stop`'s 5s poll and earns a
        // pointless SIGKILL plus a "SIGTERM ignored" message. Everything
        // durable is already on disk at this point: SQLite commits per
        // transaction, the final batch is flushed above, PID/socket files
        // are cleaned, and tracing writes line-buffered stdout.
        info!("Shutdown complete");
        std::process::exit(0);
    }

    #[allow(clippy::too_many_arguments)]
    fn process_batch(
        &self,
        batch: &[Event],
        classifier: &impl Classifier,
        storage: &Storage,
        sinks: &[Box<dyn OutputSink>],
        embedder: &dyn Embedder,
        dedup_threshold: f32,
        scorer: &ImportanceScorer,
        importance_threshold: f32,
        graph_extractor: &dyn EntityExtractor,
        async_extraction: bool,
        attributor: Option<&PeerAttributor>,
        session_tracker: Option<&std::sync::Mutex<SessionTracker>>,
    ) {
        let mut saved = 0;
        let mut skipped = 0;
        let mut deduped = 0;
        let mut low_importance = 0;

        for event in batch {
            match classifier.classify(event) {
                Some(mut entry) => {
                    // Generate embedding
                    let text = format!("{} {}", entry.title, entry.content);
                    let emb = embedder.embed(&text).ok();

                    if let Some(ref embedding) = emb {
                        // Check for semantic duplicates
                        match storage.is_duplicate(embedding, dedup_threshold) {
                            Ok(Some(sim)) => {
                                info!("Dedup skip (sim={sim:.3}): {}", entry.title);
                                deduped += 1;
                                continue;
                            }
                            Ok(None) => {}
                            // A dimension mismatch is unreachable here: the startup
                            // guard refuses to run the daemon on a mismatched store.
                            // Other (transient) dedup errors: warn and save best-effort
                            // rather than drop the captured memory.
                            Err(e) => warn!("Dedup check error: {e}"),
                        }

                        // Dynamic importance scoring
                        match scorer.score(
                            embedding,
                            &event.kind,
                            &entry.memory_type,
                            &storage.conn,
                        ) {
                            Ok(score) => {
                                entry.importance = score;
                                if score < importance_threshold {
                                    info!(
                                        "Low importance ({score:.2} < {importance_threshold:.2}): {}",
                                        entry.title
                                    );
                                    low_importance += 1;
                                    continue;
                                }
                            }
                            Err(e) => warn!("Scoring error: {e}"),
                        }
                    }

                    if let Err(e) = storage.save_with_embedding(&entry, emb.as_ref()) {
                        error!("Storage save error: {e}");
                        continue;
                    }
                    // Peer auto-tag + session link. Turn-aware via
                    // `role` metadata; SessionTracker routes by
                    // `jsonl_path`. No-op if attributor is None.
                    if let Some(att) = attributor {
                        att.attribute(storage, &entry, session_tracker);
                    }
                    // Knowledge-graph extraction. Async path enqueues; sync
                    // path runs the LLM inline. See run() for context.
                    if async_extraction {
                        if let Err(e) = storage.enqueue_extraction(&entry.id) {
                            warn!("Enqueue extraction error: {e}");
                        }
                    } else {
                        let extraction = graph_extractor.extract(&entry);
                        if (!extraction.entities.is_empty() || !extraction.edges.is_empty())
                            && let Err(e) = storage.save_graph(
                                &entry.id,
                                &extraction.entities,
                                &extraction.edges,
                            )
                        {
                            warn!("Graph save error: {e}");
                        }
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

        if saved > 0 || deduped > 0 || low_importance > 0 {
            info!(
                "Batch: {saved} saved, {skipped} skipped, {deduped} deduped, {low_importance} low-importance"
            );
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

    /// Backwards-compatible thin wrapper for the old `is_running` API.
    /// Many existing call sites only care whether SOMETHING claims to
    /// be running. Returns `Some(pid)` if PID file exists AND that PID
    /// is alive; `None` otherwise. New code should prefer
    /// `Daemon::status_check` which distinguishes stale-pid from
    /// genuinely-stopped and probes the API socket for real liveness.
    pub fn is_running(config: &Config) -> Option<u32> {
        match Self::status_check(config) {
            DaemonStatus::Running { pid, .. } => Some(pid),
            _ => None,
        }
    }

    /// Multi-axis liveness check for the daemon. Codex caught that
    /// the previous `is_running` returned `Some(pid)` for ANY process
    /// matching the PID file — including zombies in uninterruptible
    /// kernel sleep that hold no sockets or ports. Widget showed
    /// "Stopped" but `mnemonic stop` thought it was running, leading
    /// to the lifecycle deadlock we just lived through.
    ///
    /// This helper checks three independent signals:
    /// 1. PID file → parsed PID
    /// 2. Kernel says PID exists (`kill -0`)
    /// 3. Unix-domain socket file exists AND connect()s in <500ms
    ///
    /// Returns a structured enum so callers can distinguish:
    /// - `Stopped` — no PID file or empty
    /// - `StalePid` — PID file exists but process is dead (auto-cleanup
    ///   candidate)
    /// - `Hung` — process alive but socket dead (zombie state; needs
    ///   forceful `mnemonic stop`)
    /// - `Running` — process alive AND socket responds
    pub fn status_check(config: &Config) -> DaemonStatus {
        let pid_path = &config.daemon.pid_file;
        let socket_path = &config.daemon.socket_path;

        if !pid_path.exists() {
            return DaemonStatus::Stopped;
        }

        let pid_str = match std::fs::read_to_string(pid_path) {
            Ok(s) => s,
            Err(_) => return DaemonStatus::Stopped,
        };
        let pid: u32 = match pid_str.trim().parse() {
            Ok(p) => p,
            Err(_) => return DaemonStatus::StalePid { pid: 0 },
        };

        // kill -0 checks process existence without sending a real signal.
        // Uses libc directly instead of forking a `kill` subprocess —
        // faster and lets us distinguish "doesn't exist" (ESRCH) from
        // "exists but I can't signal it" (EPERM, unlikely for own user).
        let alive = unsafe { libc::kill(pid as i32, 0) } == 0;
        if !alive {
            return DaemonStatus::StalePid { pid };
        }

        // Codex P1: PID exists, but is it OURS? macOS recycles PIDs;
        // after a reboot or long uptime, the PID we wrote could now
        // belong to an unrelated process. SIGTERM'ing that would be a
        // serious bug. Verify the executable path on the running PID
        // matches a `mnemonic` binary before treating it as our daemon.
        //
        // If the check is inconclusive (kernel call fails, executable
        // name unavailable, etc.) we err on the side of trusting the
        // PID file — false-positive StalePid would silently let the
        // daemon "die" from the CLI's perspective. False-positive
        // trust is bounded by the SIGTERM-first stop path (the
        // unrelated process catches it; SIGKILL only fires after 5s,
        // and the user sees the "ForcedExit" outcome explicitly).
        if !Self::pid_looks_like_mnemonic(pid) {
            return DaemonStatus::StalePid { pid };
        }

        // Process is alive AND looks like our daemon. Now probe the
        // socket — a hung daemon (kernel uninterruptible sleep,
        // deadlocked event loop, etc.) can hold the PID without
        // responding on the API socket.
        let socket_alive = Self::probe_socket(socket_path);

        if socket_alive {
            DaemonStatus::Running { pid }
        } else {
            DaemonStatus::Hung { pid }
        }
    }

    /// Check whether a PID's executable looks like the mnemonic
    /// daemon. macOS-only proc_pidpath query; on other platforms
    /// returns true (skips the check). Returns true on inconclusive
    /// results so we don't false-positive away a real running daemon.
    ///
    /// Match is by basename ending in "mnemonic" — covers
    /// `~/.cargo/bin/mnemonic`, `~/.local/bin/mnemonic`,
    /// `target/release/mnemonic`, etc. Doesn't require exact path
    /// equality because the binary can be at multiple symlinked
    /// locations (which is fine; we proved that today).
    fn pid_looks_like_mnemonic(pid: u32) -> bool {
        // Trivially: if the PID is OURS, of course it's mnemonic
        // (the running CLI). Saves a syscall on every status check
        // AND lets unit tests that write their own PID into the
        // PID file pass without elaborate mocking.
        if pid == std::process::id() {
            return true;
        }
        #[cfg(target_os = "macos")]
        {
            // proc_pidpath fills a buffer with the full executable
            // path. PROC_PIDPATHINFO_MAXSIZE = 4*MAXPATHLEN = 4096.
            // Use a generous local buffer; the syscall is cheap.
            const PROC_PIDPATHINFO_MAXSIZE: usize = 4096;
            let mut buf = vec![0u8; PROC_PIDPATHINFO_MAXSIZE];
            // proc_pidpath signature:
            //   int proc_pidpath(int pid, void *buffer, uint32_t buffersize);
            unsafe extern "C" {
                fn proc_pidpath(
                    pid: libc::c_int,
                    buffer: *mut libc::c_void,
                    buffersize: u32,
                ) -> libc::c_int;
            }
            let n = unsafe {
                proc_pidpath(
                    pid as libc::c_int,
                    buf.as_mut_ptr() as *mut libc::c_void,
                    buf.len() as u32,
                )
            };
            if n <= 0 {
                // Couldn't read path (permission denied, race, etc.).
                // Inconclusive → trust the PID file.
                return true;
            }
            buf.truncate(n as usize);
            let path = String::from_utf8_lossy(&buf);
            let basename = std::path::Path::new(path.as_ref())
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("");
            basename == "mnemonic"
        }
        #[cfg(not(target_os = "macos"))]
        {
            // On other platforms /proc/<pid>/exe could be read, but
            // we're not targeting them for the CLI today. Skip the
            // check (return true = trust).
            let _ = pid;
            true
        }
    }

    /// launchd label for the user LaunchAgent that owns the daemon on
    /// macOS. Single source of truth so the deprecation guard, doctor,
    /// and install all agree on the name.
    pub const LAUNCHD_LABEL: &str = "com.kossvat.mnemonic.daemon";

    /// Is the launchd LaunchAgent loaded? Used to gate the deprecated
    /// manual `start -d` path: when launchd owns the daemon, a manual
    /// background start races it and produces the dual-daemon hang we
    /// spent hours debugging. Returns false on non-macOS or if
    /// `launchctl` is unavailable (then manual start is the only path).
    pub fn launchd_service_loaded() -> bool {
        if !cfg!(target_os = "macos") {
            return false;
        }
        std::process::Command::new("launchctl")
            .arg("list")
            .output()
            .map(|o| {
                o.status.success()
                    && String::from_utf8_lossy(&o.stdout)
                        .lines()
                        .any(|l| l.contains(Self::LAUNCHD_LABEL))
            })
            .unwrap_or(false)
    }

    /// Try to connect to the unix-domain API socket. Returns true if
    /// connect() succeeds within ~500ms. Doesn't send any data — just
    /// proves the daemon is accepting connections.
    ///
    /// The 500ms cap matters: a hung daemon may have its socket file
    /// on disk but not accept connections; without a deadline this
    /// probe would block indefinitely. We use std::os::unix::net
    /// directly with an explicit timeout via a worker thread + recv.
    fn probe_socket(socket_path: &std::path::Path) -> bool {
        if !socket_path.exists() {
            return false;
        }
        use std::os::unix::net::UnixStream;
        let (tx, rx) = std::sync::mpsc::channel();
        let path_owned = socket_path.to_path_buf();
        std::thread::spawn(move || {
            let result = UnixStream::connect(&path_owned).is_ok();
            let _ = tx.send(result);
        });
        rx.recv_timeout(std::time::Duration::from_millis(500))
            .unwrap_or(false)
    }

    /// Stop a running daemon properly: SIGTERM → poll for exit up to
    /// 5s → SIGKILL → cleanup PID and socket files. Returns the
    /// terminal state (`Stopped` on success, an error if the process
    /// resisted SIGKILL).
    ///
    /// Codex caught that the old `Commands::Stop` just SIGTERM'd and
    /// printed "Stopped" without verifying. A hung daemon would
    /// happily stay running while the CLI claimed it died, then
    /// `Commands::Start` would refuse with "already running" — the
    /// exact deadlock we just hit.
    ///
    /// `force_kill_after_secs = 5` by default; callers can adjust if
    /// they need a different SLA (a CLI flag could expose this).
    pub fn stop_running_daemon(config: &Config, force_kill_after_secs: u64) -> Result<StopOutcome> {
        let pid = match Self::status_check(config) {
            DaemonStatus::Stopped => return Ok(StopOutcome::AlreadyStopped),
            DaemonStatus::StalePid { pid } => {
                Self::cleanup_stale_state(config);
                return Ok(StopOutcome::StaleCleaned { pid });
            }
            DaemonStatus::Running { pid } | DaemonStatus::Hung { pid } => pid,
        };

        // SIGTERM first — gives the daemon a chance to flush state.
        unsafe {
            libc::kill(pid as i32, libc::SIGTERM);
        }

        // Poll up to N seconds for graceful exit.
        let deadline =
            std::time::Instant::now() + std::time::Duration::from_secs(force_kill_after_secs);
        while std::time::Instant::now() < deadline {
            let alive = unsafe { libc::kill(pid as i32, 0) } == 0;
            if !alive {
                Self::cleanup_stale_state(config);
                return Ok(StopOutcome::GracefulExit { pid });
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }

        // SIGTERM ignored — escalate to SIGKILL.
        unsafe {
            libc::kill(pid as i32, libc::SIGKILL);
        }
        // Brief grace for the OS to reap; if the process is in
        // uninterruptible sleep (D state) even SIGKILL won't pierce
        // until kernel I/O completes. Return Forced anyway so the
        // caller knows we did our part; user can reboot if it sticks.
        std::thread::sleep(std::time::Duration::from_millis(500));
        let still_alive = unsafe { libc::kill(pid as i32, 0) } == 0;
        Self::cleanup_stale_state(config);
        if still_alive {
            Ok(StopOutcome::ForcedAndStuck { pid })
        } else {
            Ok(StopOutcome::ForcedExit { pid })
        }
    }

    /// Remove the PID file + socket file. Used after stop and on
    /// startup when we detect a stale PID. Errors silently — these
    /// files may not exist or may already be cleaned by the daemon's
    /// own `cleanup()`.
    pub fn cleanup_stale_state(config: &Config) {
        let _ = std::fs::remove_file(&config.daemon.pid_file);
        let _ = std::fs::remove_file(&config.daemon.socket_path);
    }

    /// Public wrapper for `probe_socket` so the `mnemonic doctor`
    /// command can ping the socket directly without going through
    /// `status_check` (doctor wants to report PID and socket as
    /// independent signals).
    pub fn probe_socket_for_doctor(socket_path: &std::path::Path) -> bool {
        Self::probe_socket(socket_path)
    }
}

/// Result of `Daemon::status_check`. Distinguishes the four real
/// states: Stopped, StalePid (process dead, files linger), Hung
/// (process alive, socket dead), Running (everything live).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DaemonStatus {
    Stopped,
    StalePid { pid: u32 },
    Hung { pid: u32 },
    Running { pid: u32 },
}

impl DaemonStatus {
    /// True only for `Running` — Hung/Stale/Stopped all mean the
    /// daemon can't serve requests. Callers who want "is anything
    /// pretending to run" should match the variants explicitly.
    /// Public API surface for future callers (widget, dashboard,
    /// MCP); the CLI matches variants directly today.
    #[allow(dead_code)]
    pub fn is_healthy(&self) -> bool {
        matches!(self, DaemonStatus::Running { .. })
    }

    /// Short human-readable label for status output / doctor.
    /// Same future-API rationale as `is_healthy`.
    #[allow(dead_code)]
    pub fn label(&self) -> &'static str {
        match self {
            DaemonStatus::Stopped => "not running",
            DaemonStatus::StalePid { .. } => "stale PID (process dead, files lingering)",
            DaemonStatus::Hung { .. } => "hung (process alive but socket unresponsive)",
            DaemonStatus::Running { .. } => "running",
        }
    }
}

/// Result of `Daemon::stop_running_daemon`. Lets the CLI print an
/// accurate human-readable message instead of the previous
/// always-print-Stopped behavior.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopOutcome {
    AlreadyStopped,
    StaleCleaned { pid: u32 },
    GracefulExit { pid: u32 },
    ForcedExit { pid: u32 },
    ForcedAndStuck { pid: u32 },
}

/// Attaches default peer roles to each saved memory so retrieval can
/// later answer "what did User say?" vs "what did Claude say?".
///
/// Two regimes:
///
/// 1. **Turn-aware attribution** (when the memory's metadata carries
///    `role` from the conversation watcher). The role tells us whose
///    turn produced the line, so we can attribute speaker vs addressee
///    correctly:
///    - `role = "user"`     → user = speaker, agent = addressee
///    - `role = "assistant"` → agent = speaker, user = addressee
///
/// 2. **Fallback** (when role metadata is absent — Manual events, older
///    flows, non-conversation sources):
///    - Every memory → user peer as `speaker`
///    - ConversationWatcher memories (no role) → ALSO agent as
///      `participant` (neutral, covers both directions)
///    - Manual / Socket / File / Git events → user-only
///
/// Future watchers (Codex JSONL, voice input, etc.) will register
/// additional peers and tag their own events with `role` so the
/// turn-aware path handles them too.
pub(crate) struct PeerAttributor {
    user_peer_id: String,
    agent_peer_id: String,
    codex_peer_id: String,
    user_name: String,
    agent_name: String,
    codex_name: String,
}

impl PeerAttributor {
    /// Upsert the configured user + agent peers at daemon startup and
    /// cache their ids. Subsequent attribute() calls are pure index
    /// lookups (no name resolution per save).
    ///
    /// pub(crate): the MCP server builds one too, so MCP saves get the
    /// same peer attribution as daemon-captured memories.
    pub(crate) fn init(
        storage: &Storage,
        cfg: &crate::config::PeersConfig,
    ) -> anyhow::Result<Self> {
        let user_peer_id = storage.upsert_peer(&cfg.user_name, Some(&cfg.user_display), "human")?;
        let agent_peer_id =
            storage.upsert_peer(&cfg.agent_name, Some(&cfg.agent_display), "agent")?;
        let codex_peer_id = storage.upsert_peer(
            &cfg.codex_agent_name,
            Some(&cfg.codex_agent_display),
            "agent",
        )?;
        Ok(Self {
            user_peer_id,
            agent_peer_id,
            codex_peer_id,
            user_name: cfg.user_name.clone(),
            agent_name: cfg.agent_name.clone(),
            codex_name: cfg.codex_agent_name.clone(),
        })
    }

    /// Extract the `role` field from an entry's metadata, if present.
    /// Returns `Some("user")` / `Some("assistant")` or `None` for
    /// entries that don't carry per-turn role info.
    fn role_from_metadata(entry: &crate::event::MemoryEntry) -> Option<&str> {
        entry.metadata.get("role").and_then(|v| v.as_str())
    }

    /// Extract the `jsonl_path` field from an entry's metadata, if
    /// present. Used by `attribute` to route the memory into its
    /// per-file session via the SessionTracker.
    fn jsonl_path_from_metadata(entry: &crate::event::MemoryEntry) -> Option<&str> {
        entry.metadata.get("jsonl_path").and_then(|v| v.as_str())
    }

    /// Which agent peer this memory attributes to. Codex-watcher memories
    /// carry `agent: "codex"` in metadata and route to the dedicated Codex
    /// peer; everything else uses the default agent peer (Claude). Without
    /// this, Codex turns would be mislabeled as Claude in the graph.
    fn agent_peer_for(&self, entry: &crate::event::MemoryEntry) -> (&str, &str) {
        match entry.metadata.get("agent").and_then(|v| v.as_str()) {
            Some("codex") => (&self.codex_peer_id, &self.codex_name),
            _ => (&self.agent_peer_id, &self.agent_name),
        }
    }

    /// Link a freshly-saved memory to its peer(s) AND to its session
    /// (when the metadata carries a JSONL path). Errors are logged at
    /// warn but never propagate — peer/session attribution is
    /// non-essential metadata; a failure here must not break ingestion.
    ///
    /// `tracker` is `Mutex<SessionTracker>` shared across the daemon
    /// loop because session ids are cached in-memory and reuse across
    /// memories from the same JSONL file is the whole point. Passing
    /// `None` skips session linking (e.g. when the daemon hasn't built
    /// a tracker, or for tests that don't care about sessions).
    pub(crate) fn attribute(
        &self,
        storage: &Storage,
        entry: &crate::event::MemoryEntry,
        tracker: Option<&std::sync::Mutex<SessionTracker>>,
    ) {
        // Phase 1: peer roles (turn-aware when role is present).
        let role = Self::role_from_metadata(entry);
        // Codex turns attribute to the Codex peer, not the default Claude
        // agent peer, so the graph reflects who actually spoke.
        let (agent_id, agent_label) = self.agent_peer_for(entry);
        match role {
            Some("user") => {
                self.link(
                    storage,
                    &entry.id,
                    &self.user_peer_id,
                    "speaker",
                    &self.user_name,
                );
                self.link(storage, &entry.id, agent_id, "addressee", agent_label);
            }
            Some("assistant") => {
                self.link(storage, &entry.id, agent_id, "speaker", agent_label);
                self.link(
                    storage,
                    &entry.id,
                    &self.user_peer_id,
                    "addressee",
                    &self.user_name,
                );
            }
            Some(other) => {
                // Unknown role: fall back to user=speaker and log so we
                // notice if a watcher starts emitting a new role string.
                warn!(
                    "PeerAttributor: unknown role `{other}` on entry {}, using fallback",
                    entry.id
                );
                self.attribute_fallback(storage, entry);
            }
            None => self.attribute_fallback(storage, entry),
        }

        // Phase 2: session linkage. Only conversation-watcher memories
        // carry `jsonl_path`; if absent (Manual, File, Git events), the
        // memory stays session-less and that's correct — they don't
        // belong to any conversation thread.
        if let (Some(jsonl_path), Some(tracker)) = (Self::jsonl_path_from_metadata(entry), tracker)
        {
            // Lock contention is minimal: only the daemon's single
            // event loop calls this. Poisoned mutex would mean the
            // tracker is unusable; we degrade by skipping session
            // attribution rather than panicking.
            match tracker.lock() {
                Ok(mut t) => match t.for_jsonl(storage, jsonl_path, Some(agent_id)) {
                    Ok(session_id) => {
                        if let Err(e) = storage.set_memory_session(&entry.id, Some(&session_id)) {
                            warn!(
                                "set_memory_session({}, {}) failed: {e}",
                                entry.id,
                                &session_id[..8.min(session_id.len())]
                            );
                        }
                    }
                    Err(e) => warn!("SessionTracker::for_jsonl({jsonl_path}) failed: {e}"),
                },
                Err(poisoned) => {
                    warn!("SessionTracker mutex poisoned, skipping session link: {poisoned}")
                }
            }
        }
    }

    /// Pre-turn-parsing behavior for entries without role metadata.
    /// User as speaker on every memory; agent as `participant` (neutral
    /// role) on conversation-watcher events; non-conversation events
    /// (Manual, File, Git) stay user-only.
    fn attribute_fallback(&self, storage: &Storage, entry: &crate::event::MemoryEntry) {
        self.link(
            storage,
            &entry.id,
            &self.user_peer_id,
            "speaker",
            &self.user_name,
        );
        if matches!(
            entry.source,
            crate::event::EventSource::ConversationWatcher
                | crate::event::EventSource::CodexWatcher
        ) {
            let (agent_id, agent_label) = self.agent_peer_for(entry);
            self.link(storage, &entry.id, agent_id, "participant", agent_label);
        }
    }

    fn link(
        &self,
        storage: &Storage,
        memory_id: &str,
        peer_id: &str,
        role: &str,
        peer_label: &str,
    ) {
        if let Err(e) = storage.link_memory_peer(memory_id, peer_id, role) {
            warn!("link_memory_peer({memory_id}, {peer_label}, {role}) failed: {e}");
        }
    }
}

/// Tracks the active session per JSONL file path so all memories from
/// one continuous chat window share the same `session_id`. Sessions
/// expire after `idle_timeout` of inactivity on that path; the next
/// memory on the same path opens a fresh session and ends the old one.
///
/// Restart-survivable: the JSONL path is persisted as
/// `sessions.external_key` and last activity as `last_activity_at`,
/// so a daemon cycle re-discovers the same open session via
/// `open_or_reuse_session_for_key` instead of leaking an orphan and
/// starting a fresh one. The in-RAM cache is purely a fast path that
/// avoids a DB round-trip on the hot streak of memories from the same
/// JSONL — on cache hit we just bump `last_activity_at` in the DB so
/// the persisted view stays current for the next restart.
///
/// Sessions opened via the tracker are tagged with source `"jsonl"`
/// so future cleanup scans can distinguish them from manually-opened
/// sessions (which the tracker must not touch).
pub(crate) struct SessionTracker {
    /// jsonl_path → (session_id, last_activity Instant). Instant is
    /// only for the in-process fast-path decision; the real
    /// last-activity timestamp lives in the DB.
    cache: std::collections::HashMap<String, (String, std::time::Instant)>,
    /// Peer id under which JSONL sessions are opened (typically the
    /// agent: Claude). `sessions.peer_id` is the primary peer in the
    /// table model; per-memory `memory_peers` carries the full set.
    primary_peer_id: String,
    idle_timeout: std::time::Duration,
}

impl SessionTracker {
    fn new(primary_peer_id: String, idle_timeout: std::time::Duration) -> Self {
        Self {
            cache: std::collections::HashMap::new(),
            primary_peer_id,
            idle_timeout,
        }
    }

    /// Get the active session id for a JSONL path. Three paths:
    ///
    /// 1. **Cache hit within idle window** — reuse the cached id,
    ///    refresh `last_activity_at` in the DB so a restart later
    ///    sees the session as still-fresh. No SELECT.
    /// 2. **Cache miss / stale** — delegate to
    ///    `open_or_reuse_session_for_key`, which atomically resolves
    ///    "same key, still open and fresh → reuse / stale → close at
    ///    real end-of-activity and open new / no row → open new".
    ///    Persists the result back into the cache.
    ///
    /// Errors propagate; a failure here doesn't block the memory save,
    /// it just means `set_memory_session` won't be called.
    fn for_jsonl(
        &mut self,
        storage: &Storage,
        jsonl_path: &str,
        owner_peer_id: Option<&str>,
    ) -> Result<String> {
        let now = std::time::Instant::now();

        if let Some((id, last)) = self.cache.get(jsonl_path).cloned()
            && now.duration_since(last) < self.idle_timeout
        {
            // Fast path: still within in-process idle window. Refresh
            // DB activity timestamp (cheap UPDATE) so restart survival
            // works; bump the Instant in the cache.
            if let Err(e) = storage.touch_session_activity(&id) {
                warn!("SessionTracker: touch_session_activity({id}) failed: {e}");
            }
            self.cache.insert(jsonl_path.to_string(), (id.clone(), now));
            return Ok(id);
        }

        // Slow path: cache miss OR in-RAM stale. Delegate to the
        // atomic helper — it checks the DB for an existing open
        // session under this key, reuses if fresh-by-DB-clock,
        // closes-then-opens-new if expired, opens fresh if absent.
        // This is the path that survives daemon restarts: the cache
        // is empty post-restart so every first event for a JSONL
        // funnels through here and finds the persisted open session.
        let label = jsonl_path_label(jsonl_path);
        // Codex sessions are owned by the Codex peer; everything else
        // falls back to the tracker's primary (Claude) peer. Without this,
        // peer-scoped session lists and dream summaries mislabel Codex
        // sessions as Claude even when the memories say speaker=codex.
        let owner = owner_peer_id.unwrap_or(&self.primary_peer_id);
        let session_id = storage.open_or_reuse_session_for_key(
            owner,
            jsonl_path,
            Some(&label),
            "jsonl",
            self.idle_timeout.as_secs(),
        )?;
        let was_reused = self
            .cache
            .get(jsonl_path)
            .is_some_and(|(cached, _)| cached == &session_id);
        self.cache
            .insert(jsonl_path.to_string(), (session_id.clone(), now));
        if !was_reused {
            info!(
                "SessionTracker: opened/resumed session {} for jsonl {}",
                &session_id[..8.min(session_id.len())],
                label
            );
        }
        Ok(session_id)
    }
}

/// Derive a human-readable label from a full JSONL path: use the file
/// stem (UUID portion) prefixed with the parent dir's name so the
/// `mnemonic session list` output is identifiable at a glance.
/// Falls back to the raw path if components can't be extracted.
fn jsonl_path_label(jsonl_path: &str) -> String {
    let p = std::path::Path::new(jsonl_path);
    let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or(jsonl_path);
    let parent = p
        .parent()
        .and_then(|d| d.file_name())
        .and_then(|s| s.to_str());
    match parent {
        Some(parent) => format!("{parent}/{stem}"),
        None => stem.to_string(),
    }
}

#[cfg(test)]
mod peer_attributor_tests {
    use super::*;
    use crate::config::PeersConfig;
    use crate::event::{EventSource, MemoryEntry, MemoryType};
    use chrono::Utc;

    fn tmp_storage() -> Arc<Storage> {
        let dir =
            std::env::temp_dir().join(format!("mnemonic-attributor-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        Arc::new(Storage::open(&dir.join("memory.db")).unwrap())
    }

    fn make_entry(source: EventSource) -> MemoryEntry {
        MemoryEntry {
            id: uuid::Uuid::new_v4().to_string(),
            timestamp: Utc::now(),
            title: "t".into(),
            content: "c".into(),
            memory_type: MemoryType::Note,
            tags: vec![],
            source,
            importance: 0.5,
            metadata: serde_json::Value::Null,
        }
    }

    /// Manual / FileWatcher / GitWatcher events get ONLY the user peer
    /// linked as speaker — no agent attribution since the user is the
    /// one editing files or committing.
    #[test]
    fn non_conversation_events_get_only_user_speaker() {
        let storage = tmp_storage();
        let att = PeerAttributor::init(&storage, &PeersConfig::default()).unwrap();

        for source in [
            EventSource::Manual,
            EventSource::FileWatcher,
            EventSource::GitWatcher,
        ] {
            let entry = make_entry(source.clone());
            storage.save(&entry).unwrap();
            att.attribute(&storage, &entry, None);

            let pairs = storage.peers_for_memory(&entry.id).unwrap();
            assert_eq!(
                pairs.len(),
                1,
                "source {source:?} should produce exactly one link"
            );
            let (peer, role) = &pairs[0];
            assert_eq!(peer.name, "user", "default user_name is the generic peer");
            assert_eq!(role, "speaker");
        }
    }

    /// ConversationWatcher events get BOTH user (speaker) and agent
    /// (participant) linked — that's the multi-agent attribution case
    /// the whole foundation exists for. Agent role is `participant`
    /// rather than `addressee` because we don't yet parse JSONL turns;
    /// the agent could be speaker (in an assistant-summary memory) or
    /// addressee (in a user-message memory).
    #[test]
    fn conversation_events_get_user_speaker_and_agent_participant() {
        let storage = tmp_storage();
        let att = PeerAttributor::init(&storage, &PeersConfig::default()).unwrap();

        let entry = make_entry(EventSource::ConversationWatcher);
        storage.save(&entry).unwrap();
        att.attribute(&storage, &entry, None);

        let pairs = storage.peers_for_memory(&entry.id).unwrap();
        assert_eq!(pairs.len(), 2, "conversation event should link 2 peers");
        // peers_for_memory orders by role ASC then name ASC.
        // Roles: "participant" < "speaker" alphabetically.
        let (p0, r0) = &pairs[0];
        let (p1, r1) = &pairs[1];
        assert_eq!(r0, "participant");
        assert_eq!(p0.name, "claude");
        assert_eq!(r1, "speaker");
        assert_eq!(p1.name, "user");
    }

    /// Double attribute (same entry, same attributor) is a no-op due to
    /// the (memory_id, peer_id, role) PRIMARY KEY on memory_peers.
    /// Important because a future retry path could redundantly call
    /// attribute() — must not produce duplicates or errors.
    #[test]
    fn attribute_is_idempotent_on_same_entry() {
        let storage = tmp_storage();
        let att = PeerAttributor::init(&storage, &PeersConfig::default()).unwrap();
        let entry = make_entry(EventSource::ConversationWatcher);
        storage.save(&entry).unwrap();

        att.attribute(&storage, &entry, None);
        att.attribute(&storage, &entry, None);
        att.attribute(&storage, &entry, None);

        let pairs = storage.peers_for_memory(&entry.id).unwrap();
        assert_eq!(
            pairs.len(),
            2,
            "repeated attribute must not duplicate links"
        );
    }

    /// Codex-watcher memories carry `agent: "codex"` and must attribute to
    /// the dedicated Codex peer, not the default Claude agent peer — else
    /// the graph mislabels who spoke (the bug the first loop review missed).
    #[test]
    fn codex_assistant_turn_attributes_to_codex_peer() {
        let storage = tmp_storage();
        let att = PeerAttributor::init(&storage, &PeersConfig::default()).unwrap();
        let mut entry = make_entry(EventSource::CodexWatcher);
        entry.metadata = serde_json::json!({"role": "assistant", "agent": "codex"});
        storage.save(&entry).unwrap();
        att.attribute(&storage, &entry, None);

        let pairs = storage.peers_for_memory(&entry.id).unwrap();
        let speaker = pairs
            .iter()
            .find(|(_, r)| r.as_str() == "speaker")
            .map(|(p, _)| p.name.as_str());
        assert_eq!(
            speaker,
            Some("codex"),
            "Codex assistant turn must attribute speaker=codex, not claude"
        );
        assert!(
            !pairs.iter().any(|(p, _)| p.name == "claude"),
            "Codex memory must not be labeled with the Claude peer"
        );
    }

    /// Custom config (different user/agent names) routes correctly —
    /// proves the wiring respects PeersConfig instead of hardcoding.
    #[test]
    fn attribute_honors_config_names() {
        let storage = tmp_storage();
        let cfg = PeersConfig {
            auto_tag: true,
            user_name: "alice".into(),
            user_display: "Alice".into(),
            agent_name: "codex".into(),
            agent_display: "Codex".into(),
            codex_agent_name: "codex-cli".into(),
            codex_agent_display: "Codex CLI".into(),
        };
        let att = PeerAttributor::init(&storage, &cfg).unwrap();
        let entry = make_entry(EventSource::ConversationWatcher);
        storage.save(&entry).unwrap();
        att.attribute(&storage, &entry, None);

        let pairs = storage.peers_for_memory(&entry.id).unwrap();
        let names: Vec<&str> = pairs.iter().map(|(p, _)| p.name.as_str()).collect();
        assert!(names.contains(&"alice"));
        assert!(names.contains(&"codex"));
    }

    /// Turn-aware: when metadata carries `role = "user"`, the user is
    /// the speaker and the agent becomes addressee (not `participant`).
    /// This is the path that lets retrieval distinguish "User said X"
    /// from "Claude said X" properly.
    #[test]
    fn role_user_metadata_attributes_user_speaker_agent_addressee() {
        let storage = tmp_storage();
        let att = PeerAttributor::init(&storage, &PeersConfig::default()).unwrap();
        let mut entry = make_entry(EventSource::ConversationWatcher);
        entry.metadata = serde_json::json!({"role": "user"});
        storage.save(&entry).unwrap();
        att.attribute(&storage, &entry, None);

        let pairs = storage.peers_for_memory(&entry.id).unwrap();
        let roles: std::collections::HashMap<&str, &str> = pairs
            .iter()
            .map(|(p, r)| (p.name.as_str(), r.as_str()))
            .collect();
        assert_eq!(roles.get("user"), Some(&"speaker"));
        assert_eq!(roles.get("claude"), Some(&"addressee"));
        // No `participant` rows — turn-aware path takes over from fallback.
        assert!(!pairs.iter().any(|(_, r)| r == "participant"));
    }

    /// Turn-aware: `role = "assistant"` flips speaker/addressee. The
    /// agent is the one who originated the line; the user is the one
    /// it was addressed to.
    #[test]
    fn role_assistant_metadata_flips_speaker_and_addressee() {
        let storage = tmp_storage();
        let att = PeerAttributor::init(&storage, &PeersConfig::default()).unwrap();
        let mut entry = make_entry(EventSource::ConversationWatcher);
        entry.metadata = serde_json::json!({"role": "assistant"});
        storage.save(&entry).unwrap();
        att.attribute(&storage, &entry, None);

        let roles: std::collections::HashMap<String, String> = storage
            .peers_for_memory(&entry.id)
            .unwrap()
            .into_iter()
            .map(|(p, r)| (p.name, r))
            .collect();
        assert_eq!(roles.get("claude").map(String::as_str), Some("speaker"));
        assert_eq!(roles.get("user").map(String::as_str), Some("addressee"));
    }

    /// SessionTracker reuses the same session id for consecutive calls
    /// from the same JSONL path; produces a different id on a different
    /// path. This is the basic "session = JSONL file" invariant.
    #[test]
    fn session_tracker_reuses_id_for_same_jsonl_and_distinguishes_files() {
        let storage = tmp_storage();
        let peer_id = storage.upsert_peer("claude", None, "agent").unwrap();
        let tracker = std::sync::Mutex::new(SessionTracker::new(
            peer_id,
            std::time::Duration::from_secs(60),
        ));

        let id_a1 = tracker
            .lock()
            .unwrap()
            .for_jsonl(&storage, "/sessions/aaa.jsonl", None)
            .unwrap();
        let id_a2 = tracker
            .lock()
            .unwrap()
            .for_jsonl(&storage, "/sessions/aaa.jsonl", None)
            .unwrap();
        assert_eq!(id_a1, id_a2, "same path within timeout must reuse id");

        let id_b = tracker
            .lock()
            .unwrap()
            .for_jsonl(&storage, "/sessions/bbb.jsonl", None)
            .unwrap();
        assert_ne!(id_a1, id_b, "different path must open a fresh session");
    }

    /// SessionTracker expires after idle_timeout: a second call past
    /// the timeout closes the old session and opens a new one. The
    /// closed session's `ended_at` should be populated in the DB.
    #[test]
    fn session_tracker_expires_after_idle_timeout_and_closes_old() {
        let storage = tmp_storage();
        let peer_id = storage.upsert_peer("claude", None, "agent").unwrap();
        let tracker = std::sync::Mutex::new(SessionTracker::new(
            peer_id,
            // Zero timeout = every call is "expired" except the first.
            std::time::Duration::from_secs(0),
        ));

        let first = tracker
            .lock()
            .unwrap()
            .for_jsonl(&storage, "/sessions/x.jsonl", None)
            .unwrap();
        // Tiny sleep to ensure `Instant::now()` advances past 0 ns.
        std::thread::sleep(std::time::Duration::from_millis(2));
        let second = tracker
            .lock()
            .unwrap()
            .for_jsonl(&storage, "/sessions/x.jsonl", None)
            .unwrap();
        assert_ne!(first, second, "expired session must be replaced");

        // Old session is closed in the DB; new one is open.
        let old = storage.session_by_id(&first).unwrap().unwrap();
        let new = storage.session_by_id(&second).unwrap().unwrap();
        assert!(!old.is_open(), "expired session should be closed");
        assert!(new.is_open(), "fresh session should be open");
    }

    /// Restart survival: a fresh SessionTracker (cache empty,
    /// simulating daemon restart) on the same JSONL path must resume
    /// the same session id that the previous tracker opened. Without
    /// the persistent `external_key` column this would silently open
    /// a second session and leak the first as "ongoing" — Codex's P1.
    #[test]
    fn session_tracker_resumes_open_session_across_restart() {
        let storage = tmp_storage();
        let peer_id = storage.upsert_peer("claude", None, "agent").unwrap();
        let path = "/sessions/restart-survival.jsonl";

        // First "daemon run": open a session.
        let first_id = {
            let t = std::sync::Mutex::new(SessionTracker::new(
                peer_id.clone(),
                std::time::Duration::from_secs(3600),
            ));
            t.lock().unwrap().for_jsonl(&storage, path, None).unwrap()
        };

        // Second "daemon run" — brand-new tracker, empty cache. Must
        // re-discover the same session via external_key, NOT open a
        // new one. Storage state is the only thing that persists.
        let resumed_id = {
            let t = std::sync::Mutex::new(SessionTracker::new(
                peer_id.clone(),
                std::time::Duration::from_secs(3600),
            ));
            t.lock().unwrap().for_jsonl(&storage, path, None).unwrap()
        };

        assert_eq!(
            first_id, resumed_id,
            "fresh tracker on same path must resume the same open session"
        );

        // Sanity: only one open session for this peer, not two.
        let opens = storage.open_sessions_for_peer(&peer_id, 10).unwrap();
        let same_key_opens = opens
            .iter()
            .filter(|s| s.external_key.as_deref() == Some(path))
            .count();
        assert_eq!(same_key_opens, 1, "must not leak duplicate open sessions");
    }

    /// Idle-expiry's `ended_at` reflects the real end-of-activity
    /// (last_activity + idle_timeout), NOT the moment the next event
    /// fires. Codex's P2: an overnight gap would otherwise close the
    /// session with the morning timestamp, distorting session windows
    /// for dream/summary features.
    #[test]
    fn session_tracker_backdates_ended_at_to_real_end_of_activity() {
        let storage = tmp_storage();
        let peer_id = storage.upsert_peer("claude", None, "agent").unwrap();
        let path = "/sessions/backdate.jsonl";

        // First call: open session and explicitly set last_activity_at
        // to "1 hour ago" so we can verify the backdate math without
        // needing to actually wait. Bypass the helper for setup.
        let opened_id = storage
            .open_or_reuse_session_for_key(&peer_id, path, Some("setup"), "jsonl", 3600)
            .unwrap();
        let one_hour_ago = (chrono::Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
        {
            let conn = storage.conn.lock().unwrap();
            conn.execute(
                "UPDATE sessions SET last_activity_at = ?1 WHERE id = ?2",
                rusqlite::params![one_hour_ago, opened_id],
            )
            .unwrap();
        }

        // Now trigger reuse with idle_secs = 60 (1 minute) — the
        // 1-hour gap is past idle, so the helper must close the old
        // session at `last_activity + 60s` and open a new one.
        let new_id = storage
            .open_or_reuse_session_for_key(&peer_id, path, Some("after"), "jsonl", 60)
            .unwrap();
        assert_ne!(opened_id, new_id, "stale session must be replaced");

        let old = storage.session_by_id(&opened_id).unwrap().unwrap();
        let ended_at_str = old.ended_at.expect("old session must be closed");
        let ended_at = chrono::DateTime::parse_from_rfc3339(&ended_at_str)
            .expect("ended_at must be RFC3339")
            .with_timezone(&chrono::Utc);
        let last_act = chrono::DateTime::parse_from_rfc3339(&one_hour_ago)
            .unwrap()
            .with_timezone(&chrono::Utc);
        let expected = last_act + chrono::Duration::seconds(60);

        // ended_at should equal last_activity + idle_timeout — i.e.,
        // roughly 59 minutes ago, NOT now. Allow 1s of fuzz for the
        // chrono round-trip through RFC3339 (sub-second precision varies).
        let drift = (ended_at - expected).num_seconds().abs();
        assert!(
            drift <= 1,
            "ended_at must equal last_activity + idle (drift {drift}s); \
             ended_at={ended_at} expected={expected}"
        );
        // Sanity: NOT close to "now" (which the old buggy behavior
        // would have produced).
        let now_drift = (chrono::Utc::now() - ended_at).num_seconds();
        assert!(
            now_drift > 60 * 50,
            "ended_at should be ~1h ago, not ~now (now_drift={now_drift}s)"
        );
    }

    /// End-to-end through `attribute`: a memory carrying both `role`
    /// and `jsonl_path` lands with the right peer roles AND its
    /// `session_id` set via the SessionTracker.
    #[test]
    fn attribute_with_metadata_links_session_and_peers() {
        let storage = tmp_storage();
        let att = PeerAttributor::init(&storage, &PeersConfig::default()).unwrap();
        let tracker = std::sync::Mutex::new(SessionTracker::new(
            att.agent_peer_id.clone(),
            std::time::Duration::from_secs(60),
        ));

        let mut entry = make_entry(EventSource::ConversationWatcher);
        entry.metadata = serde_json::json!({
            "role": "user",
            "jsonl_path": "/sessions/end-to-end.jsonl",
        });
        storage.save(&entry).unwrap();
        att.attribute(&storage, &entry, Some(&tracker));

        // Peer roles: turn-aware user=speaker, agent=addressee.
        let roles: std::collections::HashMap<String, String> = storage
            .peers_for_memory(&entry.id)
            .unwrap()
            .into_iter()
            .map(|(p, r)| (p.name, r))
            .collect();
        assert_eq!(roles.get("user").map(String::as_str), Some("speaker"));
        assert_eq!(roles.get("claude").map(String::as_str), Some("addressee"));

        // Session link: memory must appear in memories_for_session for
        // the JSONL's session id (resolved via the tracker cache).
        let session_id = tracker
            .lock()
            .unwrap()
            .for_jsonl(&storage, "/sessions/end-to-end.jsonl", None)
            .unwrap();
        let in_session = storage.memories_for_session(&session_id).unwrap();
        assert_eq!(in_session.len(), 1);
        assert_eq!(in_session[0].id, entry.id);
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;
    use std::io::Write;

    /// Build a minimal config pointing at a unique temp dir for each
    /// test — pid/socket files must not collide across parallel runs.
    fn tmp_config() -> Config {
        let dir =
            std::env::temp_dir().join(format!("mnemonic-lifecycle-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut c = Config::default();
        c.daemon.pid_file = dir.join("mnemonic.pid");
        // Unix-domain sockets on macOS have SUN_LEN=104 chars including
        // the null terminator. The temp_dir path can easily blow past
        // that with our UUID-suffixed dirs, so put the socket directly
        // under /tmp with a short unique stem. Bytes of randomness via
        // UUID's first 8 hex digits — collision risk is negligible
        // for tests but keeps stems distinct across parallel runs.
        let short_id: String = uuid::Uuid::new_v4().to_string().chars().take(8).collect();
        c.daemon.socket_path = std::path::PathBuf::from(format!("/tmp/m-{short_id}.sock"));
        c.storage.db_path = dir.join("memory.db");
        c
    }

    /// No PID file → Stopped.
    #[test]
    fn status_check_stopped_when_no_pid_file() {
        let cfg = tmp_config();
        assert_eq!(Daemon::status_check(&cfg), DaemonStatus::Stopped);
    }

    /// PID file points at a dead PID → StalePid. Uses PID 999999
    /// which is essentially guaranteed to not exist on a desktop
    /// (PID range is 32-bit but live processes rarely exceed 6 digits).
    #[test]
    fn status_check_stale_pid_when_process_dead() {
        let cfg = tmp_config();
        let mut f = std::fs::File::create(&cfg.daemon.pid_file).unwrap();
        write!(f, "999999").unwrap();
        match Daemon::status_check(&cfg) {
            DaemonStatus::StalePid { pid } => assert_eq!(pid, 999999),
            other => panic!("expected StalePid, got {other:?}"),
        }
    }

    /// PID file unparseable → StalePid with pid=0 (treated as junk
    /// to be cleaned up).
    #[test]
    fn status_check_treats_unparseable_pid_as_stale() {
        let cfg = tmp_config();
        std::fs::write(&cfg.daemon.pid_file, "not-a-number\n").unwrap();
        match Daemon::status_check(&cfg) {
            DaemonStatus::StalePid { pid } => assert_eq!(pid, 0),
            other => panic!("expected StalePid(0), got {other:?}"),
        }
    }

    /// PID file points at our own (alive) process, but no socket
    /// file → Hung. Demonstrates that liveness alone isn't enough;
    /// the socket signal catches the failure mode that broke us
    /// today (process alive, no API).
    #[test]
    fn status_check_hung_when_pid_alive_but_socket_missing() {
        let cfg = tmp_config();
        // Use our own PID — guaranteed alive during the test.
        let our_pid = std::process::id();
        std::fs::write(&cfg.daemon.pid_file, our_pid.to_string()).unwrap();
        // Don't create a socket file.
        match Daemon::status_check(&cfg) {
            DaemonStatus::Hung { pid } => assert_eq!(pid, our_pid),
            other => panic!("expected Hung, got {other:?}"),
        }
    }

    /// Running state requires BOTH a live PID AND an accepting
    /// socket. Bind a real Unix listener for the duration of the
    /// test so the probe succeeds.
    #[test]
    fn status_check_running_when_pid_alive_and_socket_accepts() {
        use std::os::unix::net::UnixListener;
        let cfg = tmp_config();
        let our_pid = std::process::id();
        std::fs::write(&cfg.daemon.pid_file, our_pid.to_string()).unwrap();
        let _listener = UnixListener::bind(&cfg.daemon.socket_path).unwrap();

        match Daemon::status_check(&cfg) {
            DaemonStatus::Running { pid } => assert_eq!(pid, our_pid),
            other => panic!("expected Running, got {other:?}"),
        }
    }

    /// `stop_running_daemon` on an empty state reports AlreadyStopped
    /// without trying to signal anything — important so launchd
    /// flap-loops don't get an error code on the first stop attempt.
    #[test]
    fn stop_running_daemon_returns_already_stopped_when_clean() {
        let cfg = tmp_config();
        let out = Daemon::stop_running_daemon(&cfg, 1).unwrap();
        assert_eq!(out, StopOutcome::AlreadyStopped);
    }

    /// `stop_running_daemon` on a stale-PID state cleans up files
    /// and reports StaleCleaned — the exact recovery path that
    /// would have unblocked us without the manual `rm pid` dance.
    #[test]
    fn stop_running_daemon_cleans_stale_pid_files() {
        let cfg = tmp_config();
        std::fs::write(&cfg.daemon.pid_file, "999999").unwrap();
        std::fs::write(&cfg.daemon.socket_path, "").unwrap();

        let out = Daemon::stop_running_daemon(&cfg, 1).unwrap();
        match out {
            StopOutcome::StaleCleaned { pid } => assert_eq!(pid, 999999),
            other => panic!("expected StaleCleaned, got {other:?}"),
        }
        assert!(!cfg.daemon.pid_file.exists(), "PID file must be removed");
        assert!(
            !cfg.daemon.socket_path.exists(),
            "socket file must be removed"
        );
    }

    /// `cleanup_stale_state` is a no-op when files don't exist —
    /// callers can invoke it unconditionally without checking.
    #[test]
    fn cleanup_stale_state_noop_when_files_absent() {
        let cfg = tmp_config();
        Daemon::cleanup_stale_state(&cfg); // must not panic
        assert!(!cfg.daemon.pid_file.exists());
        assert!(!cfg.daemon.socket_path.exists());
    }

    /// PID-reuse safety: if the PID file points at a live process
    /// that is NOT mnemonic (post-reboot reuse, etc.), status_check
    /// must classify as StalePid rather than risk signalling an
    /// unrelated process. Codex's P1.
    ///
    /// On macOS the exec-name check uses `proc_pidpath` against the
    /// running process. We spawn a `/bin/sleep 30` and write its PID
    /// into the PID file — its basename is `sleep`, not `mnemonic`,
    /// so the check fails and the status drops to StalePid.
    /// (Linux path returns true / inconclusive — test is mac-only.)
    #[cfg(target_os = "macos")]
    #[test]
    fn status_check_stale_pid_when_pid_belongs_to_other_process() {
        let cfg = tmp_config();
        // Spawn a long-sleeping subprocess so its PID is unambiguously
        // alive but its executable is /bin/sleep, not mnemonic.
        let mut child = std::process::Command::new("/bin/sleep")
            .arg("30")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn /bin/sleep");

        std::fs::write(&cfg.daemon.pid_file, child.id().to_string()).unwrap();
        // No socket file → would normally be Hung; with PID-reuse
        // guard it should be StalePid because /bin/sleep is not us.
        match Daemon::status_check(&cfg) {
            DaemonStatus::StalePid { pid } => {
                assert_eq!(pid, child.id(), "PID matches the impostor process");
            }
            other => {
                panic!("expected StalePid (PID belongs to /bin/sleep, not mnemonic), got {other:?}")
            }
        }
        let _ = child.kill();
        let _ = child.wait();
    }
}
