//! Client side of the daemon's `POST /embed` endpoint.
//!
//! The MCP server runs one process per Claude Code session. Before this
//! module, every session that embedded (memory_similar / memory_save) loaded
//! its own copy of the e5 ONNX model (~2 GB transient RSS each) — several
//! sessions embedding at once stalled a 16 GB machine. Now MCP asks the
//! daemon (which already holds the model for its ingest pipeline) for
//! embeddings over the existing 0600 unix socket, so the machine holds ONE
//! model copy no matter how many sessions are open.
//!
//! Design reviewed with Codex (2026-07-08). Key requirements it set:
//! - hand-rolled HTTP/1.1 client, deliberately narrow: Content-Length
//!   framing only (reject chunked), case-insensitive headers, hard caps on
//!   header/body size, explicit timeouts, `Connection: close`.
//! - error classification instead of "fallback on any error":
//!   transport-unavailable → bounded retries then local fallback with a
//!   negative cache; HTTP 400 → hard fail; 503 (model loading) → retry,
//!   fallback only after; dim mismatch → hard fail LATCHED for the process
//!   lifetime, never fallback (falling back would hide daemon/MCP
//!   split-brain and mix incompatible vectors in the store).

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use anyhow::Result;
use serde::Deserialize;
use tracing::{debug, info, warn};

use super::{Embedder, Embedding, LazyEmbedder};

/// Which e5 instruction the daemon should apply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EmbedKind {
    Passage,
    Query,
}

impl EmbedKind {
    fn as_str(self) -> &'static str {
        match self {
            EmbedKind::Passage => "passage",
            EmbedKind::Query => "query",
        }
    }
}

/// Classified failure of a daemon embed call. The variant decides the
/// fallback policy, so classification IS the contract — don't collapse
/// these into a stringly error.
#[derive(Debug)]
pub enum EmbedClientError {
    /// Could not reach the daemon (connect/io error, or negative-cached).
    Unavailable(String),
    /// Daemon reachable but the model is not ready yet (HTTP 503).
    Loading,
    /// Daemon rejected the request as malformed (HTTP 400) — client bug,
    /// never fall back.
    BadRequest(String),
    /// Daemon returned vectors of a different dimension than this store
    /// expects — daemon/MCP split-brain. Hard fail, latched.
    Mismatch { got_dim: usize, expected_dim: usize },
    /// Daemon-side failure (HTTP 5xx) or a malformed response.
    Server(String),
}

impl std::fmt::Display for EmbedClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EmbedClientError::Unavailable(m) => write!(f, "daemon unavailable: {m}"),
            EmbedClientError::Loading => write!(f, "daemon embedding model still loading"),
            EmbedClientError::BadRequest(m) => write!(f, "daemon rejected embed request: {m}"),
            EmbedClientError::Mismatch {
                got_dim,
                expected_dim,
            } => write!(
                f,
                "embedding dimension mismatch: daemon returned {got_dim}, store expects \
                 {expected_dim}. Daemon and MCP builds have diverged — restart the daemon \
                 (and reembed if the model changed)."
            ),
            EmbedClientError::Server(m) => write!(f, "daemon embed failed: {m}"),
        }
    }
}

impl std::error::Error for EmbedClientError {}

/// Successful embed response.
#[derive(Debug, Deserialize)]
struct EmbedResponse {
    #[allow(dead_code)]
    #[serde(default)]
    embedding_api_version: u32,
    #[serde(default)]
    model_id: String,
    dim: usize,
    vectors: Vec<Vec<f32>>,
}

const HEADER_CAP: usize = 8 * 1024;
const BODY_CAP: usize = 4 * 1024 * 1024;
const CONNECT_WRITE_TIMEOUT: Duration = Duration::from_secs(2);
const READ_TIMEOUT: Duration = Duration::from_secs(20);
/// After a final transport failure, skip the daemon entirely for this long
/// so a dead daemon doesn't add connect latency to every embed.
const NEGATIVE_CACHE: Duration = Duration::from_secs(60);
/// Bounded retries for Unavailable/Loading before giving up. The daemon
/// loads its model eagerly BEFORE binding the socket, so the common cold
/// window is "socket not there yet" — worth a short wait, not a long one.
const RETRY_BACKOFF: &[Duration] = &[Duration::from_millis(200), Duration::from_secs(1)];

/// Talks to the daemon's `/embed`; implements the retry/latch policy.
pub struct DaemonEmbedder {
    socket_path: PathBuf,
    /// Dimension of the active store (from `active_embedding_dims`), when
    /// unambiguous. A daemon answer of any other dimension latches Mismatch.
    expected_dim: Option<usize>,
    /// Once a Mismatch is seen, every later call fails fast with it.
    /// Process-lifetime latch: one MCP process serves one session, and a
    /// daemon restart implies new MCP processes soon after.
    poisoned: OnceLock<(usize, usize)>,
    /// Instant of the last final transport failure (negative cache).
    down_since: Mutex<Option<Instant>>,
    /// Log the daemon's model_id once, not per call.
    logged_model: OnceLock<()>,
}

impl DaemonEmbedder {
    pub fn new(socket_path: PathBuf, expected_dim: Option<usize>) -> Self {
        Self {
            socket_path,
            expected_dim,
            poisoned: OnceLock::new(),
            down_since: Mutex::new(None),
            logged_model: OnceLock::new(),
        }
    }

    /// Embed one text via the daemon, applying retries + caches.
    pub fn embed_one(&self, text: &str, kind: EmbedKind) -> Result<Embedding, EmbedClientError> {
        if let Some(&(got_dim, expected_dim)) = self.poisoned.get() {
            return Err(EmbedClientError::Mismatch {
                got_dim,
                expected_dim,
            });
        }
        if let Ok(guard) = self.down_since.lock()
            && let Some(t) = *guard
            && t.elapsed() < NEGATIVE_CACHE
        {
            return Err(EmbedClientError::Unavailable(
                "negative-cached (daemon was down recently)".into(),
            ));
        }

        let mut attempt = 0usize;
        loop {
            match self.request(text, kind) {
                Ok(resp) => {
                    if let Ok(mut guard) = self.down_since.lock() {
                        *guard = None;
                    }
                    return self.validate(resp);
                }
                Err(e @ (EmbedClientError::Unavailable(_) | EmbedClientError::Loading)) => {
                    if attempt < RETRY_BACKOFF.len() {
                        debug!("daemon embed attempt {attempt} failed ({e}), retrying");
                        std::thread::sleep(RETRY_BACKOFF[attempt]);
                        attempt += 1;
                        continue;
                    }
                    if matches!(e, EmbedClientError::Unavailable(_))
                        && let Ok(mut guard) = self.down_since.lock()
                    {
                        *guard = Some(Instant::now());
                    }
                    return Err(e);
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// Validate dims and latch on split-brain.
    fn validate(&self, resp: EmbedResponse) -> Result<Embedding, EmbedClientError> {
        let vector = resp
            .vectors
            .into_iter()
            .next()
            .ok_or_else(|| EmbedClientError::Server("empty vectors array".into()))?;
        if vector.len() != resp.dim {
            return Err(EmbedClientError::Server(format!(
                "vector length {} != reported dim {}",
                vector.len(),
                resp.dim
            )));
        }
        if let Some(expected) = self.expected_dim
            && resp.dim != expected
        {
            let _ = self.poisoned.set((resp.dim, expected));
            return Err(EmbedClientError::Mismatch {
                got_dim: resp.dim,
                expected_dim: expected,
            });
        }
        self.logged_model.get_or_init(|| {
            info!(
                "embedding via daemon: model={} dim={}",
                resp.model_id, resp.dim
            );
        });
        Ok(vector)
    }

    /// One HTTP round-trip. Classification of transport/protocol errors
    /// happens here; policy (retry/latch/cache) lives in `embed_one`.
    fn request(&self, text: &str, kind: EmbedKind) -> Result<EmbedResponse, EmbedClientError> {
        let body = serde_json::json!({ "texts": [text], "kind": kind.as_str() }).to_string();

        let mut stream = UnixStream::connect(&self.socket_path)
            .map_err(|e| EmbedClientError::Unavailable(e.to_string()))?;
        stream
            .set_write_timeout(Some(CONNECT_WRITE_TIMEOUT))
            .and_then(|_| stream.set_read_timeout(Some(READ_TIMEOUT)))
            .map_err(|e| EmbedClientError::Unavailable(e.to_string()))?;

        let request = format!(
            "POST /embed HTTP/1.1\r\nHost: mnemonic\r\nContent-Type: application/json\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        stream
            .write_all(request.as_bytes())
            .map_err(|e| EmbedClientError::Unavailable(format!("write: {e}")))?;

        let (status, body) = read_http_response(&mut stream)?;
        match status {
            200 => serde_json::from_slice::<EmbedResponse>(&body)
                .map_err(|e| EmbedClientError::Server(format!("bad response JSON: {e}"))),
            400 => Err(EmbedClientError::BadRequest(body_text(&body))),
            503 => Err(EmbedClientError::Loading),
            s => Err(EmbedClientError::Server(format!(
                "HTTP {s}: {}",
                body_text(&body)
            ))),
        }
    }
}

fn body_text(body: &[u8]) -> String {
    String::from_utf8_lossy(body).chars().take(300).collect()
}

/// Minimal HTTP/1.1 response reader. Deliberately narrow (per design
/// review): requires Content-Length, rejects Transfer-Encoding, caps header
/// and body sizes, parses header names case-insensitively.
fn read_http_response(reader: &mut impl Read) -> Result<(u16, Vec<u8>), EmbedClientError> {
    // Read until end of headers (\r\n\r\n), capped.
    let mut buf: Vec<u8> = Vec::with_capacity(1024);
    let mut byte = [0u8; 1];
    let header_end = loop {
        match reader.read(&mut byte) {
            Ok(0) => {
                return Err(EmbedClientError::Server(
                    "connection closed before headers ended".into(),
                ));
            }
            Ok(_) => {
                buf.push(byte[0]);
                if buf.len() > HEADER_CAP {
                    return Err(EmbedClientError::Server(
                        "response headers too large".into(),
                    ));
                }
                if buf.len() >= 4 && &buf[buf.len() - 4..] == b"\r\n\r\n" {
                    break buf.len();
                }
            }
            Err(e) => return Err(EmbedClientError::Unavailable(format!("read: {e}"))),
        }
    };

    let head = String::from_utf8_lossy(&buf[..header_end]);
    let mut lines = head.split("\r\n");
    let status_line = lines
        .next()
        .ok_or_else(|| EmbedClientError::Server("empty response".into()))?;
    // "HTTP/1.1 200 OK"
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| EmbedClientError::Server(format!("bad status line: {status_line}")))?;

    let mut content_length: Option<usize> = None;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim().to_ascii_lowercase();
        let value = value.trim();
        match name.as_str() {
            "content-length" => {
                content_length = Some(value.parse().map_err(|_| {
                    EmbedClientError::Server(format!("bad content-length: {value}"))
                })?);
            }
            "transfer-encoding" => {
                return Err(EmbedClientError::Server(
                    "chunked/transfer-encoded responses are not supported".into(),
                ));
            }
            _ => {}
        }
    }

    let cl = content_length
        .ok_or_else(|| EmbedClientError::Server("response missing content-length".into()))?;
    if cl > BODY_CAP {
        return Err(EmbedClientError::Server(format!(
            "response body too large: {cl} bytes"
        )));
    }
    let mut body = vec![0u8; cl];
    reader
        .read_exact(&mut body)
        .map_err(|e| EmbedClientError::Unavailable(format!("read body: {e}")))?;
    Ok((status, body))
}

/// The embedder MCP actually uses: daemon first, local model as fallback.
///
/// Fallback policy (from the design review):
/// - Unavailable / Loading (after retries) / Server errors → warn once,
///   fall back to the local LazyEmbedder (loads the model in-process).
/// - BadRequest / Mismatch → propagate the error. Mismatch especially must
///   NOT fall back: it means daemon and MCP disagree about the model, and
///   silently embedding locally would mix incompatible vectors.
pub struct FallbackEmbedder {
    daemon: DaemonEmbedder,
    local: LazyEmbedder,
    warned: OnceLock<()>,
}

impl FallbackEmbedder {
    pub fn new(socket_path: PathBuf, expected_dim: Option<usize>) -> Self {
        Self {
            daemon: DaemonEmbedder::new(socket_path, expected_dim),
            local: LazyEmbedder::new(),
            warned: OnceLock::new(),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_local(
        socket_path: PathBuf,
        expected_dim: Option<usize>,
        local: LazyEmbedder,
    ) -> Self {
        Self {
            daemon: DaemonEmbedder::new(socket_path, expected_dim),
            local,
            warned: OnceLock::new(),
        }
    }

    fn embed_kind(&self, text: &str, kind: EmbedKind) -> Result<Embedding> {
        match self.daemon.embed_one(text, kind) {
            Ok(v) => Ok(v),
            Err(e @ (EmbedClientError::Mismatch { .. } | EmbedClientError::BadRequest(_))) => {
                Err(anyhow::Error::new(e))
            }
            Err(e) => {
                self.warned.get_or_init(|| {
                    warn!(
                        "daemon embed unavailable ({e}); falling back to in-process model \
                         (loads ~1-2GB transiently). Is the daemon running?"
                    );
                });
                match kind {
                    EmbedKind::Passage => self.local.embed(text),
                    EmbedKind::Query => self.local.embed_query(text),
                }
            }
        }
    }
}

impl Embedder for FallbackEmbedder {
    fn embed(&self, text: &str) -> Result<Embedding> {
        self.embed_kind(text, EmbedKind::Passage)
    }

    fn embed_query(&self, text: &str) -> Result<Embedding> {
        self.embed_kind(text, EmbedKind::Query)
    }

    fn model_id(&self) -> &'static str {
        "daemon-or-local"
    }
}

/// True when `err` is a daemon/MCP split-brain (dimension mismatch).
/// Production code uses `is_hard_failure` (mismatch OR bad request); this
/// finer check only distinguishes the two in tests.
#[cfg(test)]
fn is_mismatch(err: &anyhow::Error) -> bool {
    matches!(
        err.downcast_ref::<EmbedClientError>(),
        Some(EmbedClientError::Mismatch { .. })
    )
}

/// True when the embed failure is non-retryable and must surface to the
/// caller instead of degrading to an unembedded save: dimension mismatch
/// (split-brain) or a daemon 400 (e.g. text over the /embed caps). Both
/// would otherwise silently produce memories invisible to vector search
/// and dedup. (Codex review 2026-07-08: the BadRequest arm was originally
/// swallowed by the save path's catch-all.)
pub fn is_hard_failure(err: &anyhow::Error) -> bool {
    matches!(
        err.downcast_ref::<EmbedClientError>(),
        Some(EmbedClientError::Mismatch { .. } | EmbedClientError::BadRequest(_))
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::os::unix::net::UnixListener;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn http(status: &str, headers: &str, body: &str) -> Vec<u8> {
        format!(
            "HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\n\r\n{body}",
            body.len()
        )
        .into_bytes()
    }

    #[test]
    fn parses_simple_response() {
        let raw = http("200 OK", "Content-Type: application/json\r\n", "{\"a\":1}");
        let (status, body) = read_http_response(&mut Cursor::new(raw)).unwrap();
        assert_eq!(status, 200);
        assert_eq!(body, b"{\"a\":1}");
    }

    #[test]
    fn header_names_are_case_insensitive() {
        let raw = b"HTTP/1.1 200 OK\r\ncOnTeNt-LeNgTh: 2\r\n\r\nok".to_vec();
        let (status, body) = read_http_response(&mut Cursor::new(raw)).unwrap();
        assert_eq!(status, 200);
        assert_eq!(body, b"ok");
    }

    #[test]
    fn rejects_chunked() {
        let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec();
        let err = read_http_response(&mut Cursor::new(raw)).unwrap_err();
        assert!(matches!(err, EmbedClientError::Server(_)), "{err}");
    }

    #[test]
    fn rejects_missing_content_length() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n".to_vec();
        let err = read_http_response(&mut Cursor::new(raw)).unwrap_err();
        assert!(err.to_string().contains("content-length"), "{err}");
    }

    #[test]
    fn rejects_oversized_body_claim() {
        let raw = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
            BODY_CAP + 1
        );
        let err = read_http_response(&mut Cursor::new(raw.into_bytes())).unwrap_err();
        assert!(err.to_string().contains("too large"), "{err}");
    }

    /// Unique SHORT socket path. macOS caps unix socket paths at 104 bytes;
    /// temp_dir + a full UUID dir blows that, so use /tmp + a short suffix.
    fn tmp_sock() -> PathBuf {
        let short = &uuid::Uuid::new_v4().simple().to_string()[..12];
        PathBuf::from(format!("/tmp/mn-dc-{short}.sock"))
    }

    /// Fake daemon: accepts one connection, ignores the request, writes a
    /// canned response, closes. Returns None (test should skip) when the
    /// environment forbids AF_UNIX binds — sandboxed review/CI runners deny
    /// socket creation and the tests would otherwise die before testing
    /// anything (Codex review 2026-07-08).
    fn fake_daemon(sock: PathBuf, responses: Vec<Vec<u8>>) -> Option<PathBuf> {
        let _ = std::fs::remove_file(&sock);
        let listener = match UnixListener::bind(&sock) {
            Ok(l) => l,
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                eprintln!("skipping socket-backed test: unix bind denied in this sandbox ({e})");
                return None;
            }
            Err(e) => panic!("bind {}: {e}", sock.display()),
        };
        std::thread::spawn(move || {
            for resp in responses {
                let Ok((mut stream, _)) = listener.accept() else {
                    return;
                };
                // Drain the request headers+body enough to not RST the client.
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf);
                let _ = stream.write_all(&resp);
            }
        });
        Some(sock)
    }

    fn ok_response(dim: usize) -> Vec<u8> {
        let v: Vec<f32> = vec![0.5; dim];
        let body = serde_json::json!({
            "embedding_api_version": 1,
            "model_id": "test-model",
            "dim": dim,
            "vectors": [v],
        })
        .to_string();
        http("200 OK", "Content-Type: application/json\r\n", &body)
    }

    #[test]
    fn daemon_embedder_happy_path() {
        let Some(sock) = fake_daemon(tmp_sock(), vec![ok_response(8)]) else {
            return;
        };
        let d = DaemonEmbedder::new(sock, Some(8));
        let v = d.embed_one("hello", EmbedKind::Query).unwrap();
        assert_eq!(v.len(), 8);
        assert!((v[0] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn dim_mismatch_latches_and_never_falls_back() {
        let Some(sock) = fake_daemon(tmp_sock(), vec![ok_response(8)]) else {
            return;
        };

        let calls = std::sync::Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        let local = LazyEmbedder::from_builder(move || {
            c.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(crate::embedding::HashEmbedder::new()) as Box<dyn Embedder>)
        });
        // Store expects 16, daemon answers 8 → Mismatch, hard fail.
        let f = FallbackEmbedder::with_local(sock, Some(16), local);

        let err = f.embed("x").unwrap_err();
        assert!(is_mismatch(&err), "expected mismatch, got: {err}");
        // Latched: second call fails fast the same way (no server listening
        // for a second connection — must not even try).
        let err2 = f.embed_query("y").unwrap_err();
        assert!(is_mismatch(&err2), "expected latched mismatch, got: {err2}");
        // Local fallback must never have been built.
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "mismatch must not fall back"
        );
    }

    #[test]
    fn unavailable_falls_back_to_local() {
        let sock = tmp_sock(); // nothing listens

        let calls = std::sync::Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        let local = LazyEmbedder::from_builder(move || {
            c.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(crate::embedding::HashEmbedder::new()) as Box<dyn Embedder>)
        });
        let f = FallbackEmbedder::with_local(sock, Some(crate::embedding::EMBED_DIMS), local);

        let v = f.embed_query("hello world").unwrap();
        assert_eq!(v.len(), crate::embedding::EMBED_DIMS);
        assert_eq!(calls.load(Ordering::SeqCst), 1, "local fallback used");

        // Negative cache: second call goes straight to local without retry
        // delays (we can't measure time reliably here, but it must succeed
        // and not build the local embedder twice).
        let _ = f.embed("again").unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn bad_request_hard_fails_without_fallback() {
        // Daemon rejects the request (e.g. text over the /embed caps).
        // Save paths must see the error, NOT a silent local fallback and
        // NOT a silent unembedded save.
        let resp = http(
            "400 Bad Request",
            "Content-Type: application/json\r\n",
            r#"{"error":"texts too large"}"#,
        );
        let Some(sock) = fake_daemon(tmp_sock(), vec![resp]) else {
            return;
        };

        let calls = std::sync::Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        let local = LazyEmbedder::from_builder(move || {
            c.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(crate::embedding::HashEmbedder::new()) as Box<dyn Embedder>)
        });
        let f = FallbackEmbedder::with_local(sock, Some(crate::embedding::EMBED_DIMS), local);

        let err = f.embed("x").unwrap_err();
        assert!(is_hard_failure(&err), "400 must be a hard failure: {err}");
        assert!(!is_mismatch(&err), "400 is not a dim mismatch: {err}");
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "bad request must not fall back to the local model"
        );
    }
}
