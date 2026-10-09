//! The location used for plans follows successful observation only.
use super::*;
use crate::live_run::Live;
use crate::plans::location::Location;
use serde_json::{Value, json};
use std::io::Write;

struct Silent;

impl crate::live_signal::Post for Silent {
    fn post(&self, _name: &str) {}
}

struct Fixture {
    dir: tempfile::TempDir,
    storage: Arc<Storage>,
    map: ProjectMap,
    cwd: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let storage = Arc::new(Storage::open(&dir.path().join("memory.db")).unwrap());
        let project = storage.project_upsert("demoapp", "demoapp", None).unwrap();
        let root = dir.path().join("projects");
        let cwd = root.join("demoapp");
        let mut map = ProjectMap::new(&root);
        map.insert("demoapp", project);
        Self {
            dir,
            storage,
            map,
            cwd,
        }
    }

    fn path(&self) -> PathBuf {
        self.dir.path().join("session.jsonl")
    }

    fn live(&self) -> Arc<Live> {
        Live::new("plan-test", "test signal".into(), Box::new(Silent))
    }

    fn pass(&self, format: Format, live: Option<Arc<Live>>) -> Observation {
        Observation::new(self.storage.clone(), format, 1800).with_live(live)
    }

    fn checkpoint(&self, stream: &str) -> Option<Checkpoint> {
        self.storage.observation_checkpoint(stream).unwrap()
    }

    fn stored(&self) -> Vec<Vec<String>> {
        let conn = self.storage.conn.lock().unwrap();
        [
            "observation_checkpoints",
            "work_segments",
            "segment_activity",
            "owner_prompts",
        ]
        .into_iter()
        .map(|table| {
            crate::test_support::rows(&conn, &format!("SELECT * FROM {table} ORDER BY 1, 2"))
        })
        .collect()
    }

    fn changes(&self) -> u64 {
        self.storage.conn.lock().unwrap().total_changes()
    }
}

fn append(path: &Path, rows: &[Value]) {
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap();
    for row in rows {
        writeln!(file, "{row}").unwrap();
    }
}

fn claude(at: DateTime<Utc>, cwd: Option<&Path>, branch: Option<&str>) -> Value {
    let mut row = json!({"type":"user", "entrypoint":"cli", "timestamp":at.to_rfc3339(),
        "message":{"content":"Check the result"}});
    if let Some(cwd) = cwd {
        row["cwd"] = json!(cwd);
    }
    if let Some(branch) = branch {
        row["gitBranch"] = json!(branch);
    }
    row
}

#[test]
fn plan_observation_restart_recovers_latest_metadata_at_end_without_writes() {
    let f = Fixture::new();
    let path = f.path();
    let stream = stream_of(CODEX.namespace, &path);
    let now = Utc::now();
    let later = f.dir.path().join("another-checkout");
    append(
        &path,
        &[
            json!({"type":"session_meta","timestamp":(now - chrono::Duration::seconds(90)).to_rfc3339(),
            "payload":{"cwd":f.cwd,"git":{"branch":"task/T01"},"originator":"desktop"}}),
            json!({"type":"turn_context","timestamp":(now - chrono::Duration::seconds(30)).to_rfc3339(),
            "payload":{"cwd":later,"branch":"task/T02"}}),
        ],
    );
    assert_eq!(
        f.pass(CODEX, None)
            .pass(&path, &stream, &f.map, None)
            .unwrap()
            .0,
        Done::Written
    );
    let checkpoint = f.checkpoint(&stream).unwrap();
    let live = f.live();
    live.rebuild(&f.storage, now).unwrap();
    assert!(live.location(&stream).is_none());
    let (before, changes) = (f.stored(), f.changes());
    let pass = f.pass(CODEX, Some(live.clone()));
    assert_eq!(
        pass.pass(&path, &stream, &f.map, Some(checkpoint.clone()))
            .unwrap()
            .0,
        Done::AtEnd
    );
    let location = live.location(&stream).unwrap();
    assert_eq!(location.cwd, Some(later));
    assert_eq!(location.branch.as_deref(), Some("task/T02"));
    assert_eq!(f.checkpoint(&stream), Some(checkpoint.clone()));
    assert_eq!(f.stored(), before);
    assert_eq!(f.changes(), changes);

    // Even a recovered empty location is known: a full pass must not
    // recover the prefix for a second time.
    let session = live
        .state
        .sessions(now)
        .into_iter()
        .find(|(known, _)| known == &stream)
        .unwrap()
        .1;
    live.observed_with_location(&stream, session, Some(Location::default()));
    assert_eq!(
        pass.pass(&path, &stream, &f.map, Some(checkpoint))
            .unwrap()
            .0,
        Done::AtEnd
    );
    let location = live.location(&stream).unwrap();
    assert!(location.cwd.is_none() && location.branch.is_none());
    assert_eq!(f.changes(), changes);
}

#[test]
fn plan_observation_historical_at_end_does_not_recover_locations() {
    let f = Fixture::new();
    let path = f.path();
    let stream = stream_of(CLAUDE.namespace, &path);
    let at = Utc::now() - chrono::Duration::minutes(31);
    append(&path, &[claude(at, Some(&f.cwd), Some("task/T01"))]);
    f.pass(CLAUDE, None)
        .pass(&path, &stream, &f.map, None)
        .unwrap();
    let live = f.live();
    let changes = f.changes();
    let result = f
        .pass(CLAUDE, Some(live.clone()))
        .pass(&path, &stream, &f.map, f.checkpoint(&stream))
        .unwrap();
    assert_eq!(result.0, Done::AtEnd);
    assert!(live.location(&stream).is_none());
    assert_eq!(f.changes(), changes);
}

#[test]
fn plan_observation_at_end_recovery_cannot_revive_a_missing_live_session() {
    let f = Fixture::new();
    let path = f.path();
    let stream = stream_of(CLAUDE.namespace, &path);
    let now = Utc::now();
    append(&path, &[claude(now, Some(&f.cwd), Some("task/T01"))]);
    f.pass(CLAUDE, None)
        .pass(&path, &stream, &f.map, None)
        .unwrap();
    let checkpoint = f.checkpoint(&stream).unwrap();
    let changes = f.changes();
    let live = f.live();
    let pass = f.pass(CLAUDE, Some(live.clone()));
    assert_eq!(
        pass.pass(&path, &stream, &f.map, Some(checkpoint.clone()))
            .unwrap()
            .0,
        Done::AtEnd
    );
    assert!(live.location(&stream).is_none());
    assert!(live.state.sessions(now).is_empty());
    live.rebuild(&f.storage, now).unwrap();
    assert_eq!(
        pass.pass(&path, &stream, &f.map, Some(checkpoint.clone()))
            .unwrap()
            .0,
        Done::AtEnd
    );
    assert_eq!(
        live.location(&stream).unwrap().branch.as_deref(),
        Some("task/T01")
    );
    live.forget(&stream);
    assert_eq!(
        pass.pass(&path, &stream, &f.map, Some(checkpoint))
            .unwrap()
            .0,
        Done::AtEnd
    );
    assert!(live.location(&stream).is_none());
    assert_eq!(f.changes(), changes);
}

#[test]
fn plan_observation_replacement_drops_the_previous_folder_and_branch() {
    let f = Fixture::new();
    let path = f.path();
    let stream = stream_of(CLAUDE.namespace, &path);
    let now = Utc::now();
    let live = f.live();
    let pass = f.pass(CLAUDE, Some(live.clone()));
    append(
        &path,
        &[claude(
            now - chrono::Duration::seconds(20),
            Some(&f.cwd),
            Some("task/T01"),
        )],
    );
    pass.pass(&path, &stream, &f.map, None).unwrap();
    assert_eq!(live.location(&stream).unwrap().cwd, Some(f.cwd.clone()));
    let previous = f.checkpoint(&stream);
    std::fs::write(&path, format!("{}\n", claude(now, None, None))).unwrap();
    assert_eq!(
        pass.pass(&path, &stream, &f.map, previous).unwrap().0,
        Done::Written
    );
    assert_eq!(f.checkpoint(&stream).unwrap().cursor.generation, 1);
    let location = live.location(&stream).unwrap();
    assert!(location.cwd.is_none() && location.branch.is_none());

    let previous = f.checkpoint(&stream);
    std::fs::write(&path, "{}\n").unwrap();
    pass.pass(&path, &stream, &f.map, previous).unwrap();
    assert!(
        live.location(&stream).is_none(),
        "an untimed replacement is forgotten"
    );
}

#[test]
fn plan_observation_failed_transaction_keeps_the_previous_location() {
    let f = Fixture::new();
    let path = f.path();
    let stream = stream_of(CLAUDE.namespace, &path);
    let now = Utc::now();
    let live = f.live();
    let pass = f.pass(CLAUDE, Some(live.clone()));
    append(
        &path,
        &[claude(
            now - chrono::Duration::seconds(20),
            Some(&f.cwd),
            Some("task/T01"),
        )],
    );
    pass.pass(&path, &stream, &f.map, None).unwrap();
    let before = f.stored();
    let checkpoint = f.checkpoint(&stream);
    let changed = f.dir.path().join("another-checkout");
    append(&path, &[claude(now, Some(&changed), Some("task/T02"))]);
    f.storage
        .conn
        .lock()
        .unwrap()
        .execute_batch(
            "CREATE TRIGGER reject_plan_checkpoint BEFORE INSERT ON observation_checkpoints
         BEGIN SELECT RAISE(ABORT, 'injected failure'); END;",
        )
        .unwrap();
    assert!(
        pass.pass(&path, &stream, &f.map, checkpoint.clone())
            .is_err()
    );
    assert_eq!(f.stored(), before);
    let location = live.location(&stream).unwrap();
    assert_eq!(location.cwd, Some(f.cwd));
    assert_eq!(location.branch.as_deref(), Some("task/T01"));
    f.storage
        .conn
        .lock()
        .unwrap()
        .execute_batch("DROP TRIGGER reject_plan_checkpoint")
        .unwrap();
    pass.pass(&path, &stream, &f.map, checkpoint).unwrap();
    let location = live.location(&stream).unwrap();
    assert_eq!(location.cwd, Some(changed));
    assert_eq!(location.branch.as_deref(), Some("task/T02"));
}

#[test]
fn plan_observation_rewrite_before_commit_does_not_publish_location() {
    let f = Fixture::new();
    let path = f.path();
    let stream = stream_of(CLAUDE.namespace, &path);
    let now = Utc::now();
    let live = f.live();
    let pass = f.pass(CLAUDE, Some(live.clone()));
    append(
        &path,
        &[claude(
            now - chrono::Duration::seconds(20),
            Some(&f.cwd),
            Some("task/T01"),
        )],
    );
    pass.pass(&path, &stream, &f.map, None).unwrap();
    let checkpoint = f.checkpoint(&stream);
    let changed = f.dir.path().join("another-checkout");
    append(&path, &[claude(now, Some(&changed), Some("task/T02"))]);
    *pass.before_write.lock().unwrap() = Some(Box::new(|path| {
        std::fs::write(path, "{}\n").unwrap();
    }));
    assert!(
        pass.pass(&path, &stream, &f.map, checkpoint.clone())
            .is_err()
    );
    assert_eq!(f.checkpoint(&stream), checkpoint);
    let location = live.location(&stream).unwrap();
    assert_eq!(location.cwd, Some(f.cwd));
    assert_eq!(location.branch.as_deref(), Some("task/T01"));
}
