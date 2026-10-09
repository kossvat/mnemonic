//! The owner's writes to the registry of projects (live-projects, design
//! 6 and 12): archive a project, merge one into another, and undo either.
//! Each is a change of one row; nothing is moved or deleted.
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::Deserialize;

use super::{AppError, AppState};
use crate::storage::projects::{ProjectRow, ProjectState};

/// The project a key names, or the reply that says there is none. The key
/// is judged before it is looked up or said back.
fn project(state: &AppState, key: &str) -> Result<ProjectRow, AppError> {
    crate::redaction::state::admit_identities("project", [key])?;
    state
        .storage
        .project_by_key(key)?
        .ok_or_else(|| AppError(StatusCode::NOT_FOUND, "no such project".into()))
}

/// What a write answers with: the project as it is now.
fn answer(state: &AppState, id: i64) -> Result<Json<serde_json::Value>, AppError> {
    let rows = state.storage.projects()?;
    let slug = |id: i64| {
        rows.iter()
            .find(|row| row.id == id)
            .map(|row| row.slug.clone())
    };
    let row = rows
        .iter()
        .find(|row| row.id == id)
        .ok_or_else(|| AppError(StatusCode::NOT_FOUND, "no such project".into()))?;
    let canonical = state.storage.project_canonical(id)?;
    Ok(Json(serde_json::json!({
        "key": row.slug,
        "state": row.state.as_str(),
        "merged_into": (canonical != id).then(|| slug(canonical)).flatten(),
    })))
}

/// A successful reply includes the refreshed graph. A no-op retry repairs
/// a cache refresh that failed after the original registry write committed.
async fn refresh_and_answer(state: AppState, id: i64) -> Result<Json<serde_json::Value>, AppError> {
    tokio::task::spawn_blocking(move || {
        if let Some(live) = &state.live {
            // The live snapshot names and groups projects as the registry
            // has them now, not at its next pass.
            live.load_projects(&state.storage)?;
            live.refresh_graph()?;
        }
        answer(&state, id)
    })
    .await
    .map_err(|_| {
        AppError(
            StatusCode::INTERNAL_SERVER_ERROR,
            "project graph refresh unavailable".into(),
        )
    })?
}

pub(super) async fn archive(
    State(state): State<AppState>,
    Path(key): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let row = project(&state, &key)?;
    state
        .storage
        .project_set_state(row.id, ProjectState::Archived)?;
    refresh_and_answer(state, row.id).await
}

pub(super) async fn unarchive(
    State(state): State<AppState>,
    Path(key): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let row = project(&state, &key)?;
    state
        .storage
        .project_set_state(row.id, ProjectState::Active)?;
    refresh_and_answer(state, row.id).await
}

#[derive(Deserialize)]
pub(super) struct MergeInto {
    into: String,
}

pub(super) async fn merge(
    State(state): State<AppState>,
    Path(key): Path<String>,
    Json(body): Json<MergeInto>,
) -> Result<Json<serde_json::Value>, AppError> {
    let from = project(&state, &key)?;
    let into = project(&state, &body.into)?;
    let source = state.storage.project_canonical(from.id)?;
    let target = state.storage.project_canonical(into.id)?;
    let retry = from.merged_into.is_some() && source == target;
    if from.id == into.id || target == from.id || (source == target && !retry) {
        return Err(AppError(
            StatusCode::BAD_REQUEST,
            "a project cannot be merged into itself".into(),
        ));
    }
    if !retry {
        state.storage.project_merge(from.id, into.id)?;
    }
    refresh_and_answer(state, from.id).await
}

pub(super) async fn unmerge(
    State(state): State<AppState>,
    Path(key): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let row = project(&state, &key)?;
    state.storage.project_unmerge(row.id)?;
    refresh_and_answer(state, row.id).await
}

#[cfg(test)]
#[path = "http_projects_tests.rs"]
mod tests;
