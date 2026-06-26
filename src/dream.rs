//! Dream consolidation — overnight (or on-demand) generation of
//! higher-level summaries from session-grouped memories.
//!
//! The metaphor: while the user sleeps, fragmented session memories
//! get consolidated into narrative summaries — the same function REM
//! sleep performs for human memory. Retrieval at scale benefits more
//! from a few well-written summaries than from N raw atomic memories
//! that have to be reranked one by one.
//!
//! ## v1 — heuristic, deterministic, no LLM
//!
//! This module's `summarize_session_heuristic` produces a
//! `MemoryEntry { memory_type: SessionSummary, ... }` by reading
//! the memories linked to the session and assembling a structured
//! summary:
//!
//! - Window: `started_at → ended_at` (or "ongoing")
//! - Counts per memory_type (decisions / feedback / notes)
//! - Top entities by total mention_count across the session
//! - First and last memory titles as narrative anchors
//!
//! No LLM call. Cheap, deterministic, useful as a floor — and the
//! same output shape as the future v2 LLM generator will produce, so
//! callers can switch implementations without changing the consumer.
//!
//! ## v2 — LLM-driven prose (future)
//!
//! Same function signature, different body: feed the session
//! memories to an LLM with a "summarize this work session in 3
//! paragraphs" prompt, store the resulting prose. Wires into the
//! existing async extraction worker pattern.
//!
//! ## Idempotency
//!
//! Summaries link back to their source session via
//! `metadata.summary_of_session = "<session-uuid>"`. The CLI
//! exposes `summary_for_session` to check whether a summary already
//! exists; batch runs skip already-summarized sessions.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use std::collections::HashMap;

use crate::event::{EventSource, MemoryEntry, MemoryType};
use crate::graph::extractor_llm::LlmBackend;
use crate::storage::{Session, Storage};

/// Output of `summarize_session_heuristic`: a `MemoryEntry` ready to
/// save, with `metadata.summary_of_session` set to the source session
/// id (the link used by `summary_for_session` to detect duplicates).
///
/// `importance = 0.7` matches the conclusion default — session
/// summaries are above noise level but below explicit decisions.
pub fn summarize_session_heuristic(storage: &Storage, session_id: &str) -> Result<MemoryEntry> {
    summarize_session_heuristic_inner(storage, session_id, false)
}

/// Same as `summarize_session_heuristic` but explicitly opts into
/// summarizing an OPEN session (no `ended_at`). Use only when the
/// caller intends to refresh the summary later — the default
/// duplicate-detection by `summary_of_session` would otherwise
/// freeze a stale "still ongoing" snapshot. Codex caught this:
/// `dream run` happily summarized two open sessions, then `dream
/// batch` skipped them forever because the metadata link existed.
///
/// The CLI exposes this via `dream run --allow-open <id>`; the
/// default path stays strict so honest mistakes never produce
/// stale summaries on live sessions.
pub fn summarize_session_heuristic_allowing_open(
    storage: &Storage,
    session_id: &str,
) -> Result<MemoryEntry> {
    summarize_session_heuristic_inner(storage, session_id, true)
}

fn summarize_session_heuristic_inner(
    storage: &Storage,
    session_id: &str,
    allow_open: bool,
) -> Result<MemoryEntry> {
    let session = storage
        .session_by_id(session_id)?
        .ok_or_else(|| anyhow::anyhow!("session {session_id} not found"))?;

    // Codex P1: refuse open sessions unless explicit opt-in.
    // Summary text would include "Window: ... → ongoing" — fine as
    // a snapshot, but the metadata link means future runs skip the
    // session and the "ongoing" output stays stale forever.
    // Storage-side guard so this protects every caller, not just
    // the CLI.
    if !allow_open && session.is_open() {
        anyhow::bail!(
            "session {session_id} is still open (no ended_at). \
             Refusing to summarize an active session — its memories \
             are still accumulating. Wait for the idle-timeout to \
             close it, or call --allow-open if you really want a \
             snapshot."
        );
    }

    let memories = storage.memories_for_session(session_id)?;
    if memories.is_empty() {
        anyhow::bail!("session {session_id} has no memories — nothing to summarize");
    }

    let body = render_heuristic_body(&session, &memories, storage);
    let title = render_heuristic_title(&session, storage);

    let mut entry = MemoryEntry::new(title, body, MemoryType::SessionSummary, EventSource::Manual);
    // Importance just above background (0.5 default) so summaries
    // surface in retrieval but don't crowd out explicit decisions
    // (which sit at 0.7 too — fine; rerank handles tie-breaks).
    entry.importance = 0.7;
    entry.tags = vec!["session-summary".into(), "dream".into()];
    entry.metadata = serde_json::json!({
        "summary_of_session": session_id,
        "memory_count": memories.len(),
        "summarizer": "heuristic-v1",
        // Track snapshot-of-open-session so we can later distinguish
        // "permanent summary for closed session" from "snapshot of
        // active session that should be refreshable". v2 may use
        // this for freshness-based regeneration.
        "open_at_summary_time": session.is_open(),
    });
    Ok(entry)
}

/// LLM-driven summarizer (the v2 of dream consolidation). Same input
/// gating as `summarize_session_heuristic` (strict on open sessions,
/// requires at least one memory), but the body is prose generated
/// by an `LlmBackend` instead of the structured count/anchor layout.
///
/// Returns a `MemoryEntry` ready to save. Metadata still carries
/// `summary_of_session` and `open_at_summary_time` so the existing
/// snapshot-aware idempotency in `session_summary_lookup` Just Works
/// for LLM summaries too — no special-casing in the storage layer.
/// `summarizer` is tagged `"llm-v1"` so the audit path can tell
/// heuristic and LLM summaries apart.
///
/// Falls back to a clear error if the LLM produces empty output;
/// the heuristic remains the safety net (caller can retry with
/// `summarize_session_heuristic`).
pub fn summarize_session_llm(
    storage: &Storage,
    session_id: &str,
    backend: &dyn LlmBackend,
) -> Result<MemoryEntry> {
    summarize_session_llm_inner(storage, session_id, backend, false)
}

/// LLM equivalent of `summarize_session_heuristic_allowing_open` —
/// snapshot path for active sessions. Same `open_at_summary_time`
/// tagging so `session_summary_lookup` continues to skip snapshots
/// when batch runs look for a canonical summary.
#[allow(dead_code)]
pub fn summarize_session_llm_allowing_open(
    storage: &Storage,
    session_id: &str,
    backend: &dyn LlmBackend,
) -> Result<MemoryEntry> {
    summarize_session_llm_inner(storage, session_id, backend, true)
}

fn summarize_session_llm_inner(
    storage: &Storage,
    session_id: &str,
    backend: &dyn LlmBackend,
    allow_open: bool,
) -> Result<MemoryEntry> {
    // Gate identically to the heuristic path — same error messages
    // so the CLI's --llm vs heuristic choice doesn't change semantics
    // around what's a valid input.
    let session = storage
        .session_by_id(session_id)?
        .ok_or_else(|| anyhow::anyhow!("session {session_id} not found"))?;
    if !allow_open && session.is_open() {
        anyhow::bail!(
            "session {session_id} is still open (no ended_at). \
             Refusing to summarize an active session — its memories \
             are still accumulating. Wait for the idle-timeout to \
             close it, or call --allow-open if you really want a \
             snapshot."
        );
    }
    let memories = storage.memories_for_session(session_id)?;
    if memories.is_empty() {
        anyhow::bail!("session {session_id} has no memories — nothing to summarize");
    }

    let prompt = build_llm_prompt(&session, &memories);
    let body_raw = backend
        .generate(&prompt)
        .context("LLM backend failed to generate session summary")?;
    let body = extract_llm_summary_text(&body_raw)
        .with_context(|| format!("LLM returned no usable summary text: {body_raw}"))?;
    let title = render_heuristic_title(&session, storage);

    let mut entry = MemoryEntry::new(title, body, MemoryType::SessionSummary, EventSource::Manual);
    entry.importance = 0.7;
    entry.tags = vec![
        "session-summary".into(),
        "dream".into(),
        "llm-generated".into(),
    ];
    entry.metadata = serde_json::json!({
        "summary_of_session": session_id,
        "memory_count": memories.len(),
        "summarizer": "llm-v1",
        "open_at_summary_time": session.is_open(),
    });
    Ok(entry)
}

/// Build the LLM prompt. Mirrors the conclusions_generator pattern:
/// bulleted list of memories (title + truncated content), then a
/// task instruction asking for a concise narrative summary. Ollama
/// is configured for `format=json` globally; we request a wrapped
/// `{"summary": "..."}` shape so the extractor can pull out the
/// text reliably even when the model adds preamble or trailing
/// punctuation.
fn build_llm_prompt(session: &Session, memories: &[MemoryEntry]) -> String {
    let mut lines = Vec::new();
    for m in memories {
        let snippet: String = m.content.chars().take(240).collect();
        let snippet_trimmed = snippet.replace('\n', " ");
        lines.push(format!(
            "- [{}] {} — {}",
            m.memory_type, m.title, snippet_trimmed
        ));
    }
    let memories_block = lines.join("\n");
    let window = match (session.ended_at.as_deref(), session.is_open()) {
        (Some(end), _) => format!("{} → {}", session.started_at, end),
        (None, true) => format!("{} → ongoing", session.started_at),
        (None, false) => session.started_at.clone(),
    };
    format!(
        r#"You are writing a session summary for a developer's memory system.

Session window: {window}
Source: {source}

Memories captured during this session (newest first):
{memories_block}

Task: write a tight 2 to 4 sentence prose summary of what happened in this session. Focus on:
- The main thread of work (what was being built / decided / debugged)
- Concrete outcomes or decisions
- Open threads or follow-ups, if any

Do NOT enumerate every memory. Synthesize. Avoid timestamps and memory ids.

Respond as a JSON object of the form:
{{"summary": "<the prose summary>"}}
"#,
        window = window,
        source = session.source,
        memories_block = memories_block,
    )
}

/// Extract the `summary` field from the LLM's response. Tolerant of
/// two shapes:
/// 1. `{"summary": "..."}` — the prompt's requested form.
/// 2. Bare prose — fallback when the model ignores the JSON
///    instruction. We trust the raw output if it's non-empty after
///    trimming, but log nothing — the caller surfaces "LLM returned
///    no usable summary text" via context if THIS function errors.
fn extract_llm_summary_text(raw: &str) -> Result<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        anyhow::bail!("LLM returned empty response");
    }
    // Shape 1: wrapped JSON object.
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed)
        && let Some(s) = value.get("summary").and_then(|v| v.as_str())
    {
        let s = s.trim();
        if !s.is_empty() {
            return Ok(s.to_string());
        }
    }
    // Shape 2: raw prose. Accept anything non-empty after trimming.
    // This branch covers ollama configs where format=json is off and
    // local models that ignore the JSON instruction.
    Ok(trimmed.to_string())
}

/// Look up an existing summary for a session. Returns the summary
/// `MemoryEntry` if one was already generated, or `None`. Used by
/// batch dream runs to skip already-summarized sessions and by the
/// CLI to surface "already done" instead of generating a duplicate.
///
/// Implementation: SQL JSON extraction on `memories.metadata`. We
/// don't have an index for this column today; cost is O(N) over
/// session_summary rows, which stays cheap until summary counts
/// grow into the tens of thousands. Add an index then.
pub fn summary_for_session(storage: &Storage, session_id: &str) -> Result<Option<MemoryEntry>> {
    storage.session_summary_lookup(session_id)
}

/// Format the title. "Session summary: <peer> · <date>". Peer name
/// is best-effort — if we can't resolve, fall back to "(unknown)".
///
/// Date uses the parsed timestamp's YYYY-MM-DD format. Codex caught
/// that the previous `split('T')` only worked on RFC3339 — SQLite
/// rows in the live DB use `2026-05-25 06:04:35` (space, not T),
/// so the title accidentally included the full timestamp. The
/// shared `parse_session_timestamp` helper accepts both formats.
fn render_heuristic_title(session: &Session, storage: &Storage) -> String {
    let peer_label = storage
        .peer_by_id(&session.peer_id)
        .ok()
        .flatten()
        .map(|p| p.label().to_string())
        .unwrap_or_else(|| "(unknown)".into());
    let date = parse_session_timestamp(&session.started_at)
        .map(|dt| dt.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| {
            // Last-resort: lop off everything from the first
            // delimiter so a malformed row still gets a readable date.
            session
                .started_at
                .split(['T', ' '])
                .next()
                .unwrap_or(&session.started_at)
                .to_string()
        });
    format!("Session summary: {peer_label} · {date}")
}

/// Assemble the body text. Multi-section markdown so the summary is
/// readable both in `mnemonic recent` and in the eventual dashboard.
fn render_heuristic_body(session: &Session, memories: &[MemoryEntry], storage: &Storage) -> String {
    use std::fmt::Write;

    let mut out = String::new();

    // Window
    let started = &session.started_at;
    let ended = session.ended_at.as_deref().unwrap_or("ongoing");
    let _ = writeln!(out, "Window: {started} → {ended}");
    if let Some(d) = session_duration(session) {
        let _ = writeln!(out, "Duration: {d}");
    }
    let _ = writeln!(out, "Source: {}", session.source);
    let _ = writeln!(out);

    // Memory type counts
    let counts = count_by_type(memories);
    let total = memories.len();
    let _ = writeln!(
        out,
        "{total} memor{}:",
        if total == 1 { "y" } else { "ies" }
    );
    for (kind, n) in &counts {
        let _ = writeln!(out, "  · {kind}: {n}");
    }
    let _ = writeln!(out);

    // Top entities — bounded query against memory_entities → entities
    // across this session's memory ids. Returns (name, mention_count)
    // pairs we can present without further processing.
    if let Ok(entities) = storage.top_entities_for_session(&session.id, 5)
        && !entities.is_empty()
    {
        let _ = writeln!(out, "Top entities:");
        for (name, count) in entities {
            let _ = writeln!(out, "  · {name} ({count} mentions)");
        }
        let _ = writeln!(out);
    }

    // Narrative anchors
    if let (Some(first), Some(last)) = (memories.first(), memories.last()) {
        let _ = writeln!(out, "Opens with: {}", first.title);
        if first.id != last.id {
            let _ = writeln!(out, "Closes with: {}", last.title);
        }
    }

    out
}

/// Group memory_type → count. Output is a `Vec` (not HashMap) so
/// output order is stable across summaries — alphabetical by type
/// name, which matches the MemoryType enum ordering well enough.
fn count_by_type(memories: &[MemoryEntry]) -> Vec<(String, usize)> {
    let mut by_type: HashMap<String, usize> = HashMap::new();
    for m in memories {
        *by_type.entry(m.memory_type.to_string()).or_default() += 1;
    }
    let mut pairs: Vec<_> = by_type.into_iter().collect();
    pairs.sort_by(|a, b| a.0.cmp(&b.0));
    pairs
}

/// Parse a session timestamp string into UTC. Sessions opened by
/// the storage helpers use SQLite's `datetime('now')` which produces
/// `YYYY-MM-DD HH:MM:SS` (space delimiter, no timezone) and is
/// implicitly UTC. Sessions touched via `open_or_reuse_session_for_key`
/// use chrono's `to_rfc3339()`, producing the standard form with
/// timezone. This helper accepts both so duration math and date
/// extraction don't silently fall over when the live DB has mixed
/// timestamp formats — Codex caught this exact gap on the live DB
/// where `Duration:` was missing from summaries because the rows
/// were in SQLite format and only RFC3339 was being tried.
pub fn parse_session_timestamp(s: &str) -> Option<DateTime<Utc>> {
    // RFC3339 first (newer rows + manual code paths via to_rfc3339).
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Some(dt.with_timezone(&Utc));
    }
    // SQLite default: "%Y-%m-%d %H:%M:%S" in UTC.
    if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S") {
        return Some(naive.and_utc());
    }
    // SQLite sub-second variant (rarer but possible after a UPDATE
    // with datetime('now', 'subsec')).
    if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S%.f") {
        return Some(naive.and_utc());
    }
    None
}

/// Render `started_at → ended_at` as a duration string. Returns
/// None for ongoing sessions (no ended_at) or malformed timestamps.
fn session_duration(session: &Session) -> Option<String> {
    let ended_at = session.ended_at.as_ref()?;
    let start = parse_session_timestamp(&session.started_at)?;
    let end = parse_session_timestamp(ended_at)?;
    let dur = end.signed_duration_since(start);
    let secs = dur.num_seconds();
    if secs < 0 {
        // Clock skew or hand-edited rows — refuse to print a
        // negative duration. Caller drops the field.
        return None;
    }
    if secs < 60 {
        Some(format!("{secs}s"))
    } else if secs < 3600 {
        Some(format!("{}m {}s", secs / 60, secs % 60))
    } else {
        Some(format!("{}h {}m", secs / 3600, (secs % 3600) / 60))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{EventSource, MemoryEntry, MemoryType};
    use chrono::Utc;
    use std::path::PathBuf;
    use std::sync::Arc;

    fn tmp_storage() -> Arc<Storage> {
        let dir =
            std::env::temp_dir().join(format!("mnemonic-dream-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        Arc::new(Storage::open(&dir.join("memory.db")).unwrap())
    }

    fn make_entry(title: &str, mt: MemoryType) -> MemoryEntry {
        MemoryEntry {
            id: uuid::Uuid::new_v4().to_string(),
            timestamp: Utc::now(),
            title: title.into(),
            content: "c".into(),
            memory_type: mt,
            tags: vec![],
            source: EventSource::Manual,
            importance: 0.5,
            metadata: serde_json::Value::Null,
        }
    }

    /// Happy path: a CLOSED session with mixed memory types produces
    /// a summary that includes the counts and anchors. Smoke-tests
    /// the assembly pipeline end to end. Note the explicit
    /// `end_session` — after Codex's P1 fix, the default summarizer
    /// refuses open sessions, so every realistic test must close
    /// the session before summarizing.
    #[test]
    fn summarize_session_heuristic_produces_session_summary_with_counts() {
        let storage = tmp_storage();
        let peer_id = storage.upsert_peer("claude", None, "agent").unwrap();
        let session_id = storage
            .open_session(&peer_id, Some("test"), "jsonl")
            .unwrap();

        // Three memories of different types.
        for (title, mt) in [
            ("First decision", MemoryType::Decision),
            ("Feedback note", MemoryType::Feedback),
            ("Closing note", MemoryType::Note),
        ] {
            let entry = make_entry(title, mt);
            storage.save(&entry).unwrap();
            storage
                .set_memory_session(&entry.id, Some(&session_id))
                .unwrap();
        }

        // Close session — required by the strict-by-default
        // summarizer to avoid producing stale "ongoing" snapshots.
        storage.end_session(&session_id).unwrap();

        let summary = summarize_session_heuristic(&storage, &session_id).unwrap();
        assert_eq!(summary.memory_type, MemoryType::SessionSummary);
        assert!(summary.title.starts_with("Session summary:"));
        assert!(summary.content.contains("decision: 1"));
        assert!(summary.content.contains("feedback: 1"));
        assert!(summary.content.contains("note: 1"));
        assert!(summary.content.contains("First decision"));
        assert!(summary.content.contains("Closing note"));

        // Metadata link round-trip: summary_of_session must point at
        // the source session so duplicate detection works.
        assert_eq!(
            summary
                .metadata
                .get("summary_of_session")
                .and_then(|v| v.as_str()),
            Some(session_id.as_str())
        );
        // Closed-session summaries record open_at_summary_time=false
        // so future tooling can distinguish snapshots from permanent
        // summaries.
        assert_eq!(
            summary
                .metadata
                .get("open_at_summary_time")
                .and_then(|v| v.as_bool()),
            Some(false)
        );
    }

    /// An empty session can't be summarized — refuse loudly instead
    /// of producing a meaningless "0 memories" summary. v2 LLM
    /// generator will need the same guard. Session is closed here
    /// to isolate the empty-session error from the open-session
    /// guard added below.
    #[test]
    fn summarize_session_heuristic_rejects_empty_session() {
        let storage = tmp_storage();
        let peer_id = storage.upsert_peer("claude", None, "agent").unwrap();
        let session_id = storage
            .open_session(&peer_id, Some("empty"), "jsonl")
            .unwrap();
        storage.end_session(&session_id).unwrap();
        let err = summarize_session_heuristic(&storage, &session_id);
        assert!(err.is_err(), "empty session must error");
    }

    /// Unknown session id errors with a clear message — protects
    /// the batch path from silently producing summaries for
    /// non-existent sessions.
    #[test]
    fn summarize_session_heuristic_rejects_unknown_session_id() {
        let storage = tmp_storage();
        let err = summarize_session_heuristic(&storage, "nope");
        assert!(err.is_err());
    }

    /// `summary_for_session` round-trip: after saving a summary,
    /// the helper finds it. Confirms the metadata-based lookup is
    /// the right mechanism for "is this session already
    /// summarized?" — used by batch runs to skip duplicates.
    #[test]
    fn summary_for_session_finds_saved_summary_by_metadata_link() {
        let storage = tmp_storage();
        let peer_id = storage.upsert_peer("claude", None, "agent").unwrap();
        let session_id = storage
            .open_session(&peer_id, Some("test"), "jsonl")
            .unwrap();
        let memory = make_entry("any", MemoryType::Note);
        storage.save(&memory).unwrap();
        storage
            .set_memory_session(&memory.id, Some(&session_id))
            .unwrap();
        storage.end_session(&session_id).unwrap();

        // Pre-check: no summary yet.
        assert!(
            summary_for_session(&storage, &session_id)
                .unwrap()
                .is_none()
        );

        let summary = summarize_session_heuristic(&storage, &session_id).unwrap();
        storage.save(&summary).unwrap();

        let found = summary_for_session(&storage, &session_id).unwrap();
        assert!(found.is_some(), "saved summary must be discoverable");
        assert_eq!(found.unwrap().id, summary.id);
    }

    /// Codex P1: open sessions must be refused by the default
    /// summarizer. The metadata link freezes future runs into
    /// thinking the session is already summarized, so silently
    /// snapshotting an active session is a real bug. Error message
    /// suggests `--allow-open` for callers who genuinely want a
    /// snapshot.
    #[test]
    fn summarize_session_heuristic_rejects_open_session_by_default() {
        let storage = tmp_storage();
        let peer_id = storage.upsert_peer("claude", None, "agent").unwrap();
        let session_id = storage
            .open_session(&peer_id, Some("active"), "jsonl")
            .unwrap();
        let memory = make_entry("ongoing", MemoryType::Note);
        storage.save(&memory).unwrap();
        storage
            .set_memory_session(&memory.id, Some(&session_id))
            .unwrap();
        // Don't close the session — that's the failure scenario.

        let err = summarize_session_heuristic(&storage, &session_id);
        assert!(err.is_err(), "open session must be refused");
        let msg = format!("{}", err.unwrap_err());
        assert!(
            msg.contains("still open") && msg.contains("--allow-open"),
            "error message should explain the guard and the opt-out: {msg}"
        );
    }

    /// `summarize_session_heuristic_allowing_open` is the explicit
    /// opt-out the CLI's `dream run --allow-open` uses. It must
    /// succeed on the same open-session scenario the strict path
    /// rejects, and tag the summary metadata with
    /// `open_at_summary_time = true` so v2 freshness logic can
    /// distinguish snapshot summaries from permanent ones.
    #[test]
    fn summarize_session_heuristic_allowing_open_succeeds_and_tags_metadata() {
        let storage = tmp_storage();
        let peer_id = storage.upsert_peer("claude", None, "agent").unwrap();
        let session_id = storage
            .open_session(&peer_id, Some("active"), "jsonl")
            .unwrap();
        let memory = make_entry("ongoing", MemoryType::Note);
        storage.save(&memory).unwrap();
        storage
            .set_memory_session(&memory.id, Some(&session_id))
            .unwrap();

        let summary = summarize_session_heuristic_allowing_open(&storage, &session_id).unwrap();
        assert_eq!(
            summary
                .metadata
                .get("open_at_summary_time")
                .and_then(|v| v.as_bool()),
            Some(true),
            "snapshot of open session must be flagged for future freshness checks"
        );
        // Body should say "ongoing" because ended_at is NULL — the
        // snapshot is honest about what it is.
        assert!(
            summary.content.contains("ongoing"),
            "open-session snapshot should mention ongoing in body: {}",
            summary.content
        );
    }

    /// Codex P2: timestamp parser must accept BOTH RFC3339 (from
    /// chrono `to_rfc3339()` calls) AND SQLite's `datetime('now')`
    /// format (space delimiter, no timezone, implicit UTC). The
    /// previous code only tried RFC3339, so live SQLite rows
    /// silently failed to parse and `Duration:` vanished from
    /// summaries.
    #[test]
    fn parse_session_timestamp_accepts_rfc3339_and_sqlite_formats() {
        // RFC3339 with explicit Z.
        let rfc = parse_session_timestamp("2026-05-25T06:04:35Z").expect("RFC3339 must parse");
        // SQLite default — space delimiter, no zone.
        let sqlite =
            parse_session_timestamp("2026-05-25 06:04:35").expect("SQLite datetime() must parse");
        // Both should resolve to the same UTC instant.
        assert_eq!(rfc, sqlite, "RFC3339 and SQLite forms must agree");

        // Sub-second SQLite variant (`datetime('now', 'subsec')`).
        assert!(parse_session_timestamp("2026-05-25 06:04:35.123").is_some());

        // Garbage rejected — None means caller drops the field
        // rather than crashing.
        assert!(parse_session_timestamp("not a date").is_none());
        assert!(parse_session_timestamp("").is_none());
    }

    /// End-to-end: a session with SQLite-format timestamps (what
    /// `open_session` actually writes via `datetime('now')`) must
    /// produce a title with a clean YYYY-MM-DD date AND a Duration
    /// line. The previous code would have left the title as
    /// `Session summary: Claude · 2026-05-25 06:04:35` and dropped
    /// Duration entirely.
    #[test]
    fn summary_title_extracts_date_and_includes_duration_on_sqlite_timestamps() {
        let storage = tmp_storage();
        let peer_id = storage.upsert_peer("claude", None, "agent").unwrap();
        let session_id = storage
            .open_session(&peer_id, Some("test"), "jsonl")
            .unwrap();
        let memory = make_entry("note", MemoryType::Note);
        storage.save(&memory).unwrap();
        storage
            .set_memory_session(&memory.id, Some(&session_id))
            .unwrap();
        storage.end_session(&session_id).unwrap();

        let summary = summarize_session_heuristic(&storage, &session_id).unwrap();
        // Title's date part should be exactly 10 chars (YYYY-MM-DD)
        // — proves date extraction worked on the SQLite-format
        // started_at instead of dumping the full timestamp.
        let date_part = summary
            .title
            .rsplit_once(" · ")
            .map(|(_, d)| d)
            .expect("title must have ' · <date>' suffix");
        assert_eq!(
            date_part.len(),
            10,
            "title date must be YYYY-MM-DD, got `{date_part}`"
        );
        // Duration line must be present (was missing pre-fix because
        // RFC3339 parser silently failed on SQLite format).
        assert!(
            summary.content.contains("Duration:"),
            "Duration line must appear for closed sessions: {}",
            summary.content
        );
    }

    // Suppress unused PathBuf import warning when tests below are added later.
    #[allow(dead_code)]
    fn _suppress_unused() -> Option<PathBuf> {
        None
    }

    // ─── LLM summarizer tests ───
    //
    // The LLM dream path mirrors the heuristic one — same gating
    // semantics, same metadata link, snapshot-aware. Mock the
    // backend so tests don't need Ollama. The mock captures the
    // prompt via an Arc<Mutex<Option<String>>> handle (same
    // pattern as the conclusions_generator tests).

    struct FakeBackend {
        response: String,
        captured_prompt: std::sync::Arc<std::sync::Mutex<Option<String>>>,
    }
    impl FakeBackend {
        fn new(response: &str) -> Self {
            Self {
                response: response.to_string(),
                captured_prompt: std::sync::Arc::new(std::sync::Mutex::new(None)),
            }
        }
        fn prompt_handle(&self) -> std::sync::Arc<std::sync::Mutex<Option<String>>> {
            self.captured_prompt.clone()
        }
    }
    impl LlmBackend for FakeBackend {
        fn generate(&self, prompt: &str) -> anyhow::Result<String> {
            *self.captured_prompt.lock().unwrap() = Some(prompt.to_string());
            Ok(self.response.clone())
        }
    }

    /// Happy path: closed session → LLM produces wrapped JSON
    /// summary → entry carries prose body + correct metadata
    /// (`summarizer = "llm-v1"`, `open_at_summary_time = false`,
    /// `llm-generated` tag).
    #[test]
    fn summarize_session_llm_returns_prose_with_correct_metadata() {
        let storage = tmp_storage();
        let peer_id = storage.upsert_peer("claude", None, "agent").unwrap();
        let session_id = storage
            .open_session(&peer_id, Some("test"), "jsonl")
            .unwrap();
        let mem = make_entry("Fixed auth bug", MemoryType::Decision);
        storage.save(&mem).unwrap();
        storage
            .set_memory_session(&mem.id, Some(&session_id))
            .unwrap();
        storage.end_session(&session_id).unwrap();

        let backend = FakeBackend::new(
            r#"{"summary":"Worked on the auth module: identified a token validation bug and shipped the fix. Followups: backfill the test suite."}"#,
        );
        let prompt_handle = backend.prompt_handle();
        let summary = summarize_session_llm(&storage, &session_id, &backend).unwrap();

        assert_eq!(summary.memory_type, MemoryType::SessionSummary);
        assert!(summary.content.contains("auth module"));
        assert!(summary.content.contains("token validation"));
        assert_eq!(
            summary.metadata.get("summarizer").and_then(|v| v.as_str()),
            Some("llm-v1")
        );
        assert_eq!(
            summary
                .metadata
                .get("open_at_summary_time")
                .and_then(|v| v.as_bool()),
            Some(false)
        );
        assert!(summary.tags.contains(&"llm-generated".to_string()));

        // Prompt should reference the memory title — proves the
        // assembly path uses the right input.
        let prompt = prompt_handle.lock().unwrap().clone().unwrap();
        assert!(prompt.contains("Fixed auth bug"));
        assert!(prompt.contains("Session window:"));
    }

    /// Same open-session refusal as the heuristic path — LLM
    /// summarizer is not a back door to active-session snapshots.
    /// Codex's P1 still applies.
    #[test]
    fn summarize_session_llm_rejects_open_session_by_default() {
        let storage = tmp_storage();
        let peer_id = storage.upsert_peer("claude", None, "agent").unwrap();
        let session_id = storage
            .open_session(&peer_id, Some("active"), "jsonl")
            .unwrap();
        let mem = make_entry("ongoing", MemoryType::Note);
        storage.save(&mem).unwrap();
        storage
            .set_memory_session(&mem.id, Some(&session_id))
            .unwrap();

        let backend = FakeBackend::new(r#"{"summary":"shouldn't matter"}"#);
        let err = summarize_session_llm(&storage, &session_id, &backend);
        assert!(err.is_err(), "open session must be refused");
        assert!(err.unwrap_err().to_string().contains("still open"));
    }

    /// Allow-open opt-in path produces a snapshot tagged
    /// `open_at_summary_time = true` so the session_summary_lookup
    /// canonical filter skips it. Same semantics as heuristic.
    #[test]
    fn summarize_session_llm_allowing_open_tags_snapshot_metadata() {
        let storage = tmp_storage();
        let peer_id = storage.upsert_peer("claude", None, "agent").unwrap();
        let session_id = storage
            .open_session(&peer_id, Some("active"), "jsonl")
            .unwrap();
        let mem = make_entry("ongoing", MemoryType::Note);
        storage.save(&mem).unwrap();
        storage
            .set_memory_session(&mem.id, Some(&session_id))
            .unwrap();

        let backend = FakeBackend::new(r#"{"summary":"snapshot text"}"#);
        let summary = summarize_session_llm_allowing_open(&storage, &session_id, &backend).unwrap();
        assert_eq!(
            summary
                .metadata
                .get("open_at_summary_time")
                .and_then(|v| v.as_bool()),
            Some(true)
        );
    }

    /// Backend returns bare prose (no JSON wrapper) — extractor
    /// falls back to using the trimmed raw text. Covers local
    /// models that ignore the JSON format directive.
    #[test]
    fn extract_llm_summary_text_accepts_bare_prose_fallback() {
        let out = extract_llm_summary_text("  Worked on auth. Fixed a bug. Shipped.  ").unwrap();
        assert_eq!(out, "Worked on auth. Fixed a bug. Shipped.");
    }

    /// Wrapped JSON: extract the `summary` field's text.
    #[test]
    fn extract_llm_summary_text_unwraps_summary_field() {
        let out = extract_llm_summary_text(r#"{"summary":"the prose summary"}"#).unwrap();
        assert_eq!(out, "the prose summary");
    }

    /// Empty response is an error. Empty `summary` field also
    /// falls through to bare-prose, which then errors as empty.
    #[test]
    fn extract_llm_summary_text_rejects_empty() {
        assert!(extract_llm_summary_text("").is_err());
        assert!(extract_llm_summary_text("   \n  ").is_err());
    }
}
