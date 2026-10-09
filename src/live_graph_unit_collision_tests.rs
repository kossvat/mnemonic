use super::*;

fn add_unit(project: &std::path::Path, unit: &str) {
    let path = project.join("openspec/changes").join(unit);
    std::fs::create_dir_all(&path).unwrap();
    std::fs::write(path.join("tasks.md"), "- [ ] T01: Fixture work\n").unwrap();
}

fn check_unit(f: &Fixture, project: i64, unit: &str, expected: &[&str]) {
    let source = format!("project:{project}");
    let target = unit_key(project, unit).unwrap();
    let snapshot = f.live.graph_snapshot(Some(&source)).unwrap().unwrap();
    let index = snapshot
        .payload
        .n
        .iter()
        .position(|node| node.key == target)
        .unwrap();
    let node = &snapshot.payload.n[index];
    let edge = snapshot
        .payload
        .e
        .iter()
        .find(|edge| usize::from(edge.1) == index && edge.2 == 0)
        .unwrap();
    let ids = f
        .live
        .graph_edge_memories(
            &source,
            &target,
            LinkKind::Contains,
            EdgePage {
                offset: 0,
                limit: 100,
            },
        )
        .unwrap()
        .unwrap();
    assert_eq!(ids, expected);
    assert_eq!(edge.3 as usize, ids.len());
    assert_eq!(node.last_activity.is_some(), !expected.is_empty());
}

fn add_proposal(project: &std::path::Path, unit: &str) {
    let path = project.join("openspec/changes").join(unit);
    std::fs::create_dir_all(&path).unwrap();
    std::fs::write(path.join("proposal.md"), "# Fixture proposal\n").unwrap();
}

#[test]
fn curated_graph_runtime_hashed_units_keep_exact_ids_and_count_click_agreement() {
    let f = Fixture::new();
    let id = f.project("fixture");
    let workspace = f.root.path().join("workspace");
    let project = workspace.join("fixture");
    let names = [
        ("phase one".to_string(), "space-memory"),
        ("phase:two".into(), "colon-memory"),
        ("\u{8a08}\u{753b}".into(), "unicode-memory"),
        ("r".repeat(161), "long-memory"),
    ];
    for (name, _) in &names {
        add_proposal(&project, name);
    }
    let at = f.now.timestamp() - 420;
    {
        let conn = f.storage.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO entities(id,name,entity_type,mention_count)
            VALUES ('project-entity','fixture','project',1)",
            [],
        )
        .unwrap();
        for (index, (name, memory)) in names.iter().enumerate() {
            let entity = format!("named-unit-{index}");
            conn.execute(
                "INSERT INTO entities(id,name,entity_type,mention_count)
                VALUES (?1,?2,'concept',1)",
                rusqlite::params![entity, name],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO memories(id,timestamp,title,content,memory_type,source)
                VALUES (?1,datetime(?2,'unixepoch'),'Unit memory','Evidence','decision','test')",
                rusqlite::params![memory, at],
            )
            .unwrap();
            for named in [entity.as_str(), "project-entity"] {
                conn.execute(
                    "INSERT INTO memory_entities(memory_id,entity_id) VALUES (?1,?2)",
                    rusqlite::params![memory, named],
                )
                .unwrap();
            }
        }
    }
    f.live.configure_plans(
        f.storage.clone(),
        crate::config::ProjectsConfig {
            workspace: Some(workspace),
            ..Default::default()
        },
    );
    f.refresh(0);
    let plans = f.live.project_plans();
    let slots = f.storage.graph_slots().unwrap();
    let mut keys = BTreeSet::new();
    for (name, memory) in &names {
        assert!(plans[&id].units.iter().any(|unit| &unit.id == name));
        let key = unit_key(id, name).unwrap();
        assert!(key.starts_with(&format!("unit:{id}:h:")));
        assert!(keys.insert(key.clone()));
        assert!(slots.iter().any(|slot| slot.key == key));
        assert!(f.live.graph_snapshot(Some(&key)).unwrap().is_some());
        check_unit(&f, id, name, &[*memory]);
    }
    // An exact raw ID and its canonical-name alias remain separate graph
    // identities; ambiguous evidence names neither until disambiguated.
    add_proposal(&project, "phase-one");
    add_proposal(&project, "phase two");
    f.refresh(60);
    let before = f.changes();
    for name in ["phase one", "phase-one", "phase:two", "phase two"] {
        keys.insert(unit_key(id, name).unwrap());
        check_unit(&f, id, name, &[]);
    }
    check_unit(&f, id, &names[2].0, &[names[2].1]);
    check_unit(&f, id, &names[3].0, &[names[3].1]);
    assert_eq!(keys.len(), 6);
    assert_eq!(f.changes(), before);
    let after = f.storage.graph_slots().unwrap();
    assert!(slots.iter().all(|slot| after.contains(slot)));
}

#[test]
fn curated_graph_runtime_ambiguous_units_have_zero_count_and_empty_clicks() {
    let f = Fixture::new();
    let id = f.project("fixture");
    let workspace = f.root.path().join("workspace");
    let project = workspace.join("fixture");
    let prefix = "work".repeat(15);
    let long_one = format!("{prefix}-one");
    let long_two = format!("{prefix}-two");
    for unit in ["my-feature", long_one.as_str(), "safe-release"] {
        add_unit(&project, unit);
    }
    let at = f.now.timestamp() - 420;
    {
        let conn = f.storage.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO entities(id,name,entity_type,mention_count)
            VALUES ('project-entity','fixture','project',1)",
            [],
        )
        .unwrap();
        for (entity, name, memory) in [
            ("feature-entity", "feature", "feature-memory"),
            ("long-entity", prefix.as_str(), "long-memory"),
            ("safe-entity", "safe-release", "safe-memory"),
        ] {
            conn.execute(
                "INSERT INTO entities(id,name,entity_type,mention_count)
                VALUES (?1,?2,'concept',1)",
                rusqlite::params![entity, name],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO memories(id,timestamp,title,content,memory_type,source)
                VALUES (?1,datetime(?2,'unixepoch'),'Unit memory','Evidence','decision','test')",
                rusqlite::params![memory, at],
            )
            .unwrap();
            for named in [entity, "project-entity"] {
                conn.execute(
                    "INSERT INTO memory_entities(memory_id,entity_id) VALUES (?1,?2)",
                    rusqlite::params![memory, named],
                )
                .unwrap();
            }
        }
    }
    f.live.configure_plans(
        f.storage.clone(),
        crate::config::ProjectsConfig {
            workspace: Some(workspace),
            ..Default::default()
        },
    );
    f.refresh(0);
    // Before ambiguity, exact raw IDs with prefix removal and capping still
    // carry their evidence, proving the runtime lookup uses the raw key.
    check_unit(&f, id, "my-feature", &["feature-memory"]);
    check_unit(&f, id, &long_one, &["long-memory"]);
    check_unit(&f, id, "safe-release", &["safe-memory"]);
    add_unit(&project, "feature");
    add_unit(&project, &long_two);
    f.refresh(60);
    let before = f.changes();
    for unit in [
        "my-feature",
        "feature",
        long_one.as_str(),
        long_two.as_str(),
    ] {
        check_unit(&f, id, unit, &[]);
    }
    check_unit(&f, id, "safe-release", &["safe-memory"]);
    assert_eq!(f.changes(), before);
}
