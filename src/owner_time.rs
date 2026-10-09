//! The owner's time by project (live-projects, design 8). Pure.
//!
//! The owner is at the Mac in intervals of activity. Each instant of them
//! goes to the projects live in it, among the transcripts the owner
//! drives, by the first tier that names a project:
//!
//! - A: transcripts with a prompt of the owner in the last 15 minutes;
//! - B: transcripts with a record in that minute or the two before;
//! - C: transcripts inside a segment.
//!
//! The instant is split equally over the projects of the tier, not over
//! its sessions: two sessions on one project are one share. What no tier
//! covers is given back as intervals, for the fallback of the caller.
//!
//! The trust in a share: high in tier A when a record named the project
//! in the last 15 minutes; medium in tier A otherwise, in tier B, and in
//! tier C when a record named it in the last hour; low for the rest.
//!
//! The split is an estimate, and every surface that shows it says so.
use std::collections::BTreeMap;

#[path = "owner_time_trace.rs"]
mod trace;
pub use trace::{Range, Share, summarize, trace};

pub type ProjectId = i64;

/// A prompt of the owner keeps its transcript in tier A this long.
pub const PROMPT_SECS: i64 = 900;
/// A record keeps its transcript in tier B for its minute and two more.
pub const RECORD_MINUTES: i64 = 3;
/// A touch this old or younger is high confidence in tier A.
pub const FRESH_TOUCH_SECS: i64 = 900;
/// A touch this old or younger is medium confidence in any tier.
pub const KNOWN_TOUCH_SECS: i64 = 3600;

/// The key the time of a project is written under while no entity of the
/// graph carries its name: its place in the registry.
pub fn registry_key(project: ProjectId) -> String {
    format!("project:{project}")
}

/// The place in the registry a key names, when it is such a key.
pub fn registry_id(key: &str) -> Option<ProjectId> {
    key.strip_prefix("project:")?.parse().ok()
}

/// The trust in a share: how the project of the instant was known.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Trust {
    High,
    Medium,
    Low,
}

impl Trust {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
        }
    }

    fn slot(self) -> usize {
        match self {
            Self::High => 0,
            Self::Medium => 1,
            Self::Low => 2,
        }
    }
}

/// A segment of a transcript, in seconds since the epoch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub project: Option<ProjectId>,
    pub started: i64,
    pub last: i64,
}

/// A range of time the owner gave a project, or none: `[from, to)` in
/// seconds. Of two that cover one instant the later made (the greater
/// `order`) holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Relabel {
    pub from: i64,
    pub to: i64,
    pub project: Option<ProjectId>,
    pub order: i64,
}

/// The one of `relabels` that holds at `second`.
fn relabel_at(relabels: &[Relabel], second: i64) -> Option<&Relabel> {
    relabels
        .iter()
        .filter(|relabel| relabel.from <= second && second < relabel.to)
        .max_by_key(|relabel| relabel.order)
}

/// What is known of one transcript the owner drives, near a day. Every
/// list is in the order of time.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Transcript {
    pub spans: Vec<Span>,
    /// The project the owner pinned the transcript to (`Some(None)`: to
    /// no project).
    pub pin: Option<Option<ProjectId>>,
    /// The ranges of its time the owner relabelled. A range wins over
    /// the pin.
    pub relabels: Vec<Relabel>,
    /// The seconds of the owner's prompts.
    pub prompts: Vec<i64>,
    /// The minutes (since the epoch) that hold a record.
    pub minutes: Vec<i64>,
    /// The minutes that hold a record naming a project, with that project.
    pub touches: Vec<(i64, ProjectId)>,
}

impl Transcript {
    /// Put the lists in the order the reads below rely on.
    pub fn sorted(mut self) -> Self {
        self.spans.sort_by_key(|span| (span.started, span.last));
        self.prompts.sort_unstable();
        self.minutes.sort_unstable();
        self.minutes.dedup();
        self.touches.sort_unstable();
        self
    }

    /// The segment in force at `second`: the last one that began by then.
    fn span_at(&self, second: i64) -> Option<&Span> {
        let after = self.spans.partition_point(|span| span.started <= second);
        after.checked_sub(1).map(|at| &self.spans[at])
    }

    fn prompted(&self, second: i64) -> bool {
        let after = self.prompts.partition_point(|at| *at <= second);
        after > 0 && self.prompts[after - 1] > second - PROMPT_SECS
    }

    fn recorded(&self, minute: i64) -> bool {
        let after = self.minutes.partition_point(|at| *at <= minute);
        after > 0 && self.minutes[after - 1] > minute - RECORD_MINUTES
    }

    /// How old, at `second`, the last record is that named `project`.
    fn touch_age(&self, second: i64, project: ProjectId) -> Option<i64> {
        let minute = second.div_euclid(60);
        let after = self.touches.partition_point(|(at, _)| *at <= minute);
        self.touches[..after]
            .iter()
            .rev()
            .take_while(|(at, _)| second - at * 60 <= KNOWN_TOUCH_SECS)
            .find(|(_, named)| *named == project)
            .map(|(at, _)| (second - at * 60).max(0))
    }

    /// Every instant, in milliseconds, at which what this transcript says
    /// of a moment can change.
    fn edges(&self, out: &mut Vec<i64>) {
        for span in &self.spans {
            out.extend([span.started * 1000, span.last * 1000]);
        }
        for relabel in &self.relabels {
            out.extend([relabel.from * 1000, relabel.to * 1000]);
        }
        for at in &self.prompts {
            out.extend([at * 1000, (at + PROMPT_SECS) * 1000]);
        }
        for minute in &self.minutes {
            out.extend([minute * 60_000, (minute + RECORD_MINUTES) * 60_000]);
        }
        for (minute, _) in &self.touches {
            let at = minute * 60_000;
            out.extend([
                at,
                at + FRESH_TOUCH_SECS * 1000 + 1000,
                at + KNOWN_TOUCH_SECS * 1000 + 1000,
            ]);
        }
    }
}

/// The owner's time of some intervals, by project.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Split {
    /// Seconds by project and trust: high, medium, low.
    pub projects: BTreeMap<ProjectId, [f64; 3]>,
    /// The intervals no live transcript named a project for, in
    /// milliseconds, in order and not touching.
    pub uncovered: Vec<(i64, i64)>,
    /// The seconds the owner gave to no project: they go to nobody, and
    /// no fallback reads them.
    pub nobody: f64,
}

impl Split {
    pub fn give(&mut self, project: ProjectId, trust: Trust, seconds: f64) {
        self.projects.entry(project).or_default()[trust.slot()] += seconds;
    }

    /// The seconds given to projects.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn given(&self) -> f64 {
        self.projects.values().flatten().sum()
    }
}

/// The trust that holds most of some seconds kept by trust (high, medium,
/// low); a tie goes to the stronger.
pub fn dominant(by_trust: &[f64; 3]) -> Trust {
    let mut trust = Trust::High;
    for candidate in [Trust::Medium, Trust::Low] {
        if by_trust[candidate.slot()] > by_trust[trust.slot()] {
            trust = candidate;
        }
    }
    trust
}

/// The projects live at one instant, each with the trust in it, and the
/// order of the last range the owner relabelled a transcript by there.
fn live(transcripts: &[Transcript], at_ms: i64) -> (BTreeMap<ProjectId, Trust>, Option<i64>) {
    let second = at_ms.div_euclid(1000);
    let minute = second.div_euclid(60);
    // The owner's word for a project is as sure as anything is: an age
    // no touch has.
    const SAID: Option<i64> = Some(-1);
    let mut relabelled = None;
    // By tier: the youngest touch of each project among its transcripts.
    let mut tiers: [BTreeMap<ProjectId, Option<i64>>; 3] = Default::default();
    for transcript in transcripts {
        let Some(span) = transcript.span_at(second) else {
            continue;
        };
        let tier = if transcript.prompted(second) {
            0
        } else if transcript.recorded(minute) {
            1
        } else if second < span.last {
            2
        } else {
            continue;
        };
        // What the owner said of the transcript comes after the rules: a
        // range first, then a pin, then the project of the segment.
        let range = relabel_at(&transcript.relabels, second);
        let said = range.map(|range| range.project).or(transcript.pin);
        let Some(project) = said.unwrap_or(span.project) else {
            continue;
        };
        // Only a transcript that is live, and names a project, has a say
        // against a range of the owner's own time.
        relabelled = relabelled.max(range.map(|range| range.order));
        let age = match said {
            Some(_) => SAID,
            None => transcript.touch_age(second, project),
        };
        let known = tiers[tier].entry(project).or_insert(age);
        *known = match (*known, age) {
            (Some(known), Some(age)) => Some(known.min(age)),
            (known, age) => known.or(age),
        };
    }
    let Some((tier, projects)) = tiers
        .into_iter()
        .enumerate()
        .find(|(_, projects)| !projects.is_empty())
    else {
        return (BTreeMap::new(), relabelled);
    };
    let projects = projects
        .into_iter()
        .map(|(project, age)| {
            // A session the owner typed into is at least as sure as one
            // an agent wrote a record in.
            let trust = match (tier, age) {
                (_, SAID) => Trust::High,
                (0, Some(age)) if age <= FRESH_TOUCH_SECS => Trust::High,
                (0 | 1, _) => Trust::Medium,
                (_, Some(age)) if age <= KNOWN_TOUCH_SECS => Trust::Medium,
                _ => Trust::Low,
            };
            (project, trust)
        })
        .collect();
    (projects, relabelled)
}

/// Split the intervals of the owner's activity, `[from, to)` in
/// milliseconds since the epoch, over the projects live in them. `owner`
/// holds the ranges of the owner's own time that were given a project, or
/// none, by hand: such a range decides its instants, unless a range made
/// later relabels a transcript there.
// Public library compatibility: daemon reads now use resolved ranges directly.
#[cfg_attr(not(test), allow(dead_code))]
pub fn split(activity: &[(i64, i64)], transcripts: &[Transcript], owner: &[Relabel]) -> Split {
    summarize(&trace(activity, transcripts, owner))
}

#[cfg(test)]
#[path = "owner_time_tests.rs"]
mod tests;
