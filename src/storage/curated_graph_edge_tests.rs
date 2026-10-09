use super::*;

#[path = "curated_graph_edge_work_tests.rs"]
mod work_tests;

struct Fixture {
    _root: tempfile::TempDir,
    storage: Storage,
}
impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let storage = Storage::open(&root.path().join("fixture.db")).unwrap();
        Self {
            _root: root,
            storage,
        }
    }
    fn entity(&self, id: &str, name: &str, kind: &str) {
        self.storage
            .conn
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO entities(id,name,entity_type,mention_count) VALUES (?1,?2,?3,999)",
                params![id, name, kind],
            )
            .unwrap();
    }
    fn project(&self, name: &str, parent: Option<i64>) -> i64 {
        let id = self.storage.project_upsert(name, name, parent).unwrap();
        self.entity(name, name, "project");
        self.storage.project_link_entity(id, name).unwrap();
        id
    }
    fn memory(&self, id: &str, entities: &[&str]) {
        let conn = self.storage.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO memories(id,timestamp,title,content,memory_type,source)
            VALUES (?1,'2026-01-01T00:00:00Z','Fixture title','Fixture content','decision','test')",
            [id],
        )
        .unwrap();
        for entity in entities {
            conn.execute(
                "INSERT INTO memory_entities VALUES (?1,?2)",
                params![id, entity],
            )
            .unwrap();
        }
    }
}

fn clean(ids: Vec<String>) -> Vec<String> {
    // A failing disclosure assertion never prints a synthetic credential.
    assert!(
        ids.iter()
            .all(|id| crate::redaction::state::check_identities([id.as_str()]).is_ok())
    );
    ids
}

fn person(name: &str) -> String {
    let name = canonical_name(name);
    let id = git2::Oid::hash_object(git2::ObjectType::Blob, name.as_bytes()).unwrap();
    format!("entity:person:{id}")
}

fn assert_matches_background(f: &Fixture, tech: &[String], people: &[String]) {
    let before = f.storage.conn.lock().unwrap().total_changes();
    let (graph, expected) = {
        let conn = f.storage.conn.lock().unwrap();
        evidence(&conn, tech, people).unwrap()
    };
    for link in graph.links {
        let ids = &expected[&(link.source.clone(), link.target.clone(), link.kind)];
        assert_eq!(link.memory_count as usize, ids.len());
        for page in [
            EdgePage {
                offset: 0,
                limit: 1,
            },
            EdgePage {
                offset: 1,
                limit: usize::MAX,
            },
            EdgePage {
                offset: usize::MAX,
                limit: 100,
            },
            EdgePage {
                offset: 0,
                limit: 0,
            },
        ] {
            let expected: Vec<_> = ids
                .iter()
                .skip(page.offset.min(ids.len()))
                .take(page.limit.min(100))
                .cloned()
                .collect();
            let actual = clean(
                f.storage
                    .curated_graph_edge_memories(
                        &link.source,
                        &link.target,
                        link.kind,
                        tech,
                        people,
                        page,
                    )
                    .unwrap(),
            );
            assert_eq!(actual, expected);
            if link.kind == LinkKind::Relates {
                assert_eq!(
                    clean(
                        f.storage
                            .curated_graph_edge_memories(
                                &link.target,
                                &link.source,
                                link.kind,
                                tech,
                                people,
                                page,
                            )
                            .unwrap()
                    ),
                    expected
                );
            }
        }
    }
    assert_eq!(f.storage.conn.lock().unwrap().total_changes(), before);
}

#[test]
fn curated_graph_targeted_edge_pages_match_background_evidence_policies() {
    let f = Fixture::new();
    let alpha = f.project("alpha", None);
    let beta = f.project("beta", None);
    let former = f.project("former", Some(beta));
    f.storage.project_merge(former, alpha).unwrap();
    f.entity("alias", "alpha-alias", "project");
    f.storage
        .project_alias_set(
            "alpha-alias",
            alpha,
            super::super::projects::AliasKind::Name,
        )
        .unwrap();
    f.entity("tool", "rust", "tech");
    f.entity("rare", "sqlite", "tech");
    f.entity("person-a", "reviewer", "person");
    f.entity("person-b", "Reviewer", "person");
    f.entity("unit", "release", "concept");
    f.memory(
        "valid-a",
        &[
            "alpha", "former", "alias", "beta", "tool", "person-a", "unit",
        ],
    );
    f.memory("valid-b", &["former", "beta", "tool", "person-b", "unit"]);
    f.memory("valid-c", &["tool", "person-a"]);
    f.memory("rare-a", &["alpha", "rare"]);
    f.memory("rare-b", &["beta", "rare"]);
    f.memory(
        "noise-summary",
        &["alpha", "beta", "rare", "tool", "person-a", "unit"],
    );
    f.memory(
        "noise-replaced",
        &["alpha", "beta", "tool", "person-a", "unit"],
    );
    let unsafe_id = ["sk-", "proj-", &"a1B2c3D4".repeat(6)].concat();
    f.memory(&unsafe_id, &["alpha", "beta", "tool", "person-a", "unit"]);
    let long_id = "safe".repeat(70);
    f.memory(&long_id, &["alpha", "beta", "tool", "person-a", "unit"]);
    {
        let conn = f.storage.conn.lock().unwrap();
        conn.execute(
            "UPDATE memories SET memory_type='session_summary' WHERE id='noise-summary'",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE memories SET superseded_by='valid-a' WHERE id='noise-replaced'",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO memory_entities VALUES ('orphan-memory','tool')",
            [],
        )
        .unwrap();
    }
    let tech = ["Rust".into(), "sqlite".into()];
    let people = ["Reviewer".into()];
    assert_matches_background(&f, &tech, &people);
    let full = EdgePage {
        offset: 0,
        limit: 100,
    };
    for (source, target, kind, allowed_tech, allowed_people) in [
        (
            key(former),
            "entity:tool".into(),
            LinkKind::Uses,
            tech.as_slice(),
            people.as_slice(),
        ),
        (
            key(alpha),
            "entity:rare".into(),
            LinkKind::Uses,
            tech.as_slice(),
            people.as_slice(),
        ),
        (
            key(alpha),
            "entity:tool".into(),
            LinkKind::Uses,
            &[][..],
            people.as_slice(),
        ),
        (
            key(alpha),
            person("Reviewer"),
            LinkKind::Involves,
            tech.as_slice(),
            &[][..],
        ),
        (
            key(alpha),
            key(beta),
            LinkKind::Contains,
            tech.as_slice(),
            people.as_slice(),
        ),
        (
            key(alpha),
            key(alpha),
            LinkKind::Relates,
            tech.as_slice(),
            people.as_slice(),
        ),
        (
            key(alpha),
            person("Reviewer"),
            LinkKind::Uses,
            tech.as_slice(),
            people.as_slice(),
        ),
        (
            "entity:tool".into(),
            key(alpha),
            LinkKind::Contains,
            tech.as_slice(),
            people.as_slice(),
        ),
    ] {
        assert!(
            clean(
                f.storage
                    .curated_graph_edge_memories(
                        &source,
                        &target,
                        kind,
                        allowed_tech,
                        allowed_people,
                        full
                    )
                    .unwrap()
            )
            .is_empty()
        );
    }
    let units = [(alpha, "release".into())];
    for archived in [true, false] {
        {
            let conn = f.storage.conn.lock().unwrap();
            if archived {
                for id in ["alpha", "tool", "person-a"] {
                    conn.execute("INSERT INTO entity_archive VALUES (?1,100)", [id])
                        .unwrap();
                }
            } else {
                conn.execute("DELETE FROM entity_archive", []).unwrap();
            }
        }
        assert_matches_background(&f, &tech, &people);
        let activity = f.storage.project_unit_memory_activity(&units).unwrap();
        let ids = clean(
            f.storage
                .curated_graph_unit_memories(alpha, "release", &units, full)
                .unwrap(),
        );
        assert_eq!(ids.len(), activity[&(alpha, "release".into())].0 as usize);
        assert!(ids.iter().all(|id| id != &unsafe_id));
    }
    assert!(
        clean(
            f.storage
                .curated_graph_edge_memories(
                    &key(alpha),
                    &key(beta),
                    LinkKind::Relates,
                    &tech,
                    &people,
                    full
                )
                .unwrap()
        )
        .contains(&long_id)
    );
}

#[test]
fn curated_graph_targeted_pages_admit_ids_before_offset_and_minimums() {
    let f = Fixture::new();
    let alpha = f.project("alpha", None);
    let beta = f.project("beta", Some(alpha));
    f.entity("tool", "rust", "tech");
    f.entity("unit", "release", "concept");
    let unsafe_id = ["sk-", "proj-", &"a1B2c3D4".repeat(6)].concat();
    f.memory(&unsafe_id, &["alpha", "beta", "tool", "unit"]);
    f.memory("valid-a", &["alpha", "beta", "tool", "unit"]);
    f.memory("valid-b", &["tool"]);
    let first = EdgePage {
        offset: 0,
        limit: 1,
    };
    let tech = ["rust".into()];
    assert!(
        clean(
            f.storage
                .curated_graph_edge_memories(
                    &key(alpha),
                    &key(beta),
                    LinkKind::Relates,
                    &tech,
                    &[],
                    first
                )
                .unwrap()
        )
        .is_empty()
    );
    assert!(
        clean(
            f.storage
                .curated_graph_edge_memories(
                    &key(alpha),
                    "entity:tool",
                    LinkKind::Uses,
                    &tech,
                    &[],
                    first
                )
                .unwrap()
        )
        .is_empty()
    );
    f.memory("valid-c", &["alpha", "beta", "tool", "unit"]);
    let second = EdgePage {
        offset: 1,
        limit: 1,
    };
    let units = [(alpha, "release".into())];
    for (source, target, kind) in [
        (key(alpha), key(beta), LinkKind::Relates),
        (key(alpha), key(beta), LinkKind::Contains),
        (key(alpha), "entity:tool".into(), LinkKind::Uses),
    ] {
        assert_eq!(
            clean(
                f.storage
                    .curated_graph_edge_memories(&source, &target, kind, &tech, &[], first)
                    .unwrap()
            ),
            ["valid-a"]
        );
        assert_eq!(
            clean(
                f.storage
                    .curated_graph_edge_memories(&source, &target, kind, &tech, &[], second)
                    .unwrap()
            ),
            ["valid-c"]
        );
    }
    assert_eq!(
        clean(
            f.storage
                .curated_graph_unit_memories(alpha, "release", &units, second)
                .unwrap()
        ),
        ["valid-c"]
    );
    for n in 0..105 {
        f.memory(&format!("z{n:03}"), &["alpha", "beta"]);
    }
    let full = EdgePage {
        offset: 0,
        limit: usize::MAX,
    };
    let tail = EdgePage {
        offset: 100,
        limit: usize::MAX,
    };
    assert_eq!(
        clean(
            f.storage
                .curated_graph_edge_memories(
                    &key(alpha),
                    &key(beta),
                    LinkKind::Relates,
                    &tech,
                    &[],
                    full
                )
                .unwrap()
        )
        .len(),
        100
    );
    assert_eq!(
        clean(
            f.storage
                .curated_graph_edge_memories(
                    &key(alpha),
                    &key(beta),
                    LinkKind::Relates,
                    &tech,
                    &[],
                    tail
                )
                .unwrap()
        )
        .len(),
        7
    );
}
