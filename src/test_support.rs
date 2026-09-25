//! Scratch space for tests that cleans up after itself.
//!
//! Tests used to create `std::env::temp_dir()/<prefix><uuid>` directories and
//! never remove them, so every `cargo test` run left hundreds behind (about
//! 22 GB in three days). Everything here is backed by [`tempfile::TempDir`]:
//! the directory is removed when its guard is dropped, including while a
//! failing test unwinds.

use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};

use tempfile::TempDir;

/// A fresh, empty directory under the system temp dir, named
/// `<prefix><random>` and removed when the returned guard is dropped.
pub fn temp_dir(prefix: &str) -> TempDir {
    tempfile::Builder::new()
        .prefix(prefix)
        .tempdir()
        .expect("create temp dir")
}

/// Path to `file_name` inside a fresh temporary directory. The file itself is
/// not created; the directory lives as long as the returned guard.
pub fn temp_path(prefix: &str, file_name: &str) -> InTempDir<PathBuf> {
    InTempDir::new(prefix, |dir| dir.join(file_name))
}

/// Path for a unix socket in a fresh directory under `/tmp`, removed when the
/// returned guard is dropped. macOS caps socket paths at 104 bytes, which a
/// socket under the system temp dir (`/var/folders/...`) can exceed.
pub fn temp_socket_path(prefix: &str) -> InTempDir<PathBuf> {
    let dir = tempfile::Builder::new()
        .prefix(prefix)
        .tempdir_in("/tmp")
        .expect("create socket temp dir");
    let value = dir.path().join("s.sock");
    InTempDir { value, _dir: dir }
}

/// A value together with the temporary directory it lives in, e.g. a
/// `Storage` whose database file sits in that directory. Derefs to the value.
/// On drop the value goes first (closing anything it holds open), then the
/// directory is removed.
pub struct InTempDir<T> {
    value: T,
    _dir: TempDir,
}

impl<T> InTempDir<T> {
    /// Build a value inside a fresh temporary directory.
    pub fn new(prefix: &str, build: impl FnOnce(&Path) -> T) -> Self {
        let dir = temp_dir(prefix);
        let value = build(dir.path());
        Self { value, _dir: dir }
    }
}

impl<T> Deref for InTempDir<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.value
    }
}

impl<T> DerefMut for InTempDir<T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.value
    }
}

impl<T: AsRef<Path>> AsRef<Path> for InTempDir<T> {
    fn as_ref(&self) -> &Path {
        self.value.as_ref()
    }
}

/// An embedder that gives every text the same unit vector, so every pair of
/// memories is a near duplicate (cosine 1.0). A test that passes with it
/// proves the rule under test, not a lucky distance between two texts.
#[cfg(test)]
pub struct ConstEmbedder;

#[cfg(test)]
impl crate::embedding::Embedder for ConstEmbedder {
    fn embed(&self, _text: &str) -> anyhow::Result<crate::embedding::Embedding> {
        let mut vector = vec![0.0; crate::embedding::EMBED_DIMS];
        vector[0] = 1.0;
        Ok(vector)
    }

    fn model_id(&self) -> &'static str {
        "const-test"
    }
}

/// What `legacy_facts_fixture` seeded: every row, and each chain's current
/// value as (subject, predicate, value).
#[cfg(test)]
pub struct LegacyFacts {
    pub rows: usize,
    pub current: Vec<(String, String, String)>,
}

/// Write one fact the way the old `Storage::add_fact` did, straight into
/// the old table: close the current row, insert the new one. This is what
/// an old binary still running does after the upgrade.
#[cfg(test)]
pub fn legacy_add_fact_sql(
    conn: &rusqlite::Connection,
    subject: &str,
    predicate: &str,
    value: &str,
    valid_from: &str,
) -> String {
    let id = uuid::Uuid::new_v4().to_string();
    conn.execute(
        "UPDATE facts SET valid_to = ?3 WHERE subject = ?1 AND predicate = ?2 AND valid_to IS NULL",
        rusqlite::params![subject, predicate, valid_from],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO facts (id, subject, predicate, value, valid_from, valid_to, confidence,
             source_memory_id, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, NULL, 1.0, 'manual', ?5)",
        rusqlite::params![id, subject, predicate, value, valid_from],
    )
    .unwrap();
    id
}

/// An old-shaped facts table: 36 rows in 14 chains over 7 subjects, the
/// way agents used it (free predicates, one long-running checkpoint, a
/// 2105-character value), every source "manual", micro-second `+00:00`
/// times. Creates the old table if needed.
#[cfg(test)]
pub fn legacy_facts_fixture(conn: &rusqlite::Connection) -> LegacyFacts {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS facts (
             id TEXT PRIMARY KEY,
             subject TEXT NOT NULL,
             predicate TEXT NOT NULL,
             value TEXT NOT NULL,
             valid_from TEXT NOT NULL,
             valid_to TEXT,
             confidence REAL NOT NULL DEFAULT 1.0,
             source_memory_id TEXT NOT NULL,
             created_at TEXT NOT NULL DEFAULT (datetime('now'))
         );",
    )
    .unwrap();
    let chains: [(&str, &str, usize); 14] = [
        ("widget", "has-price", 3),
        ("gadget", "has-price", 2),
        ("alpha-store", "checkpoint", 11),
        ("alpha-store", "next-step", 2),
        ("gizmo-app", "status", 3),
        ("gizmo-app", "owner", 1),
        ("beta-lab", "deadline", 2),
        ("beta-lab", "notes", 1),
        ("site", "domain", 1),
        ("site", "host", 2),
        ("pipeline", "stage", 3),
        ("pipeline", "budget", 1),
        ("widget", "version", 2),
        ("gadget", "supplier", 2),
    ];
    let mut rows = 0;
    let mut current = Vec::new();
    for (chain, (subject, predicate, length)) in chains.iter().enumerate() {
        let mut last = String::new();
        for step in 0..*length {
            let value = if *predicate == "notes" {
                format!("long note {}", "x".repeat(2095))
            } else {
                format!("{predicate} value number {step} of chain {chain}")
            };
            let at = format!(
                "2026-0{}-{:02}T10:00:00.{:06}+00:00",
                5 + chain % 4,
                step + 1,
                chain * 1000 + step
            );
            legacy_add_fact_sql(conn, subject, predicate, &value, &at);
            rows += 1;
            last = value;
        }
        current.push((subject.to_string(), predicate.to_string(), last));
    }
    LegacyFacts { rows, current }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directory_is_removed_on_drop_after_the_value() {
        let db = temp_path("mnemonic-support-", "memory.db");
        std::fs::write(&*db, b"x").unwrap();
        let dir = db.parent().unwrap().to_path_buf();
        assert!(dir.is_dir());
        drop(db);
        assert!(!dir.exists());
    }
}
