//! Facts under the redaction policy: a statement is refused before any
//! effect, its note is prepared, an old row is held back as it is.
//! Credential fixtures are assembled at run time; assertions never print
//! them.
use std::sync::Mutex;

use rusqlite::Connection;

use super::declare::declare;
use super::store::{FactWrite, apply, forget_value, live_source};
use super::{admit, legacy};
use crate::embedding::{Embedder, Embedding};
use crate::redaction::SUMMARY_KEY;
use crate::redaction::state::is_refused;
use crate::storage::Storage;
use crate::test_support::{ConstEmbedder, InTempDir, legacy_add_fact_sql, legacy_facts_fixture};

fn body(n: usize) -> String {
    "a1B2c3D4e5F6".chars().cycle().take(n).collect()
}

fn token() -> String {
    ["sk-", "proj-", &body(40)].concat()
}

fn store() -> InTempDir<Storage> {
    InTempDir::new("mnemonic-fact-redaction-", |dir| {
        Storage::open(&dir.join("memory.db")).unwrap()
    })
}

/// Remembers every text it was asked to embed.
#[derive(Default)]
struct Recording(Mutex<Vec<String>>);

impl Embedder for Recording {
    fn embed(&self, text: &str) -> anyhow::Result<Embedding> {
        self.0.lock().unwrap().push(text.to_string());
        ConstEmbedder.embed(text)
    }

    fn model_id(&self) -> &'static str {
        "recording-test"
    }
}

fn rows(storage: &Storage) -> [i64; 4] {
    let conn = storage.conn.lock().unwrap();
    ["memories", "fact_slots", "fact_values", "fact_events"].map(|table| {
        conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    })
}

fn host<'a>() -> FactWrite<'a> {
    FactWrite {
        project: Some("demoapp"),
        subject: "api",
        predicate: "host",
        value: Some("db.internal"),
        actor: "test",
        ..Default::default()
    }
}

#[test]
fn redaction_state_fact_declare_refuses_before_any_effect() {
    let token = token();
    let value = body(24);
    let s = store();
    let embedder = Recording::default();
    let dirty = [
        FactWrite {
            value: Some(&token),
            ..host()
        },
        FactWrite {
            predicate: "password",
            value: Some(&value),
            ..host()
        },
        FactWrite {
            project: Some(&token),
            ..host()
        },
        FactWrite {
            subject: &token,
            ..host()
        },
        FactWrite {
            qualifier: Some(&token),
            ..host()
        },
        FactWrite {
            request_id: Some(&token),
            ..host()
        },
        FactWrite {
            agent: Some(&token),
            ..host()
        },
        // Clean as written, filed under the key `api-key`.
        FactWrite {
            predicate: "api key",
            value: Some(&value),
            ..host()
        },
        FactWrite {
            qualifier: Some("api  key"),
            value: Some(&value),
            ..host()
        },
    ];
    for (i, write) in dirty.iter().enumerate() {
        let error = declare(&s, &embedder, 0.92, write, Some("a plain note"))
            .err()
            .unwrap_or_else(|| panic!("statement {i} was admitted"));
        assert!(is_refused(&error), "statement {i}");
        let text = format!("{error:#}");
        assert!(text.contains("SENSITIVE_CONTENT"), "statement {i}");
        assert!(
            !text.contains(&token) && !text.contains(&value),
            "statement {i}"
        );
    }
    assert!(
        embedder.0.lock().unwrap().is_empty(),
        "a refused fact was embedded"
    );
    assert!(rows(&s) == [0; 4]);
}

#[test]
fn redaction_state_fact_note_is_prepared_and_the_fact_stands() {
    let token = token();
    let s = store();
    let embedder = Recording::default();
    let note = format!("from the runbook, key {token}");
    let declared = declare(&s, &embedder, 0.92, &host(), Some(&note)).unwrap();
    assert!(declared.outcome.outcome == "create");
    let current = declared.outcome.fact.current.unwrap();
    assert!(current.value.as_deref() == Some("db.internal"));
    let memory = declared.memory.unwrap();
    assert!(memory.title == "api host: db.internal");
    assert!(memory.content.starts_with("api host is db.internal."));
    assert!(memory.content.contains("[REDACTED:credential]"));
    assert!(!memory.content.contains(&token));
    assert!(memory.metadata[SUMMARY_KEY]["counts"]["provider_token"] == 1);
    assert!(memory.metadata["fact"]["value"] == "db.internal");
    assert!(current.source_memory_id.as_deref() == Some(memory.id.as_str()));
    let embedded = embedder.0.lock().unwrap();
    assert!(!embedded.is_empty() && embedded.iter().all(|t| !t.contains(&token)));
    // The stored row is the prepared one.
    let conn = s.conn.lock().unwrap();
    let stored: String = conn
        .query_row(
            "SELECT title || content || tags || metadata FROM memories WHERE id = ?1",
            [&memory.id],
            |r| r.get(0),
        )
        .unwrap();
    assert!(!stored.contains(&token));
}

/// A request id that was applied before returns its earlier success; one
/// that comes with a credential has no success to return.
#[test]
fn redaction_state_fact_direct_apply_refuses_before_replay() {
    let token = token();
    let s = store();
    let safe = FactWrite {
        request_id: Some("req-1"),
        ..host()
    };
    assert!(apply(&s, &safe).unwrap().outcome == "create");
    let before = rows(&s);
    for write in [
        FactWrite {
            value: Some(&token),
            ..safe.clone()
        },
        FactWrite {
            subject: &token,
            ..safe.clone()
        },
        FactWrite {
            evidence: None,
            source_memory_id: Some(&token),
            ..safe.clone()
        },
    ] {
        let error = apply(&s, &write).unwrap_err();
        assert!(is_refused(&error));
        assert!(!format!("{error:#}").contains(&token));
    }
    assert!(rows(&s) == before);
    // The safe retry is still a replay.
    let again = apply(&s, &safe).unwrap();
    assert!(again.replayed);
    assert!(rows(&s) == before);
}

#[test]
fn redaction_state_fact_evidence_is_stored_prepared() {
    let token = token();
    let s = store();
    let evidence = format!("seen with {token} in the log");
    apply(
        &s,
        &FactWrite {
            evidence: Some(&evidence),
            ..host()
        },
    )
    .unwrap();
    let conn = s.conn.lock().unwrap();
    let stored: String = conn
        .query_row("SELECT evidence FROM fact_values", [], |r| r.get(0))
        .unwrap();
    assert!(stored == "seen with [REDACTED:credential] in the log");
}

#[test]
fn redaction_state_fact_lookups_refuse_without_echo() {
    let token = token();
    let s = store();
    for error in [
        forget_value(&s, &token).unwrap_err(),
        live_source(&s, Some(&token)).unwrap_err(),
    ] {
        assert!(is_refused(&error));
        assert!(!format!("{error:#}").contains(&token));
    }
    assert!(admit::admit(&host()).is_ok());
}

/// An admitted statement is recorded exactly as it was stated: the memory
/// of a fact about a credential shows its time, and carries no summary.
#[test]
fn redaction_state_fact_memory_shows_an_admitted_statement_as_stated() {
    let s = store();
    let time = "2026-12-01T00:00:00Z";
    for (i, write) in [
        FactWrite {
            subject: "GITHUB_TOKEN",
            predicate: "expires",
            value: Some(time),
            ..host()
        },
        FactWrite {
            predicate: "password",
            qualifier: Some("staging"),
            value: Some(time),
            ..host()
        },
        host(),
    ]
    .iter()
    .enumerate()
    {
        let declared = declare(&s, &ConstEmbedder, 0.92, write, None).unwrap();
        let memory = declared.memory.unwrap();
        let (title, content) = admit::statement(write);
        assert!(
            memory.title == title && memory.content == content,
            "fact {i}"
        );
        assert!(memory.metadata.get(SUMMARY_KEY).is_none(), "fact {i}");
        assert!(memory.metadata["fact"]["value"] == write.value.unwrap());
    }
    // The one that reads as a credential is refused, not recorded altered.
    let reads = FactWrite {
        predicate: "password",
        value: Some(time),
        ..host()
    };
    let before = rows(&s);
    let error = declare(&s, &ConstEmbedder, 0.92, &reads, None)
        .err()
        .unwrap();
    assert!(is_refused(&error));
    assert!(rows(&s) == before);
}

/// A message about a revision that does not match shows the current value,
/// unless it is one stored before the policy that the policy refuses.
#[test]
fn redaction_state_fact_mismatch_does_not_show_a_refused_stored_value() {
    let token = token();
    let s = store();
    apply(&s, &host()).unwrap();
    let stale = FactWrite {
        value: Some("db2.internal"),
        expected_revision: Some(99),
        ..host()
    };
    let text = format!("{:#}", apply(&s, &stale).unwrap_err());
    assert!(text.contains("revision mismatch") && text.contains("db.internal"));
    s.conn
        .lock()
        .unwrap()
        .execute("UPDATE fact_values SET value = ?1", [&token])
        .unwrap();
    let text = format!("{:#}", apply(&s, &stale).unwrap_err());
    assert!(text.contains("revision mismatch") && !text.contains(&token));
}

/// The slot that is already there keeps the spelling of its first
/// statement: a value filed into it is judged under that spelling too.
#[test]
fn redaction_state_fact_is_judged_under_the_label_of_its_slot() {
    let value = body(24);
    let s = store();
    let first = FactWrite {
        predicate: "DEPLOY_TOKEN",
        value: Some("kept in the vault"),
        ..host()
    };
    assert!(apply(&s, &first).unwrap().outcome == "create");
    let before = rows(&s);
    let second = FactWrite {
        predicate: "deploy-token",
        value: Some(&value),
        ..host()
    };
    assert!(admit::check(&second).is_ok(), "clean as it is written");
    let error = apply(&s, &second).unwrap_err();
    assert!(is_refused(&error));
    assert!(!format!("{error:#}").contains(&value));
    assert!(rows(&s) == before);
    // Without such a slot the same statement is a fact.
    let other = store();
    assert!(apply(&other, &second).unwrap().outcome == "create");
}

/// An alias or a project stored before the policy can lead to a name it
/// refuses: what a field resolves to is judged, before anything is
/// embedded or written.
#[test]
fn redaction_state_fact_names_are_judged_as_they_resolve() {
    let secret = format!("password={}", body(24).to_lowercase());
    let s = store();
    {
        let conn = s.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO entities (id, name, entity_type, mention_count, first_seen, last_seen)
             VALUES ('legacy', ?1, 'project', 1, datetime('now'), datetime('now'))",
            [&secret],
        )
        .unwrap();
        for alias in ["demo", "gadget"] {
            conn.execute(
                "INSERT INTO entity_aliases (alias, canonical) VALUES (?1, ?2)",
                [alias, secret.as_str()],
            )
            .unwrap();
        }
    }
    let embedder = Recording::default();
    for write in [
        FactWrite {
            project: Some("demo"),
            ..host()
        },
        FactWrite {
            subject: "gadget",
            ..host()
        },
        FactWrite {
            project: Some("demo"),
            value: None,
            ..host()
        },
    ] {
        assert!(admit::check(&write).is_ok(), "clean as it is written");
        for error in [
            apply(&s, &write).unwrap_err(),
            declare(&s, &embedder, 0.92, &write, None).err().unwrap(),
        ] {
            assert!(is_refused(&error));
            let text = format!("{error:#}");
            assert!(text.contains("in a name as the store resolves it"));
            assert!(!text.contains(&secret));
        }
    }
    assert!(embedder.0.lock().unwrap().is_empty());
    assert!(rows(&s) == [0; 4]);
}

/// An old binary's rows: the clean ones become facts; of the others
/// nothing is copied, the old table stays as it is, and they are counted.
/// A row whose names are admitted ends the value before it: the slot then
/// has no current value, not an older one.
#[test]
fn redaction_state_fact_catch_up_holds_back_sensitive_rows() {
    let token = token();
    let value = body(24);
    let dir = crate::test_support::temp_dir("mnemonic-fact-redaction-legacy-");
    let path = dir.path().join("memory.db");
    drop(Storage::open(&path).unwrap());
    let conn = Connection::open(&path).unwrap();
    let seeded = legacy_facts_fixture(&conn);
    let (t1, t2) = (
        "2026-01-02T03:04:05.000000+00:00",
        "2026-02-03T04:05:06.000000+00:00",
    );
    // A chain whose older value is clean and whose current one is not.
    let older = legacy_add_fact_sql(&conn, "svc", "endpoint", "https://old.example", t1);
    let withheld = [
        legacy_add_fact_sql(&conn, "svc", "endpoint", &token, t2),
        legacy_add_fact_sql(&conn, "deploy", "password", &value, t1),
    ];
    // What names these is refused: nothing of them enters the new tables.
    // The third one's key would read clean (`password-...`).
    let held = [
        legacy_add_fact_sql(&conn, &token, "owner", "ops", t1),
        legacy_add_fact_sql(&conn, "deploy", &format!("password={value}"), "x", t1),
        legacy_add_fact_sql(&conn, &format!("password={value}"), "owner", "ops", t1),
    ];
    let raw = |id: &str, valid_to: Option<&str>| {
        conn.execute(
            "INSERT INTO facts (id, subject, predicate, value, valid_from, valid_to, confidence,
                 source_memory_id, created_at)
             VALUES (?1, 'deploy', 'zone', 'eu', ?2, ?3, 1.0, 'manual', ?2)",
            rusqlite::params![id, t1, valid_to],
        )
        .unwrap();
    };
    raw(&token, Some(t2));
    raw("plain-id", Some(&token));
    // Keys that cannot be read (a predicate too long for a key) and an id
    // the policy refuses: not filed, and not listed by its id either.
    let unreadable = [&token, "-b"].concat();
    conn.execute(
        "INSERT INTO facts (id, subject, predicate, value, valid_from, valid_to, confidence,
             source_memory_id, created_at)
         VALUES (?1, 'deploy', ?2, 'eu', ?3, NULL, 1.0, 'manual', ?3)",
        rusqlite::params![unreadable, "p".repeat(60), t1],
    )
    .unwrap();
    // A subject that is clean as written and an alias of a name stored
    // before the policy: its key would be made from that name.
    let target = format!("password={}", value.to_lowercase());
    conn.execute(
        "INSERT INTO entity_aliases (alias, canonical) VALUES ('sample-svc', ?1)",
        [&target],
    )
    .unwrap();
    let aliased = legacy_add_fact_sql(&conn, "sample-svc", "owner", "ops", t1);
    // Keys that cannot be read, names and id that are clean, and a value
    // that is not: it has no slot to be withheld in, and is only counted.
    conn.execute(
        "INSERT INTO facts (id, subject, predicate, value, valid_from, valid_to, confidence,
             source_memory_id, created_at)
         VALUES ('unreadable-2', 'deploy', ?1, ?2, ?3, NULL, 1.0, 'manual', ?3)",
        rusqlite::params!["q".repeat(60), token, t1],
    )
    .unwrap();
    let clean = legacy_add_fact_sql(&conn, "deploy", "region", "eu-west", t1);
    type Old = (String, String, String, String, Option<String>);
    let old_rows = |conn: &Connection| -> Vec<Old> {
        let mut stmt = conn
            .prepare("SELECT id, subject, predicate, value, valid_to FROM facts ORDER BY id")
            .unwrap();
        stmt.query_map([], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
    };
    let before = old_rows(&conn);
    drop(conn);

    let storage = Storage::open(&path).unwrap();
    {
        let conn = storage.conn.lock().unwrap();
        let audit = legacy::audit(&conn).unwrap();
        assert!(audit.legacy_rows == seeded.rows + 12);
        assert!(audit.imported == seeded.rows + 2, "the two clean rows");
        assert!(audit.held_back == 10);
        assert!(
            audit.not_imported.is_empty(),
            "held-back rows are only counted"
        );
        assert!(audit.mismatched_current.is_empty());
        assert!(!audit.clean());
        assert!(!serde_json::to_string(&audit).unwrap().contains(&token));
        // Nothing of a held-back row was copied, in any column.
        for table in ["fact_slots", "fact_values", "fact_events"] {
            let mut stmt = conn.prepare(&format!("SELECT * FROM {table}")).unwrap();
            let columns = stmt.column_count();
            let mut found = stmt.query([]).unwrap();
            while let Some(row) = found.next().unwrap() {
                for i in 0..columns {
                    if let Ok(Some(text)) = row.get::<_, Option<String>>(i) {
                        assert!(
                            !text.contains(&token)
                                && !text.to_lowercase().contains(&value.to_lowercase()),
                            "{table} holds a held-back value"
                        );
                    }
                }
            }
        }
        let twin = |id: &str| -> Option<(String, String)> {
            conn.query_row(
                "SELECT kind, value FROM fact_values WHERE legacy_fact_id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .ok()
        };
        for id in &withheld {
            assert!(twin(id) == Some(("retraction".into(), String::new())));
        }
        for id in held.iter().map(String::as_str).chain([
            token.as_str(),
            "plain-id",
            aliased.as_str(),
            unreadable.as_str(),
            "unreadable-2",
        ]) {
            assert!(twin(id).is_none());
        }
        assert!(twin(&clean) == Some(("value".into(), "eu-west".into())));
        assert!(twin(&older) == Some(("value".into(), "https://old.example".into())));
        assert!(old_rows(&conn) == before);
        // Rows held back whole are no reason to catch up again.
        assert!(!legacy::needs_catch_up(&conn).unwrap());
    }
    // The older value ended where the old binary ended it.
    let views = crate::facts::view::views(&storage, None, "svc").unwrap();
    assert!(views.len() == 1);
    assert!(
        views[0].current.is_none(),
        "an ended value reads as current"
    );
    let ended = views[0].history.iter().find(|v| v.value.is_some()).unwrap();
    assert!(ended.valid_to.is_some());
}
