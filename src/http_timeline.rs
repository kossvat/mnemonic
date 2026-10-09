//! The guarded timeline route, including the project-key collision.
use super::{AppError, AppState, project_read};
use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};

pub(super) async fn read(
    State(state): State<AppState>,
    Query(query): Query<Vec<(String, String)>>,
) -> Result<Json<serde_json::Value>, AppError> {
    let days = query
        .iter()
        .filter(|(key, _)| key == "day")
        .map(|(_, value)| value)
        .collect::<Vec<_>>();
    if days.is_empty() {
        return project_read::detail(State(state), Path("timeline".into())).await;
    }
    let day = (days.len() == 1)
        .then(|| crate::timeline::parse_day(days[0]))
        .flatten()
        .ok_or_else(|| {
            AppError(
                StatusCode::BAD_REQUEST,
                "day must be a valid Gregorian date".into(),
            )
        })?;
    tokio::task::spawn_blocking(move || {
        crate::timeline::read(
            &state.storage,
            state.activity.as_deref(),
            &crate::owner_time_day::LocalCalendar,
            day,
            chrono::Utc::now(),
            &crate::attribution::AttribCfg::default(),
        )
        .and_then(|value| Ok(serde_json::to_value(value)?))
    })
    .await
    .map_err(|_| {
        AppError(
            StatusCode::INTERNAL_SERVER_ERROR,
            "timeline reader unavailable".into(),
        )
    })?
    .map(Json)
    .map_err(AppError::from)
}

#[cfg(test)]
#[path = "http_timeline_tests.rs"]
mod tests;
