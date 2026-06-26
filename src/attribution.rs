//! Project-time attribution — deciding which project a work session's time
//! belongs to, honestly and probabilistically.
//!
//! The signal: memories (git commits, file changes, conversation turns) are
//! already linked to project-type graph entities by the extractor. So for a
//! given work session we look at the project entities that "lit up" in/around
//! its window and split the session's seconds across them by signal weight.
//!
//! Honesty rules (User + Codex were explicit — no fake precision):
//! - If there's no project signal, the time is **Unattributed**, not guessed.
//! - Confidence is reported (High / Medium / Low) and never hidden.
//! - Thin signal (one stray memory) still attributes, but at Low confidence.
//! - Multiple projects in one window split the time proportionally rather
//!   than forcing a single winner.
//!
//! This module is the pure decision core — no IO, fully unit-tested. The
//! daemon job feeds it `ProjectSignal`s gathered from the memory graph and
//! persists the resulting allocations.

/// Reported trust in an allocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confidence {
    High,
    Medium,
    Low,
}

impl Confidence {
    pub fn as_str(self) -> &'static str {
        match self {
            Confidence::High => "high",
            Confidence::Medium => "medium",
            Confidence::Low => "low",
        }
    }
}

/// One project's signal strength in a session's window — typically the count
/// (or weighted count) of project-linked memories that fell in the window.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectSignal {
    pub project_key: String,
    pub weight: f64,
}

/// Seconds attributed to one project, with the confidence of that call.
#[derive(Debug, Clone, PartialEq)]
pub struct Allocation {
    pub project_key: String,
    pub seconds: f64,
    pub confidence: Confidence,
}

/// The full result for one session.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionAttribution {
    pub allocations: Vec<Allocation>,
    /// Seconds we couldn't honestly assign to any project.
    pub unattributed_seconds: f64,
}

/// Tunables for the attribution decision.
#[derive(Debug, Clone)]
pub struct AttribCfg {
    /// A single project holding at least this share of the signal wins the
    /// whole session (rather than splitting). 0.70 = "clearly one project".
    pub dominance: f64,
    /// Below this total weight there's basically no signal → Unattributed.
    pub min_weight: f64,
    /// At/above this total weight the signal is "solid" (promotes confidence).
    pub solid_weight: f64,
    /// Shares below this are folded into Unattributed instead of producing a
    /// sliver allocation (avoids noisy 2%-of-a-session entries).
    pub min_share: f64,
}

impl Default for AttribCfg {
    fn default() -> Self {
        Self {
            dominance: 0.70,
            min_weight: 1.0,
            solid_weight: 3.0,
            min_share: 0.10,
        }
    }
}

/// Pure attribution decision for one session.
///
/// `session_seconds` is the worked duration; `signals` are the per-project
/// weights observed in the session's window.
pub fn attribute_session(
    session_seconds: f64,
    signals: &[ProjectSignal],
    cfg: &AttribCfg,
) -> SessionAttribution {
    let secs = session_seconds.max(0.0);
    let total: f64 = signals.iter().map(|s| s.weight.max(0.0)).sum();

    // No (or negligible) signal → don't guess.
    if signals.is_empty() || total < cfg.min_weight || secs <= 0.0 {
        return SessionAttribution {
            allocations: vec![],
            unattributed_seconds: secs,
        };
    }

    // Rank projects by weight, drop sliver shares into the unattributed bucket.
    let mut ranked: Vec<(&ProjectSignal, f64)> = signals
        .iter()
        .map(|s| (s, s.weight.max(0.0) / total))
        .filter(|(_, share)| *share > 0.0)
        .collect();
    ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    let top_share = ranked.first().map(|(_, sh)| *sh).unwrap_or(0.0);
    let solid = total >= cfg.solid_weight;

    // Dominant single project → assign everything to it.
    if top_share >= cfg.dominance {
        let (sig, _) = ranked[0];
        return SessionAttribution {
            allocations: vec![Allocation {
                project_key: sig.project_key.clone(),
                seconds: secs,
                confidence: if solid {
                    Confidence::High
                } else {
                    Confidence::Low
                },
            }],
            unattributed_seconds: 0.0,
        };
    }

    // Split across the meaningful projects; sliver shares → unattributed.
    let kept: Vec<(&ProjectSignal, f64)> = ranked
        .iter()
        .filter(|(_, sh)| *sh >= cfg.min_share)
        .cloned()
        .collect();
    let kept_weight: f64 = kept.iter().map(|(_, sh)| *sh).sum();

    if kept.is_empty() || kept_weight <= 0.0 {
        return SessionAttribution {
            allocations: vec![],
            unattributed_seconds: secs,
        };
    }

    // Confidence for a split: solid signal → Medium, thin → Low.
    let conf = if solid {
        Confidence::Medium
    } else {
        Confidence::Low
    };
    let mut allocations = Vec::with_capacity(kept.len());
    let mut assigned = 0.0;
    for (sig, share) in &kept {
        // Use the ORIGINAL share (no renormalization): each project gets only
        // the time its signal earned. Dropped slivers therefore become
        // unattributed rather than inflating the kept projects.
        let alloc = secs * share;
        assigned += alloc;
        allocations.push(Allocation {
            project_key: sig.project_key.clone(),
            seconds: alloc,
            confidence: conf,
        });
    }
    let unattributed = (secs - assigned).max(0.0);

    SessionAttribution {
        allocations,
        unattributed_seconds: unattributed,
    }
}

// ─────────────────────────── day-level carry-forward ───────────────────────────
//
// Per-session attribution under-counts cloud work: a project-alpha day is a
// dozen idle-split work sessions but only 1–3 saved memories, so only the
// sessions whose window happens to contain a memory get attributed — the rest
// fall into Unattributed even though the whole day was one project. Carry-forward
// extends a memory's signal to *neighbouring no-signal sessions*, but only under
// strict guards so it can never fabricate tidy-but-fake hours:
//
//   1. Same day only (never across days).
//   2. Only sessions with NO direct signal of their own are candidates — a
//      session that earned its own allocation is never overwritten.
//   3. Day-dominance gate: carry runs ONLY when one project holds ≥ `dominance`
//      of the day's directly-attributed seconds. Multi-project days don't carry.
//   4. Window: a no-signal session is eligible only within ±`window` of one of
//      the dominant project's memories. Far-from-any-memory sessions stay
//      Unattributed.
//   5. Carried time is always Low confidence.
//   6. Daily cap: carry adds at most `cap_fraction` of the day's worked seconds.
//      The excess stays Unattributed — so a single-memory day can never become
//      100 % attributed.
//   7. Unattributed always survives when signal is genuinely thin.

use chrono::{DateTime, Utc};

/// Tunables for day-level carry-forward. Conservative defaults — we would
/// rather leave cloud time Unattributed than invent a project for it.
#[derive(Debug, Clone)]
pub struct CarryCfg {
    /// One project must hold at least this share of the day's *directly*
    /// attributed seconds for carry-forward to run at all. Else the day is
    /// "multi-project" and we don't guess.
    pub dominance: f64,
    /// A no-signal session is eligible only within this many minutes of one of
    /// the dominant project's memories.
    pub window_minutes: i64,
    /// Carry-forward may attribute at most this fraction of the day's worked
    /// seconds. The remainder stays Unattributed (honest floor).
    pub cap_fraction: f64,
}

impl Default for CarryCfg {
    fn default() -> Self {
        Self {
            dominance: 0.75,
            window_minutes: 90,
            cap_fraction: 0.50,
        }
    }
}

/// One work session plus its already-computed direct attribution.
#[derive(Debug, Clone)]
pub struct DaySession {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub seconds: f64,
    pub direct: SessionAttribution,
}

/// A session whose time was assigned by carry-forward (not its own signal).
#[derive(Debug, Clone, PartialEq)]
pub struct CarriedSession {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub seconds: f64,
    pub project_key: String,
}

/// The day's final per-project seconds (confidence by majority-seconds, not
/// "best"), the carried sessions (for the dry-run audit), and what stayed
/// Unattributed. `day_project` is Some only when carry-forward actually ran.
#[derive(Debug, Clone, PartialEq)]
pub struct DayAttribution {
    pub per_project: Vec<(String, f64, Confidence)>,
    pub carried: Vec<CarriedSession>,
    pub unattributed_seconds: f64,
    pub day_project: Option<String>,
}

fn rank_conf(c: Confidence) -> u8 {
    match c {
        Confidence::High => 3,
        Confidence::Medium => 2,
        Confidence::Low => 1,
    }
}

/// The confidence level holding the most seconds for a project. Ties break to
/// the stronger level so an even split never under-reports. This replaces the
/// old "best confidence wins" aggregation: a project that is 4m High-direct +
/// 3h Low-carry now reads Low, because Low holds the majority of its seconds.
fn confidence_by_seconds(by_level: &[(Confidence, f64)]) -> Confidence {
    by_level
        .iter()
        .copied()
        .max_by(|a, b| {
            a.1.partial_cmp(&b.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| rank_conf(a.0).cmp(&rank_conf(b.0)))
        })
        .map(|(c, _)| c)
        .unwrap_or(Confidence::Low)
}

/// Minutes of gap between a session `[start,end]` and a memory timestamp `t`.
/// Zero when the memory falls inside the session.
fn gap_minutes(start: DateTime<Utc>, end: DateTime<Utc>, t: DateTime<Utc>) -> i64 {
    if t < start {
        (start - t).num_minutes()
    } else if t > end {
        (t - end).num_minutes()
    } else {
        0
    }
}

/// Day-level attribution: direct per-session results + carry-forward for the
/// dominant project's no-signal neighbours, under the guards above. Pure: the
/// caller supplies the sessions (with direct attribution already computed) and
/// the day's project-memory timestamps; we never touch IO.
pub fn carry_forward_day(
    sessions: &[DaySession],
    mem_times: &[(String, DateTime<Utc>)],
    cfg: &CarryCfg,
) -> DayAttribution {
    use std::collections::HashMap;

    let day_worked: f64 = sessions.iter().map(|s| s.seconds.max(0.0)).sum();

    // Direct per-project seconds, and seconds-by-confidence for the honest
    // majority-confidence call.
    let mut direct_secs: HashMap<String, f64> = HashMap::new();
    let mut by_level: HashMap<String, HashMap<u8, (Confidence, f64)>> = HashMap::new();
    let mut direct_total = 0.0;
    for s in sessions {
        for a in &s.direct.allocations {
            *direct_secs.entry(a.project_key.clone()).or_insert(0.0) += a.seconds;
            let e = by_level
                .entry(a.project_key.clone())
                .or_default()
                .entry(rank_conf(a.confidence))
                .or_insert((a.confidence, 0.0));
            e.1 += a.seconds;
            direct_total += a.seconds;
        }
    }

    // Helper: assemble the sorted output from the current maps + unattributed.
    let finish = |direct_secs: &HashMap<String, f64>,
                  by_level: &HashMap<String, HashMap<u8, (Confidence, f64)>>,
                  carried: Vec<CarriedSession>,
                  unattributed: f64,
                  day_project: Option<String>|
     -> DayAttribution {
        let mut per_project: Vec<(String, f64, Confidence)> = direct_secs
            .iter()
            .map(|(k, secs)| {
                let levels: Vec<(Confidence, f64)> = by_level
                    .get(k)
                    .map(|m| m.values().copied().collect())
                    .unwrap_or_default();
                (k.clone(), *secs, confidence_by_seconds(&levels))
            })
            .collect();
        per_project.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        DayAttribution {
            per_project,
            carried,
            unattributed_seconds: unattributed.max(0.0),
            day_project,
        }
    };

    let direct_unattributed = (day_worked - direct_total).max(0.0);

    // No signal at all → nothing to be dominant about; don't carry.
    if direct_total <= 0.0 {
        return finish(&direct_secs, &by_level, vec![], day_worked, None);
    }

    // Day-dominance gate (rule 3).
    let (dom_key, dom_secs) = direct_secs
        .iter()
        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(k, s)| (k.clone(), *s))
        .unwrap();
    if dom_secs / direct_total < cfg.dominance {
        // Multi-project day — leave no-signal sessions Unattributed.
        return finish(&direct_secs, &by_level, vec![], direct_unattributed, None);
    }

    // Carry candidates: sessions with NO allocation of their own (rule 2),
    // each tagged with its gap to the nearest dominant-project memory.
    let dom_mems: Vec<DateTime<Utc>> = mem_times
        .iter()
        .filter(|(k, _)| *k == dom_key)
        .map(|(_, t)| *t)
        .collect();
    let mut candidates: Vec<(usize, i64)> = Vec::new();
    for (i, s) in sessions.iter().enumerate() {
        if !s.direct.allocations.is_empty() {
            continue; // keeps its own signal
        }
        if let Some(gap) = dom_mems
            .iter()
            .map(|t| gap_minutes(s.start, s.end, *t))
            .min()
            .filter(|g| *g <= cfg.window_minutes)
        {
            candidates.push((i, gap)); // rule 4
        }
    }
    // Nearest-to-a-memory first, so the cap is spent on the most-confident carry.
    candidates.sort_by_key(|(_, gap)| *gap);

    let cap = cfg.cap_fraction * day_worked; // rule 6
    let mut carried_total = 0.0;
    let mut carried: Vec<CarriedSession> = Vec::new();
    for (i, _) in candidates {
        let s = &sessions[i];
        if carried_total + s.seconds > cap {
            continue; // would exceed the daily carry cap → stays Unattributed
        }
        carried_total += s.seconds;
        carried.push(CarriedSession {
            start: s.start,
            end: s.end,
            seconds: s.seconds,
            project_key: dom_key.clone(),
        });
    }

    if carried_total <= 0.0 {
        return finish(&direct_secs, &by_level, vec![], direct_unattributed, None);
    }

    // Fold carried seconds into the dominant project as Low confidence (rule 5).
    *direct_secs.entry(dom_key.clone()).or_insert(0.0) += carried_total;
    let e = by_level
        .entry(dom_key.clone())
        .or_default()
        .entry(rank_conf(Confidence::Low))
        .or_insert((Confidence::Low, 0.0));
    e.1 += carried_total;

    let unattributed = (day_worked - direct_total - carried_total).max(0.0);
    finish(
        &direct_secs,
        &by_level,
        carried,
        unattributed,
        Some(dom_key),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sig(k: &str, w: f64) -> ProjectSignal {
        ProjectSignal {
            project_key: k.into(),
            weight: w,
        }
    }

    #[test]
    fn no_signal_is_unattributed_not_guessed() {
        let r = attribute_session(3600.0, &[], &AttribCfg::default());
        assert!(r.allocations.is_empty());
        assert_eq!(r.unattributed_seconds, 3600.0);
    }

    #[test]
    fn dominant_project_takes_all_high_when_solid() {
        let r = attribute_session(
            3600.0,
            &[sig("mnemonic", 8.0), sig("project-eta", 1.0)],
            &AttribCfg::default(),
        );
        assert_eq!(r.allocations.len(), 1);
        assert_eq!(r.allocations[0].project_key, "mnemonic");
        assert!((r.allocations[0].seconds - 3600.0).abs() < 0.5);
        assert_eq!(r.allocations[0].confidence, Confidence::High);
        assert_eq!(r.unattributed_seconds, 0.0);
    }

    #[test]
    fn single_thin_memory_attributes_but_low_confidence() {
        // One memory total — dominant (100%) but not solid → Low, not High.
        let r = attribute_session(1800.0, &[sig("project-beta", 1.0)], &AttribCfg::default());
        assert_eq!(r.allocations.len(), 1);
        assert_eq!(r.allocations[0].confidence, Confidence::Low);
        assert!((r.allocations[0].seconds - 1800.0).abs() < 0.5);
    }

    #[test]
    fn two_projects_split_proportionally() {
        // 6 vs 4 of 10, neither hits 0.70 dominance → split, solid → Medium.
        let r = attribute_session(
            6000.0,
            &[sig("mnemonic", 6.0), sig("project-alpha", 4.0)],
            &AttribCfg::default(),
        );
        assert_eq!(r.allocations.len(), 2);
        let m = r
            .allocations
            .iter()
            .find(|a| a.project_key == "mnemonic")
            .unwrap();
        let c = r
            .allocations
            .iter()
            .find(|a| a.project_key == "project-alpha")
            .unwrap();
        assert!((m.seconds - 3600.0).abs() < 1.0, "mnemonic {}", m.seconds);
        assert!((c.seconds - 2400.0).abs() < 1.0, "cf {}", c.seconds);
        assert_eq!(m.confidence, Confidence::Medium);
        assert!((m.seconds + c.seconds - 6000.0).abs() < 1.0);
    }

    #[test]
    fn sliver_share_folds_into_unattributed() {
        // 7 vs 0.5 of 7.5: top share 0.933 ≥ dominance → actually dominant.
        // Use a non-dominant case with a sliver: 5, 4, 0.3 (total 9.3).
        let r = attribute_session(
            9300.0,
            &[sig("a", 5.0), sig("b", 4.0), sig("c", 0.3)],
            &AttribCfg::default(),
        );
        // c's share ≈ 0.032 < min_share 0.10 → dropped to unattributed.
        assert!(r.allocations.iter().all(|a| a.project_key != "c"));
        assert_eq!(r.allocations.len(), 2);
        // a+b kept time ≈ full session minus c's sliver.
        let assigned: f64 = r.allocations.iter().map(|a| a.seconds).sum();
        assert!(r.unattributed_seconds > 0.0);
        assert!((assigned + r.unattributed_seconds - 9300.0).abs() < 1.0);
    }

    // ─────────────────────────── carry-forward ───────────────────────────

    fn t(min: i64) -> DateTime<Utc> {
        chrono::DateTime::parse_from_rfc3339("2026-06-06T09:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
            + chrono::Duration::minutes(min)
    }
    fn no_signal(start_min: i64, dur_min: i64) -> DaySession {
        let secs = (dur_min * 60) as f64;
        DaySession {
            start: t(start_min),
            end: t(start_min + dur_min),
            seconds: secs,
            direct: SessionAttribution {
                allocations: vec![],
                unattributed_seconds: secs,
            },
        }
    }
    fn attributed(start_min: i64, dur_min: i64, proj: &str, conf: Confidence) -> DaySession {
        let secs = (dur_min * 60) as f64;
        DaySession {
            start: t(start_min),
            end: t(start_min + dur_min),
            seconds: secs,
            direct: SessionAttribution {
                allocations: vec![Allocation {
                    project_key: proj.into(),
                    seconds: secs,
                    confidence: conf,
                }],
                unattributed_seconds: 0.0,
            },
        }
    }

    #[test]
    fn carry_single_project_day_fills_neighbours_low_capped() {
        // One small direct project-alpha session at 0–10m + a memory at 5m,
        // then several no-signal sessions through the day near that memory.
        let sessions = vec![
            attributed(0, 10, "project-alpha", Confidence::Low),
            no_signal(20, 60),  // 20–80m, within 90m of the 5m memory
            no_signal(90, 60),  // 90–150m, gap 85m ≤ 90 → eligible
            no_signal(200, 60), // 200–260m, gap 195m > 90 → NOT eligible
        ];
        let mems = vec![("project-alpha".to_string(), t(5))];
        let r = carry_forward_day(&sessions, &mems, &CarryCfg::default());

        assert_eq!(r.day_project.as_deref(), Some("project-alpha"));
        // The far session (200m) is never carried.
        assert!(r.carried.iter().all(|c| c.start != t(200)));
        // Cap = 0.50 * day_worked (190m) = 95m. Direct 10m + carried ≤ 95m.
        let carried_secs: f64 = r.carried.iter().map(|c| c.seconds).sum();
        assert!(
            carried_secs <= 0.50 * (190.0 * 60.0) + 1.0,
            "{carried_secs}"
        );
        // Unattributed never vanishes.
        assert!(r.unattributed_seconds > 0.0);
    }

    #[test]
    fn carry_skipped_on_multi_project_day() {
        // mnemonic 60m + project-alpha 50m direct: neither ≥ 75% → no carry.
        let sessions = vec![
            attributed(0, 60, "mnemonic", Confidence::High),
            attributed(70, 50, "project-alpha", Confidence::High),
            no_signal(140, 60), // near a cf memory, but day is multi-project
        ];
        let mems = vec![("project-alpha".to_string(), t(150))];
        let r = carry_forward_day(&sessions, &mems, &CarryCfg::default());
        assert!(r.carried.is_empty(), "multi-project day must not carry");
        assert_eq!(r.day_project, None);
        // The no-signal 60m session stays Unattributed.
        assert!((r.unattributed_seconds - 60.0 * 60.0).abs() < 1.0);
    }

    #[test]
    fn carry_session_outside_window_stays_unattributed() {
        let sessions = vec![
            attributed(0, 20, "project-alpha", Confidence::Low),
            no_signal(300, 60), // 5h after the only memory → far outside 90m
        ];
        let mems = vec![("project-alpha".to_string(), t(5))];
        let r = carry_forward_day(&sessions, &mems, &CarryCfg::default());
        assert!(r.carried.is_empty());
        assert!((r.unattributed_seconds - 60.0 * 60.0).abs() < 1.0);
    }

    #[test]
    fn carry_drives_confidence_low_by_majority_seconds() {
        // 4m High direct + a big block of Low carry → project reads Low, NOT
        // High (the old best-confidence aggregation would have said High).
        let sessions = vec![
            attributed(0, 4, "project-alpha", Confidence::High),
            no_signal(10, 60),
            no_signal(75, 60),
        ];
        let mems = vec![("project-alpha".to_string(), t(5))];
        let r = carry_forward_day(&sessions, &mems, &CarryCfg::default());
        let (key, _secs, conf) = &r.per_project[0];
        assert_eq!(key, "project-alpha");
        assert_eq!(
            *conf,
            Confidence::Low,
            "carry-dominated project must read Low"
        );
    }

    #[test]
    fn carry_never_overwrites_a_sessions_own_signal() {
        // A session already attributed to the dominant project keeps its own
        // High allocation; it's not in the carried list.
        let sessions = vec![
            attributed(0, 30, "project-alpha", Confidence::High),
            attributed(40, 30, "project-alpha", Confidence::High),
            no_signal(80, 30),
        ];
        let mems = vec![("project-alpha".to_string(), t(50))];
        let r = carry_forward_day(&sessions, &mems, &CarryCfg::default());
        // Only the no-signal 80m session can ever be carried.
        assert!(r.carried.iter().all(|c| c.start == t(80)));
    }
}
