use super::*;

#[test]
fn curated_graph_ambiguous_registry_names_cannot_supply_plan_unit_evidence() {
    for explicit in [false, true] {
        let f = Fixture::new();
        let alpha = f.project("alpha", None);
        let beta = f.project("beta", None);
        let alpha_api = f
            .storage
            .project_upsert("alpha:api", "api", Some(alpha))
            .unwrap();
        let beta_api = f
            .storage
            .project_upsert("beta:api", "api", Some(beta))
            .unwrap();
        f.entity("api", "api", "project");
        if explicit {
            f.storage.project_link_entity(alpha_api, "api").unwrap();
            f.storage.project_link_entity(beta_api, "api").unwrap();
        }
        f.entity("release", "release", "concept");
        f.memory("alpha-release", 100, &["alpha", "release"]);
        f.memory("alpha-api", 300, &["alpha", "api"]);
        f.memory("beta-api", 500, &["beta", "api"]);
        let units = [(alpha, "api".into()), (alpha, "release".into())];
        let page = EdgePage {
            offset: 0,
            limit: 100,
        };
        // Resolving the two registry identities admits one exact name;
        // undoing that merge makes the same evidence ambiguous again.
        for stage in 0..3 {
            if stage == 1 {
                f.storage.project_merge(beta_api, alpha_api).unwrap();
            } else if stage == 2 {
                f.storage.project_unmerge(beta_api).unwrap();
            }
            let before = f.storage.conn.lock().unwrap().total_changes();
            let activity = f.storage.project_unit_memory_activity(&units).unwrap();
            assert_eq!(activity[&(alpha, "release".into())], (1, Some(100)));
            let api = activity
                .get(&(alpha, "api".into()))
                .copied()
                .unwrap_or((0, None));
            let ids = f
                .storage
                .curated_graph_unit_memories(alpha, "api", &units, page)
                .unwrap();
            if stage == 1 {
                assert_eq!(api, (1, Some(300)));
                assert_eq!(ids, ["alpha-api"]);
            } else {
                assert_eq!(api, (0, None));
                assert!(ids.is_empty());
            }
            let evidence = f.storage.curated_graph_evidence(&[], &[]).unwrap();
            let root = evidence
                .projects
                .iter()
                .find(|project| project.row.id == alpha)
                .unwrap();
            assert_eq!((root.memory_count, root.last_memory), (2, Some(300)));
            assert!(evidence.shelf.is_empty());
            assert_eq!(f.storage.conn.lock().unwrap().total_changes(), before);
        }
    }
}

#[test]
fn curated_graph_redirected_containment_has_zero_or_one_real_supporting_memory() {
    for count in [0, 1] {
        let f = Fixture::new();
        let alpha = f.project("alpha", None);
        let beta = f.project("beta", None);
        let part = f.project("former", Some(beta));
        if count == 1 {
            f.memory("one-shared", 100, &["beta", "former"]);
        }
        f.storage.project_merge(part, alpha).unwrap();
        let before = f.storage.conn.lock().unwrap().total_changes();
        let evidence = f.storage.curated_graph_evidence(&[], &[]).unwrap();
        let contains = link(&evidence, &key(beta), &key(alpha), LinkKind::Contains);
        assert_eq!(contains.memory_count, count);
        assert!(
            evidence
                .links
                .iter()
                .all(|link| link.kind != LinkKind::Relates)
        );
        assert!(
            evidence
                .projects
                .iter()
                .all(|project| project.row.id != part)
        );
        let ids = f
            .storage
            .curated_graph_edge_memories(
                &key(beta),
                &key(alpha),
                LinkKind::Contains,
                &[],
                &[],
                EdgePage {
                    offset: 0,
                    limit: 100,
                },
            )
            .unwrap();
        if count == 1 {
            assert_eq!(ids, ["one-shared"]);
        } else {
            assert!(ids.is_empty());
        }
        assert_eq!(f.storage.conn.lock().unwrap().total_changes(), before);
    }
}
