use super::*;
use crate::storage::Storage;
use crate::test_support::{legacy_add_fact_sql, legacy_facts_fixture};

/// A store as an old binary left it: the old table filled, nothing in v2.
fn old_store(dir: &std::path::Path) -> (std::path::PathBuf, crate::test_support::LegacyFacts) {
    let path = dir.join("memory.db");
    drop(Storage::open(&path).unwrap());
    let conn = Connection::open(&path).unwrap();
    let seeded = legacy_facts_fixture(&conn);
    (path, seeded)
}

fn events(storage: &Storage) -> i64 {
    storage
        .conn
        .lock()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM fact_events", [], |r| r.get(0))
        .unwrap()
}

#[test]
fn legacy_fixture_imports_36_rows_14_slots_14_current_verbatim() {
    let dir = crate::test_support::temp_dir("mnemonic-fact-legacy-");
    let (path, seeded) = old_store(dir.path());
    assert_eq!(seeded.rows, 36);
    let storage = Storage::open(&path).unwrap();
    let conn = storage.conn.lock().unwrap();
    let audit = audit(&conn).unwrap();
    assert_eq!((audit.legacy_rows, audit.imported), (36, 36));
    assert_eq!((audit.slots, audit.current), (14, 14));
    assert!(audit.clean(), "{audit:?}");
    drop(conn);
    let current = crate::facts::store::current_all(&storage).unwrap();
    for (subject, predicate, value) in &seeded.current {
        // Keys as the new store resolves them ("gizmo-app" is "gizmo").
        let subject_key = keys::subject_key(&storage.conn.lock().unwrap(), subject).unwrap();
        let found = current
            .iter()
            .find(|f| {
                f.subject_key == subject_key
                    && f.predicate == keys::predicate_key(predicate).unwrap()
            })
            .unwrap_or_else(|| panic!("{subject}/{predicate}"));
        assert_eq!(&found.value, value);
    }
}

#[test]
fn reopen_adds_no_events() {
    let dir = crate::test_support::temp_dir("mnemonic-fact-reopen-");
    let (path, _) = old_store(dir.path());
    let first = events(&Storage::open(&path).unwrap());
    assert_eq!(first, 36);
    assert_eq!(events(&Storage::open(&path).unwrap()), first);
}

#[test]
fn concurrent_open_imports_once() {
    let dir = crate::test_support::temp_dir("mnemonic-fact-concurrent-");
    let (path, _) = old_store(dir.path());
    let handles: Vec<_> = (0..4)
        .map(|_| {
            let path = path.clone();
            std::thread::spawn(move || drop(Storage::open(&path).unwrap()))
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }
    let storage = Storage::open(&path).unwrap();
    assert_eq!(crate::facts::store::value_count(&storage).unwrap(), 36);
}

#[test]
fn legacy_write_after_upgrade_caught_up_in_time_order() {
    let dir = crate::test_support::temp_dir("mnemonic-fact-late-");
    let (path, _) = old_store(dir.path());
    drop(Storage::open(&path).unwrap());
    // An old binary still running writes the old way.
    let conn = Connection::open(&path).unwrap();
    legacy_add_fact_sql(
        &conn,
        "widget",
        "has-price",
        "$99",
        "2026-09-20T10:00:00.000000+00:00",
    );
    drop(conn);
    let storage = Storage::open(&path).unwrap();
    let views = crate::facts::store::views(&storage, None, "widget").unwrap();
    let price = views.iter().find(|v| v.predicate == "has-price").unwrap();
    assert_eq!(
        price.current.as_ref().unwrap().value.as_deref(),
        Some("$99")
    );
    assert_eq!(price.history.len(), 4);
}

#[test]
fn legacy_table_checksum_unchanged() {
    let dir = crate::test_support::temp_dir("mnemonic-fact-untouched-");
    let (path, _) = old_store(dir.path());
    let dump = |path: &std::path::Path| -> String {
        let conn = Connection::open(path).unwrap();
        let mut stmt = conn
            .prepare("SELECT id, subject, predicate, value, valid_from, valid_to, confidence, source_memory_id, created_at FROM facts ORDER BY id")
            .unwrap();
        stmt.query_map([], |r| {
            Ok((0..9)
                .map(|i| format!("{:?}", r.get_ref(i).unwrap()))
                .collect::<Vec<_>>()
                .join("|"))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect::<Vec<_>>()
        .join("\n")
    };
    let before = dump(&path);
    drop(Storage::open(&path).unwrap());
    assert_eq!(dump(&path), before);
}

#[test]
fn fact_audit_flags_a_dangling_source() {
    let dir = crate::test_support::temp_dir("mnemonic-fact-audit-");
    let (path, _) = old_store(dir.path());
    let storage = Storage::open(&path).unwrap();
    {
        let conn = storage.conn.lock().unwrap();
        assert!(audit(&conn).unwrap().clean());
        // A delete made while the trigger was missing.
        conn.execute_batch(
            "DROP TRIGGER trg_fact_values_source_deleted;
             UPDATE fact_values SET source_memory_id = 'gone' WHERE rowid = 1;",
        )
        .unwrap();
        let found = audit(&conn).unwrap();
        assert_eq!(found.dangling_sources, 1);
        assert!(!found.clean());
    }
    drop(storage);
    // The next open repairs it (and puts the trigger back).
    let storage = Storage::open(&path).unwrap();
    assert!(audit(&storage.conn.lock().unwrap()).unwrap().clean());
}

#[test]
fn a_forgotten_imported_value_stays_forgotten() {
    let dir = crate::test_support::temp_dir("mnemonic-fact-legacy-forget-");
    let (path, _) = old_store(dir.path());
    let storage = Storage::open(&path).unwrap();
    let id: String = storage
        .conn
        .lock()
        .unwrap()
        .query_row(
            "SELECT id FROM fact_values WHERE legacy_fact_id IS NOT NULL ORDER BY seq LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let before = crate::facts::store::value_count(&storage).unwrap();
    crate::facts::store::forget_value(&storage, &id).unwrap();
    drop(storage);
    let storage = Storage::open(&path).unwrap();
    assert_eq!(
        crate::facts::store::value_count(&storage).unwrap(),
        before - 1
    );
    let conn = storage.conn.lock().unwrap();
    let audit = audit(&conn).unwrap();
    assert!(audit.not_imported.is_empty(), "{audit:?}");
    assert_eq!(audit.legacy_rows, audit.imported);
}

#[test]
fn an_old_binary_row_inside_a_reconfirmed_span_does_not_take_over() {
    let dir = crate::test_support::temp_dir("mnemonic-fact-legacy-span-");
    let path = dir.path().join("memory.db");
    {
        let storage = Storage::open(&path).unwrap();
        let at = |value, as_of| crate::facts::store::FactWrite {
            subject: "Widget",
            predicate: "price",
            value: Some(value),
            as_of: Some(as_of),
            actor: "test",
            ..Default::default()
        };
        crate::facts::store::apply(&storage, &at("$5", "2026-01-01")).unwrap();
        let again = crate::facts::store::apply(&storage, &at("$5", "2026-03-01")).unwrap();
        assert_eq!(again.outcome, "reconfirm");
    }
    {
        let conn = Connection::open(&path).unwrap();
        legacy_add_fact_sql(&conn, "Widget", "price", "$6", "2026-02-01T00:00:00+00:00");
    }
    let storage = Storage::open(&path).unwrap();
    let view = &crate::facts::store::views(&storage, None, "Widget").unwrap()[0];
    let trail: Vec<_> = view
        .history
        .iter()
        .map(|v| v.value.as_deref().unwrap())
        .collect();
    assert_eq!(trail, vec!["$5", "$6", "$5"]);
}

#[test]
fn at_a_shared_instant_the_row_left_open_stays_current() {
    let dir = crate::test_support::temp_dir("mnemonic-fact-legacy-tie-");
    let path = dir.path().join("memory.db");
    drop(Storage::open(&path).unwrap());
    {
        let conn = Connection::open(&path).unwrap();
        // The closed row sorts after the open one by id: only the order by
        // valid_to keeps the open one last.
        for (id, value, valid_to) in [
            ("zzzz-closed", "$5", Some("2026-02-01 10:00:00")),
            ("aaaa-open", "$6", None),
        ] {
            conn.execute(
                "INSERT INTO facts (id, subject, predicate, value, valid_from, valid_to, confidence,
                     source_memory_id, created_at)
                 VALUES (?1, 'Widget', 'price', ?2, '2026-02-01 10:00:00', ?3, 1.0, 'manual',
                         '2026-02-01 10:00:00')",
                rusqlite::params![id, value, valid_to],
            )
            .unwrap();
        }
    }
    let storage = Storage::open(&path).unwrap();
    let view = &crate::facts::store::views(&storage, None, "Widget").unwrap()[0];
    assert_eq!(view.current.as_ref().unwrap().value.as_deref(), Some("$6"));
}
