use super::*;

struct Fixture {
    _root: tempfile::TempDir,
    storage: Storage,
}
impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let storage = Storage::open(&root.path().join("fixture.db")).unwrap();
        storage.conn.lock().unwrap().execute_batch(SCHEMA).unwrap();
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
    fn memory(&self, id: &str, at: i64, entities: &[&str]) {
        let conn = self.storage.conn.lock().unwrap();
        conn.execute("INSERT INTO memories(id,timestamp,title,content,memory_type,source)
            VALUES (?1,datetime(?2,'unixepoch'),'Fixture title','Fixture content','decision','test')",
            params![id,at]).unwrap();
        for entity in entities {
            conn.execute(
                "INSERT INTO memory_entities(memory_id,entity_id) VALUES (?1,?2)",
                params![id, entity],
            )
            .unwrap();
        }
    }
    fn segment(&self, project: i64, at: i64) {
        self.storage.conn.lock().unwrap().execute("INSERT INTO work_segments
            (stream,driver,agent,project_id,started_at,last_at,source)
            VALUES ('fixture-stream','owner','codex',?1,datetime(?2-60,'unixepoch'),datetime(?2,'unixepoch'),'folder')",
            params![project,at]).unwrap();
    }
}
fn link<'a>(
    evidence: &'a Evidence,
    source: &str,
    target: &str,
    kind: LinkKind,
) -> &'a EvidenceLink {
    evidence
        .links
        .iter()
        .find(|link| link.source == source && link.target == target && link.kind == kind)
        .unwrap()
}

#[test]
fn curated_graph_registry_evidence_has_exact_kinds_counts_and_activity() {
    let f = Fixture::new();
    let a = f.project("alpha", None);
    let b = f.project("beta", Some(a));
    f.entity("tech", "rust", "tech");
    f.entity("low", "sqlite", "tech");
    f.entity("person", "reviewer", "person");
    f.entity("concept", "architecture", "concept");
    f.entity("unlisted", "other-tech", "tech");
    f.memory(
        "m1",
        100,
        &[
            "alpha", "beta", "tech", "low", "person", "concept", "unlisted",
        ],
    );
    f.memory(
        "m2",
        200,
        &["alpha", "beta", "tech", "low", "concept", "unlisted"],
    );
    f.memory("m3", 300, &["alpha", "tech", "unlisted"]);
    f.segment(b, 400);
    let tech = vec!["Rust".into(), "sqlite".into()];
    let people = vec!["reviewer".into()];
    let before = f.storage.conn.lock().unwrap().total_changes();
    let out = f.storage.curated_graph_evidence(&tech, &people).unwrap();
    assert_eq!(f.storage.conn.lock().unwrap().total_changes(), before);
    assert_eq!(out.projects.len(), 2);
    assert_eq!(out.entities.len(), 2);
    assert!(
        out.entities
            .iter()
            .all(|entity| entity.id == "tech" || entity.kind == EntityKind::Person)
    );
    assert_eq!(
        link(&out, &key(a), &key(b), LinkKind::Contains).memory_count,
        2
    );
    assert_eq!(
        link(&out, &key(a), &key(b), LinkKind::Relates).memory_count,
        2
    );
    assert_eq!(
        link(&out, &key(a), "entity:tech", LinkKind::Uses).memory_count,
        3
    );
    assert_eq!(
        link(&out, &key(b), "entity:tech", LinkKind::Uses).memory_count,
        2
    );
    let person = out
        .entities
        .iter()
        .find(|entity| entity.kind == EntityKind::Person)
        .unwrap();
    assert_eq!(
        link(
            &out,
            &key(a),
            &format!("entity:{}", person.id),
            LinkKind::Involves
        )
        .memory_count,
        1
    );
    let beta = out
        .projects
        .iter()
        .find(|project| project.row.id == b)
        .unwrap();
    assert_eq!(
        (beta.last_memory, beta.last_segment),
        (Some(200), Some(400))
    );
    assert_eq!(
        out.entities
            .iter()
            .find(|entity| entity.id == "tech")
            .unwrap()
            .last_memory,
        Some(300)
    );
    let ids = f
        .storage
        .curated_graph_edge_memories(
            &key(a),
            &key(b),
            LinkKind::Relates,
            &tech,
            &people,
            EdgePage {
                offset: 0,
                limit: 1000,
            },
        )
        .unwrap();
    assert_eq!(ids, ["m1", "m2"]);
    assert_eq!(
        f.storage
            .curated_graph_edge_memories(
                &key(b),
                &key(a),
                LinkKind::Relates,
                &tech,
                &people,
                EdgePage {
                    offset: 1,
                    limit: 100
                }
            )
            .unwrap(),
        ["m2"]
    );
    let terms = f.storage.curated_graph_top_terms(a, 100).unwrap();
    assert_eq!(terms.len(), 1);
    assert_eq!(terms[0].name, "architecture");
    assert_eq!(terms[0].memory_count, 2);
}

#[test]
fn curated_graph_distinct_live_memories_ignore_summary_superseded_and_orphans() {
    let f = Fixture::new();
    f.project("alpha", None);
    f.entity("tech", "rust", "tech");
    f.memory("live", 10, &["alpha", "tech"]);
    f.memory("summary", 20, &["alpha", "tech"]);
    f.memory("replaced", 30, &["alpha", "tech"]);
    {
        let conn = f.storage.conn.lock().unwrap();
        conn.execute(
            "UPDATE memories SET memory_type='session_summary' WHERE id='summary'",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE memories SET superseded_by='live' WHERE id='replaced'",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO memory_entities VALUES ('absent-memory','tech')",
            [],
        )
        .unwrap();
    }
    let out = f
        .storage
        .curated_graph_evidence(&["rust".into()], &[])
        .unwrap();
    assert_eq!(out.projects[0].memory_count, 1);
    assert_eq!(out.projects[0].last_memory, Some(10));
    assert!(
        out.entities.is_empty(),
        "999 stored mentions do not replace live-memory evidence"
    );
    f.memory("second", 40, &["alpha", "tech"]);
    f.memory("third", 50, &["alpha", "tech"]);
    let out = f
        .storage
        .curated_graph_evidence(&["rust".into()], &[])
        .unwrap();
    assert_eq!(out.entities[0].memory_count, 3);
}

#[test]
fn curated_graph_merges_and_name_aliases_sum_each_memory_once_and_undo() {
    let f = Fixture::new();
    let a = f.project("alpha", None);
    let b = f.project("beta", None);
    let old = f.project("former", Some(b));
    f.entity("alias", "alpha-alias", "project");
    f.storage
        .project_alias_set("alpha-alias", a, super::super::projects::AliasKind::Name)
        .unwrap();
    f.memory("m1", 100, &["alpha", "former", "alias", "beta"]);
    f.memory("m2", 200, &["former", "alias", "beta"]);
    f.segment(old, 300);
    f.storage.project_merge(old, a).unwrap();
    let out = f.storage.curated_graph_evidence(&[], &[]).unwrap();
    assert_eq!(out.projects.len(), 2);
    let alpha = out
        .projects
        .iter()
        .find(|project| project.row.id == a)
        .unwrap();
    assert_eq!(alpha.memory_count, 2);
    assert_eq!(alpha.last_segment, Some(300));
    assert_eq!(
        link(&out, &key(a), &key(b), LinkKind::Relates).memory_count,
        2
    );
    assert_eq!(
        link(&out, &key(b), &key(a), LinkKind::Contains).memory_count,
        2
    );
    assert!(out.shelf.iter().all(|entity| entity.id != "alias"));
    assert!(f.storage.entity_archive_set("alias", true, 400).is_err());
    f.storage.project_unmerge(old).unwrap();
    assert_eq!(
        f.storage
            .curated_graph_evidence(&[], &[])
            .unwrap()
            .projects
            .len(),
        3
    );
}

fn slot(key: &str, parent: Option<&str>, preferred: Option<i64>) -> SlotRequest {
    SlotRequest {
        key: key.into(),
        parent_key: parent.map(str::to_string),
        preferred_slot: preferred,
    }
}
#[test]
fn curated_graph_named_activity_is_exact_and_labels_are_prepared_after_capping() {
    let f = Fixture::new();
    let alpha = f.project("alpha", None);
    f.entity("unit", "release-one", "concept");
    f.entity("near", "release-one-extra", "concept");
    f.memory("m1", 100, &["alpha", "unit"]);
    f.memory("m2", 200, &["near"]);
    let named = f
        .storage
        .project_unit_memory_activity(&[(alpha, "release-one".into()), (alpha, "missing".into())])
        .unwrap();
    assert_eq!(named[&(alpha, "release-one".into())], (1, Some(100)));
    assert!(!named.contains_key(&(alpha, "missing".into())));
    let credential = format!("sk-{}", "AbCd1234".repeat(6));
    f.entity("legacy-secret", &credential, "project");
    let shelf = f.storage.entity_archive_shelf(false).unwrap();
    assert!(!format!("{shelf:?}").contains(&credential));
    let body = "AbCd1234".repeat(3);
    let title = format!(
        "{} password={body}(arg)",
        "a".repeat(80 - " password=".len() - body.len())
    );
    f.storage
        .conn
        .lock()
        .unwrap()
        .execute(
            "UPDATE entities SET name=?1 WHERE id='legacy-secret'",
            [title],
        )
        .unwrap();
    let shelf = f.storage.entity_archive_shelf(false).unwrap();
    assert!(!format!("{shelf:?}").contains(&body));
}

#[test]
fn curated_graph_edge_evidence_is_read_only_bounded_and_paged() {
    let f = Fixture::new();
    let a = f.project("alpha", None);
    let b = f.project("beta", None);
    for i in 0..105 {
        f.memory(&format!("m{i:03}"), 100 + i, &["alpha", "beta"]);
    }
    let before = f.storage.conn.lock().unwrap().total_changes();
    let first = f
        .storage
        .curated_graph_edge_memories(
            &key(a),
            &key(b),
            LinkKind::Relates,
            &[],
            &[],
            EdgePage {
                offset: 0,
                limit: 1000,
            },
        )
        .unwrap();
    let last = f
        .storage
        .curated_graph_edge_memories(
            &key(a),
            &key(b),
            LinkKind::Relates,
            &[],
            &[],
            EdgePage {
                offset: 100,
                limit: 1000,
            },
        )
        .unwrap();
    assert_eq!(first.len(), 100);
    assert_eq!(last.len(), 5);
    assert_eq!(last[0], "m100");
    assert!(first.iter().all(|id| !last.contains(id)));
    assert!(
        f.storage
            .curated_graph_edge_memories(
                &key(a),
                &key(b),
                LinkKind::Relates,
                &[],
                &[],
                EdgePage {
                    offset: usize::MAX,
                    limit: usize::MAX
                }
            )
            .unwrap()
            .is_empty()
    );
    assert_eq!(f.storage.conn.lock().unwrap().total_changes(), before);
    assert!(
        f.storage.graph_slots().unwrap().is_empty(),
        "GET evidence never allocates slots"
    );
}

#[test]
fn curated_graph_configured_person_key_survives_first_memory_and_unicode_name() {
    let f = Fixture::new();
    f.project("alpha", None);
    let name = "\u{4eba}\u{7269}";
    let people = [name.into()];
    let before = f.storage.curated_graph_evidence(&[], &people).unwrap();
    let person = &before.entities[0];
    assert_eq!((person.memory_count, person.last_memory), (0, None));
    assert!(person.id.is_ascii() && person.id.starts_with("person:"));
    let node_key = format!("entity:{}", person.id);
    f.storage
        .ensure_graph_slots(&[slot(&node_key, Some("ring:person"), None)])
        .unwrap();
    let slots = f.storage.graph_slots().unwrap();
    f.entity("first-person-entity", name, "person");
    f.memory("m1", 100, &["alpha", "first-person-entity"]);
    let after = f.storage.curated_graph_evidence(&[], &people).unwrap();
    assert_eq!(after.entities[0].id, person.id);
    assert_eq!(
        (
            after.entities[0].memory_count,
            after.entities[0].last_memory
        ),
        (1, Some(100))
    );
    assert_eq!(f.storage.graph_slots().unwrap(), slots);
}

#[path = "curated_graph_ambiguity_tests.rs"]
mod ambiguity_tests;
#[path = "curated_graph_state_tests.rs"]
mod state_tests;
#[path = "curated_graph_unit_tests.rs"]
mod unit_tests;
