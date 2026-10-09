//! Independent handles and processes must share attribution write ordering.
use super::*;
use crate::activity::ActivityStore;
use crate::attribution::AttribCfg;
use crate::graph::{Entity, EntityType};
use crate::owner_time_day::{self, Calendar, repair_live_after_entity_undo};
use crate::timeline::fixture::{Fixture, Zone, at};
use chrono::Duration;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration as Wait, Instant};

const DEADLINE: Wait = Wait::from_secs(10);

fn fixture() -> (Fixture, String) {
    let f = Fixture::empty();
    let entity = f
        .storage
        .upsert_entity(&Entity {
            name: "locked-work".into(),
            entity_type: EntityType::Project,
        })
        .unwrap();
    f.storage.entity_archive_set(&entity, true, 10).unwrap();
    let project = f
        .storage
        .project_upsert("locked-work", "locked-work", None)
        .unwrap();
    f.storage
        .project_link_entity(project, "locked-work")
        .unwrap();
    f.segment(
        "rule-start",
        "codex",
        true,
        Some(project),
        at(0, 0),
        at(0, 1),
    );
    f.active(at(9, 0), at(9, 10));
    f.memory("locked-fallback", at(9, 2), project);
    assert_eq!(
        f.storage
            .project_by_key("locked-work")
            .unwrap()
            .unwrap()
            .entity_id,
        Some(entity.clone())
    );
    owner_time_day::recompute_day(
        &f.activity,
        &f.storage,
        &Zone::UTC,
        at(0, 0).date_naive(),
        &AttribCfg::default(),
    )
    .unwrap();
    (f, entity)
}

fn stale_day(activity: &ActivityStore, storage: &Storage) -> owner_time_day::Day {
    let made = owner_time_day::compute(
        activity,
        storage,
        Zone::UTC.bounds(at(0, 0).date_naive()).unwrap(),
        &AttribCfg::default(),
    )
    .unwrap();
    assert!(made.rows.is_empty());
    assert_eq!(made.unattributed, 600.0);
    made
}

fn repair(f: &Fixture) -> usize {
    repair_live_after_entity_undo(
        &f.storage,
        Some(&f.activity),
        &Zone::UTC,
        at(23, 0) + Duration::days(1),
        &AttribCfg::default(),
    )
    .unwrap()
}

fn restored(f: &Fixture, entity: &str) {
    let day = f
        .storage
        .rekeyed_day_with_unattributed(
            f.activity
                .project_attribution_for_day(at(0, 0).date_naive())
                .unwrap(),
        )
        .unwrap();
    assert_eq!(day, (vec![(entity.into(), 600.0, Some("low".into()))], 0.0));
}

#[test]
fn curated_graph_attribution_lock_serializes_independent_store_and_activity_handles() {
    let (f, entity) = fixture();
    let other_store = Storage::open(&f.memory_path).unwrap();
    let other_activity = ActivityStore::open(&f.activity_path).unwrap();
    let (ready_tx, ready_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let (started_tx, started_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    std::thread::scope(|scope| {
        let writer = scope.spawn(move || {
            let _write = other_store.attribution_write_lock().unwrap();
            let made = stale_day(&other_activity, &other_store);
            ready_tx.send(()).unwrap();
            release_rx.recv_timeout(DEADLINE).unwrap();
            other_activity
                .write_day(at(0, 0).date_naive(), &[], made.unattributed)
                .unwrap();
        });
        ready_rx.recv_timeout(DEADLINE).unwrap();
        f.storage.entity_archive_set(&entity, false, 11).unwrap();
        let undo_fixture = &f;
        let undo = scope.spawn(move || {
            started_tx.send(()).unwrap();
            done_tx.send(repair(undo_fixture)).unwrap();
        });
        started_rx.recv_timeout(DEADLINE).unwrap();
        let pending = done_rx.recv_timeout(Wait::from_millis(30));
        release_tx.send(()).unwrap();
        writer.join().unwrap();
        undo.join().unwrap();
        assert!(matches!(pending, Err(mpsc::RecvTimeoutError::Timeout)));
        assert_eq!(done_rx.recv_timeout(DEADLINE).unwrap(), 1);
    });
    restored(&f, &entity);
}

#[test]
fn curated_graph_attribution_lock_keeps_distinct_databases_independent() {
    let first = Fixture::empty();
    let second = Fixture::empty();
    let held = first.storage.attribution_write_lock().unwrap();
    let (done_tx, done_rx) = mpsc::channel();
    std::thread::scope(|scope| {
        let writer = scope.spawn(|| {
            let _write = second.storage.attribution_write_lock().unwrap();
            done_tx.send(()).unwrap();
        });
        let completed = done_rx.recv_timeout(DEADLINE);
        drop(held);
        writer.join().unwrap();
        assert!(completed.is_ok());
    });
}

#[test]
fn curated_graph_attribution_lock_in_memory_guard_needs_no_file() {
    let connection = rusqlite::Connection::open_in_memory().unwrap();
    assert!(connection.path().is_none_or(str::is_empty));
    let local = AttributionWriteLock::new(connection.path()).unwrap();
    {
        let _write = local.acquire().unwrap();
        assert!(local.file.try_lock().is_err());
    }
    assert!(local.path.is_none());
    assert!(local.file.try_lock().unwrap().is_none());
}

#[cfg(unix)]
#[test]
fn curated_graph_attribution_lock_resolves_symlink_database_aliases() {
    let f = Fixture::empty();
    let alias = f.memory_path.with_file_name("memory-alias.db");
    std::os::unix::fs::symlink(&f.memory_path, &alias).unwrap();
    let alias_store = Storage::open(&alias).unwrap();
    assert_eq!(
        f.storage.attribution_write.path,
        alias_store.attribution_write.path
    );
    assert!(!f.storage.attribution_write.path.as_ref().unwrap().exists());
    let held = f.storage.attribution_write_lock().unwrap();
    let (started_tx, started_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    std::thread::scope(|scope| {
        let writer = scope.spawn(move || {
            started_tx.send(()).unwrap();
            let _write = alias_store.attribution_write_lock().unwrap();
            done_tx.send(()).unwrap();
        });
        started_rx.recv_timeout(DEADLINE).unwrap();
        let pending = done_rx.recv_timeout(Wait::from_millis(30));
        drop(held);
        writer.join().unwrap();
        assert!(matches!(pending, Err(mpsc::RecvTimeoutError::Timeout)));
        done_rx.recv_timeout(DEADLINE).unwrap();
    });
    assert!(f.storage.attribution_write.path.as_ref().unwrap().exists());
    assert!(
        !alias
            .with_file_name("memory-alias.db.attribution-lock")
            .exists()
    );
}

#[test]
fn curated_graph_attribution_lock_is_lazy_empty_and_persistent() {
    let f = Fixture::empty();
    let path = f.storage.attribution_write.path.clone().unwrap();
    assert_eq!(
        path,
        std::fs::canonicalize(&f.memory_path)
            .unwrap()
            .with_file_name("memory.db.attribution-lock")
    );
    assert!(!path.exists());
    let reopened = Storage::open(&f.memory_path).unwrap();
    let before = f.stores();
    let changes = f.changes();
    reopened.attribution_keys().unwrap();
    f.storage.attribution_keys().unwrap();
    assert!(
        !path.exists(),
        "opening stores and reading must not create a lock"
    );
    {
        let _write = reopened.attribution_write_lock().unwrap();
    }
    let created = std::fs::metadata(&path).unwrap();
    assert_eq!(created.len(), 0);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(created.permissions().mode() & 0o777, 0o600);
    }
    for _ in 0..3 {
        let _write = reopened.attribution_write_lock().unwrap();
        let current = std::fs::metadata(&path).unwrap();
        assert_eq!(current.len(), 0);
        assert_eq!(current.modified().unwrap(), created.modified().unwrap());
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            assert_eq!(current.ino(), created.ino());
        }
    }
    drop(reopened);
    assert!(
        path.is_file(),
        "dropping a store must never unlink the lock"
    );
    let _write = f.storage.attribution_write_lock().unwrap();
    assert!(std::fs::read(&path).unwrap().is_empty());
    assert_eq!(f.stores(), before);
    assert_eq!(f.changes(), changes);
}

#[test]
fn curated_graph_attribution_lock_rejects_nonempty_and_nonregular_paths() {
    let f = Fixture::empty();
    let path = f.storage.attribution_write.path.clone().unwrap();
    std::fs::write(&path, b"reserved collision").unwrap();
    assert!(f.storage.attribution_write_lock().is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"reserved collision");
    assert!(f.storage.attribution_write.file.lock().unwrap().is_none());
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    assert!(f.storage.attribution_write_lock().is_err());
    assert!(path.is_dir());
}

#[cfg(unix)]
#[test]
fn curated_graph_attribution_lock_rejects_symlinks_and_hardlinks() {
    let f = Fixture::empty();
    let path = f.storage.attribution_write.path.clone().unwrap();
    let target = path.with_file_name("unrelated-empty-file");
    std::fs::write(&target, []).unwrap();
    std::os::unix::fs::symlink(&target, &path).unwrap();
    assert!(f.storage.attribution_write_lock().is_err());
    assert!(std::fs::symlink_metadata(&path).unwrap().is_symlink());
    assert!(std::fs::read(&target).unwrap().is_empty());
    std::fs::remove_file(&path).unwrap();
    std::fs::hard_link(&target, &path).unwrap();
    assert!(f.storage.attribution_write_lock().is_err());
    assert!(std::fs::read(&target).unwrap().is_empty());
    assert!(f.storage.attribution_write.file.lock().unwrap().is_none());
}

struct ChildRun(Child);
impl Drop for ChildRun {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}
impl ChildRun {
    fn wait(&mut self) -> std::process::ExitStatus {
        let until = Instant::now() + DEADLINE;
        loop {
            if let Some(status) = self.0.try_wait().unwrap() {
                return status;
            }
            assert!(
                Instant::now() < until,
                "isolated attribution writer did not exit"
            );
            std::thread::sleep(Wait::from_millis(10));
        }
    }
}

#[test]
fn curated_graph_attribution_lock_serializes_a_real_child_process_before_undo() {
    let (f, entity) = fixture();
    let mut child = ChildRun(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "storage::attribution_lock::tests::curated_graph_attribution_lock_child",
                "--nocapture",
            ])
            .env("MNEMONIC_ATTRIBUTION_LOCK_CASE", "stale-writer")
            .env("MNEMONIC_ATTRIBUTION_LOCK_MEMORY", &f.memory_path)
            .env("MNEMONIC_ATTRIBUTION_LOCK_ACTIVITY", &f.activity_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
    );
    let output = child.0.stdout.take().unwrap();
    let (ready_tx, ready_rx) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut lines = Vec::new();
        for line in BufReader::new(output).lines() {
            let line = line.unwrap();
            if line.contains("attribution child ready") {
                ready_tx.send(()).unwrap();
            }
            lines.push(line);
        }
        lines.join("\n")
    });
    ready_rx.recv_timeout(DEADLINE).unwrap();
    f.storage.entity_archive_set(&entity, false, 11).unwrap();
    let (started_tx, started_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let undo_fixture = &f;
    std::thread::scope(move |scope| {
        // Own the child inside the scope so a failed handshake kills it and
        // releases its lock before the scope joins the waiting undo thread.
        let mut child = child;
        let undo = scope.spawn(move || {
            started_tx.send(()).unwrap();
            done_tx.send(repair(undo_fixture)).unwrap();
        });
        started_rx.recv_timeout(DEADLINE).unwrap();
        let pending = done_rx.recv_timeout(Wait::from_millis(30));
        let input = child.0.stdin.as_mut().unwrap();
        input.write_all(b"release\n").unwrap();
        input.flush().unwrap();
        assert!(child.wait().success());
        undo.join().unwrap();
        assert!(matches!(pending, Err(mpsc::RecvTimeoutError::Timeout)));
        assert_eq!(done_rx.recv_timeout(DEADLINE).unwrap(), 1);
    });
    let output = reader.join().unwrap();
    assert!(
        output.contains("attribution child write complete"),
        "{output}"
    );
    restored(&f, &entity);
}

#[test]
fn curated_graph_attribution_lock_child() {
    let Ok(case) = std::env::var("MNEMONIC_ATTRIBUTION_LOCK_CASE") else {
        return;
    };
    assert_eq!(case, "stale-writer");
    let memory = std::env::var_os("MNEMONIC_ATTRIBUTION_LOCK_MEMORY").unwrap();
    let activity = std::env::var_os("MNEMONIC_ATTRIBUTION_LOCK_ACTIVITY").unwrap();
    let storage = Storage::open(std::path::Path::new(&memory)).unwrap();
    let activity = ActivityStore::open(std::path::Path::new(&activity)).unwrap();
    let _write = storage.attribution_write_lock().unwrap();
    let made = stale_day(&activity, &storage);
    println!("attribution child ready");
    std::io::stdout().flush().unwrap();
    let mut command = String::new();
    std::io::stdin().read_line(&mut command).unwrap();
    assert_eq!(command, "release\n");
    activity
        .write_day(at(0, 0).date_naive(), &[], made.unattributed)
        .unwrap();
    println!("attribution child write complete");
}
