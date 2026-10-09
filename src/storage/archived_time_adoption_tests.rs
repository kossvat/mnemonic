//! Registry adoption keeps explicit attribution separate from archived history.
use super::{graph_rows, time_row};
use crate::activity::{DayAttribution, ProjectTimeData};
use crate::attribution::AttribCfg;
use crate::graph::{Entity, EntityType};
use crate::owner_time::{Trust, registry_key};
use crate::owner_time_day::{self, Calendar};
use crate::timeline::fixture::{Fixture, Zone, at};

fn timeline(f: &Fixture) -> crate::timeline::ProjectTimeline {
    crate::timeline::read(
        &f.storage,
        Some(&f.activity),
        &Zone::UTC,
        at(0, 0).date_naive(),
        at(23, 0),
        &AttribCfg::default(),
    )
    .unwrap()
}

fn assert_timeline(f: &Fixture, attributed: f64, unassigned: f64) {
    let result = timeline(f);
    let mut assigned_seconds = 0.0;
    let mut unassigned_seconds = 0.0;
    for range in result.owner {
        for assignment in range.assignments {
            let seconds = (range.to - range.from) * assignment.share;
            if assignment.project.as_deref() == Some("phantom-board") {
                assigned_seconds += seconds;
            } else {
                assert!(assignment.project.is_none());
                unassigned_seconds += seconds;
            }
        }
    }
    assert_eq!(
        (assigned_seconds, unassigned_seconds),
        (attributed, unassigned)
    );
}

#[test]
fn curated_graph_archive_adoption_preserves_fresh_explicit_time_and_cached_provenance() {
    for manual in [true, false] {
        let f = Fixture::empty();
        let entity = f
            .storage
            .upsert_entity(&Entity {
                name: "phantom-board".into(),
                entity_type: EntityType::Project,
            })
            .unwrap();
        assert!(f.storage.entity_archive_set(&entity, true, 10).unwrap());
        let id = f
            .storage
            .project_upsert("phantom-board", "phantom-board", None)
            .unwrap();
        f.storage.project_link_entity(id, "phantom-board").unwrap();
        f.memory("adoption-fallback", at(11, 5), id);
        assert_eq!(
            f.storage
                .project_by_key("phantom-board")
                .unwrap()
                .unwrap()
                .entity_id,
            Some(entity.clone()),
            "The memory fixture must keep the archived entity binding",
        );
        f.active(at(9, 0), at(9, 10));
        f.active(at(11, 0), at(11, 10));
        if manual {
            f.storage
                .override_add(None, Some((at(9, 0), at(9, 10))), Some(id))
                .unwrap();
        } else {
            f.segment(
                "adopted-work",
                "claude",
                false,
                Some(id),
                at(9, 0),
                at(9, 10),
            );
            f.prompt("adopted-work", at(9, 0));
        }
        let historical = graph_rows(&f.storage);
        let overrides = f.storage.overrides().unwrap();
        let key = registry_key(id);
        let keys = f.storage.attribution_keys_now().unwrap();
        assert!(keys.is_archived(&entity));
        assert!(!keys.is_archived(&key));
        assert_eq!(f.storage.attribution_keys().unwrap()[&id], key);
        assert!(
            f.storage
                .registry_signals(at(11, 0), at(11, 10))
                .unwrap()
                .is_empty()
        );

        let day = at(0, 0).date_naive();
        let bounds = Zone::UTC.bounds(day).unwrap();
        let before = (f.changes(), f.stores());
        let fresh = owner_time_day::compute(&f.activity, &f.storage, bounds, &AttribCfg::default())
            .unwrap();
        assert_eq!(fresh.rows, [(key.clone(), 600.0, Trust::High)]);
        assert_eq!(fresh.unattributed, 600.0);
        assert_timeline(&f, 600.0, 600.0);
        assert_eq!((f.changes(), f.stores()), before);

        // The raw entity row predates adoption; the registry row is fresh
        // explicit evidence. Archive suppresses only the former.
        f.activity
            .write_day(
                day,
                &[(&entity, 120.0, "low"), (&key, 600.0, "high")],
                600.0,
            )
            .unwrap();
        let cached = f.activity.project_attribution_for_day(day).unwrap();
        let week = ProjectTimeData {
            rows: vec![
                time_row(&entity, 120.0, 120.0),
                time_row(&key, 600.0, 600.0),
            ],
            unattributed_today: 600.0,
            unattributed_week: 600.0,
        };
        let before = (f.changes(), f.stores());
        let visible = f
            .storage
            .rekeyed_day_with_unattributed(cached.clone())
            .unwrap();
        let expected: DayAttribution = (vec![(key.clone(), 600.0, Some("high".into()))], 720.0);
        assert_eq!(visible, expected);
        let visible_week = f.storage.rekeyed_time(&week).unwrap();
        assert_eq!(visible_week.rows, vec![week.rows[1].clone()]);
        assert_eq!(
            (
                visible_week.unattributed_today,
                visible_week.unattributed_week
            ),
            (720.0, 720.0)
        );
        assert_eq!((f.changes(), f.stores()), before);

        assert!(f.storage.entity_archive_set(&entity, false, 11).unwrap());
        let before = (f.changes(), f.stores());
        assert_eq!(f.storage.attribution_keys().unwrap()[&id], entity);
        let restored = f
            .storage
            .rekeyed_day_with_unattributed(cached.clone())
            .unwrap();
        assert_eq!(
            restored,
            (vec![(entity.clone(), 720.0, Some("high".into()))], 600.0)
        );
        let restored_week = f.storage.rekeyed_time(&week).unwrap();
        assert_eq!(restored_week.rows, vec![time_row(&entity, 720.0, 720.0)]);
        assert_eq!(
            (
                restored_week.unattributed_today,
                restored_week.unattributed_week
            ),
            (600.0, 600.0)
        );
        assert_eq!(f.activity.project_attribution_for_day(day).unwrap(), cached);
        let fresh = owner_time_day::compute(&f.activity, &f.storage, bounds, &AttribCfg::default())
            .unwrap();
        assert_eq!(fresh.rows.iter().map(|row| row.1).sum::<f64>(), 1200.0);
        assert_eq!(fresh.rows[0].0, entity);
        assert_eq!(fresh.unattributed, 0.0);
        assert_timeline(&f, 1200.0, 0.0);
        assert_eq!(f.storage.overrides().unwrap(), overrides);
        assert_eq!(graph_rows(&f.storage), historical);
        assert_eq!((f.changes(), f.stores()), before);
    }
}
