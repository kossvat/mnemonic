use anyhow::Result;
use http_body_util::Full;
use hyper::body::Bytes;
use hyper::{Request, Response, body::Incoming, server::conn::http1, service::service_fn};
use hyper_util::rt::TokioIo;
use std::sync::Arc;
use tokio::net::UnixListener;
use tracing::{debug, error, info};

use crate::storage::Storage;

/// Unix socket API server for CLI and MCP integration
pub struct ApiServer {
    socket_path: std::path::PathBuf,
    storage: Arc<Storage>,
}

impl ApiServer {
    pub fn new(socket_path: std::path::PathBuf, storage: Arc<Storage>) -> Self {
        Self {
            socket_path,
            storage,
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

        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    let storage = storage.clone();
                    tokio::spawn(async move {
                        let io = TokioIo::new(stream);
                        let svc = service_fn(move |req| {
                            let storage = storage.clone();
                            async move { handle_request(req, storage).await }
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
            });
            json_response(200, &body)
        }
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

fn json_response(status: u16, body: &serde_json::Value) -> Response<Full<Bytes>> {
    Response::builder()
        .status(status)
        .header("Content-Type", "application/json")
        .body(Full::new(Bytes::from(body.to_string())))
        .unwrap()
}
