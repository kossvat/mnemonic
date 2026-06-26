//! Periodic dream-consolidation worker.
//!
//! The daemon-side counterpart to `mnemonic dream batch`. Wakes up
//! every `interval_secs`, picks up closed sessions ended in the
//! last `since_hours`, and summarizes the ones without a canonical
//! summary yet. Mirrors `extraction_worker.rs` — same tokio
//! interval loop, same `Arc<Storage>` sharing, same
//! tick-error-doesn't-kill-the-task contract.
//!
//! ## Why a separate worker (not folded into extraction_worker)
//!
//! Different cadence and scope: extraction runs every 2s on
//! recently-saved memory rows; dream runs hourly on closed
//! sessions. Wedging both into one loop would force one to
//! compromise (either dream becomes too frequent or extraction
//! becomes too slow). Cleaner to have two independent loops with
//! their own config blocks.
//!
//! ## Failure handling
//!
//! Per-session errors are logged at warn and skipped — one bad
//! session (LLM flake, malformed memory) doesn't kill the
//! batch. The next tick re-discovers any sessions that weren't
//! summarized this round. `session_summary_lookup` is the
//! idempotency gate; sessions that DID get a summary stay
//! skipped forever.

use std::sync::Arc;
use std::time::Duration;

use tokio::time::interval;
use tracing::{debug, info, warn};

use crate::config::DreamConfig;
use crate::dream;
use crate::graph::extractor_llm::OllamaBackend;
use crate::storage::Storage;

/// Spawn the dream worker. Returns immediately; the worker runs
/// forever on its own tokio task until the daemon exits.
///
/// `llm_cfg` is needed because the worker may build an Ollama
/// backend per tick when `use_llm = true`. We don't share a
/// long-lived backend across ticks because the underlying
/// `reqwest::blocking::Client` is sync and we want each tick to
/// be self-contained (tick fails → next tick tries fresh
/// connection). Embedder is also rebuilt per tick for the same
/// reason — these are cheap relative to the LLM calls.
pub fn spawn_worker(
    storage: Arc<Storage>,
    dream_cfg: DreamConfig,
    llm_cfg: crate::config::LlmConfig,
) -> tokio::task::JoinHandle<()> {
    info!(
        "Dream worker starting (interval={}s, since={}h, limit={}, llm={})",
        dream_cfg.interval_secs, dream_cfg.since_hours, dream_cfg.batch_limit, dream_cfg.use_llm
    );
    tokio::spawn(async move {
        // Effective interval — clamp to at least 60s. Anything below
        // is almost certainly a config bug; running every few
        // seconds would hammer the DB needlessly.
        let secs = dream_cfg.interval_secs.max(60);
        let mut ticker = interval(Duration::from_secs(secs));
        // Skip the immediate first tick — gives the daemon a
        // moment to finish other startup before grinding through
        // historical sessions.
        ticker.tick().await;
        loop {
            ticker.tick().await;
            // Run the sync drain in spawn_blocking so the heavy
            // Ollama calls + SQLite writes don't starve other
            // async tasks (api server, watchers).
            let s = storage.clone();
            let d = dream_cfg.clone();
            let l = llm_cfg.clone();
            let result = tokio::task::spawn_blocking(move || drain_once(&s, &d, &l)).await;
            match result {
                Ok(Ok(n)) if n > 0 => info!("Dream worker: summarized {n} session(s)"),
                Ok(Ok(_)) => debug!("Dream worker: no sessions to summarize this tick"),
                Ok(Err(e)) => warn!("Dream worker tick error: {e}"),
                Err(e) => warn!("Dream worker join error: {e}"),
            }
        }
    })
}

/// One pass: find closed sessions in the look-back window without a
/// canonical summary, summarize each (heuristic or LLM), save with
/// an embedding so they participate in retrieval. Returns the count
/// of summaries actually saved.
///
/// Mirrors the CLI `dream batch --apply` path but without the
/// dry-run / preview branches — this is daemon-managed automation.
pub fn drain_once(
    storage: &Arc<Storage>,
    dream_cfg: &DreamConfig,
    llm_cfg: &crate::config::LlmConfig,
) -> anyhow::Result<usize> {
    let sessions = storage.closed_sessions_since(dream_cfg.since_hours, dream_cfg.batch_limit)?;
    if sessions.is_empty() {
        return Ok(0);
    }

    // Filter to sessions without a canonical summary. Snapshots
    // (open_at_summary_time = true) don't satisfy the canonical
    // lookup, so this loop will produce a fresh canonical summary
    // for any session that only has a snapshot — exactly what
    // Codex's P2 fix on the lookup was supposed to enable.
    let mut pending = Vec::new();
    for s in &sessions {
        if dream::summary_for_session(storage, &s.id)?.is_none() {
            pending.push(s.clone());
        }
    }
    if pending.is_empty() {
        return Ok(0);
    }

    // Backend: only build if we'll actually use it. None for
    // heuristic mode keeps the path free of unused dependencies.
    let backend: Option<OllamaBackend> = if dream_cfg.use_llm {
        if !llm_cfg.enabled {
            warn!(
                "Dream worker: use_llm=true but llm.enabled=false in config. \
                 Falling back to heuristic summarizer for this tick."
            );
            None
        } else {
            match OllamaBackend::new(llm_cfg) {
                Ok(b) => Some(b),
                Err(e) => {
                    warn!(
                        "Dream worker: failed to init Ollama backend, falling back \
                         to heuristic for this tick: {e}"
                    );
                    None
                }
            }
        }
    } else {
        None
    };

    let embedder = crate::embedding::create_embedder()?;
    let mut saved = 0usize;
    for s in &pending {
        let result = match &backend {
            Some(b) => dream::summarize_session_llm(storage, &s.id, b),
            None => dream::summarize_session_heuristic(storage, &s.id),
        };
        match result {
            Ok(summary) => {
                let text = format!("{} {}", summary.title, summary.content);
                let emb = embedder.embed(&text).ok();
                if let Err(e) = storage.save_with_embedding(&summary, emb.as_ref()) {
                    warn!(
                        "Dream worker: save failed for session {}: {e}",
                        short_id(&s.id)
                    );
                    continue;
                }
                debug!(
                    "Dream worker: session {} → summary {}",
                    short_id(&s.id),
                    short_id(&summary.id)
                );
                saved += 1;
            }
            Err(e) => {
                // Empty sessions are the boring common case — the
                // session existed but had no linked memories. Drop
                // at debug; everything else is interesting.
                let msg = e.to_string();
                if msg.contains("no memories") {
                    debug!("Dream worker: skipping empty session {}", short_id(&s.id));
                } else {
                    warn!("Dream worker: session {} failed: {e}", short_id(&s.id));
                }
            }
        }
    }
    Ok(saved)
}

fn short_id(id: &str) -> &str {
    &id[..8.min(id.len())]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::LlmConfig;
    use crate::event::{EventSource, MemoryEntry, MemoryType};
    use std::sync::Arc;

    fn tmp_storage() -> Arc<Storage> {
        let dir = std::env::temp_dir().join(format!(
            "mnemonic-dream-worker-test-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Arc::new(Storage::open(&dir.join("memory.db")).unwrap())
    }

    fn make_entry(title: &str) -> MemoryEntry {
        MemoryEntry {
            id: uuid::Uuid::new_v4().to_string(),
            timestamp: chrono::Utc::now(),
            title: title.into(),
            content: "content body".into(),
            memory_type: MemoryType::Note,
            tags: vec![],
            source: EventSource::Manual,
            importance: 0.5,
            metadata: serde_json::Value::Null,
        }
    }

    fn dream_cfg(use_llm: bool) -> DreamConfig {
        DreamConfig {
            enabled: true,
            interval_secs: 3600,
            since_hours: 24,
            batch_limit: 50,
            use_llm,
        }
    }

    fn llm_cfg_disabled() -> LlmConfig {
        LlmConfig {
            enabled: false,
            ..Default::default()
        }
    }

    /// Empty DB: drain returns 0, no errors. Worker can run
    /// safely on a fresh install before any sessions exist.
    #[test]
    fn drain_once_returns_zero_on_empty_db() {
        let storage = tmp_storage();
        let n = drain_once(&storage, &dream_cfg(false), &llm_cfg_disabled()).unwrap();
        assert_eq!(n, 0);
    }

    /// Sessions that are open OR already-summarized must NOT be
    /// summarized again. Open is filtered by `closed_sessions_since`
    /// SQL; summarized is filtered by `summary_for_session`. Both
    /// gates apply here.
    #[test]
    fn drain_once_skips_open_and_already_summarized_sessions() {
        let storage = tmp_storage();
        let peer_id = storage.upsert_peer("claude", None, "agent").unwrap();

        // Open session → must be skipped (closed_sessions_since
        // filters it out).
        let _open_id = storage
            .open_session(&peer_id, Some("open"), "jsonl")
            .unwrap();
        let m1 = make_entry("o1");
        storage.save(&m1).unwrap();
        storage.set_memory_session(&m1.id, Some(&_open_id)).unwrap();

        // Closed + already-summarized session → skipped.
        let closed_summarized = storage
            .open_session(&peer_id, Some("closed1"), "jsonl")
            .unwrap();
        let m2 = make_entry("c1m");
        storage.save(&m2).unwrap();
        storage
            .set_memory_session(&m2.id, Some(&closed_summarized))
            .unwrap();
        storage.end_session(&closed_summarized).unwrap();
        let summary = dream::summarize_session_heuristic(&storage, &closed_summarized).unwrap();
        storage.save(&summary).unwrap();

        let n = drain_once(&storage, &dream_cfg(false), &llm_cfg_disabled()).unwrap();
        assert_eq!(
            n, 0,
            "open + already-summarized sessions must both be skipped"
        );
    }

    /// Happy path: a closed unsummarized session with memories gets
    /// summarized by the heuristic and saved. Subsequent drain is
    /// a no-op because the metadata link now matches.
    #[test]
    fn drain_once_summarizes_one_pending_then_becomes_idempotent() {
        let storage = tmp_storage();
        let peer_id = storage.upsert_peer("claude", None, "agent").unwrap();
        let session_id = storage.open_session(&peer_id, Some("c"), "jsonl").unwrap();
        let m = make_entry("note one");
        storage.save(&m).unwrap();
        storage
            .set_memory_session(&m.id, Some(&session_id))
            .unwrap();
        storage.end_session(&session_id).unwrap();

        let n = drain_once(&storage, &dream_cfg(false), &llm_cfg_disabled()).unwrap();
        assert_eq!(n, 1, "exactly one summary should land");

        // Second tick — link is now there, lookup returns it,
        // session is skipped.
        let n2 = drain_once(&storage, &dream_cfg(false), &llm_cfg_disabled()).unwrap();
        assert_eq!(n2, 0, "second tick must be idempotent");

        // Verify the summary exists in the lookup path.
        let found = dream::summary_for_session(&storage, &session_id).unwrap();
        assert!(
            found.is_some(),
            "summary must be persisted and discoverable"
        );
        assert_eq!(found.unwrap().memory_type, MemoryType::SessionSummary);
    }

    /// Empty closed sessions (closed but no memories linked) are
    /// counted as "skipped" silently — they generate no summary
    /// but also no error. Common case post-watcher startup when
    /// session_id is set but content didn't land before close.
    #[test]
    fn drain_once_skips_empty_closed_sessions_without_error() {
        let storage = tmp_storage();
        let peer_id = storage.upsert_peer("claude", None, "agent").unwrap();
        let empty = storage
            .open_session(&peer_id, Some("empty"), "jsonl")
            .unwrap();
        storage.end_session(&empty).unwrap();

        let n = drain_once(&storage, &dream_cfg(false), &llm_cfg_disabled()).unwrap();
        assert_eq!(n, 0, "empty session generates no summary");
        // Worker didn't error — that's the assertion: no panic, no
        // anyhow propagation, just a clean 0.
    }
}
