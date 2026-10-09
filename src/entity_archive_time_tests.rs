//! Undo repairs live cache history without changing earlier attribution policy.
use super::*;
use crate::graph::{Entity, EntityType};
use crate::timeline::fixture::{Fixture, Zone, at};
use std::sync::{Arc, mpsc};
use std::time::Duration as Wait;

fn instant(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> DateTime<Utc> {
    NaiveDate::from_ymd_opt(year, month, day)
        .unwrap()
        .and_hms_opt(hour, minute, 0)
        .unwrap()
        .and_utc()
}

fn earlier() -> DateTime<Utc> {
    instant(2010, 1, 12, 9, 0)
}

fn fixture() -> (Fixture, String, i64) {
    let f = Fixture::empty();
    let entity = f
        .storage
        .upsert_entity(&Entity {
            name: "adopted-work".into(),
            entity_type: EntityType::Project,
        })
        .unwrap();
    f.storage.entity_archive_set(&entity, true, 10).unwrap();
    let id = f
        .storage
        .project_upsert("adopted-work", "adopted-work", None)
        .unwrap();
    f.storage.project_link_entity(id, "adopted-work").unwrap();
    let first = instant(2010, 1, 2, 0, 0);
    f.segment(
        "live-rule-start",
        "codex",
        true,
        Some(id),
        first,
        first + Duration::minutes(1),
    );
    f.active(earlier(), earlier() + Duration::minutes(10));
    f.memory("archived-fallback", earlier() + Duration::minutes(2), id);
    assert_eq!(
        f.storage
            .project_by_key("adopted-work")
            .unwrap()
            .unwrap()
            .entity_id,
        Some(entity.clone())
    );
    f.active(at(9, 0), at(9, 10));
    f.storage
        .override_add(None, Some((at(9, 0), at(9, 10))), Some(id))
        .unwrap();
    for day in [earlier().date_naive(), at(0, 0).date_naive()] {
        recompute_day(
            &f.activity,
            &f.storage,
            &Zone::UTC,
            day,
            &AttribCfg::default(),
        )
        .unwrap();
    }
    (f, entity, id)
}

fn rows(f: &Fixture, day: NaiveDate) -> (BTreeMap<String, f64>, f64) {
    let (rows, unassigned) = f
        .storage
        .rekeyed_day_with_unattributed(f.activity.project_attribution_for_day(day).unwrap())
        .unwrap();
    (
        rows.into_iter()
            .map(|(key, seconds, _)| (key, seconds))
            .collect(),
        unassigned,
    )
}

fn repair(f: &Fixture) -> Result<usize> {
    crate::owner_time_day::repair_live_after_entity_undo(
        &f.storage,
        Some(&f.activity),
        &Zone::UTC,
        at(23, 0) + Duration::days(1),
        &AttribCfg::default(),
    )
}

#[test]
fn curated_graph_archive_live_undo_repairs_existing_history_and_keeps_explicit_time() {
    let (f, entity, id) = fixture();
    let old = earlier().date_naive();
    let recent = at(0, 0).date_naive();
    assert_eq!(rows(&f, old), (BTreeMap::new(), 600.0));
    assert_eq!(
        rows(&f, recent),
        (BTreeMap::from([(registry_key(id), 600.0)]), 0.0)
    );
    // A later correction still wins when history is repaired.
    f.storage
        .override_add(
            None,
            Some((earlier(), earlier() + Duration::minutes(5))),
            Some(f.notes),
        )
        .unwrap();
    let legacy = instant(2009, 12, 31, 0, 0).date_naive();
    let future = instant(2027, 1, 1, 0, 0).date_naive();
    f.activity
        .write_day(legacy, &[(&entity, 120.0, "low")], 30.0)
        .unwrap();
    f.activity
        .write_day(future, &[(&entity, 90.0, "low")], 15.0)
        .unwrap();
    let legacy_rows = f.activity.project_attribution_for_day(legacy).unwrap();
    let future_rows = f.activity.project_attribution_for_day(future).unwrap();
    let dates = f.activity.cached_attribution_days().unwrap();
    let tomorrow = recent.succ_opt().unwrap();
    recompute_day(
        &f.activity,
        &f.storage,
        &Zone::UTC,
        tomorrow,
        &AttribCfg::default(),
    )
    .unwrap();
    f.storage.entity_archive_set(&entity, false, 11).unwrap();
    let overrides = f.storage.overrides().unwrap();
    let source = f.stores().remove(0);
    assert_eq!(repair(&f).unwrap(), 3);
    assert_eq!(
        rows(&f, old),
        (
            BTreeMap::from([(entity.clone(), 300.0), (registry_key(f.notes), 300.0)]),
            0.0
        )
    );
    assert_eq!(
        rows(&f, recent),
        (BTreeMap::from([(entity.clone(), 600.0)]), 0.0)
    );
    assert_eq!(
        f.activity.project_attribution_for_day(legacy).unwrap(),
        legacy_rows
    );
    assert_eq!(
        f.activity.project_attribution_for_day(future).unwrap(),
        future_rows
    );
    let mut expected_dates = dates;
    expected_dates.push(tomorrow);
    expected_dates.sort();
    assert_eq!(
        f.activity.cached_attribution_days().unwrap(),
        expected_dates
    );
    assert_eq!(f.storage.overrides().unwrap(), overrides);
    assert_eq!(f.stores().remove(0), source);
    let timeline = crate::timeline::read(
        &f.storage,
        Some(&f.activity),
        &Zone::UTC,
        old,
        at(23, 0),
        &AttribCfg::default(),
    )
    .unwrap();
    let mut owner = BTreeMap::new();
    for range in timeline.owner {
        for share in range.assignments {
            *owner.entry(share.project.unwrap()).or_insert(0.0) +=
                (range.to - range.from) * share.share;
        }
    }
    assert_eq!(
        owner,
        BTreeMap::from([("adopted-work".into(), 300.0), ("notes".into(), 300.0)])
    );
    let hours = hours_of(
        &f.activity,
        &f.storage,
        &Zone::UTC,
        at(23, 0),
        &AttribCfg::default(),
    )
    .unwrap();
    assert_eq!(hours.projects[&id].owner_s, 600);
}

#[test]
fn curated_graph_archive_live_failed_partial_repair_retries_after_unchanged_undo() {
    let (f, entity, _) = fixture();
    f.storage.entity_archive_set(&entity, false, 11).unwrap();
    let conn = rusqlite::Connection::open(&f.activity_path).unwrap();
    conn.execute_batch(
        "CREATE TRIGGER fixture_repair_failure BEFORE INSERT ON project_attribution
        WHEN NEW.day = '2026-09-29' BEGIN SELECT RAISE(FAIL, 'fixture repair failure'); END;",
    )
    .unwrap();
    assert!(repair(&f).is_err());
    assert_eq!(
        rows(&f, earlier().date_naive()),
        (BTreeMap::from([(entity.clone(), 600.0)]), 0.0)
    );
    conn.execute_batch("DROP TRIGGER fixture_repair_failure")
        .unwrap();
    assert!(!f.storage.entity_archive_set(&entity, false, 12).unwrap());
    assert_eq!(repair(&f).unwrap(), 2);
    assert_eq!(
        rows(&f, at(0, 0).date_naive()),
        (BTreeMap::from([(entity, 600.0)]), 0.0)
    );
}

#[test]
fn curated_graph_archive_live_repair_skips_missing_activity_and_pre_live_history() {
    let f = Fixture::empty();
    let day = at(0, 0).date_naive();
    f.activity
        .write_day(day, &[("legacy-work", 300.0, "low")], 120.0)
        .unwrap();
    let before = (f.changes(), f.stores());
    assert_eq!(
        repair_live_after_entity_undo(
            &f.storage,
            None,
            &Zone::UTC,
            at(23, 0),
            &AttribCfg::default()
        )
        .unwrap(),
        0
    );
    assert_eq!(repair(&f).unwrap(), 0);
    assert_eq!((f.changes(), f.stores()), before);
}

#[test]
fn curated_graph_archive_live_repair_waits_for_an_archived_worker_write() {
    let (f, entity, _) = fixture();
    let second_activity = Arc::new(ActivityStore::open(&f.activity_path).unwrap());
    let (ready_tx, ready_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let (started_tx, started_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    std::thread::scope(|scope| {
        let worker_fixture = &f;
        let worker = scope.spawn(move || {
            let _write = worker_fixture.storage.attribution_write_lock().unwrap();
            let bounds = Zone::UTC.bounds(earlier().date_naive()).unwrap();
            let made = compute(
                &worker_fixture.activity,
                &worker_fixture.storage,
                bounds,
                &AttribCfg::default(),
            )
            .unwrap();
            assert!(made.rows.is_empty());
            ready_tx.send(()).unwrap();
            release_rx.recv_timeout(Wait::from_secs(3)).unwrap();
            // This is the stale compute+write interval protected by the same
            // guard in recompute_day, using the original activity connection.
            worker_fixture
                .activity
                .write_day(earlier().date_naive(), &[], made.unattributed)
                .unwrap();
        });
        ready_rx.recv_timeout(Wait::from_secs(3)).unwrap();
        f.storage.entity_archive_set(&entity, false, 11).unwrap();
        let undo_fixture = &f;
        let undo = scope.spawn(move || {
            started_tx.send(()).unwrap();
            let repaired = repair_live_after_entity_undo(
                &undo_fixture.storage,
                Some(&second_activity),
                &Zone::UTC,
                at(23, 0),
                &AttribCfg::default(),
            )
            .unwrap();
            done_tx.send(repaired).unwrap();
        });
        started_rx.recv_timeout(Wait::from_secs(3)).unwrap();
        let pending = done_rx.recv_timeout(Wait::from_millis(30));
        release_tx.send(()).unwrap();
        worker.join().unwrap();
        undo.join().unwrap();
        assert!(matches!(pending, Err(mpsc::RecvTimeoutError::Timeout)));
        assert_eq!(done_rx.recv_timeout(Wait::from_secs(3)).unwrap(), 2);
    });
    assert_eq!(
        rows(&f, earlier().date_naive()),
        (BTreeMap::from([(entity, 600.0)]), 0.0)
    );
}

#[test]
fn curated_graph_archive_live_worker_uses_the_same_write_guard() {
    let (f, _, _) = fixture();
    let (started_tx, started_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let write = f.storage.attribution_write_lock().unwrap();
    std::thread::scope(|scope| {
        let worker = scope.spawn(|| {
            started_tx.send(()).unwrap();
            recompute_day(
                &f.activity,
                &f.storage,
                &Zone::UTC,
                earlier().date_naive(),
                &AttribCfg::default(),
            )
            .unwrap();
            done_tx.send(()).unwrap();
        });
        started_rx.recv_timeout(Wait::from_secs(3)).unwrap();
        let pending = done_rx.recv_timeout(Wait::from_millis(30));
        drop(write);
        worker.join().unwrap();
        assert!(matches!(pending, Err(mpsc::RecvTimeoutError::Timeout)));
        done_rx.recv_timeout(Wait::from_secs(3)).unwrap();
    });
}
