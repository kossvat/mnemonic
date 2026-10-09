use super::*;
use serde_json::json;

struct Fixture {
    _temp: tempfile::TempDir,
    repo: Repository,
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = crate::test_support::temp_dir("gate-detail-");
        let root = temp.path().canonicalize().unwrap();
        let mut options = git2::RepositoryInitOptions::new();
        options.initial_head("main").external_template(false);
        let repo = Repository::init_opts(&root, &options).unwrap();
        // An explicitly opened local config never loads the user's files.
        let mut config = git2::Config::open(&repo.path().join("config")).unwrap();
        let ignore = repo.path().join("fixture-ignore");
        fs::write(&ignore, "").unwrap();
        config
            .set_str("core.excludesfile", ignore.to_str().unwrap())
            .unwrap();
        repo.set_config(&config).unwrap();
        let fixture = Self {
            _temp: temp,
            repo,
            root,
        };
        fixture.write("source", "one");
        fixture.stage("source");
        fixture.commit();
        fixture
    }

    fn write(&self, path: &str, value: &str) {
        let path = self.root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, value).unwrap();
    }

    fn stage(&self, path: &str) {
        let mut index = self.repo.index().unwrap();
        let bytes = fs::read(self.root.join(path)).unwrap();
        // The fixture stages known bytes directly, without clean filters or
        // attributes from the machine that happens to run the test.
        let entry = git2::IndexEntry {
            ctime: git2::IndexTime::new(0, 0),
            mtime: git2::IndexTime::new(0, 0),
            dev: 0,
            ino: 0,
            mode: 0o100644,
            uid: 0,
            gid: 0,
            file_size: bytes.len() as u32,
            id: Oid::ZERO_SHA1,
            flags: 0,
            flags_extended: 0,
            path: path.as_bytes().to_vec(),
        };
        index.add_frombuffer(&entry, &bytes).unwrap();
        index.write().unwrap();
    }

    fn skip_worktree(&self, path: &str) {
        let mut index = self.repo.index().unwrap();
        let mut entry = index.get_path(Path::new(path), 0).unwrap();
        entry.flags |= git2::IndexEntryFlag::EXTENDED.bits();
        entry.flags_extended |= IndexEntryExtendedFlag::SKIP_WORKTREE.bits();
        index.add(&entry).unwrap();
        index.write().unwrap();
    }

    fn detail(&self, relative: &str) -> Detail {
        inspect(
            &self.repo,
            Path::new(relative),
            None,
            &GlobalFiles::default(),
            &mut DetailCache::default(),
        )
    }

    fn commit(&self) -> Oid {
        let identity = ["fixture", "@", "invalid"].concat();
        let signature = git2::Signature::new("Fixture", &identity, &git2::Time::new(1, 0)).unwrap();
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
}

#[cfg(unix)]
#[test]
fn gate_detail_reports_executable_mode_changes_unless_git_ignores_them() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new();
    fixture
        .repo
        .config()
        .unwrap()
        .set_bool("core.filemode", true)
        .unwrap();
    fs::set_permissions(
        fixture.root.join("source"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    assert_eq!(fixture.detail("").dirty, Some(true));
    fixture
        .repo
        .config()
        .unwrap()
        .set_bool("core.filemode", false)
        .unwrap();
    assert_eq!(fixture.detail("").dirty, Some(false));
}

#[test]
fn gate_detail_reports_clean_worktree_tracked_staged_and_untracked_changes() {
    let fixture = Fixture::new();
    assert_eq!(fixture.detail("").dirty, Some(false));
    fixture.write("source", "two");
    assert_eq!(fixture.detail("").dirty, Some(true));
    fixture.stage("source");
    assert_eq!(fixture.detail("").dirty, Some(true));
    fixture.commit();
    fixture.write("extra", "one");
    assert_eq!(fixture.detail("").dirty, Some(true));
}

#[test]
fn gate_detail_is_project_scoped_and_skips_ignored_untracked_files() {
    let fixture = Fixture::new();
    fixture.write("project/source", "one");
    fixture.write("project/.gitignore", "scratch\n");
    fixture.stage("project/source");
    fixture.stage("project/.gitignore");
    fixture.commit();
    fixture.write("source", "two");
    fixture.write("project/scratch", "scratch");
    fixture.write("project/target/cache", "cache");
    assert_eq!(fixture.detail("project").dirty, Some(false));
}

#[test]
fn gate_detail_tracked_excluded_files_never_produce_a_false_clean_result() {
    let fixture = Fixture::new();
    let mut index = fixture.repo.index().unwrap();
    let mut entry = index.get_path(Path::new("source"), 0).unwrap();
    entry.path = b".env".to_vec();
    entry.id = fixture.repo.blob(b"synthetic").unwrap();
    index.add(&entry).unwrap();
    index.write().unwrap();
    fixture.commit();
    fixture.write(".env", "synthetic");
    assert_eq!(fixture.detail("").dirty, None);
}

#[test]
fn gate_detail_filters_attributes_and_oversized_files_are_unknown() {
    let fixture = Fixture::new();
    fixture
        .repo
        .config()
        .unwrap()
        .set_str("filter.fixture.clean", "unavailable")
        .unwrap();
    assert_eq!(fixture.detail("").dirty, None);
    fixture
        .repo
        .config()
        .unwrap()
        .remove("filter.fixture.clean")
        .unwrap();
    fixture.write(".gitattributes", "source filter=fixture\n");
    fixture.stage(".gitattributes");
    fixture.commit();
    assert_eq!(fixture.detail("").dirty, None);
    let fixture = Fixture::new();
    fs::OpenOptions::new()
        .write(true)
        .open(fixture.root.join("source"))
        .unwrap()
        .set_len(MAX_FILE_BYTES + 1)
        .unwrap();
    assert_eq!(fixture.detail("").dirty, None);
}

#[test]
fn gate_detail_explicit_global_attributes_remain_unknown() {
    let fixture = Fixture::new();
    let attributes = fixture.repo.path().join("fixture-attributes");
    fs::write(&attributes, "source filter=fixture\n").unwrap();
    let globals = GlobalFiles {
        attributes: Some(attributes),
        ignore: None,
    };
    assert_eq!(
        inspect(
            &fixture.repo,
            Path::new(""),
            None,
            &globals,
            &mut DetailCache::default()
        )
        .dirty,
        None
    );
    assert_eq!(fixture.detail("").dirty, Some(false));
}

#[cfg(unix)]
#[test]
fn gate_detail_symlink_content_is_not_read() {
    let fixture = Fixture::new();
    fs::remove_file(fixture.root.join("source")).unwrap();
    std::os::unix::fs::symlink("missing", fixture.root.join("source")).unwrap();
    assert_eq!(fixture.detail("").dirty, None);
}

#[test]
fn gate_detail_counts_commits_since_the_selected_receipt() {
    let fixture = Fixture::new();
    let head = super::super::head_id(&fixture.repo).unwrap();
    let path = fixture.root.join("target").join("receipt.json");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        &path,
        serde_json::to_vec(&json!({
            "change": "unit-a", "passed": true, "head": head
        }))
        .unwrap(),
    )
    .unwrap();
    let mut reader = super::super::Reader::default();
    assert_eq!(
        reader
            .detail(&fixture.root, "unit-a", Some(&path))
            .commits_since,
        Some(0)
    );
    fixture.write("source", "two");
    fixture.stage("source");
    fixture.commit();
    let detail = reader.detail(&fixture.root, "unit-a", Some(&path));
    assert_eq!(detail.commits_since, Some(1));
    assert_eq!(fixture.detail("").dirty, Some(false));
    assert_eq!(
        reader
            .detail(&fixture.root, "unit-b", Some(&path))
            .commits_since,
        None
    );
}

#[test]
fn gate_detail_sparse_exclusions_are_clean_but_real_deletions_are_dirty() {
    let fixture = Fixture::new();
    fixture.write("excluded/source", "kept only in the index");
    fixture.stage("excluded/source");
    fixture.commit();
    fixture
        .repo
        .config()
        .unwrap()
        .set_bool("core.sparseCheckout", true)
        .unwrap();
    fixture.skip_worktree("excluded/source");
    fs::remove_file(fixture.root.join("excluded/source")).unwrap();
    assert_eq!(fixture.detail("").dirty, Some(false));
    fs::remove_file(fixture.root.join("source")).unwrap();
    assert_eq!(fixture.detail("").dirty, Some(true));
}

#[test]
fn gate_detail_materialized_skip_worktree_file_is_unknown() {
    let fixture = Fixture::new();
    fixture.skip_worktree("source");
    assert_eq!(fixture.detail("").dirty, None);
    fixture.write(
        "source",
        "materialized contents cannot establish sparse state",
    );
    assert_eq!(fixture.detail("").dirty, None);
}

#[test]
fn gate_detail_request_cache_keeps_project_scopes_and_receipt_commits_separate() {
    let fixture = Fixture::new();
    for project in ["first", "second"] {
        fixture.write(&format!("{project}/source"), "one");
        fixture.stage(&format!("{project}/source"));
    }
    let before = fixture.commit().to_string();
    fixture.write("source", "next commit");
    fixture.stage("source");
    let head = fixture.commit().to_string();
    let globals = GlobalFiles::default();
    let mut cache = DetailCache::default();
    let first = inspect(
        &fixture.repo,
        Path::new("first"),
        Some(&before),
        &globals,
        &mut cache,
    );
    assert_eq!(
        first,
        Detail {
            dirty: Some(false),
            commits_since: Some(1)
        }
    );
    let later = inspect(
        &fixture.repo,
        Path::new("first"),
        Some(&head),
        &globals,
        &mut cache,
    );
    assert_eq!(
        later,
        Detail {
            dirty: Some(false),
            commits_since: Some(0)
        }
    );
    assert_eq!(cache.scan_count(), 1);
    fixture.write("second/source", "uncommitted");
    let second = inspect(
        &fixture.repo,
        Path::new("second"),
        None,
        &globals,
        &mut cache,
    );
    assert_eq!(second.dirty, Some(true));
    assert_eq!(cache.scan_count(), 2);
}
