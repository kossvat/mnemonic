use super::*;

#[path = "curated_graph_unit_ownership_tests.rs"]
mod ownership_tests;

#[test]
fn curated_graph_unit_evidence_is_scoped_to_project_and_resolves_merges_and_aliases() {
    let f = Fixture::new();
    let alpha = f.project("alpha", None);
    let beta = f.project("beta", None);
    let old = f.project("old-alpha", None);
    f.storage.project_merge(old, alpha).unwrap();
    f.entity("name-alias", "alpha-name", "project");
    f.storage
        .project_alias_set(
            "alpha-name",
            alpha,
            crate::storage::projects::AliasKind::Name,
        )
        .unwrap();
    f.entity("unit", "release", "concept");
    f.entity("near", "release-extra", "concept");
    f.memory(
        "alpha-one",
        100,
        &["alpha", "old-alpha", "name-alias", "unit"],
    );
    f.memory("alpha-old", 150, &["old-alpha", "unit"]);
    f.memory("alpha-alias", 175, &["name-alias", "unit"]);
    f.memory("beta-one", 200, &["beta", "unit"]);
    f.memory("unscoped", 300, &["unit"]);
    f.memory("summary", 400, &["beta", "unit"]);
    f.memory("superseded", 500, &["beta", "unit"]);
    f.memory("not-exact", 600, &["alpha", "near"]);
    {
        let conn = f.storage.conn.lock().unwrap();
        conn.execute(
            "UPDATE memories SET memory_type='session_summary' WHERE id='summary'",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE memories SET superseded_by='beta-one' WHERE id='superseded'",
            [],
        )
        .unwrap();
    }
    let before = f.storage.conn.lock().unwrap().total_changes();
    let units = [
        (alpha, "release".into()),
        (beta, "release".into()),
        (old, "release".into()),
    ];
    let found = f.storage.project_unit_memory_activity(&units).unwrap();
    assert_eq!(found.len(), 2);
    assert_eq!(found[&(alpha, "release".into())], (3, Some(175)));
    assert_eq!(found[&(beta, "release".into())], (1, Some(200)));
    let page = EdgePage {
        offset: 0,
        limit: 100,
    };
    let ids = f
        .storage
        .curated_graph_unit_memories(old, "release", &units, page)
        .unwrap();
    assert_eq!(ids, ["alpha-alias", "alpha-old", "alpha-one"]);
    assert_eq!(
        f.storage
            .curated_graph_unit_memories(beta, "release", &units, page)
            .unwrap(),
        ["beta-one"]
    );
    assert!(
        f.storage
            .curated_graph_unit_memories(alpha, "missing", &units, page)
            .unwrap()
            .is_empty()
    );
    assert!(
        f.storage
            .curated_graph_unit_memories(
                alpha,
                "release",
                &units,
                EdgePage {
                    offset: usize::MAX,
                    limit: usize::MAX,
                }
            )
            .unwrap()
            .is_empty()
    );
    assert_eq!(f.storage.conn.lock().unwrap().total_changes(), before);
}

#[test]
fn curated_graph_unit_click_pages_cap_at_one_hundred_ids() {
    let f = Fixture::new();
    let alpha = f.project("alpha", None);
    f.entity("unit", "release", "concept");
    for n in 0..105 {
        f.memory(&format!("m{n:03}"), n, &["alpha", "unit"]);
    }
    let units = [(alpha, "release".into())];
    let first = f
        .storage
        .curated_graph_unit_memories(
            alpha,
            "release",
            &units,
            EdgePage {
                offset: 0,
                limit: usize::MAX,
            },
        )
        .unwrap();
    let last = f
        .storage
        .curated_graph_unit_memories(
            alpha,
            "release",
            &units,
            EdgePage {
                offset: 100,
                limit: usize::MAX,
            },
        )
        .unwrap();
    assert_eq!((first.len(), last.len()), (100, 5));
    assert_eq!(last[0], "m100");
}

#[test]
fn curated_graph_unit_name_collisions_have_no_guessed_count_activity_or_click_ids() {
    let f = Fixture::new();
    let alpha = f.project("alpha", None);
    let beta = f.project("beta", None);
    let prefix = "work".repeat(15);
    let long_one = format!("{prefix}-one");
    let long_two = format!("{prefix}-two");
    f.entity("feature-entity", "feature", "concept");
    f.entity("long-entity", &prefix, "concept");
    f.entity("safe-entity", "safe-release", "concept");
    f.memory("alpha-feature", 100, &["alpha", "feature-entity"]);
    f.memory("alpha-long", 150, &["alpha", "long-entity"]);
    f.memory("alpha-safe", 175, &["alpha", "safe-entity"]);
    f.memory("beta-feature", 200, &["beta", "feature-entity"]);
    let units = [
        (alpha, "my-feature".to_string()),
        (alpha, "feature".into()),
        (alpha, long_one.clone()),
        (alpha, long_two.clone()),
        (alpha, "safe-release".into()),
        (beta, "feature".into()),
    ];
    assert_eq!(canonical_name("my-feature"), canonical_name("feature"));
    assert_eq!(canonical_name(&long_one), canonical_name(&long_two));
    let before = f.storage.conn.lock().unwrap().total_changes();
    let found = f.storage.project_unit_memory_activity(&units).unwrap();
    assert_eq!(found.len(), 2);
    assert_eq!(found[&(alpha, "safe-release".into())], (1, Some(175)));
    assert_eq!(found[&(beta, "feature".into())], (1, Some(200)));
    let page = EdgePage {
        offset: 0,
        limit: 100,
    };
    for (_, unit) in &units[..4] {
        assert!(!found.contains_key(&(alpha, unit.clone())));
        assert!(
            f.storage
                .curated_graph_unit_memories(alpha, unit, &units, page)
                .unwrap()
                .is_empty()
        );
    }
    assert_eq!(
        f.storage
            .curated_graph_unit_memories(beta, "feature", &units, page)
            .unwrap(),
        ["beta-feature"]
    );
    // A canonical alias is not a substitute for the exact requested unit ID.
    assert!(
        f.storage
            .curated_graph_unit_memories(beta, "Feature", &units, page)
            .unwrap()
            .is_empty()
    );
    assert_eq!(f.storage.conn.lock().unwrap().total_changes(), before);
}

#[test]
fn curated_graph_unit_collisions_follow_merges_but_exact_duplicates_deduplicate() {
    let f = Fixture::new();
    let alpha = f.project("alpha", None);
    let old = f.project("old-alpha", None);
    f.entity("feature-entity", "feature", "concept");
    f.memory("alpha-feature", 100, &["alpha", "feature-entity"]);
    f.memory("old-feature", 200, &["old-alpha", "feature-entity"]);
    let distinct = [(alpha, "my-feature".into()), (old, "feature".into())];
    let separate = f.storage.project_unit_memory_activity(&distinct).unwrap();
    assert_eq!(separate[&(alpha, "my-feature".into())], (1, Some(100)));
    assert_eq!(separate[&(old, "feature".into())], (1, Some(200)));
    f.storage.project_merge(old, alpha).unwrap();
    let before = f.storage.conn.lock().unwrap().total_changes();
    assert!(
        f.storage
            .project_unit_memory_activity(&distinct)
            .unwrap()
            .is_empty()
    );
    let page = EdgePage {
        offset: 0,
        limit: 100,
    };
    for (project, unit) in &distinct {
        assert!(
            f.storage
                .curated_graph_unit_memories(*project, unit, &distinct, page)
                .unwrap()
                .is_empty()
        );
    }
    let duplicate = [(alpha, "my-feature".into()), (old, "my-feature".into())];
    let found = f.storage.project_unit_memory_activity(&duplicate).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[&(alpha, "my-feature".into())], (2, Some(200)));
    assert_eq!(
        f.storage
            .curated_graph_unit_memories(old, "my-feature", &duplicate, page)
            .unwrap(),
        ["alpha-feature", "old-feature"]
    );
    let case_collision = [(alpha, "feature".into()), (old, "Feature".into())];
    assert!(
        f.storage
            .project_unit_memory_activity(&case_collision)
            .unwrap()
            .is_empty()
    );
    assert_eq!(f.storage.conn.lock().unwrap().total_changes(), before);
}
