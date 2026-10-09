//! Owned graph input helpers. Calendar boundaries are injectable for tests.
use super::*;
use chrono::Duration;
use serde_json::json;

pub(super) fn hours(
    storage: &Storage,
    activity: Option<&ActivityStore>,
    calendar: &dyn Calendar,
    now: DateTime<Utc>,
) -> Result<BTreeMap<i64, Hours>> {
    let keys = storage.attribution_keys()?;
    let today = calendar.date_of(now);
    let mut owners = BTreeMap::<i64, f64>::new();
    let mut result: BTreeMap<_, _> = keys
        .keys()
        .map(|id| {
            (
                *id,
                Hours {
                    owner_seconds: activity.map(|_| 0),
                    agent_seconds: Some(0),
                },
            )
        })
        .collect();
    for offset in 0..30 {
        let day = today - Duration::days(offset);
        let Some((from, to)) = calendar.bounds(day) else {
            // A calendar failure means incomplete measurements, not zero.
            for hours in result.values_mut() {
                *hours = Hours::default();
            }
            return Ok(result);
        };
        let (agents, _) = storage.agent_time(from, to.min(now))?;
        let owner = activity
            .map(|activity| {
                activity
                    .project_attribution_for_day(day)
                    .and_then(|(rows, _)| storage.rekeyed_day(rows))
            })
            .transpose()?;
        for (id, key) in &keys {
            let hours = result
                .get_mut(id)
                .expect("all attribution keys initialized");
            let seconds = u64::try_from(agents.get(&Some(*id)).copied().unwrap_or(0)).unwrap_or(0);
            hours.agent_seconds = hours.agent_seconds.map(|sum| sum.saturating_add(seconds));
            if let Some(rows) = &owner {
                let seconds = rows
                    .iter()
                    .find(|row| &row.0 == key)
                    .map_or(0.0, |row| row.1);
                if seconds.is_finite() && seconds >= 0.0 {
                    *owners.entry(*id).or_default() += seconds;
                } else {
                    hours.owner_seconds = None;
                }
            }
        }
    }
    for (id, hours) in &mut result {
        hours.owner_seconds = hours
            .owner_seconds
            .map(|_| owners.get(id).copied().unwrap_or(0.0).round() as u64);
    }
    Ok(result)
}

/// All evidence relevant to any expansion, including capped/hidden nodes.
/// Wall time enters only through displayed freshness; a minute without new
/// facts must not signal merely because the worker ticked again.
pub(super) fn material(input: &Prepared) -> Result<String> {
    let freshness = |last: Option<i64>| {
        last.map(|last| {
            let age = input.now.saturating_sub(last).max(0);
            (
                age / 86_400,
                if age <= 7 * 86_400 {
                    0
                } else if age < 30 * 86_400 {
                    1
                } else {
                    2
                },
            )
        })
    };
    let projects = input
        .evidence
        .projects
        .iter()
        .map(|p| {
            json!([
                p.row.id,
                p.row.name,
                p.row.slug,
                p.row.parent_id,
                p.row.merged_into,
                p.row.state.as_str(),
                p.memory_count,
                p.last_memory,
                p.last_segment,
                freshness(p.last_memory.max(p.last_segment)),
            ])
        })
        .collect::<Vec<_>>();
    let entities = input
        .evidence
        .entities
        .iter()
        .map(|e| {
            json!([
                e.id,
                e.name,
                e.kind as u8,
                e.memory_count,
                e.last_memory,
                freshness(e.last_memory),
            ])
        })
        .collect::<Vec<_>>();
    let links = input
        .evidence
        .links
        .iter()
        .map(|e| json!([e.source, e.target, e.kind.as_str(), e.memory_count,]))
        .collect::<Vec<_>>();
    let shelf = input
        .evidence
        .shelf
        .iter()
        .map(|s| {
            json!([
                s.id,
                s.name,
                s.memory_count,
                s.last_memory,
                s.archived_at,
                freshness(s.last_memory),
            ])
        })
        .collect::<Vec<_>>();
    let units = input
        .units
        .iter()
        .map(|u| {
            json!([
                u.project_id,
                u.id,
                u.title,
                u.closed_at.is_some(),
                u.last_memory,
                u.memory_count,
                freshness(u.last_memory),
            ])
        })
        .collect::<Vec<_>>();
    let slots = input
        .slots
        .iter()
        .map(|s| json!([s.key, s.parent_key, s.slot]))
        .collect::<Vec<_>>();
    let hours = input
        .hours
        .iter()
        .map(|(id, h)| json!([id, h.owner_seconds, h.agent_seconds,]))
        .collect::<Vec<_>>();
    let bytes = serde_json::to_vec(&json!([
        projects, entities, links, shelf, units, slots, hours
    ]))?;
    Ok(git2::Oid::hash_object(git2::ObjectType::Blob, &bytes)?.to_string())
}
