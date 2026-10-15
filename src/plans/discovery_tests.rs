use super::*;
use crate::plans::{Config, Stage};

fn write(root: &Path, relative: &str, text: &str) {
    let path = root.join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn input(root: &Path) -> Input {
    Input {
        main: root.into(),
        config: Config::default(),
        checkouts: Vec::new(),
    }
}

fn task(root: &Path, folder: &str, done: bool) {
    write(
        root,
        &format!("openspec/changes/{folder}/tasks.md"),
        &format!(
            "- [{}] T01: Complete the fixture\n",
            if done { "x" } else { " " }
        ),
    );
}

#[test]
fn plan_archive_keeps_latest_closed_unit_with_its_original_gate_id() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    task(root, "active", false);
    // Create the older archive last: file modification order must not win.
    task(root, "archive/2002-01-02-a-newer", true);
    task(root, "archive/2001-01-01-z-older", true);
    write(
        root,
        "target/checks/a-newer/1/verification.json",
        r#"{"change":"a-newer","passed":false}"#,
    );
    write(
        root,
        "target/checks/z-older/1/verification.json",
        r#"{"change":"z-older","passed":false}"#,
    );
    let mut reader = Reader::default();
    let source = input(root);
    let now = Utc::now();
    let plan = reader.read(&source, now);
    assert_eq!(plan.units.len(), 2);
    assert!(plan.units.iter().any(|unit| unit.id == "active"));
    let closed = plan.units.iter().find(|unit| unit.id == "a-newer").unwrap();
    assert_eq!((closed.done, closed.total), (1, 1));
    assert_eq!(closed.gate.status, gate::GateStatus::Failed);
    assert_eq!(
        closed.stage,
        Stage::Review,
        "an archive does not imply a passed gate"
    );
    assert!(plan.units.iter().all(|unit| unit.id != "z-older"));
    let archived = chrono::NaiveDate::from_ymd_opt(2002, 1, 2)
        .unwrap()
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_utc()
        .timestamp();
    assert_eq!((closed.closed, closed.closed_at), (true, Some(archived)));
    let active = plan.units.iter().find(|unit| unit.id == "active").unwrap();
    assert_eq!((active.closed, active.closed_at), (false, None));
    let served = serde_json::to_value(closed).unwrap();
    assert_eq!(served["closed"], true);
    assert_eq!(served["closed_at"], archived);
    let parsed = reader.parsed_count();
    assert_eq!(reader.read(&source, now), plan);
    assert_eq!(reader.parsed_count(), parsed);
    std::fs::remove_file(root.join("openspec/changes/archive/2002-01-02-a-newer/tasks.md"))
        .unwrap();
    let plan = reader.read(&source, now);
    assert!(plan.units.iter().any(|unit| unit.id == "z-older"));
    assert!(plan.units.iter().all(|unit| unit.id != "a-newer"));
}

#[test]
fn plan_archive_and_completed_active_units_share_one_closed_slot() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    task(root, "working", false);
    task(root, "just-completed", true);
    task(root, "archive/2001-01-01-earlier", true);
    let plan = Reader::default().read(&input(root), Utc::now());
    assert_eq!(plan.units.len(), 2);
    assert!(plan.units.iter().any(|unit| unit.id == "working"));
    assert!(
        plan.units
            .iter()
            .any(|unit| unit.id == "just-completed" && unit.stage == Stage::Done)
    );
    assert!(plan.units.iter().all(|unit| unit.id != "earlier"));
    // Done with no closure time recorded: closed, and no time under the name.
    let done = plan
        .units
        .iter()
        .find(|unit| unit.id == "just-completed")
        .unwrap();
    assert_eq!((done.closed, done.closed_at), (true, None));
    let served = serde_json::to_value(done).unwrap();
    assert_eq!(served["closed_at"], serde_json::Value::Null);
}

#[test]
fn plan_archive_parses_only_latest_and_never_overrides_a_reopened_unit() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    task(root, "working", false);
    for day in 1..=24 {
        task(
            root,
            &format!("archive/2001-01-{day:02}-closed-{day:02}"),
            true,
        );
    }
    let mut reader = Reader::default();
    let source = input(root);
    let plan = reader.read(&source, Utc::now());
    assert_eq!(reader.parsed_count(), 2, "old archive files are not parsed");
    assert!(plan.units.iter().any(|unit| unit.id == "closed-24"));
    task(root, "archive/2001-01-27-working", true);
    task(root, "archive/2001-01-26-newest", true);
    let plan = reader.read(&source, Utc::now());
    assert_eq!(reader.parsed_count(), 3);
    let working = plan.units.iter().find(|unit| unit.id == "working").unwrap();
    assert_eq!((working.done, working.total, working.checkouts), (0, 1, 0));
    assert!(plan.units.iter().any(|unit| unit.id == "newest"));
    assert!(plan.units.iter().all(|unit| unit.id != "closed-24"));
}

#[test]
fn plan_invalid_authority_never_falls_back_to_registry_or_projections() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    task(root, "projection", true);
    write(
        root,
        "tasks.json",
        r#"[{"id":"T01","title":"Registry","unit":"registry","status":"done"}]"#,
    );
    let mut reader = Reader::default();
    let source = input(root);
    let now = Utc::now();
    for (declaration, error) in [
        (
            "task_status_authority: ../tasks.md",
            "invalid-task-authority",
        ),
        (
            "task_status_authority: specs/tasks.md\nstatus_authority: specs/other.md",
            "ambiguous-task-authority",
        ),
    ] {
        write(root, "openspec/config.yaml", declaration);
        let plan = reader.read(&source, now);
        assert_eq!(plan.units.len(), 1);
        assert_eq!(plan.units[0].stage, Stage::Blocked);
        assert_eq!(plan.units[0].error.as_deref(), Some(error));
        assert_eq!((plan.units[0].done, plan.units[0].total), (0, 0));
        assert!(!serde_json::to_string(&plan).unwrap().contains(declaration));
        assert_eq!(
            reader.read(&source, now + chrono::Duration::seconds(10)),
            plan
        );
    }
}

#[test]
fn plan_unreadable_configuration_is_an_error_until_repaired() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    task(root, "projection", true);
    let config = root.join("openspec/config.yaml");
    std::fs::write(&config, [255, 254]).unwrap();
    let mut reader = Reader::default();
    let mut source = input(root);
    let plan = reader.read(&source, Utc::now());
    assert_eq!(plan.units[0].stage, Stage::Blocked);
    assert_eq!(
        plan.units[0].error.as_deref(),
        Some("plan configuration unreadable")
    );
    write(root, "chosen.md", "- [ ] T01: Explicit authority\n");
    source.config.authority = Some("chosen.md".into());
    let plan = reader.read(&source, Utc::now());
    assert_eq!((plan.units[0].done, plan.units[0].total), (0, 1));
    assert!(
        plan.units[0].error.is_none(),
        "explicit configuration overrides discovery"
    );
    source.config.authority = None;
    std::fs::remove_file(config).unwrap();
    let plan = reader.read(&source, Utc::now());
    assert_eq!(plan.units[0].id, "projection");
    assert_eq!(plan.units[0].stage, Stage::Done);
}

#[test]
fn plan_declared_authority_excludes_archived_projections_too() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    task(root, "archive/2001-01-01-projection", true);
    write(
        root,
        "openspec/config.yaml",
        "task_status_authority: tasks.md\n",
    );
    write(
        root,
        "tasks.md",
        "## OpenSpec: authority\n- [ ] T01: Pending work\n",
    );
    let plan = Reader::default().read(&input(root), Utc::now());
    assert_eq!(plan.units.len(), 1);
    assert_eq!(plan.units[0].id, "authority");
    assert_eq!((plan.units[0].done, plan.units[0].total), (0, 1));
}

#[test]
fn plan_missing_main_has_no_plan_and_can_use_an_idle_configured_checkout() {
    let temp = tempfile::tempdir().unwrap();
    let main = temp.path().join("removed-project");
    let alternate = temp.path().join("remaining-checkout");
    let mut source = input(&main);
    let mut reader = Reader::default();
    let plan = reader.read(&source, Utc::now());
    assert!(plan.units.is_empty());
    assert_eq!(plan.message, Some("no plan file"));
    task(&alternate, "working", false);
    source.config.checkout = Some(alternate);
    let plan = reader.read(&source, Utc::now());
    assert_eq!(plan.units.len(), 1);
    assert_eq!(plan.units[0].id, "working");
    assert!(plan.units[0].error.is_none());
}

#[test]
fn plan_details_inspect_twenty_units_once_per_request_and_refresh_next_time() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let mut options = git2::RepositoryInitOptions::new();
    options.initial_head("main").external_template(false);
    let repo = git2::Repository::init_opts(&root, &options).unwrap();
    let bytes = b"fixture source";
    write(&root, "source", std::str::from_utf8(bytes).unwrap());
    let mut index = repo.index().unwrap();
    let mut entry = git2::IndexEntry {
        ctime: git2::IndexTime::new(0, 0),
        mtime: git2::IndexTime::new(0, 0),
        dev: 0,
        ino: 0,
        mode: 0o100644,
        uid: 0,
        gid: 0,
        file_size: bytes.len() as u32,
        id: git2::Oid::ZERO_SHA1,
        flags: 0,
        flags_extended: 0,
        path: b"source".to_vec(),
    };
    index.add_frombuffer(&entry, bytes).unwrap();
    for n in 0..20 {
        let unit = format!("unit-{n:02}");
        task(&root, &unit, false);
        let text = b"- [ ] T01: Complete the fixture\n";
        entry.path = format!("openspec/changes/{unit}/tasks.md").into_bytes();
        entry.file_size = text.len() as u32;
        index.add_frombuffer(&entry, text).unwrap();
    }
    index.write().unwrap();
    let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
    let identity = ["fixture", "@", "invalid"].concat();
    let signature = git2::Signature::new("Fixture", &identity, &git2::Time::new(1, 0)).unwrap();
    repo.commit(
        Some("HEAD"),
        &signature,
        &signature,
        "Initial fixture",
        &tree,
        &[],
    )
    .unwrap();
    let source = input(&root);
    let mut reader = Reader::default();
    let plan = reader.read(&source, Utc::now());
    assert_eq!(plan.units.len(), 20);
    // Lexically different names for the same checkout must share a scan.
    for unit in plan.units.iter().take(10) {
        reader
            .selected
            .insert((root.clone(), unit.id.clone()), root.join("."));
    }
    let initial = reader.details(&source, &plan);
    assert_eq!(initial.len(), 20);
    assert!(initial.values().all(|detail| detail.dirty != Some(true)));
    assert_eq!(reader.detail_scans, 1);
    std::fs::remove_file(root.join("source")).unwrap();
    let next = reader.details(&source, &plan);
    assert_eq!(reader.detail_scans, 1);
    assert!(next.values().all(|detail| detail.dirty == Some(true)));
}

#[path = "discovery_source_tests.rs"]
mod source_tests;
