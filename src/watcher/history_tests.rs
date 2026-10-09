//! History through the real parsers and store, with no daemon.
use super::*;
use crate::watcher::ingress::IngressTail;
use std::collections::HashMap;
use std::io::Write;
use std::sync::Arc;

struct Fixture {
    root: PathBuf,
    claude: PathBuf,
    codex: PathBuf,
    storage: Arc<Storage>,
}
impl Fixture {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("mn-history-{}", uuid::Uuid::new_v4().simple()));
        let claude = root.join("claude-projects");
        let codex = root.join("codex-sessions");
        std::fs::create_dir_all(claude.join("-code-demoapp")).unwrap();
        std::fs::create_dir_all(codex.join("2026/09/01")).unwrap();
        let storage = Arc::new(Storage::open(&root.join("memory.db")).unwrap());
        Self {
            root,
            claude,
            codex,
            storage,
        }
    }
    fn config(&self, roots: &[&str]) -> Config {
        let mut config = Config::default();
        // The fixture's own store: the floor looks next to it.
        config.storage.db_path = self.root.join("memory.db");
        config.watchers.conversation_enabled = true;
        config.watchers.conversation_sessions_dir = Some(self.claude.clone());
        config.watchers.codex_enabled = true;
        config.watchers.codex_sessions_dir = Some(self.codex.clone());
        config.watchers.project_roots = roots.iter().map(PathBuf::from).collect();
        config
    }
    fn run(&self, roots: &[&str], since: Option<&str>, apply: bool) -> HistoryReport {
        let since = since.map(|day| {
            chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d")
                .unwrap()
                .and_time(chrono::NaiveTime::MIN)
                .and_utc()
        });
        ingest_history(&self.storage, &self.config(roots), since, apply).unwrap()
    }
    fn queued(&self) -> Vec<crate::event::Event> {
        self.storage
            .pending_ingest(Utc::now(), 1000)
            .unwrap()
            .into_iter()
            .map(|pending| pending.event)
            .collect()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn write(path: &Path, lines: &[String]) {
    let mut file = std::fs::File::create(path).unwrap();
    for line in lines {
        writeln!(file, "{line}").unwrap();
    }
}

fn claude(cwd: &str, id: &str, day: &str, text: &str) -> String {
    serde_json::json!({"type": "user", "uuid": id, "sessionId": format!("s-{id}"), "cwd": cwd,
        "timestamp": format!("{day}T10:00:00Z"), "message": {"content": text}})
    .to_string()
}
fn codex_meta(cwd: &str) -> String {
    serde_json::json!({"type": "session_meta", "payload": {"cwd": cwd}}).to_string()
}
fn codex_user(id: &str, day: &str, text: &str) -> String {
    serde_json::json!({"type": "response_item", "timestamp": format!("{day}T10:00:00Z"),
        "payload": {"type": "message", "role": "user", "id": id,
                    "content": [{"type": "input_text", "text": text}]}})
    .to_string()
}

/// Two months of history across both formats, two projects mixed together.
fn seed(f: &Fixture) {
    write(
        &f.claude.join("-code-demoapp/old.jsonl"),
        &[
            claude(
                "/code/demoapp",
                "c1",
                "2026-07-01",
                "это не то, цену ставим 42",
            ),
            claude(
                "/code/demoapp",
                "c3",
                "2026-09-01",
                "это не то, домен меняем на demo.example",
            ),
        ],
    );
    write(
        &f.claude.join("-code-demoapp/private.jsonl"),
        &[claude(
            "/code/private",
            "c2",
            "2026-07-02",
            "это не то, секрет другого проекта",
        )],
    );
    // Launched in the monorepo, then the agent stepped into the project.
    write(
        &f.claude.join("-code-demoapp/drifted.jsonl"),
        &[
            claude("/code", "c4", "2026-08-01", "как дела"),
            claude(
                "/code/demoapp",
                "c5",
                "2026-08-01",
                "это не то, секрет из монорепо",
            ),
        ],
    );
    // /compact copied c1 into a new transcript under a new session id.
    write(
        &f.claude.join("-code-demoapp/compacted.jsonl"),
        &[claude(
            "/code/demoapp",
            "c1",
            "2026-07-01",
            "это не то, цену ставим 42",
        )],
    );
    write(
        &f.codex.join("2026/09/01/rollout-a.jsonl"),
        &[
            codex_meta("/code/demoapp"),
            codex_user("x1", "2026-09-01", "это не то, тесты гоняем на CI"),
        ],
    );
    write(
        &f.codex.join("2026/09/01/rollout-b.jsonl"),
        &[
            codex_meta("/code/private"),
            codex_user("x2", "2026-09-01", "это не то, чужой секрет"),
        ],
    );
}

#[test]
fn a_dry_run_counts_and_writes_nothing() {
    let f = Fixture::new();
    seed(&f);
    let report = f.run(&["/code/demoapp"], None, false);
    assert_eq!(report.files, 6);
    assert_eq!(report.turns, 3, "c1, c3 and x1: {report:?}");
    assert_eq!(report.new, 3);
    assert_eq!(report.out_of_scope, 3, "c2, the drifted c5, and x2");
    assert_eq!(report.floor, None, "a fresh store has no earlier capture");
    assert_eq!(report.repeated, 1, "c1 copied by /compact");
    assert!(!report.applied);
    assert!(f.queued().is_empty());
}

#[test]
fn apply_queues_only_the_project_with_original_dates_and_the_history_mark() {
    let f = Fixture::new();
    seed(&f);
    let report = f.run(&["/code/demoapp"], None, true);
    assert!(report.applied);
    let queued = f.queued();
    let mut texts: Vec<&str> = queued.iter().map(|e| e.content.as_str()).collect();
    texts.sort();
    assert_eq!(
        texts,
        [
            "это не то, домен меняем на demo.example",
            "это не то, тесты гоняем на CI",
            "это не то, цену ставим 42",
        ]
    );
    assert!(!queued.iter().any(|e| e.content.contains("секрет")));
    // The date of the conversation, not of the import.
    let price = queued.iter().find(|e| e.content.contains("42")).unwrap();
    assert_eq!(price.timestamp.format("%Y-%m-%d").to_string(), "2026-07-01");
    // Marked so the daemon runs the duplicate check on it.
    assert!(queued.iter().all(|e| e.metadata[HISTORY_FLAG] == true));
}

#[test]
fn running_it_again_adds_nothing() {
    let f = Fixture::new();
    seed(&f);
    f.run(&["/code/demoapp"], None, true);
    let again = f.run(&["/code/demoapp"], None, true);
    assert_eq!((again.turns, again.known, again.new), (3, 3, 0));
    assert_eq!(f.queued().len(), 3);
}

#[test]
fn since_leaves_older_turns_out() {
    let f = Fixture::new();
    seed(&f);
    let report = f.run(&["/code/demoapp"], Some("2026-08-01"), false);
    assert_eq!((report.turns, report.before_since), (2, 1));
}

#[test]
fn a_turn_live_capture_already_took_is_not_added_twice() {
    let f = Fixture::new();
    let path = f.claude.join("-code-demoapp/live.jsonl");
    std::fs::write(&path, "").unwrap();
    // The daemon adopted the empty file, then captured a turn live.
    let reader = IngressTail::new(f.storage.clone(), "conversation");
    reader
        .bootstrap(std::slice::from_ref(&path), &HashMap::new())
        .unwrap();
    write(
        &path,
        &[claude(
            "/code/demoapp",
            "c9",
            "2026-09-20",
            "это не то, живой захват",
        )],
    );
    reader
        .poll_file(&path, ConversationWatcher::parse_turn)
        .unwrap();
    assert_eq!(f.queued().len(), 1);

    let report = f.run(&["/code/demoapp"], None, true);
    assert_eq!((report.known, report.new), (1, 0));
    assert_eq!(f.queued().len(), 1);
}

#[test]
fn without_project_roots_every_project_counts() {
    let f = Fixture::new();
    seed(&f);
    let report = f.run(&[], None, false);
    assert_eq!((report.turns, report.out_of_scope), (6, 0));
}

#[test]
fn a_store_that_captured_before_durable_ingress_does_not_reach_back() {
    let f = Fixture::new();
    seed(&f);
    // The pre-ingress watcher left its offset file next to the database.
    std::fs::write(f.root.join("watcher_offsets.json"), "{}").unwrap();
    let report = f.run(&["/code/demoapp"], None, false);
    assert!(report.floor.is_some());
    assert_eq!(report.turns, 0, "{report:?}");
    assert_eq!(report.before_capture, 3);
}

#[test]
fn forgetting_the_last_early_memory_does_not_lift_the_floor() {
    let mut f = Fixture::new();
    seed(&f);
    // A turn the watcher before durable ingress saved: no key, no receipt.
    let early = crate::event::MemoryEntry::new(
        "Correction",
        "это не то, цену ставим 42",
        crate::event::MemoryType::Feedback,
        crate::event::EventSource::ConversationWatcher,
    );
    f.storage.save(&early).unwrap();
    // The store is opened again (the next daemon start) while it still holds
    // that memory, then the human forgets it.
    f.storage = Arc::new(Storage::open(&f.root.join("memory.db")).unwrap());
    assert!(f.storage.forget_by_id(&early.id).unwrap());
    f.storage = Arc::new(Storage::open(&f.root.join("memory.db")).unwrap());

    let report = f.run(&["/code/demoapp"], None, false);
    assert!(report.floor.is_some(), "{report:?}");
    assert_eq!(report.turns, 0, "{report:?}");
    assert_eq!(report.before_capture, 3);
}

#[test]
fn a_fresh_store_that_only_ever_used_durable_ingress_has_no_floor() {
    let mut f = Fixture::new();
    seed(&f);
    f.run(&["/code/demoapp"], None, true);
    // Drain the queue the way the daemon does: every memory gets a receipt.
    for pending in f.storage.pending_ingest(Utc::now(), 1000).unwrap() {
        let entry = crate::event::MemoryEntry::new(
            "Correction",
            &pending.event.content,
            crate::event::MemoryType::Feedback,
            pending.event.source.clone(),
        );
        f.storage
            .finish_ingest(
                pending.seq,
                crate::ingest::ProcessingDecision::Save {
                    entry: &entry,
                    embedding: None,
                    enqueue_extraction: false,
                    link: None,
                },
                Utc::now(),
            )
            .unwrap();
    }
    f.storage = Arc::new(Storage::open(&f.root.join("memory.db")).unwrap());
    assert_eq!(f.run(&["/code/demoapp"], None, false).floor, None);
}

#[test]
fn a_turn_with_no_timestamp_is_left_out_not_dated_today() {
    let f = Fixture::new();
    seed(&f);
    let mut undated: serde_json::Value = serde_json::from_str(&claude(
        "/code/demoapp",
        "c9",
        "2026-07-03",
        "это не то, цену ставим 43",
    ))
    .unwrap();
    undated.as_object_mut().unwrap().remove("timestamp");
    write(
        &f.claude.join("-code-demoapp/undated.jsonl"),
        &[undated.to_string()],
    );
    // No cutoff at all: still not dated today.
    let report = f.run(&["/code/demoapp"], None, false);
    assert_eq!((report.turns, report.undated), (3, 1), "{report:?}");
    // With a floor it must not slip past it either.
    std::fs::write(f.root.join("watcher_offsets.json"), "{}").unwrap();
    let report = f.run(&["/code/demoapp"], None, false);
    assert_eq!((report.turns, report.undated), (0, 1), "{report:?}");
}

#[test]
fn archived_codex_sessions_are_read_in_a_profile_only_under_a_project_scope() {
    let f = Fixture::new();
    let archive = f.root.join("archived_sessions");
    std::fs::create_dir_all(&archive).unwrap();
    let archive_of = |roots: &[&str], in_profile| {
        TranscriptDirs::resolve(&f.config(roots), in_profile).codex_archive
    };
    assert_eq!(archive_of(&[], false), Some(archive.clone()));
    assert_eq!(archive_of(&["/code/demoapp"], true), Some(archive.clone()));
    // An unscoped profile names its own folder and gets nothing beside it.
    assert_eq!(archive_of(&[], true), None);
}

/// History refuses what live capture refuses: a Claude session a program
/// ran, a notice the app wrote into the user's role, a Codex session a
/// program ran. The replies are kept fixtures.
#[test]
fn capture_origin_history_import_refuses_what_capture_refuses() {
    let f = Fixture::new();
    let correction = "stop, use the other configuration for demoapp";
    let with = |line: String, fields: serde_json::Value| {
        let mut value: serde_json::Value = serde_json::from_str(&line).unwrap();
        for (key, field) in fields.as_object().unwrap() {
            value[key] = field.clone();
        }
        value.to_string()
    };
    write(
        &f.claude.join("-code-demoapp/mixed.jsonl"),
        &[
            with(
                claude("/code/demoapp", "o1", "2026-09-01", correction),
                serde_json::json!({"entrypoint": "sdk-py"}),
            ),
            with(
                claude("/code/demoapp", "o2", "2026-09-01", correction),
                serde_json::json!({"turnOrigin": "task_notification"}),
            ),
            with(
                claude("/code/demoapp", "o3", "2026-09-01", correction),
                serde_json::json!({"entrypoint": "claude-desktop", "turnOrigin": "human"}),
            ),
        ],
    );
    let meta = |payload: serde_json::Value| {
        serde_json::json!({"type": "session_meta", "payload": payload}).to_string()
    };
    write(
        &f.codex.join("2026/09/01/rollout-exec.jsonl"),
        &[
            meta(
                serde_json::json!({"cwd": "/code/demoapp", "originator": "codex_exec",
                "source": "exec"}),
            ),
            codex_user("x1", "2026-09-01", correction),
        ],
    );
    write(
        &f.codex.join("2026/09/01/rollout-desktop.jsonl"),
        &[
            meta(
                serde_json::json!({"cwd": "/code/demoapp", "originator": "Codex Desktop",
                "source": "vscode"}),
            ),
            codex_user("d1", "2026-09-01", correction),
        ],
    );
    f.run(&[], None, true);
    let queued: Vec<(crate::event::EventSource, String)> = f
        .queued()
        .into_iter()
        .map(|event| (event.source, event.content))
        .collect();
    assert_eq!(queued.len(), 2, "{queued:?}");
    assert!(queued.iter().all(|(_, content)| content == correction));
    assert!(
        queued
            .iter()
            .any(|(source, _)| *source == crate::event::EventSource::ConversationWatcher)
    );
    assert!(
        queued
            .iter()
            .any(|(source, _)| *source == crate::event::EventSource::CodexWatcher)
    );
}

/// The transcripts that were there before stretches were kept are read
/// whole: the owner's sessions give their stretches, a session a program
/// ran gives none, and reading them again changes nothing.
#[test]
fn work_stretch_earlier_transcripts_are_brought_in_once() {
    let f = Fixture::new();
    let record = |at: &str, entrypoint: &str| {
        serde_json::json!({"type": "assistant", "timestamp": at, "entrypoint": entrypoint,
            "cwd": "/code/demoapp", "message": {"content": "ok"}})
        .to_string()
    };
    write(
        &f.claude.join("-code-demoapp/long.jsonl"),
        &[
            record("2026-09-20T18:00:00Z", "claude-desktop"),
            record("2026-09-20T18:25:00Z", "claude-desktop"),
            record("2026-09-20T18:50:00Z", "claude-desktop"),
            record("2026-09-20T21:00:00Z", "claude-desktop"),
        ],
    );
    write(
        &f.claude.join("-code-demoapp/review.jsonl"),
        &[
            record("2026-09-20T18:00:00Z", "sdk-py"),
            record("2026-09-20T18:10:00Z", "sdk-py"),
        ],
    );
    let meta = |payload: serde_json::Value| {
        serde_json::json!({"type": "session_meta", "timestamp": "2026-09-21T09:00:00Z",
            "payload": payload})
        .to_string()
    };
    let codex = |at: &str| {
        serde_json::json!({"type": "response_item", "timestamp": at,
            "payload": {"type": "reasoning"}})
        .to_string()
    };
    write(
        &f.codex.join("2026/09/01/rollout-desktop.jsonl"),
        &[
            meta(
                serde_json::json!({"cwd": "/code/demoapp", "originator": "Codex Desktop",
                "source": "vscode"}),
            ),
            codex("2026-09-21T09:10:00Z"),
            codex("2026-09-21T09:20:00Z"),
        ],
    );
    write(
        &f.codex.join("2026/09/01/rollout-exec.jsonl"),
        &[
            meta(
                serde_json::json!({"cwd": "/code/demoapp", "originator": "codex_exec",
                "source": "exec"}),
            ),
            codex("2026-09-21T09:10:00Z"),
        ],
    );
    let stretches = |storage: &Storage| -> Vec<(String, String)> {
        let conn = storage.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT started_at, last_at FROM work_stretches ORDER BY started_at")
            .unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    };
    let expected = vec![
        (
            "2026-09-20 18:00:00".to_owned(),
            "2026-09-20 18:50:00".to_owned(),
        ),
        (
            "2026-09-20 21:00:00".to_owned(),
            "2026-09-20 21:00:00".to_owned(),
        ),
        (
            "2026-09-21 09:00:00".to_owned(),
            "2026-09-21 09:20:00".to_owned(),
        ),
    ];
    let (transcripts, count) = backfill_work_stretches(&f.storage, &f.config(&[])).unwrap();
    assert_eq!((transcripts, count), (4, 3));
    assert_eq!(stretches(&f.storage), expected);
    backfill_work_stretches(&f.storage, &f.config(&[])).unwrap();
    assert_eq!(stretches(&f.storage), expected);
    // A restricted scope keeps what is outside it out.
    let other = Fixture::new();
    write(
        &other.claude.join("-code-demoapp/long.jsonl"),
        &[record("2026-09-20T18:00:00Z", "claude-desktop")],
    );
    backfill_work_stretches(&other.storage, &other.config(&["/code/elsewhere"])).unwrap();
    assert!(stretches(&other.storage).is_empty());
}
