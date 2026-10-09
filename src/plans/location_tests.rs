use super::*;
use std::path::Path;

#[test]
fn plan_location_claude_reads_top_level_metadata_only() {
    let mut location = Location::default();
    location.observe(
        r#"{"cwd":"workspace/first","gitBranch":"task/T01","message":{"content":[{"type":"tool_result","content":{"cwd":"workspace/wrong","gitBranch":"wrong"}}]}}"#,
        "claude",
    );
    assert_eq!(location.cwd.as_deref(), Some(Path::new("workspace/first")));
    assert_eq!(location.branch.as_deref(), Some("task/T01"));
    location.observe(
        r#"{"message":{"content":[{"type":"tool_use","input":{"cwd":"workspace/wrong","gitBranch":"wrong"}}]}}"#,
        "claude",
    );
    assert_eq!(location.cwd.as_deref(), Some(Path::new("workspace/first")));
    assert_eq!(location.branch.as_deref(), Some("task/T01"));
}

#[test]
fn plan_location_codex_uses_session_and_turn_metadata() {
    let mut location = Location::default();
    location.observe(
        r#"{"type":"session_meta","payload":{"cwd":"workspace/first","git":{"branch":"task/T01"}}}"#,
        "codex",
    );
    assert_eq!(location.cwd.as_deref(), Some(Path::new("workspace/first")));
    assert_eq!(location.branch.as_deref(), Some("task/T01"));
    location.observe(
        r#"{"type":"turn_context","payload":{"cwd":"workspace/second","branch":"task/T02"}}"#,
        "codex",
    );
    assert_eq!(location.cwd.as_deref(), Some(Path::new("workspace/second")));
    assert_eq!(location.branch.as_deref(), Some("task/T02"));
    location.observe(
        r#"{"type":"turn_context","payload":{"git":{"branch":"task/T03"}}}"#,
        "codex",
    );
    assert_eq!(location.cwd.as_deref(), Some(Path::new("workspace/second")));
    assert_eq!(location.branch.as_deref(), Some("task/T03"));
}

#[test]
fn plan_location_codex_ignores_calls_results_and_quoted_metadata() {
    let mut location = Location::default();
    for kind in ["response_item", "event_msg", "other"] {
        let line = serde_json::json!({
            "type": kind,
            "cwd":"workspace/outer",
            "gitBranch":"wrong",
            "payload": {"cwd":"workspace/inner", "branch":"wrong", "git":{"branch":"wrong"}}
        });
        location.observe(&line.to_string(), "codex");
    }
    location.observe(
        r#"{"type":"response_item","payload":{"type":"function_call","arguments":"{\"cwd\":\"workspace/wrong\"}"}}"#,
        "codex",
    );
    assert!(location.cwd.is_none());
    assert!(location.branch.is_none());
}

#[test]
fn plan_location_null_clears_but_omission_retains() {
    let mut location = Location::default();
    location.observe(
        r#"{"cwd":"workspace/first","gitBranch":"task/T01"}"#,
        "claude",
    );
    location.observe(r#"{"type":"assistant"}"#, "claude");
    assert!(location.cwd.is_some());
    assert!(location.branch.is_some());
    location.observe(r#"{"gitBranch":null}"#, "claude");
    assert!(location.cwd.is_some());
    assert!(location.branch.is_none());
    location.observe(r#"{"cwd":null}"#, "claude");
    assert!(location.cwd.is_none());
    location.observe(
        r#"{"type":"session_meta","payload":{"cwd":"workspace/first","git":{"branch":"task/T01"}}}"#,
        "codex",
    );
    location.observe(
        r#"{"type":"turn_context","payload":{"git":{"branch":null}}}"#,
        "codex",
    );
    assert!(location.cwd.is_some());
    assert!(location.branch.is_none());
}

#[test]
fn plan_location_malformed_records_do_not_partially_change_state() {
    let mut location = Location::default();
    location.observe(
        r#"{"cwd":"workspace/first","gitBranch":"task/T01"}"#,
        "claude",
    );
    for line in [
        r#"{"cwd":"workspace/wrong","gitBranch":42}"#,
        r#"{"cwd":"workspace/wrong""#,
        r#"{"cwd":["workspace/wrong"]}"#,
    ] {
        location.observe(line, "claude");
    }
    location.observe(r#"{"cwd":"workspace/wrong"}"#, "unknown");
    assert_eq!(location.cwd.as_deref(), Some(Path::new("workspace/first")));
    assert_eq!(location.branch.as_deref(), Some("task/T01"));
}

#[test]
fn plan_location_handles_escaped_strings_without_copying_the_record() {
    let mut location = Location::default();
    location.observe(
        r#"{"cwd":"workspace/\u0066irst","gitBranch":"task/\u005401"}"#,
        "claude",
    );
    assert_eq!(location.cwd.as_deref(), Some(Path::new("workspace/first")));
    assert_eq!(location.branch.as_deref(), Some("task/T01"));
    let record: Claude<'_> = serde_json::from_str(r#"{"cwd":"workspace/first"}"#).unwrap();
    assert!(matches!(record.cwd, Field::Text(Cow::Borrowed(_))));
}

#[test]
fn plan_location_shared_claude_parse_keeps_prompts_with_invalid_optional_metadata() {
    use crate::projects::facts::{Kind, claude_with_location};
    let map = crate::projects::ProjectMap::default();
    for invalid in [
        serde_json::json!(42),
        serde_json::json!(false),
        serde_json::json!(["wrong"]),
        serde_json::json!({"nested": [1, 2]}),
    ] {
        let mut location = Location {
            cwd: Some("workspace/first".into()),
            branch: Some("task/T01".into()),
        };
        let line = serde_json::json!({"type":"user", "cwd":"workspace/second",
            "gitBranch":invalid, "timestamp":"2026-09-30T10:00:00Z", "entrypoint":"cli",
            "message":{"content":"Review the result"}})
        .to_string();
        let observed = claude_with_location(&line, &map, Some(&mut location)).unwrap();
        assert_eq!(observed.kind, Kind::Prompt);
        assert_eq!(observed.program, Some(false));
        assert!(observed.at.is_some());
        assert_eq!(location.cwd.as_deref(), Some(Path::new("workspace/first")));
        assert_eq!(location.branch.as_deref(), Some("task/T01"));
    }
}

#[test]
fn plan_location_shared_codex_parse_keeps_facts_with_invalid_optional_git() {
    use crate::projects::facts::{Kind, codex_with_location};
    let map = crate::projects::ProjectMap::default();
    for git in [
        serde_json::json!(42),
        serde_json::json!(["wrong"]),
        serde_json::json!({"branch":false}),
    ] {
        let mut location = Location {
            cwd: Some("workspace/first".into()),
            branch: Some("task/T01".into()),
        };
        let line = serde_json::json!({"type":"session_meta", "timestamp":"2026-09-30T10:00:00Z",
            "payload":{"cwd":"workspace/second", "originator":"codex_exec", "git":git}})
        .to_string();
        let observed = codex_with_location(&line, &map, Some(&mut location)).unwrap();
        assert_eq!(observed.kind, Kind::Other);
        assert_eq!(observed.program, Some(true));
        assert_eq!(
            observed.facts.folder,
            crate::projects::resolver::Folder::Outside
        );
        assert_eq!(location.cwd.as_deref(), Some(Path::new("workspace/first")));
        assert_eq!(location.branch.as_deref(), Some("task/T01"));
    }
}

#[test]
fn plan_location_shared_parse_preserves_null_omission_and_metadata_boundaries() {
    use crate::projects::facts::{claude_with_location, codex_with_location};
    let map = crate::projects::ProjectMap::default();
    let mut location = Location::default();
    claude_with_location(
        r#"{"cwd":"workspace/first","gitBranch":"task/T01"}"#,
        &map,
        Some(&mut location),
    )
    .unwrap();
    claude_with_location(r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Read","input":{"cwd":"workspace/wrong","gitBranch":null}}]}}"#,
        &map, Some(&mut location)).unwrap();
    assert_eq!(location.cwd.as_deref(), Some(Path::new("workspace/first")));
    assert_eq!(location.branch.as_deref(), Some("task/T01"));
    claude_with_location(r#"{"gitBranch":null}"#, &map, Some(&mut location)).unwrap();
    assert!(location.cwd.is_some());
    assert!(location.branch.is_none());
    codex_with_location(r#"{"type":"session_meta","payload":{"cwd":"workspace/second","git":{"branch":"task/T02"}}}"#,
        &map, Some(&mut location)).unwrap();
    codex_with_location(
        r#"{"type":"response_item","payload":{"cwd":"workspace/wrong","branch":null}}"#,
        &map,
        Some(&mut location),
    )
    .unwrap();
    assert_eq!(location.cwd.as_deref(), Some(Path::new("workspace/second")));
    assert_eq!(location.branch.as_deref(), Some("task/T02"));
    codex_with_location(
        r#"{"type":"turn_context","payload":{"branch":null,"git":{"branch":"wrong"}}}"#,
        &map,
        Some(&mut location),
    )
    .unwrap();
    assert!(location.cwd.is_some());
    assert!(location.branch.is_none());
    codex_with_location(
        r#"{"type":"turn_context","payload":{"cwd":null}}"#,
        &map,
        Some(&mut location),
    )
    .unwrap();
    assert!(location.cwd.is_none());
    assert!(location.branch.is_none());
}

#[test]
fn plan_location_shared_parse_still_refuses_invalid_fact_fields() {
    use crate::projects::facts::{claude_with_location, codex_with_location};
    let map = crate::projects::ProjectMap::default();
    let mut location = Location {
        cwd: Some("workspace/first".into()),
        branch: Some("task/T01".into()),
    };
    assert!(
        claude_with_location(r#"{"cwd":42,"gitBranch":null}"#, &map, Some(&mut location)).is_none()
    );
    assert!(
        codex_with_location(
            r#"{"type":"turn_context","payload":{"cwd":false,"branch":null}}"#,
            &map,
            Some(&mut location)
        )
        .is_none()
    );
    assert_eq!(location.cwd.as_deref(), Some(Path::new("workspace/first")));
    assert_eq!(location.branch.as_deref(), Some("task/T01"));
}
