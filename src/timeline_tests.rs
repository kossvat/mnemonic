//! Timeline policy and payload checks over invented offline stores.
use super::fixture::{Fixture, Zone, at};
use super::*;
use chrono::Duration;

fn day() -> NaiveDate {
    at(0, 0).date_naive()
}
fn read_fixture(f: &Fixture) -> ProjectTimeline {
    read(
        &f.storage,
        Some(&f.activity),
        &Zone::UTC,
        day(),
        at(23, 0),
        &AttribCfg::default(),
    )
    .unwrap()
}
fn nearly(left: f64, right: f64) {
    assert!((left - right).abs() < 1e-6, "{left} != {right}");
}

#[test]
fn timeline_shared_fixture_keeps_concurrency_points_fractional_ranges_and_read_purity() {
    let f = Fixture::full();
    let before = f.stores();
    let files = f.files();
    let changes = f.changes();
    let actual = read_fixture(&f);
    let expected: serde_json::Value = serde_json::from_str(include_str!(
        "../clients/macos/Tests/Fixtures/project-timeline.json"
    ))
    .unwrap();
    assert_eq!(serde_json::to_value(&actual).unwrap(), expected);
    assert_eq!(actual, read_fixture(&f));
    assert_eq!(actual.segments[2].from, actual.segments[2].to);
    let parallel = &actual.owner[1];
    assert_eq!(parallel.assignments.len(), 2);
    nearly(parallel.to - parallel.from, 600.0);
    for range in &actual.owner {
        nearly(range.assignments.iter().map(|share| share.share).sum(), 1.0);
    }
    let total = crate::owner_time_day::compute(
        &f.activity,
        &f.storage,
        Zone::UTC.bounds(day()).unwrap(),
        &AttribCfg::default(),
    )
    .unwrap();
    let keys = f.storage.attribution_keys().unwrap();
    let seconds = |project| {
        total
            .rows
            .iter()
            .find(|row| row.0 == keys[&project])
            .unwrap()
            .1
    };
    nearly(seconds(f.demo), 900.5);
    nearly(seconds(f.notes), 600.0);
    nearly(total.unattributed, 600.0);
    nearly(
        actual.owner.iter().map(|range| range.to - range.from).sum(),
        2100.5,
    );
    assert_eq!(f.stores(), before);
    assert_eq!(f.files(), files);
    assert_eq!(f.changes(), changes);
    assert_eq!(before[0]["recall_events"].len(), 1);
    assert_eq!(before[0]["recall_items"].len(), 1);
}

#[test]
fn timeline_today_clips_data_but_keeps_full_axis_and_future_is_empty() {
    let f = Fixture::full();
    let now = at(9, 10) + Duration::milliseconds(250);
    let value = read(
        &f.storage,
        Some(&f.activity),
        &Zone::UTC,
        day(),
        now,
        &AttribCfg::default(),
    )
    .unwrap();
    nearly(value.to - value.from, 86_400.0);
    assert_eq!(value.segments.len(), 2);
    let upper = now.timestamp_millis() as f64 / 1000.0;
    assert!(value.segments.iter().all(|segment| segment.to == upper));
    assert_eq!(value.owner.len(), 2);
    nearly(value.owner.last().unwrap().to, upper);
    let future = read(
        &f.storage,
        Some(&f.activity),
        &Zone::UTC,
        day().succ_opt().unwrap(),
        now,
        &AttribCfg::default(),
    )
    .unwrap();
    assert!(future.segments.is_empty() && future.owner.is_empty());
    assert!(future.owner_available);
    assert_eq!(future.projects.len(), 2);
    let midnight = read(
        &f.storage,
        Some(&f.activity),
        &Zone::UTC,
        day(),
        at(0, 0),
        &AttribCfg::default(),
    )
    .unwrap();
    assert!(midnight.segments.is_empty() && midnight.owner.is_empty());
}

#[test]
fn timeline_activity_unavailable_is_distinct_from_an_empty_owner_lane() {
    let f = Fixture::full();
    let missing = read(
        &f.storage,
        None,
        &Zone::UTC,
        day(),
        at(23, 0),
        &AttribCfg::default(),
    )
    .unwrap();
    assert!(!missing.owner_available);
    assert!(missing.owner.is_empty());
    assert_eq!(missing.segments.len(), 4);
    let empty = Fixture::empty();
    let available = read_fixture(&empty);
    assert!(available.owner_available);
    assert!(available.owner.is_empty() && available.segments.is_empty());
}

#[test]
fn timeline_activity_without_segments_honors_manual_ranges_newest_correction_and_explicit_none() {
    let f = Fixture::empty();
    f.active(at(9, 0), at(9, 20));
    f.memory("nearby", at(9, 8), f.demo);
    f.storage
        .override_add(None, Some((at(9, 5), at(9, 10))), Some(f.notes))
        .unwrap();
    f.storage
        .override_add(None, Some((at(9, 7), at(9, 9))), Some(f.demo))
        .unwrap();
    f.storage
        .override_add(None, Some((at(9, 10), at(9, 15))), None)
        .unwrap();
    let value = read_fixture(&f);
    assert!(value.segments.is_empty());
    assert_eq!(
        value
            .owner
            .iter()
            .map(|range| (
                range.assignments[0].project.as_deref(),
                range.assignments[0].confidence,
                range.to - range.from
            ))
            .collect::<Vec<_>>(),
        vec![
            (Some("demoapp"), Some("low"), 300.0),
            (Some("notes"), Some("high"), 120.0),
            (Some("demoapp"), Some("high"), 120.0),
            (Some("notes"), Some("high"), 60.0),
            (None, None, 300.0),
            (Some("demoapp"), Some("low"), 300.0)
        ]
    );
}

#[test]
fn timeline_fallback_joins_adjacent_uncovered_ranges_before_reading_padded_signals() {
    let f = Fixture::empty();
    f.active(at(9, 0), at(9, 20));
    // Unknown transcript boundaries must not shrink the fallback's window.
    f.segment("unknown", "claude", false, None, at(9, 5), at(9, 10));
    f.memory("padded", at(9, 25), f.demo);
    let value = read_fixture(&f);
    assert_eq!(value.owner.len(), 1);
    assert_eq!(
        value.owner[0].assignments[0].project.as_deref(),
        Some("demoapp")
    );
    assert_eq!(value.owner[0].assignments[0].confidence, Some("low"));
    nearly(value.owner[0].to - value.owner[0].from, 1200.0);
}

#[test]
fn timeline_merges_pins_and_overlapping_relabels_use_current_canonical_projects() {
    let f = Fixture::empty();
    let old = f
        .storage
        .project_upsert("old-demo", "Old demo", None)
        .unwrap();
    f.segment("corrected", "claude", false, Some(old), at(9, 0), at(10, 0));
    f.active(at(9, 0), at(10, 0));
    f.storage
        .override_add(Some("corrected"), None, Some(f.notes))
        .unwrap();
    let earlier = f
        .storage
        .override_add(Some("corrected"), Some((at(9, 10), at(9, 30))), Some(old))
        .unwrap();
    let later = f
        .storage
        .override_add(Some("corrected"), Some((at(9, 20), at(9, 40))), None)
        .unwrap();
    f.storage.project_merge(old, f.demo).unwrap();
    let value = read_fixture(&f);
    assert_eq!(
        value
            .projects
            .iter()
            .map(|project| project.key.as_str())
            .collect::<Vec<_>>(),
        ["demoapp", "notes"]
    );
    assert_eq!(
        value
            .segments
            .iter()
            .map(|segment| (segment.project.as_deref(), segment.to - segment.from))
            .collect::<Vec<_>>(),
        [
            (Some("notes"), 600.0),
            (Some("demoapp"), 600.0),
            (None, 1200.0),
            (Some("notes"), 1200.0)
        ]
    );
    // Undo reveals the earlier range; the original segment is unchanged.
    assert!(f.storage.override_remove(later).unwrap());
    let restored = read_fixture(&f);
    assert_eq!(restored.segments.len(), 3);
    assert_eq!(restored.segments[1].project.as_deref(), Some("demoapp"));
    nearly(restored.segments[1].to - restored.segments[1].from, 1200.0);
    assert!(f.storage.override_remove(earlier).unwrap());
    let pinned = read_fixture(&f);
    assert_eq!(pinned.segments.len(), 1);
    assert_eq!(pinned.segments[0].project.as_deref(), Some("notes"));
    let conn = f.storage.conn.lock().unwrap();
    assert_eq!(
        conn.query_row("SELECT project_id FROM work_segments", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        old
    );
}

#[test]
fn timeline_real_day_bounds_support_both_clock_changes_and_midnight_points() {
    for (before, after, hours) in [(-5, -4, 23), (-4, -5, 25)] {
        let zone = Zone {
            offset: before * 3600,
            turn: Some((at(7, 0), after * 3600)),
        };
        let (from, to) = zone.bounds(day()).unwrap();
        let f = Fixture::empty();
        f.segment(
            "crossing",
            "claude",
            false,
            Some(f.demo),
            from - Duration::hours(1),
            to + Duration::hours(1),
        );
        f.segment("at-start", "codex", false, None, from, from);
        f.segment("at-end", "codex", false, None, to, to);
        f.active(from - Duration::minutes(1), from + Duration::minutes(1));
        f.active(to - Duration::seconds(40), to + Duration::seconds(20));
        let value = read(
            &f.storage,
            Some(&f.activity),
            &zone,
            day(),
            to + Duration::hours(2),
            &AttribCfg::default(),
        )
        .unwrap();
        nearly(value.to - value.from, hours as f64 * 3600.0);
        assert_eq!(value.segments.len(), 2);
        assert_eq!(value.segments[0].stream, "at-start");
        assert_eq!(value.segments[0].from, value.from);
        assert_eq!(value.segments[0].from, value.segments[0].to);
        let crossing = value
            .segments
            .iter()
            .find(|segment| segment.stream == "crossing")
            .unwrap();
        assert_eq!((crossing.from, crossing.to), (value.from, value.to));
        nearly(
            value.owner.iter().map(|range| range.to - range.from).sum(),
            100.0,
        );
    }
}

#[test]
fn timeline_strict_gregorian_dates_do_not_accept_normalization_or_locale_forms() {
    for value in [
        "2026-09-29",
        "2024-02-29",
        "2000-02-29",
        "0001-01-01",
        "9999-12-31",
    ] {
        assert!(parse_day(value).is_some(), "{value}");
    }
    for value in [
        "",
        "2026-9-29",
        "2026-09-9",
        "2026-02-29",
        "1900-02-29",
        "2026-04-31",
        "0000-01-01",
        "2026-00-01",
        "2026-13-01",
        "2026-09-00",
        " 2026-09-29",
        "2026-09-29 ",
        "2026-09-29T00:00:00Z",
    ] {
        assert!(parse_day(value).is_none(), "{value}");
    }
}

#[test]
fn timeline_stream_identity_admits_real_filename_shapes_without_directory_paths() {
    assert!(safe_stream("opaque-fixture"));
    assert!(safe_stream(r#"["conversation","fixture.jsonl"]"#));
    assert!(safe_stream(r#"["codex","rollout-fixture.jsonl"]"#));
    for refused in [
        "",
        ".",
        "..",
        "folder/file",
        r"folder\file",
        "line\nnext",
        r#"["conversation","folder/file.jsonl"]"#,
        r#"["codex","folder\u002ffile.jsonl"]"#,
    ] {
        assert!(!safe_stream(refused), "{refused}");
    }
}
