//! Archive masks the legacy cache without destroying direct or carried time.
use super::*;
use crate::activity::{DayAttribution, ProjectTimeData, ProjectTimeRow};
use crate::graph::{Entity, EntityType};
use crate::timeline::fixture::Fixture;
use chrono::{DateTime, Utc};

fn memory(f: &Fixture, entity: &str, id: &str, at: DateTime<Utc>) {
    let conn = f.storage.conn.lock().unwrap();
    conn.execute(
        "INSERT INTO memories (id,timestamp,title,content,memory_type,source)
         VALUES (?1,?2,'Attribution fixture','Synthetic evidence','note','test')",
        rusqlite::params![id, at.to_rfc3339()],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO memory_entities(memory_id,entity_id) VALUES (?1,?2)",
        rusqlite::params![id, entity],
    )
    .unwrap();
}

fn week(day: &DayAttribution) -> ProjectTimeData {
    ProjectTimeData {
        rows: day
            .0
            .iter()
            .map(|(key, seconds, confidence)| ProjectTimeRow {
                project_key: key.clone(),
                today_seconds: *seconds,
                week_seconds: *seconds,
                week: [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, *seconds],
                confidence: confidence.clone(),
            })
            .collect(),
        unattributed_today: day.1,
        unattributed_week: day.1,
    }
}

#[test]
fn curated_graph_archive_legacy_recompute_and_day_rollover_preserve_undo() {
    let f = Fixture::empty();
    let day = NaiveDate::from_ymd_opt(2026, 9, 25).unwrap();
    let (start, end) = local_day_bounds_utc(day).unwrap();
    let at =
        |hour, minute| start + chrono::Duration::hours(hour) + chrono::Duration::minutes(minute);
    let archived = f
        .storage
        .upsert_entity(&Entity {
            name: "archive-work".into(),
            entity_type: EntityType::Project,
        })
        .unwrap();
    let kept = f
        .storage
        .upsert_entity(&Entity {
            name: "kept-work".into(),
            entity_type: EntityType::Project,
        })
        .unwrap();
    for (entity, prefix, hour) in [(&archived, "archived", 9), (&kept, "kept", 14)] {
        for minute in 1..=2 {
            memory(&f, entity, &format!("{prefix}-{minute}"), at(hour, minute));
        }
    }
    for (from, to) in [
        (at(9, 0), at(9, 15)),
        (at(10, 0), at(10, 10)),
        (at(14, 0), at(14, 5)),
        (at(18, 0), at(18, 10)),
    ] {
        f.active(from, to);
    }
    let cfg = AttribCfg::default();
    recompute_day(&f.activity, &f.storage, day, &cfg).unwrap();
    let original = f.activity.project_attribution_for_day(day).unwrap();
    let seconds = |key: &str| original.0.iter().find(|row| row.0 == key).unwrap().1;
    assert_eq!(
        (seconds(&archived), seconds(&kept), original.1),
        (1500.0, 300.0, 600.0)
    );
    // The archived project's total includes 900 direct and 600 carried
    // seconds; both source signals and carry anchors must survive refresh.
    let original_week = week(&original);
    assert!(f.storage.entity_archive_set(&archived, true, 10).unwrap());
    assert!(
        f.storage
            .project_signals_in_window(start, end, MIN_PROJECT_MEMS)
            .unwrap()
            .iter()
            .all(|signal| signal.project_key != archived)
    );
    assert!(
        f.storage
            .project_mem_times_in_window(start, end, MIN_PROJECT_MEMS)
            .unwrap()
            .iter()
            .all(|(key, _)| key != &archived)
    );
    for _ in 0..2 {
        recompute_day(&f.activity, &f.storage, day, &cfg).unwrap();
        assert_eq!(
            f.activity.project_attribution_for_day(day).unwrap(),
            original
        );
        let before = (f.changes(), f.stores());
        let visible = f
            .storage
            .rekeyed_day_with_unattributed(original.clone())
            .unwrap();
        assert_eq!(visible.0.len(), 1);
        assert_eq!(
            (&visible.0[0].0, visible.0[0].1, visible.1),
            (&kept, 300.0, 2100.0)
        );
        let visible_week = f.storage.rekeyed_time(&original_week).unwrap();
        assert_eq!(visible_week.rows.len(), 1);
        assert_eq!(visible_week.unattributed_today, 2100.0);
        assert_eq!(visible_week.unattributed_week, 2100.0);
        let journal = crate::journal::collect(&f.storage, &f.activity, day).unwrap();
        assert!(
            journal
                .projects
                .iter()
                .all(|project| project.key != archived)
        );
        assert_eq!(journal.unattributed_seconds, 2100.0);
        assert_eq!((f.changes(), f.stores()), before);
    }

    // Tomorrow's normal worker pass does not reconstruct yesterday. Undo
    // must reveal yesterday's saved allocation without a historical pass.
    let tomorrow = day.succ_opt().unwrap();
    let tomorrow_start = local_day_bounds_utc(tomorrow).unwrap().0;
    f.active(
        tomorrow_start + chrono::Duration::hours(9),
        tomorrow_start + chrono::Duration::hours(10),
    );
    recompute_day(&f.activity, &f.storage, tomorrow, &cfg).unwrap();
    assert_eq!(
        f.activity.project_attribution_for_day(day).unwrap(),
        original
    );
    assert!(f.storage.entity_archive_set(&archived, false, 11).unwrap());
    let before = (f.changes(), f.stores());
    assert_eq!(
        f.storage
            .rekeyed_day_with_unattributed(original.clone())
            .unwrap(),
        original
    );
    let restored_week = f.storage.rekeyed_time(&original_week).unwrap();
    assert_eq!(restored_week.rows, original_week.rows);
    assert_eq!(
        restored_week.unattributed_today,
        original_week.unattributed_today
    );
    assert_eq!(
        restored_week.unattributed_week,
        original_week.unattributed_week
    );
    let journal = crate::journal::collect(&f.storage, &f.activity, day).unwrap();
    assert!(
        journal
            .projects
            .iter()
            .any(|project| project.key == archived && project.seconds == 1500.0)
    );
    assert_eq!(journal.unattributed_seconds, 600.0);
    assert_eq!(
        f.activity.project_attribution_for_day(day).unwrap(),
        original
    );
    assert_eq!((f.changes(), f.stores()), before);
}
