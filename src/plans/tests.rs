use super::*;
use chrono::Duration;
use serde_json::json;
use std::path::Path;
use std::time::{Duration as Elapsed, Instant};

fn write(root: &Path, relative: &str, text: &str) -> PathBuf {
    let path = root.join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, text).unwrap();
    path
}

fn tasks(root: &Path, unit: &str, done: usize, total: usize) -> PathBuf {
    let text = (1..=total)
        .map(|n| {
            format!(
                "- [{}] T{n:02}: Complete step {n}\n",
                if n <= done { "x" } else { " " }
            )
        })
        .collect::<String>();
    write(root, &format!("openspec/changes/{unit}/tasks.md"), &text)
}

fn input(root: &Path) -> Input {
    Input {
        main: root.into(),
        config: Config::default(),
        checkouts: Vec::new(),
    }
}

#[test]
fn plan_reader_declared_sections_override_projections_and_registry() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    tasks(root, "change", 2, 2);
    write(
        root,
        "tasks.json",
        r#"[{"id":"T09","title":"Other","status":"done","unit":"registry"}]"#,
    );
    write(
        root,
        "openspec/config.yaml",
        "context: |\n  specs/TODO.md is the only task-status authority.\n",
    );
    write(
        root,
        "specs/TODO.md",
        "## OpenSpec: change\n- [x] T01: First\n- [ ] T02: Second\n## OpenSpec: another\n- [ ] A01: Later\n",
    );
    let plan = Reader::default().read(&input(root), Utc::now());
    assert_eq!(plan.units.len(), 2);
    let change = plan.units.iter().find(|unit| unit.id == "change").unwrap();
    assert_eq!((change.done, change.total), (1, 2));
    assert_eq!(change.provider, Provider::Checkbox);
    assert!(plan.units.iter().all(|unit| unit.id != "registry"));
}

#[test]
fn plan_reader_selects_provider_from_files_and_supports_explicit_funnel() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    tasks(root, "change", 0, 2);
    write(
        root,
        "docs/engineering/tasks.json",
        r#"[{"id":"T01","title":"Build","status":"in_progress","unit":"release"}]"#,
    );
    let mut reader = Reader::default();
    let registry = reader.read(&input(root), Utc::now());
    assert_eq!(registry.units[0].provider, Provider::Registry);
    assert_eq!(registry.units[0].id, "release");
    assert_eq!(
        registry.units[0].current_task.as_ref().unwrap().source,
        "registry"
    );
    let funnel_root = root.join("funnel");
    write(
        &funnel_root,
        "PIPELINE.md",
        "| Name | Status |\n| --- | --- |\n| First | ready |\n| Second | waiting_on_owner |\n",
    );
    let auto = reader.read(&input(&funnel_root), Utc::now());
    assert_eq!(
        auto.units.len(),
        1,
        "a status table selects the funnel provider"
    );
    assert_eq!(auto.units[0].provider, Provider::Funnel);
    assert_eq!(auto.units[0].counts.get("ready"), Some(&1));
    let mut explicit = input(&funnel_root);
    explicit.config.provider = Some(Provider::Funnel);
    assert_eq!(
        reader.read(&explicit, Utc::now()).units[0].provider,
        Provider::Funnel
    );
}

#[test]
fn plan_reader_compares_two_external_live_checkouts_with_main() {
    let temp = tempfile::tempdir().unwrap();
    let main = temp.path().join("workspace/project");
    let one = temp.path().join("external-one/project");
    let two = temp.path().join("external-two/project");
    tasks(&main, "change", 0, 3);
    tasks(&one, "change", 1, 3);
    tasks(&two, "change", 2, 3);
    let now = Utc::now();
    let mut source = input(&main);
    source.checkouts = vec![
        Checkout {
            root: one,
            branch: Some("task/T02".into()),
            last_at: now,
        },
        Checkout {
            root: two,
            branch: Some("task/T03".into()),
            last_at: now,
        },
    ];
    let plan = Reader::default().read(&source, now);
    assert_eq!((plan.units[0].done, plan.units[0].total), (2, 3));
    assert_eq!(plan.units[0].checkouts, 2);
    let served = serde_json::to_string(&plan).unwrap();
    assert!(!served.contains(temp.path().to_str().unwrap()));
    assert!(!served.contains("external-one") && !served.contains("external-two"));
}

#[test]
fn plan_reader_branch_unit_matches_boundaries_and_prefers_the_longest_name() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        "openspec/changes/core/tasks.md",
        "- [ ] C01: Core work\n",
    );
    write(
        root,
        "openspec/changes/core-ui/tasks.md",
        "- [ ] U01: Interface work\n",
    );
    let now = Utc::now();
    let mut source = input(root);
    let mut reader = Reader::default();
    for (branch, core, interface) in [
        ("work/core-ui", Stage::Planned, Stage::Building),
        ("work/core", Stage::Building, Stage::Planned),
        ("work/hardcore-ui", Stage::Planned, Stage::Planned),
    ] {
        source.checkouts = vec![Checkout {
            root: root.into(),
            branch: Some(branch.into()),
            last_at: now,
        }];
        let plan = reader.read(&source, now);
        assert_eq!(
            plan.units
                .iter()
                .find(|unit| unit.id == "core")
                .unwrap()
                .stage,
            core,
            "{branch}"
        );
        assert_eq!(
            plan.units
                .iter()
                .find(|unit| unit.id == "core-ui")
                .unwrap()
                .stage,
            interface,
            "{branch}"
        );
    }
}

#[test]
fn plan_reader_registry_always_uses_main_and_idle_checkbox_uses_configured_checkout() {
    let temp = tempfile::tempdir().unwrap();
    let main = temp.path().join("main");
    let other = temp.path().join("checkout");
    let registry =
        |status| json!([{"id":"T01","title":"First","status":status,"unit":"release"}]).to_string();
    write(&main, "tasks.json", &registry("ready"));
    write(&other, "tasks.json", &registry("done"));
    let mut source = input(&main);
    source.config.checkout = Some(other.clone());
    source.checkouts.push(Checkout {
        root: other.clone(),
        branch: None,
        last_at: Utc::now(),
    });
    let mut reader = Reader::default();
    assert_eq!(reader.read(&source, Utc::now()).units[0].done, 0);
    source.checkouts.clear();
    assert_eq!(reader.read(&source, Utc::now()).units[0].done, 0);
    std::fs::remove_file(main.join("tasks.json")).unwrap();
    tasks(&main, "change", 0, 2);
    tasks(&other, "change", 1, 2);
    let plan = reader.read(&source, Utc::now());
    assert_eq!(plan.units[0].provider, Provider::Checkbox);
    assert_eq!((plan.units[0].done, plan.units[0].total), (1, 2));
    assert_eq!(plan.units[0].checkouts, 0);
}

#[test]
fn plan_reader_caches_each_file_and_detects_edits_deletions_and_receipts() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    tasks(root, "first", 0, 1);
    let second = tasks(root, "second", 0, 1);
    let mut source = input(root);
    source.config.gate_required = Some(true);
    let mut reader = Reader::default();
    assert_eq!(reader.read(&source, Utc::now()).units.len(), 2);
    assert_eq!(reader.parsed_count(), 2);
    reader.read(&source, Utc::now());
    assert_eq!(reader.parsed_count(), 2);
    tasks(root, "first", 2, 2);
    let plan = reader.read(&source, Utc::now());
    assert_eq!(reader.parsed_count(), 3, "only the edited file was parsed");
    assert_eq!(
        plan.units
            .iter()
            .find(|unit| unit.id == "first")
            .unwrap()
            .stage,
        Stage::Review
    );
    std::fs::remove_file(second).unwrap();
    let plan = reader.read(&source, Utc::now());
    assert_eq!(plan.units.len(), 1);
    assert_eq!(reader.parsed_count(), 3);
    for run in [1, 2, 3] {
        write(
            root,
            &format!("target/checks/first/{run}/verification.json"),
            r#"{"change":"first","passed":false}"#,
        );
    }
    let plan = reader.read(&source, Utc::now());
    assert_eq!(plan.units[0].gate.failed_runs, 3);
    assert_eq!(plan.units[0].stage, Stage::Blocked);
    assert_eq!(
        reader.parsed_count(),
        3,
        "a receipt does not reparse the task file"
    );
}

#[test]
fn plan_reader_broken_file_does_not_hide_other_units_and_no_file_is_explicit() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    assert_eq!(
        Reader::default().read(&input(root), Utc::now()).message,
        Some("no plan file")
    );
    tasks(root, "valid", 0, 1);
    let broken = tasks(root, "broken", 0, 1);
    std::fs::write(broken, [255, 254]).unwrap();
    let plan = Reader::default().read(&input(root), Utc::now());
    assert_eq!(plan.units.len(), 2);
    assert!(
        plan.units
            .iter()
            .find(|unit| unit.id == "valid")
            .unwrap()
            .error
            .is_none()
    );
    assert!(
        plan.units
            .iter()
            .find(|unit| unit.id == "broken")
            .unwrap()
            .error
            .is_some()
    );
}

#[test]
fn plan_stages_keep_gate_precedence_and_staleness_separate() {
    let now = Utc::now();
    let parsed = |statuses: &[&str]| ParsedUnit {
        id: "change".into(),
        title: "Change".into(),
        tasks: statuses
            .iter()
            .enumerate()
            .map(|(i, status)| Task {
                id: format!("T{i:02}"),
                title: format!("Step {i}"),
                status: (*status).into(),
                owner: None,
            })
            .collect(),
        ..Default::default()
    };
    let build = |unit, gate, live: &[Checkout], required, at| {
        finish(unit, Provider::Checkbox, gate, at, live, required, now)
    };
    let stamp = now.timestamp();
    assert_eq!(
        build(parsed(&[]), gate::Gate::default(), &[], false, stamp).stage,
        Stage::Proposed
    );
    assert_eq!(
        build(
            parsed(&["planned"]),
            gate::Gate::default(),
            &[],
            false,
            stamp
        )
        .stage,
        Stage::Planned
    );
    assert_eq!(
        build(
            parsed(&["done", "planned"]),
            gate::Gate::default(),
            &[],
            true,
            stamp
        )
        .stage,
        Stage::Building
    );
    assert_eq!(
        build(parsed(&["done"]), gate::Gate::default(), &[], true, stamp).stage,
        Stage::Review
    );
    assert_eq!(
        build(parsed(&["done"]), gate::Gate::default(), &[], false, stamp).stage,
        Stage::Done
    );
    let passed = gate::Gate {
        status: gate::GateStatus::Passed,
        ..Default::default()
    };
    assert_eq!(
        build(parsed(&["done"]), passed, &[], true, stamp).stage,
        Stage::Done
    );
    let failed = gate::Gate {
        status: gate::GateStatus::Failed,
        failed_runs: 3,
        ..Default::default()
    };
    assert_eq!(
        build(parsed(&["done"]), failed, &[], true, stamp).stage,
        Stage::Blocked
    );
    assert_eq!(
        build(
            parsed(&["blocked"]),
            gate::Gate::default(),
            &[],
            false,
            stamp
        )
        .stage,
        Stage::Blocked
    );
    let old = (now - Duration::days(14)).timestamp();
    let stale = build(parsed(&["planned"]), gate::Gate::default(), &[], false, old);
    assert_eq!(stale.stage, Stage::Planned);
    assert!(stale.stale);
    let live = [Checkout {
        root: PathBuf::new(),
        branch: Some("task/T00".into()),
        last_at: now,
    }];
    let current = build(
        parsed(&["planned"]),
        gate::Gate::default(),
        &live,
        false,
        old,
    );
    assert_eq!(current.stage, Stage::Building);
    assert!(!current.stale);
    assert_eq!(current.current_task.unwrap().source, "branch");
}

#[test]
#[ignore = "local filesystem budget measurement"]
fn plan_sweep_benchmark() {
    let temp = tempfile::tempdir().unwrap();
    let mut inputs = Vec::new();
    let mut paths = Vec::new();
    for project in 0..66 {
        let root = temp.path().join(format!("project-{project:02}"));
        let text = (1..=24).map(|task| format!(
            "- [{}] T{task:02}: Verify the fixture step and preserve the previous result for comparison\n",
            if task < 8 { "x" } else { " " })).collect::<String>();
        paths.push(write(&root, "openspec/changes/change/tasks.md", &text));
        inputs.push(input(&root));
    }
    let mut stats = Vec::new();
    let mut parses = Vec::new();
    for _ in 0..7 {
        let start = Instant::now();
        for path in &paths {
            std::hint::black_box(std::fs::metadata(path).unwrap());
        }
        stats.push(start.elapsed());
        let mut reader = Reader::default();
        let start = Instant::now();
        for source in &inputs {
            std::hint::black_box(reader.read(source, Utc::now()));
        }
        parses.push(start.elapsed());
        assert_eq!(reader.parsed_count(), 66);
    }
    stats.sort();
    parses.sort();
    let (stats, parses) = (stats[3], parses[3]);
    eprintln!("66 stats: {stats:?}; full parse of 66 projects and 1584 tasks: {parses:?}");
    let texts: Vec<_> = paths
        .iter()
        .map(|path| std::fs::read_to_string(path).unwrap())
        .collect();
    let start = Instant::now();
    for text in &texts {
        std::hint::black_box(providers::checkboxes(text, "change", false));
    }
    eprintln!("Parser-only diagnostic: {:?}", start.elapsed());
    let start = Instant::now();
    for source in &inputs {
        for relative in [
            "openspec/config.yaml",
            "docs/engineering/tasks.json",
            "tasks.json",
            "PIPELINE.md",
            "openspec/changes",
            "openspec/changes/change/tasks.md",
            "private/verify_change.py",
            "bin/verify.sh",
            "target/checks",
        ] {
            std::hint::black_box(cache::within(&source.main, Path::new(relative)));
        }
    }
    eprintln!("Path-safety-only diagnostic: {:?}", start.elapsed());
    assert!(
        stats <= Elapsed::from_millis(1),
        "66 stats exceeded the budget: {stats:?}"
    );
    assert!(
        parses <= Elapsed::from_millis(50),
        "full sweep exceeded the budget: {parses:?}"
    );
}

#[test]
fn plan_branch_task_prefers_the_longest_matching_identifier() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        "openspec/changes/change/tasks.md",
        "- [ ] T01: First\n- [ ] T01-part: Nested step\n",
    );
    let now = Utc::now();
    let mut source = input(root);
    source.checkouts.push(Checkout {
        root: root.into(),
        branch: Some("task/T01-part".into()),
        last_at: now,
    });
    let plan = Reader::default().read(&source, now);
    assert_eq!(plan.units[0].current_task.as_ref().unwrap().id, "T01-part");
}
