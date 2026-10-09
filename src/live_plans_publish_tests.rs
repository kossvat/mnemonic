use super::*;
use crate::projects::facts::Kind;
use std::sync::mpsc;

struct Silent;
impl crate::live_signal::Post for Silent {
    fn post(&self, _: &str) {}
}

fn session(project: Option<i64>, at: DateTime<Utc>) -> Session {
    Session {
        agent: "codex".into(),
        program: false,
        project,
        last_kind: Kind::Call,
        last_at: at,
        started_at: at,
    }
}

#[test]
fn plan_live_sweep_cannot_prune_a_location_before_its_session_is_published() {
    for expired in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        let project = workspace.join("demoapp");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(
            project.join("tasks.md"),
            "- [ ] T01: First\n- [ ] T02: Second\n- [ ] T03: Third\n",
        )
        .unwrap();
        let storage = Arc::new(Storage::open(&temp.path().join("memory.db")).unwrap());
        let id = storage.project_upsert("demoapp", "demoapp", None).unwrap();
        let live = Live::new("publication-test", "signal".into(), Box::new(Silent));
        live.load_projects(&storage).unwrap();
        live.configure_plans(
            storage,
            ProjectsConfig {
                workspace: Some(workspace),
                plans: BTreeMap::from([(
                    "demoapp".into(),
                    crate::plans::Config {
                        provider: Some(crate::plans::Provider::Checkbox),
                        authority: Some("tasks.md".into()),
                        ..Default::default()
                    },
                )]),
                ..Default::default()
            },
        );
        let now = Utc::now();
        if expired {
            live.observed(
                "new-session",
                session(Some(id), now - chrono::Duration::minutes(40)),
            );
        }
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        live.plans.lock().unwrap().before_session = Some(Box::new(move || {
            entered_tx.send(()).unwrap();
            release_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        }));
        let publisher = {
            let live = live.clone();
            std::thread::spawn(move || {
                live.observed_with_location(
                    "new-session",
                    session(Some(id), now),
                    Some(Location {
                        cwd: Some(project),
                        branch: Some("task/T03".into()),
                    }),
                )
            })
        };
        entered_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        // Pause precisely after the location insertion and before the new
        // session exists. A due refresh cannot enter its pruning section.
        assert!(matches!(
            live.plans.try_lock(),
            Err(std::sync::TryLockError::WouldBlock)
        ));
        assert!(
            live.state
                .sessions(now)
                .iter()
                .all(|(stream, _)| stream != "new-session")
        );
        let (started_tx, started_rx) = mpsc::channel();
        let (finished_tx, finished_rx) = mpsc::channel();
        let reader = {
            let live = live.clone();
            std::thread::spawn(move || {
                started_tx.send(()).unwrap();
                live.refresh_plans_at(now, Instant::now(), true);
                finished_tx.send(()).unwrap();
            })
        };
        started_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(matches!(
            finished_rx.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
        release_tx.send(()).unwrap();
        publisher.join().unwrap();
        finished_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        reader.join().unwrap();
        assert_eq!(
            live.location("new-session").unwrap().branch.as_deref(),
            Some("task/T03")
        );
        assert_eq!(
            live.project_plans()[&id].units[0]
                .current_task
                .as_ref()
                .unwrap()
                .id,
            "T03"
        );
    }
}

#[test]
fn plan_live_prefix_recovery_requires_the_current_session_and_never_overwrites_location() {
    let live = Live::new("recovery-test", "signal".into(), Box::new(Silent));
    let now = Utc::now();
    let expected = session(Some(1), now);
    let metadata = || Location {
        cwd: None,
        branch: Some("task/T01".into()),
    };
    assert!(!live.recover_location("stream", &expected, metadata()));
    live.observed(
        "stream",
        session(Some(1), now + chrono::Duration::seconds(1)),
    );
    assert!(!live.recover_location("stream", &expected, metadata()));
    live.observed("stream", session(Some(2), now));
    assert!(!live.recover_location("stream", &expected, metadata()));
    live.observed("stream", expected.clone());
    assert!(live.recover_location("stream", &expected, Location::default()));
    assert!(!live.recover_location("stream", &expected, metadata()));
    assert!(live.location("stream").unwrap().branch.is_none());
    live.forget("stream");
    assert!(live.location("stream").is_none());
    assert!(!live.recover_location("stream", &expected, metadata()));
    live.observed(
        "stream",
        session(Some(1), now - chrono::Duration::minutes(40)),
    );
    assert!(!live.recover_location(
        "stream",
        &session(Some(1), now - chrono::Duration::minutes(40)),
        metadata()
    ));
}
