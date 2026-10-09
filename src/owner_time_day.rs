//! A day of the owner's time, made from the two stores (live-projects,
//! design 8): the intervals of activity of the day, split over the
//! projects live in them, the rest by the memories of the registry's
//! projects, and what is left to nobody.
//!
//! Which days are made again is a set: the days a segment, a prompt or a
//! moved clock marked, today, and the day that ended since the last pass.
//! Days before the first segment keep the rows they have.
use anyhow::Result;
use chrono::{DateTime, Duration, NaiveDate, Utc};
use std::collections::{BTreeMap, BTreeSet};

use crate::activity::ActivityStore;
use crate::attribution::AttribCfg;
use crate::owner_time::{Trust, dominant, registry_key, summarize};

#[path = "owner_time_ranges.rs"]
mod ranges;
use crate::storage::Storage;
pub use ranges::read as ranges;

#[path = "entity_archive_time.rs"]
mod archive_time;
// The private HTTP entry lives in the binary; library tests share this helper.
#[cfg_attr(not(test), allow(unused_imports))]
pub(crate) use archive_time::repair_live_after_entity_undo;

/// The wall clock may drift from the monotonic one by this much between
/// two passes before it counts as moved.
pub const CLOCK_TOLERANCE_SECS: i64 = 120;

/// What a local day is: only its bounds use the wall clock.
pub trait Calendar {
    /// The first instant of a local day and of the next.
    fn bounds(&self, day: NaiveDate) -> Option<(DateTime<Utc>, DateTime<Utc>)>;
    /// The local date an instant falls on.
    fn date_of(&self, at: DateTime<Utc>) -> NaiveDate;
}

/// The calendar of the timezone in force now.
pub struct LocalCalendar;

impl Calendar for LocalCalendar {
    fn bounds(&self, day: NaiveDate) -> Option<(DateTime<Utc>, DateTime<Utc>)> {
        crate::activity::local_day_bounds_utc(day)
    }

    fn date_of(&self, at: DateTime<Utc>) -> NaiveDate {
        at.with_timezone(&chrono::Local).date_naive()
    }
}

/// What a day came to: seconds by project key with the trust in them,
/// and the seconds given to no project.
#[derive(Debug, Clone, PartialEq)]
pub struct Day {
    pub rows: Vec<(String, f64, Trust)>,
    pub unattributed: f64,
}

/// The split of the activity between two instants, by project: seconds
/// by trust (high, medium, low), and the seconds given to no project.
type ByProject = (BTreeMap<i64, [f64; 3]>, f64);

/// Split the activity between two instants, by project. Reads both stores
/// and writes nothing.
fn compute_by_project(
    activity: &ActivityStore,
    storage: &Storage,
    (from, to): (DateTime<Utc>, DateTime<Utc>),
    cfg: &AttribCfg,
) -> Result<ByProject> {
    let split = summarize(&ranges(activity, storage, (from, to), cfg)?);
    Ok((split.projects, split.nobody))
}

/// Split the activity between two instants, as the rows a day is kept
/// in. Reads both stores and writes nothing.
pub fn compute(
    activity: &ActivityStore,
    storage: &Storage,
    bounds: (DateTime<Utc>, DateTime<Utc>),
    cfg: &AttribCfg,
) -> Result<Day> {
    let (projects, unattributed) = compute_by_project(activity, storage, bounds, cfg)?;
    let keys = storage.attribution_keys()?;
    let mut by_key: BTreeMap<String, [f64; 3]> = BTreeMap::new();
    for (project, by_trust) in &projects {
        let key = keys
            .get(project)
            .cloned()
            .unwrap_or_else(|| registry_key(*project));
        let sum = by_key.entry(key).or_default();
        for (slot, seconds) in by_trust.iter().enumerate() {
            sum[slot] += seconds;
        }
    }
    let rows = by_key
        .into_iter()
        .map(|(key, by_trust)| (key, by_trust.iter().sum::<f64>(), dominant(&by_trust)))
        .filter(|(_, seconds, _)| *seconds > 0.0)
        .collect();
    Ok(Day { rows, unattributed })
}

/// The hours of the local day of `now`, for the live state: the owner's
/// time by project with the trust in it, and the time of agents, up to
/// `now`. Reads both stores and writes nothing.
pub fn hours_of(
    activity: &ActivityStore,
    storage: &Storage,
    calendar: &dyn Calendar,
    now: DateTime<Utc>,
    cfg: &AttribCfg,
) -> Result<crate::live::Hours> {
    use crate::live::{Hours, Today};
    let today = calendar.date_of(now);
    let mut hours = Hours {
        day: Some(today),
        ..Hours::default()
    };
    let Some((from, to)) = calendar.bounds(today) else {
        return Ok(hours);
    };
    // Up to the asking, for the owner as for the agents: what a clock
    // that was set back left stamped ahead of it is not of the day so far.
    let to = to.min(now);
    let (projects, unattributed) = compute_by_project(activity, storage, (from, to), cfg)?;
    let mut owner = unattributed;
    for (project, by_trust) in &projects {
        let seconds: f64 = by_trust.iter().sum();
        owner += seconds;
        hours.projects.insert(
            *project,
            Today {
                owner_s: seconds.round() as i64,
                agent_s: 0,
                confidence: (seconds > 0.0).then(|| dominant(by_trust).as_str()),
            },
        );
    }
    hours.owner_s = owner.round() as i64;
    // The agents' total is the union over all segments: agents at work
    // on two projects at once are one stretch of time, not two.
    let (by_project, in_all) = storage.agent_time(from, to)?;
    hours.agent_s = in_all;
    for (project, seconds) in by_project {
        if let Some(project) = project {
            hours.projects.entry(project).or_default().agent_s = seconds;
        }
    }
    Ok(hours)
}

/// Make the rows of one local day again. `None` for a day the calendar
/// has no bounds of.
pub fn recompute_day(
    activity: &ActivityStore,
    storage: &Storage,
    calendar: &dyn Calendar,
    day: NaiveDate,
    cfg: &AttribCfg,
) -> Result<Option<Day>> {
    let _write = storage.attribution_write_lock()?;
    recompute_day_locked(activity, storage, calendar, day, cfg)
}

/// The caller holds Storage's attribution write guard across this whole
/// compute and activity-cache write; this inner path avoids taking it twice.
fn recompute_day_locked(
    activity: &ActivityStore,
    storage: &Storage,
    calendar: &dyn Calendar,
    day: NaiveDate,
    cfg: &AttribCfg,
) -> Result<Option<Day>> {
    let Some(bounds) = calendar.bounds(day) else {
        return Ok(None);
    };
    let made = compute(activity, storage, bounds, cfg)?;
    let rows: Vec<(&str, f64, &str)> = made
        .rows
        .iter()
        .map(|(key, seconds, trust)| (key.as_str(), *seconds, trust.as_str()))
        .collect();
    activity.write_day(day, &rows, made.unattributed)?;
    Ok(Some(made))
}

/// The local days from `first` to `last` that the new rule makes: those
/// from the day of the first segment on. Days before it keep their rows.
pub fn days_of_the_rule(
    storage: &Storage,
    calendar: &dyn Calendar,
    first: NaiveDate,
    last: NaiveDate,
) -> Result<Vec<NaiveDate>> {
    let Some(began) = storage.first_segment_at()? else {
        return Ok(Vec::new());
    };
    let from = first.max(calendar.date_of(began));
    Ok(from.iter_days().take_while(|day| *day <= last).collect())
}

/// Make again every day that is due: the marked ones, today, and the day
/// that ended since the last pass, which was `seen_today` then. The days
/// made, in order. A failure leaves the marks, and the next pass makes
/// those days again.
pub fn recompute_due(
    activity: &ActivityStore,
    storage: &Storage,
    calendar: &dyn Calendar,
    now: DateTime<Utc>,
    seen_today: &mut Option<NaiveDate>,
    cfg: &AttribCfg,
) -> Result<Vec<NaiveDate>> {
    let today = calendar.date_of(now);
    let marks = storage.attribution_dirty()?;
    // A day of UTC lies on at most two local dates.
    let dates_of = |start: DateTime<Utc>| {
        [
            calendar.date_of(start),
            calendar.date_of(start + Duration::seconds(86_399)),
        ]
    };
    let mut due: BTreeSet<NaiveDate> = BTreeSet::from([today]);
    for (start, _) in &marks {
        due.extend(dates_of(*start));
    }
    // Yesterday is finished after midnight, without a new record. A
    // daemon that starts does not know what its last pass saw: it
    // finishes yesterday too.
    due.extend(seen_today.or(today.pred_opt()));
    let began = storage
        .first_segment_at()?
        .map(|first| calendar.date_of(first));
    let made: Vec<NaiveDate> = due
        .into_iter()
        .filter(|day| began.is_some_and(|began| *day >= began) && *day <= today)
        .collect();
    for day in &made {
        recompute_day(activity, storage, calendar, *day, cfg)?;
    }
    // A mark is done with unless a date of it is still to come: a record
    // stamped by a clock that was ahead is made when its day comes.
    let done: Vec<(DateTime<Utc>, i64)> = marks
        .into_iter()
        .filter(|(start, _)| began.is_none() || dates_of(*start).iter().all(|day| *day <= today))
        .collect();
    storage.attribution_dirty_clear(&done)?;
    *seen_today = Some(today);
    Ok(made)
}

/// The span of time whose days are made again when the wall clock did not
/// move as the monotonic one did since the last pass: a clock that was
/// set, back or forward, or a sleep. `None` when the two agree.
pub fn clock_moved(
    wall_before: DateTime<Utc>,
    elapsed: std::time::Duration,
    wall_now: DateTime<Utc>,
) -> Option<(DateTime<Utc>, DateTime<Utc>)> {
    let expected = wall_before + Duration::from_std(elapsed).ok()?;
    let drift = (wall_now - expected).num_seconds().abs();
    (drift > CLOCK_TOLERANCE_SECS).then(|| {
        (
            wall_before.min(expected).min(wall_now),
            wall_before.max(expected).max(wall_now),
        )
    })
}

#[cfg(test)]
#[path = "owner_time_day_tests.rs"]
mod tests;
