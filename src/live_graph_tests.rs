use super::*;
use crate::storage::projects::ProjectState;
use chrono::{Duration, NaiveDate};
use std::sync::atomic::{AtomicUsize, Ordering};

struct Counted(Arc<AtomicUsize>);
impl crate::live_signal::Post for Counted {
    fn post(&self, _: &str) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

/// A fixed local calendar that is UTC-4. A thirty-day rolling UTC window
/// would wrongly include the oldest excluded segment used below.
struct Local4;
impl Calendar for Local4 {
    fn bounds(&self, day: NaiveDate) -> Option<(DateTime<Utc>, DateTime<Utc>)> {
        let from = day.and_hms_opt(4, 0, 0)?.and_utc();
        Some((from, from + Duration::days(1)))
    }
    fn date_of(&self, at: DateTime<Utc>) -> NaiveDate {
        (at - Duration::hours(4)).date_naive()
    }
}

struct Fixture {
    root: tempfile::TempDir,
    storage: Arc<Storage>,
    live: Arc<Live>,
    posted: Arc<AtomicUsize>,
    now: DateTime<Utc>,
    clock: Instant,
}
impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let storage = Arc::new(Storage::open(&root.path().join("memory.db")).unwrap());
        let posted = Arc::new(AtomicUsize::new(0));
        let live = Live::new(
            "graph-fixture",
            "graph-fixture".into(),
            Box::new(Counted(posted.clone())),
        );
        live.graph.lock().unwrap().source = Some(Arc::new(Source {
            storage: storage.clone(),
            tech: vec!["rust".into()],
            people: vec!["reviewer".into()],
            refresh: Mutex::new(()),
        }));
        Self {
            root,
            storage,
            live,
            posted,
            now: NaiveDate::from_ymd_opt(2026, 10, 3)
                .unwrap()
                .and_hms_opt(16, 0, 0)
                .unwrap()
                .and_utc(),
            clock: Instant::now(),
        }
    }
    fn project(&self, name: &str) -> i64 {
        self.storage.project_upsert(name, name, None).unwrap()
    }
    fn segment(&self, name: &str, project: i64, from: DateTime<Utc>, to: DateTime<Utc>) {
        self.storage.conn.lock().unwrap().execute(
            "INSERT INTO work_segments(stream,driver,agent,project_id,started_at,last_at,source)
            VALUES (?1,'owner','codex',?2,?3,?4,'path')",
            rusqlite::params![name,project,from.format("%Y-%m-%d %H:%M:%S").to_string(),to.format("%Y-%m-%d %H:%M:%S").to_string()],
        ).unwrap();
    }
    fn refresh(&self, seconds: i64) -> bool {
        self.live
            .refresh_graph_at(
                &Local4,
                self.now + Duration::seconds(seconds),
                self.clock + std::time::Duration::from_secs(seconds as u64),
            )
            .unwrap()
    }
    fn changes(&self) -> u64 {
        self.storage.conn.lock().unwrap().total_changes()
    }
}

#[test]
fn curated_graph_runtime_reads_are_byte_stable_bounded_and_write_nothing() {
    let f = Fixture::new();
    let mut ids = Vec::new();
    for n in 0..10 {
        let id = f.project(&format!("project-{n}"));
        f.segment(
            &format!("stream-{n}"),
            id,
            f.now - Duration::minutes(2),
            f.now - Duration::minutes(1),
        );
        ids.push(id);
    }
    assert!(f.refresh(0));
    let before = f.changes();
    let overview = f.live.graph_snapshot(None).unwrap().unwrap();
    let first_key = format!("project:{}", ids[0]);
    let first = f.live.graph_snapshot(Some(&first_key)).unwrap().unwrap();
    for id in ids {
        let key = format!("project:{id}");
        assert!(f.live.graph_snapshot(Some(&key)).unwrap().is_some());
    }
    assert_eq!(f.live.graph.lock().unwrap().expansions.len(), 8);
    let again = f.live.graph_snapshot(Some(&first_key)).unwrap().unwrap();
    assert_eq!(again.bytes, first.bytes);
    assert_eq!(again.etag, first.etag);
    assert_eq!(
        f.live.graph_snapshot(None).unwrap().unwrap().bytes,
        overview.bytes
    );
    assert!(
        f.live
            .graph_snapshot(Some("project:999999"))
            .unwrap()
            .is_none()
    );
    assert_eq!(f.changes(), before);
    assert_eq!(f.posted.load(Ordering::SeqCst), 1);
}

#[test]
fn curated_graph_runtime_noop_refresh_neither_writes_nor_signals() {
    let f = Fixture::new();
    let id = f.project("fixture");
    f.segment(
        "current",
        id,
        f.now - Duration::minutes(2),
        f.now - Duration::minutes(1),
    );
    assert!(f.refresh(0));
    let before = f.changes();
    let bytes = f.live.graph_snapshot(None).unwrap().unwrap().bytes.clone();
    let key = format!("project:{id}");
    f.live.graph_snapshot(Some(&key)).unwrap().unwrap();
    assert!(!f.refresh(60));
    assert_eq!(f.changes(), before);
    assert_eq!(f.posted.load(Ordering::SeqCst), 1);
    assert_eq!(f.live.graph_snapshot(None).unwrap().unwrap().bytes, bytes);
    assert!(f.live.graph.lock().unwrap().expansions.is_empty());
    // A hidden stale project still changes an expansion, even if its overview
    // row is later omitted by the overview cap.
    assert!(f.refresh(86_400));
    assert_eq!(f.posted.load(Ordering::SeqCst), 2);
}

#[test]
fn curated_graph_runtime_archive_and_new_nodes_preserve_prior_slots() {
    let f = Fixture::new();
    let first = f.project("first");
    let second = f.project("second");
    f.refresh(0);
    let slots = f.storage.graph_slots().unwrap();
    f.storage
        .project_set_state(first, ProjectState::Archived)
        .unwrap();
    assert!(f.refresh(60));
    assert!(
        f.live
            .graph_snapshot(Some(&format!("project:{first}")))
            .unwrap()
            .is_none()
    );
    assert_eq!(f.storage.graph_slots().unwrap(), slots);
    let third = f.project("third");
    assert!(f.refresh(120));
    let after = f.storage.graph_slots().unwrap();
    assert!(slots.iter().all(|slot| after.contains(slot)));
    let at = |id| {
        after
            .iter()
            .find(|s| s.key == format!("project:{id}"))
            .unwrap()
            .slot
    };
    assert!(at(third) > at(second));
}

#[test]
fn curated_graph_runtime_hours_are_separate_and_use_thirty_local_dates() {
    let f = Fixture::new();
    let id = f.project("hours");
    let today = Local4.date_of(f.now);
    let first = today - Duration::days(29);
    let (first_start, _) = Local4.bounds(first).unwrap();
    f.segment("kept", id, first_start, first_start + Duration::hours(1));
    f.segment(
        "old",
        id,
        first_start - Duration::hours(2),
        first_start - Duration::hours(1),
    );
    f.segment(
        "future",
        id,
        f.now + Duration::hours(1),
        f.now + Duration::hours(2),
    );
    let absent = data::hours(&f.storage, None, &Local4, f.now).unwrap();
    assert_eq!(
        absent[&id],
        Hours {
            owner_seconds: None,
            agent_seconds: Some(3_600)
        }
    );
    let path = f.root.path().join("activity.db");
    let activity = ActivityStore::open(&path).unwrap();
    let key = f.storage.attribution_keys().unwrap()[&id].clone();
    activity
        .write_day(first, &[(key.as_str(), 900.0, "high")], 0.0)
        .unwrap();
    activity
        .write_day(today, &[(key.as_str(), 900.0, "high")], 0.0)
        .unwrap();
    activity
        .write_day(
            first - Duration::days(1),
            &[(key.as_str(), 9_000.0, "high")],
            0.0,
        )
        .unwrap();
    let observer = rusqlite::Connection::open(&path).unwrap();
    let version = || {
        observer
            .query_row("PRAGMA data_version", [], |r| r.get::<_, i64>(0))
            .unwrap()
    };
    let before = (f.changes(), version());
    let hours = data::hours(&f.storage, Some(&activity), &Local4, f.now).unwrap();
    assert_eq!(
        hours[&id],
        Hours {
            owner_seconds: Some(1_800),
            agent_seconds: Some(3_600)
        }
    );
    assert_eq!((f.changes(), version()), before);
}

#[test]
fn curated_graph_runtime_disabled_configuration_never_allocates_slots() {
    let f = Fixture::new();
    f.project("fixture");
    let before = f.changes();
    f.live
        .configure_graph(f.storage.clone(), &Config::default())
        .unwrap();
    assert!(f.live.graph_snapshot(None).unwrap().is_none());
    assert!(f.storage.graph_slots().unwrap().is_empty());
    assert_eq!(f.changes(), before);
    let mut config = Config::default();
    config.graph.tech = vec!["My-Tech".into(), " RUST ".into()];
    config.graph.people = vec!["Reviewer".into(), "Blocked Person".into()];
    config.graph.deny = vec!["rust".into(), "blocked person".into()];
    let (tech, people) = vocabulary(&config);
    assert!(!tech.contains(&"rust".into()));
    assert!(tech.contains(&"tech".into()));
    assert_eq!(people, ["reviewer"]);
}

#[test]
fn curated_graph_runtime_vocabulary_checks_raw_and_both_canonical_forms() {
    use crate::graph::canonical::{canonicalize_name, canonicalize_name_uncapped};
    use crate::redaction::state::check_identities;
    // Synthetic values only; assertions never print these identity strings.
    let body = "a1B2c3D4e5F6".repeat(3);
    let assignment = format!("password={body}");
    assert!(check_identities([assignment.as_str()]).is_err());
    assert!(check_identities([canonicalize_name(&assignment).as_str()]).is_ok());
    let joined = ["sk", "proj", body.as_str()].join(" ");
    assert!(check_identities([joined.as_str()]).is_ok());
    assert!(check_identities([canonicalize_name(&joined).as_str()]).is_err());
    let capped = format!("{} {joined}", "ordinary ".repeat(15));
    assert!(check_identities([capped.as_str()]).is_ok());
    assert!(check_identities([canonicalize_name(&capped).as_str()]).is_ok());
    assert!(check_identities([canonicalize_name_uncapped(&capped).as_str()]).is_err());
    let unsafe_names = vec![assignment, joined, capped];
    let mut config = Config::default();
    config.graph.people = unsafe_names.clone();
    config
        .graph
        .people
        .extend(["Reviewer".into(), "Blocked Person".into()]);
    config.graph.tech = unsafe_names.clone();
    config
        .graph
        .tech
        .extend(["Safe-Tool".into(), " RUST ".into()]);
    config.graph.deny = vec!["Blocked Person".into(), "rust".into()];
    let (tech, people) = vocabulary(&config);
    assert_eq!(people, ["reviewer"]);
    assert!(tech.contains(&"safe-tool".into()));
    assert!(!tech.contains(&"rust".into()));
    for raw in unsafe_names {
        assert!(!tech.contains(&canonicalize_name(&raw)));
    }
}

#[test]
fn curated_graph_runtime_units_keep_archive_flags_without_file_activity_guesses() {
    let f = Fixture::new();
    let id = f.project("fixture");
    let workspace = f.root.path().join("workspace");
    let project = workspace.join("fixture");
    for (folder, tasks) in [
        ("active-unit", "- [ ] T01: Do the work\n"),
        ("archive/2001-01-01-closed-unit", "- [x] T02: Closed work\n"),
    ] {
        let path = project.join("openspec/changes").join(folder);
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(path.join("tasks.md"), tasks).unwrap();
    }
    let receipt = project.join("target/checks/closed-unit/1");
    std::fs::create_dir_all(&receipt).unwrap();
    std::fs::write(
        receipt.join("verification.json"),
        r#"{"change":"closed-unit","passed":false}"#,
    )
    .unwrap();
    let config = crate::config::ProjectsConfig {
        workspace: Some(workspace),
        ..Default::default()
    };
    f.live.configure_plans(f.storage.clone(), config);
    let at = f.now.timestamp() - 420;
    {
        let conn = f.storage.conn.lock().unwrap();
        conn.execute("INSERT INTO entities(id,name,entity_type,mention_count) VALUES ('unit-entity','active-unit','concept',1)", []).unwrap();
        conn.execute("INSERT INTO entities(id,name,entity_type,mention_count) VALUES ('project-entity','fixture','project',1)", []).unwrap();
        conn.execute("INSERT INTO memories(id,timestamp,title,content,memory_type,source)
            VALUES ('unit-memory',datetime(?1,'unixepoch'),'Unit memory','Evidence','decision','test')", [at]).unwrap();
        conn.execute(
            "INSERT INTO memory_entities(memory_id,entity_id) VALUES ('unit-memory','unit-entity')",
            [],
        )
        .unwrap();
        conn.execute("INSERT INTO memory_entities(memory_id,entity_id) VALUES ('unit-memory','project-entity')", []).unwrap();
    }
    f.segment(
        "project-current",
        id,
        f.now - Duration::minutes(2),
        f.now - Duration::minutes(1),
    );
    f.refresh(60);
    let snapshot = f
        .live
        .graph_snapshot(Some(&format!("project:{id}")))
        .unwrap()
        .unwrap();
    let active = snapshot
        .payload
        .n
        .iter()
        .find(|n| n.key == format!("unit:{id}:active-unit"))
        .unwrap();
    assert!(!active.closed);
    assert_eq!(active.last_activity, Some(at));
    let closed = snapshot
        .payload
        .n
        .iter()
        .find(|n| n.key == format!("unit:{id}:closed-unit"))
        .unwrap();
    assert!(closed.closed);
    assert_eq!((closed.last_activity, closed.days_stale), (None, None));
    assert_eq!(closed.hours_30d_tenths, [None, None]);
    let ids = f
        .live
        .graph_edge_memories(
            &format!("project:{id}"),
            &active.key,
            LinkKind::Contains,
            EdgePage {
                offset: 0,
                limit: usize::MAX,
            },
        )
        .unwrap()
        .unwrap();
    let index = snapshot
        .payload
        .n
        .iter()
        .position(|n| n.key == active.key)
        .unwrap() as u16;
    let link = snapshot
        .payload
        .e
        .iter()
        .find(|e| e.1 == index && e.2 == 0)
        .unwrap();
    assert_eq!(ids, ["unit-memory"]);
    assert_eq!(link.3 as usize, ids.len());
    assert!(
        f.live
            .graph_edge_memories(
                "project:999999",
                &active.key,
                LinkKind::Contains,
                EdgePage {
                    offset: 0,
                    limit: 100
                }
            )
            .unwrap()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn curated_graph_runtime_initial_registry_sync_bypasses_the_recent_plan_sweep() {
    let f = Fixture::new();
    let workspace = f.root.path().join("workspace");
    let project = workspace.join("new-project");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(project.join("tasks.md"), "- [ ] T01: First task\n").unwrap();
    let mut config = Config::default();
    config.projects.workspace = Some(workspace);
    config.projects.plans.insert(
        "new-project".into(),
        crate::plans::Config {
            provider: Some(crate::plans::Provider::Checkbox),
            authority: Some("tasks.md".into()),
            ..Default::default()
        },
    );
    f.live
        .configure_plans(f.storage.clone(), config.projects.clone());
    f.live.configure_graph(f.storage.clone(), &config).unwrap();
    assert!(f.live.project_plans().is_empty());
    let id = f.project("new-project");
    // Simulates the immediate startup registry pass, with no five-second wait.
    f.live.refresh_graph().unwrap();
    assert_eq!(f.live.project_plans()[&id].units.len(), 1);
    let snapshot = f
        .live
        .graph_snapshot(Some(&format!("project:{id}")))
        .unwrap()
        .unwrap();
    assert!(
        snapshot
            .payload
            .n
            .iter()
            .any(|node| node.kind == curated_graph::Kind::Unit)
    );
}

#[path = "live_graph_unit_collision_tests.rs"]
mod unit_collision_tests;

#[path = "live_graph_parent_tests.rs"]
mod parent_tests;
