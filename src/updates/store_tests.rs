//! Update links against a real store: the delete trigger keeps chains whole
//! whatever path removes a memory.
use super::*;
use crate::event::{EventSource, MemoryEntry, MemoryType};
use crate::storage::Storage;
use crate::test_support::InTempDir;
use chrono::{Duration, Utc};

fn store() -> InTempDir<Storage> {
    InTempDir::new("mnemonic-updates-", |dir| {
        Storage::open(&dir.join("memory.db")).unwrap()
    })
}

/// A memory saved `minutes_ago`, returning its id.
fn memory(storage: &Storage, text: &str, minutes_ago: i64) -> String {
    let mut entry = MemoryEntry::new(text, text, MemoryType::Note, EventSource::Manual);
    entry.timestamp = Utc::now() - Duration::minutes(minutes_ago);
    storage.save(&entry).unwrap();
    entry.id
}

fn value(key: &str, surface: &str) -> Value {
    Value {
        class: Class::Money,
        key: key.into(),
        surface: surface.into(),
        pred: None,
        context: Vec::new(),
    }
}

fn link(storage: &Storage, new_id: &str, old_id: &str, was: &str, now: &str) {
    let conn = storage.conn.lock().unwrap();
    assert!(
        insert(
            &conn,
            &Link {
                new_id: new_id.into(),
                old_id: old_id.into(),
                rule: Rule::ValueDiff,
                class: Class::Money,
                was: vec![value(&format!("usd:{}", &was[1..]), was)],
                now: vec![value(&format!("usd:{}", &now[1..]), now)],
                similarity: Some(0.97),
                actor: "test",
            },
        )
        .unwrap()
    );
}

/// Every active link as (new, old, was surfaces, now surfaces).
fn links(storage: &Storage) -> Vec<(String, String, String, String)> {
    let conn = storage.conn.lock().unwrap();
    let mut stmt = conn
        .prepare(
            "SELECT new_id, old_id, was_values, now_values FROM memory_updates
              WHERE status = 'active' ORDER BY created_at, new_id",
        )
        .unwrap();
    stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn head_of(storage: &Storage, id: &str) -> String {
    head(&storage.conn.lock().unwrap(), id).unwrap().unwrap()
}

#[test]
fn head_follows_the_chain_to_its_newest_end() {
    let s = store();
    let a = memory(&s, "price $5", 30);
    let b = memory(&s, "price $6", 20);
    let c = memory(&s, "price $7", 10);
    link(&s, &b, &a, "$5", "$6");
    link(&s, &c, &b, "$6", "$7");
    for id in [&a, &b, &c] {
        assert_eq!(&head_of(&s, id), &c);
    }
    // A fork: the newest end wins.
    let d = memory(&s, "price $8", 5);
    link(&s, &d, &b, "$6", "$8");
    assert_eq!(head_of(&s, &a), d);
}

#[test]
fn a_cycle_cannot_hang_the_head_lookup() {
    let s = store();
    let a = memory(&s, "price $5", 20);
    let b = memory(&s, "price $6", 10);
    link(&s, &b, &a, "$5", "$6");
    link(&s, &a, &b, "$6", "$5");
    // No end to find: nothing can be decided from this chain.
    assert_eq!(head(&s.conn.lock().unwrap(), &a).unwrap(), None);
}

#[test]
fn splice_middle_node_joins_neighbours_with_outer_values() {
    let s = store();
    let a = memory(&s, "price $5", 30);
    let b = memory(&s, "price $6", 20);
    let c = memory(&s, "price $7", 10);
    link(&s, &b, &a, "$5", "$6");
    link(&s, &c, &b, "$6", "$7");
    assert!(s.forget_by_id(&b).unwrap());
    let rows = links(&s);
    assert_eq!(rows.len(), 1, "{rows:?}");
    let (new_id, old_id, was, now) = &rows[0];
    assert_eq!((new_id, old_id), (&c, &a));
    // Nothing of the forgotten middle survives: was is A's, now is C's.
    assert!(was.contains("$5") && !was.contains("$6"), "{was}");
    assert!(now.contains("$7") && !now.contains("$6"), "{now}");
}

#[test]
fn forget_head_restores_predecessor() {
    let s = store();
    let a = memory(&s, "price $5", 20);
    let b = memory(&s, "price $6", 10);
    link(&s, &b, &a, "$5", "$6");
    assert!(s.forget_by_id(&b).unwrap());
    assert!(links(&s).is_empty());
    assert_eq!(head_of(&s, &a), a);
}

#[test]
fn forget_leaf_drops_edge() {
    let s = store();
    let a = memory(&s, "price $5", 20);
    let b = memory(&s, "price $6", 10);
    link(&s, &b, &a, "$5", "$6");
    assert!(s.forget_by_id(&a).unwrap());
    assert!(links(&s).is_empty());
    assert_eq!(head_of(&s, &b), b);
}

#[test]
fn raw_sql_delete_fires_trigger() {
    let s = store();
    let a = memory(&s, "price $5", 20);
    let b = memory(&s, "price $6", 10);
    link(&s, &b, &a, "$5", "$6");
    s.conn
        .lock()
        .unwrap()
        .execute("DELETE FROM memories WHERE id = ?1", [&b])
        .unwrap();
    assert!(links(&s).is_empty());
}

#[test]
fn insert_or_replace_keeps_links() {
    let s = store();
    let a = memory(&s, "price $5", 20);
    let mut entry = MemoryEntry::new(
        "price $6",
        "price $6",
        MemoryType::Note,
        EventSource::Manual,
    );
    entry.timestamp = Utc::now() - Duration::minutes(10);
    s.save(&entry).unwrap();
    link(&s, &entry.id, &a, "$5", "$6");
    // An id-keyed rewrite of the same memory (REPLACE fires no delete trigger).
    entry.importance = 0.9;
    s.save_with_embedding(&entry, None).unwrap();
    assert_eq!(links(&s).len(), 1);
}

#[test]
fn a_link_needs_two_live_memories() {
    let s = store();
    let a = memory(&s, "price $5", 20);
    let conn = s.conn.lock().unwrap();
    let dangling = Link {
        new_id: "no-such-memory".into(),
        old_id: a.clone(),
        rule: Rule::ValueDiff,
        class: Class::Money,
        was: vec![],
        now: vec![],
        similarity: None,
        actor: "test",
    };
    assert!(insert(&conn, &dangling).is_err());
    let own = Link {
        new_id: a.clone(),
        ..dangling
    };
    assert!(insert(&conn, &own).is_err());
}
