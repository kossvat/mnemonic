use std::fs;
use std::path::PathBuf;

use super::super::store::SharedStore;
use super::super::types::{Conflict, Expected};

struct Fixture {
    dir: PathBuf,
    store: SharedStore,
}

impl Fixture {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("mnemonic-curation-{}", uuid::Uuid::new_v4()));
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

    fn draft(&self, principal: &str, request: &str, content: &str) -> String {
        self.store
            .observe_as(
                "alpha",
                Some(principal),
                &format!("{principal}-contrib"),
                request,
                "Pricing decision",
                content,
                "call:2026-09-20",
            )
            .unwrap()
            .id
    }

    fn conflict(result: anyhow::Result<super::super::types::SharedRecord>) -> Conflict {
        result
            .unwrap_err()
            .downcast::<Conflict>()
            .expect("a typed revision conflict")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn two_people_never_share_a_writer_quota_or_request_namespace() {
    let f = Fixture::new();
    let a = f.draft("ann", "request-1", "Ann says the plan costs 42");
    let b = f.draft("ben", "request-1", "Ben says the plan costs 40");
    assert_ne!(
        a, b,
        "the same request id from two people is two observations"
    );

    // Filling one person's pending quota leaves the other person's open.
    for n in 0..99 {
        f.draft("ann", &format!("fill-{n}"), "filler");
    }
    assert!(
        f.store
            .observe_as("alpha", Some("ann"), "ann-contrib", "over", "T", "C", "s:1")
            .is_err()
    );
    f.draft("ben", "request-2", "still accepted");

    let inbox = f.store.inbox("alpha", 100).unwrap();
    assert!(
        inbox
            .iter()
            .any(|o| o.principal_id.as_deref() == Some("ben"))
    );
}

#[test]
fn a_stale_curator_cannot_overwrite_newer_text() {
    let f = Fixture::new();
    let publish = |content: &str, expected| {
        f.store.publish_checked(
            "alpha",
            "brief",
            "Brief",
            content,
            "owner:v1",
            expected,
            Some("ann"),
        )
    };
    let first = publish("first", Expected::Absent).unwrap();
    assert_eq!(first.published_by.as_deref(), Some("ann"));

    // The key exists now: "must be absent" and an old revision both conflict.
    let absent = Fixture::conflict(publish("second", Expected::Absent));
    assert_eq!(absent.current_revision, Some(first.revision));
    let second = publish("second", Expected::Revision(first.revision)).unwrap();
    let stale = Fixture::conflict(publish("third", Expected::Revision(first.revision)));
    assert_eq!(stale.current_revision, Some(second.revision));
    assert_eq!(
        f.store.get("alpha", "brief").unwrap().unwrap().content,
        "second"
    );

    // An identical retry is a no-op even with a stale expectation.
    let retry = publish("second", Expected::Revision(first.revision)).unwrap();
    assert_eq!(retry.revision, second.revision);

    // Changing only who approved it is a real change, not a retry: it must
    // go through the revision check and then be recorded.
    let reattribute = |actor, expected| {
        f.store.publish_checked(
            "alpha", "brief", "Brief", "second", "owner:v1", expected, actor,
        )
    };
    assert!(reattribute(Some("ben"), Expected::Revision(first.revision)).is_err());
    let moved = reattribute(Some("ben"), Expected::Revision(second.revision)).unwrap();
    assert_eq!(moved.published_by.as_deref(), Some("ben"));
    assert!(moved.revision > second.revision);
    assert_eq!(
        f.store
            .get("alpha", "brief")
            .unwrap()
            .unwrap()
            .published_by
            .as_deref(),
        Some("ben")
    );

    // Revoke obeys the same rule.
    assert!(
        f.store
            .revoke_checked("alpha", "brief", Expected::Revision(second.revision))
            .is_err()
    );
    assert!(
        f.store
            .revoke_checked("alpha", "brief", Expected::Revision(moved.revision))
            .unwrap()
    );
}

#[test]
fn two_connections_racing_on_one_revision_have_exactly_one_winner() {
    let f = Fixture::new();
    let base = f
        .store
        .publish_checked(
            "alpha",
            "brief",
            "Brief",
            "base",
            "owner:v1",
            Expected::Absent,
            None,
        )
        .unwrap();
    let path = f.dir.join("shared.db");
    let handles: Vec<_> = ["from-ann", "from-ben"]
        .into_iter()
        .map(|content| {
            let path = path.clone();
            let revision = base.revision;
            std::thread::spawn(move || {
                SharedStore::open(&path)
                    .unwrap()
                    .publish_checked(
                        "alpha",
                        "brief",
                        "Brief",
                        content,
                        "owner:v2",
                        Expected::Revision(revision),
                        None,
                    )
                    .is_ok()
            })
        })
        .collect();
    let wins = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .filter(|won| *won)
        .count();
    assert_eq!(wins, 1);
}

#[test]
fn promote_is_the_only_path_from_an_observation_to_published_text() {
    let f = Fixture::new();
    let id = f.draft("ben", "request-1", "The Wombat plan costs 42 per month");

    // The curator must state what the key holds; "whatever" is refused.
    assert!(
        f.store
            .promote("alpha", &id, "pricing", Expected::Any, "ann")
            .is_err()
    );
    assert!(f.store.get("alpha", "pricing").unwrap().is_none());

    let record = f
        .store
        .promote("alpha", &id, "pricing", Expected::Absent, "ann")
        .unwrap();
    assert_eq!(record.content, "The Wombat plan costs 42 per month");
    assert_eq!(record.published_by.as_deref(), Some("ann"));
    assert_eq!(record.origin_observation_id.as_deref(), Some(id.as_str()));
    assert_eq!(record.origin_writer_id.as_deref(), Some("ben-contrib"));

    // It left the inbox, and a retry neither fails nor bumps the revision.
    assert!(f.store.inbox("alpha", 10).unwrap().is_empty());
    let again = f
        .store
        .promote("alpha", &id, "pricing", Expected::Absent, "ann")
        .unwrap();
    assert_eq!(again.revision, record.revision);
    // One observation becomes one key.
    assert!(
        f.store
            .promote("alpha", &id, "other-key", Expected::Absent, "ann")
            .is_err()
    );
}

#[test]
fn promote_refuses_rejected_foreign_and_stale_drafts() {
    let f = Fixture::new();
    let rejected = f.draft("ben", "request-1", "poisoned draft");
    assert!(f.store.review("alpha", &rejected, "rejected").unwrap());
    assert!(
        f.store
            .promote("alpha", &rejected, "pricing", Expected::Absent, "ann")
            .is_err()
    );

    // An observation of another project is not promotable here.
    let foreign = f
        .store
        .observe_as(
            "beta",
            Some("ben"),
            "ben-contrib",
            "r",
            "T",
            "other project",
            "s:1",
        )
        .unwrap()
        .id;
    assert!(
        f.store
            .promote("alpha", &foreign, "pricing", Expected::Absent, "ann")
            .is_err()
    );

    // A draft written against old text cannot replace newer text.
    let current = f
        .store
        .publish_checked(
            "alpha",
            "pricing",
            "Pricing",
            "v1",
            "owner:v1",
            Expected::Absent,
            None,
        )
        .unwrap();
    let newer = f
        .store
        .publish_checked(
            "alpha",
            "pricing",
            "Pricing",
            "v2",
            "owner:v2",
            Expected::Revision(current.revision),
            None,
        )
        .unwrap();
    let draft = f.draft("ben", "request-2", "edit based on v1");
    let stale = Fixture::conflict(f.store.promote(
        "alpha",
        &draft,
        "pricing",
        Expected::Revision(current.revision),
        "ann",
    ));
    assert_eq!(stale.current_revision, Some(newer.revision));
    assert_eq!(
        f.store.get("alpha", "pricing").unwrap().unwrap().content,
        "v2"
    );
    // The refused draft is still pending for the curator to handle.
    assert_eq!(f.store.inbox("alpha", 10).unwrap().len(), 1);
}

#[test]
fn a_pinned_database_serves_exactly_one_project() {
    let f = Fixture::new();
    f.store.pin_project("alpha").unwrap();
    f.store.pin_project("alpha").unwrap();
    assert_eq!(f.store.pinned_project().unwrap().as_deref(), Some("alpha"));
    f.draft("ann", "request-1", "fine");

    // A typo no longer creates a second, empty project.
    assert!(f.store.pin_project("beta").is_err());
    assert!(f.store.context("alpah", 10).is_err());
    assert!(f.store.get("beta", "brief").is_err());
    assert!(f.store.inbox("beta", 10).is_err());
    assert!(
        f.store
            .publish_checked("beta", "brief", "B", "c", "s:1", Expected::Absent, None)
            .is_err()
    );
    assert!(
        f.store
            .observe_as("beta", Some("ann"), "ann-contrib", "r", "T", "C", "s:1")
            .is_err()
    );

    // A database that already holds two projects cannot be pinned.
    let multi = Fixture::new();
    multi.draft("ann", "request-1", "alpha text");
    multi
        .store
        .observe_as("beta", Some("ann"), "ann-contrib", "r", "T", "C", "s:1")
        .unwrap();
    assert!(multi.store.pin_project("alpha").is_err());
    assert_eq!(multi.store.pinned_project().unwrap(), None);
}

#[test]
fn revoking_one_person_leaves_the_other_untouched() {
    let f = Fixture::new();
    assert!(
        f.store
            .revoke_principal("alpha", "ben", Some("ann"))
            .unwrap()
    );
    assert!(
        !f.store
            .revoke_principal("alpha", "ben", Some("ann"))
            .unwrap()
    );
    assert!(f.store.is_principal_revoked("alpha", "ben").unwrap());
    assert!(!f.store.is_principal_revoked("alpha", "ann").unwrap());
    assert!(!f.store.is_principal_revoked("beta", "ben").unwrap());

    let listed = f.store.revocations("alpha").unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].revoked_by.as_deref(), Some("ann"));

    assert!(f.store.restore_principal("alpha", "ben").unwrap());
    assert!(!f.store.is_principal_revoked("alpha", "ben").unwrap());
}
