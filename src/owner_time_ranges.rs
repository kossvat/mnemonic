//! One read-only fallback pipeline for the day totals and the timeline.
use crate::activity::ActivityStore;
use crate::attribution::{AttribCfg, ProjectSignal, attribute_session};
use crate::owner_time::{Range, Share, Trust, trace};
use crate::storage::Storage;
use anyhow::Result;
use chrono::{DateTime, Duration, Utc};

/// Memories are often saved a few minutes after a burst of work.
const SIGNAL_PAD_MINUTES: i64 = 10;

pub fn read(
    activity: &ActivityStore,
    storage: &Storage,
    (from, to): (DateTime<Utc>, DateTime<Utc>),
    cfg: &AttribCfg,
) -> Result<Vec<Range>> {
    let intervals = activity
        .intervals_between(from, to)?
        .into_iter()
        .map(|(start, end)| (start.timestamp_millis(), end.timestamp_millis()))
        .collect::<Vec<_>>();
    let (transcripts, corrections) = storage.owner_transcripts(from, to)?;
    let mut ranges = trace(&intervals, &transcripts, &corrections.owner);
    let pad = Duration::minutes(SIGNAL_PAD_MINUTES);
    for range in &mut ranges {
        if !range.shares.is_empty() {
            continue;
        }
        let seconds = (range.to - range.from) as f64 / 1000.0;
        let window = DateTime::from_timestamp_millis(range.from)
            .zip(DateTime::from_timestamp_millis(range.to))
            .map(|(start, end)| (start - pad, end + pad));
        // A failed signal read still leaves this interval unassigned.
        let signals = window
            .and_then(|(start, end)| storage.registry_signals(start, end).ok())
            .unwrap_or_default()
            .into_iter()
            .map(|(project, weight)| ProjectSignal {
                project_key: project.to_string(),
                weight,
            })
            .collect::<Vec<_>>();
        let fallback = attribute_session(seconds, &signals, cfg);
        let mut unassigned = fallback.unattributed_seconds;
        for allocation in fallback.allocations {
            match allocation.project_key.parse() {
                Ok(project) => range.shares.push(Share {
                    project: Some(project),
                    share: allocation.seconds / seconds,
                    trust: Some(Trust::Low),
                }),
                Err(_) => unassigned += allocation.seconds,
            }
        }
        if unassigned > 0.0 {
            range.shares.push(Share {
                project: None,
                share: unassigned / seconds,
                trust: None,
            });
        }
        range.shares.sort_by_key(|share| share.project);
    }
    Ok(ranges)
}
