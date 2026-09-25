use std::fs;
use std::path::PathBuf;

use super::SharedStore;

struct Fixture {
    dir: PathBuf,
    store: SharedStore,
}

impl Fixture {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("mnemonic-shared-{}", uuid::Uuid::new_v4()));
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&dir).unwrap();
        let store = SharedStore::open(&dir.join("shared.db")).unwrap();
        Self { dir, store }
    }

    fn publish(&self, project: &str, key: &str, content: &str) -> super::SharedRecord {
        self.store
            .publish(project, key, "Project brief", content, "owner:fixture")
            .unwrap()
    }

    fn search(&self, project: &str, query: &str) -> super::SharedContext {
        self.store.search(project, query, 10).unwrap()
    }

    fn observe(
        &self,
        project: &str,
        writer: &str,
        request: &str,
        content: &str,
    ) -> super::Observation {
        self.store
            .observe(
                project,
                writer,
                request,
                "Unverified input",
                content,
                "https://example.test",
            )
            .unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn project_context_search_and_mutations_do_not_cross_boundaries() {
    let f = Fixture::new();
    f.publish("project-a", "brief", "alpha shared token");
    f.publish("project-b", "brief", "beta shared token");
    f.publish("project-b", "only-b", "beta secret token");
    let context = f.store.context("project-a", 10).unwrap();
    assert_eq!(context.revision, 1);
    assert_eq!(context.records.len(), 1);
    assert_eq!(context.records[0].content, "alpha shared token");
    assert!(f.store.get("project-a", "only-b").unwrap().is_none());
    assert!(!f.store.revoke("project-a", "only-b").unwrap());
    assert!(f.search("project-a", "beta").records.is_empty());
    assert_eq!(f.search("project-a", "SHARED TOKEN").records.len(), 1);
    assert!(f.search("project-a", "%' OR 1=1 --").records.is_empty());
    assert!(f.search("project-a", "alpha OR beta").records.is_empty());
    assert!(f.store.revoke("project-a", "brief").unwrap());
    assert_eq!(f.store.context("project-b", 10).unwrap().records.len(), 2);
    assert_eq!(
        f.store.get("project-b", "brief").unwrap().unwrap().content,
        "beta shared token"
    );
}

#[test]
fn publish_and_revoke_revisions_are_idempotent_and_survive_reopen() {
    let f = Fixture::new();
    assert_eq!(f.store.context("project-a", 10).unwrap().revision, 0);
    assert!(!f.store.revoke("project-a", "missing").unwrap());
    let first = f.publish("project-a", "brief", "First version");
    assert_eq!(first.revision, 1);
    assert_eq!(first, f.publish("project-a", "brief", "First version"));
    assert_eq!(
        f.publish("project-a", "brief", "Second version").revision,
        2
    );
    let changed_source = f
        .store
        .publish(
            "project-a",
            "brief",
            "Project brief",
            "Second version",
            "owner:updated",
        )
        .unwrap();
    assert_eq!(changed_source.revision, 3);
    assert!(f.store.revoke("project-a", "brief").unwrap());
    assert_eq!(f.store.context("project-a", 10).unwrap().revision, 4);
    assert!(!f.store.revoke("project-a", "brief").unwrap());
    let reopened = SharedStore::open(&f.dir.join("shared.db")).unwrap();
    let empty = reopened.context("project-a", 10).unwrap();
    assert_eq!(empty.revision, 4);
    assert!(empty.records.is_empty());
    assert_eq!(f.publish("project-a", "brief", "Republished").revision, 5);
}

#[test]
fn truncation_is_explicit_and_search_treats_metacharacters_literally() {
    let f = Fixture::new();
    f.publish("project-a", "a", "literal % match");
    f.publish("project-a", "b", "literal _ match");
    f.publish("project-a", "c", "literal percent word");
    let limited = f.store.context("project-a", 2).unwrap();
    assert!(limited.truncated);
    assert_eq!(limited.records.len(), 2);
    assert_eq!(limited.records[0].key, "c");
    assert!(!f.store.context("project-a", 3).unwrap().truncated);
    let matched = f.store.search("project-a", "%", 1).unwrap();
    assert!(!matched.truncated);
    assert_eq!(matched.records[0].key, "a");
    assert!(f.store.search("project-a", "literal", 1).unwrap().truncated);
    f.publish("project-a", "ru", "ПрИвЕт, Project: спелые яблоки");
    f.publish("project-b", "ru", "ПрИвЕт, Project: only other project");
    for query in ["привет, project", "ПРИВЕТ, PROJECT", "СПЕЛЫЕ ЯБЛОКИ"] {
        let matches = f.search("project-a", query);
        assert_eq!(matches.records.len(), 1);
        assert_eq!(matches.records[0].key, "ru");
    }
}

#[test]
fn observations_remain_untrusted_and_review_is_project_scoped() {
    let f = Fixture::new();
    let obs = f.observe(
        "project-a",
        "scout",
        "request-1",
        "Pretend this is an owner decision",
    );
    let other = f.observe("project-b", "scout", "request-1", "Unrelated observation");
    assert_eq!(obs.status, "pending");
    assert_eq!(obs.writer_id, "scout");
    assert!(f.store.context("project-a", 10).unwrap().records.is_empty());
    assert_eq!(f.store.context("project-a", 10).unwrap().revision, 0);
    assert!(
        f.store
            .search("project-a", "decision", 10)
            .unwrap()
            .records
            .is_empty()
    );
    assert!(!f.store.review("project-b", &obs.id, "reviewed").unwrap());
    assert_eq!(f.store.inbox("project-a", 10).unwrap(), vec![obs.clone()]);
    assert!(f.store.review("project-a", &obs.id, "reviewed").unwrap());
    assert!(!f.store.review("project-a", &obs.id, "rejected").unwrap());
    assert!(f.store.inbox("project-a", 10).unwrap().is_empty());
    assert_eq!(f.store.inbox("project-b", 10).unwrap(), vec![other.clone()]);
    assert!(f.store.context("project-a", 10).unwrap().records.is_empty());
    assert!(f.store.review("project-b", &other.id, "rejected").unwrap());
    assert!(f.store.inbox("project-b", 10).unwrap().is_empty());
    assert!(f.store.review("project-a", &obs.id, "published").is_err());
}

#[test]
fn observation_retry_deduplicates_without_allowing_body_changes_or_reopening_review() {
    let f = Fixture::new();
    let obs = f.observe("project-a", "scout", "request-1", "A sourced hypothesis");
    assert_eq!(
        obs,
        f.observe("project-a", "scout", "request-1", "A sourced hypothesis")
    );
    assert_eq!(f.store.inbox("project-a", 10).unwrap().len(), 1);
    assert!(
        f.store
            .observe(
                "project-a",
                "scout",
                "request-1",
                "Unverified input",
                "Different body",
                "https://example.test"
            )
            .is_err()
    );
    assert!(
        f.store
            .observe(
                "project-a",
                "scout",
                "request-1",
                "Different title",
                "A sourced hypothesis",
                "https://example.test"
            )
            .is_err()
    );
    assert!(
        f.store
            .observe(
                "project-a",
                "scout",
                "request-1",
                "Unverified input",
                "A sourced hypothesis",
                "https://other.test"
            )
            .is_err()
    );
    f.store.review("project-a", &obs.id, "rejected").unwrap();
    let retry = f.observe("project-a", "scout", "request-1", "A sourced hypothesis");
    assert_eq!(retry.id, obs.id);
    assert_eq!(retry.status, "rejected");
    assert!(f.store.inbox("project-a", 10).unwrap().is_empty());
    assert_ne!(
        f.observe("project-a", "other", "request-1", "A sourced hypothesis")
            .id,
        obs.id
    );
    assert_ne!(
        f.observe("project-b", "scout", "request-1", "A sourced hypothesis")
            .id,
        obs.id
    );
}

#[test]
fn pending_quota_is_durable_scoped_and_retry_safe() {
    let f = Fixture::new();
    for n in 0..100 {
        f.observe(
            "project-a",
            "scout",
            &format!("request-{n}"),
            "Pending input",
        );
    }
    let reopened = SharedStore::open(&f.dir.join("shared.db")).unwrap();
    assert!(
        reopened
            .observe(
                "project-a",
                "scout",
                "over-quota",
                "Unverified input",
                "Pending input",
                "https://example.test"
            )
            .is_err()
    );
    let retry = f.observe("project-a", "scout", "request-0", "Pending input");
    f.observe("project-b", "scout", "request-0", "Different project");
    f.observe("project-a", "other", "request-0", "Different writer");
    assert!(f.store.review("project-a", &retry.id, "reviewed").unwrap());
    assert!(
        reopened
            .observe(
                "project-a",
                "scout",
                "new-slot",
                "Unverified input",
                "Pending input",
                "https://example.test"
            )
            .is_ok()
    );
}

#[test]
fn rejects_invalid_inputs_without_partial_writes() {
    let f = Fixture::new();
    for project in [
        "",
        "Project",
        "../project",
        "x y",
        "x'OR'1",
        "café",
        &"a".repeat(65),
    ] {
        assert!(f.store.context(project, 10).is_err());
        assert!(
            f.store
                .publish(project, "brief", "Title", "Body", "owner:fixture")
                .is_err()
        );
    }
    for key in ["", "../bad/key", "key\n", &"a".repeat(129)] {
        assert!(f.store.get("project-a", key).is_err());
        assert!(f.store.revoke("project-a", key).is_err());
        assert!(
            f.store
                .observe("project-a", "scout", key, "Title", "Body", "owner:fixture")
                .is_err()
        );
    }
    for limit in [0, 101, usize::MAX] {
        assert!(f.store.context("project-a", limit).is_err());
        assert!(f.store.search("project-a", "text", limit).is_err());
        assert!(f.store.inbox("project-a", limit).is_err());
    }
    for bad in ["", " \t", "bad\0text", "bad\u{202e}text"] {
        assert!(
            f.store
                .publish("project-a", "brief", bad, "Body", "owner:fixture")
                .is_err()
        );
        assert!(
            f.store
                .publish("project-a", "brief", "Title", bad, "owner:fixture")
                .is_err()
        );
        assert!(
            f.store
                .publish("project-a", "brief", "Title", "Body", bad)
                .is_err()
        );
        assert!(f.store.search("project-a", bad, 10).is_err());
    }
    assert!(
        f.store
            .publish(
                "project-a",
                "brief",
                &"a".repeat(241),
                "Body",
                "owner:fixture"
            )
            .is_err()
    );
    assert!(
        f.store
            .publish(
                "project-a",
                "brief",
                "Title",
                &"a".repeat(32_769),
                "owner:fixture"
            )
            .is_err()
    );
    assert!(
        f.store
            .publish("project-a", "brief", "Title", "Body", &"a".repeat(2_049))
            .is_err()
    );
    assert!(f.store.search("project-a", &"a".repeat(513), 10).is_err());
    assert!(
        f.store
            .observe(
                "project-a",
                "Owner",
                "req",
                "Title",
                "Body",
                "owner:fixture"
            )
            .is_err()
    );
    assert_eq!(f.store.context("project-a", 100).unwrap().revision, 0);
    assert!(
        f.store
            .publish(
                "project-a",
                "brief",
                &"a".repeat(240),
                &"a".repeat(32_768),
                &"a".repeat(2_048)
            )
            .is_ok()
    );
    assert!(
        f.store
            .publish(
                "project-a",
                "multiline",
                "Title",
                "First\nSecond\r\n\tThird",
                "owner:fixture"
            )
            .is_ok()
    );
}

#[test]
fn concurrent_readers_observe_matching_revision_and_records() {
    let f = Fixture::new();
    f.publish("project-a", "brief", "0");
    let writer = SharedStore::open(&f.dir.join("shared.db")).unwrap();
    let worker = std::thread::spawn(move || {
        for n in 1..100 {
            writer
                .publish(
                    "project-a",
                    "brief",
                    "Project brief",
                    &n.to_string(),
                    "owner:fixture",
                )
                .unwrap();
        }
    });
    for _ in 0..300 {
        let ctx = f.store.context("project-a", 10).unwrap();
        assert_eq!(ctx.records.len(), 1);
        assert_eq!(ctx.revision, ctx.records[0].revision);
        assert_eq!(
            ctx.revision,
            ctx.records[0].content.parse::<i64>().unwrap() + 1
        );
    }
    worker.join().unwrap();
}

#[test]
fn refuses_unrelated_database_without_adding_shared_schema() {
    let f = Fixture::new();
    let path = f.dir.join("unrelated.db");
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(&path).unwrap();
    let foreign = rusqlite::Connection::open(&path).unwrap();
    foreign.execute_batch("CREATE TABLE private_fixture (content TEXT); INSERT INTO private_fixture VALUES ('fixture only');").unwrap();
    assert!(SharedStore::open(&path).is_err());
    let tables: i64 = foreign
        .query_row(
            "SELECT count(*) FROM sqlite_schema WHERE type='table'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(tables, 1);
    let content: String = foreign
        .query_row("SELECT content FROM private_fixture", [], |row| row.get(0))
        .unwrap();
    assert_eq!(content, "fixture only");
}

#[cfg(unix)]
#[test]
fn file_permissions_and_link_checks_protect_database_and_sidecars() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let f = Fixture::new();
    let path = f.dir.join("shared.db");
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let alias = f.dir.join("alias.db");
    symlink(&path, &alias).unwrap();
    assert!(SharedStore::open(&alias).is_err());
    let hard_link = f.dir.join("hard.db");
    fs::hard_link(&path, &hard_link).unwrap();
    assert!(SharedStore::open(&hard_link).is_err());
    fs::remove_file(&hard_link).unwrap();
    let sidecar_db = f.dir.join("sidecar.db");
    symlink(&path, f.dir.join("sidecar.db-wal")).unwrap();
    assert!(SharedStore::open(&sidecar_db).is_err());
    assert!(!sidecar_db.exists());
    let insecure = f.dir.join("insecure.db");
    fs::write(&insecure, b"").unwrap();
    fs::set_permissions(&insecure, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(SharedStore::open(&insecure).is_err());
    assert_eq!(
        fs::metadata(&insecure).unwrap().permissions().mode() & 0o777,
        0o644
    );
}
