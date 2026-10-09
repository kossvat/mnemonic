//! Whose words capture takes. A transcript holds more than the owner and
//! his agent: sessions a program ran, what a subagent was told and said,
//! and texts the app writes into the user's role. None of them is a turn.
//! The replies are fixtures that are kept; only where they were written
//! changes.
use super::codex::CodexWatcher;
use super::conversation::ConversationWatcher;
use super::ingress::{IngressTail, RecordScope};
use super::scope::{CodexSession, ProjectScope};
use crate::storage::Storage;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// A kept correction, as the owner types one.
const CORRECTION: &str = "stop, use the other configuration for demoapp";
/// How the owner might begin a message, the way a message of another
/// session begins.
const OWNER_HEAD: &str = "Another Claude session sent a message earlier, so";
/// A kept decision, as the agent says one.
const DECISION: &str = "1. Stop caching errors; we decided to keep PostgreSQL.";

/// A Claude Code record of `role` holding `text`, with the fields in
/// `extra` set on it.
fn claude(role: &str, text: &str, extra: serde_json::Value) -> String {
    let mut value = serde_json::json!({"type": role, "uuid": "m1", "sessionId": "s",
        "timestamp": "2026-09-30T10:00:00Z",
        "message": {"content": [{"type": "text", "text": text}]}});
    for (key, field) in extra.as_object().expect("an object of fields") {
        value[key] = field.clone();
    }
    value.to_string()
}

fn taken(line: &str) -> bool {
    ConversationWatcher::parse_turn(Path::new("/transcripts/s.jsonl"), line).is_some()
}

#[test]
fn capture_origin_a_turn_of_an_owner_session_is_taken_as_before() {
    let none = serde_json::json!({});
    assert!(taken(&claude("user", CORRECTION, none.clone())));
    assert!(taken(&claude("assistant", DECISION, none)));
    // The desktop app marks the owner's own prompts as `sdk`-sourced and
    // `human`-begun; the terminal marks them `cli`.
    for fields in [
        serde_json::json!({"entrypoint": "claude-desktop", "promptSource": "sdk",
            "turnOrigin": "human", "userType": "external", "isSidechain": false}),
        serde_json::json!({"entrypoint": "claude-desktop", "promptSource": "sdk"}),
        serde_json::json!({"entrypoint": "cli", "isMeta": false}),
        // A value this version does not name is taken, as before.
        serde_json::json!({"entrypoint": "claude-desktop", "turnOrigin": "voice"}),
    ] {
        assert!(
            taken(&claude("user", CORRECTION, fields.clone())),
            "{fields}"
        );
        assert!(
            taken(&claude("assistant", DECISION, fields.clone())),
            "{fields}"
        );
    }
}

#[test]
fn capture_origin_a_session_a_program_ran_is_not_a_turn() {
    for entrypoint in ["sdk-py", "sdk-ts", "sdk-cli"] {
        let fields = serde_json::json!({"entrypoint": entrypoint, "promptSource": "sdk",
            "turnOrigin": "sdk"});
        assert!(
            !taken(&claude("user", CORRECTION, fields.clone())),
            "{entrypoint}"
        );
        // What the agent of such a session says is not taken either.
        let fields = serde_json::json!({"entrypoint": entrypoint});
        assert!(
            !taken(&claude("assistant", DECISION, fields)),
            "{entrypoint}"
        );
    }
}

#[test]
fn capture_origin_a_subagent_is_not_the_owner() {
    let fields = serde_json::json!({"entrypoint": "claude-desktop", "isSidechain": true});
    assert!(!taken(&claude("user", CORRECTION, fields.clone())));
    assert!(!taken(&claude("assistant", DECISION, fields)));
}

#[test]
fn capture_origin_what_the_app_writes_into_the_user_role_is_not_a_turn() {
    for fields in [
        serde_json::json!({"isMeta": true}),
        serde_json::json!({"promptSource": "system"}),
        serde_json::json!({"turnOrigin": "task_notification"}),
        serde_json::json!({"promptSource": "system", "turnOrigin": "task_notification"}),
        serde_json::json!({"turnOrigin": "peer", "isMeta": true}),
        serde_json::json!({"turnOrigin": "peer"}),
        serde_json::json!({"turnOrigin": "sdk"}),
        // The summary a compaction writes, also on a turn the owner began.
        serde_json::json!({"isCompactSummary": true}),
        serde_json::json!({"isCompactSummary": true, "turnOrigin": "human",
            "isVisibleInTranscriptOnly": true}),
    ] {
        assert!(
            !taken(&claude("user", CORRECTION, fields.clone())),
            "{fields}"
        );
    }
    // The fields of the user's role say nothing of the agent's.
    let fields = serde_json::json!({"isMeta": true, "turnOrigin": "task_notification"});
    assert!(taken(&claude("assistant", DECISION, fields)));
}

#[test]
fn capture_origin_an_app_text_without_fields_is_known_by_how_it_begins() {
    let none = serde_json::json!({});
    for head in [
        "<task-notification>",
        "This session is being continued from a previous conversation",
        "<command-name>",
        "<command-message>",
        "<local-command-stdout>",
        "Another Claude session sent a message:",
        "<cross-session-message from=\"peer\">",
        "[Request interrupted by user]",
    ] {
        for text in [
            format!("{head} {CORRECTION}"),
            format!("\n  {head}\n{CORRECTION}"),
        ] {
            assert!(!taken(&claude("user", &text, none.clone())), "{text:?}");
        }
    }
    // A record that says the owner began its turn is not judged by its
    // head: he may start a message with any words.
    let owner = serde_json::json!({"entrypoint": "claude-desktop", "promptSource": "sdk",
        "turnOrigin": "human"});
    for head in [
        OWNER_HEAD,
        "<task-notification>",
        "[Request interrupted by user]",
    ] {
        let text = format!("{head} {CORRECTION}");
        assert!(taken(&claude("user", &text, owner.clone())), "{text:?}");
    }
    // Only at the head: the owner may quote such a text further on.
    let quoted = format!("{CORRECTION}\n<task-notification> was the wrong one");
    assert!(taken(&claude("user", &quoted, none.clone())));
    // And only in the user's role.
    let said = format!("<task-notification> {DECISION}");
    assert!(taken(&claude("assistant", &said, none)));
}

struct Rollouts {
    dir: PathBuf,
    storage: Arc<Storage>,
}

impl Rollouts {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("mn-origin-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(dir.join("sessions")).unwrap();
        let storage = Arc::new(Storage::open(&dir.join("memory.db")).unwrap());
        Self { dir, storage }
    }

    fn captured(&self) -> Vec<String> {
        self.storage
            .pending_ingest(chrono::Utc::now(), 100)
            .unwrap()
            .into_iter()
            .map(|pending| pending.event.content)
            .collect()
    }

    fn rollout(&self, name: &str) -> PathBuf {
        self.dir
            .join("sessions")
            .join(format!("rollout-{name}.jsonl"))
    }
}

impl Drop for Rollouts {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn append(path: &Path, lines: &[String]) {
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap();
    for line in lines {
        writeln!(file, "{line}").unwrap();
    }
}

fn session_meta(payload: serde_json::Value) -> String {
    serde_json::json!({"type": "session_meta", "timestamp": "2026-09-30T10:00:00Z",
        "payload": payload})
    .to_string()
}

fn codex(role: &str, id: &str, text: &str) -> String {
    let block = if role == "user" {
        "input_text"
    } else {
        "output_text"
    };
    serde_json::json!({"type": "response_item", "timestamp": "2026-09-30T10:00:01Z",
        "payload": {"type": "message", "role": role, "id": id,
                    "content": [{"type": block, "text": text}]}})
    .to_string()
}

fn exchange(prefix: &str) -> Vec<String> {
    vec![
        codex("user", &format!("{prefix}-u"), CORRECTION),
        codex("assistant", &format!("{prefix}-a"), DECISION),
    ]
}

/// The kinds of `session_meta` a program writes, and whether its turns are
/// the owner's.
fn sessions() -> Vec<(&'static str, serde_json::Value, bool)> {
    vec![
        (
            "exec",
            serde_json::json!({"cwd": "/code/demoapp", "originator": "codex_exec", "source": "exec"}),
            false,
        ),
        (
            "review",
            serde_json::json!({"cwd": "/code/demoapp", "originator": "codex_exec",
                "source": {"subagent": "review"}}),
            false,
        ),
        (
            "spawned",
            serde_json::json!({"cwd": "/code/demoapp", "originator": "Codex Desktop",
                "source": {"subagent": {"thread_spawn": {"parent_thread_id": "t1"}}}}),
            false,
        ),
        (
            "desktop",
            serde_json::json!({"cwd": "/code/demoapp", "originator": "Codex Desktop", "source": "vscode"}),
            true,
        ),
        (
            "terminal",
            serde_json::json!({"cwd": "/code/demoapp", "originator": "codex_cli_rs", "source": "cli"}),
            true,
        ),
        (
            "exec-source",
            serde_json::json!({"cwd": "/code/demoapp", "source": "exec"}),
            false,
        ),
        (
            "exec-originator",
            serde_json::json!({"cwd": "/code/demoapp", "originator": "codex_exec"}),
            false,
        ),
        ("bare", serde_json::json!({"cwd": "/code/demoapp"}), true),
    ]
}

#[test]
fn capture_origin_a_codex_session_a_program_ran_is_not_captured() {
    for (name, payload, owner) in sessions() {
        let r = Rollouts::new();
        let reader = IngressTail::new(r.storage.clone(), "codex");
        let mut scope = CodexSession::new(ProjectScope::default());
        let path = r.rollout(name);
        let mut lines = vec![session_meta(payload)];
        lines.extend(exchange(name));
        append(&path, &lines);
        reader
            .poll_file_scoped(&path, CodexWatcher::parse_turn, &mut scope)
            .unwrap();
        let expected: Vec<String> = if owner {
            vec![CORRECTION.to_owned(), DECISION.to_owned()]
        } else {
            vec![]
        };
        assert_eq!(r.captured(), expected, "{name}");
    }
}

#[test]
fn capture_origin_a_rollout_without_session_meta_is_taken_as_before() {
    let r = Rollouts::new();
    let reader = IngressTail::new(r.storage.clone(), "codex");
    let mut scope = CodexSession::new(ProjectScope::default());
    let path = r.rollout("headless");
    append(&path, &exchange("h"));
    reader
        .poll_file_scoped(&path, CodexWatcher::parse_turn, &mut scope)
        .unwrap();
    assert_eq!(r.captured().len(), 2);
}

/// A daemon that starts again resumes a rollout past its first record: the
/// origin is read back from that record, through the file being read.
#[test]
fn capture_origin_a_resumed_rollout_keeps_the_origin_of_its_first_record() {
    for (name, payload, owner) in sessions() {
        let r = Rollouts::new();
        let reader = IngressTail::new(r.storage.clone(), "codex");
        let path = r.rollout(name);
        let mut lines = vec![session_meta(payload)];
        lines.extend(exchange("first"));
        append(&path, &lines);
        let mut before = CodexSession::new(ProjectScope::default());
        reader
            .poll_file_scoped(&path, CodexWatcher::parse_turn, &mut before)
            .unwrap();
        let after_first = r.captured().len();

        // A new process: a new scope, the cursor from the store.
        let mut after = CodexSession::new(ProjectScope::default());
        append(&path, &exchange("second"));
        reader
            .poll_file_scoped(&path, CodexWatcher::parse_turn, &mut after)
            .unwrap();
        let expected = if owner { (2, 4) } else { (0, 0) };
        assert_eq!((after_first, r.captured().len()), expected, "{name}");
    }
}

#[test]
fn capture_origin_a_restricted_scope_still_judges_the_folder() {
    let demo = ProjectScope::new(&[PathBuf::from("/code/demoapp")]);
    let cases = [
        (
            serde_json::json!({"cwd": "/code/demoapp", "originator": "Codex Desktop", "source": "vscode"}),
            2,
        ),
        (
            serde_json::json!({"cwd": "/code/private", "originator": "Codex Desktop", "source": "vscode"}),
            0,
        ),
        (
            serde_json::json!({"cwd": "/code/demoapp", "originator": "codex_exec", "source": "exec"}),
            0,
        ),
    ];
    for (payload, count) in cases {
        let r = Rollouts::new();
        let reader = IngressTail::new(r.storage.clone(), "codex");
        let mut scope = CodexSession::new(demo.clone());
        let path = r.rollout("scoped");
        let mut lines = vec![session_meta(payload.clone())];
        lines.extend(exchange("s"));
        append(&path, &lines);
        reader
            .poll_file_scoped(
                &path,
                CodexWatcher::parse_turn,
                &mut scope as &mut dyn RecordScope,
            )
            .unwrap();
        assert_eq!(r.captured().len(), count, "{payload}");
    }
}
