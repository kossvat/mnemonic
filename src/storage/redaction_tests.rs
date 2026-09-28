//! The guard at every memory write, and import admission. Credential
//! fixtures are assembled at run time; assertions never print them.
use serde_json::json;

use super::*;
use crate::event::{EventSource, MemoryEntry, MemoryType};
use crate::redaction::SUMMARY_KEY;

fn body(n: usize) -> String {
    "a1B2c3D4e5F6".chars().cycle().take(n).collect()
}

fn token() -> String {
    ["sk-", "proj-", &body(40)].concat()
}

fn open() -> crate::test_support::InTempDir<Storage> {
    crate::test_support::InTempDir::new("mnemonic-redaction-memory-", |dir| {
        Storage::open(&dir.join("memory.db")).unwrap()
    })
}

fn entry(title: &str, content: &str) -> MemoryEntry {
    MemoryEntry::new(title, content, MemoryType::Note, EventSource::Manual)
}

fn count(storage: &Storage, sql: &str) -> i64 {
    let conn = storage.conn.lock().unwrap();
    conn.query_row(sql, [], |r| r.get(0)).unwrap()
}

/// Every text column a memory leaves behind, including its search index.
fn all_text(storage: &Storage) -> String {
    let conn = storage.conn.lock().unwrap();
    let mut out = String::new();
    for sql in [
        "SELECT title || ' ' || content || ' ' || tags || ' ' || metadata FROM memories",
        "SELECT title || ' ' || content || ' ' || tags FROM memories_fts",
    ] {
        let mut stmt = conn.prepare(sql).unwrap();
        for row in stmt.query_map([], |r| r.get::<_, String>(0)).unwrap() {
            out.push_str(&row.unwrap());
            out.push('\n');
        }
    }
    out
}

fn vector() -> Embedding {
    let mut v = vec![0.0; crate::embedding::EMBED_DIMS];
    v[0] = 1.0;
    v
}

/// The dirty shapes an unprepared caller could hand a write.
fn dirty_entries() -> Vec<(&'static str, MemoryEntry)> {
    let token = token();
    let mut in_tag = entry("t", "c");
    in_tag.tags = vec!["ok".into(), format!("tag-{token}")];
    let mut in_metadata = entry("t", "c");
    in_metadata.metadata = json!({"note": format!("see {token}")});
    let mut in_project = entry("t", "c");
    in_project.metadata = json!({"project": "p", "project_key": format!("p-{token}")});
    let mut in_id = entry("t", "c");
    in_id.id = format!("m-{token}");
    let mut forged = entry("t", "c");
    let mut map = serde_json::Map::new();
    map.insert(SUMMARY_KEY.into(), json!({"changed": true}));
    forged.metadata = serde_json::Value::Object(map);
    vec![
        ("content", entry("t", &format!("use {token} here"))),
        ("title", entry(&format!("key {token}"), "c")),
        ("private block", entry("t", "a <private>b</private> c")),
        ("tag", in_tag),
        ("metadata narrative", in_metadata),
        ("structural project key", in_project),
        ("id", in_id),
        ("forged summary", forged),
    ]
}

#[test]
fn redaction_memory_every_insert_refuses_an_unprepared_entry() {
    let token = token();
    let storage = open();
    let vec = vector();
    for (name, e) in dirty_entries() {
        assert!(storage.save(&e).is_err(), "save: {name}");
        assert!(
            storage.save_with_embedding(&e, Some(&vec)).is_err(),
            "save_with_embedding: {name}"
        );
        assert!(
            storage
                .save_with_links(&e, Some(&vec), None, "test")
                .is_err(),
            "save_with_links: {name}"
        );
        assert!(
            storage
                .apply_reflection("run", &e, Some(&vec), &[])
                .is_err(),
            "apply_reflection: {name}"
        );
    }
    assert!(count(&storage, "SELECT count(*) FROM memories") == 0);
    assert!(count(&storage, "SELECT count(*) FROM memory_updates") == 0);
    assert!(!all_text(&storage).contains(&token));

    // The prepared shape of the same text is stored, its index and vector
    // with it, and a repeated save of the same id still replaces in place.
    let prepared = crate::redaction::prepare_entry(
        entry("t", &format!("use {token} here <private>x</private>")),
        crate::redaction::STRUCTURAL_KEYS,
    )
    .unwrap()
    .into_entry();
    storage.save_with_embedding(&prepared, Some(&vec)).unwrap();
    storage.save_with_embedding(&prepared, Some(&vec)).unwrap();
    assert!(count(&storage, "SELECT count(*) FROM memories") == 1);
    assert!(
        count(
            &storage,
            "SELECT count(*) FROM memories WHERE embedding IS NOT NULL"
        ) == 1
    );
    let text = all_text(&storage);
    assert!(!text.contains(&token) && !text.contains(">x<"));
    assert!(text.contains("[REDACTED:credential]") && text.contains("[REDACTED:private]"));
    let stored = storage.get_by_id(&prepared.id).unwrap().unwrap();
    assert!(stored.metadata[SUMMARY_KEY]["counts"]["provider_token"] == 1);

    let linked = entry("t2", "another clean note");
    assert!(
        storage
            .save_with_links(&linked, Some(&vec), None, "test")
            .unwrap()
            .is_none()
    );
    assert!(count(&storage, "SELECT count(*) FROM memories") == 2);
}

/// One export record, as `export` writes it: tags, source and metadata are
/// serialized JSON strings.
fn record(id: &str, content: &str, tags: &str, metadata: &str) -> serde_json::Value {
    json!({
        "id": id,
        "timestamp": "2026-01-02T03:04:05Z",
        "title": "imported",
        "content": content,
        "memory_type": "decision",
        "tags": tags,
        "source": "\"Manual\"",
        "importance": 0.6,
        "metadata": metadata,
    })
}

/// What a canonical is said to come from is written with it: the store
/// judges the run and the sources itself, whoever calls it.
#[test]
fn redaction_memory_reflection_store_refuses_a_sensitive_provenance() {
    let token = token();
    let storage = open();
    let source = entry("use SQLite for the index", "use SQLite for the index");
    storage.save(&source).unwrap();
    let canonical = entry("Consolidated", "use SQLite for the index");
    let run = storage.begin_reflection_run("apply", 0.9, "rule").unwrap();
    let refused = [
        storage.apply_reflection(&token, &canonical, None, &[(source.id.clone(), 1.0)]),
        storage.apply_reflection(&run, &canonical, None, &[(token.clone(), 1.0)]),
        storage.apply_reflection(
            &run,
            &canonical,
            None,
            &[(source.id.clone(), 1.0), (token.to_uppercase(), 1.0)],
        ),
    ];
    for result in refused {
        let error = result.unwrap_err();
        assert!(is_refused_write(&error));
        assert!(!format!("{error:#} {error:?}").contains(&token));
    }
    assert!(storage.get_by_id(&canonical.id).unwrap().is_none());
    let conn = storage.conn.lock().unwrap();
    let written: i64 = conn
        .query_row("SELECT count(*) FROM reflection_sources", [], |r| {
            r.get::<_, i64>(0)
        })
        .unwrap();
    assert!(written == 0);
    drop(conn);
    let applied = storage
        .apply_reflection(&run, &canonical, None, &[(source.id.clone(), 1.0)])
        .unwrap();
    assert!(applied.as_deref() == Some(canonical.id.as_str()));
}

/// A run is written under the names it is given: the store judges them.
#[test]
fn redaction_memory_reflection_run_refuses_a_sensitive_name() {
    let token = token();
    let storage = open();
    for result in [
        storage.begin_reflection_run(&token, 0.9, "rule"),
        storage.begin_reflection_run("apply", 0.9, &token),
    ] {
        let error = result.unwrap_err();
        assert!(is_refused_write(&error));
        assert!(!format!("{error:#} {error:?}").contains(&token));
    }
    assert!(count(&storage, "SELECT count(*) FROM reflection_runs") == 0);
    storage.begin_reflection_run("apply", 0.9, "rule").unwrap();
    assert!(count(&storage, "SELECT count(*) FROM reflection_runs") == 1);
}

#[test]
fn redaction_memory_import_prepares_records_and_refuses_identities() {
    let token = token();
    let storage = open();
    let dirty = record(
        "imp-1",
        &format!("the key is {token}"),
        &format!("[\"ok\", \"tag-{token}\"]"),
        &format!("{{\"note\": \"see {token}\"}}"),
    );
    assert!(
        storage
            .import_entries(std::slice::from_ref(&dirty))
            .unwrap()
            == (1, 0)
    );
    let stored = storage.get_by_id("imp-1").unwrap().unwrap();
    let text = format!(
        "{} {} {:?} {}",
        stored.title, stored.content, stored.tags, stored.metadata
    );
    assert!(!text.contains(&token));
    assert!(stored.content.contains("[REDACTED:credential]"));
    assert!(stored.tags == vec!["ok".to_string(), "tag-[REDACTED:credential]".to_string()]);
    assert!(stored.metadata["note"] == "see [REDACTED:credential]");
    assert!(stored.metadata[SUMMARY_KEY]["counts"]["provider_token"] == 3);
    assert!(stored.memory_type == MemoryType::Decision);
    assert!(stored.source == EventSource::Manual);
    assert!(stored.timestamp.to_rfc3339() == "2026-01-02T03:04:05+00:00");
    // The same id again is a duplicate, as before.
    assert!(storage.import_entries(&[dirty]).unwrap() == (0, 1));
    assert!(!all_text(&storage).contains(&token));

    // A record that cannot be admitted refuses the whole file, by index and
    // fixed code, and nothing of it is written.
    let clean = record("imp-2", "plain", "[]", "{}");
    for (name, bad) in [
        (
            "structural identity",
            record(
                "imp-3",
                "plain",
                "[]",
                &format!("{{\"project_key\": \"p-{token}\"}}"),
            ),
        ),
        ("id", record(&format!("imp-{token}"), "plain", "[]", "{}")),
        ("tags not json", record("imp-4", "plain", "not json", "{}")),
        ("metadata not json", record("imp-5", "plain", "[]", "{oops")),
        (
            "timestamp",
            json!({"id": "imp-6", "timestamp": "yesterday", "title": "t", "content": "c"}),
        ),
    ] {
        let err = storage
            .import_entries(&[clean.clone(), bad])
            .unwrap_err()
            .to_string();
        assert!(err.contains("record 1"), "{name}");
        assert!(!err.contains(&token), "{name}: the error names the value");
        assert!(
            storage.get_by_id("imp-2").unwrap().is_none(),
            "{name}: partial import"
        );
    }
    assert!(!all_text(&storage).contains(&token));

    // A record whose metadata sits just under the structure limit still
    // passes the write's guard once its summary is added: the whole file
    // is judged before any row is written, in one transaction.
    let nulls = vec![serde_json::Value::Null; 99_990];
    let big = json!({"filler": nulls}).to_string();
    let near_limit = record("imp-8", &format!("key {token}"), "[]", &big);
    let err = storage
        .import_entries(&[clean.clone(), near_limit])
        .unwrap_err()
        .to_string();
    assert!(err.contains("record 1") && err.contains("STRUCTURE_TOO_LARGE"));
    assert!(
        storage.get_by_id("imp-2").unwrap().is_none(),
        "a partial import"
    );
    let fits = json!({"filler": vec![serde_json::Value::Null; 99_900]}).to_string();
    let under = record("imp-9", &format!("key {token}"), "[]", &fits);
    assert!(storage.import_entries(&[under]).unwrap() == (1, 0));
    assert!(storage.get_by_id("imp-9").unwrap().is_some());

    // Times the store's readers accept are imported too: RFC 3339 and the
    // naive forms of hand-written files, taken as UTC.
    let mut naive = record("imp-10", "plain", "[]", "{}");
    naive["timestamp"] = json!("2024-01-01 12:00:00");
    let mut naive_t = record("imp-11", "plain", "[]", "{}");
    naive_t["timestamp"] = json!("2024-01-01T12:00:00.5");
    assert!(storage.import_entries(&[naive, naive_t]).unwrap() == (2, 0));
    let stored = storage.get_by_id("imp-10").unwrap().unwrap();
    assert!(stored.timestamp.to_rfc3339() == "2024-01-01T12:00:00+00:00");

    // A clean record carries no summary, and a forged one is dropped.
    let forged = record(
        "imp-7",
        "plain",
        "[]",
        &format!("{{\"{SUMMARY_KEY}\": {{\"changed\": true, \"counts\": {{\"jwt\": 9}}}}}}"),
    );
    assert!(storage.import_entries(&[forged]).unwrap() == (1, 0));
    let stored = storage.get_by_id("imp-7").unwrap().unwrap();
    assert!(stored.metadata.get(SUMMARY_KEY).is_none());
}
