use super::*;
use crate::event::{EventSource, MemoryType};
use chrono::Utc;
use std::sync::Mutex;

/// Mock backend that returns a canned JSON response. Lets us test
/// parsing, merging, and caching without spinning up an HTTP server.
struct CannedBackend {
    response: Mutex<String>,
    calls: Mutex<usize>,
}

impl CannedBackend {
    fn new(response: &str) -> Self {
        Self {
            response: Mutex::new(response.to_string()),
            calls: Mutex::new(0),
        }
    }
    fn call_count(&self) -> usize {
        *self.calls.lock().unwrap()
    }
}

impl LlmBackend for CannedBackend {
    fn generate(&self, _prompt: &str) -> anyhow::Result<String> {
        *self.calls.lock().unwrap() += 1;
        Ok(self.response.lock().unwrap().clone())
    }
}

fn entry(title: &str, content: &str) -> MemoryEntry {
    MemoryEntry {
        id: uuid::Uuid::new_v4().to_string(),
        timestamp: Utc::now(),
        title: title.into(),
        content: content.into(),
        memory_type: MemoryType::Note,
        tags: vec![],
        source: EventSource::Manual,
        importance: 0.5,
        metadata: serde_json::Value::Null,
    }
}

fn tmp_storage() -> crate::test_support::InTempDir<std::sync::Arc<Storage>> {
    crate::test_support::InTempDir::new("mnemonic-llm-test-", |dir| {
        std::sync::Arc::new(Storage::open(&dir.join("memory.db")).unwrap())
    })
}

fn cfg() -> LlmConfig {
    LlmConfig {
        enabled: true,
        endpoint: String::new(),
        model: "qwen2.5:3b".into(),
        timeout_secs: 5,
        min_chars: 0,
    }
}

#[test]
fn parses_canned_response() {
    let resp = r#"{
        "entities": [
            {"name": "sample-service", "entity_type": "project"},
            {"name": "rust", "entity_type": "tech"}
        ],
        "relations": [
            {"source": "sample-service", "target": "rust", "relation": "uses"}
        ]
    }"#;
    let backend = CannedBackend::new(resp);
    let storage = tmp_storage();
    let ex = LlmExtractor::new(Box::new(backend), storage.clone(), &cfg());
    let r = ex.extract(&entry("Sample service", "Sample service uses Rust."));
    assert_eq!(r.entities.len(), 2);
    assert_eq!(r.edges.len(), 1);
    assert_eq!(r.edges[0].relation, "uses");
}

#[test]
fn second_extract_hits_cache_not_backend() {
    let resp = r#"{"entities":[{"name":"a","entity_type":"concept"}],"relations":[]}"#;
    let backend = std::sync::Arc::new(CannedBackend::new(resp));
    let storage = tmp_storage();
    // Wrap in an adapter that lets us peek at call count after.
    struct Adapter(std::sync::Arc<CannedBackend>);
    impl LlmBackend for Adapter {
        fn generate(&self, p: &str) -> anyhow::Result<String> {
            self.0.generate(p)
        }
    }
    let ex = LlmExtractor::new(Box::new(Adapter(backend.clone())), storage.clone(), &cfg());
    let e = entry("title", "some content long enough");
    let _ = ex.extract(&e);
    let _ = ex.extract(&e);
    assert_eq!(
        backend.call_count(),
        1,
        "second extract on identical content must hit cache"
    );
}

#[test]
fn invalid_json_returns_empty_result() {
    let backend = CannedBackend::new("not json at all");
    let storage = tmp_storage();
    let ex = LlmExtractor::new(Box::new(backend), storage.clone(), &cfg());
    let e = entry("t", "content content");
    let r = ex.extract(&e);
    assert!(r.entities.is_empty());
    assert!(r.edges.is_empty());
    // Failed extractions must land in the retry queue so we don't
    // silently lose memories during a model swap or backend hiccup.
    assert_eq!(
        storage.pending_extractions_count().unwrap(),
        1,
        "JSON parse failure should enqueue for retry"
    );
    let row = storage.pending_row(&e.id).unwrap().unwrap();
    assert_eq!(row.1.as_deref(), Some("GENERATED_JSON_INVALID"));
}

/// Backend errors (Ollama unreachable, timeout, etc.) also enqueue.
#[test]
fn backend_error_enqueues_for_retry() {
    struct DeadBackend;
    impl LlmBackend for DeadBackend {
        fn generate(&self, _p: &str) -> anyhow::Result<String> {
            anyhow::bail!("connection refused")
        }
    }
    let storage = tmp_storage();
    let ex = LlmExtractor::new(Box::new(DeadBackend), storage.clone(), &cfg());
    let e = entry("t", "content content");
    let _ = ex.extract(&e);
    assert_eq!(storage.pending_extractions_count().unwrap(), 1);
    let row = storage.pending_row(&e.id).unwrap().unwrap();
    assert_eq!(row.1.as_deref(), Some("BACKEND_FAILED"));
}

/// Successful extraction must drop any prior pending row — otherwise the
/// queue would grow forever as eventually-successful memories pile up.
#[test]
fn success_drops_pending_row() {
    let storage = tmp_storage();
    // Seed a pending row, then run a successful extraction.
    let e = entry("title", "content content for retry test");
    storage
        .enqueue_pending_extraction(&e.id, "earlier backend failure")
        .unwrap();
    assert_eq!(storage.pending_extractions_count().unwrap(), 1);

    let resp = r#"{"entities":[{"name":"sample-service","entity_type":"project"}],"relations":[]}"#;
    let backend = CannedBackend::new(resp);
    let ex = LlmExtractor::new(Box::new(backend), storage.clone(), &cfg());
    let r = ex.extract(&e);
    assert_eq!(r.entities.len(), 1);
    assert_eq!(
        storage.pending_extractions_count().unwrap(),
        0,
        "successful extraction should clear the retry queue entry"
    );
}

#[test]
fn drops_edges_to_unknown_endpoints() {
    // Endpoint "ghost" isn't in entities — must be filtered.
    let resp = r#"{
        "entities": [{"name": "real", "entity_type": "concept"}],
        "relations": [{"source": "real", "target": "ghost", "relation": "uses"}]
    }"#;
    let backend = CannedBackend::new(resp);
    let storage = tmp_storage();
    let ex = LlmExtractor::new(Box::new(backend), storage.clone(), &cfg());
    let r = ex.extract(&entry("t", "content content"));
    assert_eq!(r.entities.len(), 1);
    assert!(r.edges.is_empty(), "edge to ghost endpoint must be dropped");
}

#[test]
fn applies_rule_quality_filter_to_llm_output() {
    // The LLM emits a denied concept, a generic pronoun, and a random id
    // token alongside one real entity. Only the real one survives — the
    // LLM output goes through the same gate as the rule extractor.
    let resp = r#"{
        "entities": [
            {"name": "decision", "entity_type": "concept"},
            {"name": "you", "entity_type": "person"},
            {"name": "q7x2k9m4p1z8w3r6t5y0v2b8", "entity_type": "concept"},
            {"name": "redis", "entity_type": "tech"}
        ],
        "relations": []
    }"#;
    let backend = CannedBackend::new(resp);
    let storage = tmp_storage();
    let ex = LlmExtractor::new(Box::new(backend), storage.clone(), &cfg());
    let r = ex.extract(&entry("t", "content content here"));
    let names: Vec<&str> = r.entities.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["redis"],
        "only the real entity survives, got {names:?}"
    );
}

#[test]
fn min_chars_skips_short_entries() {
    let backend = std::sync::Arc::new(CannedBackend::new("{}"));
    struct Adapter(std::sync::Arc<CannedBackend>);
    impl LlmBackend for Adapter {
        fn generate(&self, p: &str) -> anyhow::Result<String> {
            self.0.generate(p)
        }
    }
    let storage = tmp_storage();
    let mut c = cfg();
    c.min_chars = 100;
    let ex = LlmExtractor::new(Box::new(Adapter(backend.clone())), storage.clone(), &c);
    let _ = ex.extract(&entry("t", "short"));
    assert_eq!(
        backend.call_count(),
        0,
        "below min_chars must not call backend"
    );
}

#[test]
fn composite_merges_dedup_by_name() {
    // Rule-based returns "rust", LLM returns "rust" + "tokio".
    // Composite should yield 2 entities, not 3.
    struct RuleStub;
    impl EntityExtractor for RuleStub {
        fn extract(&self, _: &MemoryEntry) -> ExtractionResult {
            ExtractionResult {
                entities: vec![Entity {
                    name: "rust".into(),
                    entity_type: EntityType::Tech,
                }],
                edges: vec![],
            }
        }
    }
    let resp = r#"{
        "entities": [
            {"name": "rust", "entity_type": "tech"},
            {"name": "tokio", "entity_type": "tech"}
        ],
        "relations": []
    }"#;
    let storage = tmp_storage();
    let llm = LlmExtractor::new(Box::new(CannedBackend::new(resp)), storage.clone(), &cfg());
    let composite = CompositeExtractor::new(Box::new(RuleStub), Box::new(llm));
    let r = composite.extract(&entry("t", "long enough content here"));
    assert_eq!(r.entities.len(), 2);
    let names: Vec<&str> = r.entities.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"rust"));
    assert!(names.contains(&"tokio"));
}
