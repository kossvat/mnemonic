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
use crate::redaction::failure::Failure;
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
                warn!(
                    "Extraction worker tick error ({}): {}",
                    Failure::Storage,
                    store_words(&e)
                );
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
                warn!("{}", failure_notice("get_by_id", &id, &e));
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
            Ok(Err(e)) if crate::redaction::state::is_refused(&e) => {
                // The guard refused a name in this graph. Trying again
                // gives the same graph: the row leaves the queue, and the
                // memory keeps whatever graph it had.
                warn!("{}", refused_notice(&id, &e));
                let _ = storage.dequeue_extraction(&id);
            }
            Ok(Err(e)) => {
                // Graph write failed (DB lock, constraint, transient I/O).
                // Bump attempts and leave the row so the NEXT tick retries;
                // after MAX_FIRST_ATTEMPTS it's dead-lettered to
                // `pending_extractions` so it can't block the queue head.
                warn!("{} — will retry", failure_notice("replace_graph", &id, &e));
                note_failure(storage, &id, Failure::Storage);
            }
            Err(_) => {
                // Extractor panicked inside spawn_blocking. Same policy as
                // a write failure — a row that panics the extractor every
                // tick is the definition of poisoned. What a panic says is
                // the extractor's, and with it the model's: the log says
                // that it happened.
                warn!(
                    "Extraction worker: extractor stopped for {} ({})",
                    crate::redaction::state::shown(&id),
                    Failure::Worker
                );
                note_failure(storage, &id, Failure::Worker);
            }
        }
    }
    debug!("Extraction worker tick processed {processed}/{batch_size} rows");
    Ok(processed)
}

/// What the worker logs for a graph the guard refused. The memory's id is
/// shown unless it is what the guard refused: a memory stored before the
/// redaction policy can have an id that is the credential.
fn refused_notice(id: &str, error: &anyhow::Error) -> String {
    format!(
        "Extraction worker: graph of {} left as is ({error})",
        crate::redaction::state::shown(id)
    )
}

/// What the worker logs when the store failed it, reading a memory or
/// taking its graph. The memory's id is shown unless the policy refuses
/// it. The store's own words help to tell a locked database from a broken
/// one; they are prepared like any text, and cut, before they are logged.
fn failure_notice(step: &'static str, id: &str, error: &anyhow::Error) -> String {
    format!(
        "Extraction worker {step} failed for {} ({}): {}",
        crate::redaction::state::shown(id),
        Failure::Storage,
        store_words(error)
    )
}

/// The store's own words of a failure, as they are logged: prepared, and
/// cut.
fn store_words(error: &anyhow::Error) -> String {
    const SHOWN: usize = 200;
    crate::redaction::state::prepare_capped(&error.to_string(), SHOWN).value
}

/// Record a failed attempt; never propagates — failure bookkeeping must
/// not abort the rest of the batch. The record holds the code.
fn note_failure(storage: &Arc<Storage>, id: &str, failure: Failure) {
    let id_shown = crate::redaction::state::shown(id);
    match storage.fail_extraction(id, failure.code(), MAX_FIRST_ATTEMPTS) {
        Ok(true) => warn!(
            "Extraction worker: {id_shown} dead-lettered to pending_extractions \
             after {MAX_FIRST_ATTEMPTS} failed attempts (`mnemonic reextract --pending`)"
        ),
        Ok(false) => {}
        Err(_) => warn!(
            "Extraction worker: fail_extraction({id_shown}) errored ({})",
            Failure::Storage
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{EventSource, MemoryEntry, MemoryType};
    use crate::graph::extractor::{ExtractionResult, RuleExtractor};
    use chrono::Utc;

    /// Database path in a fresh temp dir per call, so parallel tests never
    /// share a file (a timestamp suffix once collided on macOS; tempfile
    /// names are random and created atomically). The directory is removed
    /// when the returned guard drops, so bind it for the whole test.
    fn tmp_db() -> crate::test_support::InTempDir<std::path::PathBuf> {
        crate::test_support::temp_path("mnemonic-worker-test-", "memory.db")
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
        let db = tmp_db();
        let storage = Arc::new(Storage::open(&db).unwrap());
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
        let db = tmp_db();
        let storage = Arc::new(Storage::open(&db).unwrap());
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
        let db = tmp_db();
        let storage = Arc::new(Storage::open(&db).unwrap());
        let extractor: Arc<dyn EntityExtractor> = Arc::new(RuleExtractor::new());
        let processed = drain_once(&storage, &extractor, 10).await.unwrap();
        assert_eq!(processed, 0);
    }

    #[tokio::test]
    async fn enqueue_is_idempotent() {
        let db = tmp_db();
        let storage = Arc::new(Storage::open(&db).unwrap());
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
        let db = tmp_db();
        let storage = Arc::new(Storage::open(&db).unwrap());
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
        let db = tmp_db();
        let storage = Arc::new(Storage::open(&db).unwrap());
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
        let db = tmp_db();
        let storage = Arc::new(Storage::open(&db).unwrap());
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

    /// An extractor whose graph names something the redaction policy
    /// refuses. The value is assembled at run time.
    struct DirtyExtractor(String);
    impl EntityExtractor for DirtyExtractor {
        fn extract(&self, _entry: &MemoryEntry) -> ExtractionResult {
            ExtractionResult {
                entities: vec![
                    crate::graph::Entity {
                        name: "demoapp".into(),
                        entity_type: crate::graph::EntityType::Project,
                    },
                    crate::graph::Entity {
                        name: self.0.clone(),
                        entity_type: crate::graph::EntityType::Concept,
                    },
                ],
                edges: vec![],
            }
        }
    }

    /// A refused graph gives the same refusal on every try: the row leaves
    /// the queue at once, is not dead-lettered, and nothing of the graph
    /// is written.
    #[tokio::test]
    async fn redaction_state_worker_drops_a_refused_graph_without_retrying() {
        let db = tmp_db();
        let storage = Arc::new(Storage::open(&db).unwrap());
        let name = ["sk-", "proj-", &"a1B2c3D4e5F6".repeat(4)].concat();
        let extractor: Arc<dyn EntityExtractor> = Arc::new(DirtyExtractor(name));
        let entry = make_entry("a plain note", "plain body");
        storage.save(&entry).unwrap();
        storage.enqueue_extraction(&entry.id).unwrap();

        assert_eq!(drain_once(&storage, &extractor, 10).await.unwrap(), 0);
        assert_eq!(storage.extraction_queue_count().unwrap(), 0);
        assert!(storage.pending_row(&entry.id).unwrap().is_none());
        assert_eq!(storage.graph_stats().unwrap(), (0, 0));
    }

    /// A memory stored before the policy under an id it refuses: its graph
    /// is refused for the id alone, the row leaves the queue, and what the
    /// worker logs does not name it.
    #[tokio::test]
    async fn redaction_state_worker_does_not_name_a_refused_memory_id() {
        let db = tmp_db();
        let storage = Arc::new(Storage::open(&db).unwrap());
        let id = ["sk-", "proj-", &"a1B2c3D4e5F6".repeat(4)].concat();
        {
            let conn = storage.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO memories (id, timestamp, title, content, memory_type, tags, source,
                     importance, metadata)
                 VALUES (?1, ?2, 'a plain note', 'body', 'note', '[]', '\"Manual\"', 0.5,
                         'null')",
                rusqlite::params![id, Utc::now().to_rfc3339()],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO extraction_queue (memory_id) VALUES (?1)",
                [&id],
            )
            .unwrap();
        }
        let extractor: Arc<dyn EntityExtractor> = Arc::new(RuleExtractor::new());
        assert!(drain_once(&storage, &extractor, 10).await.unwrap() == 0);
        assert!(storage.extraction_queue_count().unwrap() == 0);
        assert!(storage.pending_row(&id).unwrap().is_none());

        let error = storage.replace_graph(&id, &[], &[]).unwrap_err();
        let notice = refused_notice(&id, &error);
        assert!(notice.contains("left as is") && notice.contains("SENSITIVE_CONTENT"));
        assert!(!notice.contains(&id), "the notice names the refused id");
        assert!(refused_notice("m-1", &error).contains("graph of m-1 left"));
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
        let db = tmp_db();
        let storage = Arc::new(Storage::open(&db).unwrap());
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
        let db = tmp_db();
        let storage = Arc::new(Storage::open(&db).unwrap());
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

    /// An extractor that stops with what a model could have said.
    struct LoudExtractor(String);
    impl EntityExtractor for LoudExtractor {
        fn extract(&self, _entry: &MemoryEntry) -> ExtractionResult {
            panic!("extractor stopped at {}", self.0);
        }
    }

    /// What a panic says is not kept: the dead letter holds the code.
    #[tokio::test]
    async fn redaction_generated_worker_panic_leaves_a_code_and_no_text() {
        let token: String = ["sk-", "proj-", &"a1B2c3D4e5F6".repeat(4)].concat();
        let db = tmp_db();
        let storage = Arc::new(Storage::open(&db).unwrap());
        let extractor: Arc<dyn EntityExtractor> = Arc::new(LoudExtractor(token.clone()));
        let entry = make_entry("rollout", "the sample service goes out on friday");
        storage.save(&entry).unwrap();
        storage.enqueue_extraction(&entry.id).unwrap();
        for _ in 0..MAX_FIRST_ATTEMPTS {
            drain_once(&storage, &extractor, 5).await.unwrap();
        }
        let row = storage.pending_row(&entry.id).unwrap().unwrap();
        assert!(row.1.as_deref() == Some("WORKER_FAILED"));
    }

    /// The store's own words are logged prepared, and the memory's id is
    /// shown unless the policy refuses it.
    #[test]
    fn redaction_generated_worker_notice_of_a_store_failure_is_prepared() {
        let token: String = ["sk-", "proj-", &"a1B2c3D4e5F6".repeat(4)].concat();
        let error = anyhow::anyhow!("constraint failed near {token} and more");
        let notice = failure_notice("replace_graph", &token, &error);
        assert!(notice.contains("STORAGE_FAILED") && notice.contains("constraint failed"));
        assert!(!notice.contains(&token), "the notice holds the token");
        let notice = failure_notice("get_by_id", "m-1", &anyhow::anyhow!("database is locked"));
        assert!(notice.contains("get_by_id failed for m-1"));
        assert!(notice.contains("database is locked"));
        // What is shown of the store's words is cut.
        let long = anyhow::anyhow!("{}", "lock ".repeat(200));
        assert!(failure_notice("get_by_id", "m-1", &long).chars().count() < 400);
    }

    /// Every line the worker logs of a failure is made by one of the
    /// notices above: none puts an id or an error into a line as it is.
    /// (A lookup that fails cannot be made to happen in a test, so the
    /// line it logs is held to this by its source.)
    #[test]
    fn redaction_generated_worker_logs_no_id_and_no_error_as_it_is() {
        let source = include_str!("extraction_worker.rs");
        let code = source.split("#[cfg(test)]").next().unwrap();
        for raw in ["{id}", "{e}", "{e:", "{error}", "{error:"] {
            let lines: Vec<&str> = code
                .lines()
                .filter(|line| line.contains(raw) && !line.contains("({error})"))
                .collect();
            assert!(lines.is_empty(), "a line holds {raw}");
        }
    }
}
