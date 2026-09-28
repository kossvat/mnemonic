//! The LLM extractor under the redaction policy: a model's answer is
//! judged as the model wrote it, only the accepted structure is cached, in
//! the namespace of the policy in force, and a failure leaves a fixed code.
//! Credential fixtures are assembled at run time; assertions never print
//! them.
use std::sync::{Arc, Mutex};

use rusqlite::params;

use super::*;
use crate::event::{EventSource, MemoryType};
use crate::test_support::InTempDir;

fn body(n: usize) -> String {
    "a1B2c3D4e5F6".chars().cycle().take(n).collect()
}

fn token() -> String {
    ["sk-", "proj-", &body(40)].concat()
}

/// Names the policy refuses as a model writes them, and that are no
/// credential once canonicalized: the separator that made an assignment
/// of one, and the tags that made a private region of the other, are gone.
fn raw_only() -> [String; 2] {
    [
        ["pass", "word=", &body(24)].concat(),
        ["<pri", "vate>", "rollout plan", "</pri", "vate>"].concat(),
    ]
}

struct Backend {
    answer: Result<String, String>,
    calls: Arc<Mutex<usize>>,
}

impl Backend {
    fn new(answer: Result<String, String>) -> (Self, Arc<Mutex<usize>>) {
        let calls = Arc::new(Mutex::new(0));
        let backend = Self {
            answer,
            calls: calls.clone(),
        };
        (backend, calls)
    }
}

impl LlmBackend for Backend {
    fn generate(&self, _prompt: &str) -> anyhow::Result<String> {
        *self.calls.lock().unwrap() += 1;
        match &self.answer {
            Ok(text) => Ok(text.clone()),
            Err(text) => Err(anyhow::anyhow!("{text}")),
        }
    }
}

fn storage() -> InTempDir<Arc<Storage>> {
    InTempDir::new("mnemonic-llm-redaction-", |dir| {
        Arc::new(Storage::open(&dir.join("memory.db")).unwrap())
    })
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

fn memory() -> MemoryEntry {
    MemoryEntry::new(
        "Rollout",
        "The sample service goes out on friday",
        MemoryType::Note,
        EventSource::Manual,
    )
}

fn hash_of(entry: &MemoryEntry) -> String {
    content_hash(&format!("{}\n{}", entry.title, entry.content))
}

fn answer(name: &str) -> String {
    serde_json::json!({
        "entities": [
            {"name": "sample-service", "entity_type": "project"},
            {"name": name, "entity_type": "concept"},
        ],
        "relations": [],
    })
    .to_string()
}

/// Every text the cache and the retry records hold.
fn durable(storage: &Storage) -> String {
    let conn = storage.conn.lock().unwrap();
    let mut out = String::new();
    for (table, columns) in [
        (
            "llm_extraction_cache",
            "content_hash || extractor_id || result_json",
        ),
        (
            "pending_extractions",
            "memory_id || coalesce(last_error, '')",
        ),
    ] {
        let mut stmt = conn
            .prepare(&format!("SELECT {columns} FROM {table}"))
            .unwrap();
        let rows = stmt.query_map([], |row| row.get::<_, String>(0)).unwrap();
        for row in rows {
            out.push_str(table);
            out.push('|');
            out.push_str(&row.unwrap());
            out.push('\n');
        }
    }
    out
}

fn legacy_cache(storage: &Storage, hash: &str, namespace: &str, json: &str) {
    storage
        .conn
        .lock()
        .unwrap()
        .execute(
            "INSERT INTO llm_extraction_cache (content_hash, extractor_id, result_json, created_at)
             VALUES (?1, ?2, ?3, datetime('now'))",
            params![hash, namespace, json],
        )
        .unwrap();
}

#[test]
fn redaction_generated_extraction_fixtures_are_what_the_policy_refuses() {
    for name in raw_only() {
        assert!(!crate::redaction::is_clean(&name));
        let canonical = crate::graph::canonical::canonicalize_name(&name);
        assert!(crate::redaction::is_clean(&canonical) && !canonical.is_empty());
    }
    assert!(!crate::redaction::is_clean(&token()));
}

/// A name is judged as the model wrote it: canonicalized, it would pass
/// the guard of the graph. The structure is refused whole, nothing of it
/// is cached, and the attempt leaves the code of a refusal.
#[test]
fn redaction_generated_extraction_refuses_a_name_before_it_is_canonical() {
    for name in raw_only() {
        let storage = storage();
        let (backend, calls) = Backend::new(Ok(answer(&name)));
        let extractor = LlmExtractor::new(Box::new(backend), storage.clone(), &cfg());
        let entry = memory();
        let result = extractor.extract(&entry);
        assert!(result.entities.is_empty() && result.edges.is_empty());
        assert!(*calls.lock().unwrap() == 1);
        let row = storage.pending_row(&entry.id).unwrap().unwrap();
        assert!(row.1.as_deref() == Some("STRUCTURAL_INPUT_REJECTED"));
        assert!(
            storage
                .llm_cache_get(&hash_of(&entry), &cache_namespace(&cfg().model))
                .unwrap()
                .is_none()
        );
        assert!(!durable(&storage).contains(&body(24)));
    }
}

/// Every field of the structure is a name: the type of an entity, and the
/// ends and the relation of an edge.
#[test]
fn redaction_generated_extraction_judges_every_field_of_the_structure() {
    let token = token();
    let shapes = [
        serde_json::json!({"entities": [{"name": "sample-service", "entity_type": token}]}),
        serde_json::json!({"entities": [{"name": "sample-service"}], "relations": [
            {"source": token, "target": "sample-service", "relation": "uses"}]}),
        serde_json::json!({"entities": [{"name": "sample-service"}], "relations": [
            {"source": "sample-service", "target": token, "relation": "uses"}]}),
        serde_json::json!({"entities": [{"name": "sample-service"}], "relations": [
            {"source": "sample-service", "target": "sample-service", "relation": token}]}),
    ];
    for shape in shapes {
        let storage = storage();
        let (backend, _) = Backend::new(Ok(shape.to_string()));
        let extractor = LlmExtractor::new(Box::new(backend), storage.clone(), &cfg());
        let entry = memory();
        assert!(extractor.extract(&entry).entities.is_empty());
        let row = storage.pending_row(&entry.id).unwrap().unwrap();
        assert!(row.1.as_deref() == Some("STRUCTURAL_INPUT_REJECTED"));
        assert!(!durable(&storage).contains(&token));
    }
}

/// What is cached is the accepted structure, written out again: a field
/// the schema does not know, and with it what the model put there, is not
/// in it. The cache is in the namespace of the policy.
#[test]
fn redaction_generated_extraction_caches_the_accepted_structure_only() {
    let token = token();
    let raw = serde_json::json!({
        "entities": [{"name": "sample-service", "entity_type": "project", "note": token}],
        "relations": [],
        "reasoning": format!("the memory mentions {token}"),
    })
    .to_string();
    let storage = storage();
    let (backend, calls) = Backend::new(Ok(raw));
    let extractor = LlmExtractor::new(Box::new(backend), storage.clone(), &cfg());
    let entry = memory();
    let result = extractor.extract(&entry);
    assert!(result.entities.len() == 1 && result.entities[0].name == "sample-service");

    let namespace = cache_namespace(&cfg().model);
    assert!(namespace.ends_with(&format!("redaction-v{}", crate::redaction::POLICY_VERSION)));
    let cached = storage
        .llm_cache_get(&hash_of(&entry), &namespace)
        .unwrap()
        .unwrap();
    let expected = serde_json::json!({
        "entities": [{"name": "sample-service", "entity_type": "project"}],
        "relations": [],
    });
    assert!(serde_json::from_str::<serde_json::Value>(&cached).unwrap() == expected);
    assert!(!durable(&storage).contains(&token));
    // The second time it is read from there.
    assert!(extractor.extract(&entry).entities.len() == 1);
    assert!(*calls.lock().unwrap() == 1);
}

/// What a build before the policy cached, under the model's name alone,
/// was never judged: a memory with the same text does not read it.
#[test]
fn redaction_generated_extraction_does_not_read_a_cache_from_before_the_policy() {
    let storage = storage();
    let entry = memory();
    let legacy = serde_json::json!({
        "entities": [{"name": "legacy-answer", "entity_type": "concept"}],
        "relations": [],
    })
    .to_string();
    legacy_cache(&storage, &hash_of(&entry), "ollama:qwen2.5:3b", &legacy);

    let (backend, calls) = Backend::new(Ok(answer("config-loader")));
    let extractor = LlmExtractor::new(Box::new(backend), storage.clone(), &cfg());
    let result = extractor.extract(&entry);
    assert!(*calls.lock().unwrap() == 1);
    assert!(result.entities.iter().all(|e| e.name != "legacy-answer"));
    assert!(result.entities.iter().any(|e| e.name == "config-loader"));
    // It is left where it is.
    let kept = storage
        .llm_cache_get(&hash_of(&entry), "ollama:qwen2.5:3b")
        .unwrap();
    assert!(kept.as_deref() == Some(legacy.as_str()));
}

/// An entry of the namespace is judged like an answer: one that holds a
/// name the policy refuses is a miss, and the model is asked.
#[test]
fn redaction_generated_extraction_takes_a_refused_cache_entry_for_a_miss() {
    for name in raw_only() {
        let storage = storage();
        let entry = memory();
        let namespace = cache_namespace(&cfg().model);
        legacy_cache(&storage, &hash_of(&entry), &namespace, &answer(&name));

        let (backend, calls) = Backend::new(Ok(answer("config-loader")));
        let extractor = LlmExtractor::new(Box::new(backend), storage.clone(), &cfg());
        let result = extractor.extract(&entry);
        assert!(*calls.lock().unwrap() == 1);
        let names: Vec<&str> = result.entities.iter().map(|e| e.name.as_str()).collect();
        assert!(names == ["sample-service", "config-loader"]);
        // The answer took its place.
        assert!(!durable(&storage).contains(&body(24)));
    }
}

/// What a backend or a parser says of a failure is the model's text as
/// much as an answer is: the retry record holds a code.
#[test]
fn redaction_generated_extraction_failures_leave_a_code_and_no_text() {
    let token = token();
    let cases = [
        (
            Err(format!("POST http://user:{token}@localhost:11434 refused")),
            "BACKEND_FAILED",
        ),
        (
            Ok(format!("{{\"entities\": [{{\"name\": \"{token}\" oops")),
            "GENERATED_JSON_INVALID",
        ),
    ];
    for (reply, code) in cases {
        let storage = storage();
        let (backend, _) = Backend::new(reply);
        let extractor = LlmExtractor::new(Box::new(backend), storage.clone(), &cfg());
        let entry = memory();
        assert!(extractor.extract(&entry).entities.is_empty());
        let row = storage.pending_row(&entry.id).unwrap().unwrap();
        assert!(row.1.as_deref() == Some(code));
        assert!(!durable(&storage).contains(&token));
    }
}
