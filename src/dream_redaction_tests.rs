//! Session summaries under the redaction policy: the text of a summary is
//! new, whether a model wrote it or it was put together from stored
//! titles, and it is prepared before anyone is given it. A failure says a
//! fixed code. Credential fixtures are assembled at run time; assertions
//! never print them.
use std::sync::Arc;

use rusqlite::params;

use super::*;
use crate::redaction::{CREDENTIAL_MARKER, SUMMARY_KEY};
use crate::test_support::InTempDir;

fn token() -> String {
    ["sk-", "proj-", &"a1B2c3D4e5F6".repeat(4)].concat()
}

fn storage() -> InTempDir<Arc<Storage>> {
    InTempDir::new("mnemonic-dream-redaction-", |dir| {
        Arc::new(Storage::open(&dir.join("memory.db")).unwrap())
    })
}

/// A closed session with one memory, and one more if `legacy` gives its
/// title: stored as a build before the policy stored it.
pub(crate) fn session(storage: &Storage, legacy: Option<&str>) -> String {
    let peer = storage.upsert_peer("claude", None, "agent").unwrap();
    let session = storage.open_session(&peer, Some("test"), "jsonl").unwrap();
    let entry = MemoryEntry::new(
        "Picked the rollout day",
        "friday",
        MemoryType::Decision,
        EventSource::Manual,
    );
    storage.save(&entry).unwrap();
    storage
        .set_memory_session(&entry.id, Some(&session))
        .unwrap();
    if let Some(title) = legacy {
        let id = uuid::Uuid::new_v4().to_string();
        storage
            .conn
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO memories (id, timestamp, title, content, memory_type, tags, source,
                    importance, metadata)
                 VALUES (?1, ?2, ?3, 'c', 'decision', '[]', '\"Manual\"', 0.5, 'null')",
                params![id, chrono::Utc::now().to_rfc3339(), title],
            )
            .unwrap();
        storage.set_memory_session(&id, Some(&session)).unwrap();
    }
    storage.end_session(&session).unwrap();
    session
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

fn holds(entry: &MemoryEntry, text: &str) -> bool {
    serde_json::to_string(entry).unwrap().contains(text)
}

/// What a model wrote is prepared in the entry the summarizer returns: a
/// preview, an embedder and the store are all given that entry.
#[test]
fn redaction_generated_dream_of_a_model_is_prepared_before_anyone_is_given_it() {
    let token = token();
    let storage = storage();
    let session = session(&storage, None);
    let mut prior: Option<String> = None;
    for answer in [
        serde_json::json!({"summary": format!("We rotated {token} and moved on")}).to_string(),
        format!("We rotated {token} and moved on"),
    ] {
        let summary = summarize_session_llm(&storage, &session, &Backend(Ok(answer))).unwrap();
        assert!(!holds(&summary, &token));
        assert!(summary.content.contains(CREDENTIAL_MARKER));
        // The summary of what was prepared is the application's own.
        assert!(summary.metadata[SUMMARY_KEY]["changed"] == true);
        // The store takes it as it is.
        replace_summary(&storage, prior.as_deref(), &summary, None).unwrap();
        prior = Some(summary.id);
    }
    // A clean answer is kept as it is, with no summary of a preparation.
    let answer = serde_json::json!({"summary": "We picked friday"}).to_string();
    let summary = summarize_session_llm(&storage, &session, &Backend(Ok(answer))).unwrap();
    assert!(summary.content == "We picked friday");
    assert!(summary.metadata.get(SUMMARY_KEY).is_none());
}

/// The summary without a model is put together from stored titles, and a
/// title stored before the policy is a text like the model's.
#[test]
fn redaction_generated_dream_from_stored_titles_is_prepared() {
    let token = token();
    let storage = storage();
    let session = session(&storage, Some(&format!("Deploy key is {token}")));
    let summary = summarize_session_heuristic(&storage, &session).unwrap();
    assert!(!holds(&summary, &token));
    assert!(summary.content.contains(CREDENTIAL_MARKER));
    assert!(summary.content.contains("Picked the rollout day"));
}

/// What a backend says of its failure, and an answer that cannot be used,
/// are the model's text: the error says a code, however it is printed.
#[test]
fn redaction_generated_dream_failures_say_a_code_and_no_text() {
    let token = token();
    let storage = storage();
    let session = session(&storage, None);
    let cases = [
        (
            Err(format!("POST http://user:{token}@host refused")),
            "BACKEND_FAILED",
        ),
        (Ok("  \n ".to_string()), "GENERATED_JSON_INVALID"),
    ];
    for (reply, code) in cases {
        let error = summarize_session_llm(&storage, &session, &Backend(reply)).unwrap_err();
        let said = format!("{error} | {error:#} | {error:?}");
        assert!(said.contains(code));
        assert!(!said.contains(&token), "the error holds the token");
    }
}

/// A session is asked for by its id, and an id the policy refuses is not
/// said back.
#[test]
fn redaction_generated_dream_does_not_say_a_refused_session_id() {
    let token = token();
    let storage = storage();
    for error in [
        summarize_session_heuristic(&storage, &token).unwrap_err(),
        summarize_session_llm(&storage, &token, &Backend(Ok("x".into()))).unwrap_err(),
    ] {
        let said = format!("{error:#} {error:?}");
        assert!(said.contains("not found") && !said.contains(&token));
    }
    let error = summarize_session_heuristic(&storage, "s-1").unwrap_err();
    assert!(error.to_string().contains("session s-1 not found"));
}
