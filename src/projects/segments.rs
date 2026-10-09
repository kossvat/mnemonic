//! The segments of a transcript: runs of its time on one project
//! (live-projects, design 7.3 and 7.4).
//!
//! The records of a transcript are walked in the order of their file. The
//! time between two records belongs to the state before the later one. A
//! pause as long as the idle time ends a stretch, as it ends a stretch of
//! work. The unknown head of a stretch takes the project of the first
//! signal in that stretch, marked as backfilled; a stretch no signal named
//! stays unknown.
use chrono::{DateTime, Duration, Utc};

use super::facts::Observed;
use super::lookup::{Merges, ProjectId};
use super::resolver::{Folder, Outcome, Source, State, step_merged};

/// How a segment's project was learned: by a rule of the resolver, or
/// from the first signal after it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Learned {
    By(Source),
    Backfill,
}

/// A run of a transcript's time on one project, or on none known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    /// The row this segment is kept in, when it was resumed from the store.
    pub stored: Option<i64>,
    pub project: Option<ProjectId>,
    pub learned: Learned,
    pub started_at: DateTime<Utc>,
    pub last_at: DateTime<Utc>,
    /// The last record that named the segment's project.
    pub last_touch_at: Option<DateTime<Utc>>,
    /// Per minute walked now, in the order of time: the minute (minutes
    /// since the epoch), the records observed in it, and how many of them
    /// named the project.
    pub activity: Vec<(i64, u32, u32)>,
}

impl Segment {
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn seconds(&self) -> i64 {
        (self.last_at - self.started_at).num_seconds().max(0)
    }

    fn observe(&mut self, at: DateTime<Utc>, touch: bool) {
        let minute = at.timestamp().div_euclid(60);
        match self.activity.last_mut() {
            Some((last, records, touches)) if *last == minute => {
                *records += 1;
                *touches += u32::from(touch);
            }
            _ => self.activity.push((minute, 1, u32::from(touch))),
        }
        if touch {
            self.last_touch_at = Some(at);
        }
    }
}

/// What the walk counted, besides time.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    pub records: usize,
    /// Calls of a tool that named exactly one project.
    pub naming: usize,
    /// Calls that named two or more.
    pub ambiguous: usize,
    pub switches: usize,
}

/// Walks the records of one transcript into segments.
pub struct Walker {
    idle: Duration,
    merges: Merges,
    state: State,
    /// The segments of the stretch in progress, the last one still open.
    stretch: Vec<Segment>,
    done: Vec<Segment>,
    pub counts: Counts,
}

impl Walker {
    pub fn new(idle_secs: u64) -> Self {
        Self::resume(idle_secs, State::default(), None)
    }

    /// Go on where an earlier walk stopped: with the state of the rules it
    /// ended in and the last segment it left, which the next record
    /// extends when it comes before the idle time is over.
    pub fn resume(idle_secs: u64, state: State, open: Option<Segment>) -> Self {
        Self {
            idle: Duration::seconds(idle_secs as i64),
            merges: Merges::default(),
            state,
            stretch: open.into_iter().collect(),
            done: Vec::new(),
            counts: Counts::default(),
        }
    }

    /// Walk with what the owner merged: two merged folders are one
    /// project to the rules.
    pub fn merged(mut self, merges: Merges) -> Self {
        self.merges = merges;
        self
    }

    /// The state of the rules after the records walked so far: what the
    /// observation pass keeps in its checkpoint.
    pub fn state(&self) -> State {
        self.state
    }

    /// Close the stretch in progress: its unknown head takes the project
    /// of the first signal in it.
    fn close(&mut self) {
        if let Some(first) = self.stretch.iter().find_map(|segment| segment.project) {
            for segment in self.stretch.iter_mut().take_while(|s| s.project.is_none()) {
                segment.project = Some(first);
                segment.learned = Learned::Backfill;
            }
        }
        self.done.append(&mut self.stretch);
    }

    /// Walk one record.
    pub fn record(&mut self, observed: &Observed, pin: Option<ProjectId>) {
        let (next, outcome) = step_merged(self.state, pin, &observed.facts, &self.merges);
        self.counts.records += 1;
        let named = match observed.facts.call {
            Some((_, super::lookup::Named::One(id))) => {
                self.counts.naming += 1;
                Some(id)
            }
            Some((_, super::lookup::Named::Many)) => {
                self.counts.ambiguous += 1;
                None
            }
            _ => None,
        };
        self.counts.switches += usize::from(outcome == Outcome::Switched);
        self.state = next;
        let Some(at) = observed.at else {
            return;
        };
        match self.stretch.last_mut() {
            // A record written before the last one (a copy made by a
            // compaction) moves no time.
            Some(open) if at < open.last_at => return,
            Some(open) if at - open.last_at >= self.idle => {
                self.close();
                self.open(at);
            }
            // The time up to this record belongs to the state before it.
            Some(open) => {
                open.last_at = at;
                if open.project != next.project {
                    self.open(at);
                }
            }
            None => self.open(at),
        }
        // The record itself is counted where it stands: in the segment it
        // opens or goes on in. It touches the project when it names it: by
        // a path, by the folder it says the session is in, or by the
        // folder a call of the shell says it runs in.
        let folder = |folder: Folder| match folder {
            Folder::Project(id) => Some(id),
            _ => None,
        };
        let names = [
            named,
            folder(observed.facts.folder),
            folder(observed.facts.shell_dir),
        ];
        if let Some(open) = self.stretch.last_mut() {
            let touch = open.project.is_some_and(|project| {
                names
                    .iter()
                    .flatten()
                    .any(|id| self.merges.same(*id, project))
            });
            open.observe(at, touch);
        }
    }

    fn open(&mut self, at: DateTime<Utc>) {
        self.stretch.push(Segment {
            stored: None,
            project: self.state.project,
            learned: Learned::By(self.state.source),
            started_at: at,
            last_at: at,
            last_touch_at: None,
            activity: Vec::new(),
        });
    }

    /// The segments of the transcript, in order.
    pub fn finish(mut self) -> (Vec<Segment>, Counts) {
        self.close();
        (self.done, self.counts)
    }
}

/// The seconds a set of intervals covers between `from` and `to`, counted
/// once where they overlap: two sessions on one project at the same time
/// are one stretch of an agent's time, and so is a copy of records made by
/// a compaction.
pub fn union_seconds(
    intervals: &[(DateTime<Utc>, DateTime<Utc>)],
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> i64 {
    let mut cut: Vec<(DateTime<Utc>, DateTime<Utc>)> = intervals
        .iter()
        .map(|(start, end)| ((*start).max(from), (*end).min(to)))
        .filter(|(start, end)| end > start)
        .collect();
    cut.sort();
    let mut total = 0;
    let mut reach: Option<DateTime<Utc>> = None;
    for (start, end) in cut {
        let begin = reach.map_or(start, |reach| start.max(reach));
        if end > begin {
            total += (end - begin).num_seconds();
        }
        reach = Some(reach.map_or(end, |reach| reach.max(end)));
    }
    total
}

#[cfg(test)]
#[path = "segments_tests.rs"]
mod tests;
