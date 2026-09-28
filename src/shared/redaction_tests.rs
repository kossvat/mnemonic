//! The shared store under the redaction policy: a sensitive request is
//! refused whole, before a retry is answered and before any row is
//! written; a promotion judges the stored observation again; safe requests
//! keep their retry and revision semantics. Credential fixtures are
//! assembled at run time; assertions never print them.
use std::path::PathBuf;

use rusqlite::params;

use super::*;
use crate::shared::curation::{NewRecord, write_record};
use crate::shared::store::SharedStore;
use crate::shared::types::Expected;

fn body(n: usize) -> String {
    "a1B2c3D4e5F6".chars().cycle().take(n).collect()
}

/// A provider token that is also a valid slug and identifier.
fn token() -> String {
    ["sk-", "proj-", &body(40).to_lowercase()].concat()
}

struct Fixture {
    dir: PathBuf,
    store: SharedStore,
}

impl Fixture {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "mnemonic-shared-redaction-{}",
            uuid::Uuid::new_v4()
        ));
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&dir).unwrap();
        let store = SharedStore::open(&dir.join("shared.db")).unwrap();
        Self { dir, store }
    }

    /// Every text and number the shared tables hold.
    fn dump(&self) -> String {
        let conn = self.store.lock().unwrap();
        let mut out = String::new();
        for table in [
            "shared_projects",
            "shared_records",
            "shared_observations",
            "shared_revocations",
            "shared_meta",
        ] {
            let mut stmt = conn.prepare(&format!("SELECT * FROM {table}")).unwrap();
            let columns = stmt.column_count();
            let mut rows = stmt.query([]).unwrap();
            while let Some(row) = rows.next().unwrap() {
                out.push_str(table);
                for i in 0..columns {
                    let value: rusqlite::types::Value = row.get(i).unwrap();
                    out.push_str(&format!("|{value:?}"));
                }
                out.push('\n');
            }
        }
        out
    }

    /// An observation as a binary before the policy stored it.
    fn legacy_observation(&self, id: &str, writer: &str, content: &str, promoted: Option<&str>) {
        let conn = self.store.lock().unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO shared_projects(project_id) VALUES ('alpha')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO shared_observations
                (id, project_id, writer_id, request_id, title, content, source, status,
                 created_at, promoted_key)
             VALUES (?1, 'alpha', ?2, ?1, 'Unverified input', ?3, 'chat:1', 'pending',
                     '2026-01-02T03:04:05Z', ?4)",
            params![id, writer, content, promoted],
        )
        .unwrap();
    }

    /// A record as a binary before the policy published it.
    fn legacy_record(&self, key: &str, content: &str, origin: Option<&str>) {
        self.legacy_record_by(key, content, origin, None);
    }

    fn legacy_record_by(&self, key: &str, content: &str, origin: Option<&str>, by: Option<&str>) {
        let conn = self.store.lock().unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO shared_projects(project_id, revision) VALUES ('alpha', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO shared_records
                (project_id, key, title, content, source, revision, updated_at,
                 origin_observation_id, published_by)
             VALUES ('alpha', ?1, 'Brief', ?2, 'owner:fixture', 1, '2026-01-02T03:04:05Z', ?3,
                     ?4)",
            params![key, content, origin, by],
        )
        .unwrap();
    }

    /// One stored field, as a binary before the policy left it.
    fn set(&self, table: &str, column: &str, (by, id): (&str, &str), value: &str) {
        let conn = self.store.lock().unwrap();
        let changed = conn
            .execute(
                &format!("UPDATE {table} SET {column}=?1 WHERE {by}=?2"),
                params![value, id],
            )
            .unwrap();
        assert!(changed == 1);
    }

    fn count(&self, table: &str) -> i64 {
        let conn = self.store.lock().unwrap();
        conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn refused(error: anyhow::Error, secret: &str) {
    assert!(is_refusal(&error), "not a refusal");
    let text = format!("{error:#} {error:?}");
    assert!(text.contains("shared write refused (SENSITIVE_CONTENT)"));
    assert!(!text.contains(secret), "the error echoes its input");
}

/// Texts the policy refuses, each holding `secret`.
fn sensitive_texts(secret: &str) -> [String; 3] {
    [
        format!("deploy with {secret} on friday"),
        format!("the password={} is in the vault", body(24)),
        format!("public <private>{secret}</private> part"),
    ]
}

#[test]
fn redaction_shared_fixtures_are_what_the_policy_refuses() {
    let token = token();
    assert!(!crate::redaction::is_clean(&token));
    crate::shared::types::validate_slug(&token, "slug").unwrap();
    crate::shared::types::validate_identifier(&token, "identifier").unwrap();
    for text in sensitive_texts(&token) {
        assert!(!crate::redaction::is_clean(&text));
        crate::shared::types::validate_payload(&text, &text, &text).unwrap();
    }
    assert!(
        text(
            Some("brief"),
            "Brief",
            "Reviewed project brief",
            "owner:fixture"
        )
        .is_ok()
    );
    assert!(text(None, "Note", "The deploy window moved", "chat:1").is_ok());
    assert!(identities(["alpha", "brief", "req-1", "ann-claude"]).is_ok());
}

#[test]
fn redaction_shared_publish_refuses_a_sensitive_request_before_any_row() {
    let token = token();
    let f = Fixture::new();
    let publish = |project: &str, key: &str, text: [&str; 3], actor: Option<&str>| {
        f.store.publish_checked(
            project,
            key,
            text[0],
            text[1],
            text[2],
            Expected::Any,
            actor,
        )
    };
    let plain = ["Brief", "Reviewed project brief", "owner:fixture"];
    let mut errors = vec![
        publish(&token, "brief", plain, None).unwrap_err(),
        publish("alpha", &token, plain, None).unwrap_err(),
        publish("alpha", "brief", plain, Some(&token)).unwrap_err(),
    ];
    for text in sensitive_texts(&token) {
        for field in 0..3 {
            let mut fields = plain;
            fields[field] = &text;
            // A title is one line: the private block fits on one too.
            errors.push(publish("alpha", "brief", fields, Some("ann")).unwrap_err());
        }
    }
    // Refused before the store is asked: a stale expectation is no
    // conflict to resolve for a request that is refused anyway.
    let word = body(24);
    let text = format!("deploy with {token} on friday");
    let stale = |project: &str, key: &str, content: &str, actor: Option<&str>| {
        f.store.publish_checked(
            project,
            key,
            "Brief",
            content,
            "owner:fixture",
            Expected::Revision(7),
            actor,
        )
    };
    errors.extend([
        stale(&token, "brief", plain[1], None).unwrap_err(),
        stale("alpha", &token, plain[1], None).unwrap_err(),
        stale("alpha", "brief", plain[1], Some(&token)).unwrap_err(),
        stale("alpha", "brief", &text, None).unwrap_err(),
        stale("alpha", "api_key", &word, None).unwrap_err(),
    ]);
    for error in errors {
        refused(error, &token);
    }
    assert!(f.dump().is_empty(), "a refused publish left a row");
}

/// A row reads as a whole: its title over its content, and its text
/// under the key and the title that name it. Parts that are clean apart
/// can be a credential together.
#[test]
fn redaction_shared_rows_are_judged_as_they_read() {
    let word = body(24);
    let token = token();
    for part in [
        word.as_str(),
        "api_key",
        "password",
        "the password:",
        "token",
    ] {
        assert!(crate::redaction::is_clean(part));
    }
    let f = Fixture::new();
    let after = format!("{word} is the current one");
    let rows: [(&str, &str, &str); 4] = [
        // The content under its key, and under its title.
        ("api_key", "Key", &word),
        ("brief", "password", &word),
        // The title under its key.
        ("token", &word, "Reviewed project brief"),
        // The title over the content.
        ("brief", "the password:", &after),
    ];
    for (i, (key, title, content)) in rows.into_iter().enumerate() {
        let error = f
            .store
            .publish("alpha", key, title, content, "owner:fixture")
            .err()
            .unwrap_or_else(|| panic!("row {i} was admitted"));
        refused(error, &word);
        if i != 0 && i != 2 {
            // Without a key an observation reads the same.
            let error = f
                .store
                .observe("alpha", "researcher", "r1", title, content, "chat:1")
                .unwrap_err();
            refused(error, &word);
        }
    }
    // A source is what a text is attributed to: judged as a name, in
    // lower case too.
    let upper = token.to_uppercase();
    assert!(crate::redaction::is_clean(&upper));
    let error = f
        .store
        .publish("alpha", "brief", "Brief", "Reviewed project brief", &upper)
        .unwrap_err();
    refused(error, &upper);
    assert!(f.dump().is_empty());
    // A time under a credential's name is knowledge, not a credential, and
    // the same word under another name is a text like any other.
    f.store
        .publish(
            "alpha",
            "token",
            "Expires",
            "2026-12-01T00:00:00Z",
            "owner:fixture",
        )
        .unwrap();
    f.store
        .publish("alpha", "build", "Build id", &word, "owner:fixture")
        .unwrap();
    // A title that ends in a separator reads the same over a time, and
    // beside a time the title is judged as ever.
    let time = "2026-12-01T00:00:00Z";
    f.store
        .publish("alpha", "expiry", "token:", time, "owner:fixture")
        .unwrap();
    f.store
        .observe(
            "alpha",
            "researcher",
            "r2",
            "password:",
            "2026-12-01",
            "chat:1",
        )
        .unwrap();
    let title = format!("deploy with {token}");
    let before = f.dump();
    for error in [
        f.store
            .publish("alpha", "window", &title, time, "owner:fixture")
            .unwrap_err(),
        f.store
            .observe("alpha", "researcher", "r3", &title, time, "chat:1")
            .unwrap_err(),
    ] {
        refused(error, &token);
    }
    assert!(f.dump() == before);
}

/// An identical retry is answered with the record that is there, and a
/// request the policy refuses is not: what a binary before the policy
/// published is no success to return.
#[test]
fn redaction_shared_publish_answers_a_safe_retry_and_not_a_refused_one() {
    let token = token();
    let f = Fixture::new();
    let first = f
        .store
        .publish(
            "alpha",
            "brief",
            "Brief",
            "Reviewed project brief",
            "owner:fixture",
        )
        .unwrap();
    let again = f
        .store
        .publish(
            "alpha",
            "brief",
            "Brief",
            "Reviewed project brief",
            "owner:fixture",
        )
        .unwrap();
    assert!(first == again && again.revision == 1);

    let content = format!("deploy with {token} on friday");
    f.legacy_record("legacy", &content, None);
    f.legacy_record(&token, "Reviewed project brief", None);
    f.legacy_record_by("signed", "Reviewed project brief", None, Some(&token));
    // Stored fields a request cannot name: the answer would show them.
    f.legacy_record("origin", "Reviewed project brief", Some(&token));
    f.legacy_record("writer", "Reviewed project brief", Some("obs-1"));
    f.set(
        "shared_records",
        "origin_writer_id",
        ("key", "writer"),
        &token,
    );
    let before = f.dump();
    let publish = |key: &str, content: &str, actor: Option<&str>| {
        f.store.publish_checked(
            "alpha",
            key,
            "Brief",
            content,
            "owner:fixture",
            Expected::Any,
            actor,
        )
    };
    for error in [
        publish("legacy", &content, None).unwrap_err(),
        publish(&token, "Reviewed project brief", None).unwrap_err(),
        publish("signed", "Reviewed project brief", Some(&token)).unwrap_err(),
        publish("origin", "Reviewed project brief", None).unwrap_err(),
        publish("writer", "Reviewed project brief", None).unwrap_err(),
    ] {
        refused(error, &token);
    }
    assert!(f.dump() == before);
    // A stale expectation is still a conflict, not a refusal.
    let conflict = f
        .store
        .publish_checked(
            "alpha",
            "brief",
            "Brief",
            "Another text",
            "owner:fixture",
            Expected::Absent,
            None,
        )
        .unwrap_err();
    assert!(!is_refusal(&conflict));
    assert!(f.dump() == before);
}

/// Taking something out of the store is never refused: what a binary
/// before the policy filed under a name the policy refuses stays
/// removable, and no message names it.
#[test]
fn redaction_shared_removal_stays_open_and_names_nothing_refused() {
    let token = token();
    let f = Fixture::new();
    f.legacy_record(&token, "Reviewed project brief", None);
    f.legacy_observation("obs-1", "researcher", &format!("the key is {token}"), None);
    f.legacy_observation(&token, "researcher", "The window moved", None);
    {
        let conn = f.store.lock().unwrap();
        conn.execute(
            "INSERT INTO shared_revocations (project_id, principal_id, revoked_at)
             VALUES ('alpha', ?1, '2026-01-02T03:04:05Z')",
            [&token],
        )
        .unwrap();
    }
    // A stale expectation says so without the key.
    let conflict = f
        .store
        .revoke_checked("alpha", &token, Expected::Absent)
        .unwrap_err();
    let said = format!("{conflict:#}");
    assert!(said.contains("revision conflict") && !said.contains(&token));
    assert!(f.count("shared_records") == 1);

    assert!(
        f.store
            .revoke_checked("alpha", &token, Expected::Any)
            .unwrap()
    );
    assert!(f.store.review("alpha", "obs-1", "rejected").unwrap());
    assert!(f.store.review("alpha", &token, "rejected").unwrap());
    assert!(f.store.restore_principal("alpha", &token).unwrap());
    assert!(f.count("shared_records") == 0 && f.count("shared_revocations") == 0);
    assert!(f.store.inbox("alpha", 10).unwrap().is_empty());
    // A conflict on a key the policy admits still names it.
    f.store
        .publish(
            "alpha",
            "brief",
            "Brief",
            "Reviewed project brief",
            "owner:fixture",
        )
        .unwrap();
    let conflict = f
        .store
        .revoke_checked("alpha", "brief", Expected::Absent)
        .unwrap_err();
    assert!(format!("{conflict:#}").contains("key brief"));
}

#[test]
fn redaction_shared_observe_refuses_a_sensitive_request_before_any_row() {
    let token = token();
    let f = Fixture::new();
    let observe =
        |project: &str, principal: Option<&str>, writer: &str, request: &str, text: [&str; 3]| {
            f.store.observe_as(
                project, principal, writer, request, text[0], text[1], text[2],
            )
        };
    let plain = ["Unverified input", "The deploy window moved", "chat:1"];
    let mut errors = vec![
        observe(&token, None, "researcher", "r1", plain).unwrap_err(),
        observe("alpha", Some(&token), "researcher", "r1", plain).unwrap_err(),
        observe("alpha", None, &token, "r1", plain).unwrap_err(),
        observe("alpha", None, "researcher", &token, plain).unwrap_err(),
    ];
    for text in sensitive_texts(&token) {
        for field in 0..3 {
            let mut fields = plain;
            fields[field] = &text;
            errors.push(observe("alpha", Some("ann"), "ann-claude", "r1", fields).unwrap_err());
        }
    }
    for error in errors {
        refused(error, &token);
    }
    // Neither a pending observation nor the project, and the writer is not
    // bound to the principal of a refused request.
    assert!(f.dump().is_empty(), "a refused observation left a row");
    observe("alpha", Some("bob"), "ann-claude", "r1", plain).unwrap();
}

#[test]
fn redaction_shared_observe_answers_a_safe_retry_and_not_a_refused_one() {
    let token = token();
    let f = Fixture::new();
    let observe = |request: &str, content: &str| {
        f.store.observe(
            "alpha",
            "researcher",
            request,
            "Unverified input",
            content,
            "chat:1",
        )
    };
    let first = observe("r1", "The deploy window moved").unwrap();
    assert!(observe("r1", "The deploy window moved").unwrap() == first);
    // The same request id with another text is still the usual rejection.
    assert!(!is_refusal(&observe("r1", "Another text").unwrap_err()));

    let content = format!("deploy with {token} on friday");
    f.legacy_observation("legacy-1", "researcher", &content, None);
    // A stored field a request cannot name: the answer would show it.
    f.legacy_observation("legacy-2", "researcher", "The window moved", Some(&token));
    // Refused as a name and as nothing else: in lower case only.
    let upper = token.to_uppercase();
    f.legacy_observation(
        "legacy-3",
        "researcher",
        "The window moved on",
        Some(&upper),
    );
    f.legacy_observation("legacy-4", "researcher", "The window moved again", None);
    f.set("shared_observations", "id", ("id", "legacy-4"), &token);
    // A text that is a credential only under the key it was promoted to.
    let word = body(24);
    f.legacy_observation("legacy-5", "researcher", &word, Some("api_key"));
    let before = f.dump();
    refused(observe("legacy-5", &word).unwrap_err(), &word);
    refused(observe("legacy-1", &content).unwrap_err(), &token);
    refused(
        observe("legacy-3", "The window moved on").unwrap_err(),
        &token,
    );
    refused(
        observe("legacy-4", "The window moved again").unwrap_err(),
        &token,
    );
    // As the helper stores it: the title and the source of its fixture.
    let stored = f.store.observe(
        "alpha",
        "researcher",
        "legacy-2",
        "Unverified input",
        "The window moved",
        "chat:1",
    );
    refused(stored.unwrap_err(), &token);
    assert!(f.dump() == before);
}

/// Promotion is the one path from an observation to shared truth: the
/// observation it selects is judged as it is stored, whenever it was.
#[test]
fn redaction_shared_promote_judges_the_stored_observation_again() {
    let token = token();
    let f = Fixture::new();
    f.legacy_observation(
        "obs-text",
        "researcher",
        &format!("the key is {token}"),
        None,
    );
    f.legacy_observation("obs-writer", &token, "The deploy window moved", None);
    // One that a binary before the policy promoted already.
    f.legacy_observation(
        "obs-done",
        "researcher",
        &format!("rotate {token} soon"),
        Some("done"),
    );
    f.legacy_record("done", &format!("rotate {token} soon"), Some("obs-done"));
    // Promoted already, with nothing to refuse in its text: by a writer,
    // and by a curator, that the policy refuses.
    f.legacy_observation("obs-by", &token, "The window moved", Some("by"));
    f.legacy_record("by", "The window moved", Some("obs-by"));
    f.legacy_observation(
        "obs-signed",
        "researcher",
        "The window moved",
        Some("signed"),
    );
    f.legacy_record_by(
        "signed",
        "The window moved",
        Some("obs-signed"),
        Some(&token),
    );
    // Promoted already, and the record was given another text since.
    f.legacy_observation(
        "obs-edited",
        "researcher",
        "The window moved",
        Some("edited"),
    );
    f.legacy_record(
        "edited",
        &format!("rotate {token} soon"),
        Some("obs-edited"),
    );
    // Promoted already, and the record has nothing of its text any more.
    f.legacy_observation(
        "obs-kept",
        "researcher",
        &format!("rotate {token} soon"),
        Some("kept"),
    );
    f.legacy_record("kept", "The window moved", Some("obs-kept"));
    // Names of an observation that a promotion does not give.
    f.legacy_observation("obs-request", "researcher", "The window moved", None);
    f.set(
        "shared_observations",
        "request_id",
        ("id", "obs-request"),
        &token,
    );
    f.legacy_observation("obs-principal", "researcher", "The window moved", None);
    f.set(
        "shared_observations",
        "principal_id",
        ("id", "obs-principal"),
        &token,
    );
    // Refused as a name and as nothing else: in lower case only.
    let upper = token.to_uppercase();
    f.legacy_observation("obs-upper", "researcher", "The window moved", Some(&upper));
    // What the key would make of a text that is clean by itself.
    let word = body(24);
    f.legacy_observation("obs-word", "researcher", &word, None);
    let before = f.dump();
    for (id, key) in [
        ("obs-edited", "edited"),
        ("obs-kept", "kept"),
        ("obs-request", "brief"),
        ("obs-principal", "brief"),
        ("obs-upper", "brief"),
        ("obs-text", "brief"),
        ("obs-writer", "brief"),
        ("obs-done", "done"),
        ("obs-by", "by"),
        ("obs-signed", "signed"),
        ("obs-word", "api_key"),
    ] {
        let error = f
            .store
            .promote("alpha", id, key, Expected::Absent, "ann")
            .unwrap_err();
        refused(error, &token);
    }
    for error in [
        f.store
            .promote("alpha", &token, "brief", Expected::Absent, "ann")
            .unwrap_err(),
        f.store
            .promote("alpha", "obs-text", &token, Expected::Absent, "ann")
            .unwrap_err(),
        f.store
            .promote("alpha", "obs-text", "brief", Expected::Absent, &token)
            .unwrap_err(),
        f.store
            .promote(&token, "obs-text", "brief", Expected::Absent, "ann")
            .unwrap_err(),
    ] {
        refused(error, &token);
    }
    assert!(f.dump() == before, "a refused promotion changed the store");

    // A safe observation is promoted as ever, and its retry answered.
    let safe = f
        .store
        .observe(
            "alpha",
            "researcher",
            "r1",
            "Note",
            "The window moved",
            "chat:1",
        )
        .unwrap();
    let record = f
        .store
        .promote("alpha", &safe.id, "window", Expected::Absent, "ann")
        .unwrap();
    let again = f
        .store
        .promote("alpha", &safe.id, "window", Expected::Absent, "ann")
        .unwrap();
    assert!(record == again && record.content == "The window moved");
}

#[test]
fn redaction_shared_curation_refuses_sensitive_names() {
    let token = token();
    let f = Fixture::new();
    f.store
        .publish(
            "alpha",
            "brief",
            "Brief",
            "Reviewed project brief",
            "owner:fixture",
        )
        .unwrap();
    let before = f.dump();
    for error in [
        f.store.pin_project(&token).unwrap_err(),
        f.store.revoke_principal(&token, "ann", None).unwrap_err(),
        f.store.revoke_principal("alpha", &token, None).unwrap_err(),
        f.store
            .revoke_principal("alpha", "ann", Some(&token))
            .unwrap_err(),
    ] {
        refused(error, &token);
    }
    assert!(f.dump() == before);
}

/// Whatever path leads to a record, the row is judged as it is about to be
/// stored, before the project or its revision is touched.
#[test]
fn redaction_shared_record_writer_refuses_every_sensitive_field() {
    let token = token();
    let text = format!("deploy with {token} on friday");
    let f = Fixture::new();
    let plain = NewRecord {
        project_id: "alpha",
        key: "brief",
        title: "Brief",
        content: "Reviewed project brief",
        source: "owner:fixture",
        published_by: Some("ann"),
        origin_observation_id: Some("obs-1"),
        origin_writer_id: Some("researcher"),
    };
    let dirty = [
        NewRecord {
            project_id: &token,
            ..plain
        },
        NewRecord {
            key: &token,
            ..plain
        },
        NewRecord {
            title: &text,
            ..plain
        },
        NewRecord {
            content: &text,
            ..plain
        },
        NewRecord {
            source: &text,
            ..plain
        },
        NewRecord {
            published_by: Some(&token),
            ..plain
        },
        NewRecord {
            origin_observation_id: Some(&token),
            ..plain
        },
        NewRecord {
            origin_writer_id: Some(&token),
            ..plain
        },
    ];
    for record in &dirty {
        let conn = f.store.lock().unwrap();
        refused(write_record(&conn, record).unwrap_err(), &token);
    }
    assert!(f.dump().is_empty());
    let conn = f.store.lock().unwrap();
    assert!(write_record(&conn, &plain).unwrap().revision == 1);
}

/// A grant renders the names it is given into a policy file and a key
/// line, and joins two of them into the writer id of every observation.
#[test]
fn redaction_shared_policy_and_grant_refuse_sensitive_names() {
    use crate::shared::mcp::Policy;
    let token = token();
    let policy = |project: &str, agent: &str| Policy {
        version: 1,
        project_id: project.into(),
        agent_id: agent.into(),
        allow_observations: true,
        max_observations_per_session: 2,
        principal_id: None,
        expires_at: None,
        max_session_secs: None,
    };
    policy("alpha", "researcher").validate().unwrap();
    for error in [
        policy(&token, "researcher").validate().unwrap_err(),
        policy("alpha", &token).validate().unwrap_err(),
    ] {
        refused(error, &token);
    }
    // Two names that are clean apart and a token joined.
    let (principal, agent) = ("sk", ["proj", &body(40).to_lowercase()].concat());
    let joined = format!("{principal}-{agent}");
    assert!(crate::redaction::is_clean(principal) && crate::redaction::is_clean(&agent));
    assert!(!crate::redaction::is_clean(&joined));
    let mut named = policy("alpha", &joined);
    named.version = 2;
    named.principal_id = Some(principal.into());
    refused(named.validate().unwrap_err(), &joined);
}

/// A policy that cannot be parsed says where, not what: the line the
/// parser stopped at may hold what the policy refuses.
#[test]
fn redaction_shared_policy_that_cannot_be_parsed_says_where_and_not_what() {
    use crate::shared::mcp::Policy;
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    let secret = token();
    let path = f.dir.join("broken.toml");
    let text = format!("version = 1\nproject_id = \"alpha\"\nagent_id = {secret}\n");
    std::fs::write(&path, text).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let error = Policy::load(&path).unwrap_err();
    let said = format!("{error:#} {error:?}");
    assert!(said.contains("(line 3)"), "the error does not say where");
    assert!(!said.contains(&secret), "the error quotes the policy");
}

/// The shared service takes one thing from outside its module, the policy,
/// and the policy opens nothing: no store, no configuration, no model.
#[test]
fn redaction_shared_service_depends_on_the_policy_only() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let imports = |dir: &str| -> Vec<String> {
        let mut found = Vec::new();
        for entry in std::fs::read_dir(root.join(dir)).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            if !name.ends_with(".rs") || name.ends_with("tests.rs") {
                continue;
            }
            let source = std::fs::read_to_string(&path).unwrap();
            for line in source.lines() {
                let mut rest = line;
                while let Some(at) = rest.find("crate::") {
                    let path: String = rest[at + "crate::".len()..]
                        .chars()
                        .take_while(|c| c.is_alphanumeric() || *c == '_')
                        .collect();
                    found.push(path);
                    rest = &rest[at + "crate::".len()..];
                }
            }
        }
        found.sort();
        found.dedup();
        found
    };
    assert!(imports("shared") == ["redaction"]);
    // Of the rest of the crate the policy knows the two record types it
    // prepares, and nothing that opens a file.
    assert!(imports("redaction") == ["event"]);
}

/// The names of a grant as the key line joins them, and a policy the lint
/// meets that the policy refuses: neither says the name.
#[test]
fn redaction_shared_grant_and_lint_judge_names_as_they_are_joined() {
    use crate::shared::access::{GrantRequest, LintRequest, grant, lint};
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    let policies = f.dir.join("policies");
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&policies)
            .unwrap();
    }
    let (db, bin) = (f.dir.join("shared.db"), f.dir.join("mnemonic"));
    // A real key in the wire format the code parses.
    let mut blob = Vec::new();
    blob.extend_from_slice(&11u32.to_be_bytes());
    blob.extend_from_slice(b"ssh-ed25519");
    blob.extend_from_slice(&32u32.to_be_bytes());
    blob.extend(1..=32u8);
    let encoded: String = {
        const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in blob.chunks(3) {
            let mut buffer = [0u8; 3];
            buffer[..chunk.len()].copy_from_slice(chunk);
            let value = u32::from_be_bytes([0, buffer[0], buffer[1], buffer[2]]);
            for shift in [18, 12, 6, 0] {
                out.push(ALPHABET[((value >> shift) & 0x3f) as usize] as char);
            }
        }
        out
    };
    let key = format!("ssh-ed25519 {encoded} ann@laptop");
    let request = |project: &'static str, agent: &'static str| GrantRequest {
        db: &db,
        bin: &bin,
        policy_dir: &policies,
        project,
        principal: "ann",
        agent,
        write: true,
        expires_at: None,
        max_session_secs: None,
        public_key: &key,
    };
    let plain = grant(&request("alpha", "claude")).unwrap();
    // Clean apart, a credential as `mnemonic:<project>:<agent id>`.
    let agent: &'static str = Box::leak(body(24).to_lowercase().into_boxed_str());
    assert!(crate::redaction::is_clean(&format!("ann-{agent}")));
    grant(&request("alpha", agent)).unwrap();
    refused(grant(&request("token", agent)).unwrap_err(), agent);

    // A policy file the policy refuses, named after what it holds.
    let secret = token();
    let refused_path = policies.join(format!("{secret}.toml"));
    let toml = plain
        .policy_toml
        .replace(
            "agent_id = \"ann-claude\"",
            &format!("agent_id = \"{secret}\""),
        )
        .replace("version = 2", "version = 1")
        .replace("principal_id = \"ann\"\n", "");
    std::fs::write(&refused_path, toml).unwrap();
    std::fs::set_permissions(&refused_path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let line = plain
        .authorized_keys_line
        .replace("ann-claude.toml", &format!("{secret}.toml"));
    let problems = lint(&LintRequest {
        db: &db,
        bin: &bin,
        policy_dir: &policies,
        authorized_keys: &line,
        owner_keys: &[],
        pinned_project: None,
    })
    .unwrap();
    let said = problems.join("\n");
    assert!(
        said.contains("SENSITIVE_CONTENT"),
        "the refusal is not reported"
    );
    assert!(!said.contains(&secret), "the lint names the refused policy");

    // The same file with nothing a policy can be read from: the problem
    // is said, the name of the file is not.
    std::fs::write(&refused_path, "version = \n").unwrap();
    let problems = lint(&LintRequest {
        db: &db,
        bin: &bin,
        policy_dir: &policies,
        authorized_keys: &line,
        owner_keys: &[],
        pinned_project: None,
    })
    .unwrap();
    let said = problems.join("\n");
    assert!(said.contains("not valid TOML"), "the problem is not said");
    assert!(!said.contains(&secret), "the lint names the file");
}
