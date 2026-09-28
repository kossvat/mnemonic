//! Generated conclusions under the redaction policy: what a model
//! answered is prepared before it is shown, a claim under a kind the
//! policy refuses is left out, and a failure says a fixed code.
//! Credential fixtures are assembled at run time; assertions never print
//! them.
use std::sync::Arc;

use super::*;
use crate::event::{EventSource, MemoryType};
use crate::graph::{Entity, EntityType};
use crate::redaction::CREDENTIAL_MARKER;
use crate::test_support::InTempDir;

fn token() -> String {
    ["sk-", "proj-", &"a1B2c3D4e5F6".repeat(4)].concat()
}

/// A store where one memory mentions `user`.
fn storage() -> InTempDir<Arc<Storage>> {
    let storage = InTempDir::new("mnemonic-concl-redaction-", |dir| {
        Arc::new(Storage::open(&dir.join("memory.db")).unwrap())
    });
    let entry = MemoryEntry::new(
        "User prefers rust",
        "User picked rust for the daemon",
        MemoryType::Note,
        EventSource::Manual,
    );
    storage.save(&entry).unwrap();
    let entity = Entity {
        name: "user".into(),
        entity_type: EntityType::Person,
    };
    storage.save_graph(&entry.id, &[entity], &[]).unwrap();
    storage
}

struct Backend(Result<String, String>);

impl LlmBackend for Backend {
    fn generate(&self, _prompt: &str) -> anyhow::Result<String> {
        match &self.0 {
            Ok(text) => Ok(text.clone()),
            Err(text) => Err(anyhow::anyhow!("{text}")),
        }
    }
}

fn generate(storage: &Storage, reply: Result<String, String>) -> Result<GenerationOutput> {
    LlmConclusionGenerator::new(Box::new(Backend(reply))).generate_for_subject(storage, "user", 10)
}

/// The claims come back as they are shown and stored: a statement
/// prepared, and a claim the model filed under a credential left out and
/// counted.
#[test]
fn redaction_generated_conclusions_are_prepared_before_they_are_shown() {
    let token = token();
    let storage = storage();
    let answer = serde_json::json!({"conclusions": [
        {"statement": format!("keeps {token} in the notes"), "kind": "pattern", "confidence": 0.7},
        {"statement": "prefers rust", "kind": token, "confidence": 0.7},
        {"statement": "ships on fridays", "kind": "pattern", "confidence": 0.6},
    ]})
    .to_string();
    let out = generate(&storage, Ok(answer)).unwrap();
    assert!(out.conclusions.len() == 2 && out.withheld == 1);
    assert!(!format!("{out:?}").contains(&token));
    assert!(out.conclusions[0].statement.contains(CREDENTIAL_MARKER));
    assert!(out.conclusions[1].statement == "ships on fridays");
    // What is shown is what the store keeps.
    for c in &out.conclusions {
        let id = storage
            .add_conclusion(
                "user",
                &c.kind,
                &c.statement,
                c.confidence,
                &out.source_memory_ids,
            )
            .unwrap();
        let kept = storage.conclusion_by_id(&id).unwrap().unwrap();
        assert!(kept.statement == c.statement);
    }
}

/// What a backend says of its failure, and an answer that cannot be
/// parsed, are the model's text: the error says a code, however it is
/// printed.
#[test]
fn redaction_generated_conclusion_failures_say_a_code_and_no_text() {
    let token = token();
    let storage = storage();
    let cases = [
        (
            Err(format!("POST http://user:{token}@host refused")),
            "BACKEND_FAILED",
        ),
        (
            Ok(format!("sure, here you go: {token}")),
            "GENERATED_JSON_INVALID",
        ),
    ];
    for (reply, code) in cases {
        let error = generate(&storage, reply).unwrap_err();
        let said = format!("{error} | {error:#} | {error:?}");
        assert!(said.contains(code));
        assert!(!said.contains(&token), "the error holds the token");
    }
}

/// A subject is what the claims are asked about, and one the policy
/// refuses is not said back.
#[test]
fn redaction_generated_conclusions_do_not_say_a_refused_subject() {
    let token = token();
    let storage = storage();
    let generator = LlmConclusionGenerator::new(Box::new(Backend(Ok("[]".into()))));
    let error = generator
        .generate_for_subject(&storage, &token, 5)
        .unwrap_err();
    let said = format!("{error:#} {error:?}");
    assert!(said.contains("no memories") && !said.contains(&token));
}
