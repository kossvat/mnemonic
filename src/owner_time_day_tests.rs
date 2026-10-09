//! Days of the owner's time over real stores, as expected rows to the
//! second. Every name here is invented.
use super::*;
use crate::graph::{Entity, EntityType};
use crate::ingest::IngestCursor;
use crate::projects::facts::Kind;
use crate::projects::resolver::{Source, State};
use crate::projects::segments::{Learned, Segment};
use crate::storage::observation::{Batch, Checkpoint};
use chrono::{NaiveTime, TimeZone};
use std::sync::Arc;

/// A calendar whose days begin `offset` seconds after those of UTC, with
/// the offset `later` from the instant `turn` on: a clock changed once.
struct Zone {
    offset: i64,
    turn: Option<(DateTime<Utc>, i64)>,
}

impl Zone {
    const UTC: Zone = Zone {
        offset: 0,
        turn: None,
    };

    fn fixed(offset_hours: i64) -> Self {
        Zone {
            offset: offset_hours * 3600,
            turn: None,
        }
    }

    fn offset_at(&self, at: DateTime<Utc>) -> i64 {
        match self.turn {
            Some((turn, later)) if at >= turn => later,
            _ => self.offset,
        }
    }

    /// The instant a local date begins.
    fn start(&self, day: NaiveDate) -> DateTime<Utc> {
        let local = day.and_time(NaiveTime::MIN).and_utc();
        let offsets = [Some(self.offset), self.turn.map(|(_, later)| later)];
        offsets
            .into_iter()
            .flatten()
            .map(|offset| local - Duration::seconds(offset))
            .find(|at| self.date_of(*at) == day && self.date_of(*at - Duration::seconds(1)) != day)
            .expect("a local midnight")
    }
}

impl Calendar for Zone {
    fn bounds(&self, day: NaiveDate) -> Option<(DateTime<Utc>, DateTime<Utc>)> {
        Some((self.start(day), self.start(day.succ_opt()?)))
    }

    fn date_of(&self, at: DateTime<Utc>) -> NaiveDate {
        (at + Duration::seconds(self.offset_at(at))).date_naive()
    }
}

fn day(number: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 9, number).unwrap()
}

/// An instant of September 2026, in UTC.
fn at(day: u32, hour: u32, minute: u32, second: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, day, hour, minute, second)
        .unwrap()
}

struct Fixture {
    _dir: tempfile::TempDir,
    storage: Arc<Storage>,
    activity: ActivityStore,
    activity_path: std::path::PathBuf,
    p: i64,
    q: i64,
}

impl Fixture {
    fn new() -> Self {
        let dir = crate::test_support::temp_dir("mn-owner-time-");
        let storage = Arc::new(Storage::open(&dir.path().join("memory.db")).unwrap());
        let activity_path = dir.path().join("activity.db");
        let activity = ActivityStore::open(&activity_path).unwrap();
        let p = storage.project_upsert("demoapp", "demoapp", None).unwrap();
        let q = storage.project_upsert("notes", "notes", None).unwrap();
        Self {
            _dir: dir,
            storage,
            activity,
            activity_path,
            p,
            q,
        }
    }

    /// The owner was at the Mac between two instants.
    fn active(&self, from: DateTime<Utc>, to: DateTime<Utc>) {
        let conn = rusqlite::Connection::open(&self.activity_path).unwrap();
        conn.execute(
            "INSERT INTO work_sessions (id, started_at, ended_at, open) VALUES (?1, ?2, ?3, 0)",
            rusqlite::params![
                uuid::Uuid::new_v4().to_string(),
                from.to_rfc3339(),
                to.to_rfc3339()
            ],
        )
        .unwrap();
    }

    /// A transcript on one project between two instants, written the way
    /// the observation pass writes it: a record in each of `records`
    /// (instants) that names the project, a prompt at each of `prompts`.
    fn session(
        &self,
        stream: &str,
        program: bool,
        project: Option<i64>,
        (from, to): (DateTime<Utc>, DateTime<Utc>),
        records: &[DateTime<Utc>],
        prompts: &[DateTime<Utc>],
    ) {
        let segment = Segment {
            stored: None,
            project,
            learned: Learned::By(Source::Path),
            started_at: from,
            last_at: to,
            last_touch_at: records.last().copied(),
            activity: records
                .iter()
                .map(|at| (at.timestamp().div_euclid(60), 1, 1))
                .collect(),
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
            program: Some(program),
            state: State::default(),
            last_kind: Kind::Other,
            last_at: Some(to),
        };
        let expected = self.storage.observation_checkpoint(stream).unwrap();
        let written = self
            .storage
            .observe(&Batch {
                expected: expected.as_ref(),
                next: &next,
                replaced: false,
                agent: "claude",
                segments: &[segment],
                prompts,
            })
            .unwrap();
        assert!(written);
    }

    fn key(&self, project: i64) -> String {
        self.storage.attribution_keys().unwrap()[&project].clone()
    }

    /// The rows of a day as (key, whole seconds, confidence), by key, and
    /// the seconds given to nobody.
    fn rows(&self, day: NaiveDate) -> (Vec<(String, i64, String)>, i64) {
        let (mut rows, unattributed) = self.activity.project_attribution_for_day(day).unwrap();
        rows.sort_by(|a, b| a.0.cmp(&b.0));
        let rows = rows
            .into_iter()
            .map(|(key, seconds, confidence)| {
                assert!((seconds - seconds.round()).abs() < 1e-6, "{seconds}");
                (key, seconds.round() as i64, confidence.unwrap_or_default())
            })
            .collect();
        (rows, unattributed.round() as i64)
    }

    fn make(&self, calendar: &dyn Calendar, day: NaiveDate) -> Day {
        recompute_day(
            &self.activity,
            &self.storage,
            calendar,
            day,
            &AttribCfg::default(),
        )
        .unwrap()
        .unwrap()
    }

    fn due(
        &self,
        calendar: &dyn Calendar,
        now: DateTime<Utc>,
        seen: &mut Option<NaiveDate>,
    ) -> Vec<NaiveDate> {
        recompute_due(
            &self.activity,
            &self.storage,
            calendar,
            now,
            seen,
            &AttribCfg::default(),
        )
        .unwrap()
    }

    /// A memory written at `at`, tied to the entity `name`.
    fn memory(&self, id: &str, at: DateTime<Utc>, name: &str) {
        let entity = self
            .storage
            .upsert_entity(&Entity {
                name: name.into(),
                entity_type: EntityType::Project,
            })
            .unwrap();
        let conn = self.storage.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO memories (id, timestamp, title, content, memory_type, source)
             VALUES (?1, ?2, 'a note', 'a note', 'note', 'test')",
            rusqlite::params![id, at.to_rfc3339()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO memory_entities (memory_id, entity_id) VALUES (?1, ?2)",
            rusqlite::params![id, entity],
        )
        .unwrap();
    }
}

#[test]
fn owner_time_day_a_short_session_and_fragments_either_side_of_midnight() {
    let f = Fixture::new();
    // One transcript on the project across midnight, prompted before it.
    f.session(
        "s",
        false,
        Some(f.p),
        (at(29, 22, 0, 0), at(30, 0, 30, 0)),
        &[at(29, 22, 0, 0), at(29, 23, 55, 0), at(30, 0, 30, 0)],
        &[at(29, 22, 0, 0), at(29, 23, 55, 0)],
    );
    // A session of 37 seconds, and one that crosses midnight: each of its
    // parts is under a minute, and both count.
    f.active(at(29, 22, 1, 3), at(29, 22, 1, 40));
    f.active(at(29, 23, 59, 40), at(30, 0, 0, 25));
    f.make(&Zone::UTC, day(29));
    f.make(&Zone::UTC, day(30));
    let key = f.key(f.p);
    assert_eq!(f.rows(day(29)), (vec![(key.clone(), 57, "high".into())], 0));
    assert_eq!(f.rows(day(30)), (vec![(key, 25, "high".into())], 0));
    // What the day shows as its total is what was split.
    for (date, total) in [(day(29), 57.0), (day(30), 25.0)] {
        let (from, to) = Zone::UTC.bounds(date).unwrap();
        let worked: f64 = f
            .activity
            .intervals_between(from, to)
            .unwrap()
            .iter()
            .map(|(start, end)| (*end - *start).num_milliseconds() as f64 / 1000.0)
            .sum();
        assert_eq!(worked, total);
    }
}

#[test]
fn owner_time_day_a_transcript_a_program_ran_takes_none_of_the_owners_time() {
    let f = Fixture::new();
    // The owner's transcript is inside a stretch and no more; the one a
    // program ran wrote a record a minute ago.
    let stretch = (at(29, 10, 0, 0), at(29, 11, 0, 0));
    f.session("owner", false, Some(f.p), stretch, &[at(29, 10, 0, 0)], &[]);
    let lately = [at(29, 10, 29, 0), at(29, 10, 30, 0)];
    f.session("program", true, Some(f.q), stretch, &lately, &[]);
    f.active(at(29, 10, 30, 0), at(29, 10, 40, 0));
    f.make(&Zone::UTC, day(29));
    assert_eq!(
        f.rows(day(29)),
        (vec![(f.key(f.p), 600, "medium".into())], 0)
    );
}

#[test]
fn owner_time_day_two_sessions_on_one_project_and_one_on_another_are_two_shares() {
    let f = Fixture::new();
    let stretch = (at(29, 10, 0, 0), at(29, 11, 0, 0));
    for stream in ["one", "two"] {
        f.session(stream, false, Some(f.p), stretch, &[at(29, 10, 0, 0)], &[]);
    }
    f.session("three", false, Some(f.q), stretch, &[at(29, 10, 0, 0)], &[]);
    f.active(at(29, 10, 20, 0), at(29, 10, 30, 0));
    f.make(&Zone::UTC, day(29));
    let mut expected = vec![
        (f.key(f.p), 300, "medium".to_string()),
        (f.key(f.q), 300, "medium".to_string()),
    ];
    expected.sort();
    assert_eq!(f.rows(day(29)), (expected, 0));
}

#[test]
fn owner_time_day_rows_are_keyed_by_the_entity_of_the_project_as_merges_resolve_it() {
    let f = Fixture::new();
    let entity = f
        .storage
        .upsert_entity(&Entity {
            name: "demoapp".into(),
            entity_type: EntityType::Concept,
        })
        .unwrap();
    assert!(f.storage.project_link_entity(f.p, "demoapp").unwrap());
    let old = f.storage.project_upsert("oldapp", "oldapp", None).unwrap();
    f.storage.project_merge(old, f.p).unwrap();
    let keys = f.storage.attribution_keys().unwrap();
    assert_eq!(keys[&f.p], entity);
    // A project no entity carries the name of is keyed by its place.
    assert_eq!(keys[&f.q], format!("project:{}", f.q));
    assert!(!keys.contains_key(&old));

    // A segment written under the merged project counts under the one it
    // was merged into; the two were live at once and are one share.
    let stretch = (at(29, 10, 0, 0), at(29, 11, 0, 0));
    f.session("a", false, Some(old), stretch, &[at(29, 10, 0, 0)], &[]);
    f.session("b", false, Some(f.p), stretch, &[at(29, 10, 0, 0)], &[]);
    f.session("c", false, Some(f.q), stretch, &[at(29, 10, 0, 0)], &[]);
    f.active(at(29, 10, 20, 0), at(29, 10, 22, 0));
    f.make(&Zone::UTC, day(29));
    let mut expected = vec![
        (entity, 60, "medium".to_string()),
        (format!("project:{}", f.q), 60, "medium".to_string()),
    ];
    expected.sort();
    assert_eq!(f.rows(day(29)), (expected, 0));
}

#[test]
fn owner_time_day_with_no_segment_falls_back_to_the_memories_of_the_registry() {
    let f = Fixture::new();
    // The rule is in force: a segment exists, days before.
    let earlier = (at(20, 10, 0, 0), at(20, 10, 5, 0));
    f.session("old", false, Some(f.q), earlier, &[at(20, 10, 0, 0)], &[]);
    // A day of work with no transcript; memories of two entities were
    // written in it. One is the entity of a project of the registry.
    f.active(at(29, 10, 0, 0), at(29, 10, 20, 0));
    f.memory("m1", at(29, 10, 5, 0), "demoapp");
    f.memory("m2", at(29, 10, 6, 0), "outsider");
    f.memory("m3", at(29, 10, 7, 0), "outsider");
    f.make(&Zone::UTC, day(29));
    // No row of the registry carries that entity yet: nobody gets time.
    assert_eq!(f.rows(day(29)), (vec![], 1200));

    assert!(f.storage.project_link_entity(f.p, "demoapp").unwrap());
    f.make(&Zone::UTC, day(29));
    // The entity outside the registry gets none, however many memories.
    assert_eq!(f.rows(day(29)), (vec![(f.key(f.p), 1200, "low".into())], 0));
}

#[test]
fn owner_time_due_a_late_record_marks_its_day_and_the_day_is_made_again() {
    let f = Fixture::new();
    let now = at(29, 12, 0, 0);
    f.active(at(26, 9, 0, 0), at(26, 9, 10, 0));
    f.active(at(29, 9, 0, 0), at(29, 9, 10, 0));
    f.session(
        "today",
        false,
        Some(f.p),
        (at(29, 9, 0, 0), at(29, 9, 30, 0)),
        &[at(29, 9, 0, 0)],
        &[at(29, 9, 0, 0)],
    );
    let mut seen = None;
    assert_eq!(f.due(&Zone::UTC, now, &mut seen), [day(29)]);
    assert_eq!(seen, Some(day(29)));
    assert_eq!(f.rows(day(29)), (vec![(f.key(f.p), 600, "high".into())], 0));
    assert!(f.storage.attribution_dirty().unwrap().is_empty());
    // Nothing new: only today is made.
    assert_eq!(f.due(&Zone::UTC, now, &mut seen), [day(29)]);

    // A transcript of three days ago is read only now.
    f.session(
        "late",
        false,
        Some(f.q),
        (at(26, 9, 0, 0), at(26, 9, 30, 0)),
        &[at(26, 9, 0, 0)],
        &[at(26, 9, 0, 0)],
    );
    assert_eq!(f.storage.attribution_dirty().unwrap().len(), 1);
    assert_eq!(f.due(&Zone::UTC, now, &mut seen), [day(26), day(29)]);
    assert_eq!(f.rows(day(26)), (vec![(f.key(f.q), 600, "high".into())], 0));
    assert!(f.storage.attribution_dirty().unwrap().is_empty());
}

#[test]
fn owner_time_due_yesterday_is_finished_after_midnight_without_a_new_record() {
    let f = Fixture::new();
    f.session(
        "s",
        false,
        Some(f.p),
        (at(28, 23, 0, 0), at(28, 23, 59, 0)),
        &[at(28, 23, 0, 0)],
        &[at(28, 23, 50, 0)],
    );
    let mut seen = None;
    f.active(at(28, 23, 50, 0), at(28, 23, 52, 0));
    assert_eq!(f.due(&Zone::UTC, at(28, 23, 52, 0), &mut seen), [day(28)]);
    assert_eq!(f.rows(day(28)).0[0].1, 120);
    // The owner worked on to the end of the day; no record was written.
    f.active(at(28, 23, 52, 0), at(28, 23, 58, 0));
    assert_eq!(
        f.due(&Zone::UTC, at(29, 0, 2, 0), &mut seen),
        [day(28), day(29)]
    );
    assert_eq!(f.rows(day(28)).0[0].1, 480);
    // Once: the next pass makes today alone.
    assert_eq!(f.due(&Zone::UTC, at(29, 0, 12, 0), &mut seen), [day(29)]);
}

#[test]
fn owner_time_due_days_before_the_first_segment_keep_their_rows() {
    let f = Fixture::new();
    f.active(at(10, 9, 0, 0), at(10, 9, 10, 0));
    f.activity
        .write_day(day(10), &[("an earlier key", 600.0, "low")], 0.0)
        .unwrap();
    let mut seen = None;
    // No segment at all: nothing is made, today included.
    f.storage
        .attribution_mark_dirty(at(10, 0, 0, 0), at(10, 12, 0, 0))
        .unwrap();
    assert!(f.due(&Zone::UTC, at(29, 12, 0, 0), &mut seen).is_empty());
    // The first segment is of day 27: day 10 is before the rule.
    let first = (at(27, 9, 0, 0), at(27, 9, 5, 0));
    f.session("s", false, Some(f.p), first, &[at(27, 9, 0, 0)], &[]);
    f.storage
        .attribution_mark_dirty(at(10, 0, 0, 0), at(10, 12, 0, 0))
        .unwrap();
    assert_eq!(
        f.due(&Zone::UTC, at(29, 12, 0, 0), &mut seen),
        [day(27), day(29)]
    );
    assert_eq!(
        f.rows(day(10)),
        (vec![("an earlier key".into(), 600, "low".into())], 0)
    );
    assert_eq!(
        days_of_the_rule(&f.storage, &Zone::UTC, day(1), day(29))
            .unwrap()
            .first(),
        Some(&day(27))
    );
}

#[test]
fn owner_time_the_days_of_a_changed_clock_add_up() {
    // The clock goes forward an hour at 07:00 UTC of day 27, and in a
    // second calendar back an hour: a day of 23 hours and one of 25.
    for (before, after, hours) in [(-5, -4, 23), (-4, -5, 25)] {
        let zone = Zone {
            offset: before * 3600,
            turn: Some((at(27, 7, 0, 0), after * 3600)),
        };
        let (from, to) = zone.bounds(day(27)).unwrap();
        assert_eq!((to - from).num_hours(), hours);
        assert_eq!(zone.bounds(day(26)).unwrap().1, from);
        assert_eq!(zone.bounds(day(28)).unwrap().0, to);

        let f = Fixture::new();
        // One transcript through the three days, prompted every ten
        // minutes; the owner is at the Mac across both ends of the day.
        let span = (at(26, 0, 0, 0), at(29, 0, 0, 0));
        let prompts: Vec<DateTime<Utc>> = (0..432)
            .map(|step| span.0 + Duration::minutes(10 * step))
            .collect();
        f.session("s", false, Some(f.p), span, &prompts, &prompts);
        f.active(from - Duration::minutes(30), from + Duration::minutes(45));
        f.active(to - Duration::seconds(50), to + Duration::seconds(70));
        f.active(at(27, 6, 30, 0), at(27, 7, 30, 0));
        let total: i64 = [day(26), day(27), day(28)]
            .into_iter()
            .map(|date| {
                f.make(&zone, date);
                let (rows, unattributed) = f.rows(date);
                rows.iter().map(|row| row.1).sum::<i64>() + unattributed
            })
            .sum();
        assert_eq!(total, 75 * 60 + 120 + 3600);
        assert_eq!(f.rows(day(26)).0[0].1, 30 * 60);
        assert_eq!(f.rows(day(27)).0[0].1, 45 * 60 + 3600 + 50);
        assert_eq!(f.rows(day(28)).0[0].1, 70);
    }
}

#[test]
fn owner_time_a_clock_that_was_set_marks_the_days_it_touched() {
    let minute = std::time::Duration::from_secs(60);
    let before = at(28, 12, 0, 0);
    // The clocks agree, within the tolerance.
    assert_eq!(clock_moved(before, minute, at(28, 12, 1, 0)), None);
    assert_eq!(clock_moved(before, minute, at(28, 12, 2, 30)), None);
    // Set forward (or the Mac slept): the time passed over.
    assert_eq!(
        clock_moved(before, minute, at(29, 3, 0, 0)),
        Some((before, at(29, 3, 0, 0)))
    );
    // Set back two days: the days between the two readings.
    assert_eq!(
        clock_moved(before, minute, at(26, 12, 1, 0)),
        Some((at(26, 12, 1, 0), at(28, 12, 1, 0)))
    );

    let f = Fixture::new();
    let span = (at(25, 0, 0, 0), at(28, 22, 0, 0));
    f.session("s", false, Some(f.p), span, &[span.0], &[]);
    for date in [26, 27, 28] {
        f.active(at(date, 9, 0, 0), at(date, 9, 10, 0));
    }
    let mut seen = None;
    f.due(&Zone::UTC, at(28, 12, 0, 0), &mut seen);
    assert!(f.storage.attribution_dirty().unwrap().is_empty());
    // Rows that no longer hold: only a mark gets them made again.
    for date in [26, 27, 28] {
        f.activity.write_day(day(date), &[], 1.0).unwrap();
    }
    let (from, to) = clock_moved(before, minute, at(26, 12, 1, 0)).unwrap();
    f.storage.attribution_mark_dirty(from, to).unwrap();
    // The clock now reads day 26: the days after it wait for their turn,
    // and so do their marks.
    assert_eq!(f.due(&Zone::UTC, at(26, 12, 1, 0), &mut seen), [day(26)]);
    assert_eq!(f.rows(day(26)).0[0].1, 600);
    assert_eq!(f.rows(day(27)), (vec![], 1));
    assert_eq!(f.storage.attribution_dirty().unwrap().len(), 2);
    // Set forward again: every day it passed over is made, and adds up.
    let (from, to) = clock_moved(at(26, 12, 1, 0), minute, at(28, 12, 3, 0)).unwrap();
    f.storage.attribution_mark_dirty(from, to).unwrap();
    assert_eq!(
        f.due(&Zone::UTC, at(28, 12, 3, 0), &mut seen),
        [day(26), day(27), day(28)]
    );
    assert!(f.storage.attribution_dirty().unwrap().is_empty());
    for date in [26, 27, 28] {
        let (rows, unattributed) = f.rows(day(date));
        assert_eq!(
            rows.iter().map(|row| row.1).sum::<i64>() + unattributed,
            600
        );
    }
}

#[test]
fn owner_time_a_changed_timezone_makes_the_days_again_and_they_add_up() {
    let f = Fixture::new();
    let span = (at(27, 0, 0, 0), at(29, 0, 0, 0));
    let prompts: Vec<DateTime<Utc>> = (0..288)
        .map(|step| span.0 + Duration::minutes(10 * step))
        .collect();
    f.session("s", false, Some(f.p), span, &prompts, &prompts);
    // Two hours of work around midnight UTC.
    f.active(at(27, 23, 0, 0), at(28, 1, 0, 0));
    let mut seen = None;
    let now = at(28, 12, 0, 0);
    assert_eq!(f.due(&Zone::UTC, now, &mut seen), [day(27), day(28)]);
    assert_eq!(
        (f.rows(day(27)).0[0].1, f.rows(day(28)).0[0].1),
        (3600, 3600)
    );

    // The owner flies four hours west: the days in progress are marked,
    // and made again by the bounds of the new zone.
    f.storage
        .attribution_mark_dirty(now - Duration::days(1), now)
        .unwrap();
    // A day of UTC lies on two dates there: the evening before counts.
    let west = Zone::fixed(-4);
    assert_eq!(f.due(&west, now, &mut seen), [day(26), day(27), day(28)]);
    // All of the work was before midnight there.
    assert_eq!(f.rows(day(26)), (vec![], 0));
    assert_eq!(f.rows(day(27)).0[0].1, 7200);
    assert_eq!(f.rows(day(28)), (vec![], 0));
}

/// Two passes read one mark. The first clears it, a late record marks the
/// day again, and the second clears what it read: the later mark is not
/// the one it read, and stays.
#[test]
fn owner_time_a_day_marked_again_after_a_clear_is_not_cleared_by_an_earlier_reader() {
    let f = Fixture::new();
    let noon = at(27, 12, 0, 0);
    f.storage.attribution_mark_dirty(noon, noon).unwrap();
    let read_by_both = f.storage.attribution_dirty().unwrap();
    f.storage.attribution_dirty_clear(&read_by_both).unwrap();
    f.storage.attribution_mark_dirty(noon, noon).unwrap();
    f.storage.attribution_dirty_clear(&read_by_both).unwrap();
    assert_eq!(f.storage.attribution_dirty().unwrap().len(), 1);
}

/// A merge and its undo change which project the kept segments count for:
/// the days of the merged project's segments are made again both times,
/// so a day made while the merge stood does not keep its hours under the
/// other project.
#[test]
fn owner_time_a_merge_and_its_undo_mark_the_days_of_the_merged_projects_segments() {
    use chrono::Datelike;
    let f = Fixture::new();
    let marked = |f: &Fixture| -> Vec<u32> {
        f.storage
            .attribution_dirty()
            .unwrap()
            .iter()
            .map(|(start, _)| start.day())
            .collect()
    };
    let clear = |f: &Fixture| {
        f.storage
            .attribution_dirty_clear(&f.storage.attribution_dirty().unwrap())
            .unwrap()
    };
    let (first, second) = (
        (at(24, 10, 0, 0), at(24, 11, 0, 0)),
        (at(26, 10, 0, 0), at(26, 11, 0, 0)),
    );
    f.session("a", false, Some(f.p), first, &[first.0], &[]);
    f.session("b", false, Some(f.p), second, &[second.0], &[]);
    f.session(
        "c",
        false,
        Some(f.q),
        (at(28, 10, 0, 0), at(28, 11, 0, 0)),
        &[],
        &[],
    );
    clear(&f);

    f.storage.project_merge(f.p, f.q).unwrap();
    assert_eq!(marked(&f), [24, 25, 26]);
    clear(&f);
    assert!(f.storage.project_unmerge(f.p).unwrap());
    assert_eq!(marked(&f), [24, 25, 26]);
}

#[test]
fn owner_time_a_write_marks_the_days_of_what_it_wrote_and_of_what_it_dropped() {
    let f = Fixture::new();
    let marked = |f: &Fixture| -> Vec<u32> {
        use chrono::Datelike;
        f.storage
            .attribution_dirty()
            .unwrap()
            .iter()
            .map(|(start, _)| start.day())
            .collect()
    };
    // A segment across midnight marks both days; a prompt its own.
    let span = (at(27, 23, 0, 0), at(28, 1, 0, 0));
    f.session("s", false, Some(f.p), span, &[span.0], &[at(26, 10, 0, 0)]);
    assert_eq!(marked(&f), [26, 27, 28]);
    let seen = f.storage.attribution_dirty().unwrap();
    // Marked again after it was read: the mark stays when the read ones
    // are cleared.
    f.storage
        .attribution_mark_dirty(at(27, 5, 0, 0), at(27, 5, 0, 0))
        .unwrap();
    f.storage.attribution_dirty_clear(&seen).unwrap();
    assert_eq!(marked(&f), [27]);
    f.storage
        .attribution_dirty_clear(&f.storage.attribution_dirty().unwrap())
        .unwrap();

    // A transcript that was replaced drops its segments: their days are
    // marked with those of what replaces them.
    let expected = f.storage.observation_checkpoint("s").unwrap();
    let mut next = expected.clone().unwrap();
    next.cursor.generation = 1;
    let replacement = Segment {
        stored: None,
        project: Some(f.q),
        learned: Learned::By(Source::Path),
        started_at: at(20, 9, 0, 0),
        last_at: at(20, 9, 5, 0),
        last_touch_at: None,
        activity: vec![],
    };
    assert!(
        f.storage
            .observe(&Batch {
                expected: expected.as_ref(),
                next: &next,
                replaced: true,
                agent: "claude",
                segments: &[replacement],
                prompts: &[],
            })
            .unwrap()
    );
    assert_eq!(marked(&f), [20, 26, 27, 28]);
}

#[test]
fn owner_time_a_project_no_entity_names_yet_is_shown_with_its_time() {
    let f = Fixture::new();
    let stretch = (at(29, 10, 0, 0), at(29, 11, 0, 0));
    f.session("s", false, Some(f.q), stretch, &[at(29, 10, 0, 0)], &[]);
    f.active(at(29, 10, 20, 0), at(29, 10, 22, 0));
    f.make(&Zone::UTC, day(29));
    let key = f.key(f.q);
    assert_eq!(crate::owner_time::registry_id(&key), Some(f.q));
    // What reads the rows by key finds the project, by its name.
    let shown = f.storage.project_overview_by_id(&key, 3).unwrap().unwrap();
    assert_eq!((shown.name.as_str(), shown.mem_count), ("notes", 0));
    assert!(
        f.storage
            .project_overview_by_id("project:9999", 3)
            .unwrap()
            .is_none()
    );
    assert_eq!(crate::owner_time::registry_id("an entity id"), None);
}

/// One transcript on the first project from 10:00 to 11:00, and the owner
/// at the Mac from 10:20 to 10:30.
fn corrected() -> Fixture {
    let f = Fixture::new();
    let stretch = (at(29, 10, 0, 0), at(29, 11, 0, 0));
    f.session("s", false, Some(f.p), stretch, &[at(29, 10, 0, 0)], &[]);
    f.active(at(29, 10, 20, 0), at(29, 10, 30, 0));
    f
}

/// The rows of the day of `corrected`, made now.
fn made(f: &Fixture) -> (Vec<(String, i64, String)>, i64) {
    f.make(&Zone::UTC, day(29));
    f.rows(day(29))
}

#[test]
fn override_a_pin_gives_the_time_of_its_transcript_to_the_project_named() {
    let f = corrected();
    assert_eq!(made(&f), (vec![(f.key(f.p), 600, "medium".into())], 0));
    let pin = f.storage.override_add(Some("s"), None, Some(f.q)).unwrap();
    // The owner's word is as sure as anything is.
    assert_eq!(made(&f), (vec![(f.key(f.q), 600, "high".into())], 0));
    // A later pin replaces it; "no project" takes the transcript out of
    // every tier, and nothing else was live.
    f.storage.override_add(Some("s"), None, None).unwrap();
    assert_eq!(made(&f), (vec![], 600));
    // The segments are as the rules wrote them.
    let (transcripts, _) = f
        .storage
        .owner_transcripts(at(29, 0, 0, 0), at(30, 0, 0, 0))
        .unwrap();
    assert_eq!(transcripts[0].spans[0].project, Some(f.p));
    assert_eq!(transcripts[0].pin, Some(None));
    assert!(f.storage.override_remove(pin).unwrap());
}

#[test]
fn override_a_range_wins_over_a_pin_and_the_later_of_two_ranges_wins() {
    let f = corrected();
    let r = f.storage.project_upsert("third", "third", None).unwrap();
    f.storage.override_add(Some("s"), None, Some(f.q)).unwrap();
    let range = |from: u32, to: u32| Some((at(29, 10, from, 0), at(29, 10, to, 0)));
    f.storage
        .override_add(Some("s"), range(22, 26), Some(r))
        .unwrap();
    f.storage
        .override_add(Some("s"), range(24, 28), Some(f.p))
        .unwrap();
    // 10:20 the pin, 10:22 the first range, 10:24 the later one, 10:28
    // the pin again.
    let mut expected = vec![
        (f.key(f.q), 240, "high".to_string()),
        (f.key(r), 120, "high".to_string()),
        (f.key(f.p), 240, "high".to_string()),
    ];
    expected.sort();
    assert_eq!(made(&f), (expected, 0));
}

#[test]
fn override_a_range_of_the_owners_own_time_names_work_that_left_no_transcript() {
    let f = corrected();
    // Half an hour with no transcript, and a memory of another project
    // written in it: the fallback would give it that project.
    f.active(at(29, 14, 0, 0), at(29, 14, 30, 0));
    assert!(
        f.storage
            .upsert_entity(&Entity {
                name: "notes".into(),
                entity_type: EntityType::Project,
            })
            .is_ok()
    );
    assert!(f.storage.project_link_entity(f.q, "notes").unwrap());
    f.memory("m1", at(29, 14, 25, 0), "notes");
    let (p, q) = (f.key(f.p), f.key(f.q));
    let mut expected = vec![
        (p.clone(), 600, "medium".to_string()),
        (q.clone(), 1800, "low".to_string()),
    ];
    expected.sort();
    assert_eq!(made(&f), (expected, 0));

    // Ten minutes of it were the first project; ten more were no project
    // at all, which is nobody's, not the fallback's.
    let range = |hour: u32, from: u32, to: u32| Some((at(29, hour, from, 0), at(29, hour, to, 0)));
    f.storage
        .override_add(None, range(14, 0, 10), Some(f.p))
        .unwrap();
    f.storage
        .override_add(None, range(14, 10, 20), None)
        .unwrap();
    let rows = made(&f);
    assert_eq!(rows.1, 600);
    let seconds = |key: &str| rows.0.iter().find(|row| row.0 == key).map(|row| row.1);
    assert_eq!((seconds(&p), seconds(&q)), (Some(1200), Some(600)));

    // A range of the owner's time decides over a live transcript too...
    let none = f
        .storage
        .override_add(None, range(10, 20, 25), None)
        .unwrap();
    let rows = made(&f);
    assert_eq!(rows.1, 900);
    // ...unless a range made later relabels the transcript there.
    f.storage
        .override_add(Some("s"), range(10, 22, 24), Some(f.q))
        .unwrap();
    let rows = made(&f);
    assert_eq!(rows.1, 900 - 120);
    let seconds = |key: &str| rows.0.iter().find(|row| row.0 == key).map(|row| row.1);
    assert_eq!(seconds(&q), Some(600 + 120));
    assert!(f.storage.override_remove(none).unwrap());
}

#[test]
fn override_is_kept_through_a_replay_and_undoing_it_restores_the_rows() {
    let f = corrected();
    let computed = made(&f);
    let pin = f.storage.override_add(Some("s"), None, Some(f.q)).unwrap();
    let pinned = made(&f);
    assert_ne!(pinned, computed);

    // The transcript is read again from its start: its segments are
    // dropped and written anew. The correction is kept apart from them.
    let expected = f.storage.observation_checkpoint("s").unwrap();
    let mut next = expected.clone().unwrap();
    next.cursor.generation += 1;
    let again = Segment {
        stored: None,
        project: Some(f.p),
        learned: Learned::By(Source::Path),
        started_at: at(29, 10, 0, 0),
        last_at: at(29, 11, 0, 0),
        last_touch_at: None,
        activity: vec![(at(29, 10, 0, 0).timestamp() / 60, 1, 1)],
    };
    let replayed = Batch {
        expected: expected.as_ref(),
        next: &next,
        replaced: true,
        agent: "claude",
        segments: &[again],
        prompts: &[],
    };
    assert!(f.storage.observe(&replayed).unwrap());
    assert_eq!(made(&f), pinned);

    assert!(f.storage.override_remove(pin).unwrap());
    assert!(!f.storage.override_remove(pin).unwrap());
    assert_eq!(made(&f), computed);
}

#[test]
fn override_marks_the_days_it_changes_when_made_and_when_undone() {
    use chrono::Datelike;
    let f = corrected();
    let marked = |f: &Fixture| -> Vec<u32> {
        let marks = f.storage.attribution_dirty().unwrap();
        f.storage.attribution_dirty_clear(&marks).unwrap();
        marks.iter().map(|(start, _)| start.day()).collect()
    };
    assert_eq!(marked(&f), [29]);
    // A pin: the days of its transcript. A range: its own days.
    let pin = f.storage.override_add(Some("s"), None, Some(f.q)).unwrap();
    assert_eq!(marked(&f), [29]);
    let range = Some((at(27, 23, 0, 0), at(28, 1, 0, 0)));
    let relabel = f.storage.override_add(None, range, None).unwrap();
    assert_eq!(marked(&f), [27, 28]);
    for (id, days) in [(pin, vec![29]), (relabel, vec![27, 28])] {
        assert!(f.storage.override_remove(id).unwrap());
        assert_eq!(marked(&f), days);
    }
    // The marked day is made again by the next pass.
    f.storage.override_add(Some("s"), None, Some(f.q)).unwrap();
    let mut seen = None;
    assert_eq!(
        f.due(&Zone::UTC, at(30, 12, 0, 0), &mut seen),
        [day(29), day(30)]
    );
    assert_eq!(f.rows(day(29)).0[0].0, f.key(f.q));
}

#[test]
fn override_corrects_the_time_of_agents_as_it_corrects_the_owners() {
    let f = corrected();
    let (from, to) = (at(29, 0, 0, 0), at(30, 0, 0, 0));
    let agents = |f: &Fixture| f.storage.agent_seconds(from, to).unwrap();
    assert_eq!(agents(&f), BTreeMap::from([(Some(f.p), 3600)]));
    f.storage.override_add(Some("s"), None, Some(f.q)).unwrap();
    assert_eq!(agents(&f), BTreeMap::from([(Some(f.q), 3600)]));
    let range = Some((at(29, 10, 30, 0), at(29, 10, 45, 0)));
    f.storage.override_add(Some("s"), range, None).unwrap();
    assert_eq!(agents(&f), BTreeMap::from([(None, 900), (Some(f.q), 2700)]));
    // A range of the owner's own time is not a transcript's.
    f.storage
        .override_add(None, Some((from, to)), Some(f.p))
        .unwrap();
    assert_eq!(agents(&f).get(&Some(f.p)), None);
}

#[test]
fn override_says_what_it_cannot_be() {
    let f = corrected();
    let range = Some((at(29, 10, 30, 0), at(29, 10, 20, 0)));
    assert!(f.storage.override_add(None, None, Some(f.p)).is_err());
    assert!(f.storage.override_add(Some("s"), range, Some(f.p)).is_err());
    assert!(f.storage.override_add(Some("s"), None, Some(9999)).is_err());
    // Every day of a range is made again, so one of ten years is not kept.
    let decade = Some((at(1, 0, 0, 0), at(1, 0, 0, 0) + Duration::days(3660)));
    assert!(f.storage.override_add(None, decade, Some(f.p)).is_err());
    assert!(f.storage.overrides().unwrap().is_empty());
    // One of two years is, and every one of its days is marked.
    let marks = f.storage.attribution_dirty().unwrap();
    f.storage.attribution_dirty_clear(&marks).unwrap();
    let years = Some((at(1, 0, 0, 0), at(1, 0, 0, 0) + Duration::days(730)));
    f.storage.override_add(None, years, Some(f.p)).unwrap();
    assert_eq!(f.storage.attribution_dirty().unwrap().len(), 731);
}

#[test]
fn owner_time_due_a_daemon_that_starts_finishes_yesterday() {
    let f = Fixture::new();
    f.session(
        "s",
        false,
        Some(f.p),
        (at(28, 23, 0, 0), at(28, 23, 59, 0)),
        &[at(28, 23, 0, 0)],
        &[at(28, 23, 50, 0)],
    );
    let mut seen = None;
    f.due(&Zone::UTC, at(28, 23, 52, 0), &mut seen);
    // The owner works on to the end of the day, and the daemon is
    // started again after midnight: it knows of no pass before.
    f.active(at(28, 23, 50, 0), at(28, 23, 58, 0));
    let mut seen = None;
    assert_eq!(
        f.due(&Zone::UTC, at(29, 0, 2, 0), &mut seen),
        [day(28), day(29)]
    );
    assert_eq!(f.rows(day(28)).0[0].1, 480);
}

#[test]
fn owner_time_what_is_written_marks_the_days_it_reaches_into() {
    use chrono::Datelike;
    let f = Fixture::new();
    let marked = |f: &Fixture| -> Vec<u32> {
        let marks = f.storage.attribution_dirty().unwrap();
        f.storage.attribution_dirty_clear(&marks).unwrap();
        marks.iter().map(|(start, _)| start.day()).collect()
    };
    // A prompt a minute before midnight holds its transcript live for a
    // quarter of an hour of the next day.
    let span = (at(27, 23, 58, 0), at(27, 23, 59, 0));
    f.session(
        "late",
        false,
        Some(f.p),
        span,
        &[span.0],
        &[at(27, 23, 59, 0)],
    );
    assert_eq!(marked(&f), [27, 28]);
    // A segment that ends half an hour before midnight: the age of its
    // last touch is told for an hour after it.
    let span = (at(20, 23, 0, 0), at(20, 23, 30, 0));
    f.session("evening", false, Some(f.p), span, &[span.0], &[]);
    assert_eq!(marked(&f), [20, 21]);
    // One that ends at noon reaches into no other day.
    let span = (at(22, 11, 0, 0), at(22, 12, 0, 0));
    f.session("noon", false, Some(f.p), span, &[span.0], &[]);
    assert_eq!(marked(&f), [22]);

    // The quarter of an hour after midnight is the prompted project's.
    f.active(at(28, 0, 0, 0), at(28, 0, 20, 0));
    f.make(&Zone::UTC, day(28));
    assert_eq!(
        f.rows(day(28)),
        (vec![(f.key(f.p), 14 * 60, "high".into())], 360)
    );
}

#[test]
fn owner_time_a_day_whose_segments_were_dropped_is_still_made_again() {
    let f = Fixture::new();
    let first = (at(20, 9, 0, 0), at(20, 10, 0, 0));
    f.session("s", false, Some(f.p), first, &[first.0], &[]);
    f.active(at(20, 9, 10, 0), at(20, 9, 20, 0));
    let mut seen = None;
    let now = at(29, 12, 0, 0);
    f.due(&Zone::UTC, now, &mut seen);
    assert_eq!(
        f.rows(day(20)),
        (vec![(f.key(f.p), 600, "medium".into())], 0)
    );

    // The transcript is rewritten: its only segment is of day 26 now.
    let expected = f.storage.observation_checkpoint("s").unwrap();
    let mut next = expected.clone().unwrap();
    next.cursor.generation += 1;
    let later = Segment {
        stored: None,
        project: Some(f.p),
        learned: Learned::By(Source::Path),
        started_at: at(26, 9, 0, 0),
        last_at: at(26, 9, 30, 0),
        last_touch_at: None,
        activity: vec![],
    };
    let rewritten = Batch {
        expected: expected.as_ref(),
        next: &next,
        replaced: true,
        agent: "claude",
        segments: &[later],
        prompts: &[],
    };
    assert!(f.storage.observe(&rewritten).unwrap());
    // The rule is in force since the first segment ever written, so the
    // day that lost its segment is made again: nobody's time now.
    assert_eq!(f.storage.first_segment_at().unwrap(), Some(first.0));
    let made = f.due(&Zone::UTC, now, &mut seen);
    assert!(made.contains(&day(20)) && made.contains(&day(26)));
    assert_eq!(f.rows(day(20)), (vec![], 600));
}

#[test]
fn owner_time_a_memory_counts_once_for_a_project_whatever_rows_carry_its_entity() {
    let f = Fixture::new();
    f.memory("m1", at(29, 10, 5, 0), "demoapp");
    f.memory("m2", at(29, 10, 6, 0), "notes");
    assert!(f.storage.project_link_entity(f.p, "demoapp").unwrap());
    assert!(f.storage.project_link_entity(f.q, "notes").unwrap());
    // Two rows merged into the first project carry its entity too.
    for name in ["formerly", "earlier"] {
        let old = f.storage.project_upsert(name, name, None).unwrap();
        assert!(f.storage.project_link_entity(old, "demoapp").unwrap());
        f.storage.project_merge(old, f.p).unwrap();
    }
    let signals = f
        .storage
        .registry_signals(at(29, 10, 0, 0), at(29, 11, 0, 0))
        .unwrap();
    assert_eq!(signals, [(f.p, 1.0), (f.q, 1.0)]);
    // So the two projects share the time no transcript covers.
    f.session(
        "old",
        false,
        Some(f.q),
        (at(20, 10, 0, 0), at(20, 10, 5, 0)),
        &[],
        &[],
    );
    f.active(at(29, 10, 0, 0), at(29, 10, 20, 0));
    f.make(&Zone::UTC, day(29));
    let mut expected = vec![
        (f.key(f.p), 600, "low".to_string()),
        (f.key(f.q), 600, "low".to_string()),
    ];
    expected.sort();
    assert_eq!(f.rows(day(29)), (expected, 0));
}

#[test]
fn owner_time_rows_are_read_under_the_key_they_stand_for_now() {
    let f = Fixture::new();
    let place = f.key(f.q);
    assert_eq!(crate::owner_time::registry_id(&place), Some(f.q));
    let low = || Some("low".to_string());
    let high = || Some("high".to_string());

    // The project gets its entity after a day was written under its place
    // in the registry: both keys are one project to the readers.
    let entity = f
        .storage
        .upsert_entity(&Entity {
            name: "notes".into(),
            entity_type: EntityType::Project,
        })
        .unwrap();
    assert!(f.storage.project_link_entity(f.q, "notes").unwrap());
    let rows = vec![
        (place.clone(), 100.0, low()),
        (entity.clone(), 50.0, high()),
    ];
    assert_eq!(
        f.storage.rekeyed_day(rows).unwrap(),
        [(entity.clone(), 150.0, low())]
    );

    // A project merged into it since is served under it; a key no
    // project stands behind is left as it is.
    let other = f
        .storage
        .upsert_entity(&Entity {
            name: "demoapp".into(),
            entity_type: EntityType::Project,
        })
        .unwrap();
    assert!(f.storage.project_link_entity(f.p, "demoapp").unwrap());
    f.storage.project_merge(f.p, f.q).unwrap();
    let row = |key: &str, today: f64, week: f64| crate::activity::ProjectTimeRow {
        project_key: key.to_string(),
        today_seconds: today,
        week_seconds: week,
        week: [0.0, 0.0, 0.0, 0.0, 0.0, week - today, today],
        confidence: high(),
    };
    let time = crate::activity::ProjectTimeData {
        rows: vec![
            row(&other, 10.0, 30.0),
            row("an earlier key", 5.0, 5.0),
            row(&place, 1.0, 100.0),
        ],
        unattributed_today: 7.0,
        unattributed_week: 9.0,
    };
    let read = f.storage.rekeyed_time(&time).unwrap();
    assert_eq!(
        read.rows,
        [row(&entity, 11.0, 130.0), row("an earlier key", 5.0, 5.0)]
    );
    assert_eq!(
        (read.unattributed_today, read.unattributed_week),
        (7.0, 9.0)
    );
}

#[test]
fn owner_time_the_journal_keeps_the_lines_of_a_merged_project_under_its_new_key() {
    let f = Fixture::new();
    let noon = at(29, 12, 0, 0);
    let local_day = noon.with_timezone(&chrono::Local).date_naive();
    // Two memories of the first project on that day, and its time kept
    // under its entity.
    f.memory("m1", noon, "demoapp");
    f.memory("m2", noon + Duration::minutes(5), "demoapp");
    assert!(f.storage.project_link_entity(f.p, "demoapp").unwrap());
    let into = f
        .storage
        .upsert_entity(&Entity {
            name: "notes".into(),
            entity_type: EntityType::Project,
        })
        .unwrap();
    assert!(f.storage.project_link_entity(f.q, "notes").unwrap());
    let written_under = f.key(f.p);
    f.activity
        .write_day(local_day, &[(written_under.as_str(), 300.0, "high")], 0.0)
        .unwrap();
    let before = crate::journal::collect(&f.storage, &f.activity, local_day).unwrap();
    assert_eq!(before.projects.len(), 1);
    assert_eq!(before.projects[0].events.len(), 2);

    // The project is merged into another afterwards: its time and its
    // lines are served together, under that one.
    f.storage.project_merge(f.p, f.q).unwrap();
    let after = crate::journal::collect(&f.storage, &f.activity, local_day).unwrap();
    assert_eq!(after.unattributed_seconds, 0.0);
    assert_eq!(after.projects.len(), 1);
    let shown = &after.projects[0];
    assert_eq!(
        (shown.key.as_str(), shown.name.as_str(), shown.seconds),
        (into.as_str(), "notes", 300.0)
    );
    assert_eq!(shown.events.len(), 2);
}

#[test]
fn owner_time_the_journal_chooses_a_memorys_project_by_the_names_it_was_tied_to() {
    let f = Fixture::new();
    let noon = at(29, 12, 0, 0);
    let local_day = noon.with_timezone(&chrono::Local).date_naive();
    // Memories tied to two projects, whose titles name the first.
    for (id, minute) in [("m1", 0), ("m2", 5)] {
        f.memory(id, noon + Duration::minutes(minute), "demoapp");
        let conn = f.storage.conn.lock().unwrap();
        conn.execute(
            "UPDATE memories SET title = 'feat(demoapp): export notes' WHERE id = ?1",
            [id],
        )
        .unwrap();
    }
    let notes = f
        .storage
        .upsert_entity(&Entity {
            name: "notes".into(),
            entity_type: EntityType::Project,
        })
        .unwrap();
    for id in ["m1", "m2", "n1", "n2"] {
        if id.starts_with('n') {
            f.memory(id, noon + Duration::minutes(20), "notes");
        } else {
            let conn = f.storage.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO memory_entities (memory_id, entity_id) VALUES (?1, ?2)",
                rusqlite::params![id, notes],
            )
            .unwrap();
        }
    }
    assert!(f.storage.project_link_entity(f.p, "demoapp").unwrap());
    assert!(f.storage.project_link_entity(f.q, "notes").unwrap());
    // The first project is merged into a third after its time was kept.
    let platform = f
        .storage
        .project_upsert("platform", "platform", None)
        .unwrap();
    let into = f
        .storage
        .upsert_entity(&Entity {
            name: "platform".into(),
            entity_type: EntityType::Project,
        })
        .unwrap();
    assert!(f.storage.project_link_entity(platform, "platform").unwrap());
    let written_under = f.key(f.p);
    let rows = [
        (written_under.as_str(), 300.0, "high"),
        (notes.as_str(), 60.0, "low"),
    ];
    f.activity.write_day(local_day, &rows, 0.0).unwrap();
    f.storage.project_merge(f.p, platform).unwrap();

    let day = crate::journal::collect(&f.storage, &f.activity, local_day).unwrap();
    let events = |key: &str| {
        day.projects
            .iter()
            .find(|project| project.key == key)
            .map(|project| (project.name.clone(), project.events.len()))
    };
    // The memories whose titles name the merged project are shown under
    // the one it was merged into, not under the other project they are
    // tied to.
    assert_eq!(events(&into), Some(("platform".to_string(), 2)));
    assert_eq!(events(&notes).map(|shown| shown.1), Some(2));
}

#[test]
fn live_the_hours_of_today_are_the_owners_time_and_the_agents_time_by_project() {
    let f = Fixture::new();
    // The owner's session on one project for an hour, a session a
    // program ran on the other beside it, and ten minutes at the Mac.
    let stretch = (at(29, 10, 0, 0), at(29, 11, 0, 0));
    f.session("owner", false, Some(f.p), stretch, &[at(29, 10, 0, 0)], &[]);
    let half = (at(29, 10, 0, 0), at(29, 10, 30, 0));
    f.session("program", true, Some(f.q), half, &[at(29, 10, 0, 0)], &[]);
    f.active(at(29, 10, 20, 0), at(29, 10, 30, 0));
    // And five minutes later on, with no transcript live.
    f.active(at(29, 14, 0, 0), at(29, 14, 5, 0));
    let now = at(29, 15, 0, 0);
    let hours = hours_of(
        &f.activity,
        &f.storage,
        &Zone::UTC,
        now,
        &AttribCfg::default(),
    )
    .unwrap();
    assert_eq!(hours.day, Some(day(29)));
    let today = |project: i64| hours.projects[&project];
    assert_eq!(
        (
            today(f.p).owner_s,
            today(f.p).agent_s,
            today(f.p).confidence
        ),
        (600, 3600, Some("medium"))
    );
    // The program's session is an agent's time and none of the owner's.
    assert_eq!(
        (
            today(f.q).owner_s,
            today(f.q).agent_s,
            today(f.q).confidence
        ),
        (0, 1800, None)
    );
    // The owner's total is all of the time at the Mac. The agents' total
    // is the union of their time: the two worked at once for half an
    // hour, which is counted once.
    assert_eq!((hours.owner_s, hours.agent_s), (900, 3600));
    // Asked earlier in the day, the time ends at the asking: for the
    // agents, and for the owner, whose later activity is not of the day
    // so far (a clock that was set back leaves such stamps).
    let earlier = hours_of(
        &f.activity,
        &f.storage,
        &Zone::UTC,
        at(29, 10, 25, 0),
        &AttribCfg::default(),
    )
    .unwrap();
    assert_eq!(earlier.projects[&f.p].agent_s, 1500);
    assert_eq!((earlier.owner_s, earlier.agent_s), (300, 1500));
}
