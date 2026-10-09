use super::*;

fn measured(f: &Fixture, alpha: i64, beta: i64) -> Vec<edges::probe::Work> {
    let one = EdgePage {
        offset: 0,
        limit: 1,
    };
    let mut work = Vec::new();
    let before = f.storage.conn.lock().unwrap().total_changes();
    for (target, kind) in [
        (key(beta), LinkKind::Contains),
        (key(beta), LinkKind::Relates),
        ("entity:tool".into(), LinkKind::Uses),
        (person("Reviewer"), LinkKind::Involves),
    ] {
        let (result, read) = edges::probe::measure(|| {
            f.storage
                .curated_graph_edge_memories(
                    &key(alpha),
                    &target,
                    kind,
                    &["rust".into()],
                    &["Reviewer".into()],
                    one,
                )
                .unwrap()
        });
        assert_eq!(result, ["edge-0"]);
        assert!(read.queries > 0 && read.vm_steps > 0);
        assert!(read.memory_rows <= 4);
        assert_eq!(read.fullscan_steps, 0);
        work.push(read);
    }
    let (result, read) = edges::probe::measure(|| {
        f.storage
            .curated_graph_unit_memories(alpha, "release", &[(alpha, "release".into())], one)
            .unwrap()
    });
    assert_eq!(result, ["edge-0"]);
    assert_eq!(
        (read.queries, read.memory_rows, read.fullscan_steps),
        (1, 1, 0)
    );
    work.push(read);
    assert_eq!(f.storage.conn.lock().unwrap().total_changes(), before);
    work
}

#[test]
fn curated_graph_edge_sql_work_does_not_grow_with_unrelated_memory_links() {
    let f = Fixture::new();
    let alpha = f.project("alpha", None);
    let beta = f.project("beta", Some(alpha));
    f.entity("tool", "rust", "tech");
    f.entity("reviewer", "reviewer", "person");
    f.entity("unit", "release", "concept");
    for n in 0..6 {
        f.memory(
            &format!("edge-{n}"),
            &["alpha", "beta", "tool", "reviewer", "unit"],
        );
    }
    // Keep every requested entity range before the index's final entry in
    // both measurements. Reaching index EOF instead of checking the next
    // entity otherwise differs by one VM opcode, independently of growth.
    f.entity("zz-tail", "unrelated-tail", "concept");
    f.memory("unrelated-tail", &["zz-tail"]);
    f.storage
        .conn
        .lock()
        .unwrap()
        .execute_batch("PRAGMA query_only=ON")
        .unwrap();
    let before = measured(&f, alpha, beta);
    f.storage
        .conn
        .lock()
        .unwrap()
        .execute_batch("PRAGMA query_only=OFF")
        .unwrap();
    for n in 0..48 {
        f.project(&format!("unrelated-{n}"), None);
    }
    {
        let conn = f.storage.conn.lock().unwrap();
        let tx = conn.unchecked_transaction().unwrap();
        for n in 0..2400 {
            let id = format!("unrelated-memory-{n}");
            tx.execute(
                "INSERT INTO memories(id,timestamp,title,content,memory_type,source)
                VALUES (?1,'2026-01-01T00:00:00Z','Unrelated title','Unrelated content','decision','test')",
                [&id],
            )
            .unwrap();
            for neighbor in 0..3 {
                let entity = format!("unrelated-{}", (n + neighbor) % 48);
                tx.execute(
                    "INSERT INTO memory_entities VALUES (?1,?2)",
                    params![id, entity],
                )
                .unwrap();
            }
        }
        tx.commit().unwrap();
        let metadata = data::metadata(&conn).unwrap();
        assert_eq!(metadata.rows.len(), 50);
        assert!(metadata.memories.is_empty());
        conn.execute_batch("PRAGMA query_only=ON").unwrap();
    }
    // Identity metadata may grow with the registry. Actual evidence SQL VM
    // steps and decoded memory rows are independent of those 7,200 links.
    assert_eq!(measured(&f, alpha, beta), before);
}
