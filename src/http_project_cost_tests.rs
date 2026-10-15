//! The cost of a board answer (design 9.4), over synthetic stores of three
//! sizes: the statements it runs and the plan files it parses are counted,
//! and the processor time is kept as evidence by an ignored test.
use super::super::AppState;
use super::{board_value, plans};
use crate::embedding::HashEmbedder;
use crate::graph::{Entity, EntityType};
use crate::storage::Storage;
use chrono::{Duration, Local, Utc};
use rusqlite::trace::TraceEvent;
use std::cell::Cell;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

const SIZES: [(usize, usize); 3] = [(5, 200), (30, 5_000), (100, 50_000)];

thread_local! {
    static STATEMENTS: Cell<usize> = const { Cell::new(0) };
}

/// Statements of a trigger arrive as comments and are not counted.
fn count(event: TraceEvent<'_>) {
    if let TraceEvent::Stmt(_, sql) = event
        && !sql.trim_start().starts_with("--")
    {
        STATEMENTS.with(|count| count.set(count.get() + 1));
    }
}

struct Silent;

impl crate::live_signal::Post for Silent {
    fn post(&self, _name: &str) {}
}

struct Store {
    _temp: tempfile::TempDir,
    state: AppState,
    plans: Vec<PathBuf>,
}

/// Projects with a plan file each, segments spread over the seven local
/// days of the board, and a day of the owner's activity for each of them.
fn store(projects: usize, segments: usize) -> Store {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("workspace");
    let storage = Arc::new(Storage::open(&temp.path().join("memory.db")).unwrap());
    let activity =
        Arc::new(crate::activity::ActivityStore::open(&temp.path().join("activity.db")).unwrap());
    let mut ids = Vec::new();
    let mut entities = Vec::new();
    let mut plans = Vec::new();
    let mut config = crate::config::Config::default();
    for n in 0..projects {
        let key = format!("project{n:03}");
        let folder = workspace.join("projects").join(&key);
        std::fs::create_dir_all(&folder).unwrap();
        let plan = folder.join("tasks.md");
        std::fs::write(
            &plan,
            "- [x] T01: Build the fixture\n- [ ] T02: Review the result\n  Owner: Confirm the copy\n",
        )
        .unwrap();
        plans.push(plan);
        let id = storage.project_upsert(&key, &key, None).unwrap();
        entities.push(
            storage
                .upsert_entity(&Entity {
                    name: key.clone(),
                    entity_type: EntityType::Project,
                })
                .unwrap(),
        );
        storage.project_link_entity(id, &key).unwrap();
        ids.push(id);
        config.projects.plans.insert(
            key,
            crate::plans::Config {
                provider: Some(crate::plans::Provider::Checkbox),
                authority: Some("tasks.md".into()),
                gate_required: Some(false),
                ..Default::default()
            },
        );
    }
    config.projects.workspace = Some(workspace);
    config.projects.folder = Some("projects".into());
    let today = Local::now().date_naive();
    let days: Vec<_> = (0..7)
        .map(|offset| today - Duration::days(offset))
        .collect();
    {
        let conn = storage.conn.lock().unwrap();
        conn.execute_batch("BEGIN").unwrap();
        for i in 0..segments {
            let (from, _) = crate::activity::local_day_bounds_utc(days[i % 7]).unwrap();
            let start = from + Duration::minutes(((i / 7) % 1_400) as i64);
            conn.execute(
                "INSERT INTO work_segments
                (stream, driver, agent, project_id, started_at, last_at, source)
                VALUES (?1, 'owner', 'claude', ?2, ?3, ?4, 'path')",
                rusqlite::params![
                    format!("stream-{i}"),
                    ids[i % projects],
                    start.format("%Y-%m-%d %H:%M:%S").to_string(),
                    (start + Duration::minutes(5))
                        .format("%Y-%m-%d %H:%M:%S")
                        .to_string()
                ],
            )
            .unwrap();
        }
        conn.execute_batch("COMMIT").unwrap();
    }
    for day in &days {
        let rows: Vec<(&str, f64, &str)> = entities
            .iter()
            .map(|entity| (entity.as_str(), 60.0, "high"))
            .collect();
        activity.write_day(*day, &rows, 30.0).unwrap();
    }
    let live = crate::live_run::Live::new("cost-test", "test signal".into(), Box::new(Silent));
    live.load_projects(&storage).unwrap();
    live.configure_plans(storage.clone(), config.projects.clone());
    Store {
        _temp: temp,
        state: AppState {
            storage,
            embedder: Arc::new(HashEmbedder::new()),
            token: "test-token".into(),
            config,
            activity: Some(activity),
            live: Some(live),
        },
        plans,
    }
}

/// The reads of a board answer before design 9.2, as the board ran them at
/// the commit before it: the additions must not add one.
fn reads_before(state: &AppState) {
    let _plans = plans(state);
    let rows = state.storage.projects().unwrap();
    state.storage.project_last_activity().unwrap();
    state.storage.attribution_keys().unwrap();
    let today = Local::now().date_naive();
    for offset in (0..7).rev() {
        let day = today - Duration::days(offset);
        let Some((from, to)) = crate::activity::local_day_bounds_utc(day) else {
            continue;
        };
        state.storage.agent_time(from, to.min(Utc::now())).unwrap();
        if let Some(activity) = &state.activity {
            let (rows, _) = activity.project_attribution_for_day(day).unwrap();
            state.storage.rekeyed_day(rows).unwrap();
        }
    }
    for row in rows {
        if crate::redaction::state::check_identities([row.slug.as_str()]).is_ok() {
            state.storage.project_canonical(row.id).unwrap();
        }
    }
}

fn answer(state: &AppState) {
    board_value(state).unwrap();
}

/// Each plan file is written again and the sweep is due, as for the first
/// answer after the files changed; then one read is counted.
fn measure(store: &Store, read: fn(&AppState)) -> (usize, usize) {
    for path in &store.plans {
        let text = std::fs::read(path).unwrap();
        std::fs::write(path, text).unwrap();
    }
    let live = store.state.live.as_ref().unwrap();
    live.expire_plan_sweep();
    let parsed = live.plan_files_parsed();
    STATEMENTS.with(|count| count.set(0));
    read(&store.state);
    (
        STATEMENTS.with(Cell::get),
        live.plan_files_parsed() - parsed,
    )
}

fn traced(store: &Store, on: bool) {
    let trace = on.then_some(count as fn(TraceEvent<'_>));
    store
        .state
        .storage
        .conn
        .lock()
        .unwrap()
        .trace_v2(rusqlite::trace::TraceEventCodes::SQLITE_TRACE_STMT, trace);
    store.state.activity.as_ref().unwrap().trace(trace);
}

#[test]
fn board_cost_runs_and_parses_what_it_did_before_the_additions() {
    for (projects, segments) in SIZES {
        let store = store(projects, segments);
        traced(&store, true);
        let before = measure(&store, reads_before);
        let after = measure(&store, answer);
        traced(&store, false);
        assert_eq!(after, before, "{projects} projects, {segments} segments");
        assert_eq!(before.1, projects, "the answer parses each plan file once");
        assert!(before.0 > 0);
    }
}

/// Processor time of the whole process, in microseconds.
fn processor_time() -> i64 {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
    // SAFETY: getrusage writes the whole structure when it returns 0.
    let usage = unsafe {
        assert_eq!(libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()), 0);
        usage.assume_init()
    };
    let micros = |time: libc::timeval| time.tv_sec * 1_000_000 + i64::from(time.tv_usec);
    micros(usage.ru_utime) + micros(usage.ru_stime)
}

/// Evidence, not a gate: run alone in a release build, at the commit before
/// the additions and at theirs, and keep the lines it prints.
#[test]
#[ignore = "evidence: cargo test --release board_cost_time -- --ignored --nocapture --test-threads=1"]
fn board_cost_time_of_a_first_answer_and_twenty_more() {
    let mut lines = BTreeMap::new();
    for (projects, segments) in SIZES {
        let store = store(projects, segments);
        let live = store.state.live.as_ref().unwrap();
        for path in &store.plans {
            let text = std::fs::read(path).unwrap();
            std::fs::write(path, text).unwrap();
        }
        live.expire_plan_sweep();
        let start = processor_time();
        answer(&store.state);
        let first = processor_time() - start;
        let start = processor_time();
        for _ in 0..20 {
            live.expire_plan_sweep();
            answer(&store.state);
        }
        let more = processor_time() - start;
        lines.insert(
            projects,
            format!(
                "board_cost {projects} projects {segments} segments: first {first} us, twenty more {more} us"
            ),
        );
    }
    for line in lines.values() {
        println!("{line}");
    }
}
