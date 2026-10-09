//! A read-only local day of agent segments and estimated owner activity.
use crate::activity::ActivityStore;
use crate::attribution::AttribCfg;
use crate::owner_time_day::Calendar;
use crate::storage::Storage;
use anyhow::{Result, anyhow};
use chrono::{DateTime, NaiveDate, Utc};
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, Serialize, PartialEq)]
pub struct ProjectTimeline {
    pub day: String,
    pub from: f64,
    pub to: f64,
    pub projects: Vec<TimelineProject>,
    pub segments: Vec<TimelineSegment>,
    pub owner: Vec<TimelineOwnerRange>,
    pub owner_available: bool,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct TimelineProject {
    pub key: String,
    pub name: String,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct TimelineSegment {
    pub id: String,
    pub stream: String,
    pub project: Option<String>,
    pub agent: String,
    pub driver: String,
    pub from: f64,
    pub to: f64,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct TimelineOwnerRange {
    pub id: String,
    pub from: f64,
    pub to: f64,
    pub assignments: Vec<TimelineAssignment>,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct TimelineAssignment {
    pub project: Option<String>,
    pub share: f64,
    pub confidence: Option<&'static str>,
}

/// Fixed Gregorian spelling, not a locale-dependent or permissive parser.
pub fn parse_day(value: &str) -> Option<NaiveDate> {
    let bytes = value.as_bytes();
    if bytes.len() != 10
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || !bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| index == 4 || index == 7 || byte.is_ascii_digit())
        || &value[..4] == "0000"
    {
        return None;
    }
    NaiveDate::parse_from_str(value, "%Y-%m-%d").ok()
}

/// Current streams are opaque IDs or JSON [namespace, filename] identities.
/// Legacy directory paths cannot become identifiers on this read surface.
fn safe_stream(value: &str) -> bool {
    let leaf = |part: &str| {
        !part.is_empty()
            && part != "."
            && part != ".."
            && !part
                .chars()
                .any(|letter| letter.is_control() || letter == '/' || letter == '\\')
    };
    if crate::redaction::state::check_identities([value]).is_err() {
        return false;
    }
    match serde_json::from_str::<[String; 2]>(value) {
        Ok(parts) => parts.iter().all(|part| leaf(part)),
        Err(_) => leaf(value),
    }
}

fn seconds(milliseconds: i64) -> f64 {
    milliseconds as f64 / 1000.0
}

/// Full calendar bounds define the axis; only the returned data is clipped to now.
/// No derived rows, dirty marks, activity rows or recall logs are changed.
pub fn read(
    storage: &Storage,
    activity: Option<&ActivityStore>,
    calendar: &dyn Calendar,
    day: NaiveDate,
    now: DateTime<Utc>,
    cfg: &AttribCfg,
) -> Result<ProjectTimeline> {
    let (from, to) = calendar
        .bounds(day)
        .ok_or_else(|| anyhow!("local day unavailable"))?;
    let mut keys = BTreeMap::new();
    let mut projects = Vec::new();
    for row in storage.projects()? {
        if storage.project_canonical(row.id)? != row.id
            || crate::redaction::state::check_identities([row.slug.as_str()]).is_err()
        {
            continue;
        }
        keys.insert(row.id, row.slug.clone());
        projects.push(TimelineProject {
            key: row.slug,
            name: crate::redaction::state::prepare_capped(&row.name, 160).value,
        });
    }
    projects.sort_by(|left, right| left.key.cmp(&right.key));
    let mut result = ProjectTimeline {
        day: day.to_string(),
        from: seconds(from.timestamp_millis()),
        to: seconds(to.timestamp_millis()),
        projects,
        segments: Vec::new(),
        owner: Vec::new(),
        owner_available: activity.is_some(),
    };
    if day > calendar.date_of(now) || now <= from {
        return Ok(result);
    }
    let until = to.min(now);
    let lower = from.timestamp_millis();
    let upper = until.timestamp_millis();
    for piece in storage.timeline_segments(from, until)? {
        if !safe_stream(&piece.stream) {
            continue;
        }
        let point = piece.from == piece.to;
        let start = (piece.from * 1000).max(lower);
        let end = (piece.to * 1000).min(upper);
        if (point && !(lower <= start && start < upper && piece.from * 1000 == start))
            || (!point && end <= start)
        {
            continue;
        }
        let agent = match piece.agent.as_str() {
            "claude" => "claude",
            "codex" => "codex",
            _ => "unknown",
        };
        let driver = match piece.driver.as_str() {
            "owner" => "owner",
            "program" => "program",
            _ => "unknown",
        };
        result.segments.push(TimelineSegment {
            id: format!("segment-{}-{start}-{end}", piece.id),
            stream: piece.stream,
            project: piece.project.and_then(|id| keys.get(&id).cloned()),
            agent: agent.into(),
            driver: driver.into(),
            from: seconds(start),
            to: seconds(end),
        });
    }
    result.segments.sort_by(|left, right| {
        left.from
            .total_cmp(&right.from)
            .then_with(|| left.to.total_cmp(&right.to))
            .then_with(|| left.id.cmp(&right.id))
    });
    if let Some(activity) = activity {
        for range in crate::owner_time_day::ranges(activity, storage, (from, until), cfg)? {
            let mut assignments = Vec::new();
            for share in range.shares {
                let project = share.project.and_then(|id| keys.get(&id).cloned());
                let confidence = project
                    .as_ref()
                    .and(share.trust.map(|trust| trust.as_str()));
                assignments.push(TimelineAssignment {
                    project,
                    share: share.share,
                    confidence,
                });
            }
            result.owner.push(TimelineOwnerRange {
                id: format!("owner-{}-{}", range.from, range.to),
                from: seconds(range.from),
                to: seconds(range.to),
                assignments,
            });
        }
    }
    Ok(result)
}

#[cfg(test)]
#[path = "timeline_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "timeline_fixture.rs"]
pub(crate) mod fixture;
