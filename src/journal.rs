//! Daily journal / digest — a readable recap of one day's work, assembled from
//! the memory graph + project-time attribution.
//!
//! ## Two honest layers
//!
//! 1. A **deterministic fact-collector** ([`collect`]) — the source of truth.
//!    It reads the day's attributed projects (with real hours + confidence from
//!    the attribution tables), the decisions and follow-ups captured that day,
//!    and a few title-bullets per project. No model, no guessing — every line
//!    traces to a real memory or a measured second.
//!
//! 2. A planned (deferred) optional **local-LLM rewrite** that would only
//!    rephrase the collected facts into a human paragraph — never adding work
//!    that didn't happen (facts are fixed before it runs), with the
//!    deterministic summary as the fallback. v1 ships the deterministic layer
//!    only; the LLM is a later polish pass, off by default.
//!
//! So the digest is always valid and always honest.

use anyhow::Result;
use chrono::NaiveDate;
use serde::Serialize;
use std::collections::HashMap;

use crate::activity::{ActivityStore, local_day_bounds_utc};
use crate::event::{MemoryEntry, MemoryType};
use crate::storage::{Storage, is_meta_memory};

/// Same noise floor as attribution: an entity needs this many real memories to
/// count as a project (keeps the journal's project set == the tracked one).
const MIN_PROJECT_MEMS: i64 = 2;
const MAX_BULLETS_PER_PROJECT: usize = 4;
const MAX_EVENTS_PER_PROJECT: usize = 10;
const MAX_DECISIONS: usize = 8;
const MAX_FOLLOWUPS: usize = 8;
/// Cap a bullet/summary line so the widget rows stay tidy.
const MAX_LINE_CHARS: usize = 110;
/// In the Journal, tiny attributed slivers with no concrete captured event read
/// as noise ("project-zeta 3m"). Keep the accounting honest by rolling them into
/// the unattributed bucket for the digest only; Projects still shows raw time.
const MIN_DISPLAY_PROJECT_SECONDS: f64 = 10.0 * 60.0;

/// A single timestamped thing that happened under a project — the richer form
/// of a bullet. Additive to the contract: `bullets` stays for older clients.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct JournalEvent {
    /// One-line readable text (same source as a bullet).
    pub text: String,
    /// Local clock label for the UI, e.g. "7:38pm".
    pub time_label: String,
    /// Full RFC3339 timestamp (UTC) for sorting / future use.
    pub timestamp: String,
    /// Source memory id — for a future tap-to-open / "show source".
    pub memory_id: String,
}

/// One project's slice of a day: measured hours + the work that produced them.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct JournalProject {
    pub key: String,
    pub name: String,
    pub seconds: f64,
    pub confidence: Option<String>,
    /// Plain title lines — kept for backward compatibility.
    pub bullets: Vec<String>,
    /// Timestamped events (text + time_label + memory_id) — the new contract.
    pub events: Vec<JournalEvent>,
}

/// A decision or follow-up, linked back to its source memory.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct JournalItem {
    pub title: String,
    pub memory_id: String,
}

/// The full digest for one local day — serializes to the `/api/journal`
/// contract the widget's Journal page consumes.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct JournalDay {
    pub day: String,
    pub summary: String,
    pub projects: Vec<JournalProject>,
    pub decisions: Vec<JournalItem>,
    pub follow_ups: Vec<JournalItem>,
    pub unattributed_seconds: f64,
}

/// Words that mark a memory as a forward-looking follow-up / TODO. Matched
/// case-insensitively against the title (English + the way User writes them).
const FOLLOWUP_MARKERS: &[&str] = &[
    "todo",
    "follow-up",
    "follow up",
    "followup",
    "backlog",
    "next step",
    "next:",
    "to do",
    "осталось",
    "надо ",
    "доделать",
    "не забыть",
];

/// Collect the deterministic facts for `day`. This is the correctness core —
/// every field is derived from stored data, never invented.
pub fn collect(storage: &Storage, activity: &ActivityStore, day: NaiveDate) -> Result<JournalDay> {
    let day_str = day.format("%Y-%m-%d").to_string();
    let Some((start, end)) = local_day_bounds_utc(day) else {
        return Ok(empty_day(&day_str));
    };

    // Measured hours per project + the honest unattributed bucket.
    let (attr_rows, unattributed_seconds) = activity.project_attribution_for_day(day)?;

    // The day's project-linked memories, for per-project bullets/events + names.
    let proj_mems = storage.project_memories_in_window(start, end, MIN_PROJECT_MEMS)?;
    type ProjBuild = (String, Vec<String>, Vec<JournalEvent>);
    let mut by_key: HashMap<String, ProjBuild> = HashMap::new();
    let mut memory_candidates: HashMap<String, (MemoryEntry, Vec<(String, String)>)> =
        HashMap::new();
    for (key, name, mems) in proj_mems {
        for m in mems.into_iter().filter(|m| !is_noise_title(&m.title)) {
            let entry = memory_candidates
                .entry(m.id.clone())
                .or_insert_with(|| (m, Vec::new()));
            if !entry.1.iter().any(|(k, _)| k == &key) {
                entry.1.push((key.clone(), name.clone()));
            }
        }
    }

    let mut memory_candidates: Vec<(MemoryEntry, Vec<(String, String)>)> =
        memory_candidates.into_values().collect();
    memory_candidates.sort_by_key(|(m, _)| m.timestamp);
    for (m, candidates) in memory_candidates {
        let Some((key, name)) = primary_project_for_memory(&m, &candidates) else {
            continue;
        };
        let text = clean_line(&m.title);
        if text.is_empty() {
            continue;
        }
        let (_, bullets, events) = by_key
            .entry(key)
            .or_insert_with(|| (name, Vec::new(), Vec::new()));
        if bullets.len() < MAX_BULLETS_PER_PROJECT {
            bullets.push(text.clone());
        }
        if events.len() < MAX_EVENTS_PER_PROJECT {
            events.push(JournalEvent {
                text,
                time_label: time_label(m.timestamp),
                timestamp: m.timestamp.to_rfc3339(),
                memory_id: m.id.clone(),
            });
        }
    }

    // Build one project entry per attributed project, preserving the
    // attribution's seconds-desc order.
    let mut projects = Vec::with_capacity(attr_rows.len());
    let mut display_unattributed_seconds = unattributed_seconds;
    for (key, seconds, confidence) in attr_rows {
        let (name, bullets, events) = match by_key.remove(&key) {
            Some(v) => v,
            // Attributed (e.g. via the padded window) but no in-day memory:
            // recover the name from the graph, leave bullets/events empty.
            None => {
                let name = storage
                    .project_overview_by_id(&key, 0)
                    .ok()
                    .flatten()
                    .map(|p| p.name)
                    .unwrap_or_else(|| key.clone());
                (name, Vec::new(), Vec::new())
            }
        };
        if should_fold_empty_project(seconds, &bullets, &events) {
            display_unattributed_seconds += seconds;
            continue;
        }
        projects.push(JournalProject {
            key,
            name,
            seconds,
            confidence,
            bullets,
            events,
        });
    }

    // Decisions + follow-ups from the day's memories. Skip anything already
    // shown as a project event — otherwise the section just repeats the same
    // commit list and the digest reads twice as noisy.
    let shown_event_ids: std::collections::HashSet<&str> = projects
        .iter()
        .flat_map(|p| p.events.iter().map(|e| e.memory_id.as_str()))
        .collect();
    let day_mems = storage.memories_in_window(start, end)?;
    let mut decisions = Vec::new();
    let mut follow_ups = Vec::new();
    let mut seen_followup_ids = std::collections::HashSet::new();
    for m in &day_mems {
        // Conversation-watcher chatter and user corrections are tagged
        // `conversation`/`correction` (or typed feedback). They're fine as raw
        // memories but read as noise in a digest — "Глянул — кадры годные",
        // "мне надо было видео" are replies, not decisions/follow-ups. Same
        // meta guard attribution uses, so the Journal highlights stay readable.
        if is_meta_memory(&m.memory_type.to_string(), &m.tags.join(",")) {
            continue;
        }
        if matches!(m.memory_type, MemoryType::Decision)
            && decisions.len() < MAX_DECISIONS
            && !is_noise_title(&m.title)
            && !shown_event_ids.contains(m.id.as_str())
        {
            decisions.push(JournalItem {
                title: clean_line(&m.title),
                memory_id: m.id.clone(),
            });
        }
        if follow_ups.len() < MAX_FOLLOWUPS
            && is_followup(m)
            && !is_noise_title(&m.title)
            && seen_followup_ids.insert(m.id.clone())
        {
            follow_ups.push(JournalItem {
                title: clean_line(&m.title),
                memory_id: m.id.clone(),
            });
        }
    }

    let summary = deterministic_summary(
        &projects,
        &decisions,
        &follow_ups,
        display_unattributed_seconds,
    );

    Ok(JournalDay {
        day: day_str,
        summary,
        projects,
        decisions,
        follow_ups,
        unattributed_seconds: display_unattributed_seconds,
    })
}

fn empty_day(day_str: &str) -> JournalDay {
    JournalDay {
        day: day_str.to_string(),
        summary: "No work recorded for this day.".to_string(),
        projects: Vec::new(),
        decisions: Vec::new(),
        follow_ups: Vec::new(),
        unattributed_seconds: 0.0,
    }
}

fn should_fold_empty_project(seconds: f64, bullets: &[String], events: &[JournalEvent]) -> bool {
    seconds < MIN_DISPLAY_PROJECT_SECONDS && bullets.is_empty() && events.is_empty()
}

/// Auto-captured titles that carry no readable meaning in a digest. The
/// watcher records git/file churn as decisions/notes ("Conversation decision",
/// "Dependency change: package.json", "new-file: …"); they're fine as raw
/// memories but pure noise in a daily recap, so the backend strips them — the
/// UI receives an already-clean contract and never re-implements this.
pub(crate) fn is_noise_title(t: &str) -> bool {
    let low = t.trim().to_lowercase();
    if low.is_empty() {
        return true;
    }
    const NOISE_PREFIXES: &[&str] = &[
        "conversation decision",
        "dependency change",
        "dependency-change",
        "new file:",
        "new-file:",
        "new-file_",
        "deleted:",
        "deleted_",
        "new file_",
        "user correction",
    ];
    NOISE_PREFIXES.iter().any(|p| low.starts_with(p))
}

/// Pick the one project a memory should be shown under in the Journal. A memory
/// may be linked to several project entities because its body mentions related
/// work; rendering it under every linked project makes the digest unreadable.
/// We only assign multi-linked memories when the headline clearly names one
/// candidate. Ambiguous memories stay out of project event rows.
fn primary_project_for_memory(
    m: &MemoryEntry,
    candidates: &[(String, String)],
) -> Option<(String, String)> {
    match candidates {
        [] => None,
        [only] => Some(only.clone()),
        _ => {
            // Score on the RAW first line (commit prefix/scope kept): the
            // `feat(mnemonic):` scope is a strong project signal we must not
            // lose. The prefix is only stripped for display (clean_line).
            let headline = m
                .title
                .lines()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("")
                .to_lowercase();
            let mut scored: Vec<(i32, &(String, String))> = candidates
                .iter()
                .map(|candidate| (project_headline_score(&headline, &candidate.1), candidate))
                .filter(|(score, _)| *score > 0)
                .collect();
            scored.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
            match (scored.first(), scored.get(1)) {
                (Some((best, _)), Some((runner_up, _))) if best == runner_up => None,
                (Some((_, candidate)), _) => Some((*candidate).clone()),
                _ => None,
            }
        }
    }
}

fn project_headline_score(headline: &str, project_name: &str) -> i32 {
    project_aliases(project_name)
        .iter()
        .map(|alias| alias_match_score(headline, alias))
        .max()
        .unwrap_or(0)
}

fn project_aliases(project_name: &str) -> Vec<String> {
    let low = project_name.trim().to_lowercase();
    let mut aliases = vec![low.clone()];
    let spaced = low.replace(['-', '_'], " ");
    if spaced != low {
        aliases.push(spaced);
    }
    match low.as_str() {
        "mnemonic" => aliases.extend([
            "mnemonicbar".into(),
            "mnemonic widget".into(),
            "journal backend".into(),
        ]),
        "project-alpha" => aliases.extend([
            "content factory".into(),
            "rendergen".into(),
            "mediagen".into(),
            "project-gamma".into(),
            "runpod".into(),
            "fal".into(),
            "comfyui".into(),
        ]),
        "project-forge" => aliases.extend(["project-forge ai".into(), "project-forge".into()]),
        "project-zeta" => aliases.extend(["zeta service".into(), "zeta-svc".into()]),
        _ => {}
    }
    aliases.sort();
    aliases.dedup();
    aliases
}

fn alias_match_score(headline: &str, alias: &str) -> i32 {
    if alias.is_empty() || !headline.contains(alias) {
        return 0;
    }
    // Prefer exact project tokens in commit prefixes (`feat(mnemonic):`) over
    // loose aliases. Longer aliases win ties among related projects.
    let boundary_bonus = if contains_tokenish(headline, alias) {
        100
    } else {
        20
    };
    boundary_bonus + alias.chars().count() as i32
}

fn contains_tokenish(haystack: &str, needle: &str) -> bool {
    let Some(pos) = haystack.find(needle) else {
        return false;
    };
    let before = haystack[..pos].chars().next_back();
    let after = haystack[pos + needle.len()..].chars().next();
    !before.is_some_and(|c| c.is_ascii_alphanumeric())
        && !after.is_some_and(|c| c.is_ascii_alphanumeric())
}

/// True when a memory reads like a forward-looking follow-up. Decisions are
/// excluded — a decision is a thing done, not a thing still to do. Matches the
/// HEADLINE only (first line of the title): git-commit memories store the whole
/// message in `title`, so scanning the full text false-positives on a "follow-up"
/// mentioned deep in a commit body. The digest should under-claim, not over-claim.
fn is_followup(m: &crate::event::MemoryEntry) -> bool {
    if matches!(m.memory_type, MemoryType::Decision) {
        return false;
    }
    let headline = clean_line(&m.title).to_lowercase();
    FOLLOWUP_MARKERS.iter().any(|w| headline.contains(w))
}

/// Local clock label for a UTC timestamp, e.g. "7:38pm" — for Journal events.
fn time_label(ts: chrono::DateTime<chrono::Utc>) -> String {
    ts.with_timezone(&chrono::Local)
        .format("%-I:%M%p")
        .to_string()
        .to_lowercase()
}

/// Strip a Conventional-Commits prefix (`feat(mnemonic): `, `fix: `, …) so a
/// Journal row reads as a plain action ("Journal backend v1") instead of a raw
/// commit subject ("feat(mnemonic): Journal backend v1"). Leaves non-commit
/// text untouched.
fn strip_commit_prefix(s: &str) -> &str {
    const TYPES: &[&str] = &[
        "feat", "fix", "docs", "style", "refactor", "perf", "test", "chore", "build", "ci",
        "revert",
    ];
    let lower = s.to_ascii_lowercase();
    for t in TYPES {
        if let Some(rest) = lower.strip_prefix(t) {
            // optional "(scope)"
            let after_scope_len = if rest.starts_with('(') {
                match rest.find(')') {
                    Some(close) => close + 1,
                    None => continue,
                }
            } else {
                0
            };
            let tail = &s[t.len() + after_scope_len..];
            if let Some(body) = tail.strip_prefix(':') {
                let trimmed = body.trim_start();
                if !trimmed.is_empty() {
                    return trimmed;
                }
            }
        }
    }
    s
}

/// First non-empty line, commit-prefix stripped, whitespace-collapsed and
/// length-capped — commit messages and multi-line notes become one tidy row.
fn clean_line(s: &str) -> String {
    let first = s.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    let first = strip_commit_prefix(first.trim());
    let collapsed = first.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() > MAX_LINE_CHARS {
        let truncated: String = collapsed.chars().take(MAX_LINE_CHARS - 1).collect();
        format!("{}…", truncated.trim_end())
    } else {
        collapsed
    }
}

/// Honest one-line recap with no model: counts + the headline project.
fn deterministic_summary(
    projects: &[JournalProject],
    decisions: &[JournalItem],
    follow_ups: &[JournalItem],
    unattributed: f64,
) -> String {
    let linked_total: f64 = projects.iter().map(|p| p.seconds).sum();
    let has_unattributed = unattributed > 0.5;
    if projects.is_empty() && decisions.is_empty() && follow_ups.is_empty() && !has_unattributed {
        return "No work recorded for this day.".to_string();
    }
    let mut parts: Vec<String> = Vec::new();
    if linked_total > 0.0 {
        let lead = &projects[0];
        if projects.len() == 1 {
            parts.push(format!("{} on {}", fmt_dur(linked_total), lead.name));
        } else {
            parts.push(format!(
                "{} across {} projects, mostly {} ({})",
                fmt_dur(linked_total),
                projects.len(),
                lead.name,
                fmt_dur(lead.seconds),
            ));
        }
    } else if has_unattributed {
        parts.push(format!(
            "{} worked, not linked to a project",
            fmt_dur(unattributed)
        ));
    }
    if !decisions.is_empty() {
        parts.push(format!(
            "{} decision{}",
            decisions.len(),
            plural(decisions.len())
        ));
    }
    if !follow_ups.is_empty() {
        parts.push(format!(
            "{} follow-up{}",
            follow_ups.len(),
            plural(follow_ups.len())
        ));
    }
    if has_unattributed
        && linked_total > 0.0
        && unattributed >= 0.25 * (linked_total + unattributed)
    {
        parts.push(format!("{} not linked to a project", fmt_dur(unattributed)));
    }
    let mut s = parts.join(" · ");
    if let Some(first) = s.get_mut(0..1) {
        first.make_ascii_uppercase();
    }
    s.push('.');
    s
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// Compact `Hh Mm` / `Mm` duration for summary text.
fn fmt_dur(seconds: f64) -> String {
    let mins = (seconds / 60.0).round() as i64;
    let h = mins / 60;
    let m = mins % 60;
    if h > 0 {
        format!("{h}h {m}m")
    } else {
        format!("{m}m")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::EventSource;

    fn proj(name: &str, secs: f64) -> JournalProject {
        JournalProject {
            key: name.into(),
            name: name.into(),
            seconds: secs,
            confidence: Some("medium".into()),
            bullets: vec![],
            events: vec![],
        }
    }
    fn item(t: &str) -> JournalItem {
        JournalItem {
            title: t.into(),
            memory_id: "m".into(),
        }
    }
    fn mem(title: &str) -> MemoryEntry {
        MemoryEntry {
            id: title.into(),
            timestamp: chrono::DateTime::parse_from_rfc3339("2026-06-01T12:00:00Z")
                .unwrap()
                .with_timezone(&chrono::Utc),
            title: title.into(),
            content: String::new(),
            memory_type: MemoryType::Note,
            tags: Vec::new(),
            source: EventSource::Manual,
            importance: 0.5,
            metadata: serde_json::Value::Null,
        }
    }

    #[test]
    fn clean_line_takes_first_line_and_caps() {
        assert_eq!(clean_line("hello   world\nsecond"), "hello world");
        assert_eq!(clean_line("\n\n  real  "), "real");
        let long = "x".repeat(200);
        let out = clean_line(&long);
        assert!(out.chars().count() <= MAX_LINE_CHARS);
        assert!(out.ends_with('…'));
    }

    #[test]
    fn strips_conventional_commit_prefixes() {
        assert_eq!(
            clean_line("feat(mnemonic): Journal backend v1"),
            "Journal backend v1"
        );
        assert_eq!(clean_line("fix: de-flake test"), "de-flake test");
        assert_eq!(clean_line("docs(x): y"), "y");
        // Non-commit text and prose with a colon are left intact.
        assert_eq!(
            clean_line("Rule: end-of-session project log"),
            "Rule: end-of-session project log"
        );
        assert_eq!(
            clean_line("project-gamma design notes"),
            "project-gamma design notes"
        );
    }

    #[test]
    fn time_label_is_local_lowercase_clock() {
        let ts = chrono::DateTime::parse_from_rfc3339("2026-05-31T19:38:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let lbl = time_label(ts);
        // Format check (local tz varies): "H:MMam"/"H:MMpm", lowercase, no space.
        assert!(lbl.ends_with("am") || lbl.ends_with("pm"), "{lbl}");
        assert!(lbl.contains(':') && !lbl.contains(' '), "{lbl}");
    }

    #[test]
    fn noise_titles_are_filtered() {
        assert!(is_noise_title("Conversation decision"));
        assert!(is_noise_title("Dependency change: package.json"));
        assert!(is_noise_title("new-file_ required-server-files_json"));
        assert!(is_noise_title("Deleted: lock"));
        assert!(is_noise_title("User correction"));
        assert!(is_noise_title("   "));
        // Real work titles survive.
        assert!(!is_noise_title(
            "fix(mnemonic): honest project-time attribution"
        ));
        assert!(!is_noise_title("project-gamma feature recipe shipped"));
    }

    #[test]
    fn meta_memories_excluded_from_highlights() {
        // The journal loop skips any memory `is_meta_memory` flags, so
        // conversation-watcher chatter and user corrections never surface as
        // decisions/follow-ups. These tag/type shapes are exactly what the live
        // DB carried for junk lines (a casual "looks good, keep it" decision and
        // a "no, redo that part" correction).
        let mut junk_decision = mem("ок, выглядит норм, оставляем");
        junk_decision.memory_type = MemoryType::Decision;
        junk_decision.tags = vec!["decision".into(), "conversation".into()];
        assert!(is_meta_memory(
            &junk_decision.memory_type.to_string(),
            &junk_decision.tags.join(",")
        ));

        let mut correction = mem("нет, переделай этот кусок");
        correction.memory_type = MemoryType::Feedback;
        correction.tags = vec!["feedback".into(), "correction".into()];
        assert!(is_meta_memory(
            &correction.memory_type.to_string(),
            &correction.tags.join(",")
        ));

        // A real Socket-sourced session-log decision must survive the filter.
        let mut real = mem("project-alpha — layout map + scene sort");
        real.memory_type = MemoryType::Decision;
        real.tags = vec![
            "project-alpha".into(),
            "project-gamma".into(),
            "session-log".into(),
        ];
        assert!(!is_meta_memory(
            &real.memory_type.to_string(),
            &real.tags.join(",")
        ));
    }

    #[test]
    fn primary_project_prefers_explicit_headline_hit() {
        let m = mem("feat(mnemonic): semantic attribution as advisory dry-run");
        let picked = primary_project_for_memory(
            &m,
            &[
                ("mnemonic-id".into(), "mnemonic".into()),
                ("content-id".into(), "project-alpha".into()),
                ("zeta-id".into(), "project-zeta".into()),
            ],
        );
        assert_eq!(picked.unwrap().0, "mnemonic-id");
    }

    #[test]
    fn primary_project_uses_domain_aliases() {
        let m = mem("project-gamma video recipe solved");
        let picked = primary_project_for_memory(
            &m,
            &[
                ("mnemonic-id".into(), "mnemonic".into()),
                ("content-id".into(), "project-alpha".into()),
            ],
        );
        assert_eq!(picked.unwrap().0, "content-id");
    }

    #[test]
    fn primary_project_drops_ambiguous_multi_project_memory() {
        let m = mem("semantic attribution dry-run cleanup");
        let picked = primary_project_for_memory(
            &m,
            &[
                ("mnemonic-id".into(), "mnemonic".into()),
                ("content-id".into(), "project-alpha".into()),
            ],
        );
        assert!(picked.is_none());
    }

    #[test]
    fn tiny_empty_projects_fold_out_of_journal_rows() {
        assert!(should_fold_empty_project(599.0, &[], &[]));
        assert!(!should_fold_empty_project(600.0, &[], &[]));
        assert!(!should_fold_empty_project(
            120.0,
            &["real event".into()],
            &[]
        ));
    }

    #[test]
    fn summary_is_empty_message_when_nothing() {
        let s = deterministic_summary(&[], &[], &[], 0.0);
        assert_eq!(s, "No work recorded for this day.");
    }

    #[test]
    fn summary_reports_unattributed_only_work() {
        let s = deterministic_summary(&[], &[], &[], 7200.0);
        assert_eq!(s, "2h 0m worked, not linked to a project.");
    }

    #[test]
    fn summary_counts_projects_decisions_followups() {
        let s = deterministic_summary(
            &[proj("mnemonic", 3600.0), proj("project-eta", 1800.0)],
            &[item("decided X")],
            &[item("todo Y")],
            0.0,
        );
        assert!(s.contains("mnemonic"), "{s}");
        assert!(s.contains("2 projects"), "{s}");
        assert!(s.contains("1 decision"), "{s}");
        assert!(s.contains("1 follow-up"), "{s}");
        assert!(s.ends_with('.'), "{s}");
    }

    #[test]
    fn summary_flags_high_unattributed() {
        // 30m project, 30m unattributed → >= 25% share, should be flagged.
        let s = deterministic_summary(&[proj("mnemonic", 1800.0)], &[], &[], 1800.0);
        assert!(s.contains("not linked to a project"), "{s}");
    }

    #[test]
    fn summary_hides_low_unattributed() {
        // 60m project, 5m unattributed → below 25%, not mentioned.
        let s = deterministic_summary(&[proj("mnemonic", 3600.0)], &[], &[], 300.0);
        assert!(!s.contains("not linked"), "{s}");
    }
}
