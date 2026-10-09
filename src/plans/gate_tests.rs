use super::*;
use serde_json::json;

struct Fixture {
    _temp: tempfile::TempDir,
    repo: Repository,
    root: PathBuf,
    project: PathBuf,
}

impl Fixture {
    fn new(nested: bool) -> Self {
        let temp = crate::test_support::temp_dir("plan-gate-");
        let root = temp.path().canonicalize().unwrap();
        let mut options = git2::RepositoryInitOptions::new();
        options.initial_head("main").external_template(false);
        let repo = Repository::init_opts(&root, &options).unwrap();
        let project = if nested {
            root.join("project")
        } else {
            root.clone()
        };
        fs::create_dir_all(&project).unwrap();
        fs::write(project.join("source"), "one").unwrap();
        let fixture = Self {
            _temp: temp,
            repo,
            root,
            project,
        };
        fixture.stage(&fixture.project.join("source"));
        fixture.commit();
        fixture
    }

    fn stage(&self, path: &Path) -> Oid {
        let mut index = self.repo.index().unwrap();
        index
            .add_path(path.strip_prefix(&self.root).unwrap())
            .unwrap();
        index.write().unwrap();
        index.write_tree().unwrap()
    }

    fn commit(&self) -> Oid {
        let email = ["fixture", "@", "invalid"].concat();
        let signature = git2::Signature::new("Fixture", &email, &git2::Time::new(1, 0)).unwrap();
        let mut index = self.repo.index().unwrap();
        let tree = self.repo.find_tree(index.write_tree().unwrap()).unwrap();
        let parent = self
            .repo
            .head()
            .ok()
            .and_then(|head| head.peel_to_commit().ok());
        let parents: Vec<_> = parent.iter().collect();
        self.repo
            .commit(
                Some("HEAD"),
                &signature,
                &signature,
                "Update fixture",
                &tree,
                &parents,
            )
            .unwrap()
    }

    fn receipt(&self, unit: &str, run: &str, value: Value) -> PathBuf {
        let path = self
            .project
            .join("target")
            .join("checks")
            .join(unit)
            .join(run);
        fs::create_dir_all(&path).unwrap();
        let path = path.join("verification.json");
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        path
    }

    fn passing(&self, run: &str) -> PathBuf {
        self.receipt(
            "unit-a",
            run,
            json!({
                "change": "unit-a", "passed": true,
                "candidate_subtree": head_tree(&self.project).unwrap()
            }),
        )
    }
}

#[test]
fn gate_candidate_remains_passed_after_commit_and_unrelated_commit() {
    let fixture = Fixture::new(true);
    let mut reader = Reader::default();
    fs::write(fixture.project.join("source"), "two").unwrap();
    let candidate = fixture.stage(&fixture.project.join("source"));
    let tree = fixture.repo.find_tree(candidate).unwrap();
    let subtree = tree
        .get_path(Path::new("project"))
        .unwrap()
        .id()
        .to_string();
    fixture.receipt(
        "unit-a",
        "10",
        json!({
            "change": "unit-a", "passed": true,
            "head": head_id(&fixture.repo), "candidate_subtree": subtree
        }),
    );
    assert_eq!(
        reader.read(&fixture.project, "unit-a", None).status,
        GateStatus::Stale
    );
    fixture.commit();
    assert_eq!(
        reader.read(&fixture.project, "unit-a", None).status,
        GateStatus::Passed
    );
    fs::write(fixture.root.join("other"), "one").unwrap();
    fixture.stage(&fixture.root.join("other"));
    fixture.commit();
    assert_eq!(
        reader.read(&fixture.project, "unit-a", None).status,
        GateStatus::Passed
    );
    fs::write(fixture.project.join("source"), "three").unwrap();
    fixture.stage(&fixture.project.join("source"));
    fixture.commit();
    assert_eq!(
        reader.read(&fixture.project, "unit-a", None).status,
        GateStatus::Stale
    );
    assert_eq!(reader.parsed_count(), 1);
}

#[test]
fn gate_legacy_repository_tree_is_never_project_evidence() {
    let fixture = Fixture::new(false);
    fixture.receipt(
        "unit-a",
        "1",
        json!({
            "change": "unit-a", "passed": true,
            "head": head_id(&fixture.repo), "source_tree": head_tree(&fixture.project)
        }),
    );
    assert_eq!(
        Reader::default()
            .read(&fixture.project, "unit-a", None)
            .status,
        GateStatus::Stale
    );
}

#[test]
fn gate_missing_and_other_unit_receipts_are_none() {
    let fixture = Fixture::new(true);
    let mut reader = Reader::default();
    assert_eq!(
        reader.read(&fixture.project, "unit-a", None),
        Gate::default()
    );
    fixture.receipt(
        "unit-b",
        "10",
        json!({ "change": "unit-b", "passed": false }),
    );
    fixture.receipt(
        "unit-a",
        "11",
        json!({ "change": "unit-b", "passed": false }),
    );
    assert_eq!(
        reader.read(&fixture.project, "unit-a", None),
        Gate::default()
    );
}

#[test]
fn gate_three_latest_failures_are_counted_and_bounded() {
    let fixture = Fixture::new(true);
    fixture.passing("8");
    for run in ["9", "10", "11", "12"] {
        fixture.receipt(
            "unit-a",
            run,
            json!({ "change": "unit-a", "passed": false }),
        );
    }
    let mut reader = Reader::default();
    let gate = reader.read(&fixture.project, "unit-a", None);
    assert_eq!(gate.status, GateStatus::Failed);
    assert_eq!(gate.at, Some(12));
    assert_eq!(gate.failed_runs, 3);
    fixture.passing("13");
    let gate = reader.read(&fixture.project, "unit-a", None);
    assert_eq!(gate.status, GateStatus::Passed);
    assert_eq!(gate.failed_runs, 0);
}

#[test]
fn gate_newest_malformed_receipt_cannot_revive_older_pass() {
    let fixture = Fixture::new(true);
    fixture.passing("9");
    let newest = fixture.passing("10");
    fs::write(newest, "{").unwrap();
    let gate = Reader::default().read(&fixture.project, "unit-a", None);
    assert_eq!(gate.status, GateStatus::Stale);
    assert_eq!(gate.at, Some(10));
    assert_eq!(gate.failed_runs, 0);
}

#[test]
fn gate_record_times_order_nonnumeric_run_folders() {
    let fixture = Fixture::new(true);
    fixture.receipt(
        "unit-a",
        "z",
        json!({ "change": "unit-a", "passed": true,
        "candidate_subtree": head_tree(&fixture.project), "timestamp": 30 }),
    );
    fixture.receipt(
        "unit-a",
        "a",
        json!({ "change": "unit-a", "passed": false, "timestamp": 40 }),
    );
    let gate = Reader::default().read(&fixture.project, "unit-a", None);
    assert_eq!(gate.status, GateStatus::Failed);
    assert_eq!(gate.at, Some(40));
}

#[test]
fn gate_unchanged_receipts_are_not_reparsed_and_edits_are_seen() {
    let fixture = Fixture::new(true);
    let path = fixture.passing("10");
    let mut reader = Reader::default();
    assert_eq!(
        reader.read(&fixture.project, "unit-a", None).status,
        GateStatus::Passed
    );
    for _ in 0..3 {
        reader.read(&fixture.project, "unit-a", None);
    }
    assert_eq!(reader.parsed_count(), 1);
    fs::write(
        &path,
        serde_json::to_vec(&json!({ "change": "unit-a", "passed": false })).unwrap(),
    )
    .unwrap();
    assert_eq!(
        reader.read(&fixture.project, "unit-a", None).status,
        GateStatus::Failed
    );
    assert_eq!(reader.parsed_count(), 2);
    fs::remove_file(path).unwrap();
    assert_eq!(
        reader.read(&fixture.project, "unit-a", None).status,
        GateStatus::None
    );
}

#[test]
fn gate_subtree_and_head_are_resolved_in_a_linked_worktree() {
    let fixture = Fixture::new(true);
    let temp = crate::test_support::temp_dir("plan-linked-");
    let root = temp.path().join("checkout");
    let worktree = fixture.repo.worktree("linked", &root, None).unwrap();
    let repo = Repository::open_from_worktree(&worktree).unwrap();
    let project = root.join("project");
    assert_eq!(head_tree(&project), head_tree(&fixture.project));
    let before = head_tree(&fixture.project).unwrap();
    fs::write(project.join("source"), "changed").unwrap();
    let mut index = repo.index().unwrap();
    index.add_path(Path::new("project/source")).unwrap();
    index.write().unwrap();
    let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
    let parent = repo.head().unwrap().peel_to_commit().unwrap();
    repo.commit(
        Some("HEAD"),
        &parent.author(),
        &parent.committer(),
        "Update linked",
        &tree,
        &[&parent],
    )
    .unwrap();
    assert_ne!(head_tree(&project).unwrap(), before);
    assert_eq!(head_tree(&fixture.project).unwrap(), before);
}

#[test]
fn gate_configured_commit_receipt_is_bound_to_its_unit() {
    let fixture = Fixture::new(false);
    let path = fixture.project.join("receipt.json");
    fs::write(
        &path,
        serde_json::to_vec(&json!({ "change": "unit-a", "passed": true,
        "commit": head_id(&fixture.repo) }))
        .unwrap(),
    )
    .unwrap();
    let mut reader = Reader::default();
    assert_eq!(
        reader.read(&fixture.project, "unit-a", Some(&path)).status,
        GateStatus::Passed
    );
    assert_eq!(
        reader.read(&fixture.project, "unit-b", Some(&path)).status,
        GateStatus::None
    );
    fs::write(fixture.project.join("source"), "new").unwrap();
    fixture.stage(&fixture.project.join("source"));
    fixture.commit();
    assert_eq!(
        reader.read(&fixture.project, "unit-a", Some(&path)).status,
        GateStatus::Stale
    );
}

#[test]
fn gate_invalid_subtree_cannot_fall_back_to_commit() {
    let fixture = Fixture::new(false);
    let path = fixture.project.join("receipt.json");
    fs::write(
        &path,
        serde_json::to_vec(&json!({ "change": "unit-a", "passed": true,
        "commit": head_id(&fixture.repo), "candidate_subtree": false }))
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        Reader::default()
            .read(&fixture.project, "unit-a", Some(&path))
            .status,
        GateStatus::Stale
    );
}

#[test]
fn gate_project_missing_from_head_is_stale() {
    let fixture = Fixture::new(true);
    let project = fixture.root.join("missing");
    fs::create_dir(&project).unwrap();
    let path = project.join("receipt.json");
    fs::write(
        &path,
        serde_json::to_vec(&json!({ "change": "unit-a", "passed": true,
        "candidate_subtree": head_tree(&fixture.project) }))
        .unwrap(),
    )
    .unwrap();
    assert_eq!(head_tree(&project), None);
    assert_eq!(
        Reader::default()
            .read(&project, "unit-a", Some(&path))
            .status,
        GateStatus::Stale
    );
}

#[test]
fn gate_reader_does_not_follow_outside_receipts_or_unit_traversal() {
    let fixture = Fixture::new(true);
    let outside = fixture.root.join("receipt.json");
    fs::write(
        &outside,
        serde_json::to_vec(&json!({ "change": "unit-a", "passed": false })).unwrap(),
    )
    .unwrap();
    let mut reader = Reader::default();
    assert_eq!(
        reader.read(&fixture.project, "unit-a", Some(&outside)),
        Gate::default()
    );
    assert_eq!(
        reader.read(&fixture.project, "../unit-a", None),
        Gate::default()
    );
    assert_eq!(reader.parsed_count(), 0);
    #[cfg(unix)]
    {
        let link = fixture.project.join("receipt.json");
        std::os::unix::fs::symlink(outside, &link).unwrap();
        assert_eq!(
            reader.read(&fixture.project, "unit-a", Some(&link)),
            Gate::default()
        );
        assert_eq!(reader.parsed_count(), 0);
    }
}

#[test]
fn gate_reader_caps_receipt_bytes() {
    let fixture = Fixture::new(true);
    let path = fixture.passing("10");
    fs::OpenOptions::new()
        .write(true)
        .open(path)
        .unwrap()
        .set_len(MAX_RECEIPT_BYTES + 1)
        .unwrap();
    assert_eq!(
        Reader::default()
            .read(&fixture.project, "unit-a", None)
            .status,
        GateStatus::Stale
    );
}

#[cfg(unix)]
#[test]
fn gate_reader_rejects_links_to_excluded_files_inside_project() {
    let fixture = Fixture::new(true);
    let secret = fixture.project.join(".env");
    fs::write(&secret, "synthetic").unwrap();
    let link = fixture.project.join("receipt.json");
    std::os::unix::fs::symlink(secret, &link).unwrap();
    let mut reader = Reader::default();
    assert_eq!(
        reader.read(&fixture.project, "unit-a", Some(&link)),
        Gate::default()
    );
    assert_eq!(reader.parsed_count(), 0);
}

#[test]
fn gate_timestamp_units_and_serialized_statuses_are_stable() {
    let expected = Some(1_700_000_000_000_000_000);
    for value in [
        1_700_000_000_u64,
        1_700_000_000_000,
        1_700_000_000_000_000,
        1_700_000_000_000_000_000,
    ] {
        assert_eq!(receipt_time(&json!(value)), expected);
    }
    assert_eq!(
        serde_json::to_value(Gate::default()).unwrap(),
        json!({ "status": "none", "at": null, "failed_runs": 0 })
    );
}
