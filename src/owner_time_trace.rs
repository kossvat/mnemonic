//! The shared pure range walk behind totals and the day timeline.
use super::{ProjectId, Relabel, Split, Transcript, Trust, live, relabel_at};

/// A simultaneous share of an interval. No project is explicitly unassigned.
#[derive(Debug, Clone, PartialEq)]
pub struct Share {
    pub project: Option<ProjectId>,
    pub share: f64,
    pub trust: Option<Trust>,
}

/// Millisecond bounds. Empty shares are uncovered and may use the fallback;
/// a share with no project is explicit none and must never use it.
#[derive(Debug, Clone, PartialEq)]
pub struct Range {
    pub from: i64,
    pub to: i64,
    pub shares: Vec<Share>,
}

fn append(out: &mut Vec<Range>, range: Range) {
    match out.last_mut() {
        Some(last) if last.to == range.from && last.shares == range.shares => last.to = range.to,
        _ => out.push(range),
    }
}

/// Walk exactly the same tier, trust and newest-correction decisions as totals.
/// Equal adjacent uncovered ranges stay joined before the fallback reads them.
pub fn trace(activity: &[(i64, i64)], transcripts: &[Transcript], owner: &[Relabel]) -> Vec<Range> {
    let mut edges = Vec::new();
    for transcript in transcripts {
        transcript.edges(&mut edges);
    }
    for relabel in owner {
        edges.extend([relabel.from * 1000, relabel.to * 1000]);
    }
    edges.sort_unstable();
    edges.dedup();
    let mut out = Vec::new();
    for &(from, to) in activity {
        if to <= from {
            continue;
        }
        let inside = edges.partition_point(|edge| *edge <= from);
        let mut start = from;
        let cuts = edges[inside..]
            .iter()
            .copied()
            .take_while(|edge| *edge < to)
            .chain([to]);
        for end in cuts {
            let (projects, relabelled) = live(transcripts, start);
            let said = relabel_at(owner, start.div_euclid(1000))
                .filter(|range| relabelled.is_none_or(|later| later < range.order));
            let shares = if let Some(range) = said {
                vec![Share {
                    project: range.project,
                    share: 1.0,
                    trust: range.project.map(|_| Trust::High),
                }]
            } else {
                let share = 1.0 / projects.len().max(1) as f64;
                projects
                    .into_iter()
                    .map(|(project, trust)| Share {
                        project: Some(project),
                        share,
                        trust: Some(trust),
                    })
                    .collect()
            };
            append(
                &mut out,
                Range {
                    from: start,
                    to: end,
                    shares,
                },
            );
            start = end;
        }
    }
    out
}

/// Add simultaneous shares without inventing sequential sub-intervals.
pub fn summarize(ranges: &[Range]) -> Split {
    let mut out = Split::default();
    for range in ranges {
        let seconds = (range.to - range.from) as f64 / 1000.0;
        if range.shares.is_empty() {
            match out.uncovered.last_mut() {
                Some((_, last)) if *last == range.from => *last = range.to,
                _ => out.uncovered.push((range.from, range.to)),
            }
        }
        for share in &range.shares {
            match (share.project, share.trust) {
                (Some(project), Some(trust)) => out.give(project, trust, seconds * share.share),
                _ => out.nobody += seconds * share.share,
            }
        }
    }
    out
}
