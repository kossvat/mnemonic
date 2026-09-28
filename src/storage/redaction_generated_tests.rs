//! The store's own guards for what generation leaves behind: the
//! extraction cache takes a structure the policy admits or nothing, and a
//! retry record holds a fixed code whatever a caller passes. Credential
//! fixtures are assembled at run time; assertions never print them.
use rusqlite::params;

use super::*;
use crate::event::{EventSource, MemoryType};

fn body(n: usize) -> String {
    "a1B2c3D4e5F6".chars().cycle().take(n).collect()
}

fn token() -> String {
    ["sk-", "proj-", &body(40)].concat()
}

fn open() -> crate::test_support::InTempDir<Storage> {
    crate::test_support::InTempDir::new("mnemonic-generated-", |dir| {
        Storage::open(&dir.join("memory.db")).unwrap()
    })
}

fn count(storage: &Storage, table: &str) -> i64 {
    let conn = storage.conn.lock().unwrap();
    conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

fn saved(storage: &Storage) -> String {
    let entry = MemoryEntry::new("Rollout", "friday", MemoryType::Note, EventSource::Manual);
    storage.save(&entry).unwrap();
    entry.id
}

#[test]
fn redaction_generated_cache_write_takes_an_admitted_structure_or_nothing() {
    let token = token();
    let storage = open();
    let clean =
        r#"{"entities":[{"name":"sample-service","entity_type":"project"}],"relations":[]}"#;
    let refused = [
        // A value, a key, and a value under a credential name.
        (
            "h1",
            "ollama:m:redaction-v1",
            format!(r#"{{"entities":[{{"name":"{token}"}}]}}"#),
        ),
        (
            "h1",
            "ollama:m:redaction-v1",
            format!(r#"{{"{token}":[]}}"#),
        ),
        (
            "h1",
            "ollama:m:redaction-v1",
            format!(r#"{{"password":"{}"}}"#, body(24)),
        ),
        // What names the entry.
        (token.as_str(), "ollama:m:redaction-v1", clean.to_string()),
        ("h1", token.as_str(), clean.to_string()),
    ];
    for (hash, namespace, json) in &refused {
        let error = storage.llm_cache_put(hash, namespace, json).unwrap_err();
        assert!(is_refused_write(&error));
        assert!(!format!("{error:#} {error:?}").contains(&token));
    }
    // An answer as it came, that is no structure at all.
    let error = storage
        .llm_cache_put("h1", "ollama:m:redaction-v1", &format!("sure: {token}"))
        .unwrap_err();
    let said = format!("{error:#} {error:?}");
    assert!(said.contains("GENERATED_JSON_INVALID") && !said.contains(&token));
    assert!(count(&storage, "llm_extraction_cache") == 0);

    storage
        .llm_cache_put("h1", "ollama:m:redaction-v1", clean)
        .unwrap();
    let cached = storage
        .llm_cache_get("h1", "ollama:m:redaction-v1")
        .unwrap();
    let cached: serde_json::Value = serde_json::from_str(&cached.unwrap()).unwrap();
    assert!(cached == serde_json::from_str::<serde_json::Value>(clean).unwrap());
}

/// What is stored is what was judged. A text can say more than the
/// structure read from it: of two members of one name a parser keeps the
/// last, and the text keeps both.
#[test]
fn redaction_generated_cache_write_stores_what_it_judged() {
    let token = token();
    let storage = open();
    let twice = [
        format!(r#"{{"entities":[{{"name":"{token}","name":"sample-service"}}]}}"#),
        format!(r#"{{"note":"{token}","entities":[],"note":"clean"}}"#),
    ];
    for json in &twice {
        let error = storage
            .llm_cache_put("h1", "ollama:m:redaction-v1", json)
            .unwrap_err();
        let said = format!("{error:#} {error:?}");
        assert!(said.contains("GENERATED_JSON_INVALID") && !said.contains(&token));
    }
    assert!(count(&storage, "llm_extraction_cache") == 0);

    // The text that is kept is the structure written out again, not the
    // text that was given.
    let spaced = "{ \"relations\" : [ ],  \"entities\" : [ ] }";
    storage
        .llm_cache_put("h2", "ollama:m:redaction-v1", spaced)
        .unwrap();
    let cached = storage
        .llm_cache_get("h2", "ollama:m:redaction-v1")
        .unwrap()
        .unwrap();
    let value: serde_json::Value = serde_json::from_str(&cached).unwrap();
    assert!(value == serde_json::json!({"entities": [], "relations": []}));
    let written = value.to_string();
    assert!(cached != spaced && cached == written);
}

/// Every writer of a retry record stores a code: the one it is given, or
/// the generic one for a text that is none.
#[test]
fn redaction_generated_retry_records_hold_a_code_whatever_is_passed() {
    let token = token();
    let message = format!("backend: POST http://user:{token}@host refused");
    let storage = open();
    let error_of = |id: &str| storage.pending_row(id).unwrap().unwrap().1;

    let id = saved(&storage);
    storage.enqueue_pending_extraction(&id, &message).unwrap();
    assert!(error_of(&id).as_deref() == Some("GENERATION_FAILED"));
    storage
        .enqueue_pending_extraction(&id, "BACKEND_FAILED")
        .unwrap();
    assert!(error_of(&id).as_deref() == Some("BACKEND_FAILED"));
    assert!(storage.mark_pending_attempted(&id, &message).unwrap());
    assert!(error_of(&id).as_deref() == Some("GENERATION_FAILED"));
    assert!(
        storage
            .mark_pending_attempted(&id, "STORAGE_FAILED")
            .unwrap()
    );
    assert!(error_of(&id).as_deref() == Some("STORAGE_FAILED"));

    // The dead letter of the first-attempt queue.
    let id = saved(&storage);
    storage.enqueue_extraction(&id).unwrap();
    assert!(storage.fail_extraction(&id, &message, 1).unwrap());
    assert!(error_of(&id).as_deref() == Some("GENERATION_FAILED"));
    let id = saved(&storage);
    storage.enqueue_extraction(&id).unwrap();
    assert!(storage.fail_extraction(&id, "WORKER_FAILED", 1).unwrap());
    assert!(error_of(&id).as_deref() == Some("WORKER_FAILED"));

    let conn = storage.conn.lock().unwrap();
    let held: i64 = conn
        .query_row(
            "SELECT count(*) FROM pending_extractions WHERE instr(last_error, ?1) > 0",
            params![token],
            |r| r.get(0),
        )
        .unwrap();
    assert!(held == 0);
}

/// A record written before the policy holds what it holds until it is
/// written again: the next attempt, which is given the old text back,
/// leaves a code in its place.
#[test]
fn redaction_generated_retry_of_a_record_from_before_the_policy_leaves_a_code() {
    let token = token();
    let message = format!("parse: expected value near {token}");
    let storage = open();
    let id = saved(&storage);
    storage
        .conn
        .lock()
        .unwrap()
        .execute(
            "INSERT INTO pending_extractions (memory_id, attempts, last_error, next_attempt_at)
             VALUES (?1, 0, ?2, datetime('now'))",
            params![id, message],
        )
        .unwrap();
    let before = storage.pending_row(&id).unwrap().unwrap().1.unwrap();
    assert!(before == message);
    assert!(storage.mark_pending_attempted(&id, &before).unwrap());
    let after = storage.pending_row(&id).unwrap().unwrap().1;
    assert!(after.as_deref() == Some("GENERATION_FAILED"));
}

/// The extraction queues name a memory by an id they cannot hold against
/// the memories: each judges the id it is handed.
#[test]
fn redaction_generated_extraction_queues_refuse_an_id_the_policy_refuses() {
    let token = token();
    let storage = open();
    for result in [
        storage.enqueue_extraction(&token),
        storage.enqueue_pending_extraction(&token, "BACKEND_FAILED"),
    ] {
        let error = result.unwrap_err();
        assert!(is_refused_write(&error));
        assert!(!format!("{error:#} {error:?}").contains(&token));
    }
    assert!(count(&storage, "extraction_queue") == 0);
    assert!(count(&storage, "pending_extractions") == 0);

    // A row queued before the policy is counted as any row is; its last
    // failure drops it, and the retry queue does not take the id.
    let queue = |storage: &Storage| {
        let conn = storage.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO extraction_queue (memory_id) VALUES (?1)",
            [&token],
        )
        .unwrap();
    };
    queue(&storage);
    assert!(
        !storage
            .fail_extraction(&token, "STORAGE_FAILED", 2)
            .unwrap()
    );
    assert!(count(&storage, "extraction_queue") == 1);
    let error = storage
        .fail_extraction(&token, "STORAGE_FAILED", 2)
        .unwrap_err();
    assert!(is_refused_write(&error));
    assert!(!format!("{error:#} {error:?}").contains(&token));
    assert!(count(&storage, "extraction_queue") == 0);
    assert!(count(&storage, "pending_extractions") == 0);

    // An admitted id is queued, and dead-lettered by its last failure.
    let id = saved(&storage);
    storage.enqueue_extraction(&id).unwrap();
    assert!(count(&storage, "extraction_queue") == 1);
    assert!(storage.fail_extraction(&id, "STORAGE_FAILED", 1).unwrap());
    assert!(count(&storage, "extraction_queue") == 0);
    assert!(count(&storage, "pending_extractions") == 1);
    let other = saved(&storage);
    storage
        .enqueue_pending_extraction(&other, "BACKEND_FAILED")
        .unwrap();
    assert!(count(&storage, "pending_extractions") == 2);
}
