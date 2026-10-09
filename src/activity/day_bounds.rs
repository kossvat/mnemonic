//! Calendar boundaries shared by activity, owner time and timeline reads.
use chrono::{DateTime, Duration, LocalResult, NaiveDate, NaiveDateTime, TimeZone, Utc};

/// A local date starts at its first representable instant. Midnight folds
/// choose the earlier instant; midnight gaps advance to the end of the gap.
/// A skipped date has no bounds, but may be crossed by the preceding day's end.
pub(super) fn in_zone<T: TimeZone>(
    zone: &T,
    day: NaiveDate,
) -> Option<(DateTime<Utc>, DateTime<Utc>)> {
    let resolve = |local: &NaiveDateTime| valid_instant(zone, local);
    let from = first_representable(day.and_hms_opt(0, 0, 0)?, resolve)?;
    // Do not turn an entirely skipped date into a zero-length local day.
    if from.with_timezone(zone).date_naive() != day {
        return None;
    }
    let to = first_representable(day.succ_opt()?.and_hms_opt(0, 0, 0)?, resolve)?;
    (from < to).then_some((from, to))
}

/// Some timezone adapters report a stale offset at the exact start of a
/// gap, or return fold candidates out of UTC order. Check the round trip
/// and choose the earliest actual instant, independent of candidate order.
fn valid_instant<T: TimeZone>(zone: &T, local: &NaiveDateTime) -> Option<DateTime<Utc>> {
    let candidates = match zone.from_local_datetime(local) {
        LocalResult::None => [None, None],
        LocalResult::Single(instant) => [Some(instant), None],
        LocalResult::Ambiguous(first, second) => [Some(first), Some(second)],
    };
    candidates
        .into_iter()
        .flatten()
        .map(|instant| instant.with_timezone(&Utc))
        .filter(|instant| instant.with_timezone(zone).naive_local() == *local)
        .min()
}

/// Ordinary boundaries need one candidate resolution. Only an initial gap is bracketed,
/// by hours, then refined to its first representable second. TZ transition
/// offsets have second precision. The 48-hour ceiling also bounds work for
/// an unavailable date or timezone; normal midnight DST gaps take at most
/// fourteen candidate resolutions, without a per-second scan.
fn first_representable(
    midnight: NaiveDateTime,
    mut resolve: impl FnMut(&NaiveDateTime) -> Option<DateTime<Utc>>,
) -> Option<DateTime<Utc>> {
    if let Some(instant) = resolve(&midnight) {
        return Some(instant);
    }
    let mut lower = 0;
    for hour in 1..=48 {
        let mut upper = hour * 3600;
        let local = midnight.checked_add_signed(Duration::seconds(upper))?;
        let Some(mut first) = resolve(&local) else {
            lower = upper;
            continue;
        };
        while upper - lower > 1 {
            let middle = lower + (upper - lower) / 2;
            let local = midnight.checked_add_signed(Duration::seconds(middle))?;
            match resolve(&local) {
                Some(instant) => {
                    upper = middle;
                    first = instant;
                }
                None => lower = middle,
            }
        }
        return Some(first);
    }
    None
}

#[cfg(test)]
#[path = "day_bounds_tests.rs"]
mod tests;
