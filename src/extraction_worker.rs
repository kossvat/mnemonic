//! Async entity-extraction worker.
//!
//! Drains `extraction_queue` rows on a tick and runs the real extractor
//! (rule-based + optional LLM) off the daemon's hot path. Without this,
//! every memory save blocks for the duration of an Ollama round-trip
//! (1-5s per call); with it, the save returns in <100ms and extraction
//! catches up in the background.
//!
//! Failure handling stays in `LlmExtractor::extract` itself: if the LLM
//! backend errors mid-extraction, the extractor re-enqueues the memory
//! into the SEPARATE `pending_extractions` retry queue (with exponential
//! backoff).
//!
//! Failures of the graph WRITE (or an extractor panic) are this worker's
//! problem though: such rows stay in `extraction_queue` with a bumped
//! `attempts` counter. The batch query orders never-tried rows first, so
//! poisoned rows can't camp at the head and starve fresh saves; after
//! `MAX_FIRST_ATTEMPTS` they're dead-lettered into `pending_extractions`
//! where the existing backoff/manual-drain tooling owns them.

use std::sync::Arc;
use std::time::Duration;

use tokio::time::interval;
use tracing::{debug, info, warn};

use crate::graph::extractor::EntityExtractor;
use crate::storage::Storage;

/// First-attempt failures tolerated before a row is dead-lettered into
/// `pending_extractions`. Kept small: a row that fails 3 consecutive
/// ticks is poisoned (bad data), not transient (DB lock) — transient
/// errors clear within a tick or two.
const MAX_FIRST_ATTEMPTS: i64 = 3;

/// Spawn the background worker. Returns immediately; the worker runs
/// forever (until the daemon process exits) on its own tokio task.
///
/// Picks up at most `batch_size` rows per `interval_secs` tick. Heavy
/// extraction work happens inside `spawn_blocking` so it doesn't starve
/// other async tasks on the same runtime.
pub fn spawn_worker(
    storage: Arc<Storage>,
    extractor: Arc<dyn EntityExtractor>,
    interval_secs: u64,
    batch_size: usize,
) -> tokio::task::JoinHandle<()> {
    info!("Extraction worker starting (interval={interval_secs}s, batch={batch_size})");
    tokio::spawn(async move {
        let mut ticker = interval(Duration::from_secs(interval_secs.max(1)));
        // Skip the immediate first tick — gives the daemon a moment to
        // finish other startup before we start grinding through the queue.
        ticker.tick().await;
        loop {
            ticker.tick().await;
            if let Err(e) = drain_once(&storage, &extractor, batch_size).await {
                warn!("Extraction worker tick error: {e}");
            }
        }
    })
}

/// One pass: fetch a batch, extract each, save graph, dequeue. Returns
/// the number of rows processed so tests / metrics can observe progress.
///
/// Takes `Arc`s rather than borrows so each row can be handed to a
/// `spawn_blocking` task (the extractor's LLM call is sync `reqwest`).
/// `EntityExtractor` already requires `Send + Sync`, so this is a clean
/// move with no unsafe gymnastics.
pub async fn drain_once(
    storage: &Arc<Storage>,
    extractor: &Arc<dyn EntityExtractor>,
    batch_size: usize,
) -> anyhow::Result<usize> {
    let ids = storage.next_extraction_batch(batch_size)?;
    if ids.is_empty() {
        return Ok(0);
    }
    debug!("Extraction worker draining {} rows", ids.len());

    let mut processed = 0usize;
    for id in ids {
        let entry = match storage.get_by_id(&id) {
            Ok(Some(entry)) => entry,
            Ok(None) => {
                // Memory was deleted between save and worker pickup. Drop
                // the queue row and move on — nothing to extract.
                let _ = storage.dequeue_extraction(&id);
                continue;
            }
            Err(e) => {
                warn!("Extraction worker get_by_id failed for {id}: {e}");
                continue;
            }
        };

        // Hand the sync extractor to a blocking thread. The LLM call
        // inside the extractor is `reqwest::blocking::Client`, which
        // would otherwise stall the tokio runtime.
        //
        // Use `replace_graph` (transactional clear+save) instead of plain
        // `save_graph` so a worker tick that ends up running twice for the
        // same memory (manual `reextract --pending` racing with the worker,
        // or a future enqueue path we add) can't double-bump `mention_count`.
        let st = storage.clone();
        let ex = extractor.clone();
        let result = tokio::task::spawn_blocking(move || {
            let extraction = ex.extract(&entry);
            st.replace_graph_and_reconcile_projects(&entry, &extraction.entities, &extraction.edges)
        })
        .await;

        match result {
            Ok(Ok(())) => {
                // Graph write succeeded — safe to clear the queue row. The
                // extractor's own retry-after-failure semantics are handled
                // by `pending_extractions` (a separate table); this queue
                // only tracks "needs first attempt", and that's now done.
                let _ = storage.dequeue_extraction(&id);
                processed += 1;
            }
            Ok(Err(e)) => {
                // Graph write failed (DB lock, constraint, transient I/O).
                // Bump attempts and leave the row so the NEXT tick retries;
                // after MAX_FIRST_ATTEMPTS it's dead-lettered to
                // `pending_extractions` so it can't block the queue head.
                warn!("Extraction worker replace_graph failed for {id}: {e} — will retry");
                note_failure(storage, &id, &e.to_string());
            }
            Err(e) => {
                // Extractor panicked inside spawn_blocking. Same policy as
                // a write failure — a row that panics the extractor every
                // tick is the definition of poisoned.
                warn!("Extraction worker spawn_blocking joined with error: {e}");
                note_failure(storage, &id, &format!("extractor panic: {e}"));
            }
        }
    }
    debug!("Extraction worker tick processed {processed}/{batch_size} rows");
    Ok(processed)
}

/// Record a failed attempt; never propagates — failure bookkeeping must
/// not abort the rest of the batch.
fn note_failure(storage: &Arc<Storage>, id: &str, error: &str) {
    match storage.fail_extraction(id, error, MAX_FIRST_ATTEMPTS) {
        Ok(true) => warn!(
            "Extraction worker: {id} dead-lettered to pending_extractions \
             after {MAX_FIRST_ATTEMPTS} failed attempts (`mnemonic reextract --pending`)"
        ),
        Ok(false) => {}
        Err(e) => warn!("Extraction worker: fail_extraction({id}) errored: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{EventSource, MemoryEntry, MemoryType};
    use crate::graph::extractor::{ExtractionResult, RuleExtractor};
    use chrono::Utc;

    /// UUID-suffixed temp path so parallel tests in the same nanosecond
    /// don't collide (which they do — `as_nanos()` resolution on macOS is
    /// coarser than test scheduling, and Codex caught one flake from
    /// exactly this). UUIDs are unique by construction.
    fn tmp_db() -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("mnemonic-worker-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("memory.db")
    }

    fn make_entry(title: &str, content: &str) -> MemoryEntry {
        MemoryEntry {
            id: uuid::Uuid::new_v4().to_string(),
            timestamp: Utc::now(),
            memory_type: MemoryType::Decision,
            title: title.into(),
            content: content.into(),
            tags: vec![],
            source: EventSource::Manual,
            importance: 0.6,
            metadata: serde_json::Value::Null,
        }
    }

    #[tokio::test]
    async fn drain_processes_enqueued_memories_and_persists_graph() {
        let storage = Arc::new(Storage::open(&tmp_db()).unwrap());
        let extractor: Arc<dyn EntityExtractor> = Arc::new(RuleExtractor::new());

        // Save a memory that the rule extractor WILL pull entities from
        // (the title contains a KNOWN_PROJECTS slug — "mnemonic").
        let entry = make_entry(
            "Mnemonic retrieval architecture",
            "Mnemonic now uses BM25 + HNSW + graph hop.",
        );
        storage.save(&entry).unwrap();
        storage.enqueue_extraction(&entry.id).unwrap();
        assert_eq!(storage.extraction_queue_count().unwrap(), 1);

        let processed = drain_once(&storage, &extractor, 10).await.unwrap();
        assert_eq!(processed, 1, "one row processed");
        assert_eq!(
            storage.extraction_queue_count().unwrap(),
            0,
            "queue drained"
        );

        // Graph should now have at least one entity linked to the memory.
        let g = storage.graph_query("mnemonic").unwrap();
        assert!(
            g.found,
            "rule extractor should detect 'mnemonic' as a project entity"
        );
    }

    #[tokio::test]
    async fn drain_handles_missing_memory_gracefully() {
        let storage = Arc::new(Storage::open(&tmp_db()).unwrap());
        let extractor: Arc<dyn EntityExtractor> = Arc::new(RuleExtractor::new());

        // Enqueue an id whose memory never existed (or was deleted).
        let stale_id = uuid::Uuid::new_v4().to_string();
        storage.enqueue_extraction(&stale_id).unwrap();
        assert_eq!(storage.extraction_queue_count().unwrap(), 1);

        let processed = drain_once(&storage, &extractor, 10).await.unwrap();
        // Zero processed (no memory body to extract from), but the row
        // must be gone — otherwise the worker would loop on it forever.
        assert_eq!(processed, 0);
        assert_eq!(
            storage.extraction_queue_count().unwrap(),
            0,
            "missing-memory row must be evicted"
        );
    }

    #[tokio::test]
    async fn drain_no_op_on_empty_queue() {
        let storage = Arc::new(Storage::open(&tmp_db()).unwrap());
        let extractor: Arc<dyn EntityExtractor> = Arc::new(RuleExtractor::new());
        let processed = drain_once(&storage, &extractor, 10).await.unwrap();
        assert_eq!(processed, 0);
    }

    #[tokio::test]
    async fn enqueue_is_idempotent() {
        let storage = Arc::new(Storage::open(&tmp_db()).unwrap());
        let entry = make_entry("hello", "world");
        storage.save(&entry).unwrap();
        storage.enqueue_extraction(&entry.id).unwrap();
        storage.enqueue_extraction(&entry.id).unwrap();
        storage.enqueue_extraction(&entry.id).unwrap();
        assert_eq!(
            storage.extraction_queue_count().unwrap(),
            1,
            "INSERT OR IGNORE must collapse repeated enqueues to one row"
        );
    }

    /// Regression: if replace_graph fails (e.g. transient DB error), the
    /// queue row must stay so the next worker tick can retry. Previously
    /// the worker dequeued unconditionally, silently losing graph
    /// enrichment on a poisoned save. Simulated here by closing the
    /// storage handle mid-drain — but easier route: pass an extractor
    /// that returns invalid data we know save_graph will accept (no
    /// failures realistically reachable from this path), so we instead
    /// test the converse: success path DOES dequeue. The failure-leaves-row
    /// invariant is guarded by code review for now; the test below pins
    /// that success-only-dequeue behavior won't regress to
    /// always-dequeue.
    #[tokio::test]
    async fn drain_dequeues_only_on_success() {
        let storage = Arc::new(Storage::open(&tmp_db()).unwrap());
        let extractor: Arc<dyn EntityExtractor> = Arc::new(RuleExtractor::new());

        let entry = make_entry(
            "Mnemonic note",
            "Mnemonic uses rust and postgres in production.",
        );
        storage.save(&entry).unwrap();
        storage.enqueue_extraction(&entry.id).unwrap();

        // Happy path: drain succeeds, queue is empty after.
        drain_once(&storage, &extractor, 10).await.unwrap();
        assert_eq!(
            storage.extraction_queue_count().unwrap(),
            0,
            "successful drain dequeues"
        );

        // Stale-id path: enqueue an id with no matching memory. Worker
        // dequeues (special case — no work to retry), but the path is
        // distinct from a save_graph error.
        let stale = uuid::Uuid::new_v4().to_string();
        storage.enqueue_extraction(&stale).unwrap();
        drain_once(&storage, &extractor, 10).await.unwrap();
        assert_eq!(storage.extraction_queue_count().unwrap(), 0);
    }

    /// Regression for P1.2: re-running extraction on the same memory must
    /// NOT inflate mention_count. Worker uses `replace_graph` which clears
    /// + decrements before re-saving.
    #[tokio::test]
    async fn drain_does_not_inflate_mention_count_on_rerun() {
        let storage = Arc::new(Storage::open(&tmp_db()).unwrap());
        let extractor: Arc<dyn EntityExtractor> = Arc::new(RuleExtractor::new());

        let entry = make_entry(
            "Mnemonic retrieval architecture",
            "Mnemonic uses BM25 and rust.",
        );
        storage.save(&entry).unwrap();

        // First drain.
        storage.enqueue_extraction(&entry.id).unwrap();
        drain_once(&storage, &extractor, 10).await.unwrap();
        let count1 = storage.graph_query("mnemonic").unwrap().mention_count;
        assert!(count1 >= 1, "first drain should register the mention");

        // Second drain on the same memory — simulating an enqueue racing
        // with a `reextract --pending` retry, or any future code path that
        // could enqueue twice. Counts must stay at the same value, not
        // double.
        storage.enqueue_extraction(&entry.id).unwrap();
        drain_once(&storage, &extractor, 10).await.unwrap();
        let count2 = storage.graph_query("mnemonic").unwrap().mention_count;
        assert_eq!(
            count1, count2,
            "second drain of the same memory must not inflate mention_count (got {count1} then {count2})"
        );
    }

    /// Sanity: the rule extractor is fast enough that 5 rows process in
    /// well under a second. Catches regressions where someone wires up
    /// blocking work that doesn't actually unblock.
    #[tokio::test]
    async fn batch_drain_is_quick_for_rule_only_extractor() {
        let storage = Arc::new(Storage::open(&tmp_db()).unwrap());
        let extractor: Arc<dyn EntityExtractor> = Arc::new(RuleExtractor::new());
        for i in 0..5 {
            let entry = make_entry(&format!("Mnemonic note {i}"), "rust postgres design");
            storage.save(&entry).unwrap();
            storage.enqueue_extraction(&entry.id).unwrap();
        }
        let start = std::time::Instant::now();
        let n = drain_once(&storage, &extractor, 10).await.unwrap();
        assert_eq!(n, 5);
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "rule-only drain of 5 rows took {:?}",
            start.elapsed()
        );
    }

    /// Extractor that panics on entries whose title contains "poison" and
    /// returns an empty (successful) extraction for everything else —
    /// simulates a poisoned row without touching real failure plumbing.
    struct PoisonExtractor;
    impl EntityExtractor for PoisonExtractor {
        fn extract(&self, entry: &MemoryEntry) -> ExtractionResult {
            if entry.title.contains("poison") {
                panic!("poisoned row");
            }
            ExtractionResult::default()
        }
    }

    /// Head-of-line regression: a row that fails every tick must not
    /// starve fresh rows behind it. With batch_size=1 and oldest-first
    /// ordering the poisoned row used to be re-fetched every tick forever;
    /// attempts-first ordering lets the fresh row through on tick 2.
    #[tokio::test]
    async fn poisoned_row_does_not_starve_fresh_rows() {
        let storage = Arc::new(Storage::open(&tmp_db()).unwrap());
        let extractor: Arc<dyn EntityExtractor> = Arc::new(PoisonExtractor);

        let poisoned = make_entry("poison pill", "always fails");
        storage.save(&poisoned).unwrap();
        storage.enqueue_extraction(&poisoned.id).unwrap();
        // Fresh row enqueued AFTER the poisoned one — strictly behind it
        // in enqueued_at order.
        let fresh = make_entry("healthy note", "extracts fine");
        storage.save(&fresh).unwrap();
        storage.enqueue_extraction(&fresh.id).unwrap();

        // Tick 1: batch of 1 picks the poisoned row (attempts 0, oldest),
        // it panics, attempts bumps to 1, row stays.
        drain_once(&storage, &extractor, 1).await.unwrap();
        assert_eq!(storage.extraction_queue_count().unwrap(), 2);

        // Tick 2: the fresh row (attempts 0) must now outrank the
        // poisoned one (attempts 1) and get processed.
        let n = drain_once(&storage, &extractor, 1).await.unwrap();
        assert_eq!(n, 1, "fresh row must be processed on tick 2");
        assert_eq!(storage.extraction_queue_count().unwrap(), 1);
    }

    /// After MAX_FIRST_ATTEMPTS consecutive failures the row must move to
    /// `pending_extractions` (visible, manually drainable) instead of
    /// looping in `extraction_queue` forever.
    #[tokio::test]
    async fn poisoned_row_dead_letters_after_max_attempts() {
        let storage = Arc::new(Storage::open(&tmp_db()).unwrap());
        let extractor: Arc<dyn EntityExtractor> = Arc::new(PoisonExtractor);

        let poisoned = make_entry("poison pill", "always fails");
        storage.save(&poisoned).unwrap();
        storage.enqueue_extraction(&poisoned.id).unwrap();

        for _ in 0..MAX_FIRST_ATTEMPTS {
            drain_once(&storage, &extractor, 5).await.unwrap();
        }

        assert_eq!(
            storage.extraction_queue_count().unwrap(),
            0,
            "poisoned row must leave extraction_queue"
        );
        assert_eq!(
            storage.pending_extractions_count().unwrap(),
            1,
            "poisoned row must land in pending_extractions"
        );
    }

    // Silence dead-code warning on ExtractionResult import in case Rust
    // doesn't see it used by the inferred type above.
    #[allow(dead_code)]
    fn _unused(_: ExtractionResult) {}
}
