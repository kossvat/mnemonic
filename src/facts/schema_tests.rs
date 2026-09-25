use crate::storage::Storage;
use crate::test_support::InTempDir;

#[test]
fn installing_twice_changes_nothing_and_the_trigger_is_there() {
    let store = InTempDir::new("mnemonic-fact-schema-", |dir| {
        Storage::open(&dir.join("memory.db")).unwrap()
    });
    let conn = store.conn.lock().unwrap();
    super::install(&conn).unwrap();
    for name in ["fact_slots", "fact_values", "fact_events"] {
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                [name],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1, "{name}");
    }
    let trigger: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'trigger'
               AND name = 'trg_fact_values_source_deleted'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(trigger, 1);
}
