//! Pure readers for the three supported kinds of plan.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Datelike, NaiveDate, Utc};
use serde_json::Value;

use super::{ParsedUnit, Task};
use crate::redaction::state::{check_identities, prepare_capped};

const STATUSES: &[&str] = &[
    "backlog",
    "todo",
    "planned",
    "ready",
    "in_progress",
    "review",
    "blocked",
    "done",
];

fn identity(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 160
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
        && value != "."
        && value != ".."
        && check_identities([value]).is_ok()
}

fn narrative(value: &str, cap: usize) -> String {
    prepare_capped(value.trim(), cap).value
}

fn owner(value: &str) -> Option<String> {
    let value = value.trim();
    let lower = value.to_ascii_lowercase();
    (!value.is_empty()
        && !matches!(lower.as_str(), "none" | "n/a" | "-" | "false")
        && !lower.starts_with("none."))
    .then(|| narrative(value, 240))
}

fn unit(id: &str) -> ParsedUnit {
    if identity(id) {
        ParsedUnit {
            id: id.into(),
            title: narrative(id, 160),
            ..Default::default()
        }
    } else {
        ParsedUnit {
            id: "invalid-unit".into(),
            title: "Unreadable unit".into(),
            error: Some("invalid-unit-id".into()),
            ..Default::default()
        }
    }
}

fn push_task(unit: &mut ParsedUnit, task: Task) {
    if unit.tasks.iter().any(|existing| existing.id == task.id) {
        unit.error = Some("duplicate-task-id".into());
        return;
    }
    *unit.counts.entry(task.status.clone()).or_default() += 1;
    unit.tasks.push(task);
}

fn value_id(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) if value.is_u64() => Some(value.to_string()),
        _ => None,
    }
}

/// Read a task registry, isolating invalid task rows to their known unit.
pub fn registry(text: &str) -> Result<Vec<ParsedUnit>, String> {
    let data: Value = serde_json::from_str(text).map_err(|_| "invalid-registry-json")?;
    let rows = data
        .as_array()
        .or_else(|| data.get("tasks").and_then(Value::as_array))
        .ok_or("invalid-registry-shape")?;
    let mut memberships: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut units: BTreeMap<String, ParsedUnit> = BTreeMap::new();
    if let Some(releases) = data.get("releases") {
        for (id, ids) in releases.as_object().ok_or("invalid-release-map")? {
            if !identity(id) {
                return Err("invalid-unit-id".into());
            }
            units.insert(id.clone(), unit(id));
            for task in ids.as_array().ok_or("invalid-release-map")? {
                let task = task.as_str().ok_or("invalid-release-map")?;
                if !identity(task) {
                    units.get_mut(id).unwrap().error = Some("invalid-task-id".into());
                    continue;
                }
                memberships.entry(task.into()).or_default().push(id.clone());
            }
        }
    }
    for row in rows {
        let task_id = row.get("id").and_then(Value::as_str).unwrap_or("");
        let explicit = row.get("unit").or_else(|| row.get("release"));
        let ids = if let Some(explicit) = explicit {
            vec![value_id(explicit).ok_or("invalid-unit-id")?]
        } else {
            memberships
                .get(task_id)
                .cloned()
                .unwrap_or_else(|| vec!["tasks".into()])
        };
        for id in ids {
            let parsed = unit(&id);
            let out = units.entry(parsed.id.clone()).or_insert(parsed);
            let status = row.get("status").and_then(Value::as_str).unwrap_or("");
            let title = row.get("title").and_then(Value::as_str);
            let error = if !identity(task_id) {
                Some("invalid-task-id")
            } else if !STATUSES.contains(&status) {
                Some("invalid-task-status")
            } else if title.is_none() {
                Some("invalid-task-title")
            } else {
                None
            };
            if let Some(error) = error {
                out.error = Some(error.into());
                continue;
            }
            let task_owner = row.get("owner").and_then(Value::as_str).and_then(owner);
            if status != "done" {
                if let Some(wait) = row
                    .get("waiting_on_owner")
                    .and_then(Value::as_str)
                    .and_then(owner)
                {
                    out.waiting_on_owner.push(wait);
                } else if row.get("waiting_on_owner").and_then(Value::as_bool) == Some(true) {
                    out.waiting_on_owner.push(
                        task_owner
                            .clone()
                            .unwrap_or_else(|| narrative(title.unwrap(), 240)),
                    );
                }
            }
            push_task(
                out,
                Task {
                    id: task_id.into(),
                    title: narrative(title.unwrap(), 160),
                    status: status.into(),
                    owner: task_owner,
                },
            );
        }
    }
    for (task, ids) in memberships {
        if !rows
            .iter()
            .any(|row| row.get("id").and_then(Value::as_str) == Some(&task))
        {
            for id in ids {
                units.get_mut(&id).unwrap().error = Some("missing-release-task".into());
            }
        }
    }
    Ok(units.into_values().collect())
}

/// The task-line pattern used by OpenSpec, without a regex dependency.
/// A whitespace-only box has its own alternative, including before links.
fn task_line(line: &str) -> Option<(bool, &str)> {
    let mut rest = line.trim_start();
    if rest.starts_with(['-', '*', '+']) {
        rest = &rest[1..];
    } else {
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        if !(1..=9).contains(&digits) || !rest.get(digits..)?.starts_with(['.', ')']) {
            return None;
        }
        rest = &rest[digits + 1..];
    }
    rest = rest.trim_start().strip_prefix('[')?;
    let close = rest.find(']')?;
    let inside = &rest[..close];
    let marker = inside.trim();
    if marker.chars().count() > 1 {
        return None;
    }
    let after = &rest[close + 1..];
    if after.starts_with(['[', '(']) && (!marker.is_empty() || inside.is_empty()) {
        return None;
    }
    Some((marker == "x", after.trim()))
}

fn task_parts(text: &str, ordinal: usize) -> Result<(String, String), &'static str> {
    let explicit = text
        .split_once(':')
        .filter(|(id, _)| !id.chars().any(char::is_whitespace));
    let numbered = text.split_once(char::is_whitespace).filter(|(id, _)| {
        id.bytes().any(|c| c.is_ascii_digit())
            && id.bytes().all(|c| c.is_ascii_digit() || c == b'.')
    });
    if let Some((id, title)) = explicit.or(numbered) {
        if !identity(id) {
            return Err("invalid-task-id");
        }
        Ok((id.into(), narrative(title, 160)))
    } else {
        Ok((format!("task-{ordinal}"), narrative(text, 160)))
    }
}

/// Read all matching lines, including examples in fences, as OpenSpec does.
pub fn checkboxes(text: &str, default_unit: &str, sections: bool) -> Vec<ParsedUnit> {
    let fallback = unit(default_unit);
    let mut current = fallback.id.clone();
    let mut units = BTreeMap::new();
    let mut section_level = None;
    let mut last_task: Option<(String, usize)> = None;
    for line in text.lines() {
        let heading = line.trim_start();
        let level = heading.bytes().take_while(|c| *c == b'#').count();
        if sections
            && level > 0
            && heading
                .get(level..)
                .is_some_and(|s| s.starts_with(char::is_whitespace))
        {
            last_task = None;
            let title = heading[level..].trim();
            if let Some(id) = title.strip_prefix("OpenSpec:") {
                let parsed = unit(id.trim());
                current = parsed.id.clone();
                section_level = Some(level);
                units.entry(current.clone()).or_insert(parsed);
            } else if section_level.is_some_and(|old| level <= old) {
                current = fallback.id.clone();
                section_level = None;
            }
        }
        if let Some((done, words)) = task_line(line) {
            let out = units
                .entry(current.clone())
                .or_insert_with(|| fallback.clone());
            match task_parts(words, out.tasks.len() + 1) {
                Ok((id, title)) => {
                    push_task(
                        out,
                        Task {
                            id,
                            title,
                            status: if done { "done" } else { "planned" }.into(),
                            owner: None,
                        },
                    );
                    last_task =
                        (!done).then(|| (current.clone(), out.tasks.len().saturating_sub(1)));
                }
                Err(error) => {
                    out.error = Some(error.into());
                    last_task = None;
                }
            }
        } else if let Some(value) = line.trim().strip_prefix("Owner:").and_then(owner)
            && let Some((id, index)) = &last_task
        {
            let out = units.get_mut(id).unwrap();
            out.waiting_on_owner.push(value.clone());
            out.tasks[*index].owner = Some(value);
        }
    }
    if !sections && units.is_empty() {
        units.insert(fallback.id.clone(), fallback);
    }
    units.into_values().collect()
}

fn table_cells(line: &str) -> Vec<String> {
    let mut cells = Vec::new();
    let mut cell = String::new();
    let mut escaped = false;
    for c in line.trim().trim_matches('|').chars() {
        if c == '|' && !escaped {
            cells.push(cell.trim().into());
            cell.clear();
        } else if c == '\\' && !escaped {
            escaped = true;
            continue;
        } else {
            cell.push(c);
        }
        escaped = false;
    }
    if escaped {
        cell.push('\\');
    }
    cells.push(cell.trim().into());
    cells
}

fn label(text: &str) -> String {
    text.trim().to_ascii_lowercase().replace([' ', '-'], "_")
}

/// Read a Markdown status table. Dates count in the current UTC calendar week.
pub fn funnel(
    text: &str,
    default_unit: &str,
    now: DateTime<Utc>,
) -> Result<Vec<ParsedUnit>, String> {
    let mut out = unit(default_unit);
    let mut headers: Option<Vec<String>> = None;
    let mut found = false;
    let week_start =
        now.date_naive() - chrono::Duration::days(now.weekday().num_days_from_monday().into());
    for line in text.lines() {
        if !line.contains('|') {
            headers = None;
            continue;
        }
        let cells = table_cells(line);
        if cells
            .iter()
            .all(|c| !c.is_empty() && c.chars().all(|c| c == '-' || c == ':' || c.is_whitespace()))
        {
            continue;
        }
        if headers.is_none() {
            let names: Vec<_> = cells.iter().map(|c| label(c)).collect();
            if names.iter().any(|c| c == "status") {
                headers = Some(names);
                found = true;
            }
            continue;
        }
        let names = headers.as_ref().unwrap();
        if cells.len() != names.len() {
            out.error = Some("invalid-funnel-row".into());
            continue;
        }
        let field = |keys: &[&str]| {
            keys.iter().find_map(|key| {
                names
                    .iter()
                    .position(|name| name.as_str() == *key)
                    .map(|i| cells[i].as_str())
            })
        };
        let raw_status = field(&["status"]).unwrap_or("");
        let status = label(raw_status);
        if !identity(&status) || check_identities([raw_status]).is_err() {
            out.error = Some("invalid-funnel-status".into());
            continue;
        }
        *out.counts.entry(status.clone()).or_default() += 1;
        if let Some(date) = field(&["date", "created_at", "created", "updated_at", "updated"]) {
            let day = DateTime::parse_from_rfc3339(date)
                .ok()
                .filter(|d| *d <= now)
                .map(|d| d.with_timezone(&Utc).date_naive())
                .or_else(|| NaiveDate::parse_from_str(date, "%Y-%m-%d").ok());
            if day.is_some_and(|d| d >= week_start && d <= now.date_naive()) {
                out.weekly_count += 1;
            }
        }
        let waiting = field(&["waiting_on_owner"]).and_then(owner).or_else(|| {
            matches!(
                status.as_str(),
                "waiting_on_owner" | "awaiting_owner" | "needs_owner"
            )
            .then(|| field(&["owner", "title", "name", "task"]).and_then(owner))
            .flatten()
        });
        if let Some(waiting) = waiting {
            out.waiting_on_owner.push(waiting);
        }
    }
    if !found {
        return Err("missing-status-table".into());
    }
    Ok(vec![out])
}

/// Extract a narrow, explicit declaration without running OpenSpec or YAML code.
pub fn declared_authority(openspec_yaml: &str) -> Result<Option<String>, &'static str> {
    let mut found = BTreeSet::new();
    for line in openspec_yaml.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        let explicit = line
            .split_once(':')
            .filter(|(key, _)| {
                matches!(
                    label(key).as_str(),
                    "task_status_authority"
                        | "task_authority"
                        | "status_authority"
                        | "taskstatusauthority"
                )
            })
            .map(|(_, path)| path.split(" #").next().unwrap_or(path));
        let sentence = line
            .strip_suffix(" is the only task-status authority.")
            .or_else(|| line.strip_suffix(" is the task-status authority."));
        if let Some(path) = explicit.or(sentence) {
            let path = path.trim().trim_matches(['`', '\'', '"']);
            if !path.is_empty()
                && !path.starts_with('/')
                && !path.contains('\\')
                && path
                    .split('/')
                    .all(|p| !p.is_empty() && p != "." && p != ".." && !p.starts_with(".env"))
                && (path.ends_with(".md") || path.ends_with(".json"))
                && check_identities([path]).is_ok()
            {
                found.insert(path.to_string());
            } else {
                return Err("invalid-task-authority");
            }
        }
    }
    if found.len() > 1 {
        Err("ambiguous-task-authority")
    } else {
        Ok(found.into_iter().next())
    }
}

#[cfg(test)]
#[path = "providers_tests.rs"]
mod tests;
