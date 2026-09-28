//! State writers in the store under the redaction policy: conclusions,
//! conflicts, peers, sessions, the graph, rename/merge and rekey. Each
//! refusal is checked for leaving nothing behind. Credential fixtures are
//! assembled at run time; assertions never print them.
use super::*;
use crate::event::{EventSource, MemoryEntry, MemoryType};
use crate::graph::{Edge, Entity, EntityType};
use crate::redaction::state::is_refused;

fn body(n: usize) -> String {
    "a1B2c3D4e5F6".chars().cycle().take(n).collect()
}

fn token() -> String {
    ["sk-", "proj-", &body(40)].concat()
}

fn open() -> crate::test_support::InTempDir<Storage> {
    crate::test_support::InTempDir::new("mnemonic-redaction-state-", |dir| {
        Storage::open(&dir.join("memory.db")).unwrap()
    })
}

fn count(storage: &Storage, table: &str) -> i64 {
    let conn = storage.conn.lock().unwrap();
    conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

/// Every text column of `tables`, joined: what a leak would show up in.
fn dump(storage: &Storage, tables: &[&str]) -> String {
    let conn = storage.conn.lock().unwrap();
    let mut out = String::new();
    for table in tables {
        let mut stmt = conn.prepare(&format!("SELECT * FROM {table}")).unwrap();
        let columns = stmt.column_count();
        let mut rows = stmt.query([]).unwrap();
        while let Some(row) = rows.next().unwrap() {
            for i in 0..columns {
                if let Ok(Some(text)) = row.get::<_, Option<String>>(i) {
                    out.push_str(&text);
                    out.push('\n');
                }
            }
        }
    }
    out
}

fn refused(error: anyhow::Error, secret: &str) {
    assert!(is_refused(&error), "not a refusal");
    let text = format!("{error:#}");
    assert!(text.contains("SENSITIVE_CONTENT"));
    assert!(!text.contains(secret), "the error echoes its input");
}

fn memory(storage: &Storage, title: &str) -> String {
    let entry = MemoryEntry::new(title, "body", MemoryType::Note, EventSource::Manual);
    storage.save(&entry).unwrap();
    entry.id
}

fn entity(name: &str) -> Entity {
    Entity {
        name: name.into(),
        entity_type: EntityType::Project,
    }
}

fn edge(source: &str, target: &str, relation: &str, memory_id: &str) -> Edge {
    Edge {
        source: source.into(),
        target: target.into(),
        relation: relation.into(),
        memory_id: memory_id.into(),
    }
}

#[test]
fn redaction_state_conclusions_refuse_identities_and_prepare_statements() {
    let token = token();
    let s = open();
    let source = memory(&s, "a plain note");
    for error in [
        s.add_conclusion(&token, "pattern", "plain", 0.5, &[])
            .unwrap_err(),
        s.add_conclusion("user", &token, "plain", 0.5, &[])
            .unwrap_err(),
        s.add_conclusion(
            "user",
            "pattern",
            "plain",
            0.5,
            std::slice::from_ref(&token),
        )
        .unwrap_err(),
        s.supersede_conclusion(&token, "other").unwrap_err(),
        s.supersede_conclusion("other", &token).unwrap_err(),
        s.delete_conclusion(&token).unwrap_err(),
    ] {
        refused(error, &token);
    }
    assert!(count(&s, "conclusions") == 0);
    assert!(count(&s, "conclusion_sources") == 0);

    let id = s
        .add_conclusion(
            "user",
            "pattern",
            &format!("keeps {token} in the shell profile"),
            0.5,
            std::slice::from_ref(&source),
        )
        .unwrap();
    let stored = s.conclusion_by_id(&id).unwrap().unwrap();
    assert!(stored.statement == "keeps [REDACTED:credential] in the shell profile");
    assert!(stored.subject == "user");
    assert!(s.conclusion_sources(&id).unwrap() == vec![source]);
    assert!(!dump(&s, &["conclusions"]).contains(&token));
}

#[test]
fn redaction_state_conflicts_refuse_identities_and_prepare_the_reason() {
    let token = token();
    let s = open();
    let (old, new) = (memory(&s, "use sqlite"), memory(&s, "use postgres"));
    for error in [
        s.upsert_conflict(&token, &new, "demoapp", "candidate", None, None)
            .unwrap_err(),
        s.upsert_conflict(&old, &token, "demoapp", "candidate", None, None)
            .unwrap_err(),
        s.upsert_conflict(&old, &new, &token, "candidate", None, None)
            .unwrap_err(),
        s.upsert_conflict(&old, &new, "demoapp", &token, None, None)
            .unwrap_err(),
    ] {
        refused(error, &token);
    }
    assert!(count(&s, "decision_conflicts") == 0);
    let reason = format!("the second one names {token}");
    s.upsert_conflict(&old, &new, "demoapp", "confirmed", Some(0.9), Some(&reason))
        .unwrap();
    let text = dump(&s, &["decision_conflicts"]);
    assert!(text.contains("the second one names [REDACTED:credential]"));
    assert!(!text.contains(&token));
}

#[test]
fn redaction_state_peers_refuse_names_before_they_are_folded() {
    let token = token();
    let upper = token.to_uppercase();
    let s = open();
    let kept = s.upsert_peer("claude", Some("Claude"), "agent").unwrap();
    let before = dump(&s, &["peers"]);
    for error in [
        s.upsert_peer(&token, None, "agent").unwrap_err(),
        s.upsert_peer("codex", Some(&token), "agent").unwrap_err(),
        s.upsert_peer("codex", None, &token).unwrap_err(),
        // An existing peer is not touched either.
        s.upsert_peer("claude", Some(&token), "agent").unwrap_err(),
        s.merge_peers(&token, "claude").unwrap_err(),
        s.merge_peers("claude", &token).unwrap_err(),
    ] {
        refused(error, &token);
    }
    assert!(dump(&s, &["peers"]) == before);
    assert!(count(&s, "peers") == 1);
    let note = memory(&s, "a plain note");
    for error in [
        s.link_memory_peer(&token, &kept, "speaker").unwrap_err(),
        s.link_memory_peer(&note, &token, "speaker").unwrap_err(),
        s.link_memory_peer(&note, &kept, &token).unwrap_err(),
    ] {
        refused(error, &token);
    }
    assert!(count(&s, "memory_peers") == 0);
    s.link_memory_peer(&note, &kept, "speaker").unwrap();
    assert!(count(&s, "memory_peers") == 1);
    assert!(
        !dump(&s, &["peers", "memory_peers"])
            .to_uppercase()
            .contains(&upper)
    );
}

#[test]
fn redaction_state_sessions_refuse_keys_and_prepare_labels() {
    let token = token();
    let s = open();
    let peer = s.upsert_peer("claude", None, "agent").unwrap();
    let key = "/work/transcripts/demo/1.jsonl";
    let first = s
        .open_or_reuse_session_for_key(&peer, key, Some("demo/1"), "jsonl", 3600)
        .unwrap();
    let before = dump(&s, &["sessions"]);
    for error in [
        s.open_or_reuse_session_for_key(&token, key, None, "jsonl", 3600)
            .unwrap_err(),
        s.open_or_reuse_session_for_key(&peer, &format!("/p/{token}/1.jsonl"), None, "jsonl", 3600)
            .unwrap_err(),
        s.open_or_reuse_session_for_key(&peer, key, None, &token, 3600)
            .unwrap_err(),
        s.open_session(&token, None, "jsonl").unwrap_err(),
        s.open_session(&peer, None, &token).unwrap_err(),
        s.end_session_at(&first, &token).unwrap_err(),
        s.end_session_at(&token, "2026-01-02T03:04:05Z")
            .unwrap_err(),
        s.set_memory_session(&token, Some(&first)).unwrap_err(),
        s.set_memory_session("m1", Some(&token)).unwrap_err(),
    ] {
        refused(error, &token);
    }
    // Not even the activity time of the open session moved.
    assert!(dump(&s, &["sessions"]) == before);

    let labelled = s
        .open_session(&peer, Some(&format!("debugging with {token}")), "jsonl")
        .unwrap();
    let other = s
        .open_or_reuse_session_for_key(
            &peer,
            "/work/transcripts/demo/2.jsonl",
            Some(&format!("demo {token}")),
            "jsonl",
            3600,
        )
        .unwrap();
    let sessions = s.sessions_for_peer(&peer, 10).unwrap();
    let label = |id: &str| {
        sessions
            .iter()
            .find(|x| x.id == id)
            .and_then(|x| x.label.clone())
            .unwrap()
    };
    assert!(label(&labelled) == "debugging with [REDACTED:credential]");
    assert!(label(&other) == "demo [REDACTED:credential]");
    assert!(label(&first) == "demo/1");
    assert!(!dump(&s, &["sessions"]).contains(&token));
}

#[test]
fn redaction_state_graph_writers_refuse_a_name_and_leave_nothing() {
    let token = token();
    let s = open();
    let note = memory(&s, "a plain note");
    for error in [
        s.upsert_entity(&entity(&token)).unwrap_err(),
        s.save_edge(&edge(&token, "sqlite", "uses", &note))
            .unwrap_err(),
        s.save_edge(&edge("demoapp", &token, "uses", &note))
            .unwrap_err(),
        s.save_edge(&edge("demoapp", "sqlite", &token, &note))
            .unwrap_err(),
        s.save_edge(&edge("demoapp", "sqlite", "uses", &token))
            .unwrap_err(),
        s.link_memory_entity(&token, "e1").unwrap_err(),
        s.link_memory_entity(&note, &token).unwrap_err(),
        // The whole graph is judged first: its clean head is not written.
        s.save_graph(&note, &[entity("demoapp"), entity(&token)], &[])
            .unwrap_err(),
        s.save_graph(
            &note,
            &[entity("demoapp")],
            &[edge("demoapp", &token, "uses", &note)],
        )
        .unwrap_err(),
        s.save_graph(&token, &[entity("demoapp")], &[]).unwrap_err(),
    ] {
        refused(error, &token);
    }
    for table in ["entities", "edges", "memory_entities"] {
        assert!(count(&s, table) == 0, "{table}");
    }
}

/// A refused replacement leaves the memory's graph as it was: the old
/// footprint is dropped only for a graph that is then written.
#[test]
fn redaction_state_graph_replace_keeps_the_old_graph_when_refused() {
    let token = token();
    let s = open();
    let note = memory(&s, "a plain note");
    s.replace_graph(
        &note,
        &[entity("demoapp"), entity("sqlite")],
        &[edge("demoapp", "sqlite", "uses", &note)],
    )
    .unwrap();
    let before = dump(&s, &["entities", "edges", "memory_entities"]);
    for error in [
        s.replace_graph(&note, &[entity("demoapp"), entity(&token)], &[])
            .unwrap_err(),
        s.replace_graph(&note, &[], &[edge(&token, "sqlite", "uses", &note)])
            .unwrap_err(),
        s.replace_graph(&token, &[entity("demoapp")], &[])
            .unwrap_err(),
        s.strip_memory_project_associations(&token).unwrap_err(),
        s.set_memory_single_project(&token, "demoapp").unwrap_err(),
        s.backlink_memory_projects(&token, "demoapp work", "note", "")
            .unwrap_err(),
    ] {
        refused(error, &token);
    }
    assert!(dump(&s, &["entities", "edges", "memory_entities"]) == before);
}

/// The scope of a commit title is only looked up: one the policy refuses
/// is no known project, the memory's graph is replaced and its links are
/// reconciled as for any other unknown scope, and nothing fails halfway.
#[test]
fn redaction_state_graph_reconcile_takes_a_refused_scope_for_an_unknown_one() {
    let value = body(24);
    let s = open();
    for name in ["demoapp", "otherapp"] {
        s.upsert_entity(&entity(name)).unwrap();
    }
    let scope = format!("password={value}");
    // Even when an entity stored before the policy carries that name.
    s.conn
        .lock()
        .unwrap()
        .execute(
            "INSERT INTO entities (id, name, entity_type, mention_count, first_seen, last_seen)
             VALUES ('legacy', ?1, 'project', 1, datetime('now'), datetime('now'))",
            [&scope],
        )
        .unwrap();
    assert!(!s.set_memory_single_project("m1", &scope).unwrap());
    // A memory stored before the policy, with such a title.
    let mut legacy = MemoryEntry::new("plain", "body", MemoryType::Note, EventSource::Manual);
    s.save(&legacy).unwrap();
    legacy.title = format!("fix({scope}): it");
    s.conn
        .lock()
        .unwrap()
        .execute(
            "UPDATE memories SET title = ?1 WHERE id = ?2",
            params![legacy.title, legacy.id],
        )
        .unwrap();
    s.replace_graph(&legacy.id, &[entity("demoapp")], &[])
        .unwrap();
    s.replace_graph_and_reconcile_projects(&legacy, &[entity("otherapp")], &[])
        .unwrap();
    let linked: Vec<String> = {
        let conn = s.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT e.name FROM memory_entities me JOIN entities e ON e.id = me.entity_id
                  WHERE me.memory_id = ?1 ORDER BY e.name",
            )
            .unwrap();
        stmt.query_map([&legacy.id], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    };
    assert!(linked == ["otherapp"]);
}

/// The repair of every memory's project links meets ids stored before the
/// policy: a refused one is left as it is, and the memories after it are
/// repaired all the same.
#[test]
fn redaction_state_project_repair_skips_a_refused_memory_and_goes_on() {
    // First by insertion and first by id, whichever way the scan reads.
    let token = format!("--token={}", body(24));
    assert!(crate::redaction::check_identity(&token).is_err());
    let s = open();
    s.upsert_entity(&entity("demoapp")).unwrap();
    // Scanned first: what comes after it must still be reached.
    s.conn
        .lock()
        .unwrap()
        .execute(
            "INSERT INTO memories (id, timestamp, title, content, memory_type, tags, source,
                 importance, metadata)
             VALUES (?1, ?2, 'fix(demoapp): older', 'body', 'note', '[]', '\"Manual\"', 0.5,
                     'null')",
            params![token, chrono::Utc::now().to_rfc3339()],
        )
        .unwrap();
    let after = [
        memory(&s, "fix(demoapp): wire the importer"),
        memory(&s, "fix(demoapp): page the listing"),
    ];
    let scanned: Vec<String> = {
        let conn = s.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT id FROM memories").unwrap();
        stmt.query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    };
    assert!(
        scanned.first() == Some(&token),
        "the refused memory comes first"
    );
    assert!(s.reconcile_all_projects().unwrap() == 3);
    for id in &after {
        let links: i64 = s
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT count(*) FROM memory_entities WHERE memory_id = ?1",
                [id],
                |r| r.get(0),
            )
            .unwrap();
        assert!(links == 1);
    }
}

/// A name is stored lowercased or as a key made from it, and either can
/// have a shape the name as written has not: each writer judges the form
/// it stores.
#[test]
fn redaction_state_names_are_judged_in_the_form_they_are_stored_in() {
    let upper = token().to_uppercase();
    let spaced = token().replace('-', " ");
    for name in [&upper, &spaced] {
        assert!(crate::redaction::is_clean(name), "clean as it is written");
    }
    let s = open();
    s.upsert_peer("claude", None, "agent").unwrap();
    s.upsert_entity(&entity("oldapp")).unwrap();
    let tables = ["peers", "conclusions", "entities", "entity_aliases"];
    let before = dump(&s, &tables);
    for error in [
        s.upsert_peer(&upper, None, "agent").unwrap_err(),
        s.merge_peers(&upper, "claude").unwrap_err(),
        s.merge_peers("claude", &upper).unwrap_err(),
        s.add_conclusion(&upper, "pattern", "plain", 0.5, &[])
            .unwrap_err(),
        s.rename_entity("oldapp", &upper).unwrap_err(),
        // Clean in either case, a token as the key made from it.
        s.rename_entity("oldapp", &spaced).unwrap_err(),
        s.merge_entities(&spaced, "oldapp").unwrap_err(),
    ] {
        refused(error, &upper);
    }
    {
        let conn = s.conn.lock().unwrap();
        for error in [
            crate::updates::store::rekey_project(&conn, "oldapp", &spaced).unwrap_err(),
            crate::facts::rekey::rekey_in_tx(&conn, "oldapp", &spaced).unwrap_err(),
        ] {
            refused(error, &spaced);
        }
    }
    assert!(dump(&s, &tables) == before);
}

/// Values that move into a slot that exists already are shown under the
/// spellings that slot keeps: they are judged under those too.
#[test]
fn redaction_state_merge_judges_values_under_the_slot_they_join() {
    let value = body(24);
    let s = open();
    for name in ["srcapp", "dstapp"] {
        s.upsert_entity(&entity(name)).unwrap();
    }
    let fact = |project: &str, predicate: &str, value: &str| {
        crate::facts::store::apply(
            &s,
            &crate::facts::store::FactWrite {
                project: Some(project),
                subject: "svc",
                predicate,
                value: Some(value),
                actor: "test",
                ..Default::default()
            },
        )
        .unwrap();
    };
    // The same slot key under both projects; only one spells it as a
    // credential's name, with a value that is none.
    fact("dstapp", "DEPLOY_TOKEN", "kept in the vault");
    fact("srcapp", "deploy-token", &value);
    let tables = ["entities", "entity_aliases", "fact_slots", "fact_values"];
    let before = dump(&s, &tables);
    refused(s.merge_entities("dstapp", "srcapp").unwrap_err(), &value);
    assert!(dump(&s, &tables) == before);
    // The other way round the values join a slot that spells it plainly.
    let other = open();
    for name in ["srcapp", "dstapp"] {
        other.upsert_entity(&entity(name)).unwrap();
    }
    crate::facts::store::apply(
        &other,
        &crate::facts::store::FactWrite {
            project: Some("srcapp"),
            subject: "svc",
            predicate: "deploy-token",
            value: Some(&value),
            actor: "test",
            ..Default::default()
        },
    )
    .unwrap();
    assert!(
        other
            .merge_entities("dstapp", "srcapp")
            .unwrap()
            .alias_dropped
    );
}

/// What a move makes of a fact must be something that could have been
/// stated: the statement is judged whole, as it will read, and by its
/// names alone where the slot holds no value.
#[test]
fn redaction_state_rename_judges_the_statement_a_fact_becomes() {
    let name = body(24);
    let token = token();
    assert!(crate::redaction::is_clean(&name) && crate::redaction::is_clean("Bearer"));
    let s = open();
    for entity_name in ["carrier", "courier", "legacy", "older", "oldest"] {
        s.upsert_entity(&entity(entity_name)).unwrap();
    }
    let stated = |subject: &str, predicate: &str, qualifier: Option<&str>, value: Option<&str>| {
        crate::facts::store::apply(
            &s,
            &crate::facts::store::FactWrite {
                subject,
                predicate,
                qualifier,
                value,
                actor: "test",
                ..Default::default()
            },
        )
        .unwrap();
    };
    let fact = |subject: &str, predicate: &str, value: Option<&str>| {
        stated(subject, predicate, None, value)
    };
    // Stored before the policy: a value under a name that is a
    // credential's only as the key it is filed by.
    stated("older", "api key", None, Some("eu"));
    stated("oldest", "host", Some("api key"), Some("eu"));
    s.conn
        .lock()
        .unwrap()
        .execute(
            "UPDATE fact_values SET value = ?1 WHERE slot_id IN
                 (SELECT id FROM fact_slots WHERE subject_key IN ('older', 'oldest'))",
            [&name],
        )
        .unwrap();
    fact("carrier", &name, Some("eu"));
    // A slot that holds no value any more: a statement, then its value
    // erased as `fact forget` would.
    fact("courier", &name, Some("eu"));
    fact("courier", &name, None);
    s.conn
        .lock()
        .unwrap()
        .execute(
            "DELETE FROM fact_values WHERE kind = 'value' AND slot_id IN
                 (SELECT id FROM fact_slots WHERE subject_key = 'courier')",
            [],
        )
        .unwrap();
    // A value stored before the policy that the policy refuses.
    fact("legacy", "host", Some("db.internal"));
    s.conn
        .lock()
        .unwrap()
        .execute(
            "UPDATE fact_values SET value = ?1 WHERE slot_id IN
                 (SELECT id FROM fact_slots WHERE subject_key = 'legacy')",
            [&token],
        )
        .unwrap();
    let tables = ["entities", "entity_aliases", "fact_slots", "fact_values"];
    let before = dump(&s, &tables);
    for error in [
        s.rename_entity("carrier", "Bearer").unwrap_err(),
        s.rename_entity("courier", "Bearer").unwrap_err(),
        s.rename_entity("legacy", "newer").unwrap_err(),
        s.rename_entity("older", "newer").unwrap_err(),
        s.rename_entity("oldest", "newer").unwrap_err(),
    ] {
        assert!(is_refused(&error));
        let text = format!("{error:#}");
        assert!(text.contains("SENSITIVE_CONTENT"));
        assert!(!text.contains(&name) && !text.contains(&token));
    }
    assert!(dump(&s, &tables) == before);
    // A name that makes nothing of the predicate takes the fact along.
    assert!(s.rename_entity("carrier", "hauler").unwrap());
}

/// A name that is both a subject and a project moves a slot twice: only
/// the second move shows the slot its values land in. Every move is
/// judged where it happens, and a refusal undoes the moves before it.
#[test]
fn redaction_state_merge_judges_the_slot_a_value_lands_in() {
    let value = body(24);
    let s = open();
    for name in ["srcapp", "dstapp"] {
        s.upsert_entity(&entity(name)).unwrap();
    }
    let fact = |name: &str, predicate: &str, value: &str| {
        crate::facts::store::apply(
            &s,
            &crate::facts::store::FactWrite {
                project: Some(name),
                subject: name,
                predicate,
                value: Some(value),
                actor: "test",
                ..Default::default()
            },
        )
        .unwrap();
    };
    fact("dstapp", "DEPLOY_TOKEN", "kept in the vault");
    fact("srcapp", "deploy-token", &value);
    let tables = [
        "entities",
        "entity_aliases",
        "fact_slots",
        "fact_values",
        "fact_events",
    ];
    let before = dump(&s, &tables);
    refused(s.merge_entities("dstapp", "srcapp").unwrap_err(), &value);
    assert!(dump(&s, &tables) == before);
    // On a connection of its own, with no transaction around it: the first
    // move is undone with the refused second one.
    {
        let conn = s.conn.lock().unwrap();
        let error = crate::facts::rekey::rekey_in_tx(&conn, "srcapp", "dstapp").unwrap_err();
        refused(error, &value);
    }
    assert!(dump(&s, &tables) == before);
}

/// A project or a subject renamed to a credential's name would show every
/// value of its facts as a credential: the move is refused before any row
/// moves. A time, and a value that reads as none under the new name, move.
#[test]
fn redaction_state_rename_judges_the_values_it_would_refile() {
    let value = body(24);
    let s = open();
    for name in ["demoapp", "secret", "svc", "relapp"] {
        s.upsert_entity(&entity(name)).unwrap();
    }
    let fact = |project: Option<&str>, subject: &str, predicate: &str, value: &str| {
        crate::facts::store::apply(
            &s,
            &crate::facts::store::FactWrite {
                project,
                subject,
                predicate,
                value: Some(value),
                actor: "test",
                ..Default::default()
            },
        )
        .unwrap();
    };
    fact(Some("demoapp"), "svc", "build id", &value);
    fact(Some("demoapp"), "svc", "released", "2026-12-01T00:00:00Z");
    // A project whose only fact states a time.
    fact(Some("relapp"), "site", "released", "2026-12-01T00:00:00Z");
    // A value that follows a name directly only as it is compared.
    s.upsert_entity(&entity("padapp")).unwrap();
    fact(
        Some("padapp"),
        "site",
        "build id",
        &format!("\u{a0}{value}"),
    );
    let tables = ["entities", "entity_aliases", "fact_slots", "fact_values"];
    let before = dump(&s, &tables);
    for error in [
        // As a project, as a subject, and into an entity that exists.
        s.rename_entity("demoapp", "token").unwrap_err(),
        s.rename_entity("svc", "api_key").unwrap_err(),
        s.merge_entities("secret", "demoapp").unwrap_err(),
        // A credential's name as it is written, and one only as a key.
        s.rename_entity("demoapp", "DEPLOY_TOKEN").unwrap_err(),
        s.rename_entity("demoapp", "api key").unwrap_err(),
        s.rename_entity("svc", "api key").unwrap_err(),
        s.rename_entity("padapp", "token").unwrap_err(),
    ] {
        refused(error, &value);
    }
    assert!(dump(&s, &tables) == before);
    {
        let conn = s.conn.lock().unwrap();
        let error = crate::facts::rekey::rekey_in_tx(&conn, "demoapp", "token").unwrap_err();
        refused(error, &value);
    }
    assert!(dump(&s, &tables) == before);
    // A name that makes nothing of the value takes the facts along, and
    // a time moves under any name.
    assert!(s.rename_entity("demoapp", "newapp").unwrap());
    assert!(s.rename_entity("relapp", "token").unwrap());
    let after = dump(&s, &["fact_slots"]);
    assert!(after.contains("newapp") && !after.contains("demoapp"));
    assert!(after.contains("token") && !after.contains("relapp"));
}

/// A rename or a merge moves follow-ups, memories and facts with the
/// entity: a refused name moves none of them.
#[test]
fn redaction_state_rename_and_merge_refuse_before_any_related_row_moves() {
    let token = token();
    let s = open();
    s.upsert_entity(&entity("oldapp")).unwrap();
    s.upsert_entity(&entity("otherapp")).unwrap();
    crate::followups::create(
        &s,
        crate::followups::NewFollowup {
            project: "oldapp",
            title: "Ship the importer",
            source_memory_id: None,
            authoritative: true,
            actor: "test",
            request_id: "r1",
        },
        chrono::Utc::now(),
    )
    .unwrap();
    let mut saved = MemoryEntry::new("note", "body", MemoryType::Note, EventSource::Manual);
    crate::updates::plan::set_project(&s, &mut saved, "oldapp").unwrap();
    s.save(&saved).unwrap();
    crate::facts::store::apply(
        &s,
        &crate::facts::store::FactWrite {
            project: Some("oldapp"),
            subject: "api",
            predicate: "host",
            value: Some("db.internal"),
            actor: "test",
            ..Default::default()
        },
    )
    .unwrap();
    let tables = [
        "entities",
        "entity_aliases",
        "edges",
        "followups",
        "memories",
        "fact_slots",
        "fact_events",
    ];
    let before = dump(&s, &tables);
    for error in [
        s.rename_entity("oldapp", &token).unwrap_err(),
        s.rename_entity(&token, "newapp").unwrap_err(),
        s.merge_entities(&token, "oldapp").unwrap_err(),
        s.merge_entities("otherapp", &token).unwrap_err(),
    ] {
        refused(error, &token);
    }
    {
        let conn = s.conn.lock().unwrap();
        for error in [
            crate::updates::store::rekey_project(&conn, "oldapp", &token).unwrap_err(),
            crate::updates::store::rekey_project(&conn, &token, "newapp").unwrap_err(),
            crate::facts::rekey::rekey_in_tx(&conn, "oldapp", &token).unwrap_err(),
            crate::facts::rekey::rekey_in_tx(&conn, &token, "newapp").unwrap_err(),
        ] {
            refused(error, &token);
        }
    }
    assert!(dump(&s, &tables) == before);
    // A clean rename still takes everything along.
    assert!(s.rename_entity("oldapp", "newapp").unwrap());
    let after = dump(&s, &tables);
    assert!(after.contains("newapp") && !after.contains("oldapp"));
}
