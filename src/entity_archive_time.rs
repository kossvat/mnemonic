//! Explicit undo repairs only existing cache dates governed by live projects.
use super::*;

/// Call after every successful undo request, including an unchanged retry,
/// and only when live-project attribution is enabled. The archive mark may
/// already be gone after a failed earlier repair. No activity handle means
/// there is no accessible cache to repair; this never opens another store.
// The private HTTP entry lives in the binary; library tests share this helper.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn repair_live_after_entity_undo(
    storage: &Storage,
    activity: Option<&ActivityStore>,
    calendar: &dyn Calendar,
    now: DateTime<Utc>,
    cfg: &AttribCfg,
) -> Result<usize> {
    let Some(activity) = activity else {
        return Ok(0);
    };
    let _write = storage.attribution_write_lock()?;
    let Some(first) = storage.first_segment_at()? else {
        return Ok(0);
    };
    let first = calendar.date_of(first);
    let today = calendar.date_of(now);
    let dates = activity.cached_attribution_days()?;
    let mut repaired = 0;
    for day in dates
        .into_iter()
        .filter(|day| *day >= first && *day <= today)
    {
        recompute_day_locked(activity, storage, calendar, day, cfg)?
            .ok_or_else(|| anyhow::anyhow!("cached attribution day unavailable"))?;
        repaired += 1;
    }
    Ok(repaired)
}

#[cfg(test)]
#[path = "entity_archive_time_tests.rs"]
mod tests;
