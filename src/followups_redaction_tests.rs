//! Follow-ups under the redaction policy. Credential fixtures are
//! assembled at run time; assertions never print them.
use super::*;
use crate::event::{EventSource, MemoryEntry, MemoryType};
use crate::redaction::state::is_refused;

fn body(n: usize) -> String {
    "a1B2c3D4e5F6".chars().cycle().take(n).collect()
}

fn token() -> String {
    ["sk-", "proj-", &body(40)].concat()
}

fn store() -> crate::test_support::InTempDir<Storage> {
    crate::test_support::InTempDir::new("mn-fu-redaction-", |dir| {
        Storage::open(&dir.join("memory.db")).unwrap()
    })
}

fn new<'a>(project: &'a str, title: &'a str, request_id: &'a str) -> NewFollowup<'a> {
    NewFollowup {
        project,
        title,
        source_memory_id: None,
        authoritative: true,
        actor: "test",
        request_id,
    }
}

fn counts(storage: &Storage) -> (i64, i64) {
    let conn = storage.conn.lock().unwrap();
    conn.query_row(
        "SELECT (SELECT count(*) FROM followups), (SELECT count(*) FROM followup_events)",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .unwrap()
}

#[test]
fn redaction_state_followup_identities_are_refused_before_any_row() {
    let token = token();
    let s = store();
    let dirty = [
        new(&token, "plain", "r1"),
        new("demoapp", "plain", &token),
        NewFollowup {
            source_memory_id: Some(&token),
            ..new("demoapp", "plain", "r2")
        },
        NewFollowup {
            actor: &token,
            ..new("demoapp", "plain", "r3")
        },
    ];
    for (i, item) in dirty.into_iter().enumerate() {
        let error = create(&s, item, Utc::now()).err().unwrap();
        assert!(is_refused(&error), "item {i}");
        let text = format!("{error:#}");
        assert!(
            text.contains("follow-up refused (SENSITIVE_CONTENT)"),
            "item {i}"
        );
        assert!(!text.contains(&token), "item {i}");
    }
    assert!(counts(&s) == (0, 0));
}

/// Resolving a project can drop the syntax that made it a credential: the
/// name is judged as it was given.
#[test]
fn redaction_state_followup_project_is_judged_before_it_is_resolved() {
    use crate::graph::{Entity, EntityType};
    let value = body(32).to_lowercase();
    let s = store();
    s.upsert_entity(&Entity {
        name: format!("password-{value}"),
        entity_type: EntityType::Project,
    })
    .unwrap();
    let raw = format!("password={value}");
    assert!(resolve_project(&s, &raw).unwrap() == format!("password-{value}"));
    let error = create(&s, new(&raw, "plain", "r1"), Utc::now())
        .err()
        .unwrap();
    assert!(is_refused(&error));
    assert!(counts(&s) == (0, 0));
}

/// An alias can lead to a project stored before the policy under a name
/// it refuses: what the name resolves to is what would be stored.
#[test]
fn redaction_state_followup_project_is_judged_as_it_resolves() {
    let secret = format!("password={}", body(24).to_lowercase());
    let s = store();
    let f = create(&s, new("demoapp", "Ship the importer", "r0"), Utc::now()).unwrap();
    {
        let conn = s.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO entity_aliases (alias, canonical) VALUES ('demo', ?1)",
            [&secret],
        )
        .unwrap();
    }
    assert!(resolve_project(&s, "demo").unwrap() == secret);
    let error = create(&s, new("demo", "plain", "r1"), Utc::now())
        .err()
        .unwrap();
    assert!(is_refused(&error));
    assert!(!format!("{error:#}").contains(&secret));
    // As a transition's project guard too, before its replay lookup.
    let error = transition(
        &s,
        Transition {
            id: &f.id,
            action: Action::Close,
            expected_revision: None,
            project: Some("demo"),
            evidence_memory_id: None,
            actor: "test",
            request_id: "r0",
        },
        Utc::now(),
    )
    .unwrap_err();
    assert!(is_refused(&error));
    assert!(!format!("{error:#}").contains(&secret));
    assert!(counts(&s) == (1, 1));
}

#[test]
fn redaction_state_followup_title_is_prepared_also_after_its_cut() {
    let token = token();
    let s = store();
    let f = create(
        &s,
        new("demoapp", &format!("rotate {token}\nsecond line"), "r1"),
        Utc::now(),
    )
    .unwrap();
    assert!(f.title == "rotate [REDACTED:credential]");
    // A title whose cut ends right after a value that read as a call.
    let value = body(24);
    let lead = "x".repeat(MAX_TITLE_CHARS - "password=".len() - value.len() - 1);
    let long = format!("{lead} password={value}(arg) and more");
    assert!(crate::redaction::is_clean(&long));
    let f = create(&s, new("demoapp", &long, "r2"), Utc::now()).unwrap();
    assert!(!f.title.contains(&value));
    assert!(crate::redaction::is_clean(&f.title));
    assert!(f.title.chars().count() <= MAX_TITLE_CHARS);
    // A clean title is stored as before.
    let f = create(
        &s,
        new("demoapp", "  Ship   the importer ", "r3"),
        Utc::now(),
    )
    .unwrap();
    assert!(f.title == "Ship the importer");
    let conn = s.conn.lock().unwrap();
    let stored: String = conn
        .query_row("SELECT group_concat(title, ' ') FROM followups", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert!(!stored.contains(&token) && !stored.contains(&value));
}

/// A request id that was applied before returns its row; one that comes
/// with a credential has no success to return.
#[test]
fn redaction_state_followup_transition_refuses_before_replay() {
    let token = token();
    let s = store();
    let f = create(&s, new("demoapp", "Ship the importer", "r1"), Utc::now()).unwrap();
    let step = |id: &str, project: Option<&str>, evidence: Option<&str>, request: &str| {
        transition(
            &s,
            Transition {
                id,
                action: Action::Close,
                expected_revision: None,
                project,
                evidence_memory_id: evidence,
                actor: "test",
                request_id: request,
            },
            Utc::now(),
        )
    };
    for error in [
        step(&token, None, None, "r1").unwrap_err(),
        step(&f.id, Some(&token), None, "r1").unwrap_err(),
        step(&f.id, None, Some(&token), "r1").unwrap_err(),
        step(&f.id, None, None, &token).unwrap_err(),
        resolve_id(&s, &token).unwrap_err(),
    ] {
        assert!(is_refused(&error));
        assert!(!format!("{error:#}").contains(&token));
    }
    assert!(counts(&s) == (1, 1));
    assert!(
        by_id(&s.conn.lock().unwrap(), &f.id)
            .unwrap()
            .unwrap()
            .status
            == Status::Open
    );
    // The safe retry is still a replay of the creation.
    assert!(step(&f.id, None, None, "r1").unwrap().status == Status::Open);
}

/// A follow-up stored before the policy under a project it refuses: the
/// message about a guard that does not match names the guard, not it.
#[test]
fn redaction_state_followup_mismatch_does_not_name_a_refused_project() {
    let secret = format!("password={}", body(24));
    let s = store();
    let now = Utc::now().to_rfc3339();
    s.conn
        .lock()
        .unwrap()
        .execute(
            "INSERT INTO followups (id, project, project_key, title, status, revision,
                 created_at, updated_at)
             VALUES ('0123456789abcdef', ?1, ?2, 'Ship it', 'open', 1, ?3, ?3)",
            params![secret, project_key(&secret), now],
        )
        .unwrap();
    let error = transition(
        &s,
        Transition {
            id: "0123456789abcdef",
            action: Action::Close,
            expected_revision: None,
            project: Some("demoapp"),
            evidence_memory_id: None,
            actor: "test",
            request_id: "r1",
        },
        Utc::now(),
    )
    .unwrap_err();
    let text = format!("{error:#}");
    assert!(text.contains("belongs to project") && text.contains("demoapp"));
    assert!(
        !text.contains(&secret),
        "the message names the stored project"
    );
}

/// A project stored before the policy can be one it refuses: that note
/// proposes nothing, every other note still does, on every pass.
#[test]
fn redaction_state_followup_sweep_skips_a_refused_project() {
    let value = body(32);
    let s = store();
    let note = |title: &str| {
        let e = MemoryEntry::new(title, "body", MemoryType::Note, EventSource::Socket);
        s.save(&e).unwrap();
        e.id
    };
    let clean = note("TODO: wire the importer retry");
    let legacy = note("TODO: move the queue worker");
    {
        let conn = s.conn.lock().unwrap();
        let now = Utc::now().to_rfc3339();
        for (id, name, memory) in [
            ("e-clean", "demoapp".to_string(), &clean),
            ("e-legacy", format!("password={value}"), &legacy),
        ] {
            conn.execute(
                "INSERT INTO entities (id, name, entity_type, mention_count, first_seen, last_seen)
                 VALUES (?1, ?2, 'project', 1, ?3, ?3)",
                params![id, name, now],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO memory_entities (memory_id, entity_id) VALUES (?1, ?2)",
                params![memory, id],
            )
            .unwrap();
        }
    }
    assert!(sweep(&s, Utc::now()).unwrap() == 1);
    assert!(sweep(&s, Utc::now()).unwrap() == 0);
    let rows = list(&s, None, true, 10).unwrap();
    assert!(rows.len() == 1);
    assert!(rows[0].source_memory_id.as_deref() == Some(clean.as_str()));
}
