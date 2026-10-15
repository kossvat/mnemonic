//! The board and project detail. All data comes from read-only queries
//! and the in-memory reader of the plan files.
use super::{AppError, AppState};
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use chrono::{Duration, Local, Utc};
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Serialize)]
struct Day {
    day: String,
    owner_s: Option<f64>,
    agent_s: i64,
    confidence: Option<String>,
}

/// The days of each project, and the totals of all of them that the same
/// walk computes for the board's week.
struct Days {
    by_project: BTreeMap<i64, Vec<Day>>,
    /// The sum of the daily unions of agent work, the segments of no known
    /// project included.
    agent_s: i64,
    /// The attributed rows and the unattributed seconds of each day, taken
    /// before rekeying drops the rows of archived projects. None when a day
    /// has no read of the owner's activity.
    owner_s: Option<f64>,
}

fn days(state: &AppState, count: u32) -> anyhow::Result<Days> {
    let today = Local::now().date_naive();
    let keys = state.storage.attribution_keys()?;
    let mut result = BTreeMap::<i64, Vec<Day>>::new();
    let (mut agent_s, mut owner_s) = (0, Some(0.0));
    for offset in (0..count).rev() {
        let day = today - Duration::days(i64::from(offset));
        let Some((from, to)) = crate::activity::local_day_bounds_utc(day) else {
            owner_s = None;
            continue;
        };
        let (agents, union) = state.storage.agent_time(from, to.min(Utc::now()))?;
        agent_s += union;
        let read = state
            .activity
            .as_ref()
            .map(|activity| activity.project_attribution_for_day(day))
            .transpose()?;
        owner_s = owner_s
            .zip(read.as_ref())
            .map(|(sum, (rows, unattributed))| {
                sum + rows.iter().map(|row| row.1).sum::<f64>() + unattributed
            });
        let owner = read
            .map(|(rows, _)| state.storage.rekeyed_day(rows))
            .transpose()?;
        for (id, key) in &keys {
            let row = owner
                .as_ref()
                .and_then(|rows| rows.iter().find(|row| &row.0 == key));
            result.entry(*id).or_default().push(Day {
                day: day.to_string(),
                owner_s: owner.as_ref().map(|_| row.map_or(0.0, |row| row.1)),
                agent_s: agents.get(&Some(*id)).copied().unwrap_or(0),
                confidence: row.and_then(|row| row.2.clone()),
            });
        }
    }
    Ok(Days {
        by_project: result,
        agent_s,
        owner_s,
    })
}

fn plans(state: &AppState) -> BTreeMap<i64, crate::plans::Plan> {
    state
        .live
        .as_ref()
        .map(|live| {
            live.refresh_plans();
            live.project_plans()
        })
        .unwrap_or_default()
}

pub(super) async fn board(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, AppError> {
    tokio::task::spawn_blocking(move || board_value(&state))
        .await
        .map_err(|_| {
            AppError(
                StatusCode::INTERNAL_SERVER_ERROR,
                "project reader unavailable".into(),
            )
        })?
        .map(Json)
        .map_err(AppError::from)
}

fn board_value(state: &AppState) -> anyhow::Result<serde_json::Value> {
    let plans = plans(state);
    let rows = state.storage.projects()?;
    let activity = state.storage.project_last_activity()?;
    let days = days(state, 7)?;
    let week = serde_json::json!({
        "owner_s": days.owner_s, "agent_s": days.agent_s, "owner_estimate": true,
    });
    let days = days.by_project;
    let canonical_keys: BTreeMap<i64, String> =
        rows.iter().map(|row| (row.id, row.slug.clone())).collect();
    let projects: Vec<_> = rows.into_iter().filter_map(|row| {
        if crate::redaction::state::check_identities([row.slug.as_str()]).is_err() { return None; }
        let canonical = state.storage.project_canonical(row.id).ok()?;
        let time = days.get(&canonical);
        let owner_s = time.filter(|days| days.iter().all(|day| day.owner_s.is_some()))
            .map(|days| days.iter().filter_map(|day| day.owner_s).sum::<f64>());
        let agent_s = time.map(|days| days.iter().map(|day| day.agent_s).sum::<i64>()).unwrap_or(0);
        let plan = plans.get(&canonical);
        let unit = plan.and_then(|plan| plan.units.iter().find(|unit| unit.current_task.is_some())
            .or_else(|| plan.units.first()));
        let last_closed = plan.and_then(|plan| plan.units.iter().find(|unit| unit.closed))
            .map(|unit| serde_json::json!({ "title": unit.title, "closed_at": unit.closed_at }));
        Some(serde_json::json!({
            "key": row.slug,
            "name": crate::redaction::state::prepare_capped(&row.name, 160).value,
            "state": row.state.as_str(), "merged_into": (canonical != row.id).then(|| canonical_keys.get(&canonical)).flatten(),
            "last_active_at": activity.get(&canonical),
            "week": { "owner_s": owner_s, "agent_s": agent_s, "owner_estimate": true },
            "plan": unit.map(|unit| unit.line()),
            "gate_at": unit.and_then(|unit| unit.gate.at),
            "plan_provider": unit.map(|unit| unit.provider),
            "plan_counts": unit.map(|unit| &unit.counts),
            "plan_weekly_count": unit.map(|unit| unit.weekly_count),
            // A plan with no unit is the reader's "no plan file": nothing to count.
            "open_units": plan.filter(|plan| !plan.units.is_empty())
                .map(|plan| plan.units.iter().filter(|unit| !unit.closed).count()),
            "waiting_on_owner": unit.map(|unit| &unit.waiting_on_owner),
            "last_closed": last_closed,
            "plan_message": plan.and_then(|plan| plan.message).or_else(|| plan.is_none().then_some("no plan file")),
        }))
    }).collect();
    Ok(serde_json::json!({ "projects": projects, "week": week }))
}

pub(super) async fn detail(
    State(state): State<AppState>,
    Path(key): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    crate::redaction::state::admit_identities("project", [key.as_str()])?;
    tokio::task::spawn_blocking(move || detail_value(&state, &key))
        .await
        .map_err(|_| {
            AppError(
                StatusCode::INTERNAL_SERVER_ERROR,
                "project reader unavailable".into(),
            )
        })?
        .map(Json)
}

fn detail_value(state: &AppState, key: &str) -> Result<serde_json::Value, AppError> {
    let row = state
        .storage
        .project_by_key(key)?
        .ok_or_else(|| AppError(StatusCode::NOT_FOUND, "no such project".into()))?;
    let canonical = state.storage.project_canonical(row.id)?;
    let mut plans = plans(state);
    let plan = plans.remove(&canonical).unwrap_or(crate::plans::Plan {
        units: Vec::new(),
        message: Some("no plan file"),
    });
    let mut time = days(state, 14)?;
    let sessions: Vec<_> = state
        .live
        .as_ref()
        .map(|live| live.state.sessions(Utc::now()))
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(_, session)| {
            let id = session.project?;
            if state.storage.project_canonical(id).ok()? != canonical {
                return None;
            }
            Some(serde_json::json!({
                "agent": session.agent, "driver": if session.program { "program" } else { "owner" },
                "state": session.state_at(Utc::now()), "started_at": session.started_at.timestamp(),
                "last_record_at": session.last_at.timestamp(),
            }))
        })
        .collect();
    Ok(serde_json::json!({
        "key": row.slug, "name": crate::redaction::state::prepare_capped(&row.name, 160).value,
        "state": row.state.as_str(), "plan": plan, "sessions": sessions,
        "gate_detail": state.live.as_ref().map(|live| live.project_plan_details(canonical)),
        "time": time.by_project.remove(&canonical).unwrap_or_default(), "owner_estimate": true,
        "history": state.storage.project_history(canonical, 10)?,
        "top_terms": state.storage.curated_graph_top_terms(canonical, 12)?.into_iter()
            .map(|term| serde_json::json!({"name": term.name, "memory_count": term.memory_count}))
            .collect::<Vec<_>>(),
    }))
}

#[cfg(test)]
#[path = "http_project_read_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "http_project_board_tests.rs"]
mod board_tests;

#[cfg(test)]
#[path = "http_project_cost_tests.rs"]
mod cost_tests;

#[cfg(test)]
#[path = "http_project_history_tests.rs"]
mod history_tests;
