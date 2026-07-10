//! Contradiction lint — flags decisions that semantically REVERSE an older
//! one ("switch to Redis" vs "use Postgres for everything").
//!
//! Dedup only catches near-identical memories; a reversal is *similar
//! enough to be about the same topic* yet says the opposite, so both
//! versions stay alive and both can surface as "standing decisions".
//!
//! Design (reviewed with Codex 2026-07-10) — FLAG-ONLY:
//! - candidates come from pairwise embedding similarity over a project's
//!   active decisions (>= threshold; no upper cap — a hard negation can be
//!   MORE similar than the dedup band);
//! - a local LLM (when enabled) judges each new pair once: confirmed /
//!   dismissed; without an LLM the pair stays a 'candidate' and nothing is
//!   hidden;
//! - verdicts live in the `decision_conflicts` AUDIT table. We never touch
//!   `memories.superseded_by` — that column means dedup consolidation, not
//!   "historically reversed". Confirmed-old decisions are merely excluded
//!   from standing-decision surfaces (digest, Key Decisions).

use anyhow::Result;
use std::sync::Arc;
use tracing::{debug, info, warn};

use crate::embedding::cosine_similarity;
use crate::graph::extractor_llm::LlmBackend;
use crate::storage::{DecisionRow, Storage};

/// At most this many decisions per project are cross-compared per pass.
/// (The similarity threshold itself lives in `[lint] similarity`; the
/// schema's `checker_version` column defaults to 1 until a future prompt
/// change needs re-judging.)
const MAX_DECISIONS_PER_PROJECT: usize = 40;

/// Outcome counts of one lint pass, for logging/tests.
#[derive(Debug, Default, PartialEq)]
pub struct LintStats {
    pub candidates_new: usize,
    pub confirmed: usize,
    pub dismissed: usize,
}

/// Run the lint over one project's active decisions.
pub fn lint_project(
    storage: &Storage,
    project: &str,
    llm: Option<&dyn LlmBackend>,
    similarity_threshold: f32,
) -> Result<LintStats> {
    let mut stats = LintStats::default();
    let decisions =
        storage.project_decisions_with_embeddings(project, MAX_DECISIONS_PER_PROJECT)?;
    if decisions.len() < 2 {
        return Ok(stats);
    }

    for i in 0..decisions.len() {
        for j in (i + 1)..decisions.len() {
            let (a, b) = (&decisions[i], &decisions[j]);
            if a.embedding.is_empty()
                || b.embedding.is_empty()
                || a.embedding.len() != b.embedding.len()
            {
                continue;
            }
            let sim = cosine_similarity(&a.embedding, &b.embedding);
            if sim < similarity_threshold {
                continue;
            }
            // Chronological order decides old vs new. PARSE the timestamps:
            // lexicographic RFC3339 comparison breaks across mixed offsets
            // ("...T05:00:00Z" sorts before "...T12:00:00+09:00" though the
            // latter is EARLIER in UTC), and a swapped pair would make the
            // digest hide the current decision instead of the obsolete one
            // (Codex review). Unparseable timestamps fall back to string
            // order rather than dropping the pair.
            let a_older = match (parse_ts(&a.timestamp), parse_ts(&b.timestamp)) {
                (Some(ta), Some(tb)) => ta <= tb,
                _ => a.timestamp <= b.timestamp,
            };
            let (old, new) = if a_older { (a, b) } else { (b, a) };
            // Only terminal verdicts are final. A 'candidate' (recorded while
            // the LLM was off or answered garbage) must graduate to
            // confirmed/dismissed once a judge is available — otherwise pairs
            // first seen without an LLM would be stranded forever (Codex
            // review).
            match storage.conflict_status(&old.id, &new.id)?.as_deref() {
                Some("confirmed") | Some("dismissed") => continue,
                Some(_) if llm.is_none() => continue, // still a candidate, no judge yet
                _ => {}
            }

            match llm {
                Some(backend) => {
                    match judge_pair(backend, &decision_text(old), &decision_text(new)) {
                        Ok(Some((contradicts, confidence, reason))) => {
                            let status = if contradicts {
                                "confirmed"
                            } else {
                                "dismissed"
                            };
                            storage.upsert_conflict(
                                &old.id,
                                &new.id,
                                project,
                                status,
                                Some(confidence),
                                Some(&reason),
                            )?;
                            if contradicts {
                                stats.confirmed += 1;
                                info!(
                                    "lint: confirmed contradiction in {project}: \"{}\" reversed by \"{}\"",
                                    old.title, new.title
                                );
                            } else {
                                stats.dismissed += 1;
                            }
                        }
                        Ok(None) | Err(_) => {
                            // Unusable LLM answer — keep the pair visible as a
                            // candidate; a later pass may re-judge it.
                            storage.upsert_conflict(
                                &old.id,
                                &new.id,
                                project,
                                "candidate",
                                Some(sim),
                                None,
                            )?;
                            stats.candidates_new += 1;
                        }
                    }
                }
                None => {
                    storage.upsert_conflict(
                        &old.id,
                        &new.id,
                        project,
                        "candidate",
                        Some(sim),
                        None,
                    )?;
                    stats.candidates_new += 1;
                }
            }
        }
    }
    Ok(stats)
}

/// One pass over the currently active projects.
pub fn run_lint_pass(
    storage: &Arc<Storage>,
    llm: Option<&dyn LlmBackend>,
    similarity_threshold: f32,
) -> Result<LintStats> {
    let mut total = LintStats::default();
    // Every project that actually has >= 2 active embedded decisions.
    // Deliberately NO small per-pass cap: already-judged pairs skip in one
    // cheap status lookup each, so a full sweep stays light — while a
    // fixed top-N by freshness would let fully-judged fresh projects hold
    // the slots forever and starve older ones (Codex review). The bound
    // here is a runaway backstop, not a work planner.
    for project in storage.projects_with_decision_pairs(1000)? {
        match lint_project(storage, &project, llm, similarity_threshold) {
            Ok(s) => {
                total.candidates_new += s.candidates_new;
                total.confirmed += s.confirmed;
                total.dismissed += s.dismissed;
            }
            Err(e) => warn!("lint: project {project} failed: {e}"),
        }
    }
    debug!(
        "lint pass: {} new candidates, {} confirmed, {} dismissed",
        total.candidates_new, total.confirmed, total.dismissed
    );
    Ok(total)
}

/// RFC3339 → UTC instant, tolerant of any legal offset spelling.
fn parse_ts(s: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.with_timezone(&chrono::Utc))
}

/// What the judge reads for one decision: the title plus a bounded slice of
/// the body (review point: titles can be generic — "Dependency change:
/// package.json" — while the actual reversal lives in the content). The cut
/// backs up to a char boundary so multi-byte text never panics the slice.
fn decision_text(d: &DecisionRow) -> String {
    const CONTENT_CAP: usize = 400;
    let body = d.content.trim();
    if body.is_empty() || body == d.title {
        return d.title.clone();
    }
    let mut end = CONTENT_CAP.min(body.len());
    while end > 0 && !body.is_char_boundary(end) {
        end -= 1;
    }
    format!("{} — {}", d.title, &body[..end])
}

/// Ask the local LLM whether `newer` reverses `older`. Returns
/// Some((contradicts, confidence, reason)) or None on an unparseable
/// answer. Strict JSON contract; anything else is treated as unusable.
fn judge_pair(
    backend: &dyn LlmBackend,
    older: &str,
    newer: &str,
) -> Result<Option<(bool, f32, String)>> {
    let prompt = format!(
        "Two project decisions, in chronological order.\n\
         OLDER: {older}\n\
         NEWER: {newer}\n\
         Does the NEWER decision reverse or replace the OLDER one (same topic, \
         opposite or superseding choice)? Decisions about different aspects do \
         NOT contradict. Answer ONLY with JSON: \
         {{\"contradicts\": true|false, \"confidence\": 0.0-1.0, \"reason\": \"short\"}}"
    );
    let raw = backend.generate(&prompt)?;
    let parsed: serde_json::Value = match serde_json::from_str(raw.trim()) {
        Ok(v) => v,
        Err(_) => return Ok(None),
    };
    let Some(contradicts) = parsed.get("contradicts").and_then(|v| v.as_bool()) else {
        return Ok(None);
    };
    let confidence = parsed
        .get("confidence")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.5) as f32;
    let reason: String = parsed
        .get("reason")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .chars()
        .take(200)
        .collect();
    Ok(Some((contradicts, confidence, reason)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{EventSource, MemoryEntry, MemoryType};
    use crate::graph::{Entity, EntityType};

    struct CannedLlm(String);
    impl LlmBackend for CannedLlm {
        fn generate(&self, _prompt: &str) -> Result<String> {
            Ok(self.0.clone())
        }
    }

    /// Canned answer + records every prompt it was asked.
    struct RecordingLlm {
        answer: String,
        prompts: std::sync::Mutex<Vec<String>>,
    }
    impl LlmBackend for RecordingLlm {
        fn generate(&self, prompt: &str) -> Result<String> {
            self.prompts.lock().unwrap().push(prompt.to_string());
            Ok(self.answer.clone())
        }
    }

    fn tmp_storage() -> Arc<Storage> {
        let dir = std::env::temp_dir().join(format!("mn-lint-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        Arc::new(Storage::open(&dir.join("memory.db")).unwrap())
    }

    /// Save a decision with a chosen synthetic embedding, linked to project.
    fn save_decision(storage: &Storage, eid: &str, title: &str, emb: Vec<f32>) -> String {
        let e = MemoryEntry::new(title, "body", MemoryType::Decision, EventSource::Socket);
        storage.save_with_embedding(&e, Some(&emb)).unwrap();
        storage.link_memory_entity(&e.id, eid).unwrap();
        e.id.clone()
    }

    fn project(storage: &Storage, name: &str) -> String {
        storage
            .upsert_entity(&Entity {
                name: name.into(),
                entity_type: EntityType::Project,
            })
            .unwrap()
    }

    #[test]
    fn no_llm_records_candidates_and_never_confirms() {
        let storage = tmp_storage();
        let eid = project(&storage, "p");
        let a = save_decision(&storage, &eid, "Use Postgres", vec![1.0, 0.0, 0.1]);
        let b = save_decision(&storage, &eid, "Switch to Redis", vec![0.98, 0.05, 0.1]);
        save_decision(
            &storage,
            &eid,
            "Unrelated: pick blue logo",
            vec![0.0, 1.0, 0.0],
        );

        let stats = lint_project(&storage, "p", None, 0.65).unwrap();
        assert_eq!(stats.candidates_new, 1, "one similar pair");
        assert_eq!(stats.confirmed, 0);
        let (old, new) = order(&storage, &a, &b);
        assert_eq!(
            storage.conflict_status(&old, &new).unwrap().as_deref(),
            Some("candidate")
        );
        assert!(storage.confirmed_conflicts().unwrap().is_empty());

        // A second no-LLM pass leaves the candidate alone (no re-count).
        let again = lint_project(&storage, "p", None, 0.65).unwrap();
        assert_eq!(again, LintStats::default());

        // Once a judge shows up, the stranded candidate GRADUATES instead
        // of being skipped forever (Codex review point).
        let llm =
            CannedLlm(r#"{"contradicts": true, "confidence": 0.9, "reason": "reversed"}"#.into());
        let graduated = lint_project(&storage, "p", Some(&llm), 0.65).unwrap();
        assert_eq!(graduated.confirmed, 1, "candidate graduated to confirmed");
        assert_eq!(
            storage.conflict_status(&old, &new).unwrap().as_deref(),
            Some("confirmed")
        );
    }

    #[test]
    fn llm_confirms_and_pair_is_not_rejudged() {
        let storage = tmp_storage();
        let eid = project(&storage, "p");
        let a = save_decision(&storage, &eid, "Use Postgres", vec![1.0, 0.0]);
        let b = save_decision(&storage, &eid, "Switch to Redis", vec![0.99, 0.01]);

        let llm = CannedLlm(
            r#"{"contradicts": true, "confidence": 0.9, "reason": "storage choice reversed"}"#
                .into(),
        );
        let stats = lint_project(&storage, "p", Some(&llm), 0.65).unwrap();
        assert_eq!(stats.confirmed, 1);
        let confirmed = storage.confirmed_conflicts().unwrap();
        assert_eq!(confirmed.len(), 1);
        let (old, new) = order(&storage, &a, &b);
        assert_eq!(confirmed[0], (old, new));

        // Second pass: already judged, nothing new (a flipping LLM answer
        // must not double-count or overwrite).
        let flip = CannedLlm(r#"{"contradicts": false, "confidence": 0.9, "reason": "x"}"#.into());
        let stats2 = lint_project(&storage, "p", Some(&flip), 0.65).unwrap();
        assert_eq!(stats2, LintStats::default(), "no re-judging");
        assert_eq!(storage.confirmed_conflicts().unwrap().len(), 1);
    }

    /// Mixed timestamp offsets must not swap old/new: "...05:00:00Z" sorts
    /// lexicographically BEFORE "...12:00:00+09:00" although the latter is
    /// two hours earlier in UTC. A swapped pair would hide the CURRENT
    /// decision from digests instead of the obsolete one.
    #[test]
    fn mixed_offset_timestamps_order_chronologically() {
        let storage = tmp_storage();
        let eid = project(&storage, "p");
        // truly older: 03:00 UTC, spelled with a +09:00 offset
        let older = save_decision(&storage, &eid, "Use Postgres", vec![1.0, 0.0]);
        // truly newer: 05:00 UTC, spelled with Z
        let newer = save_decision(&storage, &eid, "Switch to Redis", vec![0.99, 0.01]);
        {
            let conn = storage.conn.lock().unwrap();
            conn.execute(
                "UPDATE memories SET timestamp = '2026-01-01T12:00:00+09:00' WHERE id = ?1",
                [&older],
            )
            .unwrap();
            conn.execute(
                "UPDATE memories SET timestamp = '2026-01-01T05:00:00Z' WHERE id = ?1",
                [&newer],
            )
            .unwrap();
        }

        let llm = CannedLlm(r#"{"contradicts": true, "confidence": 1.0, "reason": "r"}"#.into());
        lint_project(&storage, "p", Some(&llm), 0.65).unwrap();
        let confirmed = storage.confirmed_conflicts().unwrap();
        assert_eq!(
            confirmed,
            vec![(older, newer)],
            "old/new must follow UTC instants, not string order"
        );
    }

    #[test]
    fn llm_dismisses_and_garbage_answers_degrade_to_candidate() {
        let storage = tmp_storage();
        let eid = project(&storage, "p");
        let _a = save_decision(&storage, &eid, "Ship weekly", vec![1.0, 0.0]);
        let _b = save_decision(&storage, &eid, "Ship weekly on Fridays", vec![0.99, 0.01]);
        let llm = CannedLlm(
            r#"{"contradicts": false, "confidence": 0.8, "reason": "refinement, not reversal"}"#
                .into(),
        );
        let stats = lint_project(&storage, "p", Some(&llm), 0.65).unwrap();
        assert_eq!(stats.dismissed, 1);
        assert!(storage.confirmed_conflicts().unwrap().is_empty());

        // Garbage answer on a fresh pair → candidate, not confirmed.
        let c = save_decision(&storage, &eid, "Deploy daily", vec![0.98, 0.02]);
        let garbage = CannedLlm("sure thing, they conflict!".into());
        let stats2 = lint_project(&storage, "p", Some(&garbage), 0.65).unwrap();
        assert!(stats2.confirmed == 0 && stats2.candidates_new >= 1);
        let _ = c;
    }

    /// Note-heavy projects must not exhaust the per-pass project limit:
    /// the pass is driven by projects that actually HAVE decision pairs.
    #[test]
    fn note_heavy_projects_cannot_starve_lint() {
        let storage = tmp_storage();
        // Twelve projects with a single decision each — none lintable, all
        // outranking the real pair by weighted activity.
        for i in 0..12 {
            let eid = project(&storage, &format!("noisy{i}"));
            save_decision(
                &storage,
                &eid,
                &format!("Lone decision {i}"),
                vec![0.0, 1.0],
            );
        }
        // The only project with an actual (similar) decision pair.
        let eid = project(&storage, "target");
        save_decision(&storage, &eid, "Use Postgres", vec![1.0, 0.0]);
        save_decision(&storage, &eid, "Switch to Redis", vec![0.99, 0.01]);

        let stats = run_lint_pass(&storage.clone(), None, 0.65).unwrap();
        assert_eq!(
            stats.candidates_new, 1,
            "the pair-bearing project must be linted despite 12 noisier ones"
        );
    }

    /// The judge must see decision CONTENT, not just titles: with generic
    /// titles the reversal often lives only in the body.
    #[test]
    fn judge_receives_decision_content() {
        let storage = tmp_storage();
        let eid = project(&storage, "p");
        // Generic titles; the real substance is in the bodies.
        let e1 = MemoryEntry::new(
            "Dependency change: package.json",
            "we standardize on postgres for storage",
            MemoryType::Decision,
            EventSource::Socket,
        );
        storage
            .save_with_embedding(&e1, Some(&vec![1.0, 0.0]))
            .unwrap();
        storage.link_memory_entity(&e1.id, &eid).unwrap();
        let e2 = MemoryEntry::new(
            "Dependency change: package.json",
            "we drop postgres and move storage to redis",
            MemoryType::Decision,
            EventSource::Socket,
        );
        storage
            .save_with_embedding(&e2, Some(&vec![0.99, 0.01]))
            .unwrap();
        storage.link_memory_entity(&e2.id, &eid).unwrap();

        let llm = RecordingLlm {
            answer: r#"{"contradicts": true, "confidence": 0.9, "reason": "r"}"#.into(),
            prompts: std::sync::Mutex::new(Vec::new()),
        };
        lint_project(&storage, "p", Some(&llm), 0.65).unwrap();
        let prompts = llm.prompts.lock().unwrap();
        assert_eq!(prompts.len(), 1);
        assert!(
            prompts[0].contains("standardize on postgres")
                && prompts[0].contains("move storage to redis"),
            "judge prompt must include decision bodies: {}",
            prompts[0]
        );
    }

    /// With more pair-bearing projects than any small per-pass cap, ALL of
    /// them are still linted in one pass — fully-judged fresh projects must
    /// not hold slots and starve older ones.
    #[test]
    fn all_pair_projects_are_linted_in_one_pass() {
        let storage = tmp_storage();
        for i in 0..11 {
            let eid = project(&storage, &format!("proj{i}"));
            save_decision(&storage, &eid, &format!("Use Postgres {i}"), vec![1.0, 0.0]);
            save_decision(
                &storage,
                &eid,
                &format!("Switch to Redis {i}"),
                vec![0.99, 0.01],
            );
        }
        let stats = run_lint_pass(&storage.clone(), None, 0.65).unwrap();
        assert_eq!(
            stats.candidates_new, 11,
            "every pair-bearing project gets linted, not just the freshest few"
        );
    }

    /// Confirmed conflicts must NOT touch the memories themselves.
    #[test]
    fn confirmed_conflict_leaves_memories_untouched() {
        let storage = tmp_storage();
        let eid = project(&storage, "p");
        let a = save_decision(&storage, &eid, "Use Postgres", vec![1.0, 0.0]);
        let b = save_decision(&storage, &eid, "Switch to Redis", vec![0.99, 0.01]);
        let llm = CannedLlm(r#"{"contradicts": true, "confidence": 1.0, "reason": "r"}"#.into());
        lint_project(&storage, "p", Some(&llm), 0.65).unwrap();

        let conn = storage.conn.lock().unwrap();
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM memories WHERE id IN (?1, ?2) AND superseded_by IS NULL",
                rusqlite::params![a, b],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 2, "superseded_by must stay NULL for both memories");
    }

    fn order(storage: &Storage, a: &str, b: &str) -> (String, String) {
        // Mirror lint's chronological ordering by timestamp string.
        let conn = storage.conn.lock().unwrap();
        let ts = |id: &str| -> String {
            conn.query_row("SELECT timestamp FROM memories WHERE id = ?1", [id], |r| {
                r.get(0)
            })
            .unwrap()
        };
        if ts(a) <= ts(b) {
            (a.to_string(), b.to_string())
        } else {
            (b.to_string(), a.to_string())
        }
    }
}
