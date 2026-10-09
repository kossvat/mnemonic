//! The backfill over a fixture workspace and fixture transcripts. Every
//! name here is invented.
use super::*;
use serde_json::json;
use std::io::Write;
use std::path::{Path, PathBuf};

struct Fixture {
    dir: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let dir = crate::test_support::temp_dir("mn-backfill-");
        for folder in ["workspace/projects/demoapp", "workspace/projects/notes"] {
            std::fs::create_dir_all(dir.path().join(folder)).unwrap();
        }
        for folder in ["claude/-code", "codex/2026/09/30", "store"] {
            std::fs::create_dir_all(dir.path().join(folder)).unwrap();
        }
        let f = Self { dir };
        f.write_transcripts();
        f
    }

    fn path(&self, project: &str) -> String {
        let root = self.dir.path().join("workspace/projects");
        root.join(project).to_string_lossy().into_owned()
    }

    fn file(&self, relative: &str) -> PathBuf {
        self.dir.path().join(relative)
    }

    fn append(&self, relative: &str, lines: &[serde_json::Value]) {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.file(relative))
            .unwrap();
        for line in lines {
            writeln!(file, "{line}").unwrap();
        }
    }

    fn claude(
        &self,
        minute: u32,
        entrypoint: &str,
        project: &str,
        role: &str,
    ) -> serde_json::Value {
        let content = match role {
            "user" => json!("please look at the change"),
            _ => json!([{"type": "tool_use", "name": "Edit",
                "input": {"file_path": format!("{}/a.md", self.path("notes"))}}]),
        };
        json!({"type": role, "uuid": format!("u{minute}"), "sessionId": "s",
            "cwd": self.path(project), "entrypoint": entrypoint,
            "timestamp": format!("2026-09-30T10:{minute:02}:00Z"),
            "message": {"content": content}})
    }

    /// The owner's session, a session a program ran, and a rollout.
    fn write_transcripts(&self) {
        let owner: Vec<_> = [(0, "user"), (5, "assistant"), (15, "assistant")]
            .into_iter()
            .map(|(minute, role)| self.claude(minute, "cli", "demoapp", role))
            .collect();
        self.append("claude/-code/owner.jsonl", &owner);
        let program = [
            self.claude(0, "sdk-py", "notes", "user"),
            self.claude(10, "sdk-py", "notes", "assistant"),
        ];
        self.append("claude/-code/program.jsonl", &program);
        let codex = |minute: u32, kind: &str, payload: serde_json::Value| {
            json!({"type": kind, "timestamp": format!("2026-09-30T11:{minute:02}:00Z"),
                "payload": payload})
        };
        self.append(
            "codex/2026/09/30/rollout-owner.jsonl",
            &[
                codex(
                    0,
                    "session_meta",
                    json!({"cwd": self.path("demoapp"),
                    "originator": "a desktop client"}),
                ),
                codex(
                    1,
                    "response_item",
                    json!({"type": "message", "role": "user",
                    "content": [{"type": "input_text", "text": "look at the tests"}]}),
                ),
                codex(9, "event_msg", json!({"type": "task_complete"})),
            ],
        );
    }

    fn config(&self) -> Config {
        let mut config = Config::default();
        config.storage.db_path = self.file("store/memory.db");
        config.watchers.conversation_enabled = true;
        config.watchers.conversation_sessions_dir = Some(self.file("claude"));
        config.watchers.codex_enabled = true;
        config.watchers.codex_sessions_dir = Some(self.file("codex"));
        config.projects.workspace = Some(self.file("workspace"));
        config.projects.folder = Some(PathBuf::from("projects"));
        config
    }

    fn store(&self, name: &str) -> Arc<Storage> {
        let dir = self.file(name);
        std::fs::create_dir_all(&dir).unwrap();
        Arc::new(Storage::open(&dir.join("memory.db")).unwrap())
    }

    /// Make every transcript look as old as `secs`.
    fn age(&self, secs: u64) {
        let when = std::time::SystemTime::now() - std::time::Duration::from_secs(secs);
        for relative in [
            "claude/-code/owner.jsonl",
            "claude/-code/program.jsonl",
            "codex/2026/09/30/rollout-owner.jsonl",
        ] {
            let file = std::fs::File::options()
                .write(true)
                .open(self.file(relative))
                .unwrap();
            file.set_modified(when).unwrap();
        }
    }
}

/// What the pass keeps, without the ids of rows.
fn kept(storage: &Storage) -> Vec<String> {
    let conn = storage.conn.lock().unwrap();
    let mut rows = crate::test_support::rows(
        &conn,
        "SELECT 'segment', s.stream, s.driver, s.agent, p.slug, s.started_at, s.last_at,
                COALESCE(s.last_touch_at, '-'), s.source,
                (SELECT COALESCE(group_concat(minute || ':' || records || ':' || touches, ' '), '')
                   FROM (SELECT * FROM segment_activity a
                          WHERE a.segment_id = s.id ORDER BY minute))
           FROM work_segments s LEFT JOIN projects p ON p.id = s.project_id
          ORDER BY s.stream, s.started_at, s.id",
    );
    rows.extend(crate::test_support::rows(
        &conn,
        "SELECT 'prompt', stream, at FROM owner_prompts ORDER BY 1, 2, 3",
    ));
    rows.extend(crate::test_support::rows(
        &conn,
        "SELECT 'checkpoint', stream, generation, offset, driver, last_kind, last_at
           FROM observation_checkpoints ORDER BY stream",
    ));
    rows
}

/// What the live pass of a daemon writes for the transcripts of `f`.
fn live(f: &Fixture, storage: Arc<Storage>) -> usize {
    let config = f.config();
    let (map, _) = crate::projects::sync(&storage, &config.projects)
        .unwrap()
        .unwrap();
    let claude = ConversationWatcher::new(f.file("claude")).find_jsonl_files();
    let codex = CodexWatcher::new(f.file("codex")).find_rollout_files();
    let idle = config.sessions.idle_timeout_secs;
    Observation::new(storage.clone(), observe::CLAUDE, idle)
        .tick(&claude, &map, false)
        .unwrap()
        + Observation::new(storage, observe::CODEX, idle)
            .tick(&codex, &map, false)
            .unwrap()
}

#[test]
fn backfill_gives_the_rows_the_live_pass_gives_and_a_second_run_gives_the_same() {
    let f = Fixture::new();
    let by_live = f.store("live");
    assert_eq!(live(&f, by_live.clone()), 3);

    let by_backfill = f.store("store");
    let first = run(&f.config(), by_backfill.clone()).unwrap();
    assert_eq!(
        first,
        Backfilled {
            projects: 2,
            transcripts: 3,
            written: 3
        }
    );
    let rows = kept(&by_backfill);
    assert_eq!(rows, kept(&by_live));
    // The owner's session on two projects, the program's apart, the
    // rollout, the owner's two prompts, three checkpoints.
    assert_eq!(
        rows.iter().filter(|row| row.starts_with("segment")).count(),
        4
    );
    assert_eq!(
        rows.iter().filter(|row| row.starts_with("prompt")).count(),
        2
    );

    let second = run(&f.config(), by_backfill.clone()).unwrap();
    assert_eq!((second.transcripts, second.written), (3, 0));
    assert_eq!(kept(&by_backfill), rows);
}

#[test]
fn backfill_reads_the_history_the_live_pass_leaves() {
    let f = Fixture::new();
    // Nobody wrote to these transcripts for two hours.
    f.age(7200);
    let storage = f.store("store");
    assert_eq!(live(&f, storage.clone()), 0);
    assert!(kept(&storage).is_empty());
    assert_eq!(run(&f.config(), storage.clone()).unwrap().written, 3);
    // What it wrote is what a live pass writes of the same records.
    let fresh = Fixture::new();
    let by_live = fresh.store("live");
    live(&fresh, by_live.clone());
    assert_eq!(kept(&storage), kept(&by_live));
    // The days it wrote are marked for the split of the owner's time.
    assert_eq!(storage.attribution_dirty().unwrap().len(), 1);
}

#[test]
fn backfill_that_was_interrupted_goes_on_where_it_stopped() {
    let f = Fixture::new();
    let storage = f.store("store");
    // The store refuses the segments of one transcript: the others are
    // written, and that one is left for the next run.
    let block = |on: bool| {
        let sql = if on {
            "CREATE TRIGGER test_block BEFORE INSERT ON work_segments
             WHEN NEW.agent = 'codex'
             BEGIN SELECT RAISE(ABORT, 'injected store failure'); END;"
        } else {
            "DROP TRIGGER test_block"
        };
        storage.conn.lock().unwrap().execute_batch(sql).unwrap();
    };
    block(true);
    assert_eq!(run(&f.config(), storage.clone()).unwrap().written, 2);
    block(false);
    // New records came meanwhile to a transcript that was read.
    f.append(
        "claude/-code/owner.jsonl",
        &[f.claude(20, "cli", "demoapp", "assistant")],
    );
    let next = run(&f.config(), storage.clone()).unwrap();
    assert_eq!((next.transcripts, next.written), (3, 2));

    let whole = Fixture::new();
    whole.append(
        "claude/-code/owner.jsonl",
        &[whole.claude(20, "cli", "demoapp", "assistant")],
    );
    let at_once = whole.store("store");
    run(&whole.config(), at_once.clone()).unwrap();
    assert_eq!(kept(&storage), kept(&at_once));
}

#[test]
fn backfill_while_the_live_pass_runs_writes_no_record_twice() {
    let f = Fixture::new();
    let storage = f.store("store");
    let config = f.config();
    let (map, _) = crate::projects::sync(&storage, &config.projects)
        .unwrap()
        .unwrap();
    let files = ConversationWatcher::new(f.file("claude")).find_jsonl_files();
    // The live pass has read a transcript when the backfill takes all of
    // them and writes first: the live pass finds its checkpoint moved.
    let pass = Observation::new(storage.clone(), observe::CLAUDE, 1800);
    let (theirs, their_config) = (storage.clone(), config.clone());
    let mut ran = false;
    *pass.before_write.lock().unwrap() = Some(Box::new(move |_: &Path| {
        if !std::mem::replace(&mut ran, true) {
            assert_eq!(run(&their_config, theirs.clone()).unwrap().written, 3);
        }
    }));
    pass.tick(&files, &map, false).unwrap();

    let control = Fixture::new();
    let alone = control.store("store");
    run(&control.config(), alone.clone()).unwrap();
    assert_eq!(kept(&storage), kept(&alone));
}

#[test]
fn backfill_needs_a_workspace() {
    let f = Fixture::new();
    let mut config = f.config();
    config.projects = Default::default();
    let storage = f.store("store");
    assert!(run(&config, storage.clone()).is_err());
    assert!(kept(&storage).is_empty());
}

#[test]
fn backfill_reads_the_archive_when_the_sessions_folder_is_gone() {
    let f = Fixture::new();
    // The rollout was archived, and the folder of sessions removed.
    let archive = f.file("archived_sessions");
    std::fs::create_dir_all(&archive).unwrap();
    std::fs::rename(
        f.file("codex/2026/09/30/rollout-owner.jsonl"),
        archive.join("rollout-owner.jsonl"),
    )
    .unwrap();
    std::fs::remove_dir_all(f.file("codex")).unwrap();
    let dirs = TranscriptDirs {
        conversation: Some(f.file("claude")),
        codex: Some(f.file("codex")),
        codex_archive: Some(archive),
    };
    let storage = f.store("store");
    let read = run_in(&f.config(), storage.clone(), dirs).unwrap();
    assert_eq!((read.transcripts, read.written), (3, 3));
    assert!(kept(&storage).iter().any(|row| row.contains("|codex|")));
    // Neither folder: the transcripts of the other format are read.
    let dirs = TranscriptDirs {
        conversation: Some(f.file("claude")),
        codex: Some(f.file("codex")),
        codex_archive: Some(f.file("nowhere")),
    };
    let read = run_in(&f.config(), f.store("other"), dirs).unwrap();
    assert_eq!(read.transcripts, 2);
}
