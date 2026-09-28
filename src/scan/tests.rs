//! The scanner against synthetic snapshots: what it reports, what it
//! refuses, and that it changes nothing. Credential fixtures are
//! assembled at run time; assertions never print them.
use std::path::{Path, PathBuf};

use rusqlite::{Connection, params};

use super::*;
use crate::storage::Storage;

fn body(n: usize) -> String {
    "a1B2c3D4e5F6".chars().cycle().take(n).collect()
}

fn token() -> String {
    ["sk-", "proj-", &body(40)].concat()
}

/// A directory of its own for each snapshot, so that what is beside the
/// snapshot is the scanner's doing or nobody's.
struct Fixture {
    dir: tempfile::TempDir,
    snapshots: std::cell::Cell<usize>,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::Builder::new()
            .prefix("mn-scan-")
            .tempdir_in("/tmp")
            .unwrap();
        std::fs::create_dir(dir.path().join("live")).unwrap();
        Self {
            dir,
            snapshots: std::cell::Cell::new(0),
        }
    }

    fn live(&self) -> PathBuf {
        self.dir.path().join("live").join("store.db")
    }

    /// A private store with its real schema.
    fn private() -> Self {
        let fixture = Self::new();
        drop(Storage::open(&fixture.live()).unwrap());
        fixture
    }

    fn conn(&self) -> Connection {
        Connection::open(self.live()).unwrap()
    }

    /// A closed copy of the live store, alone in a directory.
    fn snapshot(&self) -> PathBuf {
        let n = self.snapshots.get() + 1;
        self.snapshots.set(n);
        let dir = self.dir.path().join(format!("snapshot-{n}"));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("snapshot.db");
        self.conn()
            .execute("VACUUM INTO ?1", [path.to_str().unwrap()])
            .unwrap();
        path
    }

    fn memory(&self, id: &str, title: &str, content: &str, tags: &str, metadata: &str) {
        self.conn()
            .execute(
                "INSERT INTO memories (id, timestamp, title, content, memory_type, tags, source,
                    importance, metadata)
                 VALUES (?1, '2026-01-02T03:04:05Z', ?2, ?3, 'note', ?4, '\"Manual\"', 0.5, ?5)",
                params![id, title, content, tags, metadata],
            )
            .unwrap();
    }
}

fn listing(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// Scan, and check that the snapshot and its directory are as they were.
fn scanned(path: &Path) -> Report {
    let dir = path.parent().unwrap();
    let before = (std::fs::read(path).ok(), listing(dir));
    let report = scan(path);
    let after = (std::fs::read(path).ok(), listing(dir));
    assert!(before == after, "the scan changed the snapshot");
    report
}

fn places(report: &Report) -> Vec<(&'static str, &'static str)> {
    report
        .findings
        .iter()
        .map(|finding| (finding.table, finding.column))
        .collect()
}

fn classes(report: &Report, table: &str, column: &str) -> Vec<&'static str> {
    report
        .findings
        .iter()
        .filter(|finding| finding.table == table && finding.column == column)
        .flat_map(|finding| finding.classes.keys().copied())
        .collect()
}

#[test]
fn redaction_scan_of_a_clean_snapshot_is_complete_and_finds_nothing() {
    let f = Fixture::private();
    f.memory("m-1", "Rollout", "friday", "[\"ops\"]", "null");
    // A vector is bytes, and a text that holds nothing is nothing to
    // read.
    f.memory("m-2", "", " ", "[]", "null");
    f.conn()
        .execute("UPDATE memories SET embedding = X'ff00fe01'", [])
        .unwrap();
    let report = scanned(&f.snapshot());
    assert!(report.complete && report.reasons.is_empty());
    assert!(report.exit_code() == 0 && report.findings.is_empty());
    assert!(report.policy_version == crate::redaction::POLICY_VERSION);
    assert!(report.store == Some("private") && report.schema_version == Some(2));
    let memories = report
        .coverage
        .tables
        .iter()
        .find(|table| table.table == "memories")
        .unwrap();
    assert!(memories.rows == 2 && memories.locator == "rowid");
    assert!(memories.columns["content"] == "text" && memories.columns["metadata"] == "json");
    // A vector is said not to be scanned, and so is what the index keeps
    // for itself.
    assert!(memories.columns["embedding"] == "vector_not_scanned");
    let index: Vec<&TableCoverage> = report
        .coverage
        .tables
        .iter()
        .filter(|table| table.table.starts_with("memories_fts_"))
        .collect();
    assert!(index.len() == 4 && index.iter().any(|table| table.rows > 0));
    assert!(index.iter().all(|table| {
        table
            .columns
            .values()
            .any(|kind| *kind == "index_not_scanned")
            || table.table == "memories_fts_config"
    }));
    assert!(report.coverage.unknown_tables == 0 && report.coverage.unknown_columns == 0);
}

/// A finding says a table, a column, a row by its number and a class. The
/// value is not in the report, and neither is the id of a row whose id is
/// what was found.
#[test]
fn redaction_scan_says_where_and_what_class_and_nothing_of_the_value() {
    let token = token();
    let f = Fixture::private();
    f.memory("m-1", "Rollout", "friday", "[]", "null");
    f.memory(
        &token,
        "Deploy key",
        &format!("the key is {token}"),
        &serde_json::json!([format!("<private>{}</private>", body(24))]).to_string(),
        "null",
    );
    let conn = f.conn();
    conn.execute(
        "INSERT INTO entities (id, name, entity_type, mention_count, first_seen, last_seen)
         VALUES ('e-1', ?1, 'concept', 1, '2026-01-02', '2026-01-02')",
        [token.to_uppercase()],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO pending_extractions (memory_id, attempts, last_error, next_attempt_at)
         VALUES ('m-1', 0, ?1, '2026-01-02')",
        [format!("backend: POST refused for {token}")],
    )
    .unwrap();
    let row: i64 = conn
        .query_row("SELECT rowid FROM memories WHERE id = ?1", [&token], |r| {
            r.get(0)
        })
        .unwrap();
    drop(conn);

    let report = scanned(&f.snapshot());
    assert!(report.complete && report.exit_code() == 2);
    let places = places(&report);
    for place in [
        ("memories", "id"),
        ("memories", "content"),
        ("memories", "tags"),
        ("memories_fts", "content"),
        ("memories_fts", "tags"),
        ("entities", "name"),
        ("pending_extractions", "last_error"),
    ] {
        assert!(places.contains(&place), "a place is not reported");
    }
    assert!(classes(&report, "memories", "content") == ["provider_token"]);
    assert!(classes(&report, "memories", "tags") == ["private_block"]);
    assert!(classes(&report, "memories_fts", "tags") == ["private_block"]);
    assert!(classes(&report, "entities", "name") == ["provider_token"]);
    let finding = report
        .findings
        .iter()
        .find(|finding| finding.table == "memories" && finding.column == "id")
        .unwrap();
    assert!(finding.row == row && row == 2);
    assert!(report.totals["provider_token"] >= 5 && report.totals["private_block"] >= 1);

    let said = report.to_json();
    for held in [
        token.as_str(),
        &token.to_uppercase(),
        &body(24),
        "Deploy key",
    ] {
        assert!(!said.contains(held), "the report holds what it found");
    }
    assert!(serde_json::from_str::<serde_json::Value>(&said).is_ok());
}

/// A document is read as a document: a value is a credential under the
/// name it is kept under, however deep, and a key is read like a value.
#[test]
fn redaction_scan_reads_a_document_field_by_field() {
    let token = token();
    let value = body(24);
    let f = Fixture::private();
    assert!(crate::redaction::is_clean(&value));
    f.memory(
        "m-1",
        "t",
        "c",
        "[]",
        &serde_json::json!({"deploy": {"password": {"old": [value]}}}).to_string(),
    );
    f.memory(
        "m-2",
        "t",
        "c",
        "[]",
        &serde_json::json!({token.as_str(): 1}).to_string(),
    );
    let report = scanned(&f.snapshot());
    assert!(report.complete && report.exit_code() == 2);
    assert!(
        classes(&report, "memories", "metadata") == ["credential_assignment", "provider_token"]
    );
    let said = report.to_json();
    assert!(!said.contains(&token) && !said.contains(&value) && !said.contains("deploy"));
}

/// A name is read as it is and in lower case, and what either reading
/// finds is counted once.
#[test]
fn redaction_scan_counts_what_a_name_holds_in_either_case() {
    let token = token();
    let upper = token.to_uppercase();
    assert!(crate::redaction::is_clean(&upper));
    let f = Fixture::private();
    let conn = f.conn();
    let names = [
        format!("<private>plan</private> {upper}"),
        format!("{token} {upper}"),
        format!("{token} and {token}"),
    ];
    for (n, name) in names.iter().enumerate() {
        conn.execute(
            "INSERT INTO entities (id, name, entity_type, mention_count, first_seen, last_seen)
             VALUES (?1, ?2, 'concept', 1, '2026-01-02', '2026-01-02')",
            params![format!("e-{n}"), name],
        )
        .unwrap();
    }
    drop(conn);
    let report = scanned(&f.snapshot());
    let found: Vec<Vec<(&str, u32)>> = report
        .findings
        .iter()
        .filter(|finding| finding.table == "entities")
        .map(|finding| finding.classes.iter().map(|(k, v)| (*k, *v)).collect())
        .collect();
    assert!(
        found
            == [
                vec![("private_block", 1), ("provider_token", 1)],
                vec![("provider_token", 2)],
                vec![("provider_token", 2)],
            ]
    );
    assert!(!report.to_json().contains(&token) && !report.to_json().contains(&upper));
}

/// The tags the index reads are the document the memory keeps: a tag
/// that is written with an escape is a token once it is decoded, in the
/// memory and in the index alike.
#[test]
fn redaction_scan_reads_the_tags_of_the_index_as_a_document() {
    let token = token();
    let escaped = format!("[\"\\u0073{}\"]", &token[1..]);
    assert!(crate::redaction::is_clean(&escaped));
    let decoded: Vec<String> = serde_json::from_str(&escaped).unwrap();
    assert!(decoded == [token.clone()]);
    let f = Fixture::private();
    f.memory("m-1", "Rollout", "friday", &escaped, "null");
    let report = scanned(&f.snapshot());
    assert!(report.complete && report.exit_code() == 2);
    assert!(places(&report) == [("memories", "tags"), ("memories_fts", "tags")]);
    assert!(classes(&report, "memories_fts", "tags") == ["provider_token"]);
    assert!(!report.to_json().contains(&token[1..]));
}

/// A table without row numbers is pointed at by the position of the row
/// in the snapshot, by its key.
#[test]
fn redaction_scan_points_at_a_row_without_a_number_by_its_position() {
    let token = token();
    let f = Fixture::private();
    let conn = f.conn();
    for (new, old, actor) in [
        ("b", "a", "ann"),
        ("c", "b", token.as_str()),
        ("d", "c", "ann"),
    ] {
        conn.execute(
            "INSERT INTO memory_updates (new_id, old_id, rule, actor, created_at)
             VALUES (?1, ?2, 'same-subject', ?3, '2026-01-02')",
            params![new, old, actor],
        )
        .unwrap();
    }
    drop(conn);
    let report = scanned(&f.snapshot());
    let table = report
        .coverage
        .tables
        .iter()
        .find(|table| table.table == "memory_updates")
        .unwrap();
    assert!(table.locator == "ordinal" && table.rows == 3);
    let finding = report
        .findings
        .iter()
        .find(|finding| finding.table == "memory_updates")
        .unwrap();
    assert!(finding.column == "actor" && finding.row == 2);
    assert!(!report.to_json().contains(&token));
}

/// What the scanner does not know it does not read and does not name: it
/// counts it, and the scan is not complete, whatever it found elsewhere.
#[test]
fn redaction_scan_is_incomplete_over_what_it_does_not_know() {
    let token = token();
    let changes: [(&str, String, &str); 7] = [
        (
            "UNKNOWN_TABLE",
            format!("CREATE TABLE \"{token}\" (note TEXT)"),
            "unknown_tables",
        ),
        // A name that only looks like one of SQLite's own.
        (
            "UNKNOWN_TABLE",
            format!(
                "CREATE TABLE sqliteXnotes (note TEXT);
                 INSERT INTO sqliteXnotes VALUES ('{token}')"
            ),
            "unknown_tables",
        ),
        // A column that is computed, stored or not, by a name the scanner
        // does not know.
        (
            "UNKNOWN_COLUMN",
            format!("ALTER TABLE memories ADD COLUMN note TEXT AS ('{token}')"),
            "unknown_columns",
        ),
        (
            "UNKNOWN_COLUMN",
            format!(
                "DROP TABLE ingest_facts;
                 CREATE TABLE ingest_facts (name TEXT PRIMARY KEY,
                    value TEXT GENERATED ALWAYS AS ('{token}') STORED);
                 INSERT INTO ingest_facts (name) VALUES ('n')"
            ),
            "unknown_columns",
        ),
        (
            "UNKNOWN_TABLE",
            format!("CREATE VIEW \"{token}\" AS SELECT id FROM memories"),
            "unknown_tables",
        ),
        (
            "UNKNOWN_COLUMN",
            format!("ALTER TABLE memories ADD COLUMN \"{token}\" TEXT"),
            "unknown_columns",
        ),
        (
            "UNSUPPORTED_SCHEMA_VERSION",
            "PRAGMA user_version = 99".to_string(),
            "",
        ),
    ];
    for (reason, change, counter) in changes {
        let f = Fixture::private();
        f.memory(
            "m-1",
            "Deploy key",
            &format!("the key is {token}"),
            "[]",
            "null",
        );
        f.conn().execute_batch(&change).unwrap();
        let report = scanned(&f.snapshot());
        assert!(!report.complete && report.exit_code() == 1);
        assert!(report.reasons == [reason]);
        let coverage = serde_json::to_value(&report.coverage).unwrap();
        assert!(counter.is_empty() || coverage[counter] == 1);
        // What it knows it read.
        assert!(places(&report).contains(&("memories", "content")));
        assert!(!report.to_json().contains(&token));
    }
}

/// The statements the file's objects were made by are text of the file:
/// a default, a check, the body of a trigger. They are read with the
/// schema, and a finding in one is a row of the schema.
#[test]
fn redaction_scan_reads_the_statements_of_the_schema() {
    let token = token();
    let statements = [
        format!("CREATE TRIGGER note AFTER INSERT ON memories BEGIN SELECT '{token}'; END"),
        format!("CREATE INDEX note ON memories (title) WHERE title <> '{token}'"),
    ];
    for statement in statements {
        let f = Fixture::private();
        f.memory("m-1", "Rollout", "friday", "[]", "null");
        assert!(scanned(&f.snapshot()).exit_code() == 0);
        f.conn().execute_batch(&statement).unwrap();
        let report = scanned(&f.snapshot());
        assert!(report.complete && report.exit_code() == 2);
        assert!(places(&report) == [("sqlite_schema", "sql")]);
        assert!(report.findings[0].row > 0);
        assert!(!report.to_json().contains(&token));
    }
}

/// A table is known by its name and by its columns. One that has the name
/// of a table the index keeps for itself, and is not that table, is read
/// like any other: a column the index does not make is an unknown
/// column, and a text where the index keeps bytes is read.
#[test]
fn redaction_scan_reads_a_table_that_has_the_name_of_an_index_table() {
    let token = token();
    // A store that never had an index, and one whose index table was
    // replaced.
    let replaced: [(&str, &str, &str); 2] = [
        (
            "CREATE TABLE memories_fts_data (id INTEGER PRIMARY KEY, leak TEXT)",
            "INSERT INTO memories_fts_data (id, leak) VALUES (1, ?1)",
            "UNKNOWN_COLUMN",
        ),
        (
            "CREATE TABLE memories_fts_data (id INTEGER PRIMARY KEY, block BLOB)",
            "INSERT INTO memories_fts_data (id, block) VALUES (1, ?1)",
            "",
        ),
    ];
    for (create, insert, reason) in replaced {
        let f = Fixture::new();
        let conn = f.conn();
        conn.execute_batch(
            "CREATE TABLE memories (id TEXT PRIMARY KEY, title TEXT, content TEXT);
             INSERT INTO memories VALUES ('m-1', 'Rollout', 'friday');",
        )
        .unwrap();
        conn.execute_batch(create).unwrap();
        conn.execute(insert, [&token]).unwrap();
        drop(conn);
        let report = scanned(&f.snapshot());
        if reason.is_empty() {
            assert!(report.complete && report.exit_code() == 2);
            assert!(places(&report) == [("memories_fts_data", "block")]);
        } else {
            assert!(report.reasons == [reason] && report.exit_code() == 1);
            assert!(report.coverage.unknown_columns == 1);
        }
        assert!(!report.to_json().contains(&token) && !report.to_json().contains("leak"));
    }
}

/// A value that cannot be read as what its column holds makes the scan
/// incomplete. What can be read of it as text is.
#[test]
fn redaction_scan_is_incomplete_over_a_value_it_cannot_decode() {
    let token = token();
    let deep = format!("{}1{}", "[".repeat(64), "]".repeat(64));
    let cases: [(&str, String, &str, u64); 5] = [
        // A document that holds nothing is no document.
        (
            "UNDECODABLE_VALUE",
            " ".to_string(),
            "undecodable_values",
            1,
        ),
        (
            "UNDECODABLE_VALUE",
            format!("{{\"note\": \"{token}\" oops"),
            "undecodable_values",
            1,
        ),
        // Two members of one name: a parser would keep the last and not
        // look at the first.
        (
            "UNDECODABLE_VALUE",
            format!("{{\"note\": \"{token}\", \"note\": \"clean\"}}"),
            "undecodable_values",
            1,
        ),
        ("STRUCTURE_LIMIT", deep, "structures_over_limit", 1),
        // In the memory and in the copy the index keeps of its title.
        ("UNDECODABLE_VALUE", String::new(), "undecodable_values", 2),
    ];
    for (reason, metadata, counter, times) in cases {
        let f = Fixture::private();
        f.memory("m-1", "t", "c", "[]", "null");
        if metadata.is_empty() {
            // Bytes that are no text, where a title is kept.
            f.conn()
                .execute("UPDATE memories SET title = X'ff00fe'", [])
                .unwrap();
        } else {
            f.conn()
                .execute("UPDATE memories SET metadata = ?1", [&metadata])
                .unwrap();
        }
        let report = scanned(&f.snapshot());
        assert!(!report.complete && report.exit_code() == 1);
        assert!(report.reasons == [reason]);
        assert!(serde_json::to_value(&report.coverage).unwrap()[counter] == times);
        if metadata.contains(&token) {
            assert!(classes(&report, "memories", "metadata") == ["provider_token"]);
        }
        assert!(!report.to_json().contains(&token));
    }
}

/// The file has to be one regular file that nothing has open, and a
/// database the scanner knows. What is refused is refused in a fixed
/// word, and nothing is said of the path.
#[test]
fn redaction_scan_refuses_what_is_no_closed_snapshot() {
    let token = token();
    let f = Fixture::private();
    let good = f.snapshot();
    let dir = good.parent().unwrap().to_path_buf();

    let named = dir.join(format!("{token}.db"));
    let link = dir.join("link.db");
    std::os::unix::fs::symlink(&good, &link).unwrap();
    let text = dir.join("notes.db");
    std::fs::write(
        &text,
        format!("the key is {token}, and this is no database"),
    )
    .unwrap();
    let empty = dir.join("empty.db");
    std::fs::write(&empty, "").unwrap();
    let other = dir.join("other.db");
    Connection::open(&other)
        .unwrap()
        .execute_batch("CREATE TABLE notes (body TEXT)")
        .unwrap();

    let refused: [(&Path, &str); 6] = [
        (&named, "NOT_FOUND"),
        (&dir, "NOT_A_REGULAR_FILE"),
        (&link, "NOT_A_REGULAR_FILE"),
        (&text, "NOT_A_DATABASE"),
        (&empty, "NOT_A_DATABASE"),
        (&other, "UNSUPPORTED_SCHEMA"),
    ];
    for (path, reason) in refused {
        let before = listing(&dir);
        let report = scan(path);
        assert!(!report.complete && report.exit_code() == 1);
        assert!(report.reasons == [reason]);
        assert!(report.findings.is_empty());
        assert!(!report.to_json().contains(&token));
        assert!(listing(&dir) == before, "the scan left a file");
    }

    // A journal beside the file: the file is open, or was not closed.
    for suffix in ["-wal", "-shm", "-journal"] {
        let f = Fixture::private();
        let snapshot = f.snapshot();
        let mut beside = snapshot.clone().into_os_string();
        beside.push(suffix);
        std::fs::write(&beside, "").unwrap();
        let report = scanned(&snapshot);
        assert!(report.reasons == ["SIDECAR_PRESENT"] && report.exit_code() == 1);
    }
    // The good one, by a name that holds a token.
    std::fs::rename(&good, &named).unwrap();
    let report = scan(&named);
    assert!(report.complete && !report.to_json().contains(&token));
}

/// The stores beside the private one.
#[test]
fn redaction_scan_reads_the_shared_and_the_activity_store() {
    let token = token();

    let f = Fixture::new();
    {
        let store = crate::shared::store::SharedStore::open(&f.live()).unwrap();
        store
            .publish(
                "alpha",
                "brief",
                "Brief",
                "Reviewed project brief",
                "owner:fixture",
            )
            .unwrap();
    }
    f.conn()
        .execute(
            "INSERT INTO shared_observations
                (id, project_id, writer_id, request_id, title, content, source, status, created_at)
             VALUES ('o-1', 'alpha', ?1, 'r-1', 'Note', ?2, 'chat:1', 'pending', '2026-01-02')",
            params![token, format!("the key is {token}")],
        )
        .unwrap();
    let report = scanned(&f.snapshot());
    assert!(report.store == Some("shared") && report.schema_version == Some(2));
    assert!(report.complete && report.exit_code() == 2);
    let places_found = places(&report);
    assert!(places_found.contains(&("shared_observations", "writer_id")));
    assert!(places_found.contains(&("shared_observations", "content")));
    assert!(
        !places_found
            .iter()
            .any(|(table, _)| *table == "shared_records")
    );
    assert!(!report.to_json().contains(&token));

    let f = Fixture::new();
    drop(crate::activity::ActivityStore::open(&f.live()).unwrap());
    let clean = scanned(&f.snapshot());
    assert!(clean.store == Some("activity") && clean.complete && clean.exit_code() == 0);
    f.conn()
        .execute(
            "INSERT INTO project_attribution (day, project_key, seconds, confidence)
             VALUES ('2026-01-02', ?1, 60.0, 'high')",
            [&token],
        )
        .unwrap();
    let report = scanned(&f.snapshot());
    assert!(report.complete && report.exit_code() == 2);
    assert!(places(&report) == [("project_attribution", "project_key")]);
    let _ = Store::Activity.table("sqlite_stat1").unwrap();
    assert!(!report.to_json().contains(&token));
}

/// What SQLite keeps aside while it answers a query is kept in memory,
/// and the connection can write nothing.
#[test]
fn redaction_scan_connection_keeps_nothing_on_disk() {
    let f = Fixture::private();
    let (_snapshot, conn) = super::snapshot::Snapshot::open(&f.snapshot()).ok().unwrap();
    let setting = |name: &str| -> i64 {
        conn.pragma_query_value(None, name, |row| row.get(0))
            .unwrap()
    };
    assert!(
        setting("temp_store") == 2,
        "temporary data can go to a file"
    );
    assert!(setting("query_only") == 1);
    assert!(setting("trusted_schema") == 0);
    assert!(conn.execute("DELETE FROM memories", []).is_err());
}

/// A file that changes while it is read, or a file that appears beside
/// it, is seen.
#[test]
fn redaction_scan_sees_a_snapshot_change_under_it() {
    use std::io::Write;
    let f = Fixture::private();

    let path = f.snapshot();
    let (snapshot, conn) = super::snapshot::Snapshot::open(&path).ok().unwrap();
    assert!(snapshot.unchanged());
    drop(conn);
    std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(b"x")
        .unwrap();
    assert!(!snapshot.unchanged());

    let path = f.snapshot();
    let (snapshot, _conn) = super::snapshot::Snapshot::open(&path).ok().unwrap();
    std::fs::write(path.parent().unwrap().join("snapshot.db-wal"), "").unwrap();
    assert!(!snapshot.unchanged());
}

/// What was read of a snapshot that changed under the scan is no report
/// of it, whatever was found.
#[test]
fn redaction_scan_of_a_snapshot_that_changed_is_incomplete() {
    use std::io::Write;
    let token = token();
    let f = Fixture::private();
    f.memory(
        "m-1",
        "Deploy key",
        &format!("the key is {token}"),
        "[]",
        "null",
    );
    let path = f.snapshot();
    let report = scan_then(&path, || {
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"x")
            .unwrap();
    });
    assert!(report.reasons == ["CHANGED_DURING_SCAN"] && report.exit_code() == 1);
    assert!(places(&report).contains(&("memories", "content")));
}

/// A store that was closed keeps the journal mode it was written in. The
/// scanner opens it as a file that cannot change, so SQLite makes no
/// journal beside it.
#[test]
fn redaction_scan_of_a_closed_store_written_with_a_journal_makes_no_file() {
    let f = Fixture::private();
    f.memory("m-1", "Rollout", "friday", "[]", "null");
    let mode: String = f
        .conn()
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .unwrap();
    assert!(mode == "wal");
    let dir = f.dir.path().join("copy");
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("store.db");
    std::fs::copy(f.live(), &path).unwrap();
    let report = scanned(&path);
    assert!(report.complete && report.exit_code() == 0);
    assert!(listing(&dir) == ["store.db"]);
}

/// More findings than are listed are counted all the same.
#[test]
fn redaction_scan_counts_what_it_does_not_list() {
    let mut report = Report::new();
    let counts = crate::redaction::redact_text(&token()).counts;
    for row in 0..(report::MAX_LISTED as i64 + 3) {
        report.found("memories", "content", row, &counts);
    }
    assert!(report.findings.len() == report::MAX_LISTED && report.findings_not_listed == 3);
    assert!(report.totals["provider_token"] == report::MAX_LISTED as u64 + 3);
    assert!(report.exit_code() == 2);
    report.incomplete(Reason::UnknownTable);
    report.incomplete(Reason::UnknownTable);
    assert!(report.exit_code() == 1 && report.reasons == ["UNKNOWN_TABLE"]);
}

#[test]
fn redaction_scan_knows_its_own_command_line() {
    let line = |words: &[&str]| is_scan_invocation(words.iter().map(std::ffi::OsString::from));
    assert!(line(&["redact", "scan", "--db", "x"]));
    assert!(line(&["--home", "/p", "redact"]));
    assert!(line(&["--home=/p", "redact", "scan", "stray"]));
    assert!(!line(&["save", "--title", "redact", "x"]));
    assert!(!line(&["--home", "redact", "save"]));
    assert!(!line(&[]));
    // A directory whose name is no text.
    use std::os::unix::ffi::OsStringExt;
    let bytes = |text: &[u8]| std::ffi::OsString::from_vec(text.to_vec());
    assert!(is_scan_invocation([
        bytes(b"--home=/p\xff"),
        bytes(b"redact")
    ]));
    assert!(is_scan_invocation([
        bytes(b"--home"),
        bytes(b"/p\xff"),
        bytes(b"redact"),
        bytes(b"scan"),
    ]));
    assert!(!is_scan_invocation([bytes(b"\xff"), bytes(b"redact")]));
}

/// Every table of a store made by this build is one the scanner knows,
/// column by column: a column added to the schema and not to the scanner
/// fails here, not in somebody's scan.
#[test]
fn redaction_scan_knows_every_table_and_column_of_this_build() {
    let f = Fixture::private();
    let report = scanned(&f.snapshot());
    assert!(report.complete);
    let of_the_store: Vec<&TableCoverage> = report
        .coverage
        .tables
        .iter()
        .filter(|table| !table.table.starts_with("sqlite_"))
        .collect();
    assert!(of_the_store.len() == Store::Private.tables().len());
    assert!(
        report
            .coverage
            .tables
            .iter()
            .any(|t| t.table == "sqlite_schema")
    );
    assert!(
        report
            .coverage
            .tables
            .iter()
            .any(|t| t.table == "sqlite_sequence")
    );
    for table in of_the_store {
        let spec = Store::Private
            .tables()
            .iter()
            .find(|spec| spec.name == table.table)
            .unwrap();
        assert!(spec.columns.len() == table.columns.len());
    }
}
