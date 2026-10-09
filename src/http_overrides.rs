//! The owner's corrections of attribution (live-projects, design 9.1): a
//! route that keeps one and a route that undoes it. A correction pins a
//! transcript to a project, relabels a range of a transcript's time, or
//! gives a range of the owner's own time to a project; `null` for the
//! project is "no project".
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use serde::Deserialize;

use super::{AppError, AppState};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Correction {
    /// The transcript, as the live view names it.
    #[serde(default)]
    stream: Option<String>,
    /// The range of time, both ends or neither.
    #[serde(default)]
    from: Option<DateTime<Utc>>,
    #[serde(default)]
    to: Option<DateTime<Utc>>,
    /// The key of the project, or `null` for none. It has to be said: a
    /// field left out is not "no project".
    #[serde(deserialize_with = "said")]
    project: Option<String>,
}

fn said<'de, D: serde::Deserializer<'de>>(from: D) -> Result<Option<String>, D::Error> {
    Option::<String>::deserialize(from)
}

fn bad(what: &str) -> AppError {
    AppError(StatusCode::BAD_REQUEST, what.into())
}

pub(super) async fn add(
    State(state): State<AppState>,
    Json(body): Json<Correction>,
) -> Result<Json<serde_json::Value>, AppError> {
    let range = match (body.from, body.to) {
        (Some(from), Some(to)) if from < to && (to - from).num_days() < 3660 => Some((from, to)),
        (None, None) => None,
        _ => {
            return Err(bad(
                "a range has a beginning and a later end, less than ten years apart",
            ));
        }
    };
    if body.stream.is_none() && range.is_none() {
        return Err(bad("a correction names a transcript, a range, or both"));
    }
    // What names something is judged before it is looked up or kept.
    let named = body.stream.iter().chain(body.project.iter());
    crate::redaction::state::admit_identities("correction", named.map(String::as_str))?;
    let project = match &body.project {
        Some(key) => Some(
            state
                .storage
                .project_by_key(key)?
                .ok_or_else(|| AppError(StatusCode::NOT_FOUND, "no such project".into()))?
                .id,
        ),
        None => None,
    };
    let id = state
        .storage
        .override_add(body.stream.as_deref(), range, project)?;
    Ok(Json(serde_json::json!({ "id": id })))
}

pub(super) async fn remove(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, AppError> {
    if !state.storage.override_remove(id)? {
        return Err(AppError(StatusCode::NOT_FOUND, "no such correction".into()));
    }
    Ok(Json(serde_json::json!({ "removed": id })))
}

#[cfg(test)]
#[path = "http_overrides_tests.rs"]
mod tests;
