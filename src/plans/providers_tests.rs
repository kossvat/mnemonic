use super::*;
use chrono::TimeZone;
use serde_json::json;

fn find<'a>(units: &'a [ParsedUnit], id: &str) -> &'a ParsedUnit {
    units.iter().find(|unit| unit.id == id).unwrap()
}

fn token() -> String {
    format!("sk-{}", "AbCd1234".repeat(6))
}

#[test]
fn plan_registry_groups_explicit_units_and_release_memberships() {
    let text = json!({
        "releases": {"first": ["T01", "T02"], "second": ["T03"]},
        "tasks": [
            {"id":"T01", "title":"Prepare", "status":"done"},
            {"id":"T02", "title":"Build", "status":"in_progress", "owner":"Builder"},
            {"id":"T03", "title":"Review", "status":"blocked", "waiting_on_owner":"Approve the draft"},
            {"id":"T04", "title":"Next", "status":"backlog", "release":3},
            {"id":"T05", "title":"Other", "status":"ready", "unit":"fourth"}
        ]
    });
    let units = registry(&text.to_string()).unwrap();
    let first = find(&units, "first");
    assert_eq!(first.tasks.len(), 2);
    assert_eq!(first.counts.get("done"), Some(&1));
    assert_eq!(first.counts.get("in_progress"), Some(&1));
    assert!(first.waiting_on_owner.is_empty());
    assert_eq!(
        find(&units, "second").waiting_on_owner,
        ["Approve the draft"]
    );
    assert_eq!(find(&units, "3").tasks[0].id, "T04");
    assert_eq!(find(&units, "fourth").tasks[0].id, "T05");
}

/// A registry of the owner's holds tasks nobody has taken up as `todo`:
/// they are open tasks of their unit, not rows to refuse.
#[test]
fn plan_registry_counts_a_todo_task_as_an_open_task_of_its_unit() {
    let text = json!([
        {"id":"T01", "title":"Shipped", "status":"done", "unit":"sixth"},
        {"id":"T02", "title":"Not begun", "status":"todo", "unit":"sixth"}
    ]);
    let units = registry(&text.to_string()).unwrap();
    let sixth = find(&units, "sixth");
    assert!(sixth.error.is_none());
    assert_eq!(sixth.tasks.len(), 2);
    assert_eq!(sixth.counts.get("todo"), Some(&1));
    assert_eq!(sixth.counts.get("done"), Some(&1));
}

#[test]
fn plan_registry_invalid_status_breaks_only_its_unit() {
    let text = json!([
        {"id":"T01", "title":"Ready", "status":"done", "unit":"valid"},
        {"id":"T02", "title":"Unknown", "status":"almost", "unit":"invalid"}
    ]);
    let units = registry(&text.to_string()).unwrap();
    assert!(find(&units, "valid").error.is_none());
    assert_eq!(find(&units, "valid").counts.get("done"), Some(&1));
    assert_eq!(
        find(&units, "invalid").error.as_deref(),
        Some("invalid-task-status")
    );
    assert!(find(&units, "invalid").tasks.is_empty());
}

#[test]
fn plan_registry_rejects_duplicate_ids_and_missing_release_members() {
    let units = registry(
        &json!({
            "releases":{"release":["T01", "T02"]},
            "tasks":[{"id":"T01","title":"One","status":"ready"}]
        })
        .to_string(),
    )
    .unwrap();
    assert_eq!(units[0].error.as_deref(), Some("missing-release-task"));
    let units = registry(
        &json!([
            {"id":"T01","title":"One","status":"ready"},
            {"id":"T01","title":"Again","status":"done"}
        ])
        .to_string(),
    )
    .unwrap();
    assert_eq!(units[0].error.as_deref(), Some("duplicate-task-id"));
    assert_eq!(units[0].tasks.len(), 1);
    assert_eq!(registry("["), Err("invalid-registry-json".into()));
    assert_eq!(registry("{}"), Err("invalid-registry-shape".into()));
}

#[test]
fn plan_checkbox_matches_openspec_bullets_markers_and_link_exception() {
    let text = concat!(
        "- [x] T01: Complete\n",
        " * [X] T02: Uppercase stays open\n",
        "\t+[!] T03: A marker stays open\n",
        "1. [ x ] T04: Complete with padding\n",
        "999999999) [] T05: Empty stays open\n",
        "- [ ](reference)\n",
        "- [](reference)\n",
        "- [x](reference)\n",
        "- [xx] Not a task\n",
        "1000000000. [ ] Too many digits\n",
        "A [ ] Not a bullet\n",
        "```text\n- [x] T07: A counted example\n```\n"
    );
    let units = checkboxes(text, "change", false);
    assert_eq!(units[0].tasks.len(), 7);
    assert_eq!(units[0].counts.get("done"), Some(&3));
    assert_eq!(units[0].counts.get("planned"), Some(&4));
    assert!(units[0].error.is_none());
    assert_eq!(units[0].tasks[1].status, "planned");
}

#[test]
fn plan_checkbox_sections_keep_tasks_under_non_unit_headings() {
    let units = checkboxes(
        concat!(
            "# Task list\n- [ ] T01: General\n",
            "## OpenSpec: change-one\n- [x] T02: First\n",
            "### Verification\n- [ ] T03: Check\n",
            "## Maintenance\n- [ ] T04: Unrelated\n",
            "## OpenSpec: change-two\n- [ ] T05: Second\n"
        ),
        "tasks",
        true,
    );
    assert_eq!(
        find(&units, "tasks")
            .tasks
            .iter()
            .map(|t| t.id.as_str())
            .collect::<Vec<_>>(),
        ["T01", "T04"]
    );
    assert_eq!(find(&units, "change-one").tasks.len(), 2);
    assert_eq!(find(&units, "change-two").tasks.len(), 1);
}

#[test]
fn plan_checkbox_owner_annotations_apply_only_to_open_tasks() {
    let units = checkboxes(
        concat!(
            "- [ ] T01: First\n  Owner: none. Already settled.\n",
            "- [ ] T02: Approval\n  Verify: machine check\n  Owner: Approve the result\n",
            "- [x] T03: Finished\n  Owner: An old request\n",
            "- [ ] 1.1 Numbered item\n  Owner: Review the draft\n"
        ),
        "change",
        false,
    );
    assert_eq!(
        units[0].waiting_on_owner,
        ["Approve the result", "Review the draft"]
    );
    assert_eq!(units[0].tasks[3].id, "1.1");
    assert_eq!(units[0].tasks[3].title, "Numbered item");
}

#[test]
fn plan_provider_redacts_and_caps_every_narrative() {
    let secret = token();
    let text = json!([{
        "id":"T01", "unit":"change", "status":"blocked",
        "title":format!("Replace {secret} {}", "a".repeat(200)),
        "owner":format!("Review {secret} {}", "b".repeat(300)),
        "waiting_on_owner":format!("Approve {secret} {}", "c".repeat(300))
    }]);
    let units = registry(&text.to_string()).unwrap();
    let task = &units[0].tasks[0];
    assert!(!format!("{units:?}").contains(&secret));
    assert!(task.title.chars().count() <= 160);
    assert!(task.owner.as_ref().unwrap().chars().count() <= 240);
    assert!(units[0].waiting_on_owner[0].chars().count() <= 240);
    let unsafe_id = json!([{"id":secret,"title":"Hidden","status":"done","unit":"change"}]);
    let refused = registry(&unsafe_id.to_string()).unwrap();
    assert_eq!(refused[0].error.as_deref(), Some("invalid-task-id"));
    assert!(refused[0].tasks.is_empty());
}

#[test]
fn plan_provider_prepares_again_after_a_cut() {
    let body = "AbCd1234".repeat(3);
    let prefix = "a".repeat(160 - " password=".len() - body.len());
    let title = format!("{prefix} password={body}(arg) for later");
    assert!(!crate::redaction::redact_text(&title).changed);
    let units = checkboxes(&format!("- [ ] T01: {title}"), "change", false);
    assert!(!units[0].tasks[0].title.contains(&body));
    assert!(units[0].tasks[0].title.chars().count() <= 160);
    assert!(!crate::redaction::redact_text(&units[0].tasks[0].title).changed);
}

#[test]
fn plan_provider_never_exposes_refused_unit_identity() {
    let secret = token();
    let units = checkboxes("- [ ] T01: Work", &secret, false);
    assert_eq!(units[0].id, "invalid-unit");
    assert_eq!(units[0].error.as_deref(), Some("invalid-unit-id"));
    assert!(!format!("{units:?}").contains(&secret));
    let units = checkboxes(&format!("- [ ] {secret}: Work"), "change", false);
    assert_eq!(units[0].error.as_deref(), Some("invalid-task-id"));
    assert!(units[0].tasks.is_empty());
}

#[test]
fn plan_funnel_counts_statuses_and_calendar_week_without_tasks() {
    let now = Utc.with_ymd_and_hms(2026, 6, 17, 12, 0, 0).unwrap();
    let units = funnel(
        concat!(
            "| Name | Status | Date | Owner |\n",
            "| --- | --- | --- | --- |\n",
            "| Earlier | Ready | 2026-06-14 | none |\n",
            "| First | Ready | 2026-06-15 | Builder |\n",
            "| Draft | Waiting on owner | 2026-06-17 | Approve draft |\n",
            "| Later | Ready | 2026-06-18 | none |\n",
            "| Clock | Ready | 2026-06-17T13:00:00Z | none |\n"
        ),
        "pipeline",
        now,
    )
    .unwrap();
    assert_eq!(units[0].counts.get("ready"), Some(&4));
    assert_eq!(units[0].counts.get("waiting_on_owner"), Some(&1));
    assert_eq!(units[0].weekly_count, 2);
    assert_eq!(units[0].waiting_on_owner, ["Approve draft"]);
    assert!(units[0].tasks.is_empty());
}

#[test]
fn plan_funnel_isolates_invalid_rows_and_redacts_waiting_text() {
    let now = Utc.with_ymd_and_hms(2026, 6, 17, 12, 0, 0).unwrap();
    let secret = token();
    let text = format!(
        "| Name | Status | Waiting on owner |\n| --- | --- | --- |\n| Escaped \\| title | Ready | Review {secret} |\n| Broken |\n"
    );
    let units = funnel(&text, "pipeline", now).unwrap();
    assert_eq!(units[0].counts.get("ready"), Some(&1));
    assert_eq!(units[0].error.as_deref(), Some("invalid-funnel-row"));
    assert!(!format!("{units:?}").contains(&secret));
    assert_eq!(
        funnel("No table", "pipeline", now),
        Err("missing-status-table".into())
    );
}

#[test]
fn plan_authority_accepts_only_one_safe_explicit_declaration() {
    assert_eq!(
        declared_authority("context: |\n  specs/TODO.md is the only task-status authority.\n"),
        Ok(Some("specs/TODO.md".into()))
    );
    assert_eq!(
        declared_authority("task_status_authority: 'docs/tasks.md' # chosen\n"),
        Ok(Some("docs/tasks.md".into()))
    );
    assert_eq!(
        declared_authority("task_status_authority: ../tasks.md"),
        Err("invalid-task-authority")
    );
    assert_eq!(
        declared_authority("task_status_authority: .env/tasks.md"),
        Err("invalid-task-authority")
    );
    assert_eq!(
        declared_authority("# task_status_authority: docs/tasks.md"),
        Ok(None)
    );
    assert_eq!(
        declared_authority("task_status_authority: docs/tasks.md\nstatus_authority: docs/other.md"),
        Err("ambiguous-task-authority")
    );
    assert_eq!(
        declared_authority("A task file can be an authority."),
        Ok(None)
    );
}
