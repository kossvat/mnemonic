//! Who drives a Claude Code session, judged by the transcript: a record that
//! names no entrypoint takes the driver of its transcript.
use super::conversation::ConversationWatcher;
use super::ingress::IngressTail;
use super::origin::{ClaudeOrigin, ClaudeSession};
use super::scope::ProjectScope;
use crate::storage::Storage;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const CORRECTION: &str = "stop, use the other configuration for demoapp";
const DECISION: &str = "1. Stop caching errors; we decided to keep PostgreSQL.";

struct Transcripts {
    dir: PathBuf,
    storage: Arc<Storage>,
}

impl Transcripts {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("mn-driver-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
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

    fn stretches(&self) -> usize {
        self.storage
            .conn
            .lock()
            .unwrap()
            .query_row("SELECT count(*) FROM work_stretches", [], |r| {
                r.get::<_, i64>(0)
            })
            .unwrap() as usize
    }
}

impl Drop for Transcripts {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A record of `role` holding `text` at minute `minute`, naming
/// `entrypoint` when there is one.
fn record(role: &str, id: &str, minute: u32, text: &str, entrypoint: Option<&str>) -> String {
    let mut value = serde_json::json!({"type": role, "uuid": id, "sessionId": "s",
        "timestamp": format!("2026-09-30T10:{minute:02}:00Z"),
        "message": {"content": [{"type": "text", "text": text}]}});
    if let Some(entrypoint) = entrypoint {
        value["entrypoint"] = entrypoint.into();
    }
    value.to_string()
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

/// The first record names the driver; the ones after it name none.
fn session(entrypoint: &str, prefix: &str, from: u32) -> Vec<String> {
    vec![
        record(
            "user",
            &format!("{prefix}0"),
            from,
            "Begin the review of the change.",
            Some(entrypoint),
        ),
        record("user", &format!("{prefix}1"), from + 1, CORRECTION, None),
        record("assistant", &format!("{prefix}2"), from + 2, DECISION, None),
    ]
}

#[test]
fn capture_origin_claude_a_record_without_an_entrypoint_takes_the_driver_of_its_transcript() {
    for (entrypoint, owner) in [
        ("sdk-py", false),
        ("sdk-ts", false),
        ("sdk-cli", false),
        ("cli", true),
        ("claude-desktop", true),
    ] {
        let t = Transcripts::new();
        let reader = IngressTail::new(t.storage.clone(), "conversation");
        let mut scope = ClaudeSession::new(ProjectScope::default());
        let path = t.dir.join("session.jsonl");
        append(&path, &session(entrypoint, "a", 0));
        reader
            .poll_file_scoped(&path, ConversationWatcher::parse_turn, &mut scope)
            .unwrap();
        // The opening record is no correction and no decision: it is not
        // taken in either case, but it counts as work in the owner's.
        let expected: Vec<String> = if owner {
            vec![CORRECTION.to_owned(), DECISION.to_owned()]
        } else {
            vec![]
        };
        assert_eq!(t.captured(), expected, "{entrypoint}");
        assert_eq!(t.stretches(), usize::from(owner), "{entrypoint}");
    }
}

#[test]
fn capture_origin_claude_a_record_that_names_its_entrypoint_is_judged_by_it() {
    let t = Transcripts::new();
    let reader = IngressTail::new(t.storage.clone(), "conversation");
    let mut scope = ClaudeSession::new(ProjectScope::default());
    let path = t.dir.join("session.jsonl");
    append(
        &path,
        &[
            record(
                "user",
                "p0",
                0,
                "Run the checks of the change.",
                Some("sdk-py"),
            ),
            record("user", "o1", 1, CORRECTION, Some("claude-desktop")),
        ],
    );
    reader
        .poll_file_scoped(&path, ConversationWatcher::parse_turn, &mut scope)
        .unwrap();
    assert_eq!(t.captured(), vec![CORRECTION.to_owned()]);
}

#[test]
fn capture_origin_claude_a_transcript_that_names_no_entrypoint_is_taken_as_before() {
    let t = Transcripts::new();
    let reader = IngressTail::new(t.storage.clone(), "conversation");
    let mut scope = ClaudeSession::new(ProjectScope::default());
    let path = t.dir.join("session.jsonl");
    append(
        &path,
        &[
            record("user", "n1", 0, CORRECTION, None),
            record("assistant", "n2", 1, DECISION, None),
        ],
    );
    reader
        .poll_file_scoped(&path, ConversationWatcher::parse_turn, &mut scope)
        .unwrap();
    assert_eq!(
        t.captured(),
        vec![CORRECTION.to_owned(), DECISION.to_owned()]
    );
    assert_eq!(t.stretches(), 1);
}

/// A daemon that starts again resumes a transcript past its first record:
/// the driver is read back from the first records, through the file read.
#[test]
fn capture_origin_claude_a_resumed_transcript_keeps_the_driver_of_its_first_records() {
    for (entrypoint, owner) in [("sdk-py", false), ("claude-desktop", true)] {
        let t = Transcripts::new();
        let reader = IngressTail::new(t.storage.clone(), "conversation");
        let path = t.dir.join("session.jsonl");
        append(&path, &session(entrypoint, "a", 0));
        let mut before = ClaudeSession::new(ProjectScope::default());
        reader
            .poll_file_scoped(&path, ConversationWatcher::parse_turn, &mut before)
            .unwrap();
        let first = t.captured().len();

        // A new process: a new scope, the cursor from the store.
        let mut after = ClaudeSession::new(ProjectScope::default());
        append(
            &path,
            &[
                record("user", "b1", 40, CORRECTION, None),
                record("assistant", "b2", 41, DECISION, None),
            ],
        );
        reader
            .poll_file_scoped(&path, ConversationWatcher::parse_turn, &mut after)
            .unwrap();
        let expected = if owner { (2, 4) } else { (0, 0) };
        assert_eq!((first, t.captured().len()), expected, "{entrypoint}");
        assert_eq!(t.stretches(), if owner { 2 } else { 0 }, "{entrypoint}");
    }
}

#[test]
fn capture_origin_claude_the_driver_is_learned_again_for_each_transcript_history_reads() {
    let mut origin = ClaudeOrigin::default();
    let program = session("sdk-py", "p", 0);
    assert!(
        program
            .iter()
            .all(|line| !super::ingress::RecordScope::admit(&mut origin, line))
    );
    origin.reset();
    let owner = session("cli", "o", 0);
    assert!(
        owner
            .iter()
            .all(|line| super::ingress::RecordScope::admit(&mut origin, line))
    );
}

/// History reads each transcript whole, by the same rule: a session a
/// program ran gives no turn and no stretch, its records without an
/// entrypoint included.
#[test]
fn capture_origin_claude_history_takes_the_driver_of_each_transcript() {
    let t = Transcripts::new();
    let projects = t.dir.join("claude-projects");
    let project = projects.join("-code-demoapp");
    std::fs::create_dir_all(&project).unwrap();
    append(&project.join("program.jsonl"), &session("sdk-py", "p", 0));
    append(&project.join("owner.jsonl"), &session("cli", "o", 10));
    let mut config = crate::config::Config::default();
    config.storage.db_path = t.dir.join("memory.db");
    config.watchers.conversation_enabled = true;
    config.watchers.conversation_sessions_dir = Some(projects);
    config.watchers.codex_enabled = false;

    super::history::ingest_history(&t.storage, &config, None, true).unwrap();
    assert_eq!(
        t.captured(),
        vec![CORRECTION.to_owned(), DECISION.to_owned()]
    );
    let (transcripts, stretches) =
        super::history::backfill_work_stretches(&t.storage, &config).unwrap();
    assert_eq!((transcripts, stretches), (2, 1));
    assert_eq!(t.stretches(), 1);
}
