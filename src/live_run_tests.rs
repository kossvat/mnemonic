//! The live state as a daemon runs it: its signal, its timer's waits and
//! its rebuilding from a store.
use super::*;
use crate::ingest::IngestCursor;
use crate::projects::facts::Kind;
use crate::projects::resolver::{Source, State};
use crate::projects::segments::{Learned, Segment};
use crate::storage::observation::{Batch, Checkpoint};
use chrono::NaiveDate;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Counts the signals posted, by name.
#[derive(Default)]
struct Counted {
    posted: Arc<AtomicUsize>,
    names: Arc<Mutex<Vec<String>>>,
}

impl Post for Counted {
    fn post(&self, name: &str) {
        self.posted.fetch_add(1, Ordering::SeqCst);
        self.names.lock().unwrap().push(name.to_string());
    }
}

fn live(boot: &str) -> (Arc<Live>, Arc<AtomicUsize>, Arc<Mutex<Vec<String>>>) {
    let post = Counted::default();
    let (posted, names) = (post.posted.clone(), post.names.clone());
    let live = Live::new(boot, "mnemonic.live.test".to_string(), Box::new(post));
    (live, posted, names)
}

/// Days begin at midnight of UTC.
struct Utc0;

impl Calendar for Utc0 {
    fn bounds(&self, day: NaiveDate) -> Option<(DateTime<Utc>, DateTime<Utc>)> {
        let start = day.and_hms_opt(0, 0, 0)?.and_utc();
        Some((start, start + Duration::days(1)))
    }

    fn date_of(&self, at: DateTime<Utc>) -> NaiveDate {
        at.date_naive()
    }
}

/// Days turn twelve hours before and after the instant it holds.
struct NoonAt(DateTime<Utc>);

impl NoonAt {
    /// How far a day of this calendar starts after midnight of UTC.
    fn shift(&self) -> Duration {
        self.0 - self.0.date_naive().and_hms_opt(12, 0, 0).unwrap().and_utc()
    }
}

impl Calendar for NoonAt {
    fn bounds(&self, day: NaiveDate) -> Option<(DateTime<Utc>, DateTime<Utc>)> {
        let start = day.and_hms_opt(0, 0, 0)?.and_utc() + self.shift();
        Some((start, start + Duration::days(1)))
    }

    fn date_of(&self, at: DateTime<Utc>) -> NaiveDate {
        (at - self.shift()).date_naive()
    }
}

fn session(project: Option<i64>, kind: Kind, last_at: DateTime<Utc>) -> Session {
    Session {
        agent: "claude".into(),
        program: false,
        project,
        last_kind: kind,
        last_at,
        started_at: last_at,
    }
}

#[test]
fn live_run_one_signal_at_start_one_for_a_burst_and_one_after_it() {
    let (live, posted, names) = live("boot-a");
    live.started();
    assert_eq!(posted.load(Ordering::SeqCst), 1);
    // Ten changes at once: the signal of the start was just posted, so
    // none is posted now and one is owed.
    let now = Utc::now();
    for n in 0..10 {
        live.observed(&format!("s{n}"), session(None, Kind::Call, now));
    }
    assert_eq!(posted.load(Ordering::SeqCst), 1);
    let clock = Instant::now();
    assert!(live.wait(&Utc0, now, clock).unwrap() <= crate::live_signal::EVERY);
    // The timer posts it when its time is up, once.
    live.pass(&Utc0, now, clock + crate::live_signal::EVERY);
    assert_eq!(posted.load(Ordering::SeqCst), 2);
    live.pass(&Utc0, now, clock + 2 * crate::live_signal::EVERY);
    assert_eq!(posted.load(Ordering::SeqCst), 2);
    // A change that changes nothing shown posts nothing and owes nothing.
    live.observed("s0", session(None, Kind::Call, now));
    live.pass(&Utc0, now, clock + 3 * crate::live_signal::EVERY);
    assert_eq!(posted.load(Ordering::SeqCst), 2);
    assert!(
        names
            .lock()
            .unwrap()
            .iter()
            .all(|name| name == "mnemonic.live.test")
    );
}

/// A memory saved by any process reaches a closed client without a
/// request: the daemon counts on its batch tick, and a count that differs
/// is a change of the snapshot, signalled like any other.
#[test]
fn live_run_a_new_count_of_memories_is_signalled_without_a_request() {
    use crate::event::{EventSource, MemoryEntry, MemoryType};
    let dir = crate::test_support::temp_dir("mn-live-count-");
    let storage = Storage::open(&dir.path().join("memory.db")).unwrap();
    let (live, posted, _) = live("boot-a");
    let memories = |live: &Live| {
        serde_json::from_slice::<serde_json::Value>(&live.state.snapshot().1).unwrap()["memories"]
            .clone()
    };

    live.count_memories(&storage);
    assert_eq!(memories(&live), 0);
    assert_eq!(posted.load(Ordering::SeqCst), 1);
    // The same count at the next tick: nothing changes, nothing is posted.
    live.count_memories(&storage);
    assert_eq!(posted.load(Ordering::SeqCst), 1);

    let note = MemoryEntry::new("a note", "kept", MemoryType::Note, EventSource::Manual);
    storage.save(&note).unwrap();
    live.count_memories(&storage);
    assert_eq!(memories(&live), 1);
    // A signal was just posted, so this one is owed and the timer posts it.
    let (now, clock) = (Utc::now(), Instant::now());
    live.pass(&Utc0, now, clock + crate::live_signal::EVERY);
    assert_eq!(posted.load(Ordering::SeqCst), 2);
}

#[test]
fn live_run_the_timer_waits_for_the_next_change_and_for_nothing_else() {
    let (live, _, _) = live("boot-a");
    let now = Utc::now();
    // Midnight ends a wait too. Here the days turn twelve hours from now,
    // so the waits below end at the change they are for at any hour.
    let noon = NoonAt(now);
    let clock = Instant::now() + std::time::Duration::from_secs(60);
    // Nothing is live and no day is open: the timer is not armed.
    assert_eq!(live.wait(&noon, now, clock), None);

    // A running session becomes quiet two minutes after its last record.
    live.observed("s", session(None, Kind::Call, now));
    let wait = live.wait(&noon, now, clock).unwrap();
    assert_eq!(wait.as_secs(), crate::live::RUNNING_SECS as u64);
    // Its time comes: the state moves, and the next wait is to its end.
    let later = now + Duration::seconds(crate::live::RUNNING_SECS);
    let before = live.state.version();
    live.pass(&noon, later, clock);
    assert_eq!(live.state.version(), before + 1);
    let wait = live.wait(&noon, later, clock).unwrap();
    assert_eq!(
        wait.as_secs() as i64,
        crate::live::QUIET_SECS - crate::live::RUNNING_SECS
    );
    // It leaves; with no day open the timer is at rest again.
    let gone = now + Duration::seconds(crate::live::QUIET_SECS);
    let clock = clock + std::time::Duration::from_secs(1680);
    live.pass(&noon, gone, clock);
    assert_eq!(live.wait(&noon, gone, clock), None);

    // A day whose hours are open keeps the timer armed until midnight,
    // when they start again without a record.
    let evening = NaiveDate::from_ymd_opt(2026, 9, 30)
        .unwrap()
        .and_hms_opt(23, 50, 0)
        .unwrap()
        .and_utc();
    let hours = Hours {
        day: Some(evening.date_naive()),
        owner_s: 600,
        ..Hours::default()
    };
    let (live, _, _) = self::live("boot-b");
    live.settled(live.state.set_hours(hours, evening));
    let clock = Instant::now();
    assert_eq!(live.wait(&Utc0, evening, clock).unwrap().as_secs(), 600);
    let version = live.state.version();
    live.pass(&Utc0, evening + Duration::minutes(10), clock);
    assert_eq!(live.state.version(), version + 1);
    let (_, body) = live.state.snapshot();
    let snapshot: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(snapshot["today"]["owner_s"], 0);
}

/// Write what the observation pass writes for one transcript: a segment
/// and the checkpoint of its last record.
fn observe(storage: &Storage, stream: &str, project: Option<i64>, kind: Kind, last: DateTime<Utc>) {
    let segment = Segment {
        stored: None,
        project,
        learned: Learned::By(Source::Path),
        started_at: last - Duration::minutes(7),
        last_at: last,
        last_touch_at: None,
        activity: vec![],
    };
    let next = Checkpoint {
        cursor: IngestCursor {
            stream: stream.into(),
            generation: 0,
            offset: 1,
            file_id: "f".into(),
            prefix_len: 0,
            prefix_hash: String::new(),
            anchor_hash: String::new(),
        },
        program: Some(stream.starts_with("program")),
        state: State {
            project,
            ..State::default()
        },
        last_kind: kind,
        last_at: Some(last),
    };
    let batch = Batch {
        expected: None,
        next: &next,
        replaced: false,
        agent: if stream.contains("codex") {
            "codex"
        } else {
            "claude"
        },
        segments: &[segment],
        prompts: &[],
    };
    assert!(storage.observe(&batch).unwrap());
}

#[test]
fn live_run_a_restart_has_another_tag_and_rebuilds_the_sessions_from_the_store() {
    let dir = crate::test_support::temp_dir("mn-live-run-");
    let storage = Storage::open(&dir.path().join("memory.db")).unwrap();
    let p = storage.project_upsert("demoapp", "demoapp", None).unwrap();
    let q = storage.project_upsert("notes", "notes", None).unwrap();
    let old = storage
        .project_upsert("oldnotes", "oldnotes", None)
        .unwrap();
    storage.project_merge(old, q).unwrap();
    // To the second, as the store keeps times.
    let now = DateTime::from_timestamp(Utc::now().timestamp(), 0).unwrap();
    let ago = |minutes: i64| now - Duration::minutes(minutes);
    observe(&storage, "claude-a", Some(p), Kind::Call, ago(1));
    observe(&storage, "codex-b", Some(old), Kind::TurnEnd, ago(3));
    observe(&storage, "program-c", None, Kind::Call, ago(10));
    // These left the list before the restart.
    observe(&storage, "claude-d", Some(p), Kind::TurnEnd, ago(11));
    observe(&storage, "claude-e", Some(p), Kind::Call, ago(31));

    let (first, _, _) = live("boot-a");
    let (second, posted, _) = live("boot-b");
    assert_eq!(second.rebuild(&storage, now).unwrap(), 4);
    assert_ne!(first.state.snapshot().0, second.state.snapshot().0);
    let snapshot: serde_json::Value = serde_json::from_slice(&second.state.snapshot().1).unwrap();
    let projects = snapshot["projects"].as_array().unwrap();
    assert_eq!(projects.len(), 2);
    let started = (now - Duration::minutes(8)).timestamp();
    assert_eq!(
        projects[0],
        serde_json::json!({"key": "demoapp", "name": "demoapp", "state": "running",
            "sessions": [{"agent": "claude", "driver": "owner", "state": "running",
                "started_at": started}],
            "more_sessions": 0, "today": {"owner_s": 0, "agent_s": 0}, "plan": null})
    );
    // The session of the merged project is shown under the one it was
    // merged into; it waits for the owner.
    assert_eq!(
        (projects[1]["key"].as_str(), projects[1]["state"].as_str()),
        (Some("notes"), Some("waiting"))
    );
    assert_eq!(projects[1]["sessions"][0]["agent"], "codex");
    // A session with no project, run by a program, quiet for ten minutes.
    assert_eq!(
        snapshot["unknown"]["sessions"],
        serde_json::json!([{"agent": "claude", "driver": "program", "state": "quiet",
            "started_at": (ago(10) - Duration::minutes(7)).timestamp()}])
    );
    // Rebuilding changed the snapshot, so it was signalled.
    assert!(posted.load(Ordering::SeqCst) >= 1);
}

#[test]
fn live_run_the_identity_of_the_profile_names_the_signal() {
    let one = crate::test_support::temp_dir("mn-live-run-");
    let other = crate::test_support::temp_dir("mn-live-run-");
    let first = Live::of_profile(one.path()).unwrap();
    let again = Live::of_profile(one.path()).unwrap();
    let theirs = Live::of_profile(other.path()).unwrap();
    // One profile, one name, whichever run asks; another run, another
    // tag. Two profiles post under two names, whatever port they share.
    assert_eq!(first.name, again.name);
    assert_ne!(first.name, theirs.name);
    assert_ne!(first.state.snapshot().0, again.state.snapshot().0);
    let identity = std::fs::read_to_string(one.path().join("profile.id")).unwrap();
    assert_eq!(first.name, format!("mnemonic.live.{identity}"));
}
