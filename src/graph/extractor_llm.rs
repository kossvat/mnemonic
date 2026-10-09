//! LLM-backed entity and relation extractor.
//!
//! Calls an Ollama-compatible JSON endpoint, expects a strict JSON schema,
//! and falls back silently on any error (network, parse, timeout). Results
//! are cached by sha-ish hash of (title|content|extractor_id) so repeated
//! identical memories don't re-burn LLM calls.
//!
//! A model's answer is fresh input, whatever the prompt held: it is parsed,
//! every name in it is judged as the model wrote it, and only the structure
//! that passed is written out again for the cache. The cache is read in the
//! namespace of the redaction policy in force, so what was cached before
//! it, unjudged, is not read. A failure leaves a fixed code, never the
//! error's text.
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
use crate::redaction::failure::Failure;
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
    project_registry: bool,
}

impl LlmExtractor {
    pub fn new(
        backend: Box<dyn LlmBackend>,
        storage: std::sync::Arc<Storage>,
        cfg: &LlmConfig,
    ) -> Self {
        // Identify cache entries by backend + model so swapping models
        // doesn't poison results from the previous one.
        let extractor_id = cache_namespace(&cfg.model);
        Self {
            backend,
            storage,
            extractor_id,
            min_chars: cfg.min_chars,
            project_registry: false,
        }
    }

    /// The registry constrains model project types only when the caller
    /// configured projects. Leaving this off preserves legacy extraction.
    pub fn with_project_registry(mut self, enabled: bool) -> Self {
        self.project_registry = enabled;
        self
    }

    fn result(&self, accepted: &LlmResponse, memory_id: &str) -> ExtractionResult {
        let mut result = to_result(accepted, memory_id);
        if self.project_registry {
            // Resolve at read time, including cache hits: a renamed project
            // or a new alias must not need another model call. The cache
            // retains only the accepted model structure, before this clamp.
            let names = self.storage.project_type_names().unwrap_or_else(|_| {
                warn!("LLM extractor: PROJECT_REGISTRY_UNAVAILABLE");
                Default::default()
            });
            for entity in &mut result.entities {
                if entity.entity_type == EntityType::Project && !names.contains(&entity.name) {
                    entity.entity_type = EntityType::Concept;
                }
            }
        }
        result
    }
}

/// The cache namespace of a model under the redaction policy in force.
/// What a build before the policy cached under the model's name alone was
/// never judged: it stays where it is and is not read.
fn cache_namespace(model: &str) -> String {
    format!(
        "ollama:{model}:redaction-v{}",
        crate::redaction::POLICY_VERSION
    )
}

/// The structure a model answered with, if the policy admits every name
/// in it as the model wrote it: before a name is canonicalized (which
/// takes the dots out of a token and the case out of a prefix), before
/// the structure is cached and before it becomes a graph.
fn accept(parsed: LlmResponse) -> Result<LlmResponse, Failure> {
    let names = parsed
        .entities
        .iter()
        .flat_map(|e| [e.name.as_str(), e.entity_type.as_str()])
        .chain(
            parsed
                .relations
                .iter()
                .flat_map(|r| [r.source.as_str(), r.target.as_str(), r.relation.as_str()]),
        );
    match crate::redaction::state::check_identities(names) {
        Ok(()) => Ok(parsed),
        Err(_) => Err(Failure::Rejected),
    }
}

impl LlmExtractor {
    /// A failed attempt: the memory keeps its rule-based graph and waits
    /// for a retry (a backend that was down, a model that may do better
    /// next time). What failed is said by its code alone, in the log and
    /// in the retry record.
    fn failed(&self, entry: &MemoryEntry, failure: Failure) -> ExtractionResult {
        warn!("LLM extractor: {failure}");
        let _ = self
            .storage
            .enqueue_pending_extraction(&entry.id, failure.code());
        ExtractionResult::default()
    }
}

impl EntityExtractor for LlmExtractor {
    fn extract(&self, entry: &MemoryEntry) -> ExtractionResult {
        let combined = format!("{}\n{}", entry.title, entry.content);
        if combined.chars().count() < self.min_chars {
            return ExtractionResult::default();
        }

        let hash = content_hash(&combined);

        // Cache lookup. An entry that does not parse, or that holds a name
        // the policy refuses, is a miss.
        if let Ok(Some(cached)) = self.storage.llm_cache_get(&hash, &self.extractor_id)
            && let Ok(parsed) = serde_json::from_str::<LlmResponse>(&cached)
            && let Ok(accepted) = accept(parsed)
        {
            debug!("LLM extractor cache hit ({})", &hash[..8]);
            return self.result(&accepted, &entry.id);
        }

        // Cache miss — call the model.
        let prompt = build_prompt(&entry.title, &entry.content);
        let Ok(raw) = self.backend.generate(&prompt) else {
            return self.failed(entry, Failure::Backend);
        };
        let Ok(parsed) = serde_json::from_str::<LlmResponse>(&raw) else {
            return self.failed(entry, Failure::InvalidJson);
        };
        let accepted = match accept(parsed) {
            Ok(accepted) => accepted,
            Err(failure) => return self.failed(entry, failure),
        };

        // The accepted structure written out again, never the answer as it
        // came: a field the schema does not know is not in it.
        if let Ok(json) = serde_json::to_string(&accepted) {
            let _ = self.storage.llm_cache_put(&hash, &self.extractor_id, &json);
        }
        // Success — make sure the retry queue doesn't keep this around.
        let _ = self.storage.drop_pending_extraction(&entry.id);

        self.result(&accepted, &entry.id)
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
        // "Sample Service" and "sample-service" collapse instead of splitting the graph.
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
- Names are lowercase, hyphenated (e.g. "sample-service", "config-loader").
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
#[path = "extractor_llm_redaction_tests.rs"]
mod redaction_tests;

#[cfg(test)]
#[path = "extractor_llm_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "extractor_llm_project_tests.rs"]
mod project_tests;
