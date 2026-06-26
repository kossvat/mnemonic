use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use mnemonic_agent::config::LlmConfig;
use mnemonic_agent::event::{EventSource, MemoryEntry, MemoryType};
use mnemonic_agent::graph::canonical::canonicalize_name;
use mnemonic_agent::graph::extractor::{EntityExtractor, RuleExtractor};
use mnemonic_agent::graph::extractor_llm::{CompositeExtractor, LlmExtractor, OllamaBackend};
use mnemonic_agent::storage::Storage;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct EvalCase {
    title: String,
    content: String,
    memory_type: String,
    expected_entities: Vec<String>,
    expected_relations: Vec<[String; 3]>,
}

#[test]
fn fixture_shape_is_valid() {
    let cases = load_cases();
    assert_eq!(cases.len(), 10);
    for case in &cases {
        assert!(!case.title.trim().is_empty());
        assert!(!case.content.trim().is_empty());
        assert!(!case.expected_entities.is_empty());
    }
}

#[test]
#[ignore = "requires local Ollama for LLM-backed extraction quality numbers"]
fn extraction_quality_eval() {
    let cases = load_cases();
    let extractor = build_extractor();
    let using_ollama = ollama_available();

    let mut macro_recall = 0.0f32;
    let mut macro_precision = 0.0f32;
    let mut macro_relation_recall = 0.0f32;

    println!(
        "extraction_eval backend={}",
        if using_ollama {
            "rule+ollama:qwen2.5:3b"
        } else {
            "rule-only"
        }
    );

    for case in &cases {
        let entry = memory_entry(case);
        let result = extractor.extract(&entry);

        let extracted_entities: HashSet<String> = result
            .entities
            .iter()
            .map(|e| canonicalize_name(&e.name))
            .filter(|e| !e.is_empty())
            .collect();
        let expected_entities: HashSet<String> = case
            .expected_entities
            .iter()
            .map(|e| canonicalize_name(e))
            .collect();
        let matched_entities = expected_entities.intersection(&extracted_entities).count() as f32;

        let recall = matched_entities / expected_entities.len().max(1) as f32;
        let precision = if extracted_entities.is_empty() {
            0.0
        } else {
            matched_entities / extracted_entities.len() as f32
        };

        let extracted_relations: HashSet<(String, String, String)> = result
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
        let expected_relations: HashSet<(String, String, String)> = case
            .expected_relations
            .iter()
            .map(|r| {
                (
                    canonicalize_name(&r[0]),
                    canonicalize_name(&r[1]),
                    r[2].to_lowercase(),
                )
            })
            .collect();
        let matched_relations = expected_relations
            .intersection(&extracted_relations)
            .count() as f32;
        let relation_recall = matched_relations / expected_relations.len().max(1) as f32;

        macro_recall += recall;
        macro_precision += precision;
        macro_relation_recall += relation_recall;

        println!(
            "{} | entity_recall={:.2} entity_precision={:.2} relation_recall={:.2} | extracted_entities={:?} extracted_relations={:?}",
            case.title, recall, precision, relation_recall, extracted_entities, extracted_relations
        );
    }

    let n = cases.len() as f32;
    println!(
        "aggregate macro_entity_recall={:.3} macro_entity_precision={:.3} macro_relation_recall={:.3}",
        macro_recall / n,
        macro_precision / n,
        macro_relation_recall / n
    );
}

fn load_cases() -> Vec<EvalCase> {
    serde_json::from_str(include_str!("fixtures/extraction_eval.json"))
        .expect("extraction_eval fixture must be valid JSON")
}

fn build_extractor() -> Box<dyn EntityExtractor> {
    let rule: Box<dyn EntityExtractor> = Box::new(RuleExtractor::new());

    if !ollama_available() {
        return rule;
    }

    let cfg = LlmConfig {
        enabled: true,
        endpoint: "http://localhost:11434".into(),
        model: "qwen2.5:3b".into(),
        timeout_secs: 30,
        min_chars: 0,
    };
    let storage = Arc::new(Storage::open(&tmp_db()).expect("temp storage must open"));
    let backend = match OllamaBackend::new(&cfg) {
        Ok(backend) => backend,
        Err(_) => return rule,
    };
    let llm = LlmExtractor::new(Box::new(backend), storage, &cfg);

    Box::new(CompositeExtractor::new(rule, Box::new(llm)))
}

fn ollama_available() -> bool {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(1))
        .build()
        .expect("reqwest client must build");
    client
        .get("http://localhost:11434/api/tags")
        .send()
        .is_ok_and(|r| r.status().is_success())
}

fn tmp_db() -> std::path::PathBuf {
    // UUID-suffixed: parallel tests must not share a path even when their
    // start times land in the same nanosecond bucket.
    let dir =
        std::env::temp_dir().join(format!("mnemonic-extraction-eval-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("temp dir must be created");
    dir.join("memory.db")
}

fn memory_entry(case: &EvalCase) -> MemoryEntry {
    MemoryEntry {
        id: uuid::Uuid::new_v4().to_string(),
        timestamp: Utc::now(),
        title: case.title.clone(),
        content: case.content.clone(),
        memory_type: memory_type(&case.memory_type),
        tags: vec![],
        source: EventSource::Manual,
        importance: 0.7,
        metadata: serde_json::Value::Null,
    }
}

fn memory_type(s: &str) -> MemoryType {
    match s {
        "decision" => MemoryType::Decision,
        "feedback" => MemoryType::Feedback,
        "session_summary" => MemoryType::SessionSummary,
        "security" => MemoryType::Security,
        _ => MemoryType::Note,
    }
}
