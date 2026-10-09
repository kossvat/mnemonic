//! The dry run over a fixture workspace and fixture transcripts. Every
//! name here is invented.
use super::*;
use crate::storage::Storage;
use serde_json::json;
use std::collections::BTreeMap as Map;
use std::io::Write;
use std::path::PathBuf;

const SAID: &str = "stop, use the other configuration for demoapp";

struct Fixture {
    dir: PathBuf,
    projects: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("mn-replay-{}", uuid::Uuid::new_v4().simple()));
        let projects = dir.join("workspace/projects");
        for folder in ["demoapp", "notes"] {
            std::fs::create_dir_all(projects.join(folder)).unwrap();
        }
        std::fs::create_dir_all(dir.join("claude/-code")).unwrap();
        std::fs::create_dir_all(dir.join("codex/2026/09/30")).unwrap();
        std::fs::create_dir_all(dir.join("store")).unwrap();
        let fixture = Self { dir, projects };
        fixture.write_transcripts();
        fixture
    }

    fn path(&self, project: &str) -> String {
        self.projects.join(project).to_string_lossy().into_owned()
    }

    fn write(&self, relative: &str, lines: &[serde_json::Value]) {
        let mut file = std::fs::File::create(self.dir.join(relative)).unwrap();
        for line in lines {
            writeln!(file, "{line}").unwrap();
        }
    }

    fn write_transcripts(&self) {
        let at = |minute: u32| format!("2026-09-30T10:{minute:02}:00Z");
        let claude =
            |minute: u32, entrypoint: &str, cwd: &str, role: &str, content: serde_json::Value| {
                json!({"type": role, "uuid": format!("u{minute}"), "sessionId": "s", "cwd": cwd,
                "entrypoint": entrypoint, "timestamp": at(minute), "message": {"content": content}})
            };
        let edit = json!([{"type": "tool_use", "name": "Edit",
            "input": {"file_path": format!("{}/a.md", self.path("notes"))}}]);
        // The owner: launched in one project, then a write into another.
        self.write(
            "claude/-code/owner.jsonl",
            &[
                claude(0, "cli", &self.path("demoapp"), "user", json!(SAID)),
                claude(5, "cli", &self.path("demoapp"), "assistant", edit),
                claude(
                    15,
                    "cli",
                    &self.path("demoapp"),
                    "assistant",
                    json!([{"type": "text", "text": "done"}]),
                ),
            ],
        );
        // A session a program ran.
        self.write(
            "claude/-code/program.jsonl",
            &[
                claude(
                    0,
                    "sdk-py",
                    &self.path("notes"),
                    "user",
                    json!("review the change"),
                ),
                claude(
                    10,
                    "sdk-py",
                    &self.path("notes"),
                    "assistant",
                    json!([{"type": "text", "text": "ok"}]),
                ),
            ],
        );
        // A rollout launched outside every project: two calls of the shell
        // name one, and its unknown head takes that project.
        let codex = |minute: u32, kind: &str, payload: serde_json::Value| json!({"type": kind, "timestamp": format!("2026-09-30T11:{minute:02}:00Z"), "payload": payload});
        let exec = json!({"type": "custom_tool_call", "name": "exec",
            "input": format!("ls {}", self.path("demoapp"))});
        self.write(
            "codex/2026/09/30/rollout-owner.jsonl",
            &[
                codex(
                    0,
                    "session_meta",
                    json!({"cwd": self.dir.join("workspace"), "originator": "a desktop client"}),
                ),
                codex(1, "response_item", exec.clone()),
                codex(2, "response_item", exec),
                codex(10, "event_msg", json!({"type": "task_complete"})),
            ],
        );
    }

    fn config(&self) -> Config {
        let mut config = Config::default();
        config.storage.db_path = self.dir.join("store/memory.db");
        config.watchers.conversation_enabled = true;
        config.watchers.conversation_sessions_dir = Some(self.dir.join("claude"));
        config.watchers.codex_enabled = true;
        config.watchers.codex_sessions_dir = Some(self.dir.join("codex"));
        config.projects.workspace = Some(self.dir.join("workspace"));
        config.projects.folder = Some(PathBuf::from("projects"));
        config
    }

    fn run(&self) -> Report {
        let now = DateTime::parse_from_rfc3339("2026-09-30T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        dry_run(
            &self.config(),
            &Options {
                days: 7,
                sample: 20,
            },
            now,
        )
        .unwrap()
    }

    /// Every file beside the store, by name, with its bytes.
    fn store_files(&self) -> Map<String, Vec<u8>> {
        std::fs::read_dir(self.dir.join("store"))
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                (
                    entry.file_name().to_string_lossy().into_owned(),
                    std::fs::read(entry.path()).unwrap(),
                )
            })
            .collect()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn minutes(report: &Report, driver: Driver, project: Option<&str>) -> i64 {
    report
        .seconds
        .get(&(driver, project.map(str::to_owned)))
        .copied()
        .unwrap_or(0)
        / 60
}

#[test]
fn replay_the_dry_run_gives_hours_by_project_and_how_each_was_learned() {
    let f = Fixture::new();
    let report = f.run();
    assert_eq!(
        (report.projects, report.refused, report.transcripts),
        (2, 0, 3)
    );
    assert_eq!(minutes(&report, Driver::Owner, Some("demoapp")), 5 + 2 + 8);
    assert_eq!(minutes(&report, Driver::Owner, Some("notes")), 10);
    assert_eq!(minutes(&report, Driver::Owner, None), 0);
    // A session a program ran is counted apart, never as the owner's.
    assert_eq!(minutes(&report, Driver::Program, Some("notes")), 10);
    assert_eq!(minutes(&report, Driver::Program, Some("demoapp")), 0);

    let learned: Map<&str, i64> = report
        .learned
        .iter()
        .map(|(name, s)| (*name, s / 60))
        .collect();
    assert_eq!(
        learned,
        Map::from([("launch", 5), ("path", 18), ("backfill", 2)])
    );
    assert_eq!(report.counts.naming, 3);
    assert_eq!(report.counts.ambiguous, 0);
    assert_eq!(report.counts.switches, 4);
    assert_eq!((report.on_several, report.owner_transcripts), (1, 2));

    // The sample holds the owner's segments of a minute or more, as times.
    assert_eq!(report.sample.len(), 4);
    assert!(
        report
            .sample
            .iter()
            .all(|s| s.driver == Driver::Owner && s.seconds >= 60)
    );
    assert!(
        report
            .sample
            .windows(2)
            .all(|pair| pair[0].started_at <= pair[1].started_at)
    );
}

#[test]
fn replay_time_before_the_window_is_left_out() {
    let f = Fixture::new();
    let now = DateTime::parse_from_rfc3339("2026-10-20T12:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let report = dry_run(
        &f.config(),
        &Options {
            days: 7,
            sample: 20,
        },
        now,
    )
    .unwrap();
    assert_eq!(report.transcripts, 3);
    assert!(report.seconds.is_empty());
    assert!(report.sample.is_empty());
}

#[test]
fn replay_prints_aggregates_and_no_text_of_a_record() {
    let f = Fixture::new();
    let printed = f.run().to_string();
    assert!(printed.contains("nothing was written"));
    assert!(printed.contains("demoapp"));
    assert!(printed.contains("4 segments to label"));
    for text in [SAID, "review the change", "a.md", "task_complete"] {
        assert!(!printed.contains(text), "a text of a record was printed");
    }
    // Nor where anything lies.
    let root = f.dir.to_string_lossy().to_string();
    assert!(!printed.contains(&root));
    assert!(!printed.contains("workspace/"));
}

#[test]
fn replay_leaves_the_store_byte_for_byte_as_it_was() {
    let f = Fixture::new();
    // A store with rows in its ingress tables and its recall log, closed.
    {
        let storage = Storage::open(&f.config().storage.db_path).unwrap();
        storage
            .conn
            .lock()
            .unwrap()
            .execute_batch(
                "INSERT INTO work_stretches (transcript, started_at, last_at)
                 VALUES ('t', '2026-09-30 10:00:00', '2026-09-30 10:05:00');",
            )
            .unwrap();
        crate::projects::sync(&storage, &f.config().projects).unwrap();
    }
    let before = f.store_files();
    assert!(before.contains_key("memory.db"));
    let report = f.run();
    assert_eq!(report.transcripts, 3);
    let after = f.store_files();
    assert_eq!(
        after.keys().collect::<Vec<_>>(),
        before.keys().collect::<Vec<_>>(),
        "the dry run made or removed a file beside the store"
    );
    assert!(after == before, "a file of the store changed");

    // A store in use: what it holds is as it was, the journal included.
    let storage = Storage::open(&f.config().storage.db_path).unwrap();
    storage
        .conn
        .lock()
        .unwrap()
        .execute_batch(
            "INSERT INTO work_stretches (transcript, started_at, last_at)
             VALUES ('u', '2026-09-30 11:00:00', '2026-09-30 11:05:00');",
        )
        .unwrap();
    let before = f.store_files();
    f.run();
    let after = f.store_files();
    assert_eq!(
        after.keys().collect::<Vec<_>>(),
        before.keys().collect::<Vec<_>>()
    );
    for name in ["memory.db", "memory.db-wal"] {
        if before.contains_key(name) {
            assert!(after[name] == before[name], "{name} changed");
        }
    }
    drop(storage);
}

#[test]
fn replay_sums_a_merged_project_under_the_one_it_was_merged_into() {
    let f = Fixture::new();
    let apart = f.run();
    {
        let storage = Storage::open(&f.config().storage.db_path).unwrap();
        crate::projects::sync(&storage, &f.config().projects).unwrap();
        let id = |key: &str| storage.project_by_key(key).unwrap().unwrap().id;
        storage.project_merge(id("notes"), id("demoapp")).unwrap();
    }
    let report = f.run();
    assert_eq!(minutes(&report, Driver::Owner, Some("demoapp")), 25);
    assert_eq!(minutes(&report, Driver::Owner, Some("notes")), 0);
    // The two are one project to the rules, before a record is resolved:
    // the write into the merged folder is no move to another project.
    assert_eq!((apart.on_several, report.on_several), (1, 0));
    assert_eq!(report.counts.switches, apart.counts.switches - 1);
}

#[test]
fn replay_who_drives_a_session_is_what_its_first_record_that_says_so_says() {
    let f = Fixture::new();
    let at = |minute: u32| format!("2026-09-30T12:{minute:02}:00Z");
    let record = |minute: u32, entrypoint: Option<&str>| {
        let mut record = json!({"type": "assistant", "sessionId": "p", "cwd": f.path("notes"),
            "timestamp": at(minute), "message": {"content": [{"type": "text", "text": "ok"}]}});
        if let Some(entrypoint) = entrypoint {
            record["entrypoint"] = json!(entrypoint);
        }
        record
    };
    // Metadata with no time and no word of who drives comes first.
    f.write(
        "claude/-code/late.jsonl",
        &[
            json!({"type": "summary", "summary": "a title"}),
            record(0, None),
            record(4, Some("sdk-py")),
            record(9, Some("sdk-py")),
        ],
    );
    // What a later record says does not change who drove the session.
    f.write(
        "claude/-code/first.jsonl",
        &[record(20, Some("cli")), record(27, Some("sdk-py"))],
    );
    let codex = |minute: u32, kind: &str, payload: serde_json::Value| json!({"type": kind, "timestamp": at(minute), "payload": payload});
    f.write(
        "codex/2026/09/30/rollout-program.jsonl",
        &[
            codex(
                30,
                "session_meta",
                json!({"cwd": f.path("demoapp"), "originator": "codex_exec", "source": "exec"}),
            ),
            codex(33, "event_msg", json!({"type": "task_complete"})),
        ],
    );
    let now = DateTime::parse_from_rfc3339("2026-09-30T13:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let options = Options {
        days: 7,
        sample: 20,
    };
    let report = dry_run(&f.config(), &options, now).unwrap();
    assert_eq!(minutes(&report, Driver::Program, Some("notes")), 10 + 9);
    assert_eq!(minutes(&report, Driver::Owner, Some("notes")), 10 + 7);
    assert_eq!(minutes(&report, Driver::Program, Some("demoapp")), 3);
}

#[test]
fn replay_a_former_name_a_folder_still_carries_names_that_folder() {
    let f = Fixture::new();
    let mut config = f.config();
    config
        .projects
        .former
        .insert("notes".into(), "demoapp".into());
    let now = DateTime::parse_from_rfc3339("2026-09-30T12:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let options = Options {
        days: 7,
        sample: 20,
    };
    let report = dry_run(&config, &options, now).unwrap();
    assert_eq!(minutes(&report, Driver::Owner, Some("notes")), 10);
    assert_eq!(minutes(&report, Driver::Owner, Some("demoapp")), 15);
}

#[test]
fn replay_a_former_name_of_a_project_inside_another_names_it() {
    let f = Fixture::new();
    let tools = f.projects.join("demoapp/tools");
    std::fs::create_dir_all(&tools).unwrap();
    std::fs::write(tools.join("Cargo.toml"), "").unwrap();
    let mut config = f.config();
    config
        .projects
        .former
        .insert("oldtools".into(), "demoapp:tools".into());
    let record = |minute: u32| {
        json!({"type": "assistant", "sessionId": "o", "cwd": f.path("oldtools"),
            "entrypoint": "cli", "timestamp": format!("2026-09-30T09:{minute:02}:00Z"),
            "message": {"content": [{"type": "text", "text": "ok"}]}})
    };
    f.write("claude/-code/old.jsonl", &[record(0), record(6)]);
    let now = DateTime::parse_from_rfc3339("2026-09-30T12:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let options = Options {
        days: 7,
        sample: 20,
    };
    let report = dry_run(&config, &options, now).unwrap();
    assert_eq!(report.projects, 3);
    assert_eq!(minutes(&report, Driver::Owner, Some("demoapp:tools")), 6);
    assert_eq!(minutes(&report, Driver::Owner, None), 0);
}

#[test]
fn replay_a_segment_that_began_before_the_window_counts_from_its_start() {
    let f = Fixture::new();
    // The window opens at 10:10 of the day of the fixture: the owner's
    // write into `notes` lasted from 10:05 to 10:15.
    let now = DateTime::parse_from_rfc3339("2026-10-07T10:10:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let options = Options {
        days: 7,
        sample: 20,
    };
    let report = dry_run(&f.config(), &options, now).unwrap();
    assert_eq!(minutes(&report, Driver::Owner, Some("notes")), 5);
    // Before the window: the five minutes in the folder of the launch.
    // What is left to `demoapp` is the rollout of an hour later.
    assert_eq!(minutes(&report, Driver::Owner, Some("demoapp")), 10);
    let since = now - Duration::days(7);
    assert!(report.sample.iter().all(|shown| shown.started_at >= since));
    assert!(
        report
            .sample
            .iter()
            .any(|shown| shown.started_at == since && shown.seconds == 300)
    );
}

#[test]
fn replay_needs_a_workspace_and_no_store() {
    let f = Fixture::new();
    let now = Utc::now();
    let options = Options { days: 7, sample: 0 };
    // No store at all: the workspace is enough.
    assert!(!f.config().storage.db_path.exists());
    assert!(dry_run(&f.config(), &options, now).is_ok());
    assert!(
        !f.config().storage.db_path.exists(),
        "the dry run made a store"
    );
    let mut config = f.config();
    config.projects = Default::default();
    assert!(dry_run(&config, &options, now).is_err());
}

#[test]
fn replay_who_drives_a_rollout_is_read_from_a_record_the_scope_leaves_out() {
    let f = Fixture::new();
    let mut config = f.config();
    config.watchers.project_roots = vec![f.projects.join("demoapp")];
    let now = DateTime::parse_from_rfc3339("2026-09-30T13:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let options = Options {
        days: 7,
        sample: 20,
    };
    let before = dry_run(&config, &options, now).unwrap();
    assert_eq!(minutes(&before, Driver::Program, Some("demoapp")), 0);

    // A program opens a rollout outside the scope, and a later turn moves
    // it into the scope: the record that says a program ran it is one the
    // scope leaves out.
    let codex = |minute: u32, kind: &str, payload: serde_json::Value| {
        json!({"type": kind, "timestamp": format!("2026-09-30T12:{minute:02}:00Z"),
            "payload": payload})
    };
    f.write(
        "codex/2026/09/30/rollout-moved.jsonl",
        &[
            codex(
                0,
                "session_meta",
                json!({"cwd": f.dir.join("workspace"), "originator": "codex_exec",
                    "source": "exec"}),
            ),
            codex(1, "turn_context", json!({"cwd": f.path("demoapp")})),
            codex(
                2,
                "response_item",
                json!({"type": "custom_tool_call", "name": "exec",
                    "input": format!("ls {}", f.path("demoapp"))}),
            ),
            codex(9, "event_msg", json!({"type": "task_complete"})),
        ],
    );
    let after = dry_run(&config, &options, now).unwrap();
    // What the scope admits of it is a program's time, not the owner's.
    assert!(minutes(&after, Driver::Program, Some("demoapp")) > 0);
    assert_eq!(
        minutes(&after, Driver::Owner, Some("demoapp")),
        minutes(&before, Driver::Owner, Some("demoapp"))
    );
}

#[test]
fn replay_a_project_merged_into_one_whose_folder_is_gone_is_shown_under_it() {
    let f = Fixture::new();
    std::fs::create_dir_all(f.projects.join("kept")).unwrap();
    {
        let storage = Storage::open(&f.config().storage.db_path).unwrap();
        crate::projects::sync(&storage, &f.config().projects).unwrap();
        let id = |key: &str| storage.project_by_key(key).unwrap().unwrap().id;
        storage.project_merge(id("notes"), id("kept")).unwrap();
    }
    std::fs::remove_dir(f.projects.join("kept")).unwrap();
    let report = f.run();
    assert_eq!(minutes(&report, Driver::Owner, Some("kept")), 10);
    assert_eq!(minutes(&report, Driver::Owner, Some("notes")), 0);
    assert_eq!(minutes(&report, Driver::Program, Some("kept")), 10);
}
