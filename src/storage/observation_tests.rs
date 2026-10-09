use super::*;
use chrono::TimeZone;

fn at(day: u32, hour: u32, minute: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, day, hour, minute, 0).unwrap()
}

#[test]
fn segments_agent_time_is_the_union_of_its_intervals_cut_at_midnight() {
    let dir = crate::test_support::temp_dir("mn-agent-time-");
    let storage = Storage::open(&dir.path().join("memory.db")).unwrap();
    let a = storage.project_upsert("demoapp", "demoapp", None).unwrap();
    let b = storage.project_upsert("notes", "notes", None).unwrap();
    let merged = storage.project_upsert("oldapp", "oldapp", None).unwrap();
    storage.project_merge(merged, a).unwrap();
    let segment = |stream: &str, project: Option<i64>, from: DateTime<Utc>, to: DateTime<Utc>| {
        storage
            .conn
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO work_segments (stream, driver, agent, project_id, started_at, last_at, source)
                 VALUES (?1, 'owner', 'claude', ?2, ?3, ?4, 'path')",
                params![stream, project, stamp(from), stamp(to)],
            )
            .unwrap();
    };
    // Two sessions on one project at once count once; a row written under
    // a project that was merged counts under the one it was merged into.
    segment("s1", Some(a), at(29, 22, 0), at(29, 23, 0));
    segment("s2", Some(a), at(29, 22, 30), at(29, 23, 30));
    segment("s3", Some(merged), at(29, 23, 15), at(29, 23, 45));
    // One that runs over midnight is cut there.
    segment("s4", Some(b), at(29, 23, 50), at(30, 0, 20));
    segment("s5", None, at(29, 10, 0), at(29, 10, 5));

    let day = storage.agent_seconds(at(29, 0, 0), at(30, 0, 0)).unwrap();
    assert_eq!(day[&Some(a)], 105 * 60);
    assert_eq!(day[&Some(b)], 10 * 60);
    assert_eq!(day[&None], 5 * 60);
    assert!(!day.contains_key(&Some(merged)));
    // In all: the union over every segment, not the sum of the projects.
    // 22:00 to 23:45 and 23:50 to midnight, and the five minutes apart.
    let (by_project, in_all) = storage.agent_time(at(29, 0, 0), at(30, 0, 0)).unwrap();
    assert_eq!(by_project, day);
    assert_eq!(in_all, (105 + 10 + 5) * 60);
    segment("s6", Some(b), at(29, 22, 0), at(29, 23, 0));
    let (by_project, in_all) = storage.agent_time(at(29, 0, 0), at(30, 0, 0)).unwrap();
    assert_eq!(by_project[&Some(b)], 70 * 60);
    assert_eq!(in_all, (105 + 10 + 5) * 60);
    storage
        .conn
        .lock()
        .unwrap()
        .execute("DELETE FROM work_segments WHERE stream = 's6'", [])
        .unwrap();
    let next = storage.agent_seconds(at(30, 0, 0), at(30, 23, 59)).unwrap();
    assert_eq!(next.get(&Some(b)), Some(&(20 * 60)));
    assert_eq!(next.get(&Some(a)), None);
}

#[test]
fn segments_the_union_of_intervals_counts_what_overlaps_once() {
    let (from, to) = (at(29, 0, 0), at(30, 0, 0));
    assert_eq!(union_seconds(&[], from, to), 0);
    let apart = [(at(29, 1, 0), at(29, 2, 0)), (at(29, 5, 0), at(29, 5, 30))];
    assert_eq!(union_seconds(&apart, from, to), 90 * 60);
    let inside = [(at(29, 1, 0), at(29, 3, 0)), (at(29, 1, 30), at(29, 2, 0))];
    assert_eq!(union_seconds(&inside, from, to), 120 * 60);
    let touching = [(at(29, 2, 0), at(29, 3, 0)), (at(29, 1, 0), at(29, 2, 0))];
    assert_eq!(union_seconds(&touching, from, to), 120 * 60);
    let outside = [(at(28, 1, 0), at(28, 3, 0)), (at(30, 1, 0), at(30, 2, 0))];
    assert_eq!(union_seconds(&outside, from, to), 0);
}
