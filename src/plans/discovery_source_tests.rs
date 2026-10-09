use super::*;

fn checkout(root: &Path, now: DateTime<Utc>) -> Checkout {
    Checkout {
        root: root.into(),
        branch: None,
        last_at: now,
    }
}

fn assert_blocked(plan: &Plan, error: &str) {
    assert_eq!(plan.units.len(), 1);
    assert_eq!(plan.units[0].stage, Stage::Blocked);
    assert_eq!(plan.units[0].error.as_deref(), Some(error));
    assert_eq!((plan.units[0].done, plan.units[0].total), (0, 0));
}

#[test]
fn plan_missing_main_reads_declared_authority_from_live_or_idle_checkout() {
    let temp = tempfile::tempdir().unwrap();
    let main = temp.path().join("removed-main");
    let other = temp.path().join("remaining-checkout");
    task(&other, "projection", true);
    write(
        &other,
        "openspec/config.yaml",
        "context: |\n  specs/TODO.md is the only task-status authority.\n",
    );
    write(
        &other,
        "specs/TODO.md",
        "## OpenSpec: authoritative\n- [ ] T01: First real task\n- [ ] T02: Second real task\n",
    );
    let now = Utc::now();
    let mut reader = Reader::default();
    for idle in [false, true] {
        let mut source = input(&main);
        if idle {
            source.config.checkout = Some(other.clone());
        } else {
            source.checkouts.push(checkout(&other, now));
        }
        let plan = reader.read(&source, now);
        assert_eq!(plan.units.len(), 1);
        assert_eq!(plan.units[0].id, "authoritative");
        assert_eq!((plan.units[0].done, plan.units[0].total), (0, 2));
        assert_eq!(plan.units[0].provider, Provider::Checkbox);
        assert!(plan.units[0].error.is_none());
        let parsed = reader.parsed_count();
        assert_eq!(reader.read(&source, now), plan);
        assert_eq!(reader.parsed_count(), parsed);
    }
    assert_eq!(
        reader.parsed_count(),
        1,
        "both paths reuse the authority cache"
    );
}

#[test]
fn plan_existing_main_keeps_metadata_authority_over_live_checkout() {
    let temp = tempfile::tempdir().unwrap();
    let main = temp.path().join("main");
    let other = temp.path().join("checkout");
    write(
        &main,
        "openspec/config.yaml",
        "task_status_authority: specs/TODO.md\n",
    );
    write(
        &main,
        "specs/TODO.md",
        "## OpenSpec: real\n- [ ] T01: Main pending\n",
    );
    // This declaration must not be read while main is present.
    write(
        &other,
        "openspec/config.yaml",
        "task_status_authority: ../outside.md\n",
    );
    write(
        &other,
        "specs/TODO.md",
        "## OpenSpec: real\n- [x] T01: Checkout progress\n- [ ] T02: Later\n",
    );
    task(&other, "projection", true);
    let now = Utc::now();
    let mut source = input(&main);
    source.checkouts.push(checkout(&other, now));
    let plan = Reader::default().read(&source, now);
    assert_eq!(plan.units.len(), 1);
    assert_eq!(plan.units[0].id, "real");
    assert_eq!((plan.units[0].done, plan.units[0].total), (1, 2));
    assert!(plan.units[0].error.is_none());
}

#[test]
fn plan_missing_main_autodetects_funnel_in_remaining_checkout() {
    let temp = tempfile::tempdir().unwrap();
    let main = temp.path().join("removed-main");
    let other = temp.path().join("checkout");
    task(&other, "projection", true);
    write(
        &other,
        "PIPELINE.md",
        "| Name | Status |\n| --- | --- |\n| First | ready |\n| Second | waiting_on_owner |\n",
    );
    let now = Utc::now();
    let mut source = input(&main);
    source.checkouts.push(checkout(&other, now));
    let plan = Reader::default().read(&source, now);
    assert_eq!(plan.units.len(), 1);
    assert_eq!(plan.units[0].provider, Provider::Funnel);
    assert_eq!(plan.units[0].counts.get("ready"), Some(&1));
    assert_eq!(plan.units[0].counts.get("waiting_on_owner"), Some(&1));
    assert!(plan.units[0].error.is_none());
}

#[test]
fn plan_missing_main_registry_metadata_never_reads_checkout_registry_tasks() {
    let temp = tempfile::tempdir().unwrap();
    let main = temp.path().join("removed-main");
    let other = temp.path().join("checkout");
    task(&other, "projection", true);
    let registry = r#"[{"id":"T01","title":"Wrong checkout","unit":"release","status":"done"}]"#;
    write(&other, "tasks.json", registry);
    write(&other, "specs/registry.json", registry);
    let now = Utc::now();
    let mut source = input(&main);
    source.checkouts.push(checkout(&other, now));
    let mut reader = Reader::default();
    for declared in [false, true] {
        if declared {
            write(
                &other,
                "openspec/config.yaml",
                "task_status_authority: specs/registry.json\n",
            );
        }
        let plan = reader.read(&source, now);
        assert!(plan.units.is_empty());
        assert_eq!(plan.message, Some("no plan file"));
        assert_eq!(reader.parsed_count(), 0);
    }
    source.checkouts.clear();
    source.config.checkout = Some(other);
    assert!(reader.read(&source, now).units.is_empty());
    assert_eq!(reader.parsed_count(), 0);
}

#[test]
fn plan_missing_main_rejects_conflicting_checkout_authorities_and_providers() {
    let temp = tempfile::tempdir().unwrap();
    let now = Utc::now();
    for case in 0..3 {
        let base = temp.path().join(format!("case-{case}"));
        let main = base.join("removed-main");
        let first = base.join("first");
        let second = base.join("second");
        task(&first, "projection", true);
        task(&second, "projection", true);
        if case == 1 {
            write(
                &first,
                "PIPELINE.md",
                "| Name | Status |\n| --- | --- |\n| First | ready |\n",
            );
        } else {
            write(
                &first,
                "openspec/config.yaml",
                "task_status_authority: specs/TODO.md\n",
            );
            if case == 0 {
                write(
                    &second,
                    "openspec/config.yaml",
                    "task_status_authority: other.md\n",
                );
            }
        }
        let mut source = input(&main);
        source.checkouts = vec![checkout(&first, now), checkout(&second, now)];
        let mut reader = Reader::default();
        let plan = reader.read(&source, now);
        assert_blocked(&plan, "conflicting plan sources");
        assert_eq!(reader.parsed_count(), 0);
        source.checkouts.reverse();
        assert_eq!(reader.read(&source, now), plan);
        let json = serde_json::to_string(&plan).unwrap();
        assert!(!json.contains(first.to_str().unwrap()));
        assert!(!json.contains("specs/TODO.md"));
    }
}

#[test]
fn plan_missing_main_matching_authorities_still_compare_checkout_progress() {
    let temp = tempfile::tempdir().unwrap();
    let main = temp.path().join("removed-main");
    let first = temp.path().join("first");
    let second = temp.path().join("second");
    for (root, mark) in [(&first, " "), (&second, "x")] {
        write(
            root,
            "openspec/config.yaml",
            "task_status_authority: specs/TODO.md\n",
        );
        write(
            root,
            "specs/TODO.md",
            &format!("## OpenSpec: real\n- [{mark}] T01: First\n- [ ] T02: Second\n"),
        );
        task(root, "projection", true);
    }
    let now = Utc::now();
    let mut source = input(&main);
    source.checkouts = vec![checkout(&first, now), checkout(&second, now)];
    let plan = Reader::default().read(&source, now);
    assert_eq!(plan.units.len(), 1);
    assert_eq!(plan.units[0].id, "real");
    assert_eq!((plan.units[0].done, plan.units[0].total), (1, 2));
    assert!(plan.units[0].error.is_none());
}

#[test]
fn plan_missing_main_accepts_explicit_and_default_funnel_authorities() {
    let temp = tempfile::tempdir().unwrap();
    let now = Utc::now();
    for declared_first in [true, false] {
        let base = temp.path().join(format!("case-{declared_first}"));
        let first = base.join("first");
        let second = base.join("second");
        for root in [&first, &second] {
            task(root, "projection", true);
            write(
                root,
                "PIPELINE.md",
                "| Name | Status |\n| --- | --- |\n| First | ready |\n| Second | waiting_on_owner |\n",
            );
        }
        let declared = if declared_first { &first } else { &second };
        write(
            declared,
            "openspec/config.yaml",
            "task_status_authority: PIPELINE.md\n",
        );
        let mut source = input(&base.join("removed-main"));
        source.config.provider = Some(Provider::Funnel);
        source.checkouts = vec![checkout(&first, now), checkout(&second, now)];
        let mut reader = Reader::default();
        let plan = reader.read(&source, now);
        assert_eq!(plan.units.len(), 1);
        assert_eq!(plan.units[0].provider, Provider::Funnel);
        assert_eq!(plan.units[0].counts.get("ready"), Some(&1));
        assert_eq!(plan.units[0].counts.get("waiting_on_owner"), Some(&1));
        assert!(plan.units[0].error.is_none());
        source.checkouts.reverse();
        assert_eq!(reader.read(&source, now), plan);
        write(
            declared,
            "openspec/config.yaml",
            "task_status_authority: other.md\n",
        );
        assert_blocked(&reader.read(&source, now), "conflicting plan sources");
    }
}

#[test]
fn plan_missing_main_accepts_default_registry_authority_without_checkout_tasks() {
    let temp = tempfile::tempdir().unwrap();
    let now = Utc::now();
    for declared_first in [true, false] {
        let base = temp.path().join(format!("case-{declared_first}"));
        let first = base.join("first");
        let second = base.join("second");
        for root in [&first, &second] {
            write(
                root,
                "tasks.json",
                r#"[{"id":"T01","title":"Checkout task","unit":"release","status":"done"}]"#,
            );
        }
        write(
            if declared_first { &first } else { &second },
            "openspec/config.yaml",
            "task_status_authority: tasks.json\n",
        );
        let mut source = input(&base.join("removed-main"));
        source.config.provider = Some(Provider::Registry);
        source.checkouts = vec![checkout(&first, now), checkout(&second, now)];
        let mut reader = Reader::default();
        for _ in 0..2 {
            let plan = reader.read(&source, now);
            assert!(plan.units.is_empty());
            assert_eq!(plan.message, Some("no plan file"));
            assert_eq!(reader.parsed_count(), 0);
            source.checkouts.reverse();
        }
    }
}

#[test]
fn plan_missing_main_invalid_unreadable_metadata_does_not_read_projections() {
    let temp = tempfile::tempdir().unwrap();
    let main = temp.path().join("removed-main");
    let other = temp.path().join("checkout");
    task(&other, "projection", true);
    let now = Utc::now();
    let mut source = input(&main);
    source.checkouts.push(checkout(&other, now));
    let mut reader = Reader::default();
    for declaration in [
        "task_status_authority: ../outside.md\n",
        "task_status_authority: .env/tasks.md\n",
    ] {
        write(&other, "openspec/config.yaml", declaration);
        assert_blocked(&reader.read(&source, now), "invalid-task-authority");
    }
    std::fs::write(other.join("openspec/config.yaml"), [255, 254]).unwrap();
    assert_blocked(&reader.read(&source, now), "plan configuration unreadable");
    assert_eq!(reader.parsed_count(), 0);
    write(&other, "chosen.md", "- [ ] T01: Explicit choice\n");
    source.config.authority = Some("chosen.md".into());
    let plan = reader.read(&source, now);
    assert_eq!((plan.units[0].done, plan.units[0].total), (0, 1));
    assert!(plan.units[0].error.is_none());
}

#[cfg(unix)]
#[test]
fn plan_missing_main_metadata_rejects_symlink_outside_checkout() {
    let temp = tempfile::tempdir().unwrap();
    let main = temp.path().join("removed-main");
    let other = temp.path().join("checkout");
    task(&other, "projection", true);
    write(
        temp.path(),
        "outside.yaml",
        "task_status_authority: specs/TODO.md\n",
    );
    std::os::unix::fs::symlink(
        temp.path().join("outside.yaml"),
        other.join("openspec/config.yaml"),
    )
    .unwrap();
    let now = Utc::now();
    let mut source = input(&main);
    source.checkouts.push(checkout(&other, now));
    let mut reader = Reader::default();
    assert_blocked(
        &reader.read(&source, now),
        "plan configuration location refused",
    );
    assert_eq!(reader.parsed_count(), 0);
}
