//! HTTP dashboard API.
//!
//! Off by default. When `[ui] enabled = true` the daemon spawns an axum
//! server alongside the unix socket. Bound to 127.0.0.1 only — never
//! exposed to the network. Auth is a static token written to
//! `~/.mnemonic/auth.token` on first start; the dashboard sends it as
//! `X-Mnemonic-Token`.
//!
//! Endpoints are intentionally close to the storage layer — the UI does
//! pagination and grouping. Write endpoints (dedupe, reextract, merge)
//! land in Phase 4d; this is read-only.

use std::sync::Arc;

use anyhow::Result;
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderName, HeaderValue, Method, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use rand::{Rng, distr::Alphanumeric};
use serde::{Deserialize, Serialize};
use tokio_stream::StreamExt;
use tower_http::cors::CorsLayer;
use tracing::{debug, info, warn};

use crate::config::Config;
use crate::embedding::Embedder;
use crate::retrieval::{HybridOptions, hybrid_search};
use crate::storage::Storage;

const TOKEN_HEADER: &str = "x-mnemonic-token";

#[derive(Clone)]
struct AppState {
    storage: Arc<Storage>,
    /// Shared with the daemon's event loop. The neural embedder loads an
    /// ONNX model — creating one per request made every /api/search pay
    /// a full model load before embedding a single query.
    embedder: Arc<dyn Embedder>,
    token: String,
    /// Snapshot of config at startup. Used by reextract to know which
    /// LLM backend to spin up. Not mutable — PATCH /api/config can land later.
    config: Config,
    /// Read-only handle on the work-activity DB, if it exists. Opened
    /// with `ActivityStore::open` (no session finalization) so dashboard
    /// reads never disturb the live sampler. None when activity has
    /// never run.
    activity: Option<Arc<crate::activity::ActivityStore>>,
}

/// Build the axum router. `bind_test` is a hook for tests to pass a
/// preset token instead of touching the filesystem.
fn router(state: AppState) -> Router {
    // Parse cors_origins from config into HeaderValues. Bad entries are
    // logged + skipped rather than refusing to start — typo shouldn't brick
    // the daemon. If the list ends up empty, falls back to defaults.
    let mut origin_values: Vec<HeaderValue> = Vec::new();
    for raw in &state.config.ui.cors_origins {
        match raw.parse::<HeaderValue>() {
            Ok(hv) => origin_values.push(hv),
            Err(e) => warn!("Skipping bad cors_origins entry {raw:?}: {e}"),
        }
    }
    if origin_values.is_empty() {
        warn!("cors_origins empty after parsing — falling back to localhost defaults");
        for s in [
            "http://localhost:5173",
            "http://127.0.0.1:5173",
            "http://localhost:3737",
            "http://127.0.0.1:3737",
        ] {
            origin_values.push(HeaderValue::from_static(s));
        }
    }

    let cors = CorsLayer::new()
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PATCH,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers([header::CONTENT_TYPE, HeaderName::from_static(TOKEN_HEADER)])
        // Explicit allowlist — replaces previous `Any`. Combined with the
        // 127.0.0.1 bind + Host header guard, this defends against DNS
        // rebinding: even if a malicious site resolves a hostname to
        // 127.0.0.1, the browser's CORS preflight won't match.
        .allow_origin(tower_http::cors::AllowOrigin::list(origin_values));

    Router::new()
        // Read endpoints
        .route("/api/status", get(handle_status))
        .route("/api/memories", get(handle_memories))
        .route("/api/memories/{id}", get(handle_memory_get))
        .route("/api/memories/{id}/sources", get(handle_memory_sources))
        .route("/api/entities", get(handle_entities))
        .route("/api/entities/{name}", get(handle_entity_get))
        .route("/api/graph", get(handle_graph))
        .route("/api/memory-graph", get(handle_memory_graph))
        .route("/api/stats/daily", get(handle_daily))
        .route("/api/activity/today", get(handle_activity_today))
        .route("/api/activity/week", get(handle_activity_week))
        .route("/api/activity/summary", get(handle_activity_summary))
        .route("/api/activity/day", get(handle_activity_day))
        .route("/api/activity/projects", get(handle_activity_projects))
        .route("/api/journal", get(handle_journal))
        .route("/api/search", post(handle_search))
        // Write endpoints — Phase 4b. All idempotent or guarded by
        // explicit confirm/apply/dry_run flags. None of these are streaming yet;
        // long jobs (reextract on the full DB) block — UI shows spinner.
        .route("/api/dedupe", post(handle_dedupe))
        .route("/api/reextract", post(handle_reextract))
        .route("/api/reextract/stream", post(handle_reextract_stream))
        .route("/api/cleanup", post(handle_cleanup))
        .route("/api/reflect", post(handle_reflect))
        .route("/api/entities/{name}/merge", post(handle_entity_merge))
        .route("/api/memories/{id}", axum::routing::delete(handle_forget))
        .layer(middleware::from_fn_with_state(state.clone(), auth))
        .layer(middleware::from_fn(host_guard))
        .layer(cors)
        // Cap request bodies so a crafted/buggy local client can't drive the
        // daemon into OOM or heavy embedding work with a giant payload. 256 KB
        // is far above any legitimate search/reflect request.
        .layer(axum::extract::DefaultBodyLimit::max(256 * 1024))
        .with_state(state)
}

/// DNS-rebinding guard. The TCP listener binds to 127.0.0.1, but a malicious
/// website can still aim a fetch at `http://evil.example/` whose DNS A
/// record resolves to 127.0.0.1 — the browser sends `Host: evil.example`
/// and the OS routes the connection to our local port. CORS catches that
/// for browser fetches with credentials, but `no-cors` form posts, image
/// loads, and worker requests can still hit us. Require the Host header
/// to name localhost / 127.0.0.1 / [::1] (with an optional port). Anything
/// else gets a 421.
async fn host_guard(req: axum::http::Request<axum::body::Body>, next: Next) -> Response {
    let host = req
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !is_loopback_host(host) {
        return (
            StatusCode::MISDIRECTED_REQUEST,
            Json(serde_json::json!({"error": "host not allowed"})),
        )
            .into_response();
    }
    next.run(req).await
}

/// Pure host-check predicate, separated so it can be unit-tested without
/// constructing `middleware::Next` (which has no public constructor).
/// IPv6 hosts come in as `[::1]` (with brackets) and an optional `:port`;
/// IPv4 / DNS hosts come in as `host:port`. We split on the *last* colon,
/// then trim brackets, and only accept the loopback hostnames.
fn is_loopback_host(host: &str) -> bool {
    if host.is_empty() {
        return false;
    }
    // IPv6 literal in brackets — bracket span is authoritative.
    let trimmed = if let Some(end) = host.find(']') {
        // "[::1]" or "[::1]:3737" → "::1"
        host.get(1..end).unwrap_or("")
    } else if let Some((h, _port)) = host.rsplit_once(':') {
        // Reject IPv6 without brackets (multiple colons): rsplit gives
        // "::" for "::1", which isn't loopback — bail.
        if h.contains(':') {
            return false;
        }
        h
    } else {
        host
    };
    matches!(trimmed, "localhost" | "127.0.0.1" | "::1")
}

/// Start the dashboard HTTP server. Loads (or creates) the auth token,
/// then binds and serves forever. Caller spawns this on the tokio runtime.
pub async fn serve(
    config: Config,
    storage: Arc<Storage>,
    embedder: Arc<dyn Embedder>,
) -> Result<()> {
    let token = load_or_create_token(&config.ui.token_file)?;
    let bind = format!("127.0.0.1:{}", config.ui.port);
    let listener = tokio::net::TcpListener::bind(&bind).await?;
    // Log a non-reversible 32-bit hash of the token (DefaultHasher; not a
    // crypto digest — just enough to distinguish runs / confirm "yes this
    // matches the token I have") instead of token bytes. The token file is
    // mode 0600; daemon.log isn't a secret store but isn't a public bus
    // either, so even a prefix leak isn't harmless.
    let fingerprint = token_fingerprint(&token);
    info!("Dashboard API listening on http://{bind}  (token fp: {fingerprint})");
    // Read-only handle on activity.db (if it exists yet). Opened once at
    // startup; WAL + read-only open means it can't disturb the sampler.
    let activity = {
        let path = config.activity_db_path();
        if path.exists() {
            match crate::activity::ActivityStore::open(&path) {
                Ok(s) => Some(Arc::new(s)),
                Err(e) => {
                    warn!("Dashboard: activity store open failed, endpoints will 503: {e}");
                    None
                }
            }
        } else {
            None
        }
    };
    let app = router(AppState {
        storage,
        embedder,
        token,
        config: config.clone(),
        activity,
    });
    axum::serve(listener, app).await?;
    Ok(())
}

/// Auth middleware: reject anything without a matching token header.
/// Public health probe via `OPTIONS` / preflight passes through CORS layer.
async fn auth(
    State(state): State<AppState>,
    req: axum::http::Request<axum::body::Body>,
    next: Next,
) -> Response {
    if req.method() == Method::OPTIONS {
        return next.run(req).await;
    }
    let token_ok = req
        .headers()
        .get(TOKEN_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(|t| constant_time_eq(t, &state.token))
        .unwrap_or(false);
    if !token_ok {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": "missing or invalid X-Mnemonic-Token"})),
        )
            .into_response();
    }
    next.run(req).await
}

/// Constant-time string compare to avoid timing leaks on the token.
fn constant_time_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.as_bytes().iter().zip(b.as_bytes().iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Read the token file, or generate a fresh 32-char token if it doesn't exist.
///
/// Always enforces mode 0600 (owner read/write only) on the resulting file —
/// even if it existed before this version of mnemonic with looser perms.
/// Doesn't log the path at info level to avoid amplifying H1.
fn load_or_create_token(path: &std::path::Path) -> Result<String> {
    if path.exists() {
        let token = std::fs::read_to_string(path)?.trim().to_string();
        if !token.is_empty() {
            // Tighten perms on every read — handles tokens created by older
            // mnemonic versions, backups restored with looser perms, etc.
            tighten_token_perms(path);
            return Ok(token);
        }
        warn!("Token file existed but empty; regenerating");
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
        // Lock ~/.mnemonic to 0700 from the HTTP path too, so token security
        // doesn't depend on the unix-socket server having run first.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
        }
    }
    let token: String = rand::rng()
        .sample_iter(&Alphanumeric)
        .take(32)
        .map(char::from)
        .collect();
    std::fs::write(path, &token)?;
    tighten_token_perms(path);
    debug!("Generated dashboard API token at {}", path.display());
    Ok(token)
}

#[cfg(unix)]
fn tighten_token_perms(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = std::fs::metadata(path) {
        let current = meta.permissions().mode() & 0o777;
        if current != 0o600 {
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
    }
}

#[cfg(not(unix))]
fn tighten_token_perms(_path: &std::path::Path) {}

/// Stable short fingerprint for log output. Not a secret — just enough to
/// distinguish runs / confirm "yes this token matches the one I have".
fn token_fingerprint(token: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    token.hash(&mut h);
    format!("{:08x}", h.finish() as u32)
}

// ───────────────────────── handlers ─────────────────────────

#[derive(Serialize)]
struct StatusResponse {
    total_memories: usize,
    by_type: Vec<TypeCount>,
    entities: usize,
    edges: usize,
    last_activity: Option<String>,
}

#[derive(Serialize)]
struct TypeCount {
    memory_type: String,
    count: usize,
}

async fn handle_status(State(state): State<AppState>) -> Result<Json<StatusResponse>, AppError> {
    let stats = state.storage.stats()?;
    let (entities, edges) = state.storage.graph_stats()?;
    let last = state.storage.last_activity()?;
    Ok(Json(StatusResponse {
        total_memories: stats.total,
        by_type: stats
            .by_type
            .into_iter()
            .map(|(t, c)| TypeCount {
                memory_type: t,
                count: c,
            })
            .collect(),
        entities,
        edges,
        last_activity: last,
    }))
}

/// GET /api/activity/today — today's worked total + session count.
async fn handle_activity_today(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, AppError> {
    let act = state.activity.as_ref().ok_or_else(|| {
        AppError(
            StatusCode::SERVICE_UNAVAILABLE,
            "activity tracking not initialized yet".to_string(),
        )
    })?;
    let secs = act
        .seconds_on_local_day(0)
        .map_err(|e| AppError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let sessions = act
        .session_count_today()
        .map_err(|e| AppError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(serde_json::json!({
        "seconds": secs.round() as i64,
        "human": crate::activity::fmt_hm(secs),
        "sessions": sessions,
    })))
}

#[derive(Deserialize)]
struct WeekQuery {
    #[serde(default = "default_week_days")]
    days: u32,
}
fn default_week_days() -> u32 {
    7
}

/// GET /api/activity/week?days=N — per-day worked totals (oldest first,
/// dense — zero days included) for the history graph.
async fn handle_activity_week(
    State(state): State<AppState>,
    Query(q): Query<WeekQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let act = state.activity.as_ref().ok_or_else(|| {
        AppError(
            StatusCode::SERVICE_UNAVAILABLE,
            "activity tracking not initialized yet".to_string(),
        )
    })?;
    let days = q.days.clamp(1, 366);
    let totals = act
        .daily_totals(days)
        .map_err(|e| AppError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let series: Vec<_> = totals
        .iter()
        .map(|t| {
            serde_json::json!({
                "date": t.date,
                "seconds": t.seconds.round() as i64,
                "human": crate::activity::fmt_hm(t.seconds),
            })
        })
        .collect();
    let total: f64 = totals.iter().map(|t| t.seconds).sum();
    Ok(Json(serde_json::json!({
        "days": series,
        "total_seconds": total.round() as i64,
        "total_human": crate::activity::fmt_hm(total),
    })))
}

/// GET /api/activity/summary — the widget's whole main screen in one
/// payload (worked-today, live session, week stats, today's
/// detail/timeline, 7-day chart).
async fn handle_activity_summary(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, AppError> {
    let act = state.activity.as_ref().ok_or_else(|| {
        AppError(
            StatusCode::SERVICE_UNAVAILABLE,
            "activity tracking not initialized yet".to_string(),
        )
    })?;
    let v = act
        .summary_value()
        .map_err(|e| AppError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(v))
}

/// GET /api/activity/projects — projects (graph entities) with memory counts,
/// latest memories, and attributed hours (today / week / week[7] + confidence)
/// merged from activity.db, plus the unattributed bucket.
async fn handle_activity_projects(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, AppError> {
    let time = state.activity.as_ref().and_then(|a| a.project_time().ok());
    let payload = state
        .storage
        .projects_payload(12, 5, time.as_ref())
        .map_err(|e| AppError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(payload))
}

#[derive(Deserialize)]
struct DayQuery {
    /// Local date YYYY-MM-DD; defaults to today.
    date: Option<String>,
}

#[derive(Deserialize)]
struct JournalQuery {
    /// Local date YYYY-MM-DD; defaults to today.
    day: Option<String>,
}

/// GET /api/journal?day=YYYY-MM-DD — the day's readable digest: summary,
/// per-project hours + bullets (with confidence), decisions, follow-ups, and
/// the honest unattributed bucket. Deterministic; defaults to today.
async fn handle_journal(
    State(state): State<AppState>,
    Query(q): Query<JournalQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let act = state.activity.as_ref().ok_or_else(|| {
        AppError(
            StatusCode::SERVICE_UNAVAILABLE,
            "activity tracking not initialized yet".to_string(),
        )
    })?;
    let day = match q.day {
        Some(s) => chrono::NaiveDate::parse_from_str(&s, "%Y-%m-%d")
            .map_err(|_| AppError(StatusCode::BAD_REQUEST, format!("bad day {s:?}")))?,
        None => chrono::Local::now().date_naive(),
    };
    let digest = crate::journal::collect(&state.storage, act, day)
        .map_err(|e| AppError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(serde_json::to_value(digest).unwrap_or_default()))
}

/// GET /api/activity/day?date=YYYY-MM-DD — one day's detail + session
/// timeline blocks. Defaults to today.
async fn handle_activity_day(
    State(state): State<AppState>,
    Query(q): Query<DayQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let act = state.activity.as_ref().ok_or_else(|| {
        AppError(
            StatusCode::SERVICE_UNAVAILABLE,
            "activity tracking not initialized yet".to_string(),
        )
    })?;
    let day = match q.date {
        Some(s) => chrono::NaiveDate::parse_from_str(&s, "%Y-%m-%d")
            .map_err(|_| AppError(StatusCode::BAD_REQUEST, format!("bad date {s:?}")))?,
        None => chrono::Local::now().date_naive(),
    };
    let v = act
        .day_value(day)
        .map_err(|e| AppError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(v))
}

#[derive(Deserialize)]
struct MemoriesQuery {
    #[serde(default = "default_limit")]
    limit: usize,
}
fn default_limit() -> usize {
    50
}

async fn handle_memories(
    State(state): State<AppState>,
    Query(q): Query<MemoriesQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let entries = state.storage.recent(q.limit.min(500))?;
    Ok(Json(serde_json::json!({
        "results": entries.iter().map(memory_to_json).collect::<Vec<_>>(),
        "count": entries.len(),
    })))
}

async fn handle_memory_get(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    match state.storage.get_by_id(&id)? {
        Some(entry) => Ok(Json(memory_to_json(&entry))),
        None => Err(AppError(
            StatusCode::NOT_FOUND,
            format!("memory {id} not found"),
        )),
    }
}

/// Provenance trail: if `id` is a canonical memory created by reflection,
/// returns the source memories that were consolidated into it. Empty
/// array if there are no sources. Useful for the UI to show "this
/// summary consolidates 3 older memories".
async fn handle_memory_sources(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let pairs = state.storage.sources_for_canonical(&id)?;
    let mut out = Vec::with_capacity(pairs.len());
    for (sid, cosine) in &pairs {
        if let Some(entry) = state.storage.get_by_id(sid)? {
            let mut json = memory_to_json(&entry);
            if let Some(obj) = json.as_object_mut() {
                obj.insert("cosine".into(), serde_json::json!(cosine));
            }
            out.push(json);
        }
    }
    Ok(Json(serde_json::json!({
        "canonical_id": id,
        "sources": out,
        "count": out.len(),
    })))
}

#[derive(Deserialize)]
struct EntitiesQuery {
    #[serde(default = "default_limit")]
    limit: usize,
}

async fn handle_entities(
    State(state): State<AppState>,
    Query(q): Query<EntitiesQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let list = state.storage.list_entities(q.limit.min(500))?;
    let entities: Vec<_> = list
        .into_iter()
        .map(|(name, etype, count)| {
            serde_json::json!({"name": name, "type": etype, "mentions": count})
        })
        .collect();
    Ok(Json(serde_json::json!({
        "results": entities,
    })))
}

async fn handle_entity_get(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let g = state.storage.graph_query(&name)?;
    Ok(Json(serde_json::to_value(g)?))
}

async fn handle_graph(
    State(state): State<AppState>,
    Query(q): Query<EntitiesQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    // Nodes from list_entities (already sorted by mention_count DESC).
    let cap = q.limit.min(500);
    let nodes_raw = state.storage.list_entities(cap)?;
    let name_set: std::collections::HashSet<String> =
        nodes_raw.iter().map(|(n, _, _)| n.to_lowercase()).collect();

    // Edges via per-entity neighborhood query, deduped by (s, t, r).
    let mut edges_seen: std::collections::HashSet<(String, String, String)> =
        std::collections::HashSet::new();
    let mut edges_out: Vec<serde_json::Value> = Vec::new();
    for (name, _, _) in &nodes_raw {
        let g = state.storage.graph_query(name)?;
        for e in &g.edges {
            let key = (e.source.clone(), e.target.clone(), e.relation.clone());
            if !edges_seen.insert(key) {
                continue;
            }
            // Both endpoints must be in the visible node set so the UI
            // doesn't have to handle dangling refs.
            if !name_set.contains(&e.source) || !name_set.contains(&e.target) {
                continue;
            }
            edges_out.push(serde_json::json!({
                "source": e.source,
                "target": e.target,
                "relation": e.relation,
                "weight": e.weight,
            }));
        }
    }

    Ok(Json(serde_json::json!({
        "nodes": nodes_raw.iter().map(|(name, etype, count)| {
            serde_json::json!({"name": name, "type": etype, "mentions": count})
        }).collect::<Vec<_>>(),
        "edges": edges_out,
    })))
}

#[derive(Deserialize)]
struct DailyQuery {
    #[serde(default = "default_days")]
    days: usize,
}
fn default_days() -> usize {
    14
}

#[derive(Deserialize)]
struct MemoryGraphQuery {
    #[serde(default = "default_memory_graph_limit")]
    limit: usize,
    #[serde(default)]
    since_days: Option<i64>,
    #[serde(default, rename = "type")]
    memory_type: Option<String>,
    #[serde(default)]
    q: Option<String>,
    #[serde(default = "default_min_shared")]
    min_shared: usize,
}
fn default_memory_graph_limit() -> usize {
    40
}
fn default_min_shared() -> usize {
    1
}

/// Memory-centric graph: every active memory becomes a node, edges form
/// where memories share at least `min_shared` entities (weight = shared count).
/// This is the "Obsidian-style notes graph" view, distinct from
/// /api/graph which is the entity knowledge graph.
///
/// Edges are generated via an inverted entity→memories index so the cost
/// scales with shared-entity actual co-occurrence, not all-pairs N².
async fn handle_memory_graph(
    State(state): State<AppState>,
    Query(q): Query<MemoryGraphQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let cap = q.limit.clamp(1, 400);
    let memories = state.storage.memory_graph_nodes(
        cap,
        q.since_days,
        q.memory_type.as_deref(),
        q.q.as_deref(),
    )?;
    let min_shared = q.min_shared.max(1);

    let nodes: Vec<serde_json::Value> = memories
        .iter()
        .map(|(m, ents)| {
            let preview: String = m.content.chars().take(140).collect();
            serde_json::json!({
                "id": m.id,
                "title": m.title,
                "memory_type": m.memory_type.to_string(),
                "timestamp": m.timestamp.to_rfc3339(),
                "importance": m.importance,
                "entity_count": ents.len(),
                "content_preview": preview,
            })
        })
        .collect();

    // Inverted index: entity → list of memory indices that link to it.
    // Generates candidate pairs only via shared bucket walk, skipping the
    // all-pairs O(n²) loop.
    use std::collections::{BTreeMap, HashMap};
    let mut entity_to_mems: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, (_, ents)) in memories.iter().enumerate() {
        for ent in ents {
            entity_to_mems.entry(ent.as_str()).or_default().push(i);
        }
    }

    // Aggregate shared entity names per pair via BTreeMap keyed by ordered
    // (min_idx, max_idx) so each pair is counted once.
    let mut pair_shared: BTreeMap<(usize, usize), Vec<String>> = BTreeMap::new();
    for (entity, mem_ids) in &entity_to_mems {
        if mem_ids.len() < 2 {
            continue; // entity touches a single memory — no edge contribution
        }
        for a in 0..mem_ids.len() {
            for b in (a + 1)..mem_ids.len() {
                let (lo, hi) = (mem_ids[a].min(mem_ids[b]), mem_ids[a].max(mem_ids[b]));
                pair_shared
                    .entry((lo, hi))
                    .or_default()
                    .push((*entity).to_string());
            }
        }
    }

    let edges: Vec<serde_json::Value> = pair_shared
        .into_iter()
        .filter(|(_, shared)| shared.len() >= min_shared)
        .map(|((lo, hi), shared)| {
            serde_json::json!({
                "source": memories[lo].0.id,
                "target": memories[hi].0.id,
                "weight": shared.len(),
                "shared": shared,
            })
        })
        .collect();

    Ok(Json(serde_json::json!({
        "nodes": nodes,
        "edges": edges,
    })))
}

async fn handle_daily(
    State(state): State<AppState>,
    Query(q): Query<DailyQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let counts = state.storage.daily_counts(q.days.min(365))?;
    Ok(Json(serde_json::json!({
        "days": counts.iter().map(|(d, c)| serde_json::json!({"date": d, "count": c})).collect::<Vec<_>>()
    })))
}

#[derive(Deserialize)]
struct SearchBody {
    query: String,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    with_graph_hop: Option<bool>,
}

async fn handle_search(
    State(state): State<AppState>,
    Json(body): Json<SearchBody>,
) -> Result<Json<serde_json::Value>, AppError> {
    let limit = body.limit.unwrap_or(10).min(100);
    let with_graph_hop = body.with_graph_hop.unwrap_or(true);
    let opts = HybridOptions {
        limit,
        with_graph_hop,
        ..Default::default()
    };
    // Embedding + the SQLite walk are CPU-bound blocking work — run them
    // off the async runtime so one slow search can't stall every other
    // dashboard request sharing the worker.
    let storage = state.storage.clone();
    let embedder = state.embedder.clone();
    let query = body.query.clone();
    let hits =
        tokio::task::spawn_blocking(move || hybrid_search(&storage, &*embedder, &query, &opts))
            .await
            .map_err(|e| AppError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))??;
    let results: Vec<_> = hits
        .iter()
        .map(|hit| {
            serde_json::json!({
                "id": hit.entry.id,
                "title": hit.entry.title,
                "content_preview": hit.entry.content.chars().take(200).collect::<String>(),
                "memory_type": hit.entry.memory_type.to_string(),
                "timestamp": hit.entry.timestamp.to_rfc3339(),
                "rrf_score": hit.score,
                "sources": hit.source_label(),
            })
        })
        .collect();
    Ok(Json(serde_json::json!({
        "results": results,
        "count": results.len(),
    })))
}

// ───────────────────────── write handlers ─────────────────────────

#[derive(Default, Deserialize)]
struct DedupeBody {
    #[serde(default)]
    apply: bool,
}

#[derive(Serialize)]
struct DedupeReport {
    groups: Vec<DedupeGroup>,
    dry_run: bool,
    merged: usize,
    renamed: usize,
    edges_redirected: usize,
    memory_links_redirected: usize,
}

#[derive(Serialize)]
struct DedupeGroup {
    canonical: String,
    variants: Vec<String>,
}

async fn handle_dedupe(
    State(state): State<AppState>,
    body: Option<Json<DedupeBody>>,
) -> Result<Json<DedupeReport>, AppError> {
    use crate::graph::canonical::canonicalize_name;

    let body = body.map(|Json(body)| body).unwrap_or_default();
    let dry_run = !body.apply;

    let names = state.storage.list_entity_names()?;
    let mut groups: std::collections::BTreeMap<String, Vec<String>> =
        std::collections::BTreeMap::new();
    for n in &names {
        let canon = canonicalize_name(n);
        if canon.is_empty() {
            continue;
        }
        groups.entry(canon).or_default().push(n.clone());
    }

    let mut plan: Vec<DedupeGroup> = Vec::new();
    for (canonical, variants) in &groups {
        if variants.len() == 1 && &variants[0] == canonical {
            continue;
        }
        plan.push(DedupeGroup {
            canonical: canonical.clone(),
            variants: variants.clone(),
        });
    }

    if dry_run {
        return Ok(Json(DedupeReport {
            groups: plan,
            dry_run: true,
            merged: 0,
            renamed: 0,
            edges_redirected: 0,
            memory_links_redirected: 0,
        }));
    }

    let mut merged = 0usize;
    let mut renamed = 0usize;
    let mut edges_redirected = 0usize;
    let mut memory_links_redirected = 0usize;

    for group in &plan {
        let canonical_exists = group.variants.iter().any(|v| v == &group.canonical);
        let needs_rename = !canonical_exists;
        if needs_rename
            && let Some(to_rename) = group.variants.first()
            && state.storage.rename_entity(to_rename, &group.canonical)?
        {
            renamed += 1;
        }
        for variant in &group.variants {
            if variant == &group.canonical {
                continue;
            }
            let report = state.storage.merge_entities(&group.canonical, variant)?;
            if report.alias_dropped {
                merged += 1;
                edges_redirected += report.edges_redirected;
                memory_links_redirected += report.memory_links_redirected;
            }
        }
    }

    Ok(Json(DedupeReport {
        groups: plan,
        dry_run: false,
        merged,
        renamed,
        edges_redirected,
        memory_links_redirected,
    }))
}

#[derive(Deserialize)]
struct ReextractBody {
    #[serde(default)]
    since_days: Option<i64>,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Serialize)]
struct ReextractReport {
    planned: usize,
    processed: usize,
    entities_added: usize,
    edges_added: usize,
    dry_run: bool,
    extractor: String,
}

async fn handle_reextract(
    State(state): State<AppState>,
    Json(body): Json<ReextractBody>,
) -> Result<Json<ReextractReport>, AppError> {
    use crate::graph::extractor::{EntityExtractor, RuleExtractor};

    let ids = state
        .storage
        .list_memory_ids(body.since_days, body.limit, false)?;
    let planned = ids.len();

    if body.dry_run {
        return Ok(Json(ReextractReport {
            planned,
            processed: 0,
            entities_added: 0,
            edges_added: 0,
            dry_run: true,
            extractor: if state.config.llm.enabled {
                format!("composite (rule + {})", state.config.llm.model)
            } else {
                "rule-based".into()
            },
        }));
    }

    let extractor: Box<dyn EntityExtractor> = if state.config.llm.enabled {
        match crate::graph::extractor_llm::OllamaBackend::new(&state.config.llm) {
            Ok(backend) => {
                let llm = crate::graph::extractor_llm::LlmExtractor::new(
                    Box::new(backend),
                    state.storage.clone(),
                    &state.config.llm,
                );
                Box::new(crate::graph::extractor_llm::CompositeExtractor::new(
                    Box::new(RuleExtractor::new()),
                    Box::new(llm),
                ))
            }
            Err(_) => Box::new(RuleExtractor::new()),
        }
    } else {
        Box::new(RuleExtractor::new())
    };

    // Reextract is CPU+LLM heavy; run on blocking pool so the tokio
    // reactor stays responsive for other endpoints.
    let storage = state.storage.clone();
    let extractor_id = if state.config.llm.enabled {
        format!("composite (rule + {})", state.config.llm.model)
    } else {
        "rule-based".into()
    };
    let (processed, entities_added, edges_added) =
        tokio::task::spawn_blocking(move || -> anyhow::Result<(usize, usize, usize)> {
            let mut processed = 0usize;
            let mut entities_added = 0usize;
            let mut edges_added = 0usize;
            for id in &ids {
                let Some(entry) = storage.get_by_id(id)? else {
                    continue;
                };
                let result = extractor.extract(&entry);
                let n_e = result.entities.len();
                let n_r = result.edges.len();
                if storage
                    .replace_graph_and_reconcile_projects(&entry, &result.entities, &result.edges)
                    .is_ok()
                {
                    entities_added += n_e;
                    edges_added += n_r;
                }
                processed += 1;
            }
            Ok((processed, entities_added, edges_added))
        })
        .await
        .map_err(|e| AppError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))??;

    Ok(Json(ReextractReport {
        planned,
        processed,
        entities_added,
        edges_added,
        dry_run: false,
        extractor: extractor_id,
    }))
}

#[derive(Deserialize)]
struct CleanupBody {
    #[serde(default = "default_cleanup_days")]
    days: i64,
    #[serde(default = "default_cleanup_threshold")]
    threshold: f32,
    #[serde(default)]
    confirm: bool,
}
fn default_cleanup_days() -> i64 {
    30
}
fn default_cleanup_threshold() -> f32 {
    0.5
}

/// SSE-streaming variant of /api/reextract. Each processed memory emits
/// a `progress` event; final event is `done` with totals. UI shows live
/// progress bar instead of blocking on a long POST.
///
/// Event format (text/event-stream):
///   event: progress
///   data: {"processed":N,"total":T,"title":"...","entities_added":x,"edges_added":y}
///
///   event: done
///   data: {"processed":N,"entities_added":...,"edges_added":...}
///
///   event: error
///   data: {"message":"..."}
async fn handle_reextract_stream(
    State(state): State<AppState>,
    Json(body): Json<ReextractBody>,
) -> axum::response::sse::Sse<
    impl tokio_stream::Stream<
        Item = std::result::Result<axum::response::sse::Event, std::convert::Infallible>,
    >,
> {
    use crate::graph::extractor::{EntityExtractor, RuleExtractor};
    use axum::response::sse::{Event, KeepAlive};

    let (tx, rx) = tokio::sync::mpsc::channel::<Event>(64);

    // Build the extractor on the runtime thread (cheap), then move into
    // spawn_blocking which iterates the DB.
    let storage = state.storage.clone();
    let cfg_llm = state.config.llm.clone();
    let llm_enabled = cfg_llm.enabled;
    let llm_model = cfg_llm.model.clone();

    tokio::spawn(async move {
        let ids = match storage.list_memory_ids(body.since_days, body.limit, false) {
            Ok(v) => v,
            Err(e) => {
                let _ = tx
                    .send(
                        Event::default()
                            .event("error")
                            .data(serde_json::json!({"message": e.to_string()}).to_string()),
                    )
                    .await;
                return;
            }
        };
        let total = ids.len();

        if body.dry_run {
            let payload = serde_json::json!({
                "planned": total,
                "extractor": if llm_enabled { format!("composite (rule + {llm_model})") } else { "rule-based".to_string() },
                "dry_run": true,
            });
            let _ = tx
                .send(Event::default().event("done").data(payload.to_string()))
                .await;
            return;
        }

        let extractor: Box<dyn EntityExtractor + Send> = if llm_enabled {
            match crate::graph::extractor_llm::OllamaBackend::new(&cfg_llm) {
                Ok(backend) => {
                    let llm = crate::graph::extractor_llm::LlmExtractor::new(
                        Box::new(backend),
                        storage.clone(),
                        &cfg_llm,
                    );
                    Box::new(crate::graph::extractor_llm::CompositeExtractor::new(
                        Box::new(RuleExtractor::new()),
                        Box::new(llm),
                    ))
                }
                Err(_) => Box::new(RuleExtractor::new()),
            }
        } else {
            Box::new(RuleExtractor::new())
        };

        // The actual loop runs on a blocking thread so the channel send
        // (across the async boundary) stays responsive. We bridge via
        // a sync mpsc → forwarded into the async one.
        let (sync_tx, mut sync_rx) = tokio::sync::mpsc::channel::<Event>(64);
        let storage_for_block = storage.clone();
        let blocking = tokio::task::spawn_blocking(move || {
            let mut processed = 0usize;
            let mut entities_added = 0usize;
            let mut edges_added = 0usize;
            for id in &ids {
                let Ok(Some(entry)) = storage_for_block.get_by_id(id) else {
                    continue;
                };
                let result = extractor.extract(&entry);
                let n_e = result.entities.len();
                let n_r = result.edges.len();
                if storage_for_block
                    .replace_graph_and_reconcile_projects(&entry, &result.entities, &result.edges)
                    .is_ok()
                {
                    entities_added += n_e;
                    edges_added += n_r;
                }
                processed += 1;

                let payload = serde_json::json!({
                    "processed": processed,
                    "total": total,
                    "title": entry.title,
                    "memory_id": entry.id,
                    "entities_added_step": n_e,
                    "edges_added_step": n_r,
                    "entities_added_total": entities_added,
                    "edges_added_total": edges_added,
                });
                // Best-effort: if the receiver is gone (client disconnected),
                // stop processing.
                if sync_tx
                    .blocking_send(Event::default().event("progress").data(payload.to_string()))
                    .is_err()
                {
                    return (processed, entities_added, edges_added, true);
                }
            }
            (processed, entities_added, edges_added, false)
        });

        // Forward progress events as they come in.
        while let Some(evt) = sync_rx.recv().await {
            if tx.send(evt).await.is_err() {
                break;
            }
        }

        // Wait for the blocking task to finish so we can send the final
        // `done` event (or skip it if the client disconnected).
        if let Ok((processed, entities_added, edges_added, disconnected)) = blocking.await
            && !disconnected
        {
            let payload = serde_json::json!({
                "processed": processed,
                "total": total,
                "entities_added": entities_added,
                "edges_added": edges_added,
            });
            let _ = tx
                .send(Event::default().event("done").data(payload.to_string()))
                .await;
        }
    });

    let stream = tokio_stream::wrappers::ReceiverStream::new(rx).map(Ok);
    axum::response::sse::Sse::new(stream).keep_alive(KeepAlive::default())
}

async fn handle_cleanup(
    State(state): State<AppState>,
    Json(body): Json<CleanupBody>,
) -> Result<Json<serde_json::Value>, AppError> {
    if !body.confirm {
        return Ok(Json(serde_json::json!({
            "deleted": 0,
            "confirmed": false,
            "note": "send { confirm: true } to actually delete; decisions and feedback are always kept"
        })));
    }
    let deleted = state.storage.cleanup(body.days, body.threshold)?;
    Ok(Json(serde_json::json!({
        "deleted": deleted,
        "confirmed": true
    })))
}

#[derive(Deserialize)]
struct ReflectBody {
    #[serde(default)]
    apply: bool,
    #[serde(default = "default_reflect_threshold")]
    threshold: f32,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    since_days: Option<i64>,
}
fn default_reflect_threshold() -> f32 {
    crate::reflection::DEFAULT_THRESHOLD
}

/// Run reflection / consolidation. Default is dry-run; set apply=true to
/// actually create canonicals and supersede sources. Source memories are
/// never deleted — only marked superseded — and `/api/memories/{id}/sources`
/// reveals the provenance trail.
async fn handle_reflect(
    State(state): State<AppState>,
    Json(body): Json<ReflectBody>,
) -> Result<Json<serde_json::Value>, AppError> {
    use crate::reflection::{Mode, ReflectionOptions, run_reflection};
    let opts = ReflectionOptions {
        mode: if body.apply {
            Mode::Apply
        } else {
            Mode::DryRun
        },
        threshold: body.threshold,
        limit: body.limit,
        since_days: body.since_days,
    };
    let plan = run_reflection(&state.storage, &state.config, &opts)?;
    Ok(Json(serde_json::to_value(plan)?))
}

#[derive(Deserialize)]
struct MergeBody {
    into: String,
}

async fn handle_entity_merge(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(body): Json<MergeBody>,
) -> Result<Json<serde_json::Value>, AppError> {
    use crate::graph::canonical::canonicalize_name;

    let alias = canonicalize_name(&name);
    let canonical = canonicalize_name(&body.into);
    if alias.is_empty() || canonical.is_empty() {
        return Err(AppError(
            StatusCode::BAD_REQUEST,
            "alias and canonical must canonicalize to non-empty names".into(),
        ));
    }
    if alias == canonical {
        return Err(AppError(
            StatusCode::BAD_REQUEST,
            "alias and canonical are the same after canonicalization".into(),
        ));
    }

    // Promote canonical if it doesn't exist yet (uncommon path — usually
    // the caller is merging existing entities).
    let canonical_exists = state.storage.graph_query(&canonical)?.found;
    if !canonical_exists {
        state.storage.rename_entity(&alias, &canonical)?;
        return Ok(Json(serde_json::json!({
            "action": "renamed",
            "from": alias,
            "to": canonical,
        })));
    }

    let report = state.storage.merge_entities(&canonical, &alias)?;
    Ok(Json(serde_json::json!({
        "action": "merged",
        "alias": alias,
        "canonical": canonical,
        "alias_dropped": report.alias_dropped,
        "edges_redirected": report.edges_redirected,
        "memory_links_redirected": report.memory_links_redirected,
    })))
}

async fn handle_forget(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let removed = state.storage.forget_by_id(&id)?;
    if !removed {
        return Err(AppError(
            StatusCode::NOT_FOUND,
            format!("memory {id} not found"),
        ));
    }
    Ok(Json(serde_json::json!({"id": id, "removed": true})))
}

fn memory_to_json(entry: &crate::event::MemoryEntry) -> serde_json::Value {
    serde_json::json!({
        "id": entry.id,
        "timestamp": entry.timestamp.to_rfc3339(),
        "title": entry.title,
        "content": entry.content,
        "memory_type": entry.memory_type.to_string(),
        "tags": entry.tags,
        "importance": entry.importance,
    })
}

// ───────────────────────── errors ─────────────────────────

/// Lightweight error type — pairs a status code with a message body.
#[derive(Debug)]
struct AppError(StatusCode, String);

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        (self.0, Json(serde_json::json!({"error": self.1}))).into_response()
    }
}

impl From<anyhow::Error> for AppError {
    fn from(e: anyhow::Error) -> Self {
        AppError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
    }
}

impl From<serde_json::Error> for AppError {
    fn from(e: serde_json::Error) -> Self {
        AppError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedding::HashEmbedder;
    use crate::graph::{Entity, EntityType};
    use axum::body::{Body, to_bytes};
    use tower::Service;

    fn tmp_db() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("mnemonic-http-test-{}.db", uuid::Uuid::new_v4()))
    }

    fn test_state(storage: Arc<Storage>) -> AppState {
        AppState {
            storage,
            embedder: Arc::new(HashEmbedder::new()),
            token: "test-token".to_string(),
            config: Config::default(),
            activity: None,
        }
    }

    fn seed_duplicate_entities(storage: &Storage) {
        storage
            .upsert_entity(&Entity {
                name: "acme-devices".into(),
                entity_type: EntityType::Project,
            })
            .unwrap();
        storage
            .upsert_entity(&Entity {
                name: "acme-devices-co".into(),
                entity_type: EntityType::Project,
            })
            .unwrap();
    }

    fn dedupe_request(body: Option<&str>) -> axum::http::Request<Body> {
        let mut builder = axum::http::Request::builder()
            .method(Method::POST)
            .uri("/api/dedupe")
            .header(header::HOST, "localhost")
            .header(TOKEN_HEADER, "test-token");
        if body.is_some() {
            builder = builder.header(header::CONTENT_TYPE, "application/json");
        }
        builder
            .body(body.map_or_else(Body::empty, |body| Body::from(body.to_string())))
            .unwrap()
    }

    async fn response_json(response: Response) -> serde_json::Value {
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[test]
    fn constant_time_eq_compares_correctly() {
        assert!(constant_time_eq("abc", "abc"));
        assert!(!constant_time_eq("abc", "abd"));
        assert!(!constant_time_eq("abc", "abcd"));
        assert!(!constant_time_eq("", "x"));
        assert!(constant_time_eq("", ""));
    }

    #[test]
    fn token_generated_on_missing_file() {
        let dir = std::env::temp_dir().join(format!(
            "mnemonic-http-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let token_path = dir.join("auth.token");
        let token = load_or_create_token(&token_path).unwrap();
        assert_eq!(token.len(), 32);
        // Re-reading must return the same token, not generate a new one.
        let again = load_or_create_token(&token_path).unwrap();
        assert_eq!(token, again);
    }

    #[cfg(unix)]
    #[test]
    fn token_file_is_chmod_0600_after_load() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!(
            "mnemonic-http-test-perm-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let token_path = dir.join("auth.token");
        // Write a token with deliberately loose perms (0644) as if from an
        // older mnemonic version or a restored backup.
        std::fs::write(&token_path, "abc1234567890123456789012345defg").unwrap();
        std::fs::set_permissions(&token_path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let _ = load_or_create_token(&token_path).unwrap();
        let mode = std::fs::metadata(&token_path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "token file must be tightened to 0600 on load");
    }

    #[test]
    fn token_fingerprint_is_stable_and_short() {
        let fp1 = token_fingerprint("hello-world-token");
        let fp2 = token_fingerprint("hello-world-token");
        assert_eq!(fp1, fp2, "same input → same fingerprint");
        assert_eq!(fp1.len(), 8, "fingerprint is 8 hex chars");
        let fp3 = token_fingerprint("hello-world-tokeN");
        assert_ne!(fp1, fp3, "different input → different fingerprint");
    }

    #[test]
    fn loopback_host_accepts_localhost_variants() {
        for host in [
            "localhost",
            "localhost:3737",
            "127.0.0.1",
            "127.0.0.1:5173",
            "[::1]",
            "[::1]:3737",
        ] {
            assert!(
                is_loopback_host(host),
                "{host} should be treated as loopback"
            );
        }
    }

    #[test]
    fn loopback_host_rejects_foreign() {
        for host in [
            "",
            "evil.example",
            "evil.example:3737",
            "169.254.169.254",
            "kossvat.com",
            "::1", // unbracketed IPv6 must be rejected — ambiguous with host:port
        ] {
            assert!(
                !is_loopback_host(host),
                "{host} should NOT be treated as loopback"
            );
        }
    }

    #[tokio::test]
    async fn dedupe_without_body_or_apply_false_is_dry_run_and_does_not_mutate() {
        let storage = Arc::new(Storage::open(&tmp_db()).unwrap());
        seed_duplicate_entities(&storage);
        let mut app = router(test_state(storage.clone()));

        let report = response_json(app.call(dedupe_request(None)).await.unwrap()).await;
        assert_eq!(report["dry_run"], true, "missing body must be a dry-run");
        assert_eq!(report["merged"], 0);
        assert!(storage.graph_query("acme-devices").unwrap().found);
        assert!(storage.graph_query("acme-devices-co").unwrap().found);

        let report = response_json(app.call(dedupe_request(Some("{}"))).await.unwrap()).await;
        assert_eq!(
            report["dry_run"], true,
            "empty/apply=false body must be a dry-run"
        );
        assert_eq!(report["merged"], 0);
        assert!(storage.graph_query("acme-devices").unwrap().found);
        assert!(storage.graph_query("acme-devices-co").unwrap().found);
    }

    #[tokio::test]
    async fn dedupe_apply_true_mutates_duplicate_entities() {
        let storage = Arc::new(Storage::open(&tmp_db()).unwrap());
        seed_duplicate_entities(&storage);
        let mut app = router(test_state(storage.clone()));

        let report = response_json(
            app.call(dedupe_request(Some(r#"{"apply":true}"#)))
                .await
                .unwrap(),
        )
        .await;

        assert_eq!(report["dry_run"], false);
        assert_eq!(report["merged"], 1);
        assert!(storage.graph_query("acme-devices").unwrap().found);
        assert!(!storage.graph_query("acme-devices-co").unwrap().found);
    }
}
