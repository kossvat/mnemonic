//! Curated graph reads serve cached bytes or read-only evidence queries.
//! The explicit archive write refreshes the cache after changing state.
use super::{AppError, AppState};
use crate::storage::curated_graph::{EdgePage, LinkKind, valid_key};
use axum::{
    Json,
    extract::{
        Path, Query, State,
        rejection::{JsonRejection, QueryRejection},
    },
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::Deserialize;

fn unavailable() -> AppError {
    AppError(
        StatusCode::SERVICE_UNAVAILABLE,
        "curated graph unavailable".into(),
    )
}

fn bad(message: &'static str) -> AppError {
    AppError(StatusCode::BAD_REQUEST, message.into())
}

fn parsed<T>(query: Result<Query<T>, QueryRejection>) -> Result<T, AppError> {
    query
        .map(|Query(value)| value)
        .map_err(|_| bad("invalid graph query"))
}

fn identity(value: &str) -> Result<(), AppError> {
    crate::redaction::state::admit_identities("curated graph", [value])?;
    if value.is_empty() || value.len() > 192 {
        return Err(bad("invalid graph identity"));
    }
    Ok(())
}

fn node_key(value: &str) -> Result<(), AppError> {
    identity(value)?;
    // Slots additionally accept ring anchors; HTTP only accepts real nodes.
    if !valid_key(value) || value.starts_with("ring:") {
        return Err(bad("invalid graph node key"));
    }
    Ok(())
}

fn held(headers: &HeaderMap, etag: &str) -> bool {
    headers
        .get_all(header::IF_NONE_MATCH)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .any(|value| value == "*" || value.strip_prefix("W/").unwrap_or(value) == etag)
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct GraphQuery {
    expand: Option<String>,
}

pub(super) async fn graph(
    State(state): State<AppState>,
    query: Result<Query<GraphQuery>, QueryRejection>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let query = parsed(query)?;
    if let Some(expand) = &query.expand {
        node_key(expand)?;
    }
    let live = state.live.as_ref().ok_or_else(unavailable)?;
    let overview = live.graph_snapshot(None)?.ok_or_else(unavailable)?;
    let snapshot = match query.expand.as_deref() {
        None => overview,
        Some(key) => live
            .graph_snapshot(Some(key))?
            .ok_or_else(|| AppError(StatusCode::NOT_FOUND, "no such graph expansion".into()))?,
    };
    let tag = snapshot.etag.clone();
    let cache = "private, no-cache".to_string();
    if held(&headers, &tag) {
        return Ok((
            StatusCode::NOT_MODIFIED,
            [(header::ETAG, tag), (header::CACHE_CONTROL, cache)],
        )
            .into_response());
    }
    Ok((
        StatusCode::OK,
        [
            (header::ETAG, tag),
            (header::CACHE_CONTROL, cache),
            (header::CONTENT_TYPE, "application/json".into()),
        ],
        snapshot.bytes.clone(),
    )
        .into_response())
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct PageQuery {
    offset: Option<usize>,
    limit: Option<usize>,
}

impl PageQuery {
    fn page(&self) -> Result<EdgePage, AppError> {
        let limit = self.limit.unwrap_or(50).min(100);
        if limit == 0 {
            return Err(bad("graph page limit must be positive"));
        }
        Ok(EdgePage {
            offset: self.offset.unwrap_or(0),
            limit,
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct EdgeQuery {
    source: String,
    target: String,
    kind: String,
    offset: Option<usize>,
    limit: Option<usize>,
}

pub(super) async fn edge(
    State(state): State<AppState>,
    query: Result<Query<EdgeQuery>, QueryRejection>,
) -> Result<Json<serde_json::Value>, AppError> {
    let query = parsed(query)?;
    node_key(&query.source)?;
    node_key(&query.target)?;
    let kind = match query.kind.as_str() {
        "contains" => LinkKind::Contains,
        "uses" => LinkKind::Uses,
        "involves" => LinkKind::Involves,
        "relates" => LinkKind::Relates,
        _ => return Err(bad("invalid graph link kind")),
    };
    let page = PageQuery {
        offset: query.offset,
        limit: query.limit,
    }
    .page()?;
    let live = state.live.clone().ok_or_else(unavailable)?;
    let ids = tokio::task::spawn_blocking(move || {
        live.graph_edge_memories(&query.source, &query.target, kind, page)
    })
    .await
    .map_err(|_| {
        AppError(
            StatusCode::INTERNAL_SERVER_ERROR,
            "graph evidence unavailable".into(),
        )
    })??
    .ok_or_else(unavailable)?;
    let next_offset = (ids.len() == page.limit).then(|| page.offset.saturating_add(ids.len()));
    let ids: Vec<_> = ids
        .into_iter()
        .filter(|id| crate::redaction::state::check_identities([id.as_str()]).is_ok())
        .collect();
    Ok(Json(
        serde_json::json!({"memory_ids": ids, "offset": page.offset,
        "limit": page.limit, "next_offset": next_offset}),
    ))
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct ArchiveQuery {
    offset: Option<usize>,
    limit: Option<usize>,
    archived: Option<bool>,
}

pub(super) async fn archives(
    State(state): State<AppState>,
    query: Result<Query<ArchiveQuery>, QueryRejection>,
) -> Result<Json<serde_json::Value>, AppError> {
    let query = parsed(query)?;
    let archived = query.archived.unwrap_or(true);
    let page = PageQuery {
        offset: query.offset,
        limit: query.limit,
    }
    .page()?;
    tokio::task::spawn_blocking(move || {
        let rows = if archived {
            state.storage.entity_archive_history()?
        } else {
            state.storage.entity_archive_shelf(false)?
        };
        let mut rows: Vec<_> = rows
            .into_iter()
            .filter(|row| {
                row.archived_at.is_some() == archived
                    && row.id.len() <= 192
                    && crate::redaction::state::check_identities([row.id.as_str()]).is_ok()
            })
            .collect();
        rows.sort_by(|a, b| a.id.cmp(&b.id));
        let total = rows.len();
        let rows: Vec<_> = rows
            .into_iter()
            .skip(page.offset)
            .take(page.limit)
            .map(|row| {
                serde_json::json!({"id": row.id,
                "name": crate::redaction::state::prepare_capped(&row.name, 80).value,
                "memory_count": row.memory_count, "last_memory": row.last_memory,
                "archived_at": row.archived_at})
            })
            .collect();
        let after = page.offset.saturating_add(rows.len());
        Ok(Json(
            serde_json::json!({"entities": rows, "offset": page.offset,
            "limit": page.limit, "total": total, "next_offset": (after < total).then_some(after)}),
        ))
    })
    .await
    .map_err(|_| {
        AppError(
            StatusCode::INTERNAL_SERVER_ERROR,
            "entity archives unavailable".into(),
        )
    })?
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ArchiveBody {
    archived: bool,
}

pub(super) async fn archive(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Result<Json<ArchiveBody>, JsonRejection>,
) -> Result<Json<serde_json::Value>, AppError> {
    identity(&id)?;
    let Json(body) = body.map_err(|_| bad("invalid entity archive body"))?;
    tokio::task::spawn_blocking(move || {
        // Retained marks outlive entity merges and graph rebuilds. Repeating
        // either state may repair a failed refresh after the entity is gone.
        let archived = state.storage.entity_archive_at(&id)?.is_some();
        if body.archived
            && !archived
            && !state
                .storage
                .entity_archive_shelf(false)?
                .iter()
                .any(|row| row.id == id)
        {
            return Err(AppError(
                StatusCode::NOT_FOUND,
                "no such entity archive candidate".into(),
            ));
        }
        // Undo is an idempotent exact-ID delete, including an absent mark.
        // Creating a new archive still requires the live candidate above.
        let changed = if archived == body.archived {
            false
        } else {
            state
                .storage
                .entity_archive_set(&id, body.archived, chrono::Utc::now().timestamp())?
        };
        // The live worker may have rebuilt a historical cached day while
        // this entity was archived. Undo repairs existing live-policy dates
        // before publishing the graph, including after a failed first try.
        if !body.archived && state.config.projects.root().is_some() {
            crate::owner_time_day::repair_live_after_entity_undo(
                &state.storage,
                state.activity.as_deref(),
                &crate::owner_time_day::LocalCalendar,
                chrono::Utc::now(),
                &crate::attribution::AttribCfg::default(),
            )?;
        }
        // A previous write may have committed before cache refresh failed.
        // Retrying the same state must repair that cache without another write.
        if let Some(live) = &state.live {
            live.refresh_graph()?;
        }
        let at = if body.archived {
            state.storage.entity_archive_at(&id)?
        } else {
            None
        };
        Ok(Json(serde_json::json!({
            "id": id, "archived": body.archived, "archived_at": at, "changed": changed
        })))
    })
    .await
    .map_err(|_| {
        AppError(
            StatusCode::INTERNAL_SERVER_ERROR,
            "entity archive unavailable".into(),
        )
    })?
}

#[cfg(test)]
#[path = "http_curated_graph_tests.rs"]
mod tests;
