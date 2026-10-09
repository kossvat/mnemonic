//! Background project-time attribution.
//!
//! Periodically (and once on startup, as a backfill) recomputes which projects
//! each day's work sessions belong to, by correlating sessions (activity.db)
//! with the project-linked memories in their window (memory.db). The pure
//! decision logic lives in `crate::attribution`; this just orchestrates the
//! cross-DB read/recompute on the daemon's schedule.

use std::sync::Arc;
use std::time::Duration;

use chrono::NaiveDate;
use tokio::time::interval;
use tracing::{debug, info, warn};

use crate::activity::{ActivityStore, local_day_bounds_utc};
use crate::attribution::{AttribCfg, CarryCfg};
use crate::storage::Storage;

/// Signal window pad: memories are often saved a few minutes AFTER a work
/// burst (batched, or the conversation summary at a session's end), so a hard
/// [start,end] match misses them and over-inflates "unattributed". The pad is
/// clamped to neighbouring-session midpoints inside `recompute_day`, so it can
/// never reach into an adjacent session.
const SIGNAL_PAD_MINUTES: i64 = 10;

/// A project entity must have at least this many real (non-session_summary)
/// memories to generate attribution signal. Below this floor it's almost
/// certainly extractor noise (`light-card`, `project-entity`, …) and its time
/// honestly falls into Unattributed rather than a pseudo-project.
const MIN_PROJECT_MEMS: i64 = 2;

/// Recompute one local day's attribution. The session→project signal comes
/// from the raw memory graph. Archive is a reversible read mask: removing
/// its source signal here would overwrite historical allocations and make
/// undo depend on a later backfill. A failed read degrades to no signal.
pub fn recompute_day(
    activity: &ActivityStore,
    storage: &Storage,
    day: NaiveDate,
    cfg: &AttribCfg,
) -> anyhow::Result<()> {
    let _write = storage.attribution_write_lock()?;
    let pad = chrono::Duration::minutes(SIGNAL_PAD_MINUTES);
    let carry = CarryCfg::default();
    // The day's project-memory timestamps drive carry-forward's window guard.
    // A failed read degrades to "no memories" → no carry, never a panic.
    let mem_times = match local_day_bounds_utc(day) {
        Some((s, e)) => storage
            .attribution_source_memory_times(s, e, MIN_PROJECT_MEMS)
            .unwrap_or_default(),
        None => Vec::new(),
    };
    activity.recompute_day(day, cfg, pad, &carry, &mem_times, |s, e| {
        storage
            .attribution_source_signals(s, e, MIN_PROJECT_MEMS)
            .unwrap_or_default()
    })
}

/// Recompute the last `days` local days (including today). Returns how many
/// days were processed without error.
pub fn backfill(activity: &ActivityStore, storage: &Storage, days: u32, cfg: &AttribCfg) -> usize {
    let today = chrono::Local::now().date_naive();
    let mut n = 0;
    for i in 0..days as i64 {
        let d = today - chrono::Duration::days(i);
        match recompute_day(activity, storage, d, cfg) {
            Ok(()) => n += 1,
            Err(e) => warn!("Attribution backfill {d}: {e}"),
        }
    }
    n
}

/// Spawn the attribution worker: backfill `backfill_days` on startup, then
/// recompute today every `interval_secs`.
pub fn spawn_worker(
    activity: Arc<ActivityStore>,
    storage: Arc<Storage>,
    interval_secs: u64,
    backfill_days: u32,
) -> tokio::task::JoinHandle<()> {
    info!(
        "Attribution worker starting (interval={}s, backfill={}d)",
        interval_secs.max(60),
        backfill_days
    );
    tokio::spawn(async move {
        let cfg = AttribCfg::default();

        // Startup backfill so the Projects page has history immediately.
        {
            let (a, s, c) = (activity.clone(), storage.clone(), cfg.clone());
            match tokio::task::spawn_blocking(move || backfill(&a, &s, backfill_days, &c)).await {
                Ok(n) => info!("Attribution: backfilled {n} day(s)"),
                Err(e) => warn!("Attribution backfill join error: {e}"),
            }
        }

        let secs = interval_secs.max(60);
        let mut ticker = interval(Duration::from_secs(secs));
        ticker.tick().await; // skip the immediate first tick
        loop {
            ticker.tick().await;
            let (a, s, c) = (activity.clone(), storage.clone(), cfg.clone());
            let r = tokio::task::spawn_blocking(move || {
                let today = chrono::Local::now().date_naive();
                recompute_day(&a, &s, today, &c)
            })
            .await;
            match r {
                Ok(Ok(())) => debug!("Attribution: recomputed today"),
                Ok(Err(e)) => warn!("Attribution recompute error: {e}"),
                Err(e) => warn!("Attribution join error: {e}"),
            }
        }
    })
}

/// Make again the last `days` local days by the rule of live projects:
/// those from the day of the first segment on. Days before it keep the
/// rows they have. How many days were made.
pub fn backfill_live(activity: &ActivityStore, storage: &Storage, days: u32) -> usize {
    use crate::owner_time_day::{LocalCalendar, days_of_the_rule, recompute_day};
    let today = chrono::Local::now().date_naive();
    let first = today - chrono::Duration::days(i64::from(days.max(1)) - 1);
    let cfg = AttribCfg::default();
    let due = match days_of_the_rule(storage, &LocalCalendar, first, today) {
        Ok(due) => due,
        Err(e) => {
            warn!("Attribution backfill: {e}");
            return 0;
        }
    };
    let mut made = 0;
    for day in due {
        match recompute_day(activity, storage, &LocalCalendar, day, &cfg) {
            Ok(_) => made += 1,
            Err(e) => warn!("Attribution backfill {day}: {e}"),
        }
    }
    made
}

/// Spawn the worker of the owner's time by live projects (design 8 of
/// live-projects): at start and every `interval_secs` it makes again the
/// days that are due. The timer runs on the monotonic clock; only the
/// bounds of a day use the wall clock, and a wall clock that did not move
/// as the monotonic one did, or a changed timezone, marks the days it
/// touched.
pub fn spawn_live_worker(
    activity: Arc<ActivityStore>,
    storage: Arc<Storage>,
    interval_secs: u64,
    live: Option<Arc<crate::live_run::Live>>,
) -> tokio::task::JoinHandle<()> {
    use crate::owner_time_day::{LocalCalendar, clock_moved, hours_of, recompute_due};
    info!(
        "Attribution worker starting, by live projects (interval={}s)",
        interval_secs.max(60)
    );
    tokio::spawn(async move {
        let cfg = AttribCfg::default();
        let mut ticker = interval(Duration::from_secs(interval_secs.max(60)));
        let mut seen_today: Option<NaiveDate> = None;
        // The wall clock, the monotonic clock and the timezone at the
        // last pass.
        let mut watch: Option<(chrono::DateTime<chrono::Utc>, std::time::Instant, i32)> = None;
        loop {
            ticker.tick().await;
            let (a, s, c) = (activity.clone(), storage.clone(), cfg.clone());
            let (mut seen, before) = (seen_today, watch);
            let told = live.clone();
            let pass = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
                let (now, at) = (chrono::Utc::now(), std::time::Instant::now());
                let zone = chrono::Local::now().offset().local_minus_utc();
                if let Some((wall, then, zone_then)) = before {
                    if let Some((from, to)) = clock_moved(wall, at - then, now) {
                        s.attribution_mark_dirty(from, to)?;
                    }
                    if zone != zone_then {
                        // The bounds of the days in progress moved.
                        s.attribution_mark_dirty(now - chrono::Duration::days(1), now)?;
                    }
                }
                let made = recompute_due(&a, &s, &LocalCalendar, now, &mut seen, &c)?;
                // The live state shows the hours of today as they are now.
                if let Some(live) = told {
                    live.set_hours(hours_of(&a, &s, &LocalCalendar, now, &c)?);
                }
                Ok((made, seen, (now, at, zone)))
            })
            .await;
            match pass {
                Ok(Ok((made, seen, now))) => {
                    seen_today = seen;
                    watch = Some(now);
                    debug!("Attribution: made {} day(s)", made.len());
                }
                Ok(Err(e)) => warn!("Attribution recompute error: {e}"),
                Err(e) => warn!("Attribution join error: {e}"),
            }
        }
    })
}

#[cfg(test)]
#[path = "attribution_worker_archive_tests.rs"]
mod archive_tests;
