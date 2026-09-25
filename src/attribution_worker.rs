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
/// from the memory graph (`project_signals_in_window`); a failed signal read
/// degrades to "no signal" for that window (→ unattributed), never a panic.
pub fn recompute_day(
    activity: &ActivityStore,
    storage: &Storage,
    day: NaiveDate,
    cfg: &AttribCfg,
) -> anyhow::Result<()> {
    let pad = chrono::Duration::minutes(SIGNAL_PAD_MINUTES);
    let carry = CarryCfg::default();
    // The day's project-memory timestamps drive carry-forward's window guard.
    // A failed read degrades to "no memories" → no carry, never a panic.
    let mem_times = match local_day_bounds_utc(day) {
        Some((s, e)) => storage
            .project_mem_times_in_window(s, e, MIN_PROJECT_MEMS)
            .unwrap_or_default(),
        None => Vec::new(),
    };
    activity.recompute_day(day, cfg, pad, &carry, &mem_times, |s, e| {
        storage
            .project_signals_in_window(s, e, MIN_PROJECT_MEMS)
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
