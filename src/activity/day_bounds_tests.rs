//! Boundary searches and real zones, without changing the test process TZ.
use super::*;
use chrono::FixedOffset;
#[cfg(unix)]
use chrono::Timelike;

fn date(value: &str) -> NaiveDate {
    NaiveDate::parse_from_str(value, "%Y-%m-%d").unwrap()
}
#[cfg(unix)]
fn utc(value: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value)
        .unwrap()
        .with_timezone(&Utc)
}

#[test]
fn timeline_day_boundary_fast_path_and_gap_refinement_are_bounded_and_exact() {
    let midnight = date("2024-09-08").and_hms_opt(0, 0, 0).unwrap();
    let mut calls = 0;
    let ordinary = first_representable(midnight, |local| {
        calls += 1;
        Some(local.and_utc())
    });
    assert_eq!(ordinary, Some(midnight.and_utc()));
    assert_eq!(calls, 1);
    let first = midnight + Duration::seconds(3661);
    calls = 0;
    let gap = first_representable(midnight, |local| {
        calls += 1;
        (*local >= first).then(|| local.and_utc())
    });
    assert_eq!(gap, Some(first.and_utc()));
    assert!(calls <= 16, "{calls} lookups");
    calls = 0;
    assert!(
        first_representable(midnight, |_| {
            calls += 1;
            None
        })
        .is_none()
    );
    assert_eq!(calls, 49);
    assert!(in_zone(&FixedOffset::east_opt(0).unwrap(), NaiveDate::MAX).is_none());
}

#[cfg(unix)]
#[test]
fn timeline_day_boundaries_real_zones_keep_previous_days_and_earliest_folds() {
    for (case, zone) in [
        ("gap", "America/Santiago"),
        ("fold", "America/Havana"),
        ("skip", "Pacific/Apia"),
    ] {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "activity::day_bounds::tests::timeline_day_real_zone_child",
                "--nocapture",
            ])
            .env("TZ", zone)
            .env("MNEMONIC_DAY_BOUNDARY_CASE", case)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{case}: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("day boundary case passed"),
            "the isolated child must run its assertion"
        );
    }
}

#[cfg(unix)]
#[test]
fn timeline_day_real_zone_child() {
    let Ok(case) = std::env::var("MNEMONIC_DAY_BOUNDARY_CASE") else {
        return;
    };
    let (day, from, to, hours) = match case.as_str() {
        "gap" => (
            date("2024-09-08"),
            utc("2024-09-08T04:00:00Z"),
            utc("2024-09-09T03:00:00Z"),
            23,
        ),
        "fold" => (
            date("2024-11-03"),
            utc("2024-11-03T04:00:00Z"),
            utc("2024-11-04T05:00:00Z"),
            25,
        ),
        "skip" => {
            let skipped = date("2011-12-30");
            let midnight = skipped.and_hms_opt(0, 0, 0).unwrap();
            // A stale Single offset at the gap endpoint must not invent this day.
            assert!(valid_instant(&chrono::Local, &midnight).is_none());
            if let Some(candidate) = chrono::Local.from_local_datetime(&midnight).earliest() {
                assert_eq!(
                    candidate
                        .with_timezone(&Utc)
                        .with_timezone(&chrono::Local)
                        .date_naive(),
                    skipped.succ_opt().unwrap()
                );
            }
            assert!(super::super::local_day_bounds_utc(skipped).is_none());
            let previous = skipped.pred_opt().unwrap();
            let bounds = super::super::local_day_bounds_utc(previous).unwrap();
            assert_eq!(
                bounds,
                (utc("2011-12-29T10:00:00Z"), utc("2011-12-30T10:00:00Z"))
            );
            let next = super::super::local_day_bounds_utc(skipped.succ_opt().unwrap()).unwrap();
            assert_eq!(bounds.1, next.0);
            assert_eq!(
                bounds.1.with_timezone(&chrono::Local).date_naive(),
                skipped.succ_opt().unwrap()
            );
            let fixture = crate::timeline::fixture::Fixture::empty();
            assert!(
                crate::timeline::read(
                    &fixture.storage,
                    Some(&fixture.activity),
                    &crate::owner_time_day::LocalCalendar,
                    skipped,
                    next.1,
                    &crate::attribution::AttribCfg::default()
                )
                .is_err()
            );
            check_timeline(previous, bounds.0, bounds.1);
            println!("day boundary case passed");
            return;
        }
        _ => panic!("unexpected child case"),
    };
    let midnight = day.and_hms_opt(0, 0, 0).unwrap();
    let raw = chrono::Local.from_local_datetime(&midnight);
    if case == "gap" {
        // Chrono may claim the old offset for exactly 00:00; its round trip is 01:00.
        if let Some(candidate) = raw.earliest() {
            assert_ne!(
                candidate
                    .with_timezone(&Utc)
                    .with_timezone(&chrono::Local)
                    .naive_local(),
                midnight
            );
        }
        assert!(valid_instant(&chrono::Local, &midnight).is_none());
        assert_eq!(from.with_timezone(&chrono::Local).hour(), 1);
    } else {
        let LocalResult::Ambiguous(first, second) = raw else {
            panic!("expected a midnight fold");
        };
        let mut candidates = [first.with_timezone(&Utc), second.with_timezone(&Utc)];
        candidates.sort();
        assert_eq!(candidates, [from, from + Duration::hours(1)]);
        assert!(
            candidates
                .iter()
                .all(|instant| instant.with_timezone(&chrono::Local).naive_local() == midnight)
        );
        assert_eq!(valid_instant(&chrono::Local, &midnight), Some(from));
    }
    assert_eq!(super::super::local_day_bounds_utc(day), Some((from, to)));
    assert_eq!((to - from).num_hours(), hours);
    let previous = super::super::local_day_bounds_utc(day.pred_opt().unwrap()).unwrap();
    let next = super::super::local_day_bounds_utc(day.succ_opt().unwrap()).unwrap();
    assert_eq!(previous.1, from);
    assert_eq!((previous.1 - previous.0).num_hours(), 24);
    assert_eq!(next.0, to);
    check_timeline(day.pred_opt().unwrap(), previous.0, previous.1);
    check_timeline(day, from, to);
    println!("day boundary case passed");
}

#[cfg(unix)]
fn check_timeline(day: NaiveDate, from: DateTime<Utc>, to: DateTime<Utc>) {
    let fixture = crate::timeline::fixture::Fixture::empty();
    fixture.segment(
        "boundary-fixture",
        "claude",
        true,
        None,
        from - Duration::minutes(2),
        to + Duration::minutes(2),
    );
    fixture.active(from - Duration::minutes(2), to + Duration::minutes(2));
    let value = crate::timeline::read(
        &fixture.storage,
        Some(&fixture.activity),
        &crate::owner_time_day::LocalCalendar,
        day,
        to + Duration::hours(2),
        &crate::attribution::AttribCfg::default(),
    )
    .unwrap();
    assert_eq!(
        (value.from, value.to),
        (from.timestamp() as f64, to.timestamp() as f64)
    );
    assert_eq!(value.segments.len(), 1);
    assert_eq!(
        (value.segments[0].from, value.segments[0].to),
        (value.from, value.to)
    );
    assert_eq!(value.owner.len(), 1);
    assert_eq!(
        (value.owner[0].from, value.owner[0].to),
        (value.from, value.to)
    );
    assert_eq!(
        fixture.activity.seconds_on_local_date(day).unwrap(),
        (to - from).num_seconds() as f64
    );
}
