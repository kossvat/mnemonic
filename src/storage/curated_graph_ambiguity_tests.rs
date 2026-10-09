use super::*;

fn parts(f: &Fixture) -> (i64, i64) {
    let alpha = f.project("alpha", None);
    let beta = f.project("beta", None);
    (
        f.storage
            .project_upsert("alpha:api", "api", Some(alpha))
            .unwrap(),
        f.storage
            .project_upsert("beta:api", "api", Some(beta))
            .unwrap(),
    )
}

#[test]
fn curated_graph_ambiguous_parts_do_not_invent_activity_or_shared_memories() {
    let f = Fixture::new();
    let (alpha, beta) = parts(&f);
    f.entity("api-id", "api", "project");
    f.storage.project_link_entity(alpha, "api").unwrap();
    f.storage.project_link_entity(beta, "api").unwrap();
    f.entity("rust-id", "rust", "tech");
    f.entity("unit-id", "release", "concept");
    for n in 0..3 {
        f.memory(
            &format!("api-{n}"),
            10 + n,
            &["api-id", "rust-id", "unit-id"],
        );
    }
    let before = f.storage.conn.lock().unwrap().total_changes();
    let evidence = f
        .storage
        .curated_graph_evidence(&["rust".into()], &[])
        .unwrap();
    assert!(
        evidence
            .projects
            .iter()
            .all(|p| p.memory_count == 0 && p.last_memory.is_none())
    );
    assert!(
        evidence
            .links
            .iter()
            .all(|e| e.kind == LinkKind::Contains && e.memory_count == 0)
    );
    assert!(evidence.entities.is_empty());
    assert!(evidence.shelf.is_empty());
    assert!(
        f.storage
            .project_unit_memory_activity(&[(alpha, "release".into()), (beta, "release".into())])
            .unwrap()
            .is_empty()
    );
    assert!(
        f.storage
            .curated_graph_top_terms(alpha, 20)
            .unwrap()
            .is_empty()
    );
    assert_eq!(f.storage.conn.lock().unwrap().total_changes(), before);
    assert!(f.storage.entity_archive_set("api-id", true, 100).is_err());

    // Unambiguous qualified slugs and declared name aliases still identify
    // their own part. Only memories with both identities justify relates.
    f.entity("qualified", "alpha-api", "project");
    f.entity("alias", "beta-service", "project");
    f.storage
        .project_alias_set(
            "beta-service",
            beta,
            crate::storage::projects::AliasKind::Name,
        )
        .unwrap();
    f.memory("alpha-only", 100, &["qualified"]);
    f.memory("beta-only", 110, &["alias"]);
    f.memory("both-one", 200, &["qualified", "alias"]);
    f.memory("both-two", 210, &["qualified", "alias"]);
    let evidence = f.storage.curated_graph_evidence(&[], &[]).unwrap();
    for id in [alpha, beta] {
        let project = evidence.projects.iter().find(|p| p.row.id == id).unwrap();
        assert_eq!((project.memory_count, project.last_memory), (3, Some(210)));
    }
    assert_eq!(
        link(&evidence, &key(alpha), &key(beta), LinkKind::Relates).memory_count,
        2
    );
    let ids = f
        .storage
        .curated_graph_edge_memories(
            &key(alpha),
            &key(beta),
            LinkKind::Relates,
            &[],
            &[],
            EdgePage {
                offset: 0,
                limit: 100,
            },
        )
        .unwrap();
    assert_eq!(ids, ["both-one", "both-two"]);
    // A merge removes the ambiguity, and all old IDs and name aliases now
    // identify one canonical project. Shared memories still count once.
    f.storage.project_merge(beta, alpha).unwrap();
    let evidence = f.storage.curated_graph_evidence(&[], &[]).unwrap();
    let project = evidence
        .projects
        .iter()
        .find(|p| p.row.id == alpha)
        .unwrap();
    assert_eq!((project.memory_count, project.last_memory), (7, Some(210)));
    assert!(evidence.projects.iter().all(|p| p.row.id != beta));
}

#[test]
fn curated_graph_unique_explicit_binding_wins_over_ambiguous_names() {
    let f = Fixture::new();
    let (alpha, beta) = parts(&f);
    f.entity("api-id", "api", "project");
    f.storage.project_link_entity(alpha, "api").unwrap();
    f.memory("one", 100, &["api-id"]);
    f.memory("two", 200, &["api-id"]);
    let evidence = f.storage.curated_graph_evidence(&[], &[]).unwrap();
    let own = evidence
        .projects
        .iter()
        .find(|p| p.row.id == alpha)
        .unwrap();
    let other = evidence.projects.iter().find(|p| p.row.id == beta).unwrap();
    assert_eq!((own.memory_count, own.last_memory), (2, Some(200)));
    assert_eq!((other.memory_count, other.last_memory), (0, None));
    assert!(evidence.links.iter().all(|e| e.kind != LinkKind::Relates));
    // If neither row explicitly binds it, the duplicated name stays unknown.
    f.storage
        .conn
        .lock()
        .unwrap()
        .execute("UPDATE projects SET entity_id=NULL WHERE id=?1", [alpha])
        .unwrap();
    let evidence = f.storage.curated_graph_evidence(&[], &[]).unwrap();
    assert!(evidence.projects.iter().all(|p| p.memory_count == 0));
    assert!(evidence.shelf.is_empty());
}
