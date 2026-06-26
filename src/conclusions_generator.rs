//! LLM-driven conclusion generator (v2 of the #6 inductive
//! conclusions feature).
//!
//! v1 (commit history) shipped the schema and a manual CLI: the
//! user typed `mnemonic conclusion add <subject> <claim>` to record
//! a pattern by hand. This module is the v2 layer that auto-mines
//! conclusions from memory clusters via the Ollama backend already
//! used by the graph extractor.
//!
//! ## Pipeline
//!
//! 1. Gather: pull the N most recent non-superseded memories that
//!    mention the subject (via `memories_for_entity_name`).
//! 2. Prompt: feed them to the LLM with a structured request — list
//!    3–5 inductive patterns / preferences / trends in JSON form.
//! 3. Parse: deserialize the JSON array into `GeneratedConclusion`s.
//! 4. Return (NOT save): the caller decides whether to persist
//!    (default dry-run preview; `--apply` saves via
//!    `Storage::add_conclusion`).
//!
//! ## Why caller-saves
//!
//! Persisting conclusions is destructive — once saved, retrieval
//! starts surfacing them as authoritative claims. A dry-run preview
//! step lets the user spot hallucinations before they pollute the
//! retrieval surface. The trait returns `Vec<GeneratedConclusion>`
//! and the CLI handles persistence; the worker (when added) will
//! save without preview, but the trait stays the same.
//!
//! ## Mocking
//!
//! Tests use a fake `LlmBackend` from `crate::graph::extractor_llm`
//! that returns canned JSON. No Ollama dependency for unit tests.

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::event::MemoryEntry;
use crate::graph::extractor_llm::LlmBackend;
use crate::storage::Storage;

/// Output of `LlmConclusionGenerator::generate_for_subject` — a
/// claim the LLM made about the subject, along with the memory ids
/// it was synthesized from. Caller persists via
/// `Storage::add_conclusion(subject, &kind, &statement, confidence,
/// &source_memory_ids)`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct GeneratedConclusion {
    pub statement: String,
    /// Free-form category: "pattern" | "preference" | "trend" |
    /// "observation". The storage layer doesn't validate; we pass
    /// through whatever the LLM produced. CLI display normalizes
    /// for casing.
    #[serde(default = "default_kind")]
    pub kind: String,
    /// Confidence in [0.0, 1.0]. The storage `add_conclusion` helper
    /// rejects out-of-range / NaN; we leave that validation there
    /// rather than duplicating.
    #[serde(default = "default_confidence")]
    pub confidence: f32,
}

fn default_kind() -> String {
    "pattern".into()
}
fn default_confidence() -> f32 {
    0.6
}

/// The full result returned by the generator: the claims plus the
/// source memory ids those claims should attribute to (when saved).
/// All claims share the same source set in v1 — every retrieved
/// memory is considered evidence for every claim — because the LLM
/// doesn't give us per-claim citations. v2 could thread that
/// through if the prompt is changed to require attribution.
#[derive(Debug, Clone)]
pub struct GenerationOutput {
    pub conclusions: Vec<GeneratedConclusion>,
    pub source_memory_ids: Vec<String>,
}

/// Trait abstracting the generation step so tests can mock without
/// touching Ollama. Production wires this to `LlmConclusionGenerator`
/// backed by `OllamaBackend`.
pub trait ConclusionGenerator: Send + Sync {
    /// Generate up to ~5 conclusions about `subject` from the most
    /// recent `limit` memories that mention it. Returns the LLM's
    /// claims and the source memory ids the caller should record.
    fn generate_for_subject(
        &self,
        storage: &Storage,
        subject: &str,
        limit: usize,
    ) -> Result<GenerationOutput>;
}

/// Production generator using an `LlmBackend` (Ollama in practice).
pub struct LlmConclusionGenerator {
    backend: Box<dyn LlmBackend>,
}

impl LlmConclusionGenerator {
    pub fn new(backend: Box<dyn LlmBackend>) -> Self {
        Self { backend }
    }
}

impl ConclusionGenerator for LlmConclusionGenerator {
    fn generate_for_subject(
        &self,
        storage: &Storage,
        subject: &str,
        limit: usize,
    ) -> Result<GenerationOutput> {
        let memories = storage.memories_for_entity_name(subject, limit)?;
        if memories.is_empty() {
            anyhow::bail!(
                "no memories mention `{subject}` — nothing to generate from. \
                 (Subject lookup is case-insensitive against the entities table.)"
            );
        }

        let prompt = build_prompt(subject, &memories);
        let raw = self
            .backend
            .generate(&prompt)
            .context("LLM backend failed to respond")?;
        let conclusions = parse_llm_response(&raw)
            .with_context(|| format!("could not parse LLM JSON response: {raw}"))?;

        Ok(GenerationOutput {
            conclusions,
            source_memory_ids: memories.into_iter().map(|m| m.id).collect(),
        })
    }
}

/// Build the prompt sent to the LLM. The Ollama backend is configured
/// with `format=json` so the model is biased to output a valid JSON
/// document; we constrain to an array of objects so deserialization
/// has a stable shape.
///
/// The memory list is bullet-formatted with title + first ~200 chars
/// of content. Truncating content keeps the prompt small (most local
/// models have small context windows) and reduces hallucination
/// surface — the LLM produces stronger claims when grounded on
/// short, focused inputs.
fn build_prompt(subject: &str, memories: &[MemoryEntry]) -> String {
    let mut lines = Vec::new();
    for m in memories {
        let snippet: String = m.content.chars().take(200).collect();
        let snippet_trimmed = snippet.replace('\n', " ");
        lines.push(format!(
            "- [{}] {} — {}",
            m.memory_type, m.title, snippet_trimmed
        ));
    }
    let memories_block = lines.join("\n");

    format!(
        r#"You are analyzing memories about `{subject}` to surface inductive patterns.

Memories (newest first):
{memories_block}

Task: list 3 to 5 inductive claims you can make about `{subject}` based on the memories above.
Each claim should be a high-level pattern, preference, trend, or observation that emerges from the cluster — NOT just a restatement of any single memory.

Respond with a JSON object of the form:
{{
  "conclusions": [
    {{"statement": "<claim text>", "kind": "pattern|preference|trend|observation", "confidence": 0.0-1.0}}
  ]
}}

Rules:
- 3 to 5 claims.
- `kind` must be one of: pattern, preference, trend, observation.
- `confidence` is your subjective certainty given the memories shown, in [0.0, 1.0].
- No claim should mention specific timestamps or memory ids — these are inductive generalizations.
"#
    )
}

/// Wrapper struct for deserializing the LLM's `{"conclusions": [...]}`
/// response. Separated from `GeneratedConclusion` so the on-disk
/// metadata format stays flat (no `conclusions` wrapper saved).
#[derive(Debug, Deserialize)]
struct LlmResponse {
    #[serde(default)]
    conclusions: Vec<GeneratedConclusion>,
}

/// Parse the LLM's JSON response. Tolerates two shapes:
/// 1. `{"conclusions": [...]}` — the format the prompt requests.
/// 2. `[...]` — bare array, in case the model drops the wrapper.
///
/// Returns the parsed claims or an error. Empty arrays are NOT an
/// error — the caller can choose how to surface "LLM found nothing".
fn parse_llm_response(raw: &str) -> Result<Vec<GeneratedConclusion>> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        anyhow::bail!("LLM returned empty response");
    }
    // Try the wrapper shape first.
    if let Ok(wrapped) = serde_json::from_str::<LlmResponse>(trimmed) {
        return Ok(wrapped.conclusions);
    }
    // Fall back to a bare array.
    if let Ok(bare) = serde_json::from_str::<Vec<GeneratedConclusion>>(trimmed) {
        return Ok(bare);
    }
    // Both failed → real parse error.
    serde_json::from_str::<Vec<GeneratedConclusion>>(trimmed)
        .map(|_| Vec::new())
        .context("response is neither {\"conclusions\": [...]} nor [...]")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{EventSource, MemoryEntry, MemoryType};
    use crate::storage::Storage;
    use chrono::Utc;
    use std::sync::Arc;

    fn tmp_storage() -> Arc<Storage> {
        let dir =
            std::env::temp_dir().join(format!("mnemonic-concl-gen-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        Arc::new(Storage::open(&dir.join("memory.db")).unwrap())
    }

    fn make_entry(title: &str, content: &str) -> MemoryEntry {
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

    /// Fake LlmBackend that returns a fixed JSON string. Lets us
    /// test the full pipeline (gather → prompt → parse) without
    /// touching Ollama. The captured prompt lives behind a shared
    /// `Arc<Mutex>` so the test can inspect it AFTER the generator
    /// has consumed the boxed-trait-object wrapper — no raw-pointer
    /// dance required.
    struct FakeBackend {
        response: String,
        captured_prompt: std::sync::Arc<std::sync::Mutex<Option<String>>>,
    }
    impl FakeBackend {
        fn new(response: &str) -> Self {
            Self {
                response: response.to_string(),
                captured_prompt: std::sync::Arc::new(std::sync::Mutex::new(None)),
            }
        }
        /// Clone the shared handle so the caller can inspect the
        /// prompt without holding a reference to the backend
        /// (which gets moved into the generator).
        fn prompt_handle(&self) -> std::sync::Arc<std::sync::Mutex<Option<String>>> {
            self.captured_prompt.clone()
        }
    }
    impl LlmBackend for FakeBackend {
        fn generate(&self, prompt: &str) -> anyhow::Result<String> {
            *self.captured_prompt.lock().unwrap() = Some(prompt.to_string());
            Ok(self.response.clone())
        }
    }

    /// Wrapper-shape response parses cleanly.
    #[test]
    fn parse_llm_response_accepts_wrapped_shape() {
        let raw = r#"{"conclusions": [
            {"statement": "prefers terse outputs", "kind": "preference", "confidence": 0.8}
        ]}"#;
        let out = parse_llm_response(raw).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].statement, "prefers terse outputs");
        assert_eq!(out[0].kind, "preference");
        assert!((out[0].confidence - 0.8).abs() < 1e-6);
    }

    /// Bare-array fallback parses too — some local models drop the
    /// wrapper despite the prompt asking for it. Be tolerant.
    #[test]
    fn parse_llm_response_accepts_bare_array_fallback() {
        let raw = r#"[
            {"statement": "ships incrementally", "kind": "pattern", "confidence": 0.7}
        ]"#;
        let out = parse_llm_response(raw).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, "pattern");
    }

    /// Missing fields fall back to defaults — `kind` → "pattern",
    /// `confidence` → 0.6. Tolerant deserialize keeps us robust to
    /// model output variance.
    #[test]
    fn parse_llm_response_applies_defaults_for_missing_fields() {
        let raw = r#"{"conclusions": [{"statement": "claim"}]}"#;
        let out = parse_llm_response(raw).unwrap();
        assert_eq!(out[0].kind, "pattern");
        assert!((out[0].confidence - 0.6).abs() < 1e-6);
    }

    /// Empty response is an error (vs. successful "no conclusions").
    /// Callers want to distinguish "model said nothing" from "model
    /// genuinely found no patterns".
    #[test]
    fn parse_llm_response_rejects_empty_string() {
        assert!(parse_llm_response("").is_err());
        assert!(parse_llm_response("   \n  ").is_err());
    }

    /// Unparseable response surfaces the parse error with context
    /// rather than silently returning empty.
    #[test]
    fn parse_llm_response_errors_on_garbage() {
        assert!(parse_llm_response("not json at all").is_err());
        assert!(parse_llm_response("{broken").is_err());
    }

    /// End-to-end through the generator: save memories mentioning
    /// `user` via the rule extractor pipeline, then run the LLM
    /// generator with a mock backend, verify it:
    /// 1. Gathered the right memories (prompt includes their titles)
    /// 2. Returned the parsed conclusions
    /// 3. Reported the source memory ids
    #[test]
    fn generator_assembles_prompt_and_returns_conclusions_with_sources() {
        let storage = tmp_storage();
        // Save 2 memories mentioning "user" — the rule extractor
        // links entities by lowercased name. Direct entity link via
        // storage.link_memory_entity since we don't run the full
        // extraction pipeline in unit tests.
        let m1 = make_entry(
            "User prefers rust",
            "User picked rust for the memory daemon",
        );
        let m2 = make_entry(
            "User ships incrementally",
            "small commits with codex review",
        );
        storage.save(&m1).unwrap();
        storage.save(&m2).unwrap();

        // Manually create the entity and links so the SQL JOIN
        // in memories_for_entity_name finds them.
        use crate::graph::{Entity, EntityType};
        let entity = Entity {
            name: "user".into(),
            entity_type: EntityType::Person,
        };
        storage
            .save_graph(&m1.id, std::slice::from_ref(&entity), &[])
            .unwrap();
        storage.save_graph(&m2.id, &[entity], &[]).unwrap();

        // Fake backend returns 2 claims. Grab a clone of the
        // prompt-capture handle BEFORE moving the backend into the
        // generator — that way the test can inspect what was sent
        // without touching the consumed Box.
        let fake = FakeBackend::new(
            r#"{"conclusions":[
                {"statement":"prefers rust for systems work","kind":"preference","confidence":0.75},
                {"statement":"ships incrementally with external review","kind":"pattern","confidence":0.8}
            ]}"#,
        );
        let prompt_handle = fake.prompt_handle();
        let generator = LlmConclusionGenerator::new(Box::new(fake));

        let out = generator
            .generate_for_subject(&storage, "user", 10)
            .unwrap();
        assert_eq!(out.conclusions.len(), 2);
        assert_eq!(out.source_memory_ids.len(), 2);

        // Prompt should include both titles so the LLM has the
        // grounding context the heuristic claims it does.
        let prompt = prompt_handle
            .lock()
            .unwrap()
            .clone()
            .expect("backend was called");
        assert!(prompt.contains("User prefers rust"));
        assert!(prompt.contains("User ships incrementally"));
        assert!(prompt.contains("`user`")); // subject in the instructions
    }

    /// Unknown subject (no entity by that name) → loud error.
    /// Protects the future async-worker path from silently
    /// generating zero conclusions on typos.
    #[test]
    fn generator_errors_on_unknown_subject() {
        let storage = tmp_storage();
        let backend = Box::new(FakeBackend::new("[]"));
        let generator = LlmConclusionGenerator::new(backend);
        let err = generator.generate_for_subject(&storage, "no-such-subject", 5);
        assert!(err.is_err());
        let msg = err.unwrap_err().to_string();
        assert!(
            msg.contains("no memories"),
            "error should explain why: {msg}"
        );
    }
}
