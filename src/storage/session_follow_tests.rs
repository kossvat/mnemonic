//! A session lasts as long as its transcript keeps growing: its last
//! activity walks the times the transcript's records were written at,
//! forward only and never across a pause as long as the idle time.
use super::*;
use chrono::{DateTime, Duration, Timelike, Utc};

/// The idle time of the tests, in seconds.
const IDLE: u64 = 1800;
const TRANSCRIPT: &str = "/sessions/work.jsonl";

struct Sessions {
    _dir: tempfile::TempDir,
    storage: Storage,
    peer: String,
}

impl Sessions {
    fn new() -> Self {
        let dir = crate::test_support::temp_dir("mn-session-follow-");
        let storage = Storage::open(&dir.path().join("memory.db")).unwrap();
        let peer = storage.upsert_peer("claude", None, "agent").unwrap();
        Self {
            _dir: dir,
            storage,
            peer,
        }
    }

    /// An open session of `source` for `transcript`, last active at `last`.
    fn open(&self, transcript: &str, source: &str, last: DateTime<Utc>) -> String {
        let id = self
            .storage
            .open_or_reuse_session_for_key(&self.peer, transcript, None, source, IDLE)
            .unwrap();
        self.set_last(&id, &last.format("%Y-%m-%d %H:%M:%S").to_string());
        id
    }

    fn set_last(&self, id: &str, last: &str) {
        let conn = self.storage.conn.lock().unwrap();
        conn.execute(
            "UPDATE sessions SET last_activity_at = ?1 WHERE id = ?2",
            params![last, id],
        )
        .unwrap();
    }

    fn last(&self, id: &str) -> DateTime<Utc> {
        let conn = self.storage.conn.lock().unwrap();
        let raw: String = conn
            .query_row(
                "SELECT last_activity_at FROM sessions WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .unwrap();
        parse_session_timestamp(&raw).expect("a readable last activity")
    }

    fn ended(&self, id: &str) -> Option<String> {
        self.storage.session_by_id(id).unwrap().unwrap().ended_at
    }

    fn follow(&self, written: &[DateTime<Utc>], now: DateTime<Utc>) -> bool {
        self.storage
            .follow_transcript(TRANSCRIPT, written, IDLE, now)
            .unwrap()
    }
}

fn whole(at: DateTime<Utc>) -> DateTime<Utc> {
    at.with_nanosecond(0).unwrap()
}

#[test]
fn session_follow_a_growing_transcript_keeps_its_session_open() {
    let s = Sessions::new();
    let now = whole(Utc::now());
    // Three hours of work with no capture: a record every twenty minutes,
    // read in batches as the watcher polls.
    let began = now - Duration::hours(3);
    let id = s.open(TRANSCRIPT, "jsonl", began);
    let written: Vec<DateTime<Utc>> = (1..=9).map(|n| began + Duration::minutes(20 * n)).collect();
    for batch in written.chunks(2) {
        assert!(s.follow(batch, now), "{batch:?}");
        assert_eq!(s.last(&id), *batch.last().unwrap());
    }
    assert_eq!(s.last(&id), now);
    // The idle sweep leaves it open, and the tracker reuses it.
    assert_eq!(s.storage.close_idle_sessions(IDLE, 0).unwrap(), 0);
    assert_eq!(s.ended(&id), None);
    let again = s
        .storage
        .open_or_reuse_session_for_key(&s.peer, TRANSCRIPT, None, "jsonl", IDLE)
        .unwrap();
    assert_eq!(again, id);
}

/// All of it at once, as a daemon that starts again reads what it missed.
#[test]
fn session_follow_a_backlog_is_walked_record_by_record() {
    let s = Sessions::new();
    let now = whole(Utc::now());
    let began = now - Duration::hours(3);
    let id = s.open(TRANSCRIPT, "jsonl", began);
    let written: Vec<DateTime<Utc>> = (1..=9).map(|n| began + Duration::minutes(20 * n)).collect();
    assert!(s.follow(&written, now));
    assert_eq!(s.last(&id), now);
}

/// Two writes two seconds apart on either side of the idle time after the
/// last activity: the first is short of it, so there was no pause.
#[test]
fn session_follow_a_step_short_of_the_idle_time_is_no_pause() {
    let s = Sessions::new();
    let now = whole(Utc::now());
    let idle = Duration::seconds(IDLE as i64);
    let last = now - Duration::minutes(40);
    let id = s.open(TRANSCRIPT, "jsonl", last);
    let before = last + idle - Duration::seconds(1);
    let after = last + idle + Duration::seconds(1);
    assert!(s.follow(&[before, after], now));
    assert_eq!(s.last(&id), after);
}

/// The transcript was quiet for the idle time or longer and then grew
/// again: the session ended where it was, and what comes next is a new
/// one, however long nobody looked.
#[test]
fn session_follow_a_pause_as_long_as_the_idle_time_ends_the_session() {
    let idle = Duration::seconds(IDLE as i64);
    for pause in [idle, idle + Duration::minutes(2), Duration::hours(14)] {
        let s = Sessions::new();
        let now = whole(Utc::now());
        let last = now - pause - Duration::minutes(10);
        let id = s.open(TRANSCRIPT, "jsonl", last);
        // Work after the pause, record after record, does not reach back.
        let resumed = last + pause;
        let written = [
            resumed,
            resumed + Duration::minutes(4),
            resumed + Duration::minutes(8),
        ];
        assert!(!s.follow(&written, now), "{pause}");
        assert_eq!(s.last(&id), last, "{pause}");
        assert_eq!(
            s.storage.close_idle_sessions(IDLE, 0).unwrap(),
            1,
            "{pause}"
        );
        let ended = parse_session_timestamp(&s.ended(&id).unwrap()).unwrap();
        assert_eq!(ended, last + idle, "{pause}");
        let next = s
            .storage
            .open_or_reuse_session_for_key(&s.peer, TRANSCRIPT, None, "jsonl", IDLE)
            .unwrap();
        assert_ne!(next, id, "{pause}");
    }
}

/// Work, a pause, work: the walk takes the first stretch and stops.
#[test]
fn session_follow_the_walk_stops_at_the_pause() {
    let s = Sessions::new();
    let now = whole(Utc::now());
    let last = now - Duration::hours(2);
    let id = s.open(TRANSCRIPT, "jsonl", last);
    let first = [last + Duration::minutes(10), last + Duration::minutes(25)];
    let second = [last + Duration::minutes(70), last + Duration::minutes(80)];
    assert!(s.follow(&[first[0], first[1], second[0], second[1]], now));
    assert_eq!(s.last(&id), first[1]);
    // Read again, as after a poll that failed: nothing moves.
    assert!(!s.follow(&[first[0], first[1], second[0], second[1]], now));
    assert_eq!(s.last(&id), first[1]);
}

#[test]
fn session_follow_times_that_say_nothing_are_passed_over() {
    let s = Sessions::new();
    let now = whole(Utc::now());
    let last = now - Duration::minutes(10);
    let id = s.open(TRANSCRIPT, "jsonl", last);
    // No record, a record at or before the last activity.
    assert!(!s.follow(&[], now));
    assert!(!s.follow(&[last, last - Duration::hours(1)], now));
    // A time ahead of the clock is no time a record was written at: it is
    // passed over at every pass, and the records around it still count.
    for ahead in [Duration::seconds(1), Duration::days(2)] {
        assert!(!s.follow(&[now + ahead], now), "{ahead}");
        assert!(!s.follow(&[now + ahead], now), "{ahead}");
    }
    let later = now + Duration::minutes(5);
    assert!(!s.follow(&[now + Duration::days(2)], later));
    assert_eq!(s.last(&id), last);
    let real = now - Duration::minutes(1);
    assert!(s.follow(&[now + Duration::days(2), real], now));
    assert_eq!(s.last(&id), real);
    // The clock itself is not ahead of the clock.
    assert!(s.follow(&[now], now));
    assert_eq!(s.last(&id), now);
}

#[test]
fn session_follow_only_the_open_session_of_the_transcript_is_followed() {
    let s = Sessions::new();
    let now = whole(Utc::now());
    let written = [now - Duration::minutes(1)];
    // No session for the transcript.
    assert!(!s.follow(&written, now));
    // Another transcript's session is not this one's.
    let last = now - Duration::minutes(10);
    let other = s.open("/sessions/other.jsonl", "jsonl", last);
    assert!(!s.follow(&written, now));
    assert_eq!(s.last(&other), last);
    // A closed session stays closed, with the end it had.
    let closed = s.open(TRANSCRIPT, "jsonl", now - Duration::hours(2));
    {
        let conn = s.storage.conn.lock().unwrap();
        conn.execute(
            "UPDATE sessions SET ended_at = datetime(last_activity_at, '+1800 seconds')
              WHERE id = ?1",
            params![closed],
        )
        .unwrap();
    }
    let ended = s.ended(&closed);
    assert!(!s.follow(&[now - Duration::minutes(100)], now));
    assert_eq!(s.ended(&closed), ended);
    // A session opened by hand is not a transcript's.
    let manual = s.open(TRANSCRIPT, "manual", last);
    assert!(!s.follow(&written, now));
    assert_eq!(s.last(&manual), last);
}

#[test]
fn session_follow_a_last_activity_in_either_format_is_read() {
    let s = Sessions::new();
    let now = whole(Utc::now());
    let last = now - Duration::minutes(10);
    let id = s.open(TRANSCRIPT, "jsonl", last);
    // The tracker writes RFC3339 when it reuses a session.
    s.set_last(&id, &last.to_rfc3339());
    let written = now - Duration::minutes(1);
    assert!(s.follow(&[written], now));
    assert_eq!(s.last(&id), written);
    // A fraction of a second is dropped: the same record moves nothing.
    assert!(!s.follow(&[written + Duration::milliseconds(700)], now));
    // A last activity that cannot be read is left alone.
    s.set_last(&id, "not a time");
    assert!(!s.follow(&[now], now));
}

/// The sweep gives a record written just inside the idle time the time to
/// be read: a session quiet for the idle time and less than the grace is
/// still open, the late record moves it, and past the grace it closes with
/// the end the idle rule gives.
#[test]
fn session_follow_the_sweep_waits_a_grace_for_a_record_not_read_yet() {
    let s = Sessions::new();
    let now = whole(Utc::now());
    let idle = Duration::seconds(IDLE as i64);
    let last = now - idle - Duration::seconds(30);
    let id = s.open(TRANSCRIPT, "jsonl", last);
    assert_eq!(s.storage.close_idle_sessions(IDLE, 60).unwrap(), 0);
    assert_eq!(s.ended(&id), None);
    // Written one second inside the idle time, read only now.
    let late = last + idle - Duration::seconds(1);
    assert!(s.follow(&[late], now));
    assert_eq!(s.last(&id), late);
    assert_eq!(s.storage.close_idle_sessions(IDLE, 60).unwrap(), 0);

    // Quiet for longer than the idle time and the grace: closed, and the
    // end is the last activity plus the idle time, not plus the grace.
    let old = now - idle - Duration::seconds(90);
    let quiet = s.open("/sessions/quiet.jsonl", "jsonl", old);
    assert_eq!(s.storage.close_idle_sessions(IDLE, 60).unwrap(), 1);
    let ended = parse_session_timestamp(&s.ended(&quiet).unwrap()).unwrap();
    assert_eq!(ended, old + idle);
    assert_eq!(s.ended(&id), None);
    // Without a grace it is the sweep it was.
    let other = s.open(
        "/sessions/plain.jsonl",
        "jsonl",
        now - idle - Duration::seconds(30),
    );
    assert_eq!(s.storage.close_idle_sessions(IDLE, 0).unwrap(), 1);
    assert!(s.ended(&other).is_some());
}
