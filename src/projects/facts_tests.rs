//! The facts of records, read from lines as the two clients write them.
use super::*;
use crate::projects::resolver::{Outcome, State, step};
use serde_json::json;
use std::path::Path;

const A: i64 = 1;
const B: i64 = 2;

fn map() -> ProjectMap {
    let mut map = ProjectMap::new(Path::new("/h/workspace/projects"));
    map.insert("demoapp", A);
    map.insert("notes", B);
    map
}

fn tool_use(name: &str, input: serde_json::Value) -> serde_json::Value {
    json!({"type": "tool_use", "id": "t1", "name": name, "input": input})
}

fn assistant(cwd: Option<&str>, blocks: Vec<serde_json::Value>) -> String {
    let mut record = json!({"type": "assistant", "isSidechain": false,
        "message": {"role": "assistant", "content": blocks, "stop_reason": "tool_use"},
        "uuid": "u1", "timestamp": "2026-09-30T10:00:00.250Z", "entrypoint": "cli"});
    if let Some(cwd) = cwd {
        record["cwd"] = cwd.into();
    }
    record.to_string()
}

fn facts(line: &str) -> Facts {
    claude(line, &map()).expect("a record").facts
}

const IN_A: &str = "/h/workspace/projects/demoapp";

#[test]
fn resolver_facts_a_call_of_a_tool_has_a_class_and_the_projects_its_input_names() {
    let edit = tool_use(
        "Edit",
        json!({"file_path": "/h/workspace/projects/notes/a.md",
        "old_string": "x", "new_string": "y"}),
    );
    let line = assistant(Some(IN_A), vec![edit]);
    let observed = claude(&line, &map()).unwrap();
    // The record says 10:00:00.250: times are kept to the second.
    assert_eq!(
        observed.at.unwrap().to_rfc3339(),
        "2026-09-30T10:00:00+00:00"
    );
    assert_eq!(observed.facts.folder, Folder::Project(A));
    assert_eq!(observed.facts.call, Some((ToolClass::Write, Named::One(B))));

    for (tool, class) in [
        ("Write", ToolClass::Write),
        ("MultiEdit", ToolClass::Write),
        ("NotebookEdit", ToolClass::Write),
        ("Bash", ToolClass::Shell),
        ("Read", ToolClass::Read),
        ("Grep", ToolClass::Read),
        ("a_tool_of_a_server", ToolClass::Read),
    ] {
        let block = tool_use(tool, json!({"path": "/h/workspace/projects/notes"}));
        assert_eq!(
            facts(&assistant(None, vec![block])).call,
            Some((class, Named::One(B))),
            "{tool}"
        );
    }
    // A call whose input names no project is still a call.
    let block = tool_use("Bash", json!({"command": "cargo test --release"}));
    assert_eq!(
        facts(&assistant(None, vec![block])).call,
        Some((ToolClass::Shell, Named::None))
    );
}

#[test]
fn resolver_facts_two_calls_in_one_record_are_read_together() {
    let read = tool_use(
        "Read",
        json!({"file_path": "/h/workspace/projects/demoapp/a"}),
    );
    let edit = tool_use(
        "Edit",
        json!({"file_path": "/h/workspace/projects/demoapp/b"}),
    );
    let other = tool_use(
        "Read",
        json!({"file_path": "/h/workspace/projects/notes/c"}),
    );
    assert_eq!(
        facts(&assistant(None, vec![read.clone(), edit])).call,
        Some((ToolClass::Write, Named::One(A)))
    );
    assert_eq!(
        facts(&assistant(None, vec![read, other])).call,
        Some((ToolClass::Read, Named::Many))
    );
}

#[test]
fn resolver_facts_a_path_found_only_in_a_result_changes_nothing() {
    // What a tool gave back, and what was said, name a project; no call does.
    let result = json!({"type": "user", "cwd": IN_A, "timestamp": "2026-09-30T10:00:01Z",
        "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "t1",
            "content": "/h/workspace/projects/notes/a.md:1: found"}]},
        "toolUseResult": {"stdout": "/h/workspace/projects/notes/a.md"}})
    .to_string();
    let said = json!({"type": "assistant", "cwd": IN_A,
        "message": {"content": [{"type": "text",
            "text": "I will look at /h/workspace/projects/notes next."}]}})
    .to_string();
    let typed = json!({"type": "user", "cwd": IN_A,
        "message": {"content": "open /h/workspace/projects/notes/a.md"}})
    .to_string();
    let state = step(
        State::default(),
        None,
        &facts(&assistant(Some(IN_A), vec![])),
    )
    .0;
    assert_eq!(state.project, Some(A));
    for line in [result, said, typed] {
        let observed = facts(&line);
        assert_eq!(observed.call, None, "{line}");
        assert_eq!(step(state, None, &observed), (state, Outcome::Stayed));
    }
}

#[test]
fn resolver_facts_a_record_says_where_the_session_is_or_does_not() {
    assert_eq!(
        facts(&assistant(Some(IN_A), vec![])).folder,
        Folder::Project(A)
    );
    assert_eq!(
        facts(&assistant(
            Some("/h/workspace/projects/demoapp/src/deep"),
            vec![]
        ))
        .folder,
        Folder::Project(A)
    );
    assert_eq!(
        facts(&assistant(Some("/h/workspace"), vec![])).folder,
        Folder::Outside
    );
    assert_eq!(facts(&assistant(None, vec![])).folder, Folder::Unsaid);
    // A record of another kind is still a record, with its time.
    let other = json!({"type": "attachment", "timestamp": "2026-09-30T10:00:02Z"}).to_string();
    let observed = claude(&other, &map()).unwrap();
    assert!(observed.at.is_some());
    assert_eq!(observed.facts, Facts::default());
    // What is no record is none.
    for line in ["", "not json", "[1, 2]", "{\"type\": \"user\""] {
        assert!(claude(line, &map()).is_none(), "{line}");
        assert!(codex(line, &map()).is_none(), "{line}");
    }
}

fn codex_item(payload: serde_json::Value) -> String {
    json!({"type": "response_item", "timestamp": "2026-09-30T10:00:03Z", "payload": payload})
        .to_string()
}

#[test]
fn resolver_facts_a_codex_rollout_gives_the_same_facts() {
    let map = map();
    let read = |line: &str| codex(line, &map).expect("a record").facts;
    for kind in ["session_meta", "turn_context"] {
        let line = json!({"type": kind, "timestamp": "2026-09-30T10:00:00Z",
            "payload": {"cwd": "/h/workspace/projects/notes", "originator": "a client"}})
        .to_string();
        assert_eq!(read(&line).folder, Folder::Project(B), "{kind}");
    }
    let outside = json!({"type": "turn_context", "payload": {"cwd": "/h/workspace"}}).to_string();
    assert_eq!(read(&outside).folder, Folder::Outside);

    // The shell, under each name it had, with its input as a text.
    let arguments = json!({"command": ["bash", "-lc", "cd /h/workspace/projects/demoapp && ls"],
        "workdir": "/h/workspace"})
    .to_string();
    for name in ["shell", "exec_command", "shell_command"] {
        let line = codex_item(json!({"type": "function_call", "name": name,
            "arguments": arguments, "call_id": "c1"}));
        assert_eq!(
            read(&line).call,
            Some((ToolClass::Shell, Named::One(A))),
            "{name}"
        );
    }
    let custom = codex_item(json!({"type": "custom_tool_call", "name": "exec",
        "input": "ls /h/workspace/projects/notes", "status": "completed"}));
    assert_eq!(read(&custom).call, Some((ToolClass::Shell, Named::One(B))));
    let local = codex_item(json!({"type": "local_shell_call",
        "action": {"type": "exec", "command": ["ls", "/h/workspace/projects/notes"]}}));
    assert_eq!(read(&local).call, Some((ToolClass::Shell, Named::One(B))));

    // A patch is a write, by its own tool or through the shell.
    let patch = "*** Begin Patch\n*** Update File: /h/workspace/projects/notes/a.md\n*** End Patch";
    for payload in [
        json!({"type": "custom_tool_call", "name": "apply_patch", "input": patch}),
        json!({"type": "custom_tool_call", "name": "exec", "input": patch}),
        json!({"type": "function_call", "name": "apply_patch",
            "arguments": json!({"input": patch}).to_string()}),
    ] {
        let call = read(&codex_item(payload.clone())).call;
        assert_eq!(call, Some((ToolClass::Write, Named::One(B))), "{payload}");
    }
    // Another tool reads.
    let other = codex_item(json!({"type": "function_call", "name": "view_image",
        "arguments": "{\"path\":\"/h/workspace/projects/notes/a.png\"}"}));
    assert_eq!(read(&other).call, Some((ToolClass::Read, Named::One(B))));

    // What a call gave back, and what was said, are no call.
    for payload in [
        json!({"type": "custom_tool_call_output", "call_id": "c1",
            "output": "/h/workspace/projects/notes/a.md"}),
        json!({"type": "function_call_output", "call_id": "c1",
            "output": "/h/workspace/projects/notes/a.md"}),
        json!({"type": "message", "role": "assistant", "content": [{"type": "output_text",
            "text": "see /h/workspace/projects/notes"}]}),
    ] {
        assert_eq!(
            read(&codex_item(payload.clone())),
            Facts::default(),
            "{payload}"
        );
    }
}

/// The readers and the rules over a transcript in memory. Run with
/// `cargo test --release resolver_benchmark -- --ignored --nocapture`.
/// Prints sizes and times only.
#[test]
#[ignore]
fn resolver_benchmark_reads_200_mb_a_second() {
    let map = map();
    // A transcript as a long session writes it: short calls, long results,
    // text of the agent, and records of other kinds.
    let result_text = "line of output that a tool printed, with nothing to find\n".repeat(140);
    let lines = [
        assistant(
            Some(IN_A),
            vec![tool_use(
                "Bash",
                json!({"command": "cd /h/workspace/projects/demoapp && cargo test"}),
            )],
        ),
        json!({"type": "user", "cwd": IN_A, "timestamp": "2026-09-30T10:00:01Z",
            "message": {"role": "user", "content": [{"type": "tool_result",
                "tool_use_id": "t1", "content": result_text}]}})
        .to_string(),
        assistant(
            Some(IN_A),
            vec![json!({"type": "text", "text": "what was done ".repeat(60)})],
        ),
        assistant(
            Some(IN_A),
            vec![tool_use(
                "Edit",
                json!({"file_path": "/h/workspace/projects/notes/a.md",
                "old_string": "before ".repeat(40), "new_string": "after ".repeat(40)}),
            )],
        ),
        json!({"type": "attachment", "timestamp": "2026-09-30T10:00:02Z",
            "attachment": {"kind": "context", "text": "more text that names nothing ".repeat(50)}})
        .to_string(),
    ];
    let mut transcript = String::new();
    while transcript.len() < 64 << 20 {
        for line in &lines {
            transcript.push_str(line);
            transcript.push('\n');
        }
    }
    let mut best = f64::MAX;
    let mut switches = 0usize;
    for _ in 0..3 {
        let began = std::time::Instant::now();
        let mut state = State::default();
        switches = 0;
        for line in transcript.lines() {
            if let Some(observed) = claude(line, &map) {
                let (next, outcome) = step(state, None, &observed.facts);
                switches += usize::from(outcome == Outcome::Switched);
                state = next;
            }
        }
        std::hint::black_box(state);
        best = best.min(began.elapsed().as_secs_f64());
    }
    let rate = transcript.len() as f64 / best / 1e6;
    eprintln!(
        "resolver: {} bytes in {:.1} ms, {rate:.0} MB/s, {switches} switches",
        transcript.len(),
        best * 1e3
    );
    assert!(switches > 0, "the fixture turned no rule");
    assert!(rate >= 200.0, "the resolver read {rate:.0} MB a second");
}

#[test]
fn resolver_facts_a_codex_shell_call_says_where_it_runs() {
    let map = map();
    let read = |line: &str| codex(line, &map).expect("a record").facts;
    let arguments = json!({"command": ["cargo", "test"], "workdir": "/h/workspace/projects/notes"});
    let call = codex_item(json!({"type": "function_call", "name": "exec_command",
        "arguments": arguments.to_string()}));
    assert_eq!(read(&call).shell_dir, Folder::Project(B));
    let local = codex_item(json!({"type": "local_shell_call",
        "action": {"type": "exec", "command": ["ls"], "working_directory": "/h/elsewhere"}}));
    assert_eq!(read(&local).shell_dir, Folder::Outside);
    // A call that says nothing of its folder runs where the session is.
    let bare = codex_item(json!({"type": "custom_tool_call", "name": "exec", "input": "ls"}));
    assert_eq!(read(&bare).shell_dir, Folder::Unsaid);
}

fn user(text: &str, fields: serde_json::Value) -> String {
    let mut record = json!({"type": "user", "cwd": IN_A, "timestamp": "2026-09-30T10:00:00Z",
        "message": {"role": "user", "content": text}});
    for (key, field) in fields.as_object().unwrap() {
        record[key] = field.clone();
    }
    record.to_string()
}

#[test]
fn observation_facts_a_prompt_of_the_owner_is_what_the_owner_typed() {
    let map = map();
    let kind = |line: &str| claude(line, &map).unwrap().kind;
    let none = json!({});
    assert_eq!(kind(&user("look at the tests", none.clone())), Kind::Prompt);
    // However short: the owner was there.
    assert_eq!(kind(&user("ok", none.clone())), Kind::Prompt);
    assert_eq!(
        kind(&user(
            "ok",
            json!({"entrypoint": "claude-desktop", "turnOrigin": "human"})
        )),
        Kind::Prompt
    );
    let blocks =
        json!({"type": "user", "message": {"content": [{"type": "text", "text": "and this"}]}});
    assert_eq!(kind(&blocks.to_string()), Kind::Prompt);

    for fields in [
        json!({"isSidechain": true}),
        json!({"isCompactSummary": true}),
        json!({"isCompactSummary": true, "turnOrigin": "human"}),
        json!({"isMeta": true}),
        json!({"promptSource": "system"}),
        json!({"turnOrigin": "sdk"}),
        json!({"turnOrigin": "task_notification"}),
        json!({"turnOrigin": "peer"}),
        // A session a program ran has no owner at its keyboard.
        json!({"entrypoint": "sdk-py"}),
    ] {
        assert_eq!(
            kind(&user("look at the tests", fields.clone())),
            Kind::Other,
            "{fields}"
        );
    }
    // What the app writes into the user's role, and what a tool gave back.
    for text in [
        "<task-notification> done",
        "<system-reminder>context</system-reminder>",
        "  ",
    ] {
        assert_eq!(kind(&user(text, none.clone())), Kind::Other, "{text}");
    }
    let result = json!({"type": "user", "message": {"content": [{"type": "tool_result",
        "content": "look at the tests"}]}});
    assert_eq!(kind(&result.to_string()), Kind::Other);
    // The agent's records: a call, the close of a turn, anything else.
    let said = |stop: &str| {
        json!({"type": "assistant", "message": {"stop_reason": stop,
        "content": [{"type": "text", "text": "done"}]}})
        .to_string()
    };
    assert_eq!(kind(&said("end_turn")), Kind::TurnEnd);
    assert_eq!(kind(&said("tool_use")), Kind::Other);
    let call = tool_use(
        "Read",
        json!({"file_path": "/h/workspace/projects/notes/a"}),
    );
    assert_eq!(kind(&assistant(None, vec![call])), Kind::Call);
}

#[test]
fn observation_facts_a_codex_prompt_is_what_the_owner_typed() {
    let map = map();
    let message = |role: &str, text: &str| {
        codex_item(json!({"type": "message", "role": role,
            "content": [{"type": "input_text", "text": text}]}))
    };
    let kind = |line: &str| codex(line, &map).unwrap().kind;
    assert_eq!(kind(&message("user", "look at the tests")), Kind::Prompt);
    // What the client puts into the user's role opens with a tag.
    assert_eq!(
        kind(&message(
            "user",
            "<environment_context>cwd</environment_context>"
        )),
        Kind::Other
    );
    assert_eq!(
        kind(&message("developer", "look at the tests")),
        Kind::Other
    );
    assert_eq!(kind(&message("assistant", "looking")), Kind::Other);
    for closed in ["task_complete", "turn_aborted"] {
        let line = json!({"type": "event_msg", "payload": {"type": closed}}).to_string();
        assert_eq!(kind(&line), Kind::TurnEnd);
    }
    // Who drives the session is said by the record that opens it.
    let meta = |payload: serde_json::Value| {
        let line = json!({"type": "session_meta", "payload": payload}).to_string();
        codex(&line, &map).unwrap().program
    };
    assert_eq!(
        meta(json!({"originator": "a desktop client", "source": "vscode"})),
        Some(false)
    );
    assert_eq!(
        meta(json!({"originator": "codex_exec", "source": "exec"})),
        Some(true)
    );
    assert_eq!(
        meta(json!({"originator": "a desktop client", "source": {"subagent": {"id": 1}}})),
        Some(true)
    );
    assert_eq!(
        codex(&message("user", "hello there"), &map)
            .unwrap()
            .program,
        None
    );
}
