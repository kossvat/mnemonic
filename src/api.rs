use anyhow::Result;
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::Bytes;
use hyper::{Request, Response, body::Incoming, server::conn::http1, service::service_fn};
use hyper_util::rt::TokioIo;
use std::sync::Arc;
use tokio::net::UnixListener;
use tracing::{debug, error, info};

use crate::embedding::Embedder;
use crate::storage::Storage;

/// Caps for POST /embed (design-reviewed): keep the endpoint narrow so a
/// misbehaving client can't OOM the daemon through the socket.
const EMBED_MAX_BODY_BYTES: usize = 256 * 1024;
const EMBED_MAX_TEXTS: usize = 32;
const EMBED_MAX_TOTAL_CHARS: usize = 64 * 1024;

/// Unix socket API server for CLI and MCP integration
pub struct ApiServer {
    socket_path: std::path::PathBuf,
    storage: Arc<Storage>,
    embedder: Arc<dyn Embedder>,
    /// Serializes /embed model calls: ONNX already parallelizes internally,
    /// and unbounded concurrent embeds would oversubscribe threads and spike
    /// memory (the exact failure this endpoint exists to prevent).
    embed_gate: Arc<tokio::sync::Semaphore>,
}

impl ApiServer {
    pub fn new(
        socket_path: std::path::PathBuf,
        storage: Arc<Storage>,
        embedder: Arc<dyn Embedder>,
    ) -> Self {
        Self {
            socket_path,
            storage,
            embedder,
            embed_gate: Arc::new(tokio::sync::Semaphore::new(1)),
        }
    }

    pub async fn start(self) -> Result<()> {
        // Clean up stale socket
        let _ = std::fs::remove_file(&self.socket_path);

        // Lock down parent dir to 0o700 (user-only) before binding — on multi-user
        // hosts, the default 0o755 would let other local accounts connect to the
        // socket and read/write memory entries.
        #[cfg(unix)]
        if let Some(parent) = self.socket_path.parent() {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
        }

        let listener = UnixListener::bind(&self.socket_path)?;

        // Restrict socket to owner only (0o600). Without this, bind inherits
        // umask, typically leaving the socket 0o755 and world-connectable.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&self.socket_path, std::fs::Permissions::from_mode(0o600))?;
        }

        info!("API listening on {}", self.socket_path.display());

        let storage = self.storage;
        let embedder = self.embedder;
        let embed_gate = self.embed_gate;

        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    let storage = storage.clone();
                    let embedder = embedder.clone();
                    let embed_gate = embed_gate.clone();
                    tokio::spawn(async move {
                        let io = TokioIo::new(stream);
                        let svc = service_fn(move |req| {
                            let storage = storage.clone();
                            let embedder = embedder.clone();
                            let embed_gate = embed_gate.clone();
                            async move { handle_request(req, storage, embedder, embed_gate).await }
                        });

                        if let Err(e) = http1::Builder::new().serve_connection(io, svc).await {
                            // Liveness probes (mnemonic doctor / status)
                            // connect to the socket and immediately
                            // close — produces "error shutting down
                            // connection" / IncompleteMessage at the
                            // hyper layer. Those aren't real errors,
                            // just early-close. Demote to debug. Codex
                            // caught this: live daemon log was filling
                            // with ERROR-tagged liveness noise.
                            //
                            // Heuristic: hyper's `Error::is_incomplete_message`
                            // covers the "client closed before we got a
                            // full request" case (probe_socket pattern).
                            // The text "shutdown" / "shutting down"
                            // catches the close-during-response variant.
                            let msg = e.to_string();
                            if e.is_incomplete_message()
                                || msg.contains("shutting down")
                                || msg.contains("shutdown")
                            {
                                debug!("Connection ended early (probe?): {e}");
                            } else {
                                error!("Connection error: {e}");
                            }
                        }
                    });
                }
                Err(e) => {
                    error!("Accept error: {e}");
                }
            }
        }
    }
}

async fn handle_request(
    req: Request<Incoming>,
    storage: Arc<Storage>,
    embedder: Arc<dyn Embedder>,
    embed_gate: Arc<tokio::sync::Semaphore>,
) -> Result<Response<Full<Bytes>>, hyper::Error> {
    let path = req.uri().path().to_string();
    let method = req.method().clone();

    let response = match (method.as_str(), path.as_str()) {
        ("GET", "/status") => {
            let stats = storage
                .stats()
                .unwrap_or_else(|_| crate::storage::StorageStats {
                    total: 0,
                    by_type: vec![],
                });
            let body = serde_json::json!({
                "status": "running",
                "memories": stats.total,
                "by_type": stats.by_type.iter()
                    .map(|(t, c)| serde_json::json!({ "type": t, "count": c }))
                    .collect::<Vec<_>>(),
                // Model metadata so clients (MCP) can log daemon/client
                // divergence without loading a model themselves.
                "embedding": {
                    "model_id": embedder.model_id(),
                    "dim": embedder.dim_hint(),
                },
            });
            json_response(200, &body)
        }
        ("POST", "/embed") => handle_embed(req, embedder, embed_gate).await,
        ("GET", path) if path.starts_with("/query/") => {
            let query = path.trim_start_matches("/query/");
            let query = urlencoding::decode(query).unwrap_or_default();
            match storage.search(&query, 10) {
                Ok(entries) => {
                    let body = serde_json::json!({
                        "results": entries.iter().map(|e| serde_json::json!({
                            "title": e.title,
                            "content": e.content,
                            "type": e.memory_type.to_string(),
                            "tags": e.tags,
                            "importance": e.importance,
                            "timestamp": e.timestamp.to_rfc3339(),
                        })).collect::<Vec<_>>(),
                        "count": entries.len(),
                    });
                    json_response(200, &body)
                }
                Err(e) => json_response(500, &serde_json::json!({ "error": e.to_string() })),
            }
        }
        ("GET", "/recent") => match storage.recent(20) {
            Ok(entries) => {
                let body = serde_json::json!({
                    "results": entries.iter().map(|e| serde_json::json!({
                        "title": e.title,
                        "type": e.memory_type.to_string(),
                        "importance": e.importance,
                        "timestamp": e.timestamp.to_rfc3339(),
                    })).collect::<Vec<_>>(),
                });
                json_response(200, &body)
            }
            Err(e) => json_response(500, &serde_json::json!({ "error": e.to_string() })),
        },
        _ => json_response(404, &serde_json::json!({ "error": "not found" })),
    };

    Ok(response)
}

/// POST /embed — embed texts with the daemon's already-loaded model, so MCP
/// processes never load their own copy (~2 GB transient each).
///
/// Request:  {"texts": ["..."], "kind": "query"|"passage"}
/// Response: {"embedding_api_version": 1, "model_id": "...", "dim": N,
///            "vectors": [[f32...]]}
async fn handle_embed(
    req: Request<Incoming>,
    embedder: Arc<dyn Embedder>,
    embed_gate: Arc<tokio::sync::Semaphore>,
) -> Response<Full<Bytes>> {
    // Body cap FIRST — an unbounded collect() would let any local client
    // OOM the daemon through the socket.
    let body = match Limited::new(req.into_body(), EMBED_MAX_BODY_BYTES)
        .collect()
        .await
    {
        Ok(collected) => collected.to_bytes(),
        Err(_) => {
            return json_response(
                400,
                &serde_json::json!({ "error": "request body missing or too large" }),
            );
        }
    };

    let (texts, is_query) = match parse_embed_request(&body) {
        Ok(v) => v,
        Err(msg) => return json_response(400, &serde_json::json!({ "error": msg })),
    };

    // Serialize model access; ONNX embedding is blocking CPU work, so it
    // runs on the blocking pool, not the async workers.
    let _permit = match embed_gate.acquire().await {
        Ok(p) => p,
        Err(_) => return json_response(500, &serde_json::json!({ "error": "gate closed" })),
    };
    let emb = embedder.clone();
    let result = tokio::task::spawn_blocking(move || {
        texts
            .iter()
            .map(|t| {
                if is_query {
                    emb.embed_query(t)
                } else {
                    emb.embed(t)
                }
            })
            .collect::<anyhow::Result<Vec<_>>>()
    })
    .await;

    match result {
        Ok(Ok(vectors)) => {
            let dim = vectors.first().map(|v| v.len()).unwrap_or(0);
            json_response(
                200,
                &serde_json::json!({
                    "embedding_api_version": 1,
                    "model_id": embedder.model_id(),
                    "dim": dim,
                    "vectors": vectors,
                }),
            )
        }
        Ok(Err(e)) => json_response(500, &serde_json::json!({ "error": e.to_string() })),
        Err(e) => json_response(500, &serde_json::json!({ "error": format!("join: {e}") })),
    }
}

/// Validate the /embed request body. Pure so it unit-tests without hyper.
/// Returns (texts, is_query).
fn parse_embed_request(body: &[u8]) -> Result<(Vec<String>, bool), String> {
    let value: serde_json::Value =
        serde_json::from_slice(body).map_err(|e| format!("bad JSON: {e}"))?;
    let kind = value
        .get("kind")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing 'kind' (\"query\" or \"passage\")".to_string())?;
    let is_query = match kind {
        "query" => true,
        "passage" => false,
        other => return Err(format!("bad 'kind': {other}")),
    };
    let texts: Vec<String> = value
        .get("texts")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "missing 'texts' array".to_string())?
        .iter()
        .map(|t| {
            t.as_str()
                .map(str::to_string)
                .ok_or_else(|| "texts must be strings".to_string())
        })
        .collect::<Result<_, _>>()?;
    if texts.is_empty() {
        return Err("texts is empty".into());
    }
    if texts.len() > EMBED_MAX_TEXTS {
        return Err(format!("too many texts (max {EMBED_MAX_TEXTS})"));
    }
    let total: usize = texts.iter().map(|t| t.len()).sum();
    if total > EMBED_MAX_TOTAL_CHARS {
        return Err(format!(
            "texts too large (max {EMBED_MAX_TOTAL_CHARS} bytes total)"
        ));
    }
    Ok((texts, is_query))
}

fn json_response(status: u16, body: &serde_json::Value) -> Response<Full<Bytes>> {
    let payload = body.to_string();
    Response::builder()
        .status(status)
        .header("Content-Type", "application/json")
        // Explicit Content-Length: the hand-rolled MCP client frames by it
        // and rejects chunked responses.
        .header("Content-Length", payload.len())
        .body(Full::new(Bytes::from(payload)))
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embed_request_happy_path() {
        let (texts, is_query) =
            parse_embed_request(br#"{"texts":["a","b"],"kind":"query"}"#).unwrap();
        assert_eq!(texts, vec!["a", "b"]);
        assert!(is_query);
        let (_, is_query) = parse_embed_request(br#"{"texts":["a"],"kind":"passage"}"#).unwrap();
        assert!(!is_query);
    }

    #[test]
    fn embed_request_rejects_bad_input() {
        assert!(parse_embed_request(b"not json").is_err());
        assert!(parse_embed_request(br#"{"texts":[],"kind":"query"}"#).is_err());
        assert!(parse_embed_request(br#"{"texts":["a"],"kind":"banana"}"#).is_err());
        assert!(parse_embed_request(br#"{"texts":["a"]}"#).is_err());
        assert!(parse_embed_request(br#"{"kind":"query"}"#).is_err());
        assert!(parse_embed_request(br#"{"texts":[1,2],"kind":"query"}"#).is_err());

        let too_many: Vec<String> = (0..EMBED_MAX_TEXTS + 1).map(|i| i.to_string()).collect();
        let body = serde_json::json!({ "texts": too_many, "kind": "query" }).to_string();
        assert!(parse_embed_request(body.as_bytes()).is_err());

        let huge = "x".repeat(EMBED_MAX_TOTAL_CHARS + 1);
        let body = serde_json::json!({ "texts": [huge], "kind": "query" }).to_string();
        assert!(parse_embed_request(body.as_bytes()).is_err());
    }
}
