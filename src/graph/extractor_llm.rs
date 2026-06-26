//! LLM-backed entity and relation extractor.
//!
//! Calls an Ollama-compatible JSON endpoint, expects a strict JSON schema,
//! and falls back silently on any error (network, parse, timeout). Results
//! are cached by sha-ish hash of (title|content|extractor_id) so repeated
//! identical memories don't re-burn LLM calls.
//!
//! Pure rule-based extraction still runs alongside via `CompositeExtractor`.
//! This module never panics — extraction failure must degrade gracefully.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::config::LlmConfig;
use crate::event::MemoryEntry;
use crate::graph::extractor::{EntityExtractor, ExtractionResult};
use crate::graph::{Edge, Entity, EntityType};
use crate::storage::Storage;

/// Strict JSON schema the LLM must return. Anything else is dropped.
#[derive(Debug, Deserialize, Serialize)]
struct LlmResponse {
    #[serde(default)]
    entities: Vec<LlmEntity>,
    #[serde(default)]
    relations: Vec<LlmRelation>,
}

#[derive(Debug, Deserialize, Serialize)]
struct LlmEntity {
    name: String,
    /// project | tech | module | file | concept | person — anything else → concept
    #[serde(default)]
    entity_type: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct LlmRelation {
    source: String,
    target: String,
    #[serde(default = "default_relation")]
    relation: String,
}

fn default_relation() -> String {
    "related_to".into()
}

/// Transport trait so tests can mock the backend without spinning up Ollama.
pub trait LlmBackend: Send + Sync {
    fn generate(&self, prompt: &str) -> anyhow::Result<String>;
}

/// Production backend: Ollama /api/generate with format=json.
pub struct OllamaBackend {
    endpoint: String,
    model: String,
    client: reqwest::blocking::Client,
}

impl OllamaBackend {
    pub fn new(cfg: &LlmConfig) -> anyhow::Result<Self> {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(cfg.timeout_secs))
            .build()?;
        Ok(Self {
            endpoint: cfg.endpoint.trim_end_matches('/').to_string(),
            model: cfg.model.clone(),
            client,
        })
    }
}

impl LlmBackend for OllamaBackend {
    fn generate(&self, prompt: &str) -> anyhow::Result<String> {
        // Ollama supports format=json to force valid JSON output.
        let body = serde_json::json!({
            "model": self.model,
            "prompt": prompt,
            "stream": false,
            "format": "json",
            "options": {
                // Lower temperature for stable structured output.
                "temperature": 0.1,
            },
        });
        let resp = self
            .client
            .post(format!("{}/api/generate", self.endpoint))
            .json(&body)
            .send()?
            .error_for_status()?;
        let value: serde_json::Value = resp.json()?;
        // Ollama wraps the model output in {"response": "..."}.
        Ok(value
            .get("response")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string())
    }
}

/// The LLM-driven extractor. Composes with the rule-based one in
/// `CompositeExtractor`.
pub struct LlmExtractor {
    backend: Box<dyn LlmBackend>,
    storage: std::sync::Arc<Storage>,
    extractor_id: String,
    min_chars: usize,
}

impl LlmExtractor {
    pub fn new(
        backend: Box<dyn LlmBackend>,
        storage: std::sync::Arc<Storage>,
        cfg: &LlmConfig,
    ) -> Self {
        // Identify cache entries by backend + model so swapping models
        // doesn't poison results from the previous one.
        let extractor_id = format!("ollama:{}", cfg.model);
        Self {
            backend,
            storage,
            extractor_id,
            min_chars: cfg.min_chars,
        }
    }
}

impl EntityExtractor for LlmExtractor {
    fn extract(&self, entry: &MemoryEntry) -> ExtractionResult {
        let combined = format!("{}\n{}", entry.title, entry.content);
        if combined.chars().count() < self.min_chars {
            return ExtractionResult::default();
        }

        let hash = content_hash(&combined);

        // Cache lookup. Treat parse failure as cache miss.
        if let Ok(Some(cached)) = self.storage.llm_cache_get(&hash, &self.extractor_id)
            && let Ok(parsed) = serde_json::from_str::<LlmResponse>(&cached)
        {
            debug!("LLM extractor cache hit ({})", &hash[..8]);
            return to_result(&parsed, &entry.id);
        }

        // Cache miss — call the model.
        let prompt = build_prompt(&entry.title, &entry.content);
        let raw = match self.backend.generate(&prompt) {
            Ok(s) => s,
            Err(e) => {
                warn!("LLM extractor backend error: {e}");
                // Queue for retry. Backend hiccups (Ollama not running, model
                // download in progress, network flake) shouldn't permanently
                // strand a memory in rule-only land.
                let _ = self
                    .storage
                    .enqueue_pending_extraction(&entry.id, &format!("backend: {e}"));
                return ExtractionResult::default();
            }
        };

        let parsed: LlmResponse = match serde_json::from_str(&raw) {
            Ok(v) => v,
            Err(e) => {
                warn!("LLM extractor JSON parse failed: {e}");
                // Same logic — a model that returned malformed JSON might
                // do better next time (different seed, or after a swap).
                let _ = self
                    .storage
                    .enqueue_pending_extraction(&entry.id, &format!("parse: {e}"));
                return ExtractionResult::default();
            }
        };

        // Store raw model output, not parsed — re-parsing is cheap and keeps
        // forward compat if we extend the response schema later.
        let _ = self.storage.llm_cache_put(&hash, &self.extractor_id, &raw);
        // Success — make sure the retry queue doesn't keep this around.
        let _ = self.storage.drop_pending_extraction(&entry.id);

        to_result(&parsed, &entry.id)
    }
}

/// Composite extractor — runs rule-based first, then LLM (if enabled),
/// merges entities by lowercased name to avoid duplicate nodes.
pub struct CompositeExtractor {
    rule_based: Box<dyn EntityExtractor>,
    llm: Box<dyn EntityExtractor>,
}

impl CompositeExtractor {
    pub fn new(rule_based: Box<dyn EntityExtractor>, llm: Box<dyn EntityExtractor>) -> Self {
        Self { rule_based, llm }
    }
}

impl EntityExtractor for CompositeExtractor {
    fn extract(&self, entry: &MemoryEntry) -> ExtractionResult {
        use crate::graph::canonical::canonicalize_name;

        let mut base = self.rule_based.extract(entry);
        let extra = self.llm.extract(entry);

        // Merge entities by canonical name (not just lowercase) so
        // "internal-crm" and "internal-crm" collapse instead of splitting the graph.
        // Rule-based wins on type conflict (deterministic; LLM noisy on types).
        let mut existing: std::collections::HashSet<String> = base
            .entities
            .iter()
            .map(|e| canonicalize_name(&e.name))
            .collect();
        for mut e in extra.entities {
            let canon = canonicalize_name(&e.name);
            if canon.is_empty() {
                continue;
            }
            // LlmExtractor already canonicalizes its output, so this is a
            // safety belt — keep entity.name canonical at the type boundary.
            e.name = canon.clone();
            if existing.insert(canon) {
                base.entities.push(e);
            }
        }

        // Edges: dedup by (canonical source, canonical target, lowercased
        // relation). Canonicalize endpoints so rule-based vs LLM variants
        // don't multiply edges.
        let mut existing_edges: std::collections::HashSet<(String, String, String)> = base
            .edges
            .iter()
            .map(|e| {
                (
                    canonicalize_name(&e.source),
                    canonicalize_name(&e.target),
                    e.relation.to_lowercase(),
                )
            })
            .collect();
        for mut e in extra.edges {
            let s = canonicalize_name(&e.source);
            let t = canonicalize_name(&e.target);
            let r = e.relation.to_lowercase();
            let key = (s.clone(), t.clone(), r.clone());
            if !existing_edges.insert(key) {
                continue;
            }
            e.source = s;
            e.target = t;
            e.relation = r;
            base.edges.push(e);
        }

        base
    }
}

/// Hash that's stable, deterministic, and dependency-free. Hex-encoded
/// u64 — collision risk is fine for an extraction cache (not a security
/// boundary).
fn content_hash(s: &str) -> String {
    let mut h = DefaultHasher::new();
    s.hash(&mut h);
    format!("{:016x}", h.finish())
}

fn build_prompt(title: &str, content: &str) -> String {
    format!(
        r#"You extract a knowledge graph from short memory entries.

Output ONLY valid JSON matching this schema:
{{
  "entities": [{{"name": "string", "entity_type": "project|tech|module|file|concept|person"}}],
  "relations": [{{"source": "string", "target": "string", "relation": "uses|depends_on|blocks|replaces|owns|costs|part_of|related_to"}}]
}}

Rules:
- Names are lowercase, hyphenated (e.g. "inventory-labeler", "internal-crm").
- entity_type defaults to "concept" if unsure.
- Only include relations where BOTH endpoints appear in entities.
- Skip generic stopwords (the, a, this, etc).
- Maximum 10 entities, 10 relations per entry.

Title: {title}
Content: {content}
"#
    )
}

fn map_entity_type(s: &str) -> EntityType {
    match s.to_lowercase().as_str() {
        "project" => EntityType::Project,
        "tech" | "technology" => EntityType::Tech,
        "module" => EntityType::Module,
        "file" => EntityType::File,
        "person" => EntityType::Person,
        _ => EntityType::Concept,
    }
}

fn to_result(parsed: &LlmResponse, memory_id: &str) -> ExtractionResult {
    use crate::graph::canonical::{canonicalize_name, is_attribute_like};

    // Canonicalize + dedup entities. Dropping attribute-like names ("-cost",
    // "-price", "-deadline") here prevents the graph from growing junk nodes.
    let mut entities: Vec<Entity> = Vec::with_capacity(parsed.entities.len());
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for e in &parsed.entities {
        let canon = canonicalize_name(&e.name);
        // Same quality + deny gate as the rule extractor, so the LLM can't
        // re-introduce denied concepts, generic pronouns, or junk id tokens
        // that the rule path already drops.
        if canon.is_empty()
            || is_attribute_like(&canon)
            || !crate::graph::extractor::RuleExtractor::is_valid_entity(&canon)
            || !seen.insert(canon.clone())
        {
            continue;
        }
        entities.push(Entity {
            name: canon,
            entity_type: map_entity_type(&e.entity_type),
        });
    }

    // Edge endpoints must canonicalize to known entities. LLMs both
    // hallucinate endpoints AND inconsistently abbreviate them — this
    // catches both.
    let mut edges = Vec::with_capacity(parsed.relations.len());
    for r in &parsed.relations {
        let s = canonicalize_name(&r.source);
        let t = canonicalize_name(&r.target);
        if s.is_empty() || t.is_empty() || s == t {
            continue;
        }
        if !seen.contains(&s) || !seen.contains(&t) {
            continue;
        }
        edges.push(Edge {
            source: s,
            target: t,
            relation: r.relation.trim().to_lowercase(),
            memory_id: memory_id.to_string(),
        });
    }

    ExtractionResult { entities, edges }
}

#[cfg(test)]
mod tests {
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

    fn tmp_storage() -> std::sync::Arc<Storage> {
        let dir = std::env::temp_dir().join(format!(
            "mnemonic-llm-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::sync::Arc::new(Storage::open(&dir.join("memory.db")).unwrap())
    }

    fn cfg() -> LlmConfig {
        LlmConfig {
            enabled: true,
            endpoint: "http://localhost:11434".into(),
            model: "qwen2.5:3b".into(),
            timeout_secs: 5,
            min_chars: 0,
        }
    }

    #[test]
    fn parses_canned_response() {
        let resp = r#"{
            "entities": [
                {"name": "inventory-labeler", "entity_type": "project"},
                {"name": "rust", "entity_type": "tech"}
            ],
            "relations": [
                {"source": "inventory-labeler", "target": "rust", "relation": "uses"}
            ]
        }"#;
        let backend = CannedBackend::new(resp);
        let storage = tmp_storage();
        let ex = LlmExtractor::new(Box::new(backend), storage, &cfg());
        let r = ex.extract(&entry(
            "Inventory labeler",
            "Inventory labeler is written in Rust.",
        ));
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
        let ex = LlmExtractor::new(Box::new(Adapter(backend.clone())), storage, &cfg());
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
        assert!(
            row.1.as_deref().unwrap_or("").starts_with("parse:"),
            "last_error should record parse failure, got {:?}",
            row.1
        );
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
        assert!(
            row.1.as_deref().unwrap_or("").starts_with("backend:"),
            "last_error should record backend failure, got {:?}",
            row.1
        );
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

        let resp =
            r#"{"entities":[{"name":"inventory-labeler","entity_type":"project"}],"relations":[]}"#;
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
        let ex = LlmExtractor::new(Box::new(backend), storage, &cfg());
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
                {"name": "uconcy13i6keatbmmqmslgbq", "entity_type": "concept"},
                {"name": "redis", "entity_type": "tech"}
            ],
            "relations": []
        }"#;
        let backend = CannedBackend::new(resp);
        let storage = tmp_storage();
        let ex = LlmExtractor::new(Box::new(backend), storage, &cfg());
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
        let ex = LlmExtractor::new(Box::new(Adapter(backend.clone())), storage, &c);
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
        let llm = LlmExtractor::new(Box::new(CannedBackend::new(resp)), tmp_storage(), &cfg());
        let composite = CompositeExtractor::new(Box::new(RuleStub), Box::new(llm));
        let r = composite.extract(&entry("t", "long enough content here"));
        assert_eq!(r.entities.len(), 2);
        let names: Vec<&str> = r.entities.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"rust"));
        assert!(names.contains(&"tokio"));
    }
}
