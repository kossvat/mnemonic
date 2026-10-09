//! Real CLI checks using explicit synthetic roots and unusable profiles.
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{Value, json};

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    project: PathBuf,
    user: PathBuf,
    profile: PathBuf,
}

impl Fixture {
    fn new(profile_exists: bool) -> Self {
        let temp = tempfile::Builder::new()
            .prefix("plan-counts-")
            .tempdir()
            .unwrap();
        let root = temp.path().canonicalize().unwrap();
        let project = root.join("project");
        let user = root.join("user");
        let profile = root.join("profile");
        fs::create_dir_all(&project).unwrap();
        fs::create_dir_all(user.join(".config").join("mnemonic")).unwrap();
        fs::create_dir(root.join("scratch")).unwrap();
        fs::write(
            user.join(".config").join("mnemonic").join("config.toml"),
            "invalid [ configuration",
        )
        .unwrap();
        if profile_exists {
            fs::create_dir(&profile).unwrap();
            fs::write(profile.join("config.toml"), "invalid [ configuration").unwrap();
            fs::write(profile.join("memory.db"), "invalid memory store").unwrap();
            fs::write(profile.join("activity.db"), "invalid activity store").unwrap();
        }
        let title = format!("Hidden task narrative {}", project.display());
        let tasks = json!({ "tasks": [
            {"id": "A-1", "unit": "unit-a", "title": title, "status": "done"},
            {"id": "A-2", "unit": "unit-a", "title": title, "status": "in_progress"},
            {"id": "A-3", "unit": "unit-a", "title": title, "status": "blocked",
             "waiting_on_owner": "Hidden owner narrative"}
        ]});
        fs::write(
            project.join("tasks.json"),
            serde_json::to_vec(&tasks).unwrap(),
        )
        .unwrap();
        Self {
            _temp: temp,
            root,
            project,
            user,
            profile,
        }
    }

    fn run(&self, root: &Path) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_mnemonic"));
        command
            .env_clear()
            .current_dir(&self.root)
            .env("HOME", &self.user)
            .env("MNEMONIC_HOME", self.root.join("environment-profile"))
            .env("XDG_CONFIG_HOME", self.user.join(".config"))
            .env("XDG_CACHE_HOME", self.user.join("cache"))
            .env("XDG_DATA_HOME", self.user.join("data"))
            .env("TMPDIR", self.root.join("scratch"))
            .env("FASTEMBED_CACHE_DIR", self.user.join("models"));
        // Keep only the loader settings cargo may need for its own binary.
        for key in [
            "DYLD_LIBRARY_PATH",
            "DYLD_FALLBACK_LIBRARY_PATH",
            "LD_LIBRARY_PATH",
        ] {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
        command
            .arg("--home")
            .arg(&self.profile)
            .arg("plan-counts")
            .arg("--root")
            .arg(root)
            .output()
            .unwrap()
    }
}

/// Include empty directories, so profile or cache creation is detected too.
fn snapshot(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    let mut result = BTreeMap::new();
    let mut directories = vec![root.to_path_buf()];
    while let Some(directory) = directories.pop() {
        for entry in fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let relative = path.strip_prefix(root).unwrap().to_path_buf();
            if entry.file_type().unwrap().is_dir() {
                result.insert(relative, None);
                directories.push(path);
            } else {
                result.insert(relative, Some(fs::read(path).unwrap()));
            }
        }
    }
    result
}

fn assert_counts_only(output: &Output, fixture: &Fixture) {
    assert!(output.status.success(), "aggregate command failed");
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        value,
        json!({ "message": null, "units": [{
            "id": "unit-a", "done": 1, "total": 3, "provider": "registry",
            "counts": {"done": 1, "in_progress": 1, "blocked": 1},
            "weekly_count": 0, "waiting_on_owner": 1, "stage": "blocked", "error": null
        }]})
    );
    let text = String::from_utf8_lossy(&output.stdout);
    for hidden in [
        "Hidden task narrative",
        "Hidden owner narrative",
        fixture.root.to_str().unwrap(),
    ] {
        assert!(
            !text.contains(hidden),
            "aggregate output contains excluded text"
        );
    }
    assert!(
        output.stderr.is_empty(),
        "aggregate command wrote unexpected diagnostics"
    );
}

#[test]
fn plan_counts_uses_explicit_fixture_without_creating_a_missing_profile() {
    let fixture = Fixture::new(false);
    let before = snapshot(&fixture.root);
    let output = fixture.run(&fixture.project);
    assert_counts_only(&output, &fixture);
    assert!(
        before == snapshot(&fixture.root),
        "aggregate command changed fixture files"
    );
    assert!(!fixture.profile.exists());
}

#[test]
fn plan_counts_bypasses_malformed_config_and_invalid_stores_without_writes() {
    let fixture = Fixture::new(true);
    let before = snapshot(&fixture.root);
    let output = fixture.run(&fixture.project);
    assert_counts_only(&output, &fixture);
    assert!(
        before == snapshot(&fixture.root),
        "aggregate command changed fixture files"
    );
}

#[test]
fn plan_counts_refuses_relative_and_missing_roots_without_profile_access() {
    let fixture = Fixture::new(true);
    let before = snapshot(&fixture.root);
    for root in [PathBuf::from("project"), fixture.root.join("missing")] {
        let output = fixture.run(&root);
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(error.contains("plan root must be an existing absolute directory"));
        assert!(!error.contains(fixture.root.to_str().unwrap()));
    }
    assert!(
        before == snapshot(&fixture.root),
        "refused command changed fixture files"
    );
}
