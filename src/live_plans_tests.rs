use super::*;
use crate::live::Session;
use crate::projects::facts::Kind;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Counted(Arc<AtomicUsize>);

impl crate::live_signal::Post for Counted {
    fn post(&self, _name: &str) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

struct Fixture {
    temp: tempfile::TempDir,
    live: Arc<Live>,
    posted: Arc<AtomicUsize>,
    project: i64,
    main: PathBuf,
    now: DateTime<Utc>,
}

fn write_tasks(root: &Path, done: usize) {
    std::fs::create_dir_all(root).unwrap();
    let text = (1..=3)
        .map(|n| {
            format!(
                "- [{}] T{n:02}: Step {n}\n",
                if n <= done { "x" } else { " " }
            )
        })
        .collect::<String>();
    // Changing the length avoids relying on the filesystem's time precision.
    std::fs::write(
        root.join("tasks.md"),
        format!("{text}{}", "\n".repeat(done)),
    )
    .unwrap();
}

fn isolate_repository(root: &Path) {
    std::fs::create_dir_all(root).unwrap();
    let mut options = git2::RepositoryInitOptions::new();
    options.external_template(false).initial_head("main");
    git2::Repository::init_opts(root, &options).unwrap();
}

fn commit_repository(root: &Path) {
    let repo = git2::Repository::open(root).unwrap();
    let mut index = repo.index().unwrap();
    index
        .add_all(["."], git2::IndexAddOption::DEFAULT, None)
        .unwrap();
    index.write().unwrap();
    let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
    let email = ["fixture", "invalid"].join("@");
    let signature = git2::Signature::new("Fixture", &email, &git2::Time::new(1, 0)).unwrap();
    repo.commit(Some("HEAD"), &signature, &signature, "Fixture", &tree, &[])
        .unwrap();
}

#[test]
fn plan_registry_path_cannot_reset_or_escape_the_workspace() {
    let temp = tempfile::tempdir().unwrap();
    let storage = Storage::open(&temp.path().join("memory.db")).unwrap();
    storage.project_upsert("fixture", "fixture", None).unwrap();
    let mut row = storage.projects().unwrap().remove(0);
    for name in ["/", ".", "..", "../outside", "nested/child"] {
        row.name = name.into();
        assert!(project_path(temp.path(), &row, &[]).is_none());
    }
    row.name = "fixture".into();
    assert_eq!(
        project_path(temp.path(), &row, &[]),
        Some(temp.path().join("fixture"))
    );
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        isolate_repository(&workspace);
        let main = workspace.join("projects/demoapp");
        write_tasks(&main, 0);
        commit_repository(&workspace);
        let storage = Arc::new(Storage::open(&temp.path().join("memory.db")).unwrap());
        let project = storage.project_upsert("demoapp", "demoapp", None).unwrap();
        let posted = Arc::new(AtomicUsize::new(0));
        let live = Live::new(
            "plan-test",
            "plan-signal".into(),
            Box::new(Counted(posted.clone())),
        );
        let now = Utc::now();
        live.load_projects(&storage).unwrap();
        live.observed(
            "fixture-session",
            Session {
                agent: "claude".into(),
                program: false,
                project: Some(project),
                last_kind: Kind::Call,
                last_at: now,
                started_at: now,
            },
        );
        live.configure_plans(
            storage,
            ProjectsConfig {
                workspace: Some(workspace),
                folder: Some("projects".into()),
                plans: BTreeMap::from([(
                    "demoapp".into(),
                    crate::plans::Config {
                        provider: Some(crate::plans::Provider::Checkbox),
                        authority: Some("tasks.md".into()),
                        gate_required: Some(true),
                        ..Default::default()
                    },
                )]),
                ..Default::default()
            },
        );
        Self {
            temp,
            live,
            posted,
            project,
            main,
            now,
        }
    }

    fn done(&self) -> u32 {
        self.live.project_plans()[&self.project].units[0].done
    }

    fn clock(&self) -> Instant {
        self.live.plans.lock().unwrap().last_sweep.unwrap()
    }

    fn signal_pending_or_sent(&self, before: usize) -> bool {
        self.posted.load(Ordering::SeqCst) > before
            || self.live.throttle.lock().unwrap().due().is_some()
    }
}

#[test]
fn plan_live_due_request_reads_edits_without_observed_records_and_keeps_identical_version() {
    let f = Fixture::new();
    let clock = f.clock();
    let original = f.live.state.snapshot();
    write_tasks(&f.main, 1);
    f.live
        .refresh_plans_at(f.now, clock + Duration::from_secs(4), false);
    assert_eq!(f.done(), 0);
    assert_eq!(f.live.state.snapshot(), original);
    let posted = f.posted.load(Ordering::SeqCst);
    f.live
        .refresh_plans_at(f.now, clock + Duration::from_secs(5), false);
    assert_eq!(f.done(), 1);
    assert_ne!(f.live.state.snapshot().0, original.0);
    assert!(f.signal_pending_or_sent(posted));
    let changed = f.live.state.snapshot();
    let parsed = f.live.plans.lock().unwrap().reader.parsed_count();
    f.live
        .refresh_plans_at(f.now, clock + Duration::from_secs(10), false);
    assert_eq!(f.live.state.snapshot(), changed);
    assert_eq!(f.live.plans.lock().unwrap().reader.parsed_count(), parsed);
}

#[test]
fn plan_live_forced_sweep_sees_new_receipt_without_reparsing_tasks() {
    let f = Fixture::new();
    let clock = f.clock();
    let parsed = f.live.plans.lock().unwrap().reader.parsed_count();
    let folder = f.main.join("target/checks/plan/1");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(
        folder.join("verification.json"),
        r#"{"change":"plan","passed":false}"#,
    )
    .unwrap();
    let posted = f.posted.load(Ordering::SeqCst);
    f.live
        .refresh_plans_at(f.now, clock + Duration::from_secs(1), true);
    assert_eq!(
        f.live.project_plans()[&f.project].units[0].gate.status,
        crate::plans::gate::GateStatus::Failed
    );
    assert_eq!(f.live.plans.lock().unwrap().reader.parsed_count(), parsed);
    assert!(f.signal_pending_or_sent(posted));
}

#[test]
fn plan_live_locations_find_two_checkouts_outside_the_workspace() {
    let f = Fixture::new();
    let clock = f.clock();
    for (name, done) in [("external-one", 1), ("external-two", 2)] {
        let root = f.temp.path().join(name);
        git2::Repository::discover(&f.main)
            .unwrap()
            .worktree(name, &root, None)
            .unwrap();
        let project = root.join("projects/demoapp");
        write_tasks(&project, done);
        f.live.observed_with_location(
            name,
            Session {
                agent: "codex".into(),
                program: false,
                project: Some(f.project),
                last_kind: Kind::Call,
                last_at: f.now,
                started_at: f.now,
            },
            Some(Location {
                cwd: Some(project),
                branch: Some("task/T03".into()),
            }),
        );
    }
    f.live
        .refresh_plans_at(f.now, clock + Duration::from_secs(5), false);
    let plans = f.live.project_plans();
    let unit = &plans[&f.project].units[0];
    assert_eq!(unit.done, 2);
    assert_eq!(unit.checkouts, 2);
    assert_eq!(unit.current_task.as_ref().unwrap().id, "T03");
    let served = serde_json::to_string(&plans).unwrap();
    assert!(!served.contains(f.temp.path().to_str().unwrap()));
    assert!(!served.contains("external-one") && !served.contains("external-two"));
    f.live.forget("external-one");
    f.live.forget("external-two");
    f.live
        .refresh_plans_at(f.now, clock + Duration::from_secs(6), true);
    assert_eq!(f.done(), 0);
}

#[test]
fn plan_live_merged_session_selects_canonical_checkout_and_unmerge_restores_its_plan() {
    let f = Fixture::new();
    let (storage, mut config) = f.live.plans.lock().unwrap().source.clone().unwrap();
    let former = storage.project_upsert("former", "former", None).unwrap();
    write_tasks(&config.root().unwrap().join("former"), 0);
    config
        .plans
        .insert("former".into(), config.plans["demoapp"].clone());
    f.live.configure_plans(storage.clone(), config);
    let clock = f.clock();
    let external = f.temp.path().join("external");
    git2::Repository::discover(&f.main)
        .unwrap()
        .worktree("external", &external, None)
        .unwrap();
    write_tasks(&external.join("projects/demoapp"), 2);
    let former_checkout = external.join("projects/former");
    write_tasks(&former_checkout, 1);
    f.live.observed_with_location(
        "merged-session",
        Session {
            agent: "codex".into(),
            program: false,
            project: Some(former),
            last_kind: Kind::Call,
            last_at: f.now,
            started_at: f.now,
        },
        Some(Location {
            cwd: Some(former_checkout),
            branch: Some("task/T03".into()),
        }),
    );
    storage.project_merge(former, f.project).unwrap();
    f.live.load_projects(&storage).unwrap();
    f.live
        .refresh_plans_at(f.now, clock + Duration::from_secs(5), true);
    let merged = f.live.project_plans();
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[&f.project].units[0].done, 2);
    assert_eq!(merged[&f.project].units[0].checkouts, 1);
    assert_eq!(
        f.live
            .state
            .sessions(f.now)
            .into_iter()
            .find(|(stream, _)| stream == "merged-session")
            .unwrap()
            .1
            .project,
        Some(former)
    );

    assert!(storage.project_unmerge(former).unwrap());
    f.live.load_projects(&storage).unwrap();
    f.live
        .refresh_plans_at(f.now, clock + Duration::from_secs(6), true);
    let restored = f.live.project_plans();
    assert_eq!(restored.len(), 2);
    assert_eq!(restored[&f.project].units[0].done, 0);
    assert_eq!(restored[&former].units[0].done, 1);
    assert_eq!(restored[&former].units[0].checkouts, 1);
}
