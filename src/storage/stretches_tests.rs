//! Stretches of work: the walk over the times of records, the live
//! extension, the whole-transcript replacement, and the leaderboard.
use super::*;
use crate::event::{EventSource, MemoryType};

/// The idle time of the tests, in seconds.
const IDLE: u64 = 1800;
const TRANSCRIPT: &str = "/sessions/work.jsonl";

fn at(text: &str) -> DateTime<Utc> {
    parse_session_timestamp(text).expect("a time")
}

fn far() -> DateTime<Utc> {
    at("2030-01-01 00:00:00")
}

struct Store {
    _dir: tempfile::TempDir,
    storage: Storage,
}

impl Store {
    fn new() -> Self {
        let dir = crate::test_support::temp_dir("mn-stretches-");
        let storage = Storage::open(&dir.path().join("memory.db")).unwrap();
        Self { _dir: dir, storage }
    }

    /// The stretches of a transcript as (start, last), oldest first.
    fn of(&self, transcript: &str) -> Vec<(String, String)> {
        let conn = self.storage.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT started_at, last_at FROM work_stretches
                  WHERE transcript = ?1 ORDER BY started_at",
            )
            .unwrap();
        stmt.query_map(params![transcript], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }

    fn extend(&self, written: &[&str]) {
        let written: Vec<DateTime<Utc>> = written.iter().map(|t| at(t)).collect();
        self.storage
            .extend_work_stretches(TRANSCRIPT, &written, IDLE, far())
            .unwrap();
    }

    fn insert(&self, transcript: &str, start: &str, last: &str) -> String {
        let conn = self.storage.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO work_stretches (transcript, started_at, last_at) VALUES (?1, ?2, ?3)",
            params![transcript, start, last],
        )
        .unwrap();
        format!("stretch-{}", conn.last_insert_rowid())
    }
}

fn pair(start: &str, last: &str) -> (String, String) {
    (start.to_owned(), last.to_owned())
}

#[test]
fn work_stretch_the_walk_splits_at_a_pause_as_long_as_the_idle_time() {
    let idle = Duration::seconds(IDLE as i64);
    let times: Vec<DateTime<Utc>> = [
        "2026-09-20 18:00:00",
        "2026-09-20 18:20:00",
        "2026-09-20 18:49:59", // one second short of the idle time
        "2026-09-20 19:19:59", // the idle time exactly: a pause
        "2026-09-20 19:30:00",
        "2026-09-20 19:10:00", // out of order: adds nothing
        "2026-09-20 21:00:00", // another pause
    ]
    .iter()
    .map(|t| at(t))
    .collect();
    let stretches = walk(None, &times, idle, far());
    assert_eq!(
        stretches,
        vec![
            (at("2026-09-20 18:00:00"), at("2026-09-20 18:49:59")),
            (at("2026-09-20 19:19:59"), at("2026-09-20 19:30:00")),
            (at("2026-09-20 21:00:00"), at("2026-09-20 21:00:00")),
        ]
    );
    // From an open stretch: it comes first, as it now stands.
    let open = (at("2026-09-20 17:00:00"), at("2026-09-20 17:45:00"));
    let from_open = walk(Some(open), &times[..2], idle, far());
    assert_eq!(from_open, vec![(open.0, at("2026-09-20 18:20:00"))]);
    // Nothing read leaves the open stretch as it was, and nothing at all.
    assert_eq!(walk(Some(open), &[], idle, far()), vec![open]);
    assert!(walk(None, &[], idle, far()).is_empty());
    // A time ahead of the clock is passed over; a fraction is dropped.
    let now = at("2026-09-20 18:10:00");
    let fraction = at("2026-09-20 18:05:00") + Duration::milliseconds(700);
    assert_eq!(
        walk(None, &[fraction, at("2026-09-20 18:20:00")], idle, now),
        vec![(at("2026-09-20 18:05:00"), at("2026-09-20 18:05:00"))]
    );
}

#[test]
fn work_stretch_the_reader_extends_the_last_stretch_and_begins_new_ones() {
    let s = Store::new();
    s.extend(&["2026-09-20 18:00:00", "2026-09-20 18:10:00"]);
    assert_eq!(
        s.of(TRANSCRIPT),
        vec![pair("2026-09-20 18:00:00", "2026-09-20 18:10:00")]
    );
    // The next polls: more of the same stretch, hour after hour.
    s.extend(&["2026-09-20 18:30:00"]);
    s.extend(&["2026-09-20 18:55:00", "2026-09-20 19:20:00"]);
    assert_eq!(
        s.of(TRANSCRIPT),
        vec![pair("2026-09-20 18:00:00", "2026-09-20 19:20:00")]
    );
    // Read again after a failed poll: nothing changes.
    s.extend(&["2026-09-20 18:55:00", "2026-09-20 19:20:00"]);
    assert_eq!(s.of(TRANSCRIPT).len(), 1);
    // A pause, then work: a new stretch, and the old one keeps its end.
    s.extend(&["2026-09-20 20:00:00", "2026-09-20 20:05:00"]);
    assert_eq!(
        s.of(TRANSCRIPT),
        vec![
            pair("2026-09-20 18:00:00", "2026-09-20 19:20:00"),
            pair("2026-09-20 20:00:00", "2026-09-20 20:05:00"),
        ]
    );
    // Another transcript has stretches of its own.
    s.storage
        .extend_work_stretches(
            "/sessions/other.jsonl",
            &[at("2026-09-20 18:05:00")],
            IDLE,
            far(),
        )
        .unwrap();
    assert_eq!(s.of("/sessions/other.jsonl").len(), 1);
    assert_eq!(s.of(TRANSCRIPT).len(), 2);
    // No record, nothing.
    s.storage
        .extend_work_stretches("/sessions/empty.jsonl", &[], IDLE, far())
        .unwrap();
    assert!(s.of("/sessions/empty.jsonl").is_empty());
}

/// Another process writes while the reader extends a stretch (a backfill
/// beside the daemon). The reader waits for it: a lock taken to read and
/// then raised to write would be refused the moment the other one commits.
#[test]
fn work_stretch_the_reader_waits_for_another_writer() {
    let s = Store::new();
    s.extend(&["2026-09-20 18:00:00"]);
    let other = rusqlite::Connection::open(s._dir.path().join("memory.db")).unwrap();
    other
        .execute_batch(
            "BEGIN IMMEDIATE;
             INSERT INTO work_stretches (transcript, started_at, last_at)
             VALUES ('/sessions/other.jsonl', '2026-09-20 09:00:00', '2026-09-20 09:05:00');",
        )
        .unwrap();
    let release = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(300));
        other.execute_batch("COMMIT").unwrap();
    });
    s.extend(&["2026-09-20 18:10:00"]);
    release.join().unwrap();
    assert_eq!(
        s.of(TRANSCRIPT),
        vec![pair("2026-09-20 18:00:00", "2026-09-20 18:10:00")]
    );
}

#[test]
fn work_stretch_a_transcript_read_whole_gives_the_same_stretches() {
    let live = Store::new();
    let whole = Store::new();
    let times = [
        "2026-09-20 18:00:00",
        "2026-09-20 18:25:00",
        "2026-09-20 18:50:00",
        "2026-09-20 20:00:00",
        "2026-09-20 20:20:00",
    ];
    for batch in times.chunks(2) {
        live.extend(batch);
    }
    let all: Vec<DateTime<Utc>> = times.iter().map(|t| at(t)).collect();
    assert_eq!(
        whole
            .storage
            .bring_in_work_stretches(TRANSCRIPT, &all, IDLE, far())
            .unwrap(),
        2
    );
    assert_eq!(whole.of(TRANSCRIPT), live.of(TRANSCRIPT));
    // Read whole a second time: the same two, not four.
    whole
        .storage
        .bring_in_work_stretches(TRANSCRIPT, &all, IDLE, far())
        .unwrap();
    assert_eq!(whole.of(TRANSCRIPT), live.of(TRANSCRIPT));
    // And the reader goes on from what the whole reading left.
    whole.extend(&["2026-09-20 20:40:00"]);
    assert_eq!(
        whole.of(TRANSCRIPT)[1],
        pair("2026-09-20 20:00:00", "2026-09-20 20:40:00")
    );
    // A reading that gives no time leaves what is there.
    assert_eq!(
        whole
            .storage
            .bring_in_work_stretches(TRANSCRIPT, &[], IDLE, far())
            .unwrap(),
        2
    );
    assert_eq!(whole.of(TRANSCRIPT).len(), 2);
}

/// The transcripts that were there before are read while the reader goes
/// on. What the reader kept of records written after a reading began is
/// not in that reading, and stays.
#[test]
fn work_stretch_bringing_in_keeps_what_the_reader_wrote_meanwhile() {
    let s = Store::new();
    // The reader, during the reading: the stretch went on to 20:40, and a
    // new one began at 22:00.
    s.extend(&[
        "2026-09-20 20:30:00",
        "2026-09-20 20:40:00",
        "2026-09-20 22:00:00",
    ]);
    // The reading saw the transcript up to 20:20, from its first record.
    let seen: Vec<DateTime<Utc>> = [
        "2026-09-20 18:00:00",
        "2026-09-20 18:20:00",
        "2026-09-20 20:00:00",
        "2026-09-20 20:20:00",
    ]
    .iter()
    .map(|t| at(t))
    .collect();
    assert_eq!(
        s.storage
            .bring_in_work_stretches(TRANSCRIPT, &seen, IDLE, far())
            .unwrap(),
        3
    );
    assert_eq!(
        s.of(TRANSCRIPT),
        vec![
            pair("2026-09-20 18:00:00", "2026-09-20 18:20:00"),
            pair("2026-09-20 20:00:00", "2026-09-20 20:40:00"),
            pair("2026-09-20 22:00:00", "2026-09-20 22:00:00"),
        ]
    );
    // And the reader goes on from the last of them.
    s.extend(&["2026-09-20 22:10:00"]);
    assert_eq!(
        s.of(TRANSCRIPT)[2],
        pair("2026-09-20 22:00:00", "2026-09-20 22:10:00")
    );
}

#[test]
fn work_stretch_two_readings_join_where_they_touch() {
    let idle = Duration::seconds(IDLE as i64);
    let span = |a: &str, b: &str| (at(a), at(b));
    let out = joined(
        vec![
            span("2026-09-20 20:00:00", "2026-09-20 20:20:00"),
            span("2026-09-20 18:00:00", "2026-09-20 18:20:00"),
            // Overlaps the first, and reaches further.
            span("2026-09-20 20:10:00", "2026-09-20 20:40:00"),
            // One second short of the idle time after 20:40: the same.
            span("2026-09-20 21:09:59", "2026-09-20 21:15:00"),
            // The idle time after 21:15: another.
            span("2026-09-20 21:45:00", "2026-09-20 21:50:00"),
            // Inside one that is there already.
            span("2026-09-20 18:05:00", "2026-09-20 18:10:00"),
        ],
        idle,
    );
    assert_eq!(
        out,
        vec![
            span("2026-09-20 18:00:00", "2026-09-20 18:20:00"),
            span("2026-09-20 20:00:00", "2026-09-20 21:15:00"),
            span("2026-09-20 21:45:00", "2026-09-20 21:50:00"),
        ]
    );
    assert!(joined(vec![], idle).is_empty());
    // With no idle time at all, every record is a stretch of its own, and
    // the same reading twice is still the same stretches.
    let none = Duration::zero();
    let once = vec![
        span("2026-09-20 18:00:00", "2026-09-20 18:00:00"),
        span("2026-09-20 18:00:05", "2026-09-20 18:00:05"),
    ];
    let twice: Vec<_> = once.iter().chain(once.iter()).copied().collect();
    assert_eq!(joined(twice, none), once);
}

#[test]
fn work_stretch_bringing_in_twice_with_no_idle_time_adds_nothing() {
    let s = Store::new();
    let seen = [at("2026-09-20 18:00:00"), at("2026-09-20 18:00:05")];
    for _ in 0..2 {
        assert_eq!(
            s.storage
                .bring_in_work_stretches(TRANSCRIPT, &seen, 0, far())
                .unwrap(),
            2
        );
    }
    assert_eq!(s.of(TRANSCRIPT).len(), 2);
}

#[test]
fn work_stretch_the_mark_of_the_earlier_transcripts_is_kept() {
    let s = Store::new();
    assert!(!s.storage.work_stretches_backfilled().unwrap());
    s.storage
        .mark_work_stretches_backfilled(at("2026-10-01 10:00:00"))
        .unwrap();
    assert!(s.storage.work_stretches_backfilled().unwrap());
    // Marked again: still one mark.
    s.storage
        .mark_work_stretches_backfilled(at("2026-10-02 10:00:00"))
        .unwrap();
    assert!(s.storage.work_stretches_backfilled().unwrap());
}

/// Leaderboard: the longest stretches first, the one still going on among
/// them, and top_project = the strongest attribution-window signal. A
/// single-memory project stays under the noise floor.
#[test]
fn work_stretch_the_leaderboard_ranks_stretches_and_marks_the_live_one() {
    let s = Store::new();
    let long = s.insert(
        "/s/long.jsonl",
        "2026-07-01 10:00:00",
        "2026-07-01 13:00:00",
    );
    let mid = s.insert("/s/mid.jsonl", "2026-07-01 10:00:00", "2026-07-01 10:40:00");
    let short = s.insert(
        "/s/short.jsonl",
        "2026-07-01 10:00:00",
        "2026-07-01 10:01:30",
    );
    // Still going on: written to two minutes before now.
    let now = at("2026-07-03 12:00:00");
    let live = s.insert(
        "/s/live.jsonl",
        "2026-07-03 06:00:00",
        "2026-07-03 11:58:00",
    );
    // Unreadable times and an end before its start are left out.
    s.insert("/s/bad.jsonl", "not a time", "2026-07-01 13:00:00");
    s.insert(
        "/s/skew.jsonl",
        "2026-07-02 10:00:00",
        "2026-07-02 09:00:00",
    );

    // Project signal INSIDE the long stretch's window (11:00-11:30, also
    // outside the mid stretch's padded 09:50-10:50 window): example-app
    // gets 2 memories (clears the floor), sample-service only 1.
    let app = s
        .storage
        .upsert_entity(&crate::graph::Entity {
            name: "example-app".into(),
            entity_type: crate::graph::EntityType::Project,
        })
        .unwrap();
    let svc = s
        .storage
        .upsert_entity(&crate::graph::Entity {
            name: "sample-service".into(),
            entity_type: crate::graph::EntityType::Project,
        })
        .unwrap();
    let link = |title: &str, ts: &str, entity: &str| {
        let m =
            crate::event::MemoryEntry::new(title, "body", MemoryType::Note, EventSource::Socket);
        s.storage.save(&m).unwrap();
        s.storage.link_memory_entity(&m.id, entity).unwrap();
        let conn = s.storage.conn.lock().unwrap();
        conn.execute(
            "UPDATE memories SET timestamp = ?2 WHERE id = ?1",
            params![m.id, ts],
        )
        .unwrap();
    };
    link("work note 1", "2026-07-01T11:00:00+00:00", &app);
    link("work note 2", "2026-07-01T11:30:00+00:00", &app);
    link("side note", "2026-07-01T11:15:00+00:00", &svc);

    let rows = s.storage.longest_sessions(10, now).unwrap();
    let ids: Vec<&str> = rows.iter().map(|r| r.session_id.as_str()).collect();
    assert_eq!(
        ids,
        vec![live.as_str(), long.as_str(), mid.as_str(), short.as_str()]
    );
    let seconds: Vec<i64> = rows.iter().map(|r| r.duration_seconds).collect();
    assert_eq!(seconds, vec![5 * 3600 + 58 * 60, 3 * 3600, 40 * 60, 90]);
    let lives: Vec<bool> = rows.iter().map(|r| r.live).collect();
    assert_eq!(lives, vec![true, false, false, false]);
    assert_eq!(rows[0].started_at, "2026-07-03 06:00:00");
    assert_eq!(rows[1].top_project.as_deref(), Some("example-app"));
    assert!(rows[2].top_project.is_none(), "no project memories");
    // What is going on now, whatever its rank: the live stretch alone,
    // and with it a shorter one written to a minute ago.
    let brief = s.insert(
        "/s/brief.jsonl",
        "2026-07-03 11:50:00",
        "2026-07-03 11:59:00",
    );
    let going: Vec<String> = s
        .storage
        .current_sessions(5, now)
        .unwrap()
        .into_iter()
        .map(|r| r.session_id)
        .collect();
    assert_eq!(going, vec![live.clone(), brief.clone()]);
    assert_eq!(s.storage.current_sessions(1, now).unwrap().len(), 1);
    assert!(s.storage.current_sessions(0, now).unwrap().is_empty());
    assert!(
        s.storage
            .current_sessions(5, at("2026-07-03 13:00:00"))
            .unwrap()
            .is_empty()
    );
    // Five minutes after its last record a stretch is no longer live.
    let later = at("2026-07-03 12:03:00");
    assert!(!s.storage.longest_sessions(10, later).unwrap()[0].live);
    assert_eq!(s.storage.longest_sessions(1, now).unwrap().len(), 1);
    assert!(s.storage.longest_sessions(0, now).unwrap().is_empty());
}

/// The signal pad is clamped to the midpoint toward the neighbouring
/// stretch: a memory saved shortly AFTER stretch A ends counts toward A
/// only — stretch B starting 10 minutes later must not inherit it through
/// its own pre-pad (Codex P1, cross-session contamination).
#[test]
fn work_stretch_the_signal_pad_clamps_to_the_neighbour_midpoint() {
    let s = Store::new();
    // A 10:00-11:00, B 11:10-11:20 — the gap's midpoint is 11:05.
    let a = s.insert("/s/a.jsonl", "2026-07-01 10:00:00", "2026-07-01 11:00:00");
    let b = s.insert("/s/b.jsonl", "2026-07-01 11:10:00", "2026-07-01 11:20:00");
    // A row that cannot be a stretch (it ends before it starts) sits in
    // the gap. Were it kept among the neighbours, its 11:03 start would
    // clamp A's post-pad to 11:01:30 and steal the memories below.
    s.insert(
        "/s/ghost.jsonl",
        "2026-07-01 11:03:00",
        "2026-07-01 09:00:00",
    );
    let proj = s
        .storage
        .upsert_entity(&crate::graph::Entity {
            name: "gapproj".into(),
            entity_type: crate::graph::EntityType::Project,
        })
        .unwrap();
    // Two project memories land 1-2 minutes after A ends: inside A's
    // clamped post-pad (up to 11:05), OUTSIDE B's clamped pre-pad.
    for (title, ts) in [
        ("wrapup note 1", "2026-07-01T11:01:00+00:00"),
        ("wrapup note 2", "2026-07-01T11:02:00+00:00"),
    ] {
        let m =
            crate::event::MemoryEntry::new(title, "body", MemoryType::Note, EventSource::Socket);
        s.storage.save(&m).unwrap();
        s.storage.link_memory_entity(&m.id, &proj).unwrap();
        let conn = s.storage.conn.lock().unwrap();
        conn.execute(
            "UPDATE memories SET timestamp = ?2 WHERE id = ?1",
            params![m.id, ts],
        )
        .unwrap();
    }
    let rows = s.storage.longest_sessions(10, far()).unwrap();
    let row_a = rows.iter().find(|r| r.session_id == a).unwrap();
    let row_b = rows.iter().find(|r| r.session_id == b).unwrap();
    assert_eq!(
        row_a.top_project.as_deref(),
        Some("gapproj"),
        "the closer stretch owns the wrap-up memories"
    );
    assert!(
        row_b.top_project.is_none(),
        "the later stretch must not inherit its neighbour's signal"
    );
}
